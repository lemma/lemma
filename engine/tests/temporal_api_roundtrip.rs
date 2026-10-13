//! Display/FromStr round-trips for temporal API types.
//!
//! `PartialEq` / `Ord` / `Hash` for [`DateTimeValue`] ignore `granularity`, so every
//! case asserts the granularity discriminant explicitly.
//!
//! Semantic midnight without timezone canonicalizes to the date form — same value.

use chrono::{Datelike, NaiveDate, Weekday};
use lemma::__test_support::{SemanticDateTime, SemanticTime, SemanticTimezone, TimeValue};
use lemma::{DateGranularity, DateTimeValue, TimezoneValue};
use std::str::FromStr;

fn assert_granularity(dt: &DateTimeValue, expected: DateGranularity) {
    assert_eq!(
        std::mem::discriminant(&dt.granularity),
        std::mem::discriminant(&expected),
        "granularity discriminant: got {:?}, expected {:?}",
        dt.granularity,
        expected
    );
    match (&dt.granularity, &expected) {
        (
            DateGranularity::IsoWeek {
                iso_year: a_y,
                week: a_w,
            },
            DateGranularity::IsoWeek {
                iso_year: e_y,
                week: e_w,
            },
        ) => {
            assert_eq!(a_y, e_y, "IsoWeek iso_year");
            assert_eq!(a_w, e_w, "IsoWeek week");
        }
        (DateGranularity::Year, DateGranularity::Year)
        | (DateGranularity::YearMonth, DateGranularity::YearMonth)
        | (DateGranularity::Full, DateGranularity::Full)
        | (DateGranularity::DateTime, DateGranularity::DateTime) => {}
        _ => panic!(
            "granularity variant mismatch: got {:?}, expected {:?}",
            dt.granularity, expected
        ),
    }
}

fn roundtrip_datetime(input: &str, expected_granularity: DateGranularity) -> DateTimeValue {
    let parsed = DateTimeValue::from_str(input)
        .unwrap_or_else(|e| panic!("parse {input:?} must succeed: {e}"));
    assert_granularity(&parsed, expected_granularity);
    let displayed = parsed.to_string();
    assert_eq!(
        displayed, input,
        "Display must emit the canonical input form"
    );
    let again = DateTimeValue::from_str(&displayed)
        .unwrap_or_else(|e| panic!("re-parse {displayed:?} must succeed: {e}"));
    assert_granularity(&again, expected_granularity);
    assert_eq!(again.year, parsed.year);
    assert_eq!(again.month, parsed.month);
    assert_eq!(again.day, parsed.day);
    assert_eq!(again.hour, parsed.hour);
    assert_eq!(again.minute, parsed.minute);
    assert_eq!(again.second, parsed.second);
    assert_eq!(again.microsecond, parsed.microsecond);
    assert_eq!(again.timezone, parsed.timezone);
    parsed
}

fn assert_iso_week_calendar_agrees(dt: &DateTimeValue, iso_year: i32, week: u32) {
    let DateGranularity::IsoWeek {
        iso_year: g_y,
        week: g_w,
    } = dt.granularity
    else {
        panic!("expected IsoWeek granularity, got {:?}", dt.granularity);
    };
    assert_eq!(g_y, iso_year);
    assert_eq!(g_w, week);
    let recomputed = NaiveDate::from_isoywd_opt(iso_year, week, Weekday::Mon)
        .unwrap_or_else(|| panic!("from_isoywd_opt({iso_year}, {week})"));
    assert_eq!(dt.year, recomputed.year(), "calendar year vs recomputed");
    assert_eq!(dt.month, recomputed.month(), "calendar month vs recomputed");
    assert_eq!(dt.day, recomputed.day(), "calendar day vs recomputed");
}

fn assert_serde_string_roundtrip<T>(value: &T, expected: &str)
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let json = serde_json::to_value(value).expect("serialize");
    assert_eq!(json, serde_json::Value::String(expected.to_string()));
    let restored: T = serde_json::from_value(json).expect("deserialize");
    assert_eq!(&restored, value);
}

#[test]
fn datetime_year_granularity_roundtrip() {
    let dt = roundtrip_datetime("2025", DateGranularity::Year);
    assert_eq!(dt.year, 2025);
    assert_eq!(dt.month, 1);
    assert_eq!(dt.day, 1);
}

#[test]
fn datetime_year_month_granularity_roundtrip() {
    let dt = roundtrip_datetime("2025-03", DateGranularity::YearMonth);
    assert_eq!(dt.year, 2025);
    assert_eq!(dt.month, 3);
    assert_eq!(dt.day, 1);
}

#[test]
fn datetime_full_granularity_roundtrip() {
    let dt = roundtrip_datetime("2025-03-04", DateGranularity::Full);
    assert_eq!(dt.year, 2025);
    assert_eq!(dt.month, 3);
    assert_eq!(dt.day, 4);
}

#[test]
fn datetime_datetime_granularity_roundtrip() {
    let dt = roundtrip_datetime("2025-03-04T12:30:45", DateGranularity::DateTime);
    assert_eq!(dt.hour, 12);
    assert_eq!(dt.minute, 30);
    assert_eq!(dt.second, 45);
    assert_eq!(dt.microsecond, 0);
    assert!(dt.timezone.is_none());
}

#[test]
fn datetime_microseconds_roundtrip() {
    let dt = roundtrip_datetime("2025-03-04T12:30:45.123456", DateGranularity::DateTime);
    assert_eq!(dt.microsecond, 123456);
}

#[test]
fn datetime_microseconds_trailing_nonzero_zeros_roundtrip() {
    let dt = roundtrip_datetime("2025-03-04T12:30:45.100000", DateGranularity::DateTime);
    assert_eq!(dt.microsecond, 100000);
}

#[test]
fn datetime_iso_week_2026_w01_calendar_year_differs() {
    let dt = roundtrip_datetime(
        "2026-W01",
        DateGranularity::IsoWeek {
            iso_year: 2026,
            week: 1,
        },
    );
    assert_iso_week_calendar_agrees(&dt, 2026, 1);
    assert_eq!(dt.year, 2025);
    assert_eq!(dt.month, 12);
    assert_eq!(dt.day, 29);
}

#[test]
fn datetime_iso_week_2019_w01_begins_previous_year() {
    let dt = roundtrip_datetime(
        "2019-W01",
        DateGranularity::IsoWeek {
            iso_year: 2019,
            week: 1,
        },
    );
    assert_iso_week_calendar_agrees(&dt, 2019, 1);
    assert_eq!(dt.year, 2018);
    assert_eq!(dt.month, 12);
    assert_eq!(dt.day, 31);
}

#[test]
fn datetime_iso_week_53_week_years() {
    for (s, iso_year, week) in [("2020-W53", 2020, 53), ("2015-W53", 2015, 53)] {
        let dt = roundtrip_datetime(s, DateGranularity::IsoWeek { iso_year, week });
        assert_iso_week_calendar_agrees(&dt, iso_year, week);
    }
}

#[test]
fn datetime_offset_z() {
    let dt = roundtrip_datetime("2025-03-04T12:30:45Z", DateGranularity::DateTime);
    let tz = dt.timezone.expect("Z timezone");
    assert_eq!(tz.offset_minutes, 0);
}

#[test]
fn datetime_offset_plus_two_hours() {
    let dt = roundtrip_datetime("2025-03-04T12:30:45+02:00", DateGranularity::DateTime);
    let tz = dt.timezone.expect("+02:00");
    assert_eq!(tz.offset_minutes, 120);
}

#[test]
fn datetime_offset_minus_five_hours() {
    let dt = roundtrip_datetime("2025-03-04T12:30:45-05:00", DateGranularity::DateTime);
    let tz = dt.timezone.expect("-05:00");
    assert_eq!(tz.offset_minutes, -300);
}

#[test]
fn datetime_offset_non_hour_plus() {
    let dt = roundtrip_datetime("2025-03-04T12:30:45+05:45", DateGranularity::DateTime);
    let tz = dt.timezone.expect("+05:45");
    assert_eq!(tz.offset_minutes, 345);
}

#[test]
fn datetime_offset_non_hour_minus() {
    let dt = roundtrip_datetime("2025-03-04T12:30:45-03:30", DateGranularity::DateTime);
    let tz = dt.timezone.expect("-03:30");
    assert_eq!(tz.offset_minutes, -210);
}

#[test]
fn datetime_offset_across_date_boundary() {
    let dt = roundtrip_datetime("2025-01-01T01:00:00+10:00", DateGranularity::DateTime);
    assert_eq!(dt.year, 2025);
    assert_eq!(dt.month, 1);
    assert_eq!(dt.day, 1);
    let tz = dt.timezone.expect("timezone");
    assert_eq!(tz.offset_minutes, 600);
}

#[test]
fn datetime_leap_day_2024() {
    let dt = roundtrip_datetime("2024-02-29", DateGranularity::Full);
    assert_eq!(dt.day, 29);
}

#[test]
fn datetime_non_leap_feb_28_2023() {
    let dt = roundtrip_datetime("2023-02-28", DateGranularity::Full);
    assert_eq!(dt.day, 28);
}

#[test]
fn datetime_end_of_year_with_max_microseconds() {
    let dt = roundtrip_datetime("2025-12-31T23:59:59.999999", DateGranularity::DateTime);
    assert_eq!(dt.microsecond, 999999);
}

#[test]
fn datetime_start_of_year() {
    let dt = roundtrip_datetime("2025-01-01T00:00:00", DateGranularity::DateTime);
    assert_eq!(dt.hour, 0);
    assert_eq!(dt.minute, 0);
    assert_eq!(dt.second, 0);
}

#[test]
fn datetime_malformed_returns_err_never_panics() {
    let cases = [
        "",
        "   ",
        "2025-13",
        "2025-00",
        "2025-02-30",
        "2023-02-29",
        "2025-W54",
        "2025-W00",
        "2025-03-04T25:00",
        "2025-03-04T12:60",
        "2025-03-04T12:30:45+99:00",
        "not-a-date",
        "2025-03-04xyz",
    ];
    for input in cases {
        let parsed = DateTimeValue::from_str(input);
        assert!(
            parsed.is_err(),
            "FromStr({input:?}) must return Err, got {parsed:?}"
        );
    }
}

#[test]
fn datetime_serde_is_iso_string() {
    let dt = DateTimeValue::from_str("2025-03-04T12:30:45+02:00").expect("parse");
    assert_serde_string_roundtrip(&dt, "2025-03-04T12:30:45+02:00");
}

fn roundtrip_time(input: &str) -> TimeValue {
    let parsed =
        TimeValue::from_str(input).unwrap_or_else(|e| panic!("parse {input:?} must succeed: {e}"));
    let displayed = parsed.to_string();
    assert_eq!(displayed, input, "TimeValue Display must match input");
    let again = TimeValue::from_str(&displayed)
        .unwrap_or_else(|e| panic!("re-parse {displayed:?} must succeed: {e}"));
    assert_eq!(again, parsed);
    parsed
}

#[test]
fn time_midnight_roundtrip() {
    let t = roundtrip_time("00:00:00");
    assert_eq!(t.hour, 0);
    assert_eq!(t.minute, 0);
    assert_eq!(t.second, 0);
}

#[test]
fn time_end_of_day_with_max_microseconds() {
    let t = roundtrip_time("23:59:59.999999");
    assert_eq!(t.microsecond, 999999);
}

#[test]
fn time_offset_z() {
    let t = roundtrip_time("12:30:45Z");
    let tz = t.timezone.expect("Z");
    assert_eq!(tz.offset_minutes, 0);
}

#[test]
fn time_offset_plus_and_minus() {
    let plus = roundtrip_time("12:30:45+02:00");
    assert_eq!(plus.timezone.as_ref().map(|z| z.offset_minutes), Some(120));
    let minus = roundtrip_time("12:30:45-05:00");
    assert_eq!(
        minus.timezone.as_ref().map(|z| z.offset_minutes),
        Some(-300)
    );
}

#[test]
fn time_offset_non_hour() {
    let plus = roundtrip_time("12:30:45+05:45");
    let tz = plus.timezone.expect("+05:45");
    assert_eq!(tz.offset_minutes, 345);
    let minus = roundtrip_time("12:30:45-03:30");
    let tz = minus.timezone.expect("-03:30");
    assert_eq!(tz.offset_minutes, -210);
}

#[test]
fn time_microseconds_trailing_nonzero_zeros() {
    let t = roundtrip_time("12:30:45.100000");
    assert_eq!(t.microsecond, 100000);
}

#[test]
fn time_malformed_returns_err_never_panics() {
    let cases = [
        "",
        "   ",
        "25:00:00",
        "12:60:00",
        "12:30:45+99:00",
        "not-a-time",
        "12:30:45xyz",
    ];
    for input in cases {
        let parsed = TimeValue::from_str(input);
        assert!(
            parsed.is_err(),
            "FromStr({input:?}) must return Err, got {parsed:?}"
        );
    }
}

#[test]
fn time_serde_is_iso_string() {
    let t = TimeValue::from_str("12:30:45+02:00").expect("parse");
    assert_serde_string_roundtrip(&t, "12:30:45+02:00");
}

fn roundtrip_timezone(input: &str) -> TimezoneValue {
    let parsed = TimezoneValue::from_str(input)
        .unwrap_or_else(|e| panic!("parse {input:?} must succeed: {e}"));
    let displayed = parsed.to_string();
    assert_eq!(displayed, input, "TimezoneValue Display must match input");
    let again = TimezoneValue::from_str(&displayed)
        .unwrap_or_else(|e| panic!("re-parse {displayed:?} must succeed: {e}"));
    assert_eq!(again, parsed);
    parsed
}

#[test]
fn timezone_z_roundtrip() {
    let tz = roundtrip_timezone("Z");
    assert_eq!(tz.offset_minutes, 0);
}

#[test]
fn timezone_offsets_roundtrip() {
    for input in ["+02:00", "-05:00", "+05:45", "-03:30", "+00:30"] {
        roundtrip_timezone(input);
    }
}

#[test]
fn timezone_serde_is_iso_string() {
    let tz = TimezoneValue::from_str("+02:00").expect("parse");
    assert_serde_string_roundtrip(&tz, "+02:00");
}

fn roundtrip_semantic_datetime_canonical(input: &str, canonical: &str) -> SemanticDateTime {
    let parsed = SemanticDateTime::from_str(input)
        .unwrap_or_else(|e| panic!("parse {input:?} must succeed: {e}"));
    let displayed = parsed.to_string();
    assert_eq!(
        displayed, canonical,
        "SemanticDateTime Display must be canonical form"
    );
    let again = SemanticDateTime::from_str(&displayed)
        .unwrap_or_else(|e| panic!("re-parse {displayed:?} must succeed: {e}"));
    assert_eq!(again, parsed);
    parsed
}

#[test]
fn semantic_datetime_date_form_roundtrip() {
    let dt = roundtrip_semantic_datetime_canonical("2025-03-04", "2025-03-04");
    assert_eq!(dt.year, 2025);
    assert_eq!(dt.month, 3);
    assert_eq!(dt.day, 4);
    assert_eq!(dt.hour, 0);
    assert!(dt.timezone.is_none());
}

#[test]
fn semantic_datetime_midnight_without_timezone_canonicalizes_to_date() {
    let dt = roundtrip_semantic_datetime_canonical("2025-01-01T00:00:00", "2025-01-01");
    assert_eq!(dt.year, 2025);
    assert_eq!(dt.month, 1);
    assert_eq!(dt.day, 1);
    assert_eq!(dt.hour, 0);
    assert!(dt.timezone.is_none());
}

#[test]
fn semantic_datetime_with_time_roundtrip() {
    let dt = roundtrip_semantic_datetime_canonical(
        "2025-03-04T12:30:45+02:00",
        "2025-03-04T12:30:45+02:00",
    );
    assert_eq!(dt.hour, 12);
    assert_eq!(dt.timezone.as_ref().map(|z| z.offset_minutes), Some(120));
}

#[test]
fn semantic_datetime_serde_is_iso_string() {
    let dt = SemanticDateTime::from_str("2025-03-04T12:30:45Z").expect("parse");
    assert_serde_string_roundtrip(&dt, "2025-03-04T12:30:45Z");
}

fn roundtrip_semantic_time(input: &str) -> SemanticTime {
    let parsed = SemanticTime::from_str(input)
        .unwrap_or_else(|e| panic!("parse {input:?} must succeed: {e}"));
    let displayed = parsed.to_string();
    assert_eq!(displayed, input, "SemanticTime Display must match input");
    let again = SemanticTime::from_str(&displayed)
        .unwrap_or_else(|e| panic!("re-parse {displayed:?} must succeed: {e}"));
    assert_eq!(again, parsed);
    parsed
}

#[test]
fn semantic_time_roundtrip() {
    roundtrip_semantic_time("00:00:00");
    roundtrip_semantic_time("23:59:59.999999");
    roundtrip_semantic_time("12:30:45Z");
    roundtrip_semantic_time("12:30:45+02:00");
}

#[test]
fn semantic_time_serde_is_iso_string() {
    let t = SemanticTime::from_str("12:30:45-05:00").expect("parse");
    assert_serde_string_roundtrip(&t, "12:30:45-05:00");
}

#[test]
fn semantic_timezone_roundtrip() {
    for input in ["Z", "+02:00", "-05:00", "+05:45"] {
        let parsed = SemanticTimezone::from_str(input).expect("parse");
        assert_eq!(parsed.to_string(), input);
        let again = SemanticTimezone::from_str(&parsed.to_string()).expect("re-parse");
        assert_eq!(again, parsed);
    }
}

#[test]
fn semantic_timezone_serde_is_iso_string() {
    let tz = SemanticTimezone::from_str("Z").expect("parse");
    assert_serde_string_roundtrip(&tz, "Z");
}

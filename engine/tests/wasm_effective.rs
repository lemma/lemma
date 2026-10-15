//! WASM `run` / `show` must reject invalid `effective` strings (same contract as HTTP/MCP).

use lemma::{DateGranularity, DateTimeValue, Error};

#[test]
fn invalid_effective_string_must_error_not_default_to_now() {
    assert!(
        lemma::resolve_effective(Some("not-a-datetime")).is_err(),
        "invalid effective must return error before planning, not fall back to now"
    );
}

#[test]
fn absent_effective_resolves_to_now() {
    let before = DateTimeValue::now();
    let resolved = lemma::resolve_effective(None).expect("absent effective must succeed");
    let after = DateTimeValue::now();
    assert!(
        resolved >= before && resolved <= after,
        "absent effective must resolve to current time"
    );
}

#[test]
fn whitespace_effective_resolves_to_now() {
    let before = DateTimeValue::now();
    let resolved =
        lemma::resolve_effective(Some("   ")).expect("whitespace effective must succeed");
    let after = DateTimeValue::now();
    assert!(
        resolved >= before && resolved <= after,
        "whitespace effective must resolve to current time"
    );
}

#[test]
fn date_only_effective_is_utc_midnight_instant() {
    let parsed = lemma::resolve_effective(Some("2020-01-01")).expect("valid effective must parse");
    assert_eq!(parsed.granularity, DateGranularity::DateTime);
    assert_eq!(parsed.to_string(), "2020-01-01T00:00:00Z");
    assert_eq!(parsed.hour, 0);
    assert_eq!(parsed.minute, 0);
    assert_eq!(parsed.second, 0);
    assert_eq!(parsed.microsecond, 0);
    assert_eq!(
        parsed.timezone.as_ref().map(|tz| tz.offset_minutes),
        Some(0)
    );
}

#[test]
fn year_only_effective_is_jan_first_utc_midnight() {
    let parsed = lemma::resolve_effective(Some("2025")).expect("year effective must parse");
    assert_eq!(parsed.granularity, DateGranularity::DateTime);
    assert_eq!(parsed.to_string(), "2025-01-01T00:00:00Z");
}

#[test]
fn year_month_effective_is_first_of_month_utc_midnight() {
    let parsed =
        lemma::resolve_effective(Some("2025-03")).expect("year-month effective must parse");
    assert_eq!(parsed.granularity, DateGranularity::DateTime);
    assert_eq!(parsed.to_string(), "2025-03-01T00:00:00Z");
}

#[test]
fn datetime_effective_keeps_clock_and_timezone() {
    let parsed = lemma::resolve_effective(Some("2025-03-04T14:30:00+02:00"))
        .expect("datetime effective must parse");
    assert_eq!(parsed.granularity, DateGranularity::DateTime);
    assert_eq!(parsed.to_string(), "2025-03-04T14:30:00+02:00");
    assert_eq!(parsed.hour, 14);
    assert_eq!(parsed.minute, 30);
}

#[test]
fn invalid_effective_is_request_error() {
    let err = lemma::resolve_effective(Some("garbage"))
        .expect_err("invalid effective must be request error");
    assert!(matches!(err, Error::Request { .. }));
}

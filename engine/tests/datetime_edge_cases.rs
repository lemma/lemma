#[path = "support/get_rule_value.rs"]
mod get_rule_value;

use get_rule_value::get_rule_value;
use lemma::Engine;

#[test]
fn test_leap_year_feb_29_valid() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data leap_date: 2024-02-29
rule check: leap_date
    "#;

    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Failed to parse");

    let lit = get_rule_value(&engine, "test", "check");
    if let lemma::ValueKind::Date(date) = &lit.value {
        assert_eq!(date.year, 2024);
        assert_eq!(date.month, 2);
        assert_eq!(date.day, 29);
    } else {
        panic!("Expected Date value");
    }
}

#[test]
fn test_leap_year_century_2000() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data leap_date: 2000-02-29
rule check: leap_date
    "#;

    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Failed to parse");

    let lit = get_rule_value(&engine, "test", "check");
    if let lemma::ValueKind::Date(date) = &lit.value {
        assert_eq!(date.year, 2000);
        assert_eq!(date.month, 2);
        assert_eq!(date.day, 29);
    } else {
        panic!("Expected Date value");
    }
}

#[test]
fn test_non_leap_year_century_1900() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data start_date: 1900-02-28
rule next_day: start_date + 1 day
    "#;

    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Failed to parse");

    let lit = get_rule_value(&engine, "test", "next_day");
    if let lemma::ValueKind::Date(date) = &lit.value {
        assert_eq!(date.year, 1900);
        assert_eq!(date.month, 3);
        assert_eq!(date.day, 1);
    } else {
        panic!("Expected Date value");
    }
}

#[test]
fn test_leap_year_century_2100_not_leap() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data start_date: 2100-02-28
rule next_day: start_date + 1 day
    "#;

    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Failed to parse");

    let lit = get_rule_value(&engine, "test", "next_day");
    if let lemma::ValueKind::Date(date) = &lit.value {
        assert_eq!(date.year, 2100);
        assert_eq!(date.month, 3);
        assert_eq!(date.day, 1);
    } else {
        panic!("Expected Date value");
    }
}

#[test]
fn test_add_month_with_day_clamp_jan_31_to_feb() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data start_date: 2024-01-31
rule next_month: start_date + 1 month
    "#;

    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Failed to parse");

    let lit = get_rule_value(&engine, "test", "next_month");
    if let lemma::ValueKind::Date(date) = &lit.value {
        assert_eq!(date.year, 2024);
        assert_eq!(date.month, 2);
        assert_eq!(date.day, 29);
    } else {
        panic!("Expected Date value");
    }
}

#[test]
fn test_add_month_with_day_clamp_jan_31_to_feb_non_leap() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data start_date: 2023-01-31
rule next_month: start_date + 1 month
    "#;

    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Failed to parse");

    let lit = get_rule_value(&engine, "test", "next_month");
    if let lemma::ValueKind::Date(date) = &lit.value {
        assert_eq!(date.year, 2023);
        assert_eq!(date.month, 2);
        assert_eq!(date.day, 28);
    } else {
        panic!("Expected Date value");
    }
}

#[test]
fn test_add_year_to_feb_29_leap_to_non_leap() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data leap_date: 2024-02-29
rule next_year: leap_date + 1 year
    "#;

    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Failed to parse");

    let lit = get_rule_value(&engine, "test", "next_year");
    if let lemma::ValueKind::Date(date) = &lit.value {
        assert_eq!(date.year, 2025);
        assert_eq!(date.month, 2);
        assert_eq!(date.day, 28);
    } else {
        panic!("Expected Date value");
    }
}

#[test]
fn test_add_4_years_to_feb_29_leap_to_leap() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data leap_date: 2024-02-29
rule four_years_later: leap_date + 4 year
    "#;

    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Failed to parse");

    let lit = get_rule_value(&engine, "test", "four_years_later");
    if let lemma::ValueKind::Date(date) = &lit.value {
        assert_eq!(date.year, 2028);
        assert_eq!(date.month, 2);
        assert_eq!(date.day, 29);
    } else {
        panic!("Expected Date value");
    }
}

#[test]
fn test_subtract_months_cross_year_boundary() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data start_date: 2024-02-15
rule three_months_ago: start_date - 3 month
    "#;

    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Failed to parse");

    let lit = get_rule_value(&engine, "test", "three_months_ago");
    if let lemma::ValueKind::Date(date) = &lit.value {
        assert_eq!(date.year, 2023);
        assert_eq!(date.month, 11);
        assert_eq!(date.day, 15);
    } else {
        panic!("Expected Date value");
    }
}

#[test]
fn test_add_months_cross_multiple_years() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data start_date: 2023-01-15
rule twenty_months_later: start_date + 20 month
    "#;

    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Failed to parse");

    let lit = get_rule_value(&engine, "test", "twenty_months_later");
    if let lemma::ValueKind::Date(date) = &lit.value {
        assert_eq!(date.year, 2024);
        assert_eq!(date.month, 9);
        assert_eq!(date.day, 15);
    } else {
        panic!("Expected Date value");
    }
}

#[test]
fn test_subtract_year_from_year_boundary() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data start_date: 2024-01-01
rule last_year: start_date - 1 year
    "#;

    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Failed to parse");

    let lit = get_rule_value(&engine, "test", "last_year");
    if let lemma::ValueKind::Date(date) = &lit.value {
        assert_eq!(date.year, 2023);
        assert_eq!(date.month, 1);
        assert_eq!(date.day, 1);
    } else {
        panic!("Expected Date value");
    }
}

#[test]
fn test_date_difference_across_leap_year() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data start_date: 2024-01-01
data end_date: 2025-01-01
rule days_diff: start_date...end_date as second as number
    "#;

    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Failed to parse");

    let lit = get_rule_value(&engine, "test", "days_diff");
    if let lemma::ValueKind::Number(second) = &lit.value {
        // 366 day = 31,622,400 second
        assert_eq!(
            lemma::ValueKind::Number(second.clone())
                .as_decimal_magnitude()
                .unwrap(),
            rust_decimal::Decimal::from(31_622_400)
        );
    } else {
        panic!("Expected Number value, got {:?}", lit.value);
    }
}

#[test]
fn test_date_difference_non_leap_year() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data start_date: 2023-01-01
data end_date: 2024-01-01
rule days_diff: start_date...end_date as second as number
    "#;

    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Failed to parse");

    let lit = get_rule_value(&engine, "test", "days_diff");
    if let lemma::ValueKind::Number(second) = &lit.value {
        // 365 day = 31,536,000 second
        assert_eq!(
            lemma::ValueKind::Number(second.clone())
                .as_decimal_magnitude()
                .unwrap(),
            rust_decimal::Decimal::from(31_536_000)
        );
    } else {
        panic!("Expected Number value, got {:?}", lit.value);
    }
}

#[test]
fn test_add_hours_crossing_midnight() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data start_datetime: 2024-03-15T22:00:00
rule next_day: start_datetime + 5 hour
    "#;

    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Failed to parse");

    let lit = get_rule_value(&engine, "test", "next_day");
    if let lemma::ValueKind::Date(date) = &lit.value {
        assert_eq!(date.year, 2024);
        assert_eq!(date.month, 3);
        assert_eq!(date.day, 16);
        assert_eq!(date.hour, 3);
        assert_eq!(date.minute, 0);
    } else {
        panic!("Expected Date value");
    }
}

#[test]
fn test_subtract_hours_crossing_midnight_backward() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data start_datetime: 2024-03-16T02:00:00
rule prev_day: start_datetime - 5 hour
    "#;

    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Failed to parse");

    let lit = get_rule_value(&engine, "test", "prev_day");
    if let lemma::ValueKind::Date(date) = &lit.value {
        assert_eq!(date.year, 2024);
        assert_eq!(date.month, 3);
        assert_eq!(date.day, 15);
        assert_eq!(date.hour, 21);
        assert_eq!(date.minute, 0);
    } else {
        panic!("Expected Date value");
    }
}

#[test]
fn test_add_minutes_precise() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data start_time: 2024-03-15T10:30:45
rule later: start_time + 90 minute
    "#;

    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Failed to parse");

    let lit = get_rule_value(&engine, "test", "later");
    if let lemma::ValueKind::Date(date) = &lit.value {
        assert_eq!(date.hour, 12);
        assert_eq!(date.minute, 0);
        assert_eq!(date.second, 45);
    } else {
        panic!("Expected Date value");
    }
}

#[test]
fn test_add_seconds_carry_to_minutes() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data start_time: 2024-03-15T10:30:30
rule later: start_time + 90 second
    "#;

    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Failed to parse");

    let lit = get_rule_value(&engine, "test", "later");
    if let lemma::ValueKind::Date(date) = &lit.value {
        assert_eq!(date.hour, 10);
        assert_eq!(date.minute, 32);
        assert_eq!(date.second, 0);
    } else {
        panic!("Expected Date value");
    }
}

#[test]
fn test_time_arithmetic_crossing_midnight() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data evening_time: 23:30:00
rule after_midnight: evening_time + 90 minute
    "#;

    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Failed to parse");

    let lit = get_rule_value(&engine, "test", "after_midnight");
    if let lemma::ValueKind::Time(time) = &lit.value {
        assert_eq!(time.hour, 1);
        assert_eq!(time.minute, 0);
        assert_eq!(time.second, 0);
    } else {
        panic!("Expected Time value");
    }
}

#[test]
fn test_time_difference_rejected() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data start_time: 10:00:00
data end_time: 15:30:00
rule timespan: end_time - start_time
    "#;

    let result = engine.load([(lemma::SourceType::Volatile, code.to_string())]);
    assert!(result.is_err(), "Expected planning error");
    let combined = result
        .unwrap_err()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ");
    assert!(
        combined.to_lowercase().contains("datetime range"),
        "Expected datetime range suggestion, got: {}",
        combined
    );
}

#[test]
fn test_negative_time_difference_rejected() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data start_time: 15:30:00
data end_time: 10:00:00
rule timespan: end_time - start_time
    "#;

    let result = engine.load([(lemma::SourceType::Volatile, code.to_string())]);
    assert!(result.is_err(), "Expected planning error");
    let combined = result
        .unwrap_err()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ");
    assert!(
        combined.to_lowercase().contains("datetime range"),
        "Expected datetime range suggestion, got: {}",
        combined
    );
}

#[test]
fn test_add_large_duration_days() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data start_date: 2024-01-01
rule future: start_date + 1000 day
    "#;

    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Failed to parse");

    let lit = get_rule_value(&engine, "test", "future");
    if let lemma::ValueKind::Date(date) = &lit.value {
        assert_eq!(date.year, 2026);
        assert_eq!(date.month, 9);
        assert_eq!(date.day, 27);
    } else {
        panic!("Expected Date value");
    }
}

#[test]
fn test_fractional_hours() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data start_time: 2024-03-15T10:00:00
rule later: start_time + 2.5 hour
    "#;

    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Failed to parse");

    let lit = get_rule_value(&engine, "test", "later");
    if let lemma::ValueKind::Date(date) = &lit.value {
        assert_eq!(date.hour, 12);
        assert_eq!(date.minute, 30);
        assert_eq!(date.second, 0);
    } else {
        panic!("Expected Date value");
    }
}

#[test]
fn test_datetime_comparison_across_years() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data date1: 2023-12-31T23:59:59
data date2: 2024-01-01T00:00:00
rule is_before: date1 < date2
    "#;

    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Failed to parse");

    let lit = get_rule_value(&engine, "test", "is_before");
    if let lemma::ValueKind::Boolean(value) = &lit.value {
        assert!(*value);
    } else {
        panic!("Expected Boolean value");
    }
}

#[test]
fn test_month_31_to_30_day_month() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data start_date: 2024-03-31
rule april: start_date + 1 month
    "#;

    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Failed to parse");

    let lit = get_rule_value(&engine, "test", "april");
    if let lemma::ValueKind::Date(date) = &lit.value {
        assert_eq!(date.year, 2024);
        assert_eq!(date.month, 4);
        assert_eq!(date.day, 30);
    } else {
        panic!("Expected Date value");
    }
}

#[test]
fn time_plus_extreme_duration_conversion_fails() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data t: 12:00:00+00:00
rule result: t + 9999999999999 second
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("parse");
    let now = lemma::DateTimeValue::now();
    let response = engine
        .run(
            None,
            "test",
            Some(&now),
            std::collections::HashMap::new(),
            Some(&["result".to_string()]),
            false,
        )
        .expect("run");
    let rule_result = response.get("result").expect("rule exists");
    assert!(rule_result.vetoed, "extreme duration must veto, not panic");
    assert!(
        rule_result
            .veto_reason
            .as_deref()
            .is_some_and(|r| r.contains("Duration conversion failed")),
        "huge second count must fail duration→chrono conversion: {:?}",
        rule_result.veto_reason
    );
}

#[test]
fn time_minus_extreme_duration_conversion_fails() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data t: 12:00:00+00:00
rule result: t - 9999999999999 second
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("parse");
    let now = lemma::DateTimeValue::now();
    let response = engine
        .run(
            None,
            "test",
            Some(&now),
            std::collections::HashMap::new(),
            Some(&["result".to_string()]),
            false,
        )
        .expect("run");
    let rule_result = response.get("result").expect("rule exists");
    assert!(
        rule_result.vetoed,
        "extreme duration subtraction must veto, not panic"
    );
    assert!(
        rule_result
            .veto_reason
            .as_deref()
            .is_some_and(|r| r.contains("Duration conversion failed")),
        "huge second count must fail duration→chrono conversion: {:?}",
        rule_result.veto_reason
    );
}

#[test]
fn time_plus_convertible_duration_time_overflow() {
    let mut engine = Engine::new();
    // Converts to chrono Duration (fits i64 micros) but overflows Time epoch arithmetic.
    let code = r#"
spec test
uses lemma units
data t: 12:00:00+00:00
rule result: t + 9223372036854 second
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("parse");
    let now = lemma::DateTimeValue::now();
    let response = engine
        .run(
            None,
            "test",
            Some(&now),
            std::collections::HashMap::new(),
            Some(&["result".to_string()]),
            false,
        )
        .expect("run");
    let rule_result = response.get("result").expect("rule exists");
    assert!(rule_result.vetoed, "must veto, not panic");
    assert_eq!(
        rule_result.veto_reason.as_deref(),
        Some("Time overflow"),
        "convertible extreme duration must hit Time overflow: {:?}",
        rule_result.veto_reason
    );
}

#[test]
fn time_minus_convertible_duration_time_overflow() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data t: 12:00:00+00:00
rule result: t - 9223372036854 second
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("parse");
    let now = lemma::DateTimeValue::now();
    let response = engine
        .run(
            None,
            "test",
            Some(&now),
            std::collections::HashMap::new(),
            Some(&["result".to_string()]),
            false,
        )
        .expect("run");
    let rule_result = response.get("result").expect("rule exists");
    assert!(rule_result.vetoed, "must veto, not panic");
    assert_eq!(
        rule_result.veto_reason.as_deref(),
        Some("Time overflow"),
        "convertible extreme duration must hit Time overflow: {:?}",
        rule_result.veto_reason
    );
}

#[test]
fn date_plus_extreme_duration_conversion_fails() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data d: 2024-01-01
rule result: d + 9999999999999 second
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("parse");
    let now = lemma::DateTimeValue::now();
    let response = engine
        .run(
            None,
            "test",
            Some(&now),
            std::collections::HashMap::new(),
            Some(&["result".to_string()]),
            false,
        )
        .expect("run");
    let rule_result = response.get("result").expect("rule exists");
    assert!(rule_result.vetoed, "extreme duration must veto, not panic");
    assert!(
        rule_result
            .veto_reason
            .as_deref()
            .is_some_and(|r| r.contains("Duration conversion failed")),
        "huge second count must fail duration→chrono conversion: {:?}",
        rule_result.veto_reason
    );
}

#[test]
fn date_minus_extreme_duration_conversion_fails() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data d: 2024-01-01
rule result: d - 9999999999999 second
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("parse");
    let now = lemma::DateTimeValue::now();
    let response = engine
        .run(
            None,
            "test",
            Some(&now),
            std::collections::HashMap::new(),
            Some(&["result".to_string()]),
            false,
        )
        .expect("run");
    let rule_result = response.get("result").expect("rule exists");
    assert!(
        rule_result.vetoed,
        "extreme duration subtraction must veto, not panic"
    );
    assert!(
        rule_result
            .veto_reason
            .as_deref()
            .is_some_and(|r| r.contains("Duration conversion failed")),
        "huge second count must fail duration→chrono conversion: {:?}",
        rule_result.veto_reason
    );
}

#[test]
fn date_plus_convertible_duration_date_overflow() {
    let mut engine = Engine::new();
    // Converts to chrono Duration (fits i64 micros) but overflows Date arithmetic.
    let code = r#"
spec test
uses lemma units
data d: 2024-01-01
rule result: d + 9223372036854 second
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("parse");
    let now = lemma::DateTimeValue::now();
    let response = engine
        .run(
            None,
            "test",
            Some(&now),
            std::collections::HashMap::new(),
            Some(&["result".to_string()]),
            false,
        )
        .expect("run");
    let rule_result = response.get("result").expect("rule exists");
    assert!(rule_result.vetoed, "must veto, not panic");
    assert_eq!(
        rule_result.veto_reason.as_deref(),
        Some("Date overflow"),
        "convertible extreme duration must hit Date overflow: {:?}",
        rule_result.veto_reason
    );
}

#[test]
fn date_minus_convertible_duration_date_overflow() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data d: 2024-01-01
rule result: d - 9223372036854 second
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("parse");
    let now = lemma::DateTimeValue::now();
    let response = engine
        .run(
            None,
            "test",
            Some(&now),
            std::collections::HashMap::new(),
            Some(&["result".to_string()]),
            false,
        )
        .expect("run");
    let rule_result = response.get("result").expect("rule exists");
    assert!(rule_result.vetoed, "must veto, not panic");
    assert_eq!(
        rule_result.veto_reason.as_deref(),
        Some("Date overflow"),
        "convertible extreme duration must hit Date overflow: {:?}",
        rule_result.veto_reason
    );
}

#[test]
fn date_plus_i32_max_months_calendar_offset_too_large() {
    let mut engine = Engine::new();
    // Fits i32 months, then year*12 + offset overflows i32 — must Veto, not panic.
    let code = r#"
spec test
uses lemma units
data d: 2024-01-01
rule result: d + 2147483647 month
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("parse");
    let now = lemma::DateTimeValue::now();
    let response = engine
        .run(
            None,
            "test",
            Some(&now),
            std::collections::HashMap::new(),
            Some(&["result".to_string()]),
            false,
        )
        .expect("run");
    let rule_result = response.get("result").expect("rule exists");
    assert!(rule_result.vetoed, "must veto, not panic");
    assert_eq!(
        rule_result.veto_reason.as_deref(),
        Some("Calendar offset too large"),
        "i32-max month add must hit Calendar offset too large: {:?}",
        rule_result.veto_reason
    );
}

#[test]
fn date_minus_i32_min_months_calendar_offset_too_large() {
    let mut engine = Engine::new();
    // Negating i32::MIN overflows — must Veto, not panic.
    let code = r#"
spec test
uses lemma units
data d: 2024-01-01
rule result: d - 2147483648 month
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("parse");
    let now = lemma::DateTimeValue::now();
    let response = engine
        .run(
            None,
            "test",
            Some(&now),
            std::collections::HashMap::new(),
            Some(&["result".to_string()]),
            false,
        )
        .expect("run");
    let rule_result = response.get("result").expect("rule exists");
    assert!(rule_result.vetoed, "must veto, not panic");
    assert_eq!(
        rule_result.veto_reason.as_deref(),
        Some("Calendar offset too large"),
        "i32::MIN month subtract must hit Calendar offset too large: {:?}",
        rule_result.veto_reason
    );
}

#[test]
fn date_plus_huge_years_calendar_offset_too_large() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data d: 2024-01-01
rule result: d + 999999999 year
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("parse");
    let now = lemma::DateTimeValue::now();
    let response = engine
        .run(
            None,
            "test",
            Some(&now),
            std::collections::HashMap::new(),
            Some(&["result".to_string()]),
            false,
        )
        .expect("run");
    let rule_result = response.get("result").expect("rule exists");
    assert!(rule_result.vetoed, "must veto, not panic");
    assert_eq!(
        rule_result.veto_reason.as_deref(),
        Some("Calendar offset too large"),
        "huge year add must hit Calendar offset too large: {:?}",
        rule_result.veto_reason
    );
}

#[test]
fn test_dec_31_plus_1_month() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data start_date: 2023-12-31
rule january: start_date + 1 month
    "#;

    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Failed to parse");

    let lit = get_rule_value(&engine, "test", "january");
    if let lemma::ValueKind::Date(date) = &lit.value {
        assert_eq!(date.year, 2024);
        assert_eq!(date.month, 1);
        assert_eq!(date.day, 31);
    } else {
        panic!("Expected Date value");
    }
}

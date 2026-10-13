#[path = "support/get_rule_value.rs"]
mod get_rule_value;

use get_rule_value::get_rule_value;
use lemma::Engine;
use rust_decimal::Decimal;
use std::str::FromStr;

fn decimal_lit(s: &str) -> Decimal {
    Decimal::from_str(s).expect("BUG: test decimal literal must parse")
}

#[test]
fn test_timezone_comparison_same_instant() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data time_nyc: 2024-03-15T10:00:00-05:00
data time_london: 2024-03-15T15:00:00+00:00
rule are_equal: time_nyc is time_london
    "#;

    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Failed to parse");

    if let lemma::LiteralValue {
        value: lemma::ValueKind::Boolean(value),
        ..
    } = get_rule_value(&engine, "test", "are_equal")
    {
        assert!(value, "Same instant in different timezones should be equal");
    } else {
        panic!("Expected Boolean value");
    }
}

#[test]
fn test_timezone_comparison_different_instants() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data time_nyc: 2024-03-15T10:00:00-05:00
data time_tokyo: 2024-03-15T10:00:00+09:00
rule nyc_is_later: time_nyc > time_tokyo
    "#;

    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Failed to parse");

    if let lemma::LiteralValue {
        value: lemma::ValueKind::Boolean(value),
        ..
    } = get_rule_value(&engine, "test", "nyc_is_later")
    {
        assert!(value, "NYC 10am is later than Tokyo 10am (same local time)");
    } else {
        panic!("Expected Boolean value");
    }
}

#[test]
fn test_timezone_arithmetic_preserved() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data start_time: 2024-03-15T10:00:00+01:00
rule later: start_time + 2 hour
    "#;

    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Failed to parse");

    if let lemma::LiteralValue {
        value: lemma::ValueKind::Date(date),
        ..
    } = get_rule_value(&engine, "test", "later")
    {
        assert_eq!(date.hour, 12);
        assert_eq!(date.minute, 0);
        assert_eq!(date.second, 0);
        if let Some(tz) = &date.timezone {
            assert_eq!(tz.offset_minutes, 60);
        } else {
            panic!("Expected timezone to be preserved");
        }
    } else {
        panic!("Expected Date value");
    }
}

// Note: Z notation parsing has issues in current grammar
// Using +00:00 is more reliable

#[test]
fn test_negative_timezone_offset() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data west_coast: 2024-03-15T09:00:00-08:00
rule later: west_coast + 3 hour
    "#;

    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Failed to parse");

    if let lemma::LiteralValue {
        value: lemma::ValueKind::Date(date),
        ..
    } = get_rule_value(&engine, "test", "later")
    {
        assert_eq!(date.hour, 12);
        if let Some(tz) = date.timezone {
            assert_eq!(tz.offset_minutes, -480);
        } else {
            panic!("Expected timezone to be preserved");
        }
    } else {
        panic!("Expected Date value");
    }
}

#[test]
fn test_timezone_crossing_midnight() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data evening: 2024-03-15T23:00:00+05:30
rule next_day: evening + 2 hour
    "#;

    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Failed to parse");

    if let lemma::LiteralValue {
        value: lemma::ValueKind::Date(date),
        ..
    } = get_rule_value(&engine, "test", "next_day")
    {
        assert_eq!(date.year, 2024);
        assert_eq!(date.month, 3);
        assert_eq!(date.day, 16);
        assert_eq!(date.hour, 1);
        assert_eq!(date.minute, 0);
        if let Some(tz) = date.timezone {
            assert_eq!(tz.offset_minutes, 330);
        } else {
            panic!("Expected timezone to be preserved");
        }
    } else {
        panic!("Expected Date value");
    }
}

#[test]
fn test_timezone_date_difference() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data time1: 2024-03-15T10:00:00-05:00
data time2: 2024-03-15T16:00:00+01:00
rule hours_diff: time1...time2 as hour as number
    "#;

    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Failed to parse");

    if let lemma::LiteralValue {
        value: lemma::ValueKind::Number(hour),
        ..
    } = get_rule_value(&engine, "test", "hours_diff")
    {
        assert_eq!(
            lemma::ValueKind::Number(hour)
                .as_decimal_magnitude()
                .unwrap(),
            decimal_lit("0")
        );
    } else {
        panic!("Expected Measure duration value");
    }
}

#[test]
fn test_timezone_45_minute_offset() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data nepal_time: 2024-03-15T14:30:00+05:45
rule preserved: nepal_time
    "#;

    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Failed to parse");

    if let lemma::LiteralValue {
        value: lemma::ValueKind::Date(date),
        ..
    } = get_rule_value(&engine, "test", "preserved")
    {
        if let Some(tz) = date.timezone {
            assert_eq!(tz.offset_minutes, 345);
        } else {
            panic!("Expected timezone to be present");
        }
    } else {
        panic!("Expected Date value");
    }
}

fn rule_boolean(code: &str, rule: &str) -> bool {
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Failed to parse");
    match get_rule_value(&engine, "test", rule).value {
        lemma::ValueKind::Boolean(value) => value,
        other => panic!("Expected Boolean value, got {other:?}"),
    }
}

#[test]
fn test_negative_half_hour_offset_is_same_instant_as_utc() {
    let code = r#"
spec test
data st_johns: 2024-01-01T12:00:00-03:30
data utc: 2024-01-01T15:30:00+00:00
rule same_instant: st_johns is utc
    "#;
    assert!(
        rule_boolean(code, "same_instant"),
        "12:00-03:30 is 15:30 UTC"
    );
}

#[test]
fn test_negative_half_hour_offset_arithmetic_keeps_instant() {
    let code = r#"
spec test
uses lemma units
data marquesas: 2024-01-01T12:00:00-09:30
data utc: 2024-01-01T22:30:00+00:00
rule same_instant: marquesas + 1 hour is utc
    "#;
    assert!(
        rule_boolean(code, "same_instant"),
        "12:00-09:30 plus one hour is 22:30 UTC"
    );
}

#[test]
fn test_negative_offset_below_one_hour_keeps_sign() {
    let code = r#"
spec test
data behind: 2024-01-01T12:00:00-00:30
data utc: 2024-01-01T12:30:00+00:00
rule same_instant: behind is utc
    "#;
    assert!(
        rule_boolean(code, "same_instant"),
        "12:00-00:30 is 12:30 UTC"
    );
}

#[test]
fn test_negative_offset_below_one_hour_displays_sign() {
    let mut engine = Engine::new();
    let code = r#"
spec test
data behind: 2024-01-01T12:00:00-00:30
rule kept: behind
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Failed to parse");
    let shown = get_rule_value(&engine, "test", "kept").to_string();
    assert!(shown.contains("-00:30"), "expected -00:30 in {shown}");
}

// Note: Time literals with timezones are not supported by chrono parser
// (time-only strings can't be parsed with timezone offsets)
// Use full datetime if timezone is needed

#[test]
fn test_extreme_western_timezone() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data hawaii: 2024-03-15T12:00:00-10:00
rule later: hawaii + 1 hour
    "#;

    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Failed to parse");

    if let lemma::LiteralValue {
        value: lemma::ValueKind::Date(date),
        ..
    } = get_rule_value(&engine, "test", "later")
    {
        assert_eq!(date.hour, 13);
        if let Some(tz) = date.timezone {
            assert_eq!(tz.offset_minutes, -600);
        } else {
            panic!("Expected timezone to be preserved");
        }
    } else {
        panic!("Expected Date value");
    }
}

#[test]
fn test_extreme_eastern_timezone() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data kiribati: 2024-03-15T12:00:00+14:00
rule earlier: kiribati - 1 hour
    "#;

    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Failed to parse");

    if let lemma::LiteralValue {
        value: lemma::ValueKind::Date(date),
        ..
    } = get_rule_value(&engine, "test", "earlier")
    {
        assert_eq!(date.hour, 11);
        if let Some(tz) = date.timezone {
            assert_eq!(tz.offset_minutes, 840);
        } else {
            panic!("Expected timezone to be preserved");
        }
    } else {
        panic!("Expected Date value");
    }
}

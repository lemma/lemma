use lemma::{DateGranularity, DateTimeValue, Engine, TimezoneValue, ValueKind};
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

fn source() -> lemma::SourceType {
    lemma::SourceType::Path(Arc::new(PathBuf::from("test.lemma")))
}

const MONEY_TYPEDEF: &str = r#"
data money: measure
  -> unit eur: 1
"#;

fn effective(y: i32, m: u32, d: u32, h: u32, min: u32, s: u32) -> DateTimeValue {
    DateTimeValue {
        year: y,
        month: m,
        day: d,
        hour: h,
        minute: min,
        second: s,
        microsecond: 0,
        timezone: Some(TimezoneValue {
            offset_hours: 0,
            offset_minutes: 0,
        }),
        granularity: DateGranularity::DateTime,
    }
}

fn eval_literal(
    code: impl AsRef<str>,
    spec_name: &str,
    rule_name: &str,
    effective: &DateTimeValue,
) -> lemma::LiteralValue {
    let code = code.as_ref();
    let mut engine = Engine::new();
    engine
        .load([(source(), code.to_string())])
        .expect("Should parse and plan");
    let response = engine
        .run(None, spec_name, Some(effective), HashMap::new(), None, true)
        .expect("Should evaluate");
    let rule = response
        .results
        .get(rule_name)
        .unwrap_or_else(|| panic!("Rule '{}' not found", rule_name));
    if rule.vetoed {
        panic!(
            "Rule '{}' vetoed: {}",
            rule_name,
            rule.veto_reason.as_deref().unwrap_or("Vetoed")
        );
    }
    rule.explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("BUG: non-vetoed rule missing value")
        .clone()
}

fn eval_rule(
    code: impl AsRef<str>,
    spec_name: &str,
    rule_name: &str,
    effective: &DateTimeValue,
) -> String {
    let code = code.as_ref();
    let mut engine = Engine::new();
    engine
        .load([(source(), code.to_string())])
        .expect("Should parse and plan");
    let response = engine
        .run(None, spec_name, Some(effective), HashMap::new(), None, true)
        .expect("Should evaluate");
    response
        .results
        .get(rule_name)
        .unwrap_or_else(|| panic!("Rule '{}' not found", rule_name))
        .result()
        .expect("result")
        .to_string()
}

fn eval_rule_measure_unit(
    code: impl AsRef<str>,
    spec_name: &str,
    rule_name: &str,
    unit: &str,
    effective: &DateTimeValue,
) -> Decimal {
    let code = code.as_ref();
    let mut engine = Engine::new();
    engine
        .load([(source(), code.to_string())])
        .expect("Should parse and plan");
    let response = engine
        .run(None, spec_name, Some(effective), HashMap::new(), None, true)
        .expect("Should evaluate");
    let rule = response
        .results
        .get(rule_name)
        .unwrap_or_else(|| panic!("Rule '{}' not found", rule_name));
    if rule.vetoed {
        panic!(
            "Rule '{}' vetoed: {}",
            rule_name,
            rule.veto_reason.as_deref().unwrap_or("Vetoed")
        );
    }
    *rule
        .result
        .as_ref()
        .and_then(|v| v.measure.as_ref())
        .and_then(|m| m.get(unit))
        .unwrap_or_else(|| panic!("measure map missing unit '{unit}'"))
}

fn eval_bool(
    code: impl AsRef<str>,
    spec_name: &str,
    rule_name: &str,
    effective: &DateTimeValue,
) -> bool {
    match eval_literal(code, spec_name, rule_name, effective).value {
        ValueKind::Boolean(value) => value,
        other => panic!("Expected Boolean, got {:?}", other),
    }
}

fn expect_plan_error(code: impl AsRef<str>, expected_fragment: &str) {
    let code = code.as_ref();
    let mut engine = Engine::new();
    let result = engine.load([(source(), code.to_string())]);
    assert!(result.is_err(), "Expected planning error");
    let combined = result
        .unwrap_err()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ");
    assert!(
        combined
            .to_lowercase()
            .contains(&expected_fragment.to_lowercase()),
        "Expected error containing '{}', got: {}",
        expected_fragment,
        combined
    );
}

fn assert_contains_all(actual: &str, expected_parts: &[&str]) {
    assert!(
        !actual.contains("..."),
        "Expected scalar/date output, got range-like output '{}'",
        actual
    );
    let lower = actual.to_lowercase();
    for part in expected_parts {
        assert!(
            contains_expected_fragment(&lower, &part.to_lowercase()),
            "Expected '{}' to contain '{}'",
            actual,
            part
        );
    }
}

fn contains_expected_fragment(haystack: &str, needle: &str) -> bool {
    if is_numeric_fragment(needle) {
        contains_numeric_fragment(haystack, needle)
    } else {
        haystack.contains(needle)
    }
}

fn contains_numeric_fragment(haystack: &str, needle: &str) -> bool {
    let mut search_from = 0;
    while let Some(relative_index) = haystack[search_from..].find(needle) {
        let index = search_from + relative_index;
        let mut start = index;
        while start > 0 {
            let previous = haystack[..start].chars().next_back().unwrap();
            if !is_numeric_context_character(previous) {
                break;
            }
            start -= previous.len_utf8();
        }

        let mut end = index + needle.len();
        while end < haystack.len() {
            let next = haystack[end..].chars().next().unwrap();
            if !is_numeric_context_character(next) {
                break;
            }
            end += next.len_utf8();
        }

        let candidate = &haystack[start..end];
        if candidate == needle {
            return true;
        }
        if let (Ok(candidate_decimal), Ok(needle_decimal)) = (
            candidate.parse::<rust_decimal::Decimal>(),
            needle.parse::<rust_decimal::Decimal>(),
        ) {
            if candidate_decimal == needle_decimal {
                return true;
            }
        }
        if start == index && end == index + needle.len() {
            return true;
        }
        search_from = index + needle.len();
    }
    false
}

fn is_numeric_fragment(fragment: &str) -> bool {
    let mut has_digit = false;
    for character in fragment.chars() {
        if character.is_ascii_digit() {
            has_digit = true;
            continue;
        }
        if character == '-' || character == '.' {
            continue;
        }
        return false;
    }
    has_digit
}

fn is_numeric_context_character(character: char) -> bool {
    character.is_ascii_digit() || character == '.' || character == '-'
}

#[test]
fn temporal_date_plus_duration() {
    let code = r#"spec test
uses lemma units
rule value: 2024-01-01T00:00:00Z + 2 hour"#;
    let value = eval_rule(code, "test", "value", &effective(2026, 3, 8, 12, 0, 0));
    assert_contains_all(&value, &["2024-01-01", "02:00:00"]);
}

#[test]
fn temporal_date_minus_duration() {
    let code = r#"spec test
uses lemma units
rule value: 2024-01-01T01:00:00Z - 2 hour"#;
    let value = eval_rule(code, "test", "value", &effective(2026, 3, 8, 12, 0, 0));
    assert_contains_all(&value, &["2023-12-31", "23:00:00"]);
}

#[test]
fn temporal_duration_plus_date() {
    let code = r#"spec test
uses lemma units
rule value: 2 hour + 2024-01-01T01:00:00Z"#;
    let value = eval_rule(code, "test", "value", &effective(2026, 3, 8, 12, 0, 0));
    assert_contains_all(&value, &["2024-01-01", "03:00:00"]);
}

#[test]
fn temporal_time_plus_duration() {
    let code = r#"spec test
uses lemma units
rule value: 14:30:00 + 90 minute"#;
    let value = eval_rule(code, "test", "value", &effective(2026, 3, 8, 12, 0, 0));
    assert_contains_all(&value, &["16:00:00"]);
}

#[test]
fn temporal_time_minus_duration() {
    let code = r#"spec test
uses lemma units
rule value: 14:30:00 - 90 minute"#;
    let value = eval_rule(code, "test", "value", &effective(2026, 3, 8, 12, 0, 0));
    assert_contains_all(&value, &["13:00:00"]);
}

#[test]
fn temporal_past_keyword_still_builds_range() {
    let code = r#"spec test
uses lemma units
rule value: (past 7 day) as day as number"#;
    let value = eval_rule(code, "test", "value", &effective(2026, 3, 8, 12, 0, 0));
    assert_contains_all(&value, &["7"]);
}

#[test]
fn temporal_future_keyword_still_builds_range() {
    let code = r#"spec test
uses lemma units
rule value: (future 2 hour) as minute"#;
    assert_eq!(
        eval_rule_measure_unit(
            code,
            "test",
            "value",
            "minute",
            &effective(2026, 3, 8, 12, 0, 0)
        ),
        Decimal::from(120)
    );
}

#[test]
fn temporal_event_in_past_range() {
    let code = r#"spec test
uses lemma units
data event: 2026-03-05T12:00:00Z
rule ok: event in past 7 day"#;
    assert!(eval_bool(
        code,
        "test",
        "ok",
        &effective(2026, 3, 8, 12, 0, 0)
    ));
}

#[test]
fn temporal_event_in_future_range() {
    let code = r#"spec test
uses lemma units
data event: 2026-03-08T13:30:00Z
rule ok: event in future 2 hour"#;
    assert!(eval_bool(
        code,
        "test",
        "ok",
        &effective(2026, 3, 8, 12, 0, 0)
    ));
}

#[test]
fn temporal_explicit_now_minus_duration_range_span() {
    let code = r#"spec test
uses lemma units
rule value: (now - 7 day...now) as day as number"#;
    let value = eval_rule(code, "test", "value", &effective(2026, 3, 8, 12, 0, 0));
    assert_contains_all(&value, &["7"]);
}

#[test]
fn temporal_explicit_now_minus_duration_range_contains_start_boundary() {
    let code = r#"spec test
uses lemma units
data event: 2026-03-01T12:00:00Z
rule ok: event in now - 7 day...now"#;
    assert!(eval_bool(
        code,
        "test",
        "ok",
        &effective(2026, 3, 8, 12, 0, 0)
    ));
}

#[test]
fn temporal_explicit_now_minus_duration_range_excludes_end_boundary() {
    let code = r#"spec test
uses lemma units
data event: 2026-03-08T12:00:00Z
rule ok: event in now - 7 day...now"#;
    assert!(!eval_bool(
        code,
        "test",
        "ok",
        &effective(2026, 3, 8, 12, 0, 0)
    ));
}

#[test]
fn temporal_explicit_now_minus_duration_range_rejects_just_outside_left() {
    let code = r#"spec test
uses lemma units
data event: 2026-03-01T11:59:59Z
rule ok: event in now - 7 day...now"#;
    assert!(!eval_bool(
        code,
        "test",
        "ok",
        &effective(2026, 3, 8, 12, 0, 0)
    ));
}

#[test]
fn temporal_explicit_range_matches_past_keyword_for_inside_case() {
    let code = r#"spec test
uses lemma units
data event: 2026-03-05T12:00:00Z
rule past_check: event in past 7 day
rule range_check: event in now - 7 day...now"#;
    let effective = effective(2026, 3, 8, 12, 0, 0);
    let past_check = eval_bool(code, "test", "past_check", &effective);
    let range_check = eval_bool(code, "test", "range_check", &effective);
    assert_eq!(past_check, range_check);
    assert!(past_check);
}

#[test]
fn temporal_explicit_range_matches_past_keyword_for_outside_case() {
    let code = r#"spec test
uses lemma units
data event: 2026-02-28T11:59:59Z
rule past_check: event in past 7 day
rule range_check: event in now - 7 day...now"#;
    let effective = effective(2026, 3, 8, 12, 0, 0);
    let past_check = eval_bool(code, "test", "past_check", &effective);
    let range_check = eval_bool(code, "test", "range_check", &effective);
    assert_eq!(past_check, range_check);
    assert!(!past_check);
}

#[test]
fn temporal_calendar_future_behavior_remains_independent() {
    let code = r#"spec test
uses lemma units
data event: 2026-04-01T12:00:00Z
rule ok: event in future 1 month"#;
    assert!(eval_bool(
        code,
        "test",
        "ok",
        &effective(2026, 3, 8, 12, 0, 0)
    ));
}

#[test]
fn temporal_date_plus_unrelated_measure_rejected() {
    let code = format!(
        r#"spec test
uses lemma units
{money}
rule value: 2024-01-01 + 2 eur"#,
        money = MONEY_TYPEDEF
    );
    expect_plan_error(code, "date");
}

#[test]
fn temporal_time_plus_unrelated_measure_rejected() {
    let code = format!(
        r#"spec test
uses lemma units
{money}
rule value: 14:30:00 + 2 eur"#,
        money = MONEY_TYPEDEF
    );
    expect_plan_error(code, "time");
}

#[test]
fn temporal_past_without_visible_duration_typedef_rejected() {
    let code = r#"spec test
rule value: past 7 day"#;
    expect_plan_error(code, "Unknown unit");
}

#[test]
fn temporal_explicit_now_minus_duration_range_without_visible_typedef_rejected() {
    let code = r#"spec test
rule value: now - 7 day...now"#;
    expect_plan_error(code, "Unknown unit");
}

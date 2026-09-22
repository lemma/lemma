use lemma::{DateGranularity, DateTimeValue, Engine, TimezoneValue, ValueKind};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

fn source() -> lemma::SourceType {
    lemma::SourceType::Path(Arc::new(PathBuf::from("test.lemma")))
}

fn effective_us(y: i32, m: u32, d: u32, h: u32, min: u32, s: u32, us: u32) -> DateTimeValue {
    DateTimeValue {
        year: y,
        month: m,
        day: d,
        hour: h,
        minute: min,
        second: s,
        microsecond: us,
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
fn precision_duration_microsecond_addition_exact() {
    let code = r#"spec test
uses lemma units
rule value: ((1 microsecond + 999 microsecond) as millisecond)"#;
    let value = eval_rule(
        code,
        "test",
        "value",
        &effective_us(2026, 3, 8, 12, 0, 0, 0),
    );
    assert_contains_all(&value, &["1", "millisecond"]);
}

#[test]
fn precision_duration_microsecond_to_millisecond_fraction() {
    let code = r#"spec test
uses lemma units
rule value: 1500 microsecond as millisecond"#;
    let value = eval_rule(
        code,
        "test",
        "value",
        &effective_us(2026, 3, 8, 12, 0, 0, 0),
    );
    assert_contains_all(&value, &["1.5", "millisecond"]);
}

#[test]
fn precision_second_to_millisecond_exact() {
    let code = r#"spec test
uses lemma units
rule value: 0.001 second as millisecond"#;
    let value = eval_rule(
        code,
        "test",
        "value",
        &effective_us(2026, 3, 8, 12, 0, 0, 0),
    );
    assert_contains_all(&value, &["1", "millisecond"]);
}

#[test]
fn precision_datetime_range_to_microseconds() {
    let code = r#"spec test
uses lemma units
rule value: (2024-01-01T00:00:00.000001Z...2024-01-01T00:00:00.000003Z) as microsecond"#;
    let value = eval_rule(
        code,
        "test",
        "value",
        &effective_us(2026, 3, 8, 12, 0, 0, 0),
    );
    assert_contains_all(&value, &["2", "microsecond"]);
}

#[test]
fn precision_reversed_datetime_range_span_is_absolute_microseconds() {
    let code = r#"spec test
uses lemma units
rule value: (2024-01-01T00:00:00.000003Z...2024-01-01T00:00:00.000001Z) as microsecond"#;
    let value = eval_rule(
        code,
        "test",
        "value",
        &effective_us(2026, 3, 8, 12, 0, 0, 0),
    );
    assert_contains_all(&value, &["2", "microsecond"]);
}

#[test]
fn precision_time_minus_time_rejected_with_datetime_range_suggestion() {
    let code = r#"spec test
uses lemma units
rule value: (00:00:00.000001 - 00:00:00.000000) as microsecond"#;
    expect_plan_error(code, "datetime range");
}

#[test]
fn precision_datetime_plus_microsecond() {
    let code = r#"spec test
uses lemma units
rule value: 2024-01-01T00:00:00.000001Z + 1 microsecond"#;
    let value = eval_rule(
        code,
        "test",
        "value",
        &effective_us(2026, 3, 8, 12, 0, 0, 0),
    );
    assert_contains_all(&value, &["2024-01-01", "00:00:00.000002"]);
}

#[test]
fn precision_datetime_crosses_second_boundary_exactly() {
    let code = r#"spec test
uses lemma units
rule value: 2024-01-01T00:00:00.999999Z + 2 microsecond"#;
    let value = eval_rule(
        code,
        "test",
        "value",
        &effective_us(2026, 3, 8, 12, 0, 0, 0),
    );
    assert_contains_all(&value, &["2024-01-01", "00:00:01.000001"]);
}

#[test]
fn precision_past_one_microsecond_includes_exact_left_boundary() {
    let code = r#"spec test
uses lemma units
data event: 2026-03-08T12:00:00.000001Z
rule ok: event in past 1 microsecond"#;
    assert!(eval_bool(
        code,
        "test",
        "ok",
        &effective_us(2026, 3, 8, 12, 0, 0, 2)
    ));
}

#[test]
fn precision_explicit_now_minus_one_microsecond_range_includes_start() {
    let code = r#"spec test
uses lemma units
data event: 2026-03-08T12:00:00.000001Z
rule ok: event in now - 1 microsecond...now"#;
    assert!(eval_bool(
        code,
        "test",
        "ok",
        &effective_us(2026, 3, 8, 12, 0, 0, 2)
    ));
}

#[test]
fn precision_explicit_now_minus_one_microsecond_range_excludes_just_outside() {
    let code = r#"spec test
uses lemma units
data event: 2026-03-08T12:00:00.000000Z
rule ok: event in now - 1 microsecond...now"#;
    assert!(!eval_bool(
        code,
        "test",
        "ok",
        &effective_us(2026, 3, 8, 12, 0, 0, 2)
    ));
}

#[test]
fn precision_future_one_microsecond_excludes_upper_boundary() {
    let code = r#"spec test
uses lemma units
data event: 2026-03-08T12:00:00.000001Z
rule ok: event in future 1 microsecond"#;
    assert!(!eval_bool(
        code,
        "test",
        "ok",
        &effective_us(2026, 3, 8, 12, 0, 0, 0)
    ));
}

#[test]
fn precision_time_literal_without_subseconds_is_still_compatible() {
    let code = r#"spec test
uses lemma units
rule value: 14:30:00 + 1 microsecond"#;
    let value = eval_rule(
        code,
        "test",
        "value",
        &effective_us(2026, 3, 8, 12, 0, 0, 0),
    );
    assert_contains_all(&value, &["14:30:00.000001"]);
}

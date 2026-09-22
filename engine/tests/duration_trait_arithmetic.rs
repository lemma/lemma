use lemma::DateTimeValue;
use lemma::{Engine, ValueKind};
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

fn eval_literal(code: impl AsRef<str>, spec_name: &str, rule_name: &str) -> lemma::LiteralValue {
    let code = code.as_ref();
    let mut engine = Engine::new();
    engine
        .load([(source(), code.to_string())])
        .expect("Should parse and plan");
    let now = DateTimeValue::now();
    let response = engine
        .run(None, spec_name, Some(&now), HashMap::new(), None, true)
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

fn eval_rule(code: impl AsRef<str>, spec_name: &str, rule_name: &str) -> String {
    let code = code.as_ref();
    let mut engine = Engine::new();
    engine
        .load([(source(), code.to_string())])
        .expect("Should parse and plan");
    let now = DateTimeValue::now();
    let response = engine
        .run(None, spec_name, Some(&now), HashMap::new(), None, true)
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
) -> Decimal {
    let code = code.as_ref();
    let mut engine = Engine::new();
    engine
        .load([(source(), code.to_string())])
        .expect("Should parse and plan");
    let now = DateTimeValue::now();
    let response = engine
        .run(None, spec_name, Some(&now), HashMap::new(), None, true)
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

fn eval_bool(code: impl AsRef<str>, spec_name: &str, rule_name: &str) -> bool {
    match eval_literal(code, spec_name, rule_name).value {
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
fn arith_add_mixed_aliases() {
    let code = r#"spec test
uses lemma units
rule value: (2 hour + 30 minute) as minute"#;
    let value = eval_rule(code, "test", "value");
    assert_contains_all(&value, &["150", "minute"]);
}

#[test]
fn arith_subtract_mixed_aliases() {
    let code = r#"spec test
uses lemma units
rule value: (2 hour - 30 minute) as minute"#;
    let value = eval_rule(code, "test", "value");
    assert_contains_all(&value, &["90", "minute"]);
}

#[test]
fn arith_negative_duration_result() {
    let code = r#"spec test
uses lemma units
rule value: (30 minute - 2 hour) as minute"#;
    let value = eval_rule(code, "test", "value");
    assert_contains_all(&value, &["-90", "minute"]);
}

#[test]
fn arith_duration_times_number() {
    let code = r#"spec test
uses lemma units
rule value: (2 hour * 3) as hour as number"#;
    let value = eval_rule(code, "test", "value");
    assert_contains_all(&value, &["6"]);
}

#[test]
fn arith_number_times_duration() {
    let code = r#"spec test
uses lemma units
rule value: (3 * 2 hour) as hour as number"#;
    let value = eval_rule(code, "test", "value");
    assert_contains_all(&value, &["6"]);
}

#[test]
fn arith_duration_divided_by_number() {
    let code = r#"spec test
uses lemma units
rule value: (2 hour / 2) as minute"#;
    assert_eq!(
        eval_rule_measure_unit(code, "test", "value", "minute"),
        Decimal::from(60)
    );
}

#[test]
fn arith_duration_divided_by_duration_yields_number() {
    let code = r#"spec test
uses lemma units
rule value: 2 hour / 30 minute"#;
    assert_eq!(eval_rule(code, "test", "value"), "4");
}

#[test]
fn arith_named_duration_compare_same_alias_family() {
    let code = r#"spec test
uses lemma units
rule ok: 2 hour > 90 minute"#;
    assert!(eval_bool(code, "test", "ok"));
}

#[test]
fn arith_named_duration_compare_equal_after_conversion() {
    let code = r#"spec test
uses lemma units
rule ok: 2 hour >= 120 minute"#;
    assert!(eval_bool(code, "test", "ok"));
}

#[test]
fn arith_number_cast_to_duration_for_comparison() {
    let code = r#"spec test
uses lemma units
data threshold: 1.5
rule ok: (2 hour as hour as number) > threshold"#;
    assert!(eval_bool(code, "test", "ok"));
}

#[test]
fn arith_named_duration_compare_to_bare_number_rejected() {
    let code = r#"spec test
uses lemma units
rule ok: 2 hour > 5"#;
    expect_plan_error(code, "number");
}

#[test]
fn arith_named_duration_plus_calendar_rejected() {
    let code = r#"spec test
uses lemma units
rule value: 2 hour + 3 month"#;
    expect_plan_error(code, "calendar");
}

#[test]
fn arith_named_duration_plus_unrelated_measure_rejected() {
    let code = format!(
        r#"spec test
uses lemma units
{money}
rule value: 2 hour + 5 eur"#,
        money = MONEY_TYPEDEF
    );
    expect_plan_error(code, "unrelated");
}

#[test]
fn arith_named_duration_cast_to_unrelated_unit_rejected() {
    let code = format!(
        r#"spec test
uses lemma units
{money}
data elapsed: units.duration
data d: elapsed -> suggest 2 hour
rule value: d as eur"#,
        money = MONEY_TYPEDEF
    );
    expect_plan_error(code, "different measure families");
}

#[test]
fn arith_bare_number_not_implicitly_duration_compatible() {
    let code = r#"spec test
uses lemma units
data threshold: 1.5
rule ok: 2 hour > threshold"#;
    expect_plan_error(code, "Cannot compare");
}

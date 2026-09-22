use lemma::{DateGranularity, DateTimeValue, Engine, LiteralValue, TimezoneValue, ValueKind};
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

fn source() -> lemma::SourceType {
    lemma::SourceType::Path(Arc::new(PathBuf::from("test.lemma")))
}

fn default_effective() -> DateTimeValue {
    DateTimeValue {
        year: 2026,
        month: 3,
        day: 7,
        hour: 12,
        minute: 0,
        second: 0,
        microsecond: 0,
        timezone: Some(TimezoneValue {
            offset_hours: 0,
            offset_minutes: 0,
        }),
        granularity: DateGranularity::DateTime,
    }
}

fn eval_literal(code: &str, spec_name: &str, rule_name: &str) -> LiteralValue {
    let mut engine = Engine::new();
    engine
        .load([(source(), code.to_string())])
        .expect("Should parse and plan");
    let effective = default_effective();
    let response = engine
        .run(
            None,
            spec_name,
            Some(&effective),
            HashMap::new(),
            Some(&[rule_name.to_string()]),
            true,
        )
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

fn eval_rule(code: &str, spec_name: &str, rule_name: &str) -> String {
    let mut engine = Engine::new();
    engine
        .load([(source(), code.to_string())])
        .expect("Should parse and plan");
    let effective = default_effective();
    let response = engine
        .run(
            None,
            spec_name,
            Some(&effective),
            HashMap::new(),
            Some(&[rule_name.to_string()]),
            true,
        )
        .expect("Should evaluate");
    response
        .results
        .get(rule_name)
        .unwrap_or_else(|| panic!("Rule '{}' not found", rule_name))
        .result()
        .expect("result")
        .to_string()
}

fn eval_rule_measure_unit(code: &str, spec_name: &str, rule_name: &str, unit: &str) -> Decimal {
    let mut engine = Engine::new();
    engine
        .load([(source(), code.to_string())])
        .expect("Should parse and plan");
    let effective = default_effective();
    let response = engine
        .run(
            None,
            spec_name,
            Some(&effective),
            HashMap::new(),
            Some(&[rule_name.to_string()]),
            true,
        )
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
        .unwrap_or_else(|| panic!("measure map missing unit '{unit}' for rule '{rule_name}'"))
}

fn eval_bool(code: &str, spec_name: &str, rule_name: &str) -> bool {
    match eval_literal(code, spec_name, rule_name).value {
        ValueKind::Boolean(val) => val,
        other => panic!("Expected Boolean, got {:?}", other),
    }
}

fn expect_plan_error(code: &str, expected_fragment: &str) {
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
        combined.contains(expected_fragment),
        "Expected error containing '{}', got: {}",
        expected_fragment,
        combined
    );
}

fn assert_contains_all(haystack: &str, needles: &[&str]) {
    assert!(
        !haystack.contains("..."),
        "Expected scalar/date output, got range-like output '{}'",
        haystack
    );
    for needle in needles {
        assert!(
            contains_expected_fragment(haystack, needle),
            "Expected '{}' to contain '{}'",
            haystack,
            needle
        );
    }
}

fn contains_expected_fragment(haystack: &str, needle: &str) -> bool {
    let haystack = haystack.to_lowercase();
    let needle = needle.to_lowercase();
    if is_numeric_fragment(&needle) {
        contains_numeric_fragment(&haystack, &needle)
    } else {
        haystack.contains(&needle)
    }
}

fn contains_numeric_fragment(haystack: &str, needle: &str) -> bool {
    let mut search_from = 0;
    while let Some(relative_index) = haystack[search_from..].find(needle) {
        let index = search_from + relative_index;
        let before = haystack[..index].chars().next_back();
        let after = haystack[index + needle.len()..].chars().next();
        let before_ok = before.is_none_or(|character| !is_numeric_context_character(character));
        let after_ok = after.is_none_or(|character| !is_numeric_context_character(character));
        if before_ok && after_ok {
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
fn number_containment_is_half_open_and_reversed() {
    let code = r#"spec test
rule start: 3 in 3...5
rule end: 5 in 3...5
rule outside: 2 in 3...5
rule point: 5 in 5...5
rule reversed: 4 in 5...3"#;
    assert!(eval_bool(code, "test", "start"));
    assert!(!eval_bool(code, "test", "end"));
    assert!(!eval_bool(code, "test", "outside"));
    assert!(!eval_bool(code, "test", "point"));
    assert!(eval_bool(code, "test", "reversed"));
}

#[test]
fn number_mixed_addition_uses_range_size() {
    let code = r#"spec test
rule left: (3...5) + 2
rule right: 2 + (3...5)"#;
    assert_eq!(eval_rule(code, "test", "left"), "4");
    assert_eq!(eval_rule(code, "test", "right"), "4");
}

#[test]
fn number_mixed_subtraction_uses_range_size() {
    let code = r#"spec test
rule left: (3...5) - 2
rule right: 10 - (3...5)"#;
    assert_eq!(eval_rule(code, "test", "left"), "0");
    assert_eq!(eval_rule(code, "test", "right"), "8");
}

#[test]
fn number_range_span_is_independent_of_endpoint_order() {
    let code = r#"spec test
rule forward: (3...5) - (6...7)
rule reversed: (5...3) - (7...6)"#;
    assert_eq!(
        eval_rule(code, "test", "forward"),
        eval_rule(code, "test", "reversed")
    );
}

#[test]
fn number_range_arithmetic_and_comparison_use_sizes() {
    let code = r#"spec test
rule sum: (3...5) + (6...7)
rule diff: (3...5) - (6...7)
rule gte: (3...5) >= 2
rule gt: (3...5) > 2
rule reversed_add: (5...3) + 2
rule reversed_cmp: (5...3) >= 0"#;
    assert_eq!(eval_rule(code, "test", "sum"), "3");
    assert_eq!(eval_rule(code, "test", "diff"), "1");
    assert!(eval_bool(code, "test", "gte"));
    assert!(!eval_bool(code, "test", "gt"));
    assert_eq!(eval_rule(code, "test", "reversed_add"), "4");
    assert!(eval_bool(code, "test", "reversed_cmp"));
}

#[test]
fn measure_mixed_arithmetic_uses_range_size() {
    let code = r#"spec test
data weight: measure -> unit gram: 1 -> unit kilogram: 1000
rule plus: (3 kilogram...5 kilogram) + 2 kilogram
rule rplus: 2 kilogram + (3 kilogram...5 kilogram)
rule minus: (3 kilogram...5 kilogram) - 2 kilogram
rule rminus: 5 kilogram - (3 kilogram...5 kilogram)"#;
    assert_eq!(
        eval_rule_measure_unit(code, "test", "plus", "kilogram"),
        Decimal::from(4)
    );
    assert_eq!(
        eval_rule_measure_unit(code, "test", "rplus", "kilogram"),
        Decimal::from(4)
    );
    assert_eq!(
        eval_rule_measure_unit(code, "test", "minus", "kilogram"),
        Decimal::from(0)
    );
    assert_eq!(
        eval_rule_measure_unit(code, "test", "rminus", "kilogram"),
        Decimal::from(3)
    );
}

#[test]
fn measure_range_arithmetic_and_comparison_use_sizes() {
    let code = r#"spec test
data weight: measure -> unit gram: 1 -> unit kilogram: 1000
rule sum: (3 kilogram...5 kilogram) + (6 kilogram...7 kilogram)
rule diff: (3 kilogram...5 kilogram) - (6 kilogram...7 kilogram)
rule cmp: (3 kilogram...5 kilogram) >= 2 kilogram"#;
    // Span result promotes via signature_index to the canonical unit (gram).
    assert_contains_all(&eval_rule(code, "test", "sum"), &["3000", "gram"]);
    assert_contains_all(&eval_rule(code, "test", "diff"), &["1000", "gram"]);
    assert!(eval_bool(code, "test", "cmp"));
}

#[test]
fn ratio_mixed_arithmetic_uses_range_size() {
    let code = r#"spec test
rule plus: (10%...50%) + 5%
rule rplus: 5% + (10%...50%)
rule minus: (10%...50%) - 40%
rule rminus: 60% - (10%...50%)"#;
    assert_eq!(eval_rule(code, "test", "plus"), "45%");
    assert_eq!(eval_rule(code, "test", "rplus"), "45%");
    assert_eq!(eval_rule(code, "test", "minus"), "0%");
    assert_eq!(eval_rule(code, "test", "rminus"), "20%");
}

#[test]
fn ratio_range_arithmetic_and_comparison_use_sizes() {
    let code = r#"spec test
rule sum: (10%...50%) + (20%...30%)
rule diff: (10%...50%) - (20%...30%)
rule cmp: (10%...50%) >= 40%"#;
    assert_eq!(eval_rule(code, "test", "sum"), "50%");
    assert_eq!(eval_rule(code, "test", "diff"), "30%");
    assert!(eval_bool(code, "test", "cmp"));
}

#[test]
fn date_mixed_duration_arithmetic_uses_range_size() {
    let code = r#"spec test
uses lemma units
rule plus: (2024-02-15...2024-03-15 + 1 day) as day as number
rule minus: (2024-02-15...2024-03-15 - 1 day) as day as number
rule rminus: (40 day - (2024-02-15...2024-03-15)) as day as number"#;
    assert_contains_all(&eval_rule(code, "test", "plus"), &["30"]);
    assert_contains_all(&eval_rule(code, "test", "minus"), &["28"]);
    assert_contains_all(&eval_rule(code, "test", "rminus"), &["11"]);
}

#[test]
fn date_range_arithmetic_and_comparison_use_sizes() {
    let code = r#"spec test
uses lemma units
rule sum: ((2024-01-01...2024-01-03) + (2024-02-01...2024-02-02)) as day as number
rule diff: ((2024-01-01...2024-01-03) - (2024-02-01...2024-02-02)) as day as number
rule cmp: (2024-01-01...2024-01-03) >= 2 day
rule reversed: (2024-01-03...2024-01-01) as day as number"#;
    assert_contains_all(&eval_rule(code, "test", "sum"), &["3"]);
    assert_contains_all(&eval_rule(code, "test", "diff"), &["1"]);
    assert!(eval_bool(code, "test", "cmp"));
    assert_contains_all(&eval_rule(code, "test", "reversed"), &["2"]);
}

#[test]
fn measure_range_arithmetic_rejects_mixed_families() {
    let code = r#"spec test
data weight: measure -> unit gram: 1 -> unit kilogram: 1000
data money: measure -> unit eur: 1
rule bad: (3 kilogram...5 kilogram) + (6 eur...7 eur)"#;
    expect_plan_error(code, "Cannot apply");
}

#[test]
fn invalid_range_construction_is_rejected() {
    let code = r#"spec test
rule bad: 2024-01-01...100"#;
    expect_plan_error(code, "range");
}

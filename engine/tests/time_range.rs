//! Time range: containment, span as duration, planning rejections.

use lemma::{DateGranularity, DateTimeValue, Engine, LiteralValue, TimezoneValue, ValueKind};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

fn source() -> lemma::SourceType {
    lemma::SourceType::Path(Arc::new(PathBuf::from("time_range.lemma")))
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

fn eval_bool(code: &str, spec_name: &str, rule_name: &str) -> bool {
    eval_bool_with_data(code, spec_name, rule_name, HashMap::new())
}

fn eval_bool_with_data(
    code: &str,
    spec_name: &str,
    rule_name: &str,
    data: HashMap<String, String>,
) -> bool {
    match eval_literal_with_data(code, spec_name, rule_name, data).value {
        ValueKind::Boolean(val) => val,
        other => panic!("Expected Boolean, got {:?}", other),
    }
}

fn eval_literal_with_data(
    code: &str,
    spec_name: &str,
    rule_name: &str,
    data: HashMap<String, String>,
) -> LiteralValue {
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
            data,
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

const USES_UNITS: &str = "uses lemma units";

#[test]
fn declared_time_range_default_and_containment() {
    let code = r#"spec test
data window: time range -> suggest 09:00...17:00
rule inside: 12:30 in window
rule lower: 09:00 in window
rule upper_excluded: 17:00 in window"#;
    let mut data = HashMap::new();
    data.insert("window".to_string(), "09:00...17:00".into());
    assert!(eval_bool_with_data(code, "test", "inside", data.clone()));
    assert!(eval_bool_with_data(code, "test", "lower", data.clone()));
    assert!(!eval_bool_with_data(code, "test", "upper_excluded", data));
}

#[test]
fn span_as_hours() {
    let code = format!(
        r#"spec test
{USES_UNITS}
rule span: (09:00...17:00) as hour as number"#
    );
    let out = eval_rule(&code, "test", "span");
    assert!(out.contains('8'), "expected span 8 hour, got {out}");
}

#[test]
fn span_as_minutes() {
    let code = format!(
        r#"spec test
{USES_UNITS}
rule span: (09:00...17:00) as minute as number"#
    );
    let out = eval_rule(&code, "test", "span");
    assert!(out.contains("480"), "expected span 480 minute, got {out}");
}

#[test]
fn rejects_bare_as_number() {
    let code = r#"spec test
rule bad: (09:00...17:00) as number"#;
    expect_plan_error(code, "as number");
}

#[test]
fn rejects_span_as_calendar_unit() {
    let code = format!(
        r#"spec test
{USES_UNITS}
rule bad: (09:00...17:00) as year as number"#
    );
    expect_plan_error(&code, "convert");
}

#[test]
fn rejects_mixed_timezones() {
    let code = r#"spec test
rule bad: 09:00Z...17:00+01:00"#;
    expect_plan_error(code, "timezone");
}

#[test]
fn reversed_clock_order_half_open_no_wraparound() {
    let code = format!(
        r#"spec test
{USES_UNITS}
rule span: (22:00...02:00) as hour as number
rule three_inside: 03:00 in 22:00...02:00
rule ten_inside: 10:00 in 22:00...02:00
rule midnight_outside: 01:00 in 22:00...02:00
rule late_outside: 23:00 in 22:00...02:00"#
    );
    let out = eval_rule(&code, "test", "span");
    assert!(out.contains("20"), "expected 20h span, got {out}");
    assert!(eval_bool(&code, "test", "three_inside"));
    assert!(eval_bool(&code, "test", "ten_inside"));
    assert!(!eval_bool(&code, "test", "midnight_outside"));
    assert!(!eval_bool(&code, "test", "late_outside"));
}

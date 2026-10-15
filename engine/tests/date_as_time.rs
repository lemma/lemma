//! `date as time` and `date in time range` sugar.

use indexmap::IndexMap;
use lemma::{
    DateGranularity, DateTimeValue, Engine, EvaluateDataValue, LiteralValue, TimezoneValue,
    ValueKind,
};
use std::path::PathBuf;
use std::sync::Arc;

fn source() -> lemma::SourceType {
    lemma::SourceType::Path(Arc::new(PathBuf::from("date_as_time.lemma")))
}

fn effective_at(hour: u32, minute: u32) -> DateTimeValue {
    DateTimeValue {
        year: 2026,
        month: 3,
        day: 7,
        hour,
        minute,
        second: 0,
        microsecond: 0,
        timezone: Some(TimezoneValue { offset_minutes: 0 }),
        granularity: DateGranularity::DateTime,
    }
}

fn eval_literal(
    code: &str,
    spec_name: &str,
    rule_name: &str,
    effective: &DateTimeValue,
    data: IndexMap<String, EvaluateDataValue>,
) -> LiteralValue {
    let mut engine = Engine::new();
    engine
        .load([(source(), code.to_string())])
        .expect("Should parse and plan");
    let response = engine
        .evaluate(
            None,
            spec_name,
            Some(effective),
            data,
            Some(&[rule_name.to_string()]),
            true,
        )
        .expect("Should evaluate");
    let rule = response
        .results
        .get(rule_name)
        .unwrap_or_else(|| panic!("Rule '{rule_name}' not found"));
    if rule.vetoed {
        panic!(
            "Rule '{rule_name}' vetoed: {}",
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

fn eval_bool(
    code: &str,
    spec_name: &str,
    rule_name: &str,
    effective: &DateTimeValue,
    data: IndexMap<String, EvaluateDataValue>,
) -> bool {
    match eval_literal(code, spec_name, rule_name, effective, data).value {
        ValueKind::Boolean(val) => val,
        other => panic!("Expected Boolean, got {other:?}"),
    }
}

fn eval_time_display(
    code: &str,
    spec_name: &str,
    rule_name: &str,
    effective: &DateTimeValue,
) -> String {
    match eval_literal(code, spec_name, rule_name, effective, IndexMap::new()).value {
        ValueKind::Time(t) => t.to_string(),
        other => panic!("Expected Time, got {other:?}"),
    }
}

#[test]
fn now_as_time_projects_clock() {
    let code = r#"spec test
rule clock: now as time"#;
    let out = eval_time_display(code, "test", "clock", &effective_at(14, 30));
    assert_eq!(out, "14:30:00Z");
}

#[test]
fn date_only_as_time_is_midnight() {
    let code = r#"spec test
rule clock: 2024-06-01 as time"#;
    let out = eval_time_display(code, "test", "clock", &effective_at(12, 0));
    assert_eq!(out, "00:00:00");
}

#[test]
fn datetime_literal_as_time_keeps_timezone() {
    let code = r#"spec test
rule clock: 2024-06-01T09:15:00+02:00 as time"#;
    let out = eval_time_display(code, "test", "clock", &effective_at(12, 0));
    assert_eq!(out, "09:15:00+02:00");
}

#[test]
fn now_as_time_in_shift() {
    let code = r#"spec test
data shift: time range
rule in_evening: now as time in shift"#;
    let mut data = IndexMap::new();
    data.insert("shift".to_string(), "09:00...17:00".into());
    assert!(eval_bool(
        code,
        "test",
        "in_evening",
        &effective_at(12, 0),
        data.clone()
    ));
    assert!(!eval_bool(
        code,
        "test",
        "in_evening",
        &effective_at(18, 0),
        data
    ));
}

#[test]
fn now_in_shift_sugar() {
    let code = r#"spec test
data shift: time range
rule in_evening: now in shift"#;
    let mut data = IndexMap::new();
    data.insert("shift".to_string(), "09:00...17:00".into());
    assert!(eval_bool(
        code,
        "test",
        "in_evening",
        &effective_at(12, 0),
        data.clone()
    ));
    assert!(!eval_bool(
        code,
        "test",
        "in_evening",
        &effective_at(18, 0),
        data
    ));
}

#[test]
fn date_literal_in_time_range_uses_midnight() {
    let code = r#"spec test
rule early: 2024-06-01 in 00:00...01:00
rule late: 2024-06-01 in 09:00...17:00"#;
    assert!(eval_bool(
        code,
        "test",
        "early",
        &effective_at(12, 0),
        IndexMap::new()
    ));
    assert!(!eval_bool(
        code,
        "test",
        "late",
        &effective_at(12, 0),
        IndexMap::new()
    ));
}

#[test]
fn time_as_time_is_identity() {
    let code = r#"spec test
rule clock: 09:30:00 as time"#;
    let out = eval_time_display(code, "test", "clock", &effective_at(12, 0));
    assert_eq!(out, "09:30:00");
}

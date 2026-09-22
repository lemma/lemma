//! `-> fill` sets a committed value on a typed data slot (overridable by caller).
//! Distinct from `-> suggest` (UI hint, never committed).

use lemma::{DateTimeValue, Engine, SourceType};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

fn path_source(file: &str) -> SourceType {
    SourceType::Path(Arc::new(PathBuf::from(file)))
}

fn load_ok(code: &str) -> Engine {
    let mut engine = Engine::new();
    engine
        .load([(path_source("test.lemma"), code.to_string())])
        .unwrap_or_else(|errs| {
            let joined = errs
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("\n");
            panic!("expected load to succeed, got: {joined}");
        });
    engine
}

fn load_err_joined(code: &str) -> String {
    let mut engine = Engine::new();
    let err = engine
        .load([(path_source("test.lemma"), code.to_string())])
        .expect_err("expected load to fail");
    err.iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

fn eval_rule(engine: &mut Engine, spec: &str, data: HashMap<String, String>, rule: &str) -> String {
    let now = DateTimeValue::now();
    let response = engine
        .run(None, spec, Some(&now), data, None, false)
        .expect("run");
    response
        .results
        .get(rule)
        .unwrap_or_else(|| panic!("rule '{rule}' missing"))
        .result()
        .expect("result")
        .to_string()
}

#[test]
fn fill_number_evaluates() {
    let code = r#"spec test
data n: number -> fill 42
rule r: n"#;
    let mut engine = load_ok(code);
    let result = eval_rule(&mut engine, "test", HashMap::new(), "r");
    assert_eq!(result, "42");
}

#[test]
fn fill_measure_evaluates() {
    let code = r#"spec test
uses lemma units
data age: calendar -> fill 25 year
rule r: age as year"#;
    let mut engine = load_ok(code);
    let result = eval_rule(&mut engine, "test", HashMap::new(), "r");
    assert_eq!(result, "25 year");
}

#[test]
fn fill_overridden_by_caller() {
    let code = r#"spec test
uses lemma units
data age: calendar -> fill 25 year
rule r: age as year"#;
    let mut engine = load_ok(code);
    let mut data = HashMap::new();
    data.insert("age".to_string(), "30 year".into());
    let result = eval_rule(&mut engine, "test", data, "r");
    assert_eq!(result, "30 year");
}

#[test]
fn fill_boolean() {
    let code = r#"spec test
data active: boolean -> fill true
rule r: active"#;
    let mut engine = load_ok(code);
    let result = eval_rule(&mut engine, "test", HashMap::new(), "r");
    assert_eq!(result, "true");
}

#[test]
fn fill_text_with_options() {
    let code = r#"spec test
data status: text
  -> option "a"
  -> option "b"
  -> fill "a"
rule r: status"#;
    let mut engine = load_ok(code);
    let result = eval_rule(&mut engine, "test", HashMap::new(), "r");
    assert_eq!(result, "a");
}

#[test]
fn fill_violating_minimum_is_planning_error() {
    let code = r#"spec test
data n: number -> minimum 10 -> fill 5
rule r: n"#;
    let joined = load_err_joined(code);
    assert!(
        joined.to_lowercase().contains("fill")
            || joined.to_lowercase().contains("minimum")
            || joined.to_lowercase().contains("suggestion")
            || joined.to_lowercase().contains("invalid"),
        "expected planning error for fill below minimum, got: {joined}"
    );
}

#[test]
fn fill_and_suggest_mutually_exclusive() {
    let code = r#"spec test
data n: number -> fill 1 -> suggest 2
rule r: n"#;
    let joined = load_err_joined(code);
    assert!(
        joined.to_lowercase().contains("fill") && joined.to_lowercase().contains("suggest"),
        "expected mutual exclusion error mentioning fill and suggest, got: {joined}"
    );
}

#[test]
fn fill_on_imported_type() {
    let code = r#"spec test
uses lemma units
data age: calendar -> fill 25 year
rule r: age as year"#;
    let mut engine = load_ok(code);
    let result = eval_rule(&mut engine, "test", HashMap::new(), "r");
    assert_eq!(result, "25 year");
}

#[test]
fn show_exposes_fill_not_suggestion() {
    let code = r#"spec test
data n: number -> fill 42
rule r: n"#;
    let engine = load_ok(code);
    let now = DateTimeValue::now();
    let entry = engine
        .show(None, "test", Some(&now))
        .expect("show")
        .data
        .get("n")
        .expect("n in show")
        .clone();
    assert!(entry.fill.is_some(), "-> fill must surface as fill on show");
    assert!(
        entry.suggestion.is_none(),
        "-> fill must not surface as suggestion"
    );
    assert_eq!(
        entry.fill.as_ref().and_then(|v| v.number),
        Some(rust_decimal::Decimal::from(42))
    );
}

#[test]
fn fill_ratio() {
    let code = r#"spec test
data rate: ratio -> fill 15%
rule r: rate"#;
    let mut engine = load_ok(code);
    let result = eval_rule(&mut engine, "test", HashMap::new(), "r");
    assert_eq!(result, "15%");
}

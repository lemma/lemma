//! Unqualified imported type resolution: `data age: calendar` resolves to an
//! imported type when exactly one import exports that name.

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

// ---------------------------------------------------------------------------
// 1. Bare imported type resolves and evaluates
// ---------------------------------------------------------------------------
#[test]
fn unqualified_imported_type_resolves() {
    let code = r#"spec test
uses lemma units
data age: calendar
rule value: age as year"#;
    let mut engine = load_ok(code);
    let mut data = HashMap::new();
    data.insert("age".to_string(), "42 year".into());
    let result = eval_rule(&mut engine, "test", data, "value");
    assert_eq!(result, "42 year");
}

// ---------------------------------------------------------------------------
// 2. Bare imported type with constraint
// ---------------------------------------------------------------------------
#[test]
fn unqualified_imported_type_with_constraints() {
    let code = r#"spec test
uses lemma units
data age: calendar -> minimum 1 year
rule value: age as year"#;
    let mut engine = load_ok(code);
    let mut data = HashMap::new();
    data.insert("age".to_string(), "36 month".into());
    let result = eval_rule(&mut engine, "test", data, "value");
    assert_eq!(result, "3 year");
}

// ---------------------------------------------------------------------------
// 3. Bare imported type + range suffix
// ---------------------------------------------------------------------------
#[test]
fn unqualified_imported_type_range() {
    let code = r#"spec test
uses lemma units
data span: calendar range
rule value: span"#;
    load_ok(code);
}

// ---------------------------------------------------------------------------
// 4. Ambiguous: two imports export same type name
// ---------------------------------------------------------------------------
#[test]
fn unqualified_ambiguous_type_errors() {
    let lib_a = r#"spec lib_a
data duration: measure
  -> unit second: 1
  -> unit minute: 60"#;
    let lib_b = r#"spec lib_b
data duration: measure
  -> unit tick: 1"#;
    let consumer = r#"spec consumer
uses lib_a
uses lib_b
data elapsed: duration
rule value: elapsed"#;

    let mut engine = Engine::new();
    engine
        .load([(path_source("lib_a.lemma"), lib_a.to_string())])
        .expect("lib_a loads");
    engine
        .load([(path_source("lib_b.lemma"), lib_b.to_string())])
        .expect("lib_b loads");
    let err = engine
        .load([(path_source("consumer.lemma"), consumer.to_string())])
        .expect_err("expected ambiguity error");
    let joined = err
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        joined.to_lowercase().contains("ambiguous"),
        "expected 'ambiguous' in error, got: {joined}"
    );
    assert!(
        joined.contains("lib_a") && joined.contains("lib_b"),
        "expected both import aliases in error, got: {joined}"
    );
}

// ---------------------------------------------------------------------------
// 5. Local typedef shadows imported type (existing behavior)
// ---------------------------------------------------------------------------
#[test]
fn unqualified_type_shadowed_by_local_typedef() {
    let code = r#"spec test
uses lemma units
data length: measure
  -> unit span: 1
  -> unit rod: 16.5
data height: length
rule value: height as rod"#;
    let mut engine = load_ok(code);
    let mut data = HashMap::new();
    data.insert("height".to_string(), "33 span".into());
    let result = eval_rule(&mut engine, "test", data, "value");
    assert_eq!(result, "2 rod");
}

// ---------------------------------------------------------------------------
// 6. Import alias name still errors (regression guard)
// ---------------------------------------------------------------------------
#[test]
fn unqualified_type_import_alias_still_errors() {
    let code = r#"spec test
uses lemma units
data x: units
rule r: x"#;
    let joined = load_err_joined(code);
    assert!(
        joined.contains("import alias"),
        "expected 'import alias' hint, got: {joined}"
    );
}

// ---------------------------------------------------------------------------
// 7. Unknown name still errors (regression guard)
// ---------------------------------------------------------------------------
#[test]
fn unqualified_type_still_errors_when_no_import_has_it() {
    let code = r#"spec test
uses lemma units
data x: nonexistent
rule r: x"#;
    let joined = load_err_joined(code);
    assert!(
        joined.contains("Unknown parent"),
        "expected 'Unknown parent', got: {joined}"
    );
}

// ---------------------------------------------------------------------------
// 8. Qualified form still works (regression guard)
// ---------------------------------------------------------------------------
#[test]
fn qualified_form_still_works() {
    let code = r#"spec test
uses lemma units
data age: units.calendar
rule value: age as year"#;
    let mut engine = load_ok(code);
    let mut data = HashMap::new();
    data.insert("age".to_string(), "36 month".into());
    let result = eval_rule(&mut engine, "test", data, "value");
    assert_eq!(result, "3 year");
}

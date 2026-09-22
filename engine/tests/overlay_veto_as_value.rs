//! Overlay binds bad overrides as Veto values; evaluation still completes.

use lemma::DateTimeValue;
use lemma::Engine;
use lemma::ErrorKind;
use std::collections::HashMap;

fn load(engine: &mut Engine, code: &str) {
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("t.lemma"))),
            code.to_string(),
        )])
        .expect("load");
}

#[test]
fn import_alias_override_ignored_missing_leaf_if_needed() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec inner
data amount: number

spec outer
uses p: inner
rule total: p.amount
"#,
    );
    let now = DateTimeValue::now();
    let mut data = HashMap::new();
    data.insert("p".to_string(), "42".into());
    let response = engine
        .run(None, "outer", Some(&now), data, None, false)
        .expect("import alias ignored; run completes");
    let total = response.results.get("total").expect("total");
    assert!(total.vetoed, "p.amount still missing");
    let reason = total.veto_reason.as_deref().expect("veto reason");
    assert!(
        reason.contains("Missing data") && reason.contains("amount"),
        "got: {reason}"
    );
}

#[test]
fn ignored_typo_suggests_on_missing_data() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec s
data age: number
rule r: age
"#,
    );
    let now = DateTimeValue::now();
    let mut data = HashMap::new();
    data.insert("agge".to_string(), "1".into());
    let response = engine
        .run(None, "s", Some(&now), data, None, false)
        .expect("typo ignored; run completes");
    let reason = response
        .results
        .get("r")
        .expect("r")
        .veto_reason
        .as_deref()
        .expect("veto reason");
    assert!(
        reason.contains("Missing data") && reason.contains("did you mean 'agge'"),
        "got: {reason}"
    );
}

#[test]
fn bound_key_no_typo_nag_for_extra_ignored() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec s
data age: number
rule r: age
"#,
    );
    let now = DateTimeValue::now();
    let mut data = HashMap::new();
    data.insert("age".to_string(), "1".into());
    data.insert("agge".to_string(), "2".into());
    let response = engine
        .run(None, "s", Some(&now), data, None, false)
        .expect("run");
    let r = response.results.get("r").expect("r");
    assert!(!r.vetoed, "age bound");
    assert_eq!(r.result(), Some("1"));
}

#[test]
fn duplicate_canonical_keys_request_error() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec s
data age: number
rule r: age
"#,
    );
    let now = DateTimeValue::now();
    let mut data = HashMap::new();
    data.insert("Age".to_string(), "1".into());
    data.insert("age".to_string(), "2".into());
    let err = engine
        .run(None, "s", Some(&now), data, None, false)
        .expect_err("duplicate canonical keys");
    assert_eq!(err.kind(), ErrorKind::Request);
    assert!(err.to_string().contains("Duplicate data key"), "got: {err}");
}

#[test]
fn maximum_constraint_vetoes_dependent_rule_others_still_evaluate() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec s
data x: number -> maximum 30
data y: number
rule uses_x: x * 2
rule uses_y: y * 2
"#,
    );
    let now = DateTimeValue::now();
    let mut data = HashMap::new();
    data.insert("x".to_string(), "42".into());
    data.insert("y".to_string(), "3".into());
    let response = engine
        .run(None, "s", Some(&now), data, None, false)
        .expect("constraint failure must not abort run");
    let uses_x = response.results.get("uses_x").expect("uses_x");
    assert!(uses_x.vetoed, "x over maximum vetoes uses_x");
    let reason = uses_x.veto_reason.as_deref().expect("reason");
    assert!(
        reason.contains("maximum") || reason.contains("30"),
        "got: {reason}"
    );
    let uses_y = response.results.get("uses_y").expect("uses_y");
    assert!(!uses_y.vetoed);
    assert_eq!(uses_y.result(), Some("6"));
}

#[test]
fn parse_failure_vetoes_dependent_rule() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec s
data age: number
rule r: age
"#,
    );
    let now = DateTimeValue::now();
    let mut data = HashMap::new();
    data.insert("age".to_string(), "thirty".into());
    let response = engine
        .run(None, "s", Some(&now), data, None, false)
        .expect("parse failure completes with veto");
    let r = response.results.get("r").expect("r");
    assert!(r.vetoed);
    let reason = r.veto_reason.as_deref().expect("reason");
    assert!(reason.contains("number"), "got: {reason}");
}

#[test]
fn default_never_auto_commits() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec s
data age: number -> suggest 18
rule r: age
"#,
    );
    let now = DateTimeValue::now();
    let response = engine
        .run(None, "s", Some(&now), HashMap::new(), None, false)
        .expect("unbound default still runs");
    let r = response.results.get("r").expect("r");
    assert!(r.vetoed, "-> suggest must not commit");
    let reason = r.veto_reason.as_deref().expect("reason");
    assert!(
        reason.contains("Missing data") && reason.contains("age"),
        "got: {reason}"
    );
    assert_ne!(r.result(), Some("18"));
}

#[test]
fn typo_with_default_still_missing() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec s
data age: number -> suggest 18
rule r: age
"#,
    );
    let now = DateTimeValue::now();
    let mut data = HashMap::new();
    data.insert("agge".to_string(), "99".into());
    let response = engine
        .run(None, "s", Some(&now), data, None, false)
        .expect("typo ignored; run completes");
    let reason = response
        .results
        .get("r")
        .expect("r")
        .veto_reason
        .as_deref()
        .expect("veto reason");
    assert!(
        reason.contains("Missing data") && reason.contains("did you mean 'agge'"),
        "got: {reason}"
    );
}

#[test]
fn typo_hint_case_insensitive() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec s
data age: number
rule r: age
"#,
    );
    let now = DateTimeValue::now();
    let mut data = HashMap::new();
    data.insert("Agge".to_string(), "1".into());
    let response = engine
        .run(None, "s", Some(&now), data, None, false)
        .expect("typo ignored; run completes");
    let reason = response
        .results
        .get("r")
        .expect("r")
        .veto_reason
        .as_deref()
        .expect("veto reason");
    assert!(
        reason.contains("Missing data") && reason.contains("did you mean 'Agge'"),
        "got: {reason}"
    );
}

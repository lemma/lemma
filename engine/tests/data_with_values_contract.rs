//! QA coverage for caller-data binding on every `DataDefinition` variant.
//!
//! Matrix:
//!   - unknown key → ignored
//!   - SpecRef override → evaluation completes (import alias ignored like unknown)
//!   - schema-backed [`DataDefinition::TypeDeclaration`] → success
//!   - Value → replaces literal
//!   - Reference → replaces reference target copy
//!   - validation failures per primitive → Veto on dependent rules
//!   - invalid override reasons surface in rule veto text

use lemma::DateTimeValue;
use lemma::Engine;
use std::collections::HashMap;

fn assert_rule_vetoed(
    result: Result<lemma::Response, lemma::Error>,
    rule_name: &str,
    reason_contains: &str,
) -> String {
    let resp = result.unwrap_or_else(|err| {
        panic!("run must complete with veto, not abort with Error — got: {err}")
    });
    let rr = resp
        .results
        .get(rule_name)
        .unwrap_or_else(|| panic!("rule '{rule_name}' not found"));
    assert!(
        rr.vetoed,
        "rule '{rule_name}' must veto on invalid override, got {:?}",
        rr.result()
    );
    let reason = rr.veto_reason.clone().expect("veto reason");
    if !reason_contains.is_empty() {
        assert!(
            reason.contains(reason_contains),
            "expected '{reason_contains}' in veto reason, got: {reason}"
        );
    }
    reason
}

fn rule_value(result: &lemma::Response, name: &str) -> String {
    let rr = result
        .results
        .get(name)
        .unwrap_or_else(|| panic!("rule '{}' not found", name));
    if rr.vetoed {
        return format!("VETO({})", rr.veto_reason.as_deref().unwrap_or("Vetoed"));
    }
    rr.result().expect("result").to_string()
}

#[test]
fn unknown_key_is_ignored() {
    let code = r#"
spec s
data x: number
rule r: x
"#;
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("w.lemma"))),
            code.to_string(),
        )])
        .unwrap();

    let mut data = HashMap::new();
    data.insert("x".to_string(), "1".into());
    data.insert("does_not_exist".to_string(), "42".into());

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "s", Some(&now), data, None, false)
        .expect("unknown keys must not abort evaluation");
    assert_eq!(rule_value(&response, "r"), "1");
}

#[test]
fn override_spec_reference_completes_evaluation() {
    let code = r#"
spec inner
data x: number -> suggest 1

spec outer
uses i: inner
rule r: i.x
"#;
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("w.lemma"))),
            code.to_string(),
        )])
        .unwrap();

    let mut data = HashMap::new();
    // `i` is a SpecRef, not a data value — overriding it is meaningless.
    data.insert("i".to_string(), "42".into());

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "outer", Some(&now), data, None, false)
        .expect("spec-ref override must not abort evaluation");
    let r = response.results.get("r").expect("r");
    assert!(
        r.vetoed,
        "import override ignored; i.x unbound (default never commits)"
    );
    let reason = r.veto_reason.as_deref().expect("veto reason");
    assert!(reason.contains("Missing data"), "got: {reason}");
}

#[test]
fn override_of_type_declaration_succeeds() {
    let code = r#"
spec s
data x: number
rule r: x
"#;
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("w.lemma"))),
            code.to_string(),
        )])
        .unwrap();

    let mut data = HashMap::new();
    data.insert("x".to_string(), "42".into());

    let now = DateTimeValue::now();
    let resp = engine
        .run(None, "s", Some(&now), data, None, false)
        .expect("evaluates");
    assert_eq!(rule_value(&resp, "r"), "42");
}

#[test]
fn override_of_literal_value_replaces() {
    let code = r#"
spec s
data x: 10
rule r: x
"#;
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("w.lemma"))),
            code.to_string(),
        )])
        .unwrap();

    let mut data = HashMap::new();
    data.insert("x".to_string(), "99".into());

    let now = DateTimeValue::now();
    let resp = engine
        .run(None, "s", Some(&now), data, None, false)
        .expect("evaluates");
    assert_eq!(rule_value(&resp, "r"), "99");
}

#[test]
fn override_wrong_primitive_kind_fails_with_related_data() {
    let code = r#"
spec s
data age: number
rule r: age
"#;
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("w.lemma"))),
            code.to_string(),
        )])
        .unwrap();

    let mut data = HashMap::new();
    data.insert("age".to_string(), "thirty".into());

    let now = DateTimeValue::now();
    assert_rule_vetoed(
        engine.run(None, "s", Some(&now), data, None, false),
        "r",
        "number",
    );
}

#[test]
fn override_violating_minimum_fails() {
    let code = r#"
spec s
data n: number -> minimum 10
rule r: n
"#;
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("w.lemma"))),
            code.to_string(),
        )])
        .unwrap();

    let mut data = HashMap::new();
    data.insert("n".to_string(), "5".into());

    let now = DateTimeValue::now();
    assert_rule_vetoed(
        engine.run(None, "s", Some(&now), data, None, false),
        "r",
        "minimum",
    );
}

#[test]
fn override_violating_maximum_fails() {
    let code = r#"
spec s
data n: number -> maximum 5
rule r: n
"#;
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("w.lemma"))),
            code.to_string(),
        )])
        .unwrap();

    let mut data = HashMap::new();
    data.insert("n".to_string(), "10".into());

    let now = DateTimeValue::now();
    assert_rule_vetoed(
        engine.run(None, "s", Some(&now), data, None, false),
        "r",
        "maximum",
    );
}

#[test]
fn override_violating_length_fails() {
    let code = r#"
spec s
data msg: text -> length 3
rule r: msg
"#;
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("w.lemma"))),
            code.to_string(),
        )])
        .unwrap();

    let mut data = HashMap::new();
    data.insert("msg".to_string(), "way too long".into());

    let now = DateTimeValue::now();
    assert_rule_vetoed(
        engine.run(None, "s", Some(&now), data, None, false),
        "r",
        "length",
    );
}

#[test]
fn override_violating_options_fails() {
    // `options` should restrict to a set.
    let code = r#"
spec s
data color: text -> options red green blue
rule r: color
"#;
    let mut engine = Engine::new();
    let load_result = engine.load([(
        lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("w.lemma"))),
        code.to_string(),
    )]);
    if let Err(errors) = &load_result {
        // If `options` on text is not yet supported, this test pins the gap.
        panic!(
            "`text -> options ...` must be supported or rejected with a clear error at load; \
             got load errors: {}",
            errors
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    let mut data = HashMap::new();
    data.insert("color".to_string(), "purple".into());

    let now = DateTimeValue::now();
    assert_rule_vetoed(
        engine.run(None, "s", Some(&now), data, None, false),
        "r",
        "option",
    );
}

#[test]
fn empty_override_map_is_noop() {
    let code = r#"
spec s
data x: 10
rule r: x
"#;
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("w.lemma"))),
            code.to_string(),
        )])
        .unwrap();

    let now = DateTimeValue::now();
    let resp = engine
        .run(None, "s", Some(&now), HashMap::new(), None, false)
        .expect("evaluates");
    assert_eq!(rule_value(&resp, "r"), "10");
}

#[test]
fn override_on_reference_replaces_and_wins_over_target() {
    let code = r#"
spec inner
data v: number -> suggest 1

spec outer
uses i: inner
  -> with v: 42
rule r: i.v
"#;
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("w.lemma"))),
            code.to_string(),
        )])
        .unwrap();

    let mut data = HashMap::new();
    data.insert("i.v".to_string(), "500".into());

    let now = DateTimeValue::now();
    let resp = engine
        .run(None, "outer", Some(&now), data, None, false)
        .expect("evaluates");
    assert_eq!(rule_value(&resp, "r"), "500");
}

#[test]
fn override_on_reference_still_validates_against_merged_type() {
    // Reference LHS declares a tighter max. User provides an out-of-range
    // value. Validation MUST use the merged type (LHS + target + local),
    // not just the target's type.
    let code = r#"
spec inner
data n: number -> maximum 5
data v: number

spec outer
uses i: inner
  -> with n: i.v
rule r: i.n
"#;
    let mut engine = Engine::new();
    let load_result = engine.load([(
        lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("w.lemma"))),
        code.to_string(),
    )]);
    load_result.expect("binding with i.n must plan");

    let mut data = HashMap::new();
    data.insert("i.n".to_string(), "10".into());

    let now = DateTimeValue::now();
    assert_rule_vetoed(
        engine.run(None, "outer", Some(&now), data, None, false),
        "r",
        "maximum",
    );
}

//! QA coverage for nested LHS paths (`with outer.inner: ...` for values; `data` for slot defs) — the binding
//! mechanism used to push values into child specs via `with` references.

use lemma::DateTimeValue;
use lemma::Engine;
use std::collections::HashMap;

fn load_ok(engine: &mut Engine, code: &str) {
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "nested.lemma",
            ))),
            code.to_string(),
        )])
        .unwrap_or_else(|errs| {
            let joined = errs
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("\n");
            panic!("expected load to succeed, got: {joined}");
        });
}

fn load_err_joined(engine: &mut Engine, code: &str) -> String {
    let err = engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "nested.lemma",
            ))),
            code.to_string(),
        )])
        .expect_err("expected load to fail");
    err.iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

fn rule_value(result: &lemma::Response, name: &str) -> String {
    let rr = result
        .results
        .get(name)
        .unwrap_or_else(|| panic!("rule '{}' not found", name));
    if rr.vetoed {
        format!("VETO({})", rr.veto_reason.as_deref().unwrap_or("Vetoed"))
    } else {
        rr.display().expect("display").to_string()
    }
}

// ─── Happy path: depth 2 and 3 ────────────────────────────────────────

#[test]
fn nested_binding_depth_2_literal() {
    let code = r#"
spec inner
data x: number

spec outer
uses i: inner
  -> with x: 42
rule r: i.x
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let now = DateTimeValue::now();
    let resp = engine
        .run(None, "outer", Some(&now), HashMap::new(), None, false)
        .expect("evaluates");
    assert_eq!(rule_value(&resp, "r"), "42");
}

#[test]
fn nested_binding_depth_3_literal() {
    let code = r#"
spec leaf
data v: number

spec middle
uses l: leaf

spec outer
uses m: middle
  -> with l.v: 7
rule r: m.l.v
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let now = DateTimeValue::now();
    let resp = engine
        .run(None, "outer", Some(&now), HashMap::new(), None, false)
        .expect("evaluates");
    assert_eq!(rule_value(&resp, "r"), "7");
}

#[test]
fn nested_binding_depth_3_data_target_reference_through_imported_spec() {
    let code = r#"
spec zones
data dest_zip3: text
rule zone: dest_zip3

spec quote
uses z: zones
  -> with dest_zip3: dest_zip3
data dest_zip3: text
rule zone: z.zone

spec shop
uses q: quote
  -> with dest_zip3: dest_zip3
data dest_zip3: text
rule zone: q.zone

spec wrap
uses r: shop
  -> with dest_zip3: dest_zip3
data dest_zip3: text
rule answer: r.zone
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let now = DateTimeValue::now();

    let missing = engine
        .run(None, "wrap", Some(&now), HashMap::new(), None, false)
        .expect("evaluates");
    assert_eq!(
        missing
            .results
            .get("answer")
            .expect("answer")
            .missing_data(),
        &["dest_zip3".to_string()]
    );

    let mut data = HashMap::new();
    data.insert("dest_zip3".to_string(), "100".to_string());
    let resp = engine
        .run(None, "wrap", Some(&now), data, None, false)
        .expect("evaluates");
    assert_eq!(rule_value(&resp, "answer"), "100");

    let show = engine.show(None, "wrap", Some(&now)).expect("show");
    let dest = show.data.get("dest_zip3").expect("dest_zip3 in show.data");
    assert!(
        dest.needed_by_rules.iter().any(|name| name == "answer"),
        "wrapper dest_zip3 must be needed by answer, got {:?}",
        dest.needed_by_rules
    );
}

#[test]
fn nested_binding_depth_3_rule_target_reference_through_imported_spec() {
    let code = r#"
spec inner
data slot: text

spec mid
uses i: inner
  -> with slot: computed
rule computed: "ok"

spec shop
uses m: mid
rule through: m.i.slot

spec wrap
uses s: shop
rule answer: s.through
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let now = DateTimeValue::now();
    let resp = engine
        .run(None, "wrap", Some(&now), HashMap::new(), None, false)
        .expect("evaluates");
    assert_eq!(rule_value(&resp, "answer"), "ok");
}

// ─── Error cases: structural ─────────────────────────────────────────

#[test]
fn binding_where_first_segment_is_not_spec_ref_is_rejected() {
    let code = r#"
spec s
data x: number -> suggest 1
with x.y: 42
rule r: x
"#;
    let mut engine = Engine::new();
    let joined = load_err_joined(&mut engine, code);
    assert!(
        joined.contains("uses") || joined.contains("standalone"),
        "standalone with must be rejected at parse, got: {joined}"
    );
}

#[test]
fn stdlib_units_with_missing_with_target_is_rejected() {
    let code = r#"
spec outer
uses lemma units
  -> with non_existing_data: 1
rule r: 1
"#;
    let mut engine = Engine::new();
    let joined = load_err_joined(&mut engine, code);
    assert!(
        joined.contains("`units` has no data field `non_existing_data`"),
        "got:\n{joined}"
    );
    assert!(!joined.contains("Data binding"), "got:\n{joined}");
    assert!(
        !joined.contains("Did you mean"),
        "no close field — should not suggest Did you mean, got:\n{joined}"
    );
}

#[test]
fn stdlib_units_with_typo_with_target_suggests_did_you_mean() {
    let code = r#"
spec outer
uses lemma units
  -> with durration: 1
rule r: 1
"#;
    let mut engine = Engine::new();
    let joined = load_err_joined(&mut engine, code);
    assert!(
        joined.contains("`units` has no data field `durration`"),
        "got:\n{joined}"
    );
    assert!(
        joined.contains("Did you mean `duration`?"),
        "expected typo hint for durration → duration, got:\n{joined}"
    );
}

#[test]
fn binding_targeting_nonexistent_child_data_is_rejected() {
    let code = r#"
spec inner
data x: number

spec outer
uses i: inner
  -> with nonexistent: 42
rule r: i.x
"#;
    let mut engine = Engine::new();
    let joined = load_err_joined(&mut engine, code);
    assert!(
        joined.contains("nonexistent")
            || joined.contains("does not exist")
            || joined.contains("not found"),
        "binding to non-existent child data must be rejected and mention the name, got: {joined}"
    );
}

#[test]
fn duplicate_binding_is_rejected_with_previous_location() {
    let code = r#"
spec inner
data x: number

spec outer
uses i: inner
  -> with x: 1
  -> with x: 2
rule r: i.x
"#;
    let mut engine = Engine::new();
    let joined = load_err_joined(&mut engine, code);
    assert!(
        joined.contains("Duplicate")
            || joined.contains("duplicate")
            || joined.contains("previously"),
        "duplicate binding must be rejected and reference prior location, got: {joined}"
    );
}

#[test]
fn binding_rhs_as_definition_is_rejected() {
    // Binding to a child data with `number` (schema definition RHS) is semantically
    // wrong — bindings must supply VALUES, not defs.
    let code = r#"
spec inner
data x: number

spec outer
uses i: inner
data i.x: number
rule r: i.x
"#;
    let mut engine = Engine::new();
    let joined = load_err_joined(&mut engine, code);
    assert!(
        joined.contains("dotted")
            || joined.contains("uses")
            || joined.contains("literal value")
            || joined.contains("data definition")
            || joined.contains("Binding paths")
            || joined.contains("`with`"),
        "binding with schema definition RHS must be rejected, got: {joined}"
    );
}

#[test]
fn binding_rhs_as_spec_reference_is_rejected() {
    // `with i.x: spec …` cannot use `spec` as the start of a reference (structural keyword).
    let code = r#"
spec other
data y: number -> suggest 1

spec inner
data x: number

spec outer
uses i: inner
  -> with x: spec other
uses o: other
rule r: i.x
"#;
    let mut engine = Engine::new();
    let joined = load_err_joined(&mut engine, code);
    assert!(
        joined.contains("spec") && joined.contains("Expected a reference"),
        "with RHS `spec` token must not parse as a value reference, got: {joined}"
    );
}

// ─── User override via RunData::resolve uses dotted input_key ────────

#[test]
fn user_override_of_nested_binding_via_dotted_key() {
    let code = r#"
spec inner
data x: number

spec outer
uses i: inner
  -> with x: 42
rule r: i.x
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let mut data = HashMap::new();
    data.insert("i.x".to_string(), "99".to_string());
    let now = DateTimeValue::now();
    let resp = engine
        .run(None, "outer", Some(&now), data, None, false)
        .expect("evaluates");
    assert_eq!(
        rule_value(&resp, "r"),
        "99",
        "user override via 'i.x' dotted key must win over binding literal"
    );
}

#[test]
fn user_override_of_depth_3_binding() {
    let code = r#"
spec leaf
data v: number

spec middle
uses l: leaf

spec outer
uses m: middle
  -> with l.v: 5
rule r: m.l.v
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let mut data = HashMap::new();
    data.insert("m.l.v".to_string(), "123".to_string());
    let now = DateTimeValue::now();
    let resp = engine
        .run(None, "outer", Some(&now), data, None, false)
        .expect("evaluates");
    assert_eq!(rule_value(&resp, "r"), "123");
}

// ─── Override key casing: case-insensitive API match ─────────────────

#[test]
fn user_override_key_is_case_insensitive() {
    let code = r#"
spec s
data x: number -> suggest 1
rule r: x
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let mut data = HashMap::new();
    data.insert("X".to_string(), "99".to_string());
    let now = DateTimeValue::now();
    let resp = engine
        .run(None, "s", Some(&now), data, None, false)
        .expect("evaluates");
    assert_eq!(rule_value(&resp, "r"), "99");
}

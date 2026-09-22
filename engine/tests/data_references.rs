/// Integration tests for `with` bindings (import-path LHS) and local-with rejection.
use lemma::DateTimeValue;
use lemma::Engine;
use std::collections::HashMap;

fn rule_value(result: &lemma::Response, rule_name: &str) -> String {
    let rr = result
        .results
        .get(rule_name)
        .unwrap_or_else(|| panic!("rule '{}' not found", rule_name));
    if rr.vetoed {
        return format!("VETO({})", rr.veto_reason.as_deref().unwrap_or("Vetoed"));
    }
    rr.result().expect("result").to_string()
}

fn load_err_joined(engine_res: Result<(), lemma::Errors>) -> String {
    let err = engine_res.expect_err("expected load to fail");
    err.iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn local_fill_literal_rejected_at_parse() {
    let mut engine = Engine::new();
    let joined = load_err_joined(
        engine.load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "reference.lemma",
            ))),
            r#"spec s
with x: 42"#
                .to_string(),
        )]),
    );
    assert!(
        joined.contains("Standalone") || joined.contains("uses"),
        "local with must be rejected at parse, got: {joined}"
    );
}

#[test]
fn local_fill_import_reference_rejected_at_parse() {
    let mut engine = Engine::new();
    let joined = load_err_joined(
        engine.load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "reference.lemma",
            ))),
            r#"spec inner
data v: number

spec outer
uses i: inner
with copy: i.v"#
                .to_string(),
        )]),
    );
    assert!(
        joined.contains("Standalone") || joined.contains("uses"),
        "local with must be rejected at parse, got: {joined}"
    );
}

#[test]
fn binding_reference_copies_cross_spec_target_value() {
    let code = r#"
spec law
data other: number -> suggest 99

spec inner
uses l: law
data slot: number

spec top
uses lic: inner
  -> with slot: lw.other
uses lw: law
rule answer: lic.slot
"#;

    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "reference.lemma",
            ))),
            code.to_string(),
        )])
        .unwrap();

    let now = DateTimeValue::now();
    let mut data = HashMap::new();
    data.insert("lw.other".to_string(), "99".into());
    let result = engine
        .run(None, "top", Some(&now), data, None, false)
        .expect("should run");

    assert_eq!(rule_value(&result, "answer"), "99");
}

#[test]
fn closed_reference_cycle_is_rejected() {
    let code = r#"
spec inner
data a: number
data b: number

spec outer
uses i: inner
  -> with a: i.b
  -> with b: i.a
"#;

    let mut engine = Engine::new();
    let joined = load_err_joined(engine.load([(
        lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
            "reference.lemma",
        ))),
        code.to_string(),
    )]));

    assert!(
        joined.contains("Circular data reference"),
        "closed reference cycle must be reported as a circular data reference, got: {joined}"
    );
}

#[test]
fn self_referential_binding_reference_is_rejected() {
    let code = r#"
spec inner
data x: number

spec outer
uses i: inner
  -> with x: i.x
"#;

    let mut engine = Engine::new();
    let joined = load_err_joined(engine.load([(
        lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
            "reference.lemma",
        ))),
        code.to_string(),
    )]));

    assert!(
        joined.contains("Circular data reference"),
        "self-referential binding reference must be reported as a circular data reference, got: {joined}"
    );
}

#[test]
fn binding_reference_target_type_incompatible_with_child_declared_type_is_rejected() {
    let code = r#"
spec inner
data n: number

spec source_spec
data s: text -> suggest "hello"

spec outer
uses i: inner
  -> with n: src.s
uses src: source_spec
rule r: i.n
"#;

    let mut engine = Engine::new();
    let joined = load_err_joined(engine.load([(
        lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
            "reference.lemma",
        ))),
        code.to_string(),
    )]));

    assert!(
        joined.contains("type mismatch"),
        "binding reference with target of a different base kind must be rejected, got: {joined}"
    );
}

#[test]
fn rule_target_binding_reference_cycle_through_self_is_rejected() {
    let code = r#"
spec inner
data slot: number

spec outer
uses i: inner
  -> with slot: r
rule r: i.slot
"#;

    let mut engine = Engine::new();
    let joined = load_err_joined(engine.load([(
        lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
            "reference.lemma",
        ))),
        code.to_string(),
    )]));

    assert!(
        joined.to_lowercase().contains("circular") || joined.to_lowercase().contains("cycle"),
        "rule-target binding forming a cycle must be rejected, got: {joined}"
    );
}

#[test]
fn rule_target_binding_reference_lhs_type_mismatch_is_rejected() {
    let code = r#"
spec inner
data v: number

spec source_spec
rule greeting: "hello"

spec outer
uses i: inner
  -> with v: src.greeting
uses src: source_spec
rule r: i.v
"#;

    let mut engine = Engine::new();
    let joined = load_err_joined(engine.load([(
        lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
            "reference.lemma",
        ))),
        code.to_string(),
    )]));

    assert!(
        joined.contains("type mismatch"),
        "rule-target binding with type kind mismatch must be rejected, got: {joined}"
    );
}

#[test]
fn reference_value_violating_child_declared_max_is_rejected() {
    let code = r#"
spec inner
data limited: number -> maximum 5

spec source_spec
data v: number -> suggest 10

spec outer
uses i: inner
  -> with limited: src.v
uses src: source_spec
rule r: i.limited
"#;

    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "reference.lemma",
            ))),
            code.to_string(),
        )])
        .expect("plan must succeed; constraint failure is a runtime veto");

    let now = DateTimeValue::now();
    let mut data = HashMap::new();
    data.insert("src.v".to_string(), "10".into());
    let resp = engine
        .run(None, "outer", Some(&now), data, None, false)
        .expect("run must complete with veto, not Error");
    let rr = resp.results.get("r").expect("rule 'r'");
    assert!(
        rr.vetoed,
        "expected max-constraint veto; got {:?}",
        rr.result()
    );
    let s = rr.veto_reason.as_deref().expect("veto reason");
    assert!(
        s.contains("maximum") || s.contains("exceeds"),
        "expected max-constraint veto, got: {s}"
    );
}

#[test]
fn local_non_dotted_rhs_stays_definition() {
    let code = r#"
spec s
data age: number -> suggest 30
data person: age
rule r: person
"#;

    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "reference.lemma",
            ))),
            code.to_string(),
        )])
        .expect("loads: `data person: age` is a typedef reference, not a value-copy reference");

    let now = DateTimeValue::now();
    let mut data = HashMap::new();
    data.insert("person".to_string(), "30".into());
    let result = engine
        .run(None, "s", Some(&now), data, None, false)
        .expect("evaluates; `person` is typed 'age' and uses supplied value");

    assert_eq!(rule_value(&result, "r"), "30");
}

#[test]
fn binding_non_dotted_rhs_resolves_in_enclosing_spec() {
    let code = r#"
spec inner
data slot: number

spec outer
uses i: inner
  -> with slot: src
data src: number -> suggest 123
rule r: i.slot
"#;

    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "reference.lemma",
            ))),
            code.to_string(),
        )])
        .expect("non-dotted RHS in binding context must resolve in the enclosing spec");

    let now = DateTimeValue::now();
    let mut data = HashMap::new();
    data.insert("src".to_string(), "123".into());
    let result = engine
        .run(None, "outer", Some(&now), data, None, false)
        .expect("evaluates");
    assert_eq!(rule_value(&result, "r"), "123");
}

#[test]
fn binding_reference_measure_family_mismatch_is_rejected() {
    let code = r#"
spec inner
data money: measure -> unit eur: 1.00
data payment: money

spec source_spec
data temp_unit: measure -> unit celsius: 1.0
data temperature: temp_unit

spec outer
uses i: inner
  -> with payment: src.temperature
uses src: source_spec
rule r: i.payment
"#;

    let mut engine = Engine::new();
    let res = engine.load([(
        lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
            "reference.lemma",
        ))),
        code.to_string(),
    )]);
    let joined = load_err_joined(res);
    assert!(
        joined.contains("measure family")
            || joined.contains("measure_family")
            || joined.contains("family")
            || joined.contains("type mismatch"),
        "expected measure-family-mismatch error, got: {joined}"
    );
}

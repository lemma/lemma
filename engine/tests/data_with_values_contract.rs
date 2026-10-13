//! QA coverage for caller-data binding on every `DataDefinition` variant.
//!
//! Matrix:
//!   - unknown key → ignored
//!   - SpecRef override → evaluation completes (import alias ignored like unknown)
//!   - schema-backed [`DataDefinition::TypeDeclaration`] → success
//!   - Value → replaces literal
//!   - Reference (slot bound to data or a rule) → request Error, never overridden
//!   - validation failures per primitive → Veto on dependent rules
//!   - invalid override reasons surface in rule veto text

use indexmap::IndexMap;
use lemma::Engine;
use lemma::{DateTimeValue, EvaluateDataValue};

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

    let mut data = IndexMap::new();
    data.insert("x".to_string(), "1".into());
    data.insert("does_not_exist".to_string(), "42".into());

    let now = DateTimeValue::now();
    let response = engine
        .evaluate(None, "s", Some(&now), data, None, false)
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

    let mut data = IndexMap::new();
    // `i` is a SpecRef, not a data value — overriding it is meaningless.
    data.insert("i".to_string(), "42".into());

    let now = DateTimeValue::now();
    let response = engine
        .evaluate(None, "outer", Some(&now), data, None, false)
        .expect("spec-ref override must not abort evaluation");
    let r = response.results.get("r").expect("r");
    assert!(
        r.vetoed,
        "import override ignored; i.x unbound (default never commits)"
    );
    assert_eq!(
        r.missing_data(),
        &["i.x"],
        "unbound nested slot under uses alias"
    );
    assert_eq!(
        r.veto_reason.as_deref(),
        Some("Missing data: i.x"),
        "got: {:?}",
        r.veto_reason
    );
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

    let mut data = IndexMap::new();
    data.insert("x".to_string(), "42".into());

    let now = DateTimeValue::now();
    let resp = engine
        .evaluate(None, "s", Some(&now), data, None, false)
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

    let mut data = IndexMap::new();
    data.insert("x".to_string(), "99".into());

    let now = DateTimeValue::now();
    let resp = engine
        .evaluate(None, "s", Some(&now), data, None, false)
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

    let mut data = IndexMap::new();
    data.insert("age".to_string(), "thirty".into());

    let now = DateTimeValue::now();
    assert_rule_vetoed(
        engine.evaluate(None, "s", Some(&now), data, None, false),
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

    let mut data = IndexMap::new();
    data.insert("n".to_string(), "5".into());

    let now = DateTimeValue::now();
    assert_rule_vetoed(
        engine.evaluate(None, "s", Some(&now), data, None, false),
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

    let mut data = IndexMap::new();
    data.insert("n".to_string(), "10".into());

    let now = DateTimeValue::now();
    assert_rule_vetoed(
        engine.evaluate(None, "s", Some(&now), data, None, false),
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

    let mut data = IndexMap::new();
    data.insert("msg".to_string(), "way too long".into());

    let now = DateTimeValue::now();
    assert_rule_vetoed(
        engine.evaluate(None, "s", Some(&now), data, None, false),
        "r",
        "length",
    );
}

#[test]
fn override_violating_options_fails() {
    // `options` should restrict to a set.
    let code = r#"
spec s
data color: text -> options "red", "green", "blue"
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

    let mut data = IndexMap::new();
    data.insert("color".to_string(), "purple".into());

    let now = DateTimeValue::now();
    assert_rule_vetoed(
        engine.evaluate(None, "s", Some(&now), data, None, false),
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
        .evaluate(None, "s", Some(&now), IndexMap::new(), None, false)
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

    let mut data = IndexMap::new();
    data.insert("i.v".to_string(), "500".into());

    let now = DateTimeValue::now();
    let resp = engine
        .evaluate(None, "outer", Some(&now), data, None, false)
        .expect("evaluates");
    assert_eq!(rule_value(&resp, "r"), "500");
}

fn load(code: &str) -> Engine {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("w.lemma"))),
            code.to_string(),
        )])
        .expect("must plan");
    engine
}

fn assert_bound_slot_rejected(engine: &Engine, spec: &str, key: &str, target: &str) {
    let mut data = IndexMap::new();
    data.insert(key.to_string(), "10".into());
    let now = DateTimeValue::now();
    let error = engine
        .evaluate(None, spec, Some(&now), data, None, false)
        .expect_err("run data for a bound slot must be a request error");
    assert_eq!(
        error.message(),
        format!("data '{key}' is bound to '{target}' by the spec and cannot be supplied")
    );
    assert_eq!(error.related_data(), Some(key));
}

#[test]
fn evaluate_data_for_slot_bound_to_data_is_request_error() {
    let engine = load(
        r#"
spec inner
data n: number -> maximum 5
data v: number

spec outer
uses i: inner
  -> with n: i.v
rule r: i.n
"#,
    );
    assert_bound_slot_rejected(&engine, "outer", "i.n", "i.v");
}

#[test]
fn copied_value_still_validates_against_merged_type() {
    let engine = load(
        r#"
spec inner
data n: number -> maximum 5
data v: number

spec outer
uses i: inner
  -> with n: i.v
rule r: i.n
"#,
    );
    let mut data = IndexMap::new();
    data.insert("i.v".to_string(), "10".into());
    let now = DateTimeValue::now();
    assert_rule_vetoed(
        engine.evaluate(None, "outer", Some(&now), data, None, false),
        "r",
        "maximum",
    );
}

#[test]
fn evaluate_data_for_slot_bound_to_rule_is_request_error() {
    let engine = load(
        r#"
spec source_spec
data x: number
rule computed: x * 2

spec inner
data slot: number

spec outer
uses src: source_spec
uses i: inner
  -> with slot: src.computed
rule r: i.slot
"#,
    );
    assert_bound_slot_rejected(&engine, "outer", "i.slot", "src.computed");
}

#[test]
fn evaluate_data_for_row_field_bound_in_row_spec_is_request_error() {
    let engine = load(
        r#"
spec rate
data base: number
data amount: number

spec line
data price: number
uses r: rate
  -> with amount: price
rule total: r.amount

spec order
uses lines: line list
rule all_totals: lines -> sum total
"#,
    );
    let mut row = IndexMap::new();
    row.insert("price".to_string(), EvaluateDataValue::from("4"));
    row.insert("r.amount".to_string(), EvaluateDataValue::from("9"));
    let mut data = IndexMap::new();
    data.insert(
        "lines".to_string(),
        EvaluateDataValue::List(vec![EvaluateDataValue::Map(row)]),
    );
    let now = DateTimeValue::now();
    let error = engine
        .evaluate(None, "order", Some(&now), data, None, false)
        .expect_err("row field bound in the row spec must be a request error");
    assert_eq!(
        error.message(),
        "data 'r.amount' is bound to 'price' by the spec and cannot be supplied"
    );
}

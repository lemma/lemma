use lemma::DateTimeValue;
use lemma::Engine;
use std::collections::HashMap;

/// Test that when a rule in a referenced spec fails due to missing data,
/// the error message correctly shows "Missing data" instead of "Rule not found"
/// when another rule references it.
#[test]
fn test_missing_data_propagation_through_rule_reference() {
    let mut engine = Engine::new();

    // Referenced spec with a rule that requires a data
    let private_spec = r#"
spec private_rules
data base_price: number
data quantity: number
rule total_before_discount: base_price * quantity
rule final_total: total_before_discount
"#;

    // Main spec that references the other spec
    let main_spec = r#"
spec rules_and_unless
uses rules: private_rules
  -> with base_price: 500
rule total: rules.final_total
"#;

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "private.lemma",
            ))),
            private_spec.to_string(),
        )])
        .unwrap();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("main.lemma"))),
            main_spec.to_string(),
        )])
        .unwrap();

    let now = DateTimeValue::now();
    // Evaluate with missing quantity data
    let response = engine
        .run(
            None,
            "rules_and_unless",
            Some(&now),
            HashMap::new(),
            None,
            false,
        )
        .unwrap();

    let total_rule = response
        .results
        .values()
        .find(|r| r.rule.name == "total")
        .expect("total rule should be in results");

    assert!(total_rule.vetoed, "total should be vetoed");
    let msg_str = total_rule.veto_reason.as_deref().expect("veto reason");
    assert!(
        msg_str.contains("Missing data"),
        "Veto reason should contain 'Missing data', but got: {}",
        msg_str
    );
    assert!(
        !msg_str.contains("not found"),
        "Veto reason should NOT contain 'not found', but got: {}",
        msg_str
    );
}

/// Test that rules not depending on missing data still evaluate correctly
#[test]
fn test_rules_without_missing_data_still_evaluate() {
    let mut engine = Engine::new();

    let spec = r#"
spec test_spec
data price: number
data quantity: number
rule subtotal: price * quantity
rule message: "Order processed"
"#;

    engine
        .load([(lemma::SourceType::Volatile, spec.to_string())])
        .unwrap();

    let mut data = std::collections::HashMap::new();
    data.insert("price".to_string(), "10".to_string());

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "test_spec", Some(&now), data, None, false)
        .unwrap();

    // subtotal should fail due to missing measure
    let subtotal_rule = response
        .results
        .values()
        .find(|r| r.rule.name == "subtotal")
        .expect("subtotal rule should be in results");
    assert!(
        subtotal_rule.vetoed,
        "subtotal should be Veto due to missing quantity"
    );

    // message should still evaluate successfully (doesn't depend on missing data, None)
    let message_rule = response
        .results
        .values()
        .find(|r| r.rule.name == "message")
        .expect("message rule should be in results");
    assert!(
        !message_rule.vetoed,
        "message rule should evaluate successfully"
    );
    assert_eq!(
        message_rule
            .value
            .as_ref()
            .expect("rule result value")
            .text
            .as_deref(),
        Some("Order processed")
    );
}

/// A reference whose target has no value at eval time must surface as a
/// MissingData veto at any rule consuming the reference, naming the
/// reference path (not the target path, which is an implementation detail).
#[test]
fn reference_with_missing_target_vetoes_as_missing_data() {
    let code = r#"
spec inner
data slot: number

spec outer
uses i: inner
rule r: i.slot
"#;
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "missing.lemma",
            ))),
            code.to_string(),
        )])
        .unwrap();

    let now = DateTimeValue::now();
    let resp = engine
        .run(None, "outer", Some(&now), HashMap::new(), None, false)
        .expect("evaluates");

    let rr = resp.results.get("r").expect("rule 'r'");
    assert!(rr.vetoed, "expected MissingData veto");
    match rr.veto_detail.as_ref().expect("veto detail") {
        lemma::VetoType::MissingData { data, .. } => {
            let shown = data.to_string();
            assert!(
                shown.contains("slot") || shown.contains("i.slot"),
                "missing-data veto should name the missing data path; got: {shown}"
            );
        }
        other => panic!("expected MissingData veto, got: {:?}", other),
    }
}

/// Rule-target reference whose target rule returns a Veto must propagate
/// that veto to any consumer.
#[test]
fn rule_target_reference_veto_propagates_to_consumer() {
    let code = r#"
spec inner
data denom: 0
rule divided: 10 / denom

spec top
uses i: inner
rule out: i.divided
"#;
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "missing.lemma",
            ))),
            code.to_string(),
        )])
        .expect("rule-target reference must be accepted at plan time");

    let now = DateTimeValue::now();
    let resp = engine
        .run(None, "top", Some(&now), HashMap::new(), None, false)
        .expect("evaluator must run; veto is a domain result, not an error");

    let rr = resp.results.get("out").expect("rule 'out'");
    assert!(rr.vetoed, "expected propagated veto");
    let s = rr.veto_reason.as_deref().expect("veto reason");
    assert!(
        s.contains("Division by zero"),
        "rule-target reference must propagate the exact veto reason of the target rule, \
         got: {s}"
    );
}

/// Unary minus and reciprocal over an unbound input veto with Missing data
/// (they used to abort the process: the operand passed no veto check).
#[test]
fn unary_minus_and_reciprocal_over_unbound_input_veto_with_missing_data() {
    let code = r#"
spec unary
data b: number
rule negated: -b
rule inverted: 1 / b
rule negated_explained: -b
"#;
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("unary.lemma"))),
            code.to_string(),
        )])
        .expect("spec must load");

    let now = DateTimeValue::now();
    for explain in [false, true] {
        let response = engine
            .run(None, "unary", Some(&now), HashMap::new(), None, explain)
            .expect("run must not error");
        for rule in ["negated", "inverted", "negated_explained"] {
            let result = response.results.get(rule).expect("rule result");
            assert!(
                result.vetoed,
                "{rule} must veto without b (explain={explain})"
            );
            assert_eq!(
                result.missing_data(),
                ["b".to_string()],
                "{rule} must list b as missing (explain={explain})"
            );
        }
    }
}

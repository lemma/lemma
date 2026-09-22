//! `is veto` / `ResultIsVeto` — boolean coercion from operand `OperationResult`.

use lemma::DateTimeValue;
use lemma::Engine;
use std::collections::HashMap;

#[test]
fn unless_validated_price_is_veto_then_fallback_value() {
    let code = r#"
spec pricing
data price: -5
data quantity: 2
rule validated_price: price
    unless price < 0 then veto "Price cannot be negative"
rule total: validated_price * quantity
    unless validated_price is veto then 0
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let now = DateTimeValue::now();
    let response = engine
        .run(None, "pricing", Some(&now), HashMap::new(), None, true)
        .unwrap();

    let total = response
        .results
        .values()
        .find(|r| r.rule.name == "total")
        .expect("total rule");

    assert_eq!(total.result(), Some("0"));
}

#[test]
fn unless_re_veto_uses_outer_message_only() {
    let code = r#"
spec pricing
data price: -5
rule validated_price: price
    unless price < 0 then veto "Price cannot be negative"
rule total: validated_price
    unless validated_price is veto then veto "Upstream failed"
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let now = DateTimeValue::now();
    let response = engine
        .run(None, "pricing", Some(&now), HashMap::new(), None, true)
        .unwrap();

    let total = response
        .results
        .values()
        .find(|r| r.rule.name == "total")
        .expect("total rule");

    assert!(total.vetoed);
    assert_eq!(total.veto_reason.as_deref(), Some("Upstream failed"));
}

#[test]
fn rule_reference_is_veto_when_rule_vetoed() {
    let code = r#"
spec pricing
data quantity: -1
rule validated_quantity: quantity
    unless quantity < 0 then veto "Measure cannot be negative"
rule flag: validated_quantity is veto
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let now = DateTimeValue::now();
    let response = engine
        .run(None, "pricing", Some(&now), HashMap::new(), None, true)
        .unwrap();

    let flag = response
        .results
        .values()
        .find(|r| r.rule.name == "flag")
        .expect("flag");

    assert_eq!(
        flag.result.as_ref().expect("rule result value").boolean,
        Some(true)
    );
}

#[test]
fn unless_on_rule_reference_is_veto_uses_rule_result_not_inlined_body() {
    let code = r#"
spec pricing
data price: 10
data quantity: -1
rule validated_quantity: quantity
    unless quantity < 0 then veto "Measure cannot be negative"
rule total: price * validated_quantity
    unless validated_quantity is veto then 0
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let now = DateTimeValue::now();
    let response = engine
        .run(None, "pricing", Some(&now), HashMap::new(), None, true)
        .unwrap();

    let total = response
        .results
        .values()
        .find(|r| r.rule.name == "total")
        .expect("total");

    assert_eq!(total.result(), Some("0"));
}

#[test]
fn operand_is_veto_false_when_operand_has_value() {
    let code = r#"
spec pricing
data price: 10
rule validated_price: price
    unless price < 0 then veto "Price cannot be negative"
rule flag: validated_price is veto
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let now = DateTimeValue::now();
    let response = engine
        .run(None, "pricing", Some(&now), HashMap::new(), None, true)
        .unwrap();

    let flag = response
        .results
        .values()
        .find(|r| r.rule.name == "flag")
        .expect("flag");

    assert_eq!(
        flag.result.as_ref().expect("rule result value").boolean,
        Some(false)
    );
}

#[test]
fn commutative_veto_is_validated_price_matches_is_veto() {
    let code = r#"
spec pricing
data price: -5
rule validated_price: price
    unless price < 0 then veto "negative"
rule left_to_right: validated_price is veto
rule right_to_left: veto is validated_price
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let now = DateTimeValue::now();
    let response = engine
        .run(None, "pricing", Some(&now), HashMap::new(), None, true)
        .unwrap();

    let ltr = response
        .results
        .values()
        .find(|r| r.rule.name == "left_to_right")
        .expect("left_to_right");
    let rtl = response
        .results
        .values()
        .find(|r| r.rule.name == "right_to_left")
        .expect("right_to_left");

    let ltr_bool = ltr.result.as_ref().expect("rule result value").boolean;
    let rtl_bool = rtl.result.as_ref().expect("rule result value").boolean;
    assert_eq!(ltr_bool, rtl_bool);
    assert_eq!(ltr_bool, Some(true));
}

#[test]
fn not_veto_is_x_matches_is_not_veto() {
    let code = r#"
spec pricing
data price: 10
rule validated_price: price
rule a: validated_price is not veto
rule b: not veto is validated_price
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let now = DateTimeValue::now();
    let response = engine
        .run(None, "pricing", Some(&now), HashMap::new(), None, true)
        .unwrap();

    let a = response
        .results
        .values()
        .find(|r| r.rule.name == "a")
        .expect("a");
    let b = response
        .results
        .values()
        .find(|r| r.rule.name == "b")
        .expect("b");
    let a_bool = a.result.as_ref().expect("rule result value").boolean;
    let b_bool = b.result.as_ref().expect("rule result value").boolean;
    assert_eq!(a_bool, b_bool);
    assert_eq!(a_bool, Some(true));
}

#[test]
fn explanation_records_result_is_veto_on_unless_fallback() {
    let code = r#"
spec pricing
data price: -5
data quantity: 1
rule validated_price: price
    unless price < 0 then veto "negative"
rule total: validated_price * quantity
    unless validated_price is veto then 0
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let now = DateTimeValue::now();
    let response = engine
        .run(None, "pricing", Some(&now), HashMap::new(), None, true)
        .unwrap();

    let total = response
        .results
        .values()
        .find(|r| r.rule.name == "total")
        .expect("total");
    let explanation = total.explanation.as_ref().expect("explanation");
    assert!(
        explanation
            .causes
            .iter()
            .any(|c| c.condition.contains("is veto") && c.value == "true"),
        "expected unless cause with is veto true, got {:?}",
        explanation.causes
    );
}

#[test]
fn parse_rejects_veto_with_message_in_is_veto_comparison() {
    let code = r#"
spec bad
data x: 1
rule r: x is veto "no"
"#;

    let mut engine = Engine::new();
    let err = engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect_err("veto message in is veto position must fail");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("veto with a message") || msg.contains("is veto"),
        "unexpected error: {msg}"
    );
}

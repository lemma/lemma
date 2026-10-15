//! Exact ASCII explanation trees for stress_explain (from 09_stress_test).

use indexmap::IndexMap;
use lemma::{
    format_explanation, DateTimeValue, Engine, EvaluateDataValue, ExplanationNode, SourceType,
};

const SPEC: &str = include_str!("fixtures/explain/stress_explain.lemma");

fn load() -> Engine {
    let mut engine = Engine::new();
    engine
        .load([(SourceType::Volatile, SPEC.to_string())])
        .expect("stress_explain must load");
    engine
}

fn explain(
    engine: &Engine,
    spec: &str,
    rule: &str,
    data: IndexMap<String, EvaluateDataValue>,
) -> lemma::Explanation {
    let now = DateTimeValue::now();
    let response = engine
        .evaluate(
            None,
            spec,
            Some(&now),
            data,
            Some(&[rule.to_string()]),
            true,
        )
        .expect("eval must succeed");
    response
        .results
        .get(rule)
        .expect("rule")
        .explanation
        .clone()
        .expect("explanation")
}

fn stress_order_data(
    tier: &str,
    payment: &str,
    fragile: &str,
    express: &str,
) -> IndexMap<String, EvaluateDataValue> {
    let mut data = IndexMap::new();
    data.insert("base_price".into(), "100".into());
    data.insert("quantity".into(), "10".into());
    data.insert("delivery_distance".into(), "50".into());
    data.insert("loyalty_points".into(), "0".into());
    data.insert("package_weight".into(), "5".into());
    data.insert("customer_tier".into(), tier.into());
    data.insert("payment_method".into(), payment.into());
    data.insert("is_fragile".into(), fragile.into());
    data.insert("is_express".into(), express.into());
    data
}

fn extended_order_data(
    tier: &str,
    payment: &str,
    fragile: &str,
    express: &str,
) -> IndexMap<String, EvaluateDataValue> {
    let mut data = IndexMap::new();
    data.insert("order.base_price".into(), "100".into());
    data.insert("order.quantity".into(), "10".into());
    data.insert("order.delivery_distance".into(), "50".into());
    data.insert("order.loyalty_points".into(), "0".into());
    data.insert("order.package_weight".into(), "5".into());
    data.insert("order.customer_tier".into(), tier.into());
    data.insert("order.payment_method".into(), payment.into());
    data.insert("order.is_fragile".into(), fragile.into());
    data.insert("order.is_express".into(), express.into());
    data
}

#[test]
fn payment_cash_fragile_ascii() {
    let engine = load();
    let explanation = explain(
        &engine,
        "stress_test",
        "payment_allowed",
        stress_order_data("vip", "cash", "true", "false"),
    );
    assert_eq!(
        format_explanation(&explanation),
        include_str!("fixtures/explain/stress_payment_cash_fragile.ascii.txt")
    );
}

#[test]
fn payment_express_noncredit_ascii() {
    let engine = load();
    let explanation = explain(
        &engine,
        "stress_test",
        "payment_allowed",
        stress_order_data("vip", "debit", "false", "true"),
    );
    assert_eq!(
        format_explanation(&explanation),
        include_str!("fixtures/explain/stress_payment_express_noncredit.ascii.txt")
    );
}

#[test]
fn payment_ok_ascii() {
    let engine = load();
    let explanation = explain(
        &engine,
        "stress_test",
        "payment_allowed",
        stress_order_data("vip", "credit", "false", "true"),
    );
    assert_eq!(
        format_explanation(&explanation),
        include_str!("fixtures/explain/stress_payment_ok.ascii.txt")
    );
}

#[test]
fn grand_total_vip_express_ascii() {
    let engine = load();
    let explanation = explain(
        &engine,
        "stress_test",
        "grand_total",
        stress_order_data("vip", "credit", "false", "true"),
    );
    assert_eq!(
        format_explanation(&explanation),
        include_str!("fixtures/explain/stress_grand_total_vip_express.ascii.txt")
    );
}

#[test]
fn rush_and_cap_ok_ascii() {
    let engine = load();
    let explanation = explain(
        &engine,
        "stress_test_extended",
        "rush_and_cap_ok",
        extended_order_data("vip", "credit", "false", "true"),
    );
    assert_eq!(
        format_explanation(&explanation),
        include_str!("fixtures/explain/stress_rush_and_cap_ok.ascii.txt")
    );
}

#[test]
fn rush_and_cap_ok_import_rule_is_rule_ref() {
    let engine = load();
    let explanation = explain(
        &engine,
        "stress_test_extended",
        "rush_and_cap_ok",
        extended_order_data("vip", "credit", "false", "true"),
    );
    let winning = explanation
        .causes
        .iter()
        .find(|c| c.condition.contains("order.is_rush") && c.value == "true")
        .expect("held import and cause");
    let names: Vec<String> = winning
        .children
        .iter()
        .map(|node| match node {
            ExplanationNode::RuleRef { name, .. } => name.input_key(),
            other => panic!("unexpected cause child: {other:?}"),
        })
        .collect();
    assert_eq!(
        names,
        vec![
            "order.is_rush".to_string(),
            "discount_within_cap".to_string()
        ]
    );
}

/// ImportRule ExplainedRule must carry the underlying expression, not the ref name.
#[test]
fn rush_and_cap_ok_import_rule_body_is_underlying_expression() {
    let engine = load();
    let explanation = explain(
        &engine,
        "stress_test_extended",
        "rush_and_cap_ok",
        extended_order_data("vip", "credit", "false", "true"),
    );
    let is_rush = explanation
        .rules
        .iter()
        .find(|r| r.name.input_key() == "order.is_rush")
        .expect("order.is_rush in rules table");
    assert_eq!(
        is_rush.body.as_str(),
        "is_express and estimated_delivery_days <= 2",
        "ImportRule body must be the source rule expression, not the ref name"
    );
    let savings = explanation
        .rules
        .iter()
        .find(|r| r.name.input_key() == "order.savings_percent")
        .expect("order.savings_percent in rules table");
    assert_eq!(
        savings.body.as_str(),
        "(savings_total / subtotal) as percent",
        "ImportRule body must be the source rule expression, not the ref name"
    );
}

#[test]
fn final_status_import_rule_bodies_are_underlying_expressions() {
    let engine = load();
    let explanation = explain(
        &engine,
        "stress_test_extended",
        "final_status",
        extended_order_data("vip", "credit", "false", "true"),
    );
    for (key, expected_body) in [
        (
            "order.savings_percent",
            "(savings_total / subtotal) as percent",
        ),
        ("order.grand_total", "final_before_fees + processing_fee"),
        ("order.subtotal", "base_price * quantity"),
    ] {
        let rule = explanation
            .rules
            .iter()
            .find(|r| r.name.input_key() == key)
            .unwrap_or_else(|| panic!("{key} in rules table"));
        assert_eq!(
            rule.body.as_str(),
            expected_body,
            "{key}: ImportRule body must be source expression, not ref name"
        );
    }
}

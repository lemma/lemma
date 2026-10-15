//! Exact ASCII explanation trees for shipping_explain (from 07_shipping_policy).

use indexmap::IndexMap;
use lemma::{format_explanation, DateTimeValue, Engine, EvaluateDataValue, SourceType};

const SPEC: &str = include_str!("fixtures/explain/shipping_explain.lemma");

fn explain(rule: &str, data: IndexMap<String, EvaluateDataValue>) -> lemma::Explanation {
    let mut engine = Engine::new();
    engine
        .load([(SourceType::Volatile, SPEC.to_string())])
        .expect("shipping_explain must load");
    let now = DateTimeValue::now();
    let response = engine
        .evaluate(
            None,
            "shipping_policy",
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

fn base(
    country: &str,
    tier: &str,
    weight: &str,
    total: &str,
    hazardous: &str,
) -> IndexMap<String, EvaluateDataValue> {
    let mut data = IndexMap::new();
    data.insert("destination_country".into(), country.into());
    data.insert("customer_tier".into(), tier.into());
    data.insert("item_weight".into(), weight.into());
    data.insert("order_total".into(), total.into());
    data.insert("is_expedited".into(), "false".into());
    data.insert("is_hazardous".into(), hazardous.into());
    data.insert("is_po_box".into(), "false".into());
    data
}

#[test]
fn final_shipping_nl_gold_ascii() {
    let explanation = explain("final_shipping", base("NL", "gold", "2", "50", "false"));
    assert_eq!(
        format_explanation(&explanation),
        include_str!("fixtures/explain/shipping_final_nl_gold.ascii.txt")
    );
}

#[test]
fn final_shipping_free_eligible_ascii() {
    let explanation = explain("final_shipping", base("NL", "gold", "2", "150", "false"));
    assert_eq!(
        format_explanation(&explanation),
        include_str!("fixtures/explain/shipping_free_eligible.ascii.txt")
    );
}

#[test]
fn hazardous_intl_veto_ascii() {
    let explanation = explain("hazardous_fee", base("DE", "standard", "2", "50", "true"));
    assert_eq!(
        format_explanation(&explanation),
        include_str!("fixtures/explain/shipping_hazardous_intl_veto.ascii.txt")
    );
}

#[test]
fn hazardous_is_not_nl_ascii() {
    let explanation = explain("hazardous_fee", base("BE", "standard", "2", "50", "true"));
    assert_eq!(
        format_explanation(&explanation),
        include_str!("fixtures/explain/shipping_is_not_nl.ascii.txt")
    );
}

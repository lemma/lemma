//! Exact ASCII explanation trees for lists_explain (from coffee_order_lists).

use indexmap::IndexMap;
use lemma::{format_explanation, DateTimeValue, Engine, EvaluateDataValue, SourceType};

const SPEC: &str = include_str!("fixtures/explain/lists_explain.lemma");

fn explain(rule: &str) -> lemma::Explanation {
    let mut engine = Engine::new();
    engine
        .load([(SourceType::Volatile, SPEC.to_string())])
        .expect("lists_explain must load");
    let now = DateTimeValue::now();
    let response = engine
        .evaluate(
            None,
            "coffee_order",
            Some(&now),
            IndexMap::<String, EvaluateDataValue>::new(),
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

#[test]
fn selected_price_ascii() {
    assert_eq!(
        format_explanation(&explain("selected_price")),
        include_str!("fixtures/explain/lists_selected_price.ascii.txt")
    );
}

#[test]
fn total_ascii() {
    assert_eq!(
        format_explanation(&explain("total")),
        include_str!("fixtures/explain/lists_total.ascii.txt")
    );
}

#[test]
fn any_large_ascii() {
    assert_eq!(
        format_explanation(&explain("any_large")),
        include_str!("fixtures/explain/lists_any_large.ascii.txt")
    );
}

#[test]
fn large_where_ascii() {
    assert_eq!(
        format_explanation(&explain("large")),
        include_str!("fixtures/explain/lists_large_where.ascii.txt")
    );
}

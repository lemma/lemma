//! Exact ASCII explanation trees for lists_explain (from coffee_order_lists).

use indexmap::IndexMap;
use lemma::{format_explanation, DateTimeValue, Engine, EvaluateDataValue, SourceType};

const SPEC: &str = include_str!("fixtures/explain/lists_explain.lemma");
const CENSUS: &str = include_str!("fixtures/explain/lists_census.lemma");
const EMPTY_AVG: &str = include_str!("fixtures/explain/lists_empty_avg.lemma");
const WITH_AVG: &str = include_str!("fixtures/explain/lists_with_avg.lemma");
const CATALOG_KEY: &str = include_str!("fixtures/explain/lists_catalog_key.lemma");

fn explain_spec(spec: &str, target: &str, rule: &str) -> lemma::Explanation {
    let mut engine = Engine::new();
    engine
        .load([(SourceType::Volatile, spec.to_string())])
        .expect("spec must load");
    let now = DateTimeValue::now();
    let response = engine
        .evaluate(
            None,
            target,
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

fn explain(rule: &str) -> lemma::Explanation {
    explain_spec(SPEC, "coffee_order", rule)
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

#[test]
fn census_where_rules_ascii() {
    assert_eq!(
        format_explanation(&explain_spec(CENSUS, "household", "selected")),
        include_str!("fixtures/explain/lists_census.ascii.txt")
    );
}

#[test]
fn empty_avg_ascii() {
    assert_eq!(
        format_explanation(&explain_spec(EMPTY_AVG, "scores", "average")),
        include_str!("fixtures/explain/lists_empty_avg.ascii.txt")
    );
}

#[test]
fn with_then_avg_ascii() {
    assert_eq!(
        format_explanation(&explain_spec(WITH_AVG, "scores", "average")),
        include_str!("fixtures/explain/lists_with_avg.ascii.txt")
    );
}

#[test]
fn total_json_is_list_command_sum() {
    let explanation = explain("total");
    let json = serde_json::to_value(&explanation).expect("serialize");
    let child = &json["children"][0];
    assert_eq!(child["type"], "list_command");
    assert_eq!(child["expression"], "menu -> sum price");
    assert_eq!(child["stage"], "sum price");
    assert_eq!(child["op"], "sum");
}

#[test]
fn catalog_key_json_is_list_command_key() {
    let explanation = explain_spec(CATALOG_KEY, "catalog", "items");
    let json = serde_json::to_value(&explanation).expect("serialize");
    let with = &json["children"][0];
    assert_eq!(with["type"], "list_command");
    assert_eq!(with["op"], "with");
    let key = &with["operands"][0];
    assert_eq!(key["type"], "list_command");
    assert_eq!(key["expression"], "row list -> key name");
    assert_eq!(key["stage"], "key name");
    assert_eq!(key["op"], "key");
}

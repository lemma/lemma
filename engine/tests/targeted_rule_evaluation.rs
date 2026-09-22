//! Targeted rule evaluation: `rules` slice scopes tree eval (explain:false) and response.

use lemma::{DateTimeValue, Engine};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

fn order_pipeline_inputs() -> HashMap<String, String> {
    HashMap::from([
        ("customer_tier".into(), "gold".into()),
        ("payment_method".into(), "credit".into()),
        ("shipping_zone".into(), "national".into()),
        ("quantity".into(), "12".into()),
        ("unit_price".into(), "85".into()),
        ("package_weight".into(), "3.5".into()),
        ("delivery_distance".into(), "180".into()),
        ("loyalty_points".into(), "6500".into()),
        ("coupon_percent".into(), "10".into()),
        ("is_fragile".into(), "true".into()),
        ("is_express".into(), "true".into()),
        ("is_hazardous".into(), "false".into()),
        ("is_gift".into(), "false".into()),
        ("is_first_time".into(), "false".into()),
    ])
}

fn load_order_pipeline() -> (Engine, HashMap<String, String>) {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(Arc::new(PathBuf::from(
                "engine/benches/specs/order_pipeline.lemma",
            ))),
            include_str!("../benches/specs/order_pipeline.lemma"),
        )])
        .expect("load order_pipeline");
    (engine, order_pipeline_inputs())
}

fn explanation_tree_has_rule_node(value: &serde_json::Value, rule_name: &str) -> bool {
    match value {
        serde_json::Value::Object(obj) => {
            if obj.get("type").and_then(|t| t.as_str()) == Some("rule")
                && obj.get("name").and_then(|r| r.as_str()) == Some(rule_name)
            {
                return true;
            }
            obj.values()
                .any(|v| explanation_tree_has_rule_node(v, rule_name))
        }
        serde_json::Value::Array(arr) => arr
            .iter()
            .any(|v| explanation_tree_has_rule_node(v, rule_name)),
        _ => false,
    }
}

#[test]
fn targeted_eval_one_rule_explain_false() {
    let (engine, data) = load_order_pipeline();
    let now = DateTimeValue::now();
    let response = engine
        .run(
            None,
            "bench_order_pipeline",
            Some(&now),
            data,
            Some(&["grand_total".to_string()]),
            false,
        )
        .expect("run");

    assert_eq!(response.results.len(), 1);
    let grand_total = response.results.get("grand_total").expect("grand_total");
    assert!(!grand_total.vetoed);
    assert!(grand_total.result().is_some());
    assert!(!response.results.contains_key("is_high_value"));
}

#[test]
fn explain_true_subset_full_embed() {
    let (engine, data) = load_order_pipeline();
    let now = DateTimeValue::now();
    let response = engine
        .run(
            None,
            "bench_order_pipeline",
            Some(&now),
            data,
            Some(&["grand_total".to_string()]),
            true,
        )
        .expect("run");

    assert_eq!(response.results.len(), 1);
    let grand_total = response.results.get("grand_total").expect("grand_total");
    let explanation = grand_total
        .explanation
        .as_ref()
        .expect("grand_total explanation");
    let json: serde_json::Value = serde_json::to_value(explanation).expect("serialize");
    assert!(
        explanation_tree_has_rule_node(&json, "pre_tax_total"),
        "expected embedded rule node for pre_tax_total, got: {json}"
    );
}

#[test]
fn empty_rules_errors() {
    let (engine, data) = load_order_pipeline();
    let now = DateTimeValue::now();
    let err = engine
        .run(
            None,
            "bench_order_pipeline",
            Some(&now),
            data,
            Some(&[]),
            false,
        )
        .expect_err("empty rules");
    assert!(err.to_string().contains("at least one rule required"));
}

#[test]
fn unknown_rule_errors() {
    let (engine, data) = load_order_pipeline();
    let now = DateTimeValue::now();
    let err = engine
        .run(
            None,
            "bench_order_pipeline",
            Some(&now),
            data,
            Some(&["not_a_real_rule".to_string()]),
            false,
        )
        .expect_err("unknown rule");
    assert!(err.to_string().contains("not_a_real_rule"));
}

#[test]
fn all_local_rules_explain_false() {
    let (engine, data) = load_order_pipeline();
    let now = DateTimeValue::now();
    let response = engine
        .run(None, "bench_order_pipeline", Some(&now), data, None, false)
        .expect("run");
    let rule_count = engine
        .show(None, "bench_order_pipeline", Some(&now))
        .expect("show")
        .rules
        .len();
    assert_eq!(response.results.len(), rule_count);
    assert!(response.results.contains_key("grand_total"));
    assert!(response.results.contains_key("is_high_value"));
}

#[test]
fn ratio_canonical_matches_rule_result_value() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Volatile,
            r#"
spec ratio_canonical
rule rate: 23 percent
"#
            .to_string(),
        )])
        .expect("load");
    let now = DateTimeValue::now();
    let response = engine
        .run(
            None,
            "ratio_canonical",
            Some(&now),
            HashMap::new(),
            None,
            false,
        )
        .expect("run");
    let rate = response.results.get("rate").expect("rate key canonical");
    assert_eq!(rate.rule.name, "rate");
    assert_eq!(rate.result(), Some("23%"));
}

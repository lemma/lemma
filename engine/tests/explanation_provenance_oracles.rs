//! Explain value parity and folded cross-rule product embeds.

use lemma::{DateTimeValue, Engine};
use std::collections::HashMap;

fn load(code: &str) -> Engine {
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("spec must load");
    engine
}

fn run(
    engine: &Engine,
    spec: &str,
    data: HashMap<String, String>,
    rules: Option<&[String]>,
    explain: bool,
) -> lemma::Response {
    let now = DateTimeValue::now();
    engine
        .run(None, spec, Some(&now), data, rules, explain)
        .expect("evaluation must succeed")
}

fn explanation_tree_rule_nodes(value: &serde_json::Value) -> Vec<&serde_json::Value> {
    let mut found = Vec::new();
    fn walk<'a>(value: &'a serde_json::Value, found: &mut Vec<&'a serde_json::Value>) {
        match value {
            serde_json::Value::Object(obj) => {
                if obj.get("type").and_then(|t| t.as_str()) == Some("rule") {
                    found.push(value);
                }
                for child in obj.values() {
                    walk(child, found);
                }
            }
            serde_json::Value::Array(arr) => {
                for child in arr {
                    walk(child, found);
                }
            }
            _ => {}
        }
    }
    walk(value, &mut found);
    found
}

/// Same spec/data/rules: response-rule values identical for explain true vs false;
/// explanation absent when false, present when true.
#[test]
fn explain_true_false_response_rule_value_parity() {
    let engine = load(
        r#"
spec parity_oracle
data price: 100
data qty: 3
rule line: price * qty
rule total: line
"#,
    );
    let data = HashMap::from([("price".into(), "100".into()), ("qty".into(), "3".into())]);
    let rules = ["total".to_string()];

    let without = run(&engine, "parity_oracle", data.clone(), Some(&rules), false);
    let with = run(&engine, "parity_oracle", data, Some(&rules), true);

    assert_eq!(without.results.len(), with.results.len());
    for (name, left) in &without.results {
        let right = with
            .results
            .get(name)
            .unwrap_or_else(|| panic!("rule '{name}' missing from explain:true response"));
        assert_eq!(left.vetoed, right.vetoed, "vetoed mismatch for {name}");
        assert_eq!(left.result(), right.result(), "display mismatch for {name}");
        assert_eq!(left.rule.name, right.rule.name);
        assert!(
            left.explanation.is_none(),
            "explain:false must not attach explanation for {name}"
        );
        assert!(
            right.explanation.is_some(),
            "explain:true must attach explanation for {name}"
        );
    }
}

/// Folded `sqrt_two * sqrt_two` still explains with two `sqrt_two` rule references.
#[test]
fn folded_sqrt_product_explanation_embeds_both_sqrt_two_rules() {
    let engine = load(
        r#"
spec fold_embed_oracle
rule sqrt_two: sqrt 2
rule sqrt_product: sqrt_two * sqrt_two
"#,
    );
    let rules = ["sqrt_product".to_string()];

    let values_only = run(
        &engine,
        "fold_embed_oracle",
        HashMap::new(),
        Some(&rules),
        false,
    );
    let product_value = values_only
        .results
        .get("sqrt_product")
        .expect("sqrt_product in response");
    assert!(!product_value.vetoed);
    assert_eq!(
        product_value.result(),
        Some("2"),
        "folded product value must be 2, got {:?}",
        product_value.result()
    );

    let response = run(
        &engine,
        "fold_embed_oracle",
        HashMap::new(),
        Some(&rules),
        true,
    );

    assert_eq!(response.results.len(), 1);
    let product = response
        .results
        .get("sqrt_product")
        .expect("sqrt_product in response");
    assert_eq!(product.result(), product_value.result());
    assert_eq!(product.vetoed, product_value.vetoed);

    let explanation = product
        .explanation
        .as_ref()
        .expect("sqrt_product explanation");
    let json = serde_json::to_value(explanation).expect("serialize explanation");
    let rule_nodes = explanation_tree_rule_nodes(&json);
    let sqrt_two_embeds: Vec<_> = rule_nodes
        .iter()
        .filter(|node| node.get("name").and_then(|r| r.as_str()) == Some("sqrt_two"))
        .collect();
    assert_eq!(
        sqrt_two_embeds.len(),
        2,
        "expected two sqrt_two rule references in explanation, got {json}"
    );
    for embed in &sqrt_two_embeds {
        let result = embed
            .get("result")
            .and_then(|r| r.as_str())
            .expect("rule reference result string");
        assert!(
            !result.is_empty(),
            "sqrt_two embed must carry a result, got {embed}"
        );
    }
}

/// Statically true unless wins: explanation body/result match the winner;
/// winning condition is narrated as a held cause.
#[test]
fn static_true_unless_winner_explains_winner_not_default() {
    let engine = load(
        r#"
spec static_true_unless_oracle
rule out: 1 unless true then 2
"#,
    );
    let rules = ["out".to_string()];
    let response = run(
        &engine,
        "static_true_unless_oracle",
        HashMap::new(),
        Some(&rules),
        true,
    );
    let out = response.results.get("out").expect("out in response");
    assert!(!out.vetoed);
    assert_eq!(out.result(), Some("2"));

    let explanation = out.explanation.as_ref().expect("out explanation");
    assert_eq!(explanation.body, "2");
    assert_eq!(explanation.causes.len(), 1);
    assert_eq!(explanation.causes[0].condition, "true");
    assert_eq!(explanation.causes[0].value, "true");
}

/// Statically false unless: default body, falsified arm narrated as a cause.
#[test]
fn static_false_unless_explains_default_without_false_winner() {
    let engine = load(
        r#"
spec static_false_unless_oracle
rule out: 1 unless false then 2
"#,
    );
    let rules = ["out".to_string()];
    let response = run(
        &engine,
        "static_false_unless_oracle",
        HashMap::new(),
        Some(&rules),
        true,
    );
    let out = response.results.get("out").expect("out in response");
    assert!(!out.vetoed);
    assert_eq!(out.result(), Some("1"));

    let explanation = out.explanation.as_ref().expect("out explanation");
    assert_eq!(explanation.body, "1");
    assert_eq!(explanation.causes.len(), 1);
    assert_eq!(explanation.causes[0].condition, "false");
    assert_eq!(explanation.causes[0].value, "false");
}

/// A → B → C: outermost explanation embeds B, which embeds A (multi-hop).
#[test]
fn multi_hop_rule_ref_embeds_nest() {
    let engine = load(
        r#"
spec multi_hop_oracle
rule a: 10
rule b: a
rule c: b
"#,
    );
    let rules = ["c".to_string()];
    let response = run(
        &engine,
        "multi_hop_oracle",
        HashMap::new(),
        Some(&rules),
        true,
    );
    let c = response.results.get("c").expect("c in response");
    assert_eq!(c.result(), Some("10"));
    let explanation = c.explanation.as_ref().expect("c explanation");
    let json = serde_json::to_value(explanation).expect("serialize");
    let rule_nodes = explanation_tree_rule_nodes(&json);
    let b_embeds: Vec<_> = rule_nodes
        .iter()
        .filter(|node| node.get("name").and_then(|r| r.as_str()) == Some("b"))
        .collect();
    assert_eq!(b_embeds.len(), 1, "c must embed b once, got {json}");
    let b_children = b_embeds[0]
        .get("children")
        .and_then(|c| c.as_array())
        .expect("b embed children");
    let a_in_b = b_children.iter().any(|child| {
        child.get("type").and_then(|t| t.as_str()) == Some("rule")
            && child.get("name").and_then(|r| r.as_str()) == Some("a")
    });
    assert!(
        a_in_b,
        "b embed must nest an a rule child, got {b_embeds:?}"
    );
}

/// Nested unless via rule reference: outer arm taken; body/result is the inner winner.
#[test]
fn nested_piecewise_outer_taken_explains_inner_winner() {
    let engine = load(
        r#"
spec nested_piecewise_oracle
data outer: true
data inner: true
rule inner_choice: 2 unless inner then 3
rule out: 1 unless outer then inner_choice
"#,
    );
    let rules = ["out".to_string()];
    let data = HashMap::from([
        ("outer".into(), "true".into()),
        ("inner".into(), "true".into()),
    ]);
    let response = run(&engine, "nested_piecewise_oracle", data, Some(&rules), true);
    let out = response.results.get("out").expect("out in response");
    assert!(!out.vetoed);
    assert_eq!(out.result(), Some("3"));
    let explanation = out.explanation.as_ref().expect("out explanation");
    let json = serde_json::to_value(explanation).expect("serialize");
    assert_eq!(json["result"].as_str(), Some("3"));
    let causes = json["causes"].as_array().expect("outer causes");
    assert_eq!(
        causes.len(),
        1,
        "outer taken arm is the sole cause, got {json}"
    );
    assert_eq!(causes[0]["value"].as_str(), Some("true"));
    let rule_nodes = explanation_tree_rule_nodes(&json);
    let inner_embeds: Vec<_> = rule_nodes
        .iter()
        .filter(|node| node.get("name").and_then(|r| r.as_str()) == Some("inner_choice"))
        .collect();
    assert_eq!(
        inner_embeds.len(),
        1,
        "outer winner must embed inner_choice, got {json}"
    );
}

/// Vetoed source into unit conversion: veto text on result; children follow the walk
/// (data), not a synthetic veto node.
#[test]
fn vetoed_unit_conversion_explains_with_veto_result() {
    let engine = load(
        r#"
spec veto_conversion_oracle
data mass: measure -> unit kg: 1
data zero: 0
rule as_kg: (1 / zero) as kg
"#,
    );
    let rules = ["as_kg".to_string()];
    let response = run(
        &engine,
        "veto_conversion_oracle",
        HashMap::new(),
        Some(&rules),
        true,
    );
    let as_kg = response.results.get("as_kg").expect("as_kg in response");
    assert!(
        as_kg.vetoed,
        "1/0 as kg must veto, got display={:?}",
        as_kg.result()
    );
    let explanation = as_kg.explanation.as_ref().expect("as_kg explanation");
    let json = serde_json::to_value(explanation).expect("serialize");
    assert_eq!(
        json["result"].as_str(),
        Some("Division by zero"),
        "veto text on display, got {json}"
    );
    assert!(
        explanation_tree_has_type(&json, "data"),
        "walk children preserved (data for zero), got {json}"
    );
}

fn explanation_tree_has_type(value: &serde_json::Value, type_name: &str) -> bool {
    match value {
        serde_json::Value::Object(obj) => {
            if obj.get("type").and_then(|t| t.as_str()) == Some(type_name) {
                return true;
            }
            obj.values()
                .any(|v| explanation_tree_has_type(v, type_name))
        }
        serde_json::Value::Array(arr) => {
            arr.iter().any(|v| explanation_tree_has_type(v, type_name))
        }
        _ => false,
    }
}

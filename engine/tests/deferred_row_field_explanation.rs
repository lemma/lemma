//! A list row field bound to a rule (`-> with bonus: extra`) narrates as that rule.

use indexmap::IndexMap;
use lemma::{
    format_explanation, DateTimeValue, Engine, EvaluateDataValue, Explanation, SourceType,
};
use serde_json::Value;

const SPEC: &str = include_str!("fixtures/explain/deferred_row_field.lemma");

fn explain(spec: &str, rule: &str) -> Explanation {
    let mut engine = Engine::new();
    engine
        .load([(SourceType::Volatile, SPEC.to_string())])
        .expect("spec must load");
    let now = DateTimeValue::now();
    let response = engine
        .evaluate(
            None,
            spec,
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

/// Fields of the one row node in `explanation`.
fn row_fields(explanation: &Explanation) -> Vec<Value> {
    let json = serde_json::to_value(explanation).expect("serialize");
    let mut rows = Vec::new();
    let mut pending = vec![&json];
    while let Some(node) = pending.pop() {
        match node {
            Value::Object(object) => {
                if object.get("type").and_then(Value::as_str) == Some("row") {
                    rows.push(object["fields"].as_array().expect("row fields").clone());
                }
                pending.extend(object.values());
            }
            Value::Array(items) => pending.extend(items),
            _ => {}
        }
    }
    assert_eq!(rows.len(), 1, "one row node in {json}");
    rows.remove(0)
}

fn assert_rule_ref_field(explanation: &Explanation, name: &str, result: &str) {
    let fields = row_fields(explanation);
    assert_eq!(fields.len(), 1, "{fields:?}");
    assert_eq!(fields[0]["type"], "rule_ref", "{fields:?}");
    assert_eq!(fields[0]["name"], name, "{fields:?}");
    assert_eq!(fields[0]["result"], result, "{fields:?}");
}

#[test]
fn row_field_bound_to_a_root_rule_narrates_as_the_rule() {
    let explanation = explain("own_rule", "first_bonus");
    assert_rule_ref_field(&explanation, "extra", "5");
    assert_eq!(
        format_explanation(&explanation),
        include_str!("fixtures/explain/deferred_row_field_own_rule.ascii.txt")
    );
}

#[test]
fn row_field_bound_to_a_member_rule_narrates_as_the_member_rule() {
    let explanation = explain("member_rule", "first_bonus");
    assert_rule_ref_field(&explanation, "src.rate", "7");
    assert_eq!(
        format_explanation(&explanation),
        include_str!("fixtures/explain/deferred_row_field_member_rule.ascii.txt")
    );
}

//! Resolve serialized `rule_ref` nodes against a root explanation's rules table.

use serde_json::Value;

/// Look up a serialized rules-table entry by id under a root explanation JSON object.
pub fn json_rule_by_id(explanation: &Value, id: u64) -> &Value {
    explanation["rules"]
        .as_array()
        .expect("explanation.rules")
        .iter()
        .find(|rule| rule["id"].as_u64() == Some(id))
        .unwrap_or_else(|| panic!("explanation missing rules entry for id {id}"))
}

/// Resolve a serialized `rule_ref` node against the root explanation's rules table.
pub fn json_resolve_ref<'a>(explanation: &'a Value, node: &Value) -> &'a Value {
    assert_eq!(node["type"], "rule_ref", "expected rule_ref, got {node}");
    let id = node["id"].as_u64().expect("rule_ref.id");
    json_rule_by_id(explanation, id)
}

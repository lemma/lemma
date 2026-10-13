//! Walk serialized explanation JSON for root / rules-table entries.

use serde_json::Value;

/// True when `value` is a root `"type":"rule"` node or a rules-table entry (`id` + `body`).
fn json_is_explained_rule(value: &Value) -> bool {
    match value.as_object() {
        Some(obj) if obj.get("type").and_then(|t| t.as_str()) == Some("rule") => true,
        Some(obj)
            if obj.contains_key("id")
                && obj.contains_key("body")
                && obj.contains_key("name")
                && !obj.contains_key("type") =>
        {
            true
        }
        _ => false,
    }
}

/// Walk JSON for root rule nodes and rules-table entries (not `rule_ref` stubs).
pub fn json_explained_rules(value: &Value) -> Vec<&Value> {
    let mut found = Vec::new();
    fn walk<'a>(value: &'a Value, found: &mut Vec<&'a Value>) {
        match value {
            Value::Object(obj) => {
                if json_is_explained_rule(value) {
                    found.push(value);
                }
                for child in obj.values() {
                    walk(child, found);
                }
            }
            Value::Array(arr) => {
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

//! JSON → [`EvaluateDataValue`] tree. The only serde_json conversion into run data.

use crate::evaluation::evaluate_data::{
    attribute_evaluate_data_error, evaluate_data_from_entries, evaluate_data_map_from_entries,
    evaluate_data_null_list_item, EvaluateDataValue,
};
use crate::Error;
use indexmap::IndexMap;
use serde_json::Value;

/// Parse one JSON value into a [`EvaluateDataValue`]. JSON `null` means omit: `Ok(None)`.
/// A `null` list item is a request error: a list has no unsupplied row.
///
/// JSON numbers at any depth are a request error: wire JSON has no exact native type.
pub fn evaluate_data_value_from_json_value(
    value: Value,
) -> Result<Option<EvaluateDataValue>, Error> {
    let mut frames: Vec<JsonFrame> = Vec::new();
    let mut step = JsonStep::Descend(value);
    loop {
        step = match step {
            JsonStep::Descend(value) => match value {
                Value::Null => JsonStep::Deliver(None),
                Value::String(s) => JsonStep::Deliver(Some(EvaluateDataValue::Literal(s))),
                Value::Bool(b) => JsonStep::Deliver(Some(EvaluateDataValue::Boolean(b))),
                Value::Number(n) => {
                    return Err(Error::request(
                        format!("pass as string: \"{n}\""),
                        Some("JSON numbers have no exact native type"),
                    ))
                }
                Value::Object(obj) => {
                    frames.push(JsonFrame::Object {
                        entries: Vec::with_capacity(obj.len()),
                        children: obj.into_iter(),
                        pending_key: None,
                    });
                    JsonStep::NextChild
                }
                Value::Array(items) => {
                    frames.push(JsonFrame::Array {
                        list: Vec::with_capacity(items.len()),
                        children: items.into_iter(),
                    });
                    JsonStep::NextChild
                }
            },
            JsonStep::NextChild => {
                let frame = frames
                    .last_mut()
                    .expect("BUG: json frame stack empty while reading a child");
                match frame.next_child() {
                    Some(child) => JsonStep::Descend(child),
                    None => {
                        let frame = frames
                            .pop()
                            .expect("BUG: json frame stack empty on completion");
                        JsonStep::Deliver(Some(frame.finish()?))
                    }
                }
            }
            JsonStep::Deliver(item) => match frames.last_mut() {
                None => return Ok(item),
                Some(JsonFrame::Object {
                    entries,
                    pending_key,
                    ..
                }) => {
                    if let Some(parsed) = item {
                        let key = pending_key
                            .take()
                            .expect("BUG: json object child delivered without its key");
                        entries.push((key, parsed));
                    }
                    JsonStep::NextChild
                }
                Some(JsonFrame::Array { list, .. }) => {
                    let parsed =
                        item.ok_or_else(|| evaluate_data_null_list_item(list.len() + 1))?;
                    list.push(parsed);
                    JsonStep::NextChild
                }
            },
        };
    }
}

enum JsonStep {
    Descend(Value),
    NextChild,
    Deliver(Option<EvaluateDataValue>),
}

enum JsonFrame {
    Object {
        entries: Vec<(String, EvaluateDataValue)>,
        children: serde_json::map::IntoIter,
        pending_key: Option<String>,
    },
    Array {
        list: Vec<EvaluateDataValue>,
        children: std::vec::IntoIter<Value>,
    },
}

impl JsonFrame {
    fn next_child(&mut self) -> Option<Value> {
        match self {
            JsonFrame::Object {
                children,
                pending_key,
                ..
            } => children.next().map(|(key, child)| {
                *pending_key = Some(key);
                child
            }),
            JsonFrame::Array { children, .. } => children.next(),
        }
    }

    fn finish(self) -> Result<EvaluateDataValue, Error> {
        match self {
            JsonFrame::Object { entries, .. } => evaluate_data_map_from_entries(entries),
            JsonFrame::Array { list, .. } => Ok(EvaluateDataValue::List(list)),
        }
    }
}

/// Optional `data` object from MCP / HTTP JSON → overlay tree for [`crate::Engine::evaluate`].
/// `null` / omitted → empty map. Non-object → error. JSON `null` values are omitted.
pub fn parse_evaluate_data_object(
    data: &Option<Value>,
) -> Result<IndexMap<String, EvaluateDataValue>, Error> {
    match data {
        None | Some(Value::Null) => Ok(IndexMap::new()),
        Some(Value::Object(map)) => {
            let mut entries = Vec::with_capacity(map.len());
            for (key, value) in map {
                let parsed = evaluate_data_value_from_json_value(value.clone())
                    .map_err(|e| attribute_evaluate_data_error(e, key))?;
                if let Some(parsed) = parsed {
                    entries.push((key.clone(), parsed));
                }
            }
            evaluate_data_from_entries(entries)
        }
        Some(_) => Err(Error::request("data must be a JSON object", None::<String>)),
    }
}

/// MCP `rules` / HTTP query `?rule=` — one name, a JSON array of names, or omit (all).
pub fn resolve_evaluate_rules(rules: &Option<Value>) -> Result<Option<Vec<String>>, Error> {
    match rules {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => {
            let name = s.trim();
            if name.is_empty() {
                return Err(Error::request("rules must not be empty", None::<String>));
            }
            Ok(Some(vec![name.to_string()]))
        }
        Some(Value::Array(arr)) => {
            if arr.is_empty() {
                return Err(Error::request("rules must not be empty", None::<String>));
            }
            let mut names = Vec::with_capacity(arr.len());
            for (i, v) in arr.iter().enumerate() {
                match v {
                    Value::String(s) => {
                        let name = s.trim();
                        if name.is_empty() {
                            return Err(Error::request(
                                format!("rules[{i}] must not be empty"),
                                None::<String>,
                            ));
                        }
                        names.push(name.to_string());
                    }
                    _ => {
                        return Err(Error::request(
                            format!("rules[{i}] must be a string"),
                            None::<String>,
                        ));
                    }
                }
            }
            Ok(Some(names))
        }
        Some(_) => Err(Error::request(
            "rules must be a string or an array of strings",
            None::<String>,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_null_is_omitted() {
        assert_eq!(
            evaluate_data_value_from_json_value(Value::Null).expect("null is omit"),
            None
        );
    }

    #[test]
    fn json_nesting_beyond_serde_limit_fails_to_parse() {
        let text = format!("{}1{}", "[".repeat(1000), "]".repeat(1000));
        assert!(serde_json::from_str::<Value>(&text).is_err());
    }

    #[test]
    fn json_number_is_request_error() {
        let err = evaluate_data_value_from_json_value(serde_json::json!(84)).expect_err("number");
        assert!(err.message().contains("pass as string: \"84\""), "{err}");
    }

    #[test]
    fn json_number_error_names_top_level_key() {
        let err = parse_evaluate_data_object(&Some(serde_json::json!({
            "price": { "eur": 84 }
        })))
        .expect_err("nested number");
        assert!(err.message().contains("pass as string: \"84\""), "{err}");
        assert_eq!(err.related_data(), Some("price"));
    }

    #[test]
    fn parse_evaluate_data_object_null_field_is_omitted() {
        let only_null = Some(serde_json::json!({ "x": null }));
        let empty = parse_evaluate_data_object(&only_null).expect("null field omits");
        assert!(empty.is_empty(), "got: {empty:?}");

        let mixed = Some(serde_json::json!({ "x": null, "y": "1" }));
        let map = parse_evaluate_data_object(&mixed).expect("null field omits");
        assert_eq!(map.len(), 1);
        assert_eq!(
            map.get("y"),
            Some(&EvaluateDataValue::Literal("1".to_string()))
        );
        assert!(!map.contains_key("x"));
    }

    #[test]
    fn null_list_item_is_request_error_naming_position_and_key() {
        let err = parse_evaluate_data_object(&Some(serde_json::json!({
            "fares": ["89 eur", null, "120 eur"]
        })))
        .expect_err("null list item");
        assert_eq!(err.message(), "list item 2 is null");
        assert_eq!(err.related_data(), Some("fares"));
    }

    #[test]
    fn null_nested_list_item_is_request_error() {
        let err = parse_evaluate_data_object(&Some(serde_json::json!({
            "components": [{ "own_cost": "5 eur", "components": [null] }]
        })))
        .expect_err("nested null list item");
        assert_eq!(err.message(), "list item 1 is null");
        assert_eq!(err.related_data(), Some("components"));
    }

    #[test]
    fn null_field_inside_list_row_is_omitted() {
        let parsed = parse_evaluate_data_object(&Some(serde_json::json!({
            "staff": [{ "name": "Ada", "salary": null }]
        })))
        .expect("null row field omits");
        let Some(EvaluateDataValue::List(rows)) = parsed.get("staff") else {
            panic!("expected list, got {parsed:?}");
        };
        assert_eq!(rows.len(), 1);
        let EvaluateDataValue::Map(row) = &rows[0] else {
            panic!("expected row map, got {:?}", rows[0]);
        };
        assert_eq!(row.len(), 1);
        assert_eq!(
            row.get("name"),
            Some(&EvaluateDataValue::Literal("Ada".to_string()))
        );
    }

    #[test]
    fn parse_evaluate_data_object_unit_map_and_array() {
        let parsed = parse_evaluate_data_object(&Some(serde_json::json!({
            "price": { "eur": "84" },
            "flags": [true, "x"]
        })))
        .expect("object");
        match parsed.get("price") {
            Some(EvaluateDataValue::Map(m)) => {
                assert_eq!(
                    m.get("eur"),
                    Some(&EvaluateDataValue::Literal("84".to_string()))
                );
            }
            other => panic!("expected map, got {other:?}"),
        }
        match parsed.get("flags") {
            Some(EvaluateDataValue::List(items)) => {
                assert_eq!(items[0], EvaluateDataValue::Boolean(true));
                assert_eq!(items[1], EvaluateDataValue::Literal("x".to_string()));
            }
            other => panic!("expected list, got {other:?}"),
        }
    }

    #[test]
    fn duplicate_case_keys_error() {
        let err = parse_evaluate_data_object(&Some(serde_json::json!({
            "Qty": "1",
            "qty": "2"
        })))
        .expect_err("dup");
        assert!(err.message().contains("Duplicate data key"), "{err}");
    }

    #[test]
    fn duplicate_case_keys_in_nested_object_error() {
        let err = parse_evaluate_data_object(&Some(serde_json::json!({
            "price": { "EUR": "1", "eur": "2" }
        })))
        .expect_err("dup");
        assert!(
            err.message().contains("Duplicate object key 'eur'"),
            "{err}"
        );
        assert_eq!(err.related_data(), Some("price"));
    }
}

//! OpenAPI must not contradict itself on temporal field types.

use lemma::{Engine, SourceType};
use lemma_openapi::generate_openapi;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

#[test]
fn effective_from_has_one_consistent_declaration() {
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Path(Arc::new(PathBuf::from("sample.lemma"))),
            r#"
spec sample 2024-01-01
data n: number
rule r: n
"#
            .to_string(),
        )])
        .expect("load");
    let doc = generate_openapi(&engine, false);

    let mut declarations: BTreeMap<String, Vec<String>> = BTreeMap::new();
    collect_effective_from_types(&doc, "$", &mut declarations);

    for (path, types) in &declarations {
        let unique: BTreeSet<&String> = types.iter().collect();
        assert_eq!(
            unique.len(),
            1,
            "effective_from at {path} has contradictory types {types:?}; \
             today list uses object|null while show uses string|null"
        );
        let only = types[0].as_str();
        assert!(
            only.contains("string"),
            "effective_from must be string|null after temporal API fix, got {only} at {path}"
        );
    }
}

use std::collections::BTreeSet;

fn collect_effective_from_types(
    value: &serde_json::Value,
    path: &str,
    out: &mut BTreeMap<String, Vec<String>>,
) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(props) = map.get("properties").and_then(|p| p.as_object()) {
                if let Some(ef) = props.get("effective_from") {
                    out.entry(format!("{path}.properties.effective_from"))
                        .or_default()
                        .push(ef.to_string());
                }
            }
            for (key, child) in map {
                collect_effective_from_types(child, &format!("{path}.{key}"), out);
            }
        }
        serde_json::Value::Array(items) => {
            for (i, child) in items.iter().enumerate() {
                collect_effective_from_types(child, &format!("{path}[{i}]"), out);
            }
        }
        _ => {}
    }
}

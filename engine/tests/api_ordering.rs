//! Map key order in the API output is part of the contract.
//!
//! `Show.data`, `Show.rules`, `Show.meta`, `Response.results`, and
//! `Response.rows` / nested `SpecResult` `data`/`rules` are `IndexMap` in Rust,
//! preserving declaration/dependency order instead of alphabetical `BTreeMap`
//! or `HashMap` randomization.

use indexmap::IndexMap;
use lemma::{DateTimeValue, Engine};
use std::path::PathBuf;
use std::sync::Arc;

fn path_source(file: &str) -> lemma::SourceType {
    lemma::SourceType::Path(Arc::new(PathBuf::from(file)))
}

fn load(code: &str, file: &str) -> Engine {
    let mut engine = Engine::new();
    engine
        .load([(path_source(file), code.to_string())])
        .unwrap_or_else(|e| panic!("load must succeed: {e:?}"));
    engine
}

fn json_object_keys(value: &serde_json::Value) -> Vec<String> {
    value.as_object().expect("object").keys().cloned().collect()
}

const META_ORDER_SPEC: &str = r#"
spec ordered
meta zebra: "z"
meta yankee: "y"
meta xray: "x"
meta whiskey: "w"
meta victor: "v"
meta uniform: "u"
data n: number
rule r: n
"#;

const DATA_ORDER_SPEC: &str = r#"
spec data_order
data zebra: number
data yankee: number
data alpha: number
rule r: zebra + yankee + alpha
"#;

const RULE_TOPO_SPEC: &str = r#"
spec topo
data n: number
rule z: n
rule a: z * 2
"#;

#[test]
fn show_meta_iterates_in_declaration_order() {
    let engine = load(META_ORDER_SPEC, "ordered.lemma");
    let now = DateTimeValue::now();
    let show = engine.show(None, "ordered", Some(&now)).expect("show");
    let keys: Vec<_> = show.meta.keys().cloned().collect();
    assert_eq!(
        keys,
        vec![
            "zebra".to_string(),
            "yankee".to_string(),
            "xray".to_string(),
            "whiskey".to_string(),
            "victor".to_string(),
            "uniform".to_string(),
        ],
        "meta must preserve declaration order, not HashMap randomization"
    );

    let json = lemma::api::show_document(&engine, &show);
    assert_eq!(
        json_object_keys(&json["x-lemma-meta"]),
        vec!["zebra", "yankee", "xray", "whiskey", "victor", "uniform"]
    );
}

#[test]
fn show_data_iterates_in_declaration_order() {
    let engine = load(DATA_ORDER_SPEC, "data_order.lemma");
    let now = DateTimeValue::now();
    let show = engine.show(None, "data_order", Some(&now)).expect("show");
    let keys: Vec<_> = show.data.keys().cloned().collect();
    assert_eq!(keys, vec!["zebra", "yankee", "alpha"]);
    let json = lemma::api::show_document(&engine, &show);
    assert_eq!(
        json_object_keys(&json["properties"]),
        vec!["zebra", "yankee", "alpha"]
    );
}

#[test]
fn show_rules_follow_topological_not_alphabetical_order() {
    let engine = load(RULE_TOPO_SPEC, "topo.lemma");
    let now = DateTimeValue::now();
    let show = engine.show(None, "topo", Some(&now)).expect("show");
    let keys: Vec<_> = show.rules.keys().cloned().collect();
    assert_eq!(
        keys,
        vec!["z".to_string(), "a".to_string()],
        "rule z must precede a (topological); alphabetical would be a, z"
    );
}

#[test]
fn response_results_follow_same_order_as_local_show_rules() {
    let engine = load(RULE_TOPO_SPEC, "topo.lemma");
    let now = DateTimeValue::now();
    let show = engine.show(None, "topo", Some(&now)).expect("show");
    let show_keys: Vec<_> = show
        .rules
        .iter()
        .filter(|(_, rule)| rule.path.is_empty())
        .map(|(name, _)| name.clone())
        .collect();
    let response = engine
        .evaluate(
            None,
            "topo",
            Some(&now),
            IndexMap::from([("n".to_string(), "1".into())]),
            None,
            false,
        )
        .expect("evaluate");
    let result_keys: Vec<_> = response.results.keys().cloned().collect();
    assert_eq!(result_keys, show_keys);
    let json = serde_json::to_value(lemma::api::Response::from(&response)).expect("Response JSON");
    assert_eq!(json_object_keys(&json["results"]), show_keys);
}

const ROW_MEMBER_ORDER_SPEC: &str = r#"
spec xx
data z: number
data type: text
rule late: z
rule early: late + 1

spec test
uses zz: xx list
  -> key z
  -> with z: 5
  -> with z: 7, type: "hi"

rule x:
  zz -> find 7
"#;

#[test]
fn response_row_data_and_rules_follow_plan_order() {
    let engine = load(ROW_MEMBER_ORDER_SPEC, "row_order.lemma");
    let now = DateTimeValue::now();
    let response = engine
        .evaluate(None, "test", Some(&now), IndexMap::new(), None, false)
        .expect("evaluate");
    let row = response.rows.first().expect("one found row");
    let data_keys: Vec<_> = row.data.keys().cloned().collect();
    assert_eq!(
        data_keys,
        vec!["z".to_string(), "type".to_string()],
        "row data must follow declaration order; alphabetical would be type, z"
    );
    let rule_keys: Vec<_> = row.rules.keys().cloned().collect();
    assert_eq!(
        rule_keys,
        vec!["late".to_string(), "early".to_string()],
        "row rules must follow plan order; alphabetical would be early, late"
    );
    let json = serde_json::to_value(lemma::api::Response::from(&response)).expect("Response JSON");
    assert_eq!(
        json_object_keys(&json["rows"][0]["data"]),
        vec!["z", "type"]
    );
    assert_eq!(
        json_object_keys(&json["rows"][0]["rules"]),
        vec!["late", "early"]
    );
}

#[test]
fn load_errors_preserve_source_order() {
    // Ordered multi-source load: when several sources fail, errors follow input order.
    let mut engine = Engine::new();
    let err = engine
        .load([
            (path_source("zebra.lemma"), "this is not lemma".into()),
            (
                path_source("yankee.lemma"),
                "spec ok\nrule r: 1\n".to_string(),
            ),
            (path_source("xray.lemma"), "also not lemma".into()),
        ])
        .expect_err("two sources must fail parse");
    let attrs: Vec<_> = err
        .errors
        .iter()
        .filter_map(|e| e.location().map(|s| s.source_type.to_string()))
        .collect();
    assert_eq!(
        attrs,
        vec!["zebra.lemma".to_string(), "xray.lemma".to_string()],
        "parse errors must follow input source order, got {attrs:?}"
    );
}

#[test]
fn invalid_data_values_report_deterministic_vetoes_not_errors() {
    // Counterpart to the JNI HashMap in string_map_from_java: five keys, three invalid.
    // Per error-model.mdc, an invalid override is a Veto (data makes the rule impossible),
    // not an Error (which is reserved for invalid Lemma source) — matches
    // data_with_values_contract.rs's `override_wrong_primitive_kind_fails_with_related_data`
    // and friends. `run()` must succeed regardless of iteration order over the input map.
    const SPEC: &str = r#"
spec multi
data a: number
data b: number
data c: number
data d: number
data e: number
rule r: a
"#;
    let engine = load(SPEC, "multi.lemma");
    let now = DateTimeValue::now();
    let data = IndexMap::from([
        ("a".to_string(), "bad".into()),
        ("b".to_string(), "1".into()),
        ("c".to_string(), "bad".into()),
        ("d".to_string(), "2".into()),
        ("e".to_string(), "bad".into()),
    ]);
    let response = engine
        .evaluate(None, "multi", Some(&now), data, None, false)
        .expect("invalid data values must Veto, not Error");
    let rule_result = response.results.get("r").expect("rule 'r' present");
    assert!(
        rule_result.vetoed,
        "rule 'r' must veto when its only dependency 'a' has an invalid override, got {:?}",
        rule_result.result()
    );
    let reason = rule_result
        .veto_reason
        .as_deref()
        .expect("veto reason present");
    assert!(
        reason.contains("bad"),
        "veto reason must name the invalid value 'bad', got: {reason}"
    );
}

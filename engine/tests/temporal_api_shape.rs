//! Temporal fields in the public API must be ISO-8601 strings, never objects.
//!
//! Tell-tale for the object form: a `"year"` key anywhere in the JSON document.

use lemma::{DateTimeValue, Engine};
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
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

fn assert_no_year_key(value: &serde_json::Value, path: &str) {
    match value {
        serde_json::Value::Object(map) => {
            assert!(
                !map.contains_key("year"),
                "object form leaked: \"year\" key at {path}: {value}"
            );
            for (key, child) in map {
                assert_no_year_key(child, &format!("{path}.{key}"));
            }
        }
        serde_json::Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                assert_no_year_key(child, &format!("{path}[{index}]"));
            }
        }
        _ => {}
    }
}

fn assert_json_string_or_absent(value: &serde_json::Value, key: &str) {
    match value.get(key) {
        None => {}
        Some(serde_json::Value::String(_)) => {}
        Some(other) => panic!("{key} must be a JSON string or absent, got {other}"),
    }
}

fn assert_json_string(value: &serde_json::Value, key: &str) {
    match value.get(key) {
        Some(serde_json::Value::String(_)) => {}
        other => panic!("{key} must be a JSON string, got {other:?}"),
    }
}

fn assert_key_absent(value: &serde_json::Value, key: &str) {
    assert!(
        value.get(key).is_none(),
        "{key} must be omitted entirely, got {:?}",
        value.get(key)
    );
}

const TWO_VERSION_SPEC: &str = r#"
spec policy 2024-01-01
data amount: number
rule ok: amount

spec policy 2025-06-01
data amount: number
rule ok: amount * 2
"#;

const SINGLE_VERSION_SPEC: &str = r#"
spec solo 2024-01-01
data amount: number
rule ok: amount
"#;

const TEMPORAL_BOUNDS_SPEC: &str = r#"
spec bounds
data start: date
  -> minimum 2020-01-01
  -> maximum 2030-12-31
data when: time
  -> minimum 09:00:00
  -> maximum 17:00:00
rule the_start: start
rule the_when: when
"#;

const DATE_RANGE_VALUE_SPEC: &str = r#"
spec ranges
data window: 2020-01-01...2030-12-31
data shift: 09:00:00...17:00:00
rule the_window: window
rule the_shift: shift
"#;

const DATE_TIME_RULE_SPEC: &str = r#"
spec temporal_rules
data d: 2025-03-04
data t: 12:30:45
rule the_date: d
rule the_time: t
"#;

#[test]
fn show_effective_fields_are_json_strings() {
    let engine = load(TWO_VERSION_SPEC, "policy.lemma");
    let effective = DateTimeValue::from_str("2024-06-01").expect("effective");
    let show = engine.show(None, "policy", Some(&effective)).expect("show");
    let json = serde_json::to_value(lemma::api::Show::from(&show)).expect("serialize Show");
    assert_no_year_key(&json, "show");
    assert_json_string(&json, "effective_from");
    assert_json_string(&json, "effective_to");
    assert_eq!(json["effective_from"], "2024-01-01");
    assert_eq!(json["effective_to"], "2025-06-01");
}

#[test]
fn show_versions_entries_are_json_strings() {
    let engine = load(TWO_VERSION_SPEC, "policy.lemma");
    let effective = DateTimeValue::from_str("2024-06-01").expect("effective");
    let show = engine.show(None, "policy", Some(&effective)).expect("show");
    let json = serde_json::to_value(lemma::api::Show::from(&show)).expect("serialize Show");
    let versions = json["versions"]
        .as_array()
        .expect("versions must be an array");
    assert!(versions.len() >= 2, "expected two version rows");
    for (index, version) in versions.iter().enumerate() {
        assert_no_year_key(version, &format!("versions[{index}]"));
        assert_json_string_or_absent(version, "effective_from");
        assert_json_string_or_absent(version, "effective_to");
        if let Some(serde_json::Value::String(from)) = version.get("effective_from") {
            assert!(
                !from.contains("\"year\""),
                "version effective_from must not embed object JSON"
            );
        }
    }
}

#[test]
fn latest_version_omits_effective_to_key() {
    let engine = load(TWO_VERSION_SPEC, "policy.lemma");
    let effective = DateTimeValue::from_str("2025-07-01").expect("effective");
    let show = engine.show(None, "policy", Some(&effective)).expect("show");
    let json = serde_json::to_value(lemma::api::Show::from(&show)).expect("serialize Show");
    assert_json_string(&json, "effective_from");
    assert_key_absent(&json, "effective_to");

    let versions = json["versions"].as_array().expect("versions");
    let latest = versions
        .iter()
        .find(|v| v.get("effective_to").is_none())
        .expect("one version row must omit effective_to");
    assert_key_absent(latest, "effective_to");
    assert_json_string(latest, "effective_from");
}

#[test]
fn list_temporal_fields_are_json_strings() {
    let engine = load(TWO_VERSION_SPEC, "policy.lemma");
    let repos = engine.list();
    let json = serde_json::to_value(&repos).expect("serialize list");
    assert_no_year_key(&json, "list");

    let mut saw_policy = false;
    for repo in json.as_array().expect("list array") {
        for spec in repo["specs"].as_array().expect("specs") {
            if spec["name"] == "policy" {
                saw_policy = true;
                assert_json_string_or_absent(spec, "effective_from");
                assert_json_string_or_absent(spec, "effective_to");
            }
        }
    }
    assert!(saw_policy, "list must include policy");
}

#[test]
fn rule_result_date_and_time_are_json_strings() {
    let engine = load(DATE_TIME_RULE_SPEC, "temporal_rules.lemma");
    let now = DateTimeValue::now();
    let response = engine
        .run(
            None,
            "temporal_rules",
            Some(&now),
            HashMap::new(),
            None,
            false,
        )
        .expect("run");
    let json =
        serde_json::to_value(lemma::api::Response::from(&response)).expect("serialize Response");
    assert_no_year_key(&json, "response");
    assert_json_string(&json["results"]["the_date"], "date");
    assert_json_string(&json["results"]["the_time"], "time");
    assert_eq!(json["results"]["the_date"]["date"], "2025-03-04");
    assert_eq!(json["results"]["the_time"]["time"], "12:30:45");
}

#[test]
fn date_and_time_type_bounds_are_json_strings() {
    let engine = load(TEMPORAL_BOUNDS_SPEC, "bounds.lemma");
    let now = DateTimeValue::now();
    let show = engine.show(None, "bounds", Some(&now)).expect("show");
    let json = serde_json::to_value(lemma::api::Show::from(&show)).expect("serialize Show");
    assert_no_year_key(&json, "show");

    for name in ["start", "when"] {
        let ty = &json["data"][name]["type"];
        assert_json_string(ty, "minimum");
        assert_json_string(ty, "maximum");
    }
    assert_eq!(json["data"]["start"]["type"]["minimum"], "2020-01-01");
    assert_eq!(json["data"]["start"]["type"]["maximum"], "2030-12-31");
    assert_eq!(json["data"]["when"]["type"]["minimum"], "09:00:00");
    assert_eq!(json["data"]["when"]["type"]["maximum"], "17:00:00");
}

#[test]
fn date_range_and_time_range_values_have_string_endpoints() {
    let engine = load(DATE_RANGE_VALUE_SPEC, "ranges.lemma");
    let now = DateTimeValue::now();
    let response = engine
        .run(None, "ranges", Some(&now), HashMap::new(), None, false)
        .expect("run");
    let json =
        serde_json::to_value(lemma::api::Response::from(&response)).expect("serialize Response");
    assert_no_year_key(&json, "response");

    let window = &json["results"]["the_window"]["range"];
    assert_json_string(&window["from"], "date");
    assert_json_string(&window["to"], "date");
    assert_eq!(window["from"]["date"], "2020-01-01");
    assert_eq!(window["to"]["date"], "2030-12-31");

    let shift = &json["results"]["the_shift"]["range"];
    assert_json_string(&shift["from"], "time");
    assert_json_string(&shift["to"], "time");
    assert_eq!(shift["from"]["time"], "09:00:00");
    assert_eq!(shift["to"]["time"], "17:00:00");
}

#[test]
fn response_effective_remains_display_string() {
    let engine = load(SINGLE_VERSION_SPEC, "solo.lemma");
    let effective = DateTimeValue::from_str("2024-06-15T12:00:00Z").expect("effective");
    let response = engine
        .run(
            None,
            "solo",
            Some(&effective),
            HashMap::from([("amount".to_string(), "1".into())]),
            None,
            false,
        )
        .expect("run");
    let json =
        serde_json::to_value(lemma::api::Response::from(&response)).expect("serialize Response");
    assert_eq!(
        json["effective"],
        serde_json::Value::String(effective.to_string())
    );
    assert_no_year_key(&json, "response");
}

#[test]
fn date_granularity_absent_from_api() {
    let engine = load(TWO_VERSION_SPEC, "policy.lemma");
    let effective = DateTimeValue::from_str("2024-06-01").expect("effective");
    let show = engine.show(None, "policy", Some(&effective)).expect("show");
    let show_json = serde_json::to_value(lemma::api::Show::from(&show)).expect("Show");
    let show_text = serde_json::to_string(&show_json).expect("Show text");
    assert!(
        !show_text.contains("granularity"),
        "DateGranularity must not appear in the API output: {show_text}"
    );

    let response = engine
        .run(
            None,
            "policy",
            Some(&effective),
            HashMap::from([("amount".to_string(), "1".into())]),
            None,
            false,
        )
        .expect("run");
    let response_text =
        serde_json::to_string(&lemma::api::Response::from(&response)).expect("Response text");
    assert!(
        !response_text.contains("granularity"),
        "DateGranularity must not appear in the API output: {response_text}"
    );
}

#[test]
fn show_deserializes_from_iso_string_temporal_fields() {
    let engine = load(TWO_VERSION_SPEC, "policy.lemma");
    let effective = DateTimeValue::from_str("2024-06-01").expect("effective");
    let show = engine.show(None, "policy", Some(&effective)).expect("show");
    let show_json = serde_json::to_string(&lemma::api::Show::from(&show)).expect("serialize");
    let show_value: serde_json::Value = serde_json::from_str(&show_json).expect("parse");
    assert!(
        show_value["effective_from"].is_string(),
        "effective_from must be a JSON string before Show can round-trip: {}",
        show_value["effective_from"]
    );

    let restored: lemma::api::Show = serde_json::from_str(&show_json)
        .expect("Show must deserialize from ISO-8601 string temporal fields");
    assert_eq!(
        restored.effective_from.as_ref().map(|d| d.to_string()),
        Some("2024-01-01".to_string())
    );
}

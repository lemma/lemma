//! Unified error API contract.
//!
//! Until one derived error type replaces the three hand-built projections, these
//! tests assert against [`lemma::__test_support::current_binding_error_json`]
//! (today's WASM/`JsError` shape). Missing structured fields are the intended red.

use indexmap::IndexMap;
use lemma::__test_support::current_binding_error_json;
use lemma::{
    DateTimeValue, Engine, Error, RegistryErrorKind, RequestErrorKind, ResourceLimits, Source,
    SourceType,
};
use std::path::PathBuf;
use std::sync::Arc;

fn path_source(file: &str) -> SourceType {
    SourceType::Path(Arc::new(PathBuf::from(file)))
}

fn source(file: &str) -> Source {
    Source {
        source_type: path_source(file),
        span: lemma::Span {
            start: 0,
            end: 4,
            line: 1,
            col: 1,
        },
    }
}

#[test]
fn resource_limit_carries_structured_limit_fields() {
    let limits = ResourceLimits {
        max_source_size_bytes: 100,
        ..ResourceLimits::default()
    };
    let mut engine = Engine::with_limits(limits);
    let large = "spec test\ndata x: 1\n".repeat(10);
    let err = engine
        .load([(SourceType::Volatile, large)])
        .expect_err("oversize source");
    let limit = err
        .errors
        .iter()
        .find(|e| matches!(e, Error::ResourceLimitExceeded { .. }))
        .expect("ResourceLimitExceeded");
    let json = current_binding_error_json(limit);
    assert_eq!(json["kind"], "resource_limit");
    assert!(
        json.get("limit_name").and_then(|v| v.as_str()).is_some(),
        "limit_name must be a structured field, got {json}"
    );
    assert!(
        json.get("limit_value").and_then(|v| v.as_str()).is_some(),
        "limit_value must be a structured field, got {json}"
    );
    assert!(
        json.get("actual_value").and_then(|v| v.as_str()).is_some(),
        "actual_value must be a structured field, got {json}"
    );
    assert_eq!(json["limit_name"], "max_source_size_bytes");
}

#[test]
fn registry_error_carries_sub_kind() {
    let err = Error::registry(
        "not found",
        source("reg.lemma"),
        "@iso/missing",
        RegistryErrorKind::NotFound,
        None::<String>,
        None,
        None,
    );
    let json = current_binding_error_json(&err);
    assert_eq!(json["kind"], "registry");
    assert!(
        json.get("registry_kind")
            .or_else(|| json.get("sub_kind"))
            .and_then(|v| v.as_str())
            .is_some(),
        "registry sub-kind must be structured, got {json}"
    );
}

#[test]
fn request_error_carries_sub_kind() {
    let err = Error::request_not_found("no spec", None::<String>);
    let json = current_binding_error_json(&err);
    assert_eq!(json["kind"], "request");
    assert!(
        json.get("request_kind")
            .or_else(|| json.get("sub_kind"))
            .and_then(|v| v.as_str())
            .is_some(),
        "request sub-kind must be structured, got {json}"
    );
    match err {
        Error::Request {
            kind: RequestErrorKind::SpecNotFound,
            ..
        } => {}
        other => panic!("expected SpecNotFound, got {other:?}"),
    }
}

#[test]
fn source_location_uses_source_key_with_attribute_and_length() {
    let mut engine = Engine::new();
    let err = engine
        .load([(path_source("bad.lemma"), "this is not lemma".to_string())])
        .expect_err("parse");
    let json = current_binding_error_json(&err.errors[0]);
    let source = json
        .get("source")
        .expect("location key must be `source`, not `location`");
    assert!(source.get("attribute").and_then(|v| v.as_str()).is_some());
    assert!(source.get("line").and_then(|v| v.as_u64()).is_some());
    assert!(source.get("column").and_then(|v| v.as_u64()).is_some());
    assert!(
        source.get("length").and_then(|v| v.as_u64()).is_some(),
        "source.length must be present, got {source}"
    );
    assert!(
        json.get("location").is_none(),
        "must not emit Hex `location` key"
    );
}

#[test]
fn parse_error_carries_source() {
    let mut engine = Engine::new();
    let err = engine
        .load([(path_source("parse.lemma"), "spec x\ndata :".to_string())])
        .expect_err("parse");
    let json = current_binding_error_json(&err.errors[0]);
    assert_eq!(json["kind"], "parsing");
    assert!(json.get("source").is_some());
}

#[test]
fn validation_error_carries_attribution_fields_when_applicable() {
    let mut engine = Engine::new();
    let err = engine
        .load([(
            path_source("val.lemma"),
            r#"
spec s
data x: number
rule r: y
"#
            .to_string(),
        )])
        .expect_err("undefined y");
    let json = current_binding_error_json(&err.errors[0]);
    assert_eq!(json["kind"], "validation");
    // spec context is expected when validation fails inside a named spec
    assert!(
        json.get("spec").and_then(|v| v.as_str()).is_some()
            || json.get("related_data").and_then(|v| v.as_str()).is_some()
            || json.get("related_spec").and_then(|v| v.as_str()).is_some(),
        "validation API response must carry attribution fields when applicable: {json}"
    );
}

#[test]
fn all_seven_error_kind_snake_case_spellings_appear() {
    let mut kinds = std::collections::BTreeSet::new();

    let mut engine = Engine::new();
    let parse_err = engine
        .load([(path_source("p.lemma"), "not lemma".to_string())])
        .expect_err("parse");
    kinds.insert(
        current_binding_error_json(&parse_err.errors[0])["kind"]
            .as_str()
            .unwrap()
            .to_string(),
    );

    let val_err = Engine::new()
        .load([(
            path_source("v.lemma"),
            "spec s\ndata x: number\nrule r: missing\n".to_string(),
        )])
        .expect_err("validation");
    kinds.insert(
        current_binding_error_json(&val_err.errors[0])["kind"]
            .as_str()
            .unwrap()
            .to_string(),
    );

    let inv = Error::inversion("unsupported", Some(source("i.lemma")), None::<String>);
    kinds.insert(
        current_binding_error_json(&inv)["kind"]
            .as_str()
            .unwrap()
            .to_string(),
    );

    let reg = Error::registry(
        "gone",
        source("r.lemma"),
        "@x/y",
        RegistryErrorKind::NotFound,
        None::<String>,
        None,
        None,
    );
    kinds.insert(
        current_binding_error_json(&reg)["kind"]
            .as_str()
            .unwrap()
            .to_string(),
    );

    let missing = Error::missing_repository(
        "absent",
        Some(source("m.lemma")),
        "@iso/countries",
        None::<String>,
        None,
    );
    kinds.insert(
        current_binding_error_json(&missing)["kind"]
            .as_str()
            .unwrap()
            .to_string(),
    );

    let req = Error::request("bad", None::<String>);
    kinds.insert(
        current_binding_error_json(&req)["kind"]
            .as_str()
            .unwrap()
            .to_string(),
    );

    let limits = ResourceLimits {
        max_source_size_bytes: 50,
        ..ResourceLimits::default()
    };
    let limit_err = Engine::with_limits(limits)
        .load([(SourceType::Volatile, "spec t\ndata x: 1\n".repeat(20))])
        .expect_err("limit");
    kinds.insert(
        current_binding_error_json(&limit_err.errors[0])["kind"]
            .as_str()
            .unwrap()
            .to_string(),
    );

    let expected = [
        "parsing",
        "validation",
        "inversion",
        "registry",
        "missing_repository",
        "request",
        "resource_limit",
    ];
    for kind in expected {
        assert!(
            kinds.contains(kind),
            "missing ErrorKind API spelling {kind:?} in {kinds:?}"
        );
    }
    assert_eq!(kinds.len(), 7);
}

#[test]
fn evaluate_missing_spec_is_request_error_on_api() {
    let engine = Engine::new();
    let now = DateTimeValue::now();
    let err = engine
        .evaluate(None, "absent", Some(&now), IndexMap::new(), None, false)
        .expect_err("missing spec");
    let json = current_binding_error_json(&err);
    assert_eq!(json["kind"], "request");
}

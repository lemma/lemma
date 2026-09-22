use crate::computation::OperationResult;
use crate::evaluation::response::{EvaluatedRule, Response, RuleResult};
use crate::parsing::ast::Span;
use crate::planning::semantics::{LiteralValue, RulePath, Source};
use indexmap::IndexMap;
use rust_decimal::Decimal;

fn dummy_source() -> Source {
    Source::new(
        crate::parsing::source::SourceType::Volatile,
        Span {
            start: 0,
            end: 0,
            line: 1,
            col: 1,
        },
    )
}

fn dummy_rule(name: &str) -> EvaluatedRule {
    EvaluatedRule {
        name: name.to_string(),
        path: RulePath::new(vec![], name.to_string()),
        source_location: dummy_source(),
        rule_type: std::sync::Arc::clone(crate::planning::semantics::primitive_boolean_arc()),
    }
}

fn number_rule_result(name: &str, value: Decimal) -> RuleResult {
    RuleResult::from_operation_result(
        dummy_rule(name),
        &OperationResult::from_literal(LiteralValue::number_from_decimal(value)),
        crate::planning::semantics::primitive_number_arc().as_ref(),
        &crate::planning::unit_family::FamilyUnitCatalog::default(),
        None,
        Vec::new(),
    )
}

#[test]
fn test_response_serialization() {
    let mut results = IndexMap::new();
    results.insert(
        "test_rule".to_string(),
        number_rule_result("test_rule", 42.into()),
    );
    let response = Response {
        spec_name: "test_spec".to_string(),
        effective: "2026-01-01".to_string(),
        spec_effective_from: None,
        spec_effective_to: None,
        results,
    };

    let json = serde_json::to_string(&crate::api::Response::from(&response)).unwrap();
    let deserialized: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized["spec"], "test_spec");
    assert!(deserialized["results"]
        .as_object()
        .unwrap()
        .contains_key("test_rule"));
    assert_eq!(deserialized["results"]["test_rule"]["result"], "42");
    assert_eq!(deserialized["results"]["test_rule"]["number"], "42");
    assert_eq!(deserialized["results"]["test_rule"]["vetoed"], false);
}

#[test]
fn response_number_json_is_scalar() {
    let mut results = IndexMap::new();
    results.insert(
        "double".to_string(),
        number_rule_result("double", 20.into()),
    );
    let response = Response {
        spec_name: "test_spec".to_string(),
        effective: "2026-01-01".to_string(),
        spec_effective_from: None,
        spec_effective_to: None,
        results,
    };
    let json: serde_json::Value = serde_json::from_str(
        &serde_json::to_string(&crate::api::Response::from(&response)).unwrap(),
    )
    .unwrap();
    let number = &json["results"]["double"]["number"];
    assert!(!number.is_array());
    assert_eq!(number.as_str(), Some("20"));
}

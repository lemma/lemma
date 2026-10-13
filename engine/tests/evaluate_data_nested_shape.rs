//! Wrong-shape items nested in a list are request Errors naming the data key, never a panic.

use indexmap::IndexMap;
use lemma::{DateTimeValue, Engine, ErrorKind, EvaluateDataValue, SourceType};

const FARES_SPEC: &str = "spec fares\ndata money: measure\n  -> unit eur: 1.00\ndata fares: money list\nrule total: fares -> sum\n";

fn fares_engine() -> Engine {
    let mut engine = Engine::new();
    engine
        .load([(SourceType::Volatile, FARES_SPEC.to_string())])
        .unwrap_or_else(|errs| panic!("load must succeed: {errs:?}"));
    engine
}

fn evaluate_fares(items: Vec<EvaluateDataValue>) -> Result<lemma::Response, lemma::Error> {
    let now = DateTimeValue::now();
    let mut data = IndexMap::new();
    data.insert("fares".to_string(), EvaluateDataValue::List(items));
    fares_engine().evaluate(None, "fares", Some(&now), data, None, false)
}

fn unit_map(unit: &str, magnitude: &str) -> EvaluateDataValue {
    let mut map = IndexMap::new();
    map.insert(
        unit.to_string(),
        EvaluateDataValue::Literal(magnitude.to_string()),
    );
    EvaluateDataValue::Map(map)
}

fn assert_request_error_on_fares(result: Result<lemma::Response, lemma::Error>) {
    let error = match result {
        Err(error) => error,
        Ok(_) => panic!("wrong-shape nested item must be a request error"),
    };
    assert_eq!(error.kind(), ErrorKind::Request, "{}", error.message());
    assert_eq!(error.related_data(), Some("fares"));
}

#[test]
fn nested_boolean_item_is_request_error() {
    assert_request_error_on_fares(evaluate_fares(vec![
        EvaluateDataValue::Literal("1 eur".to_string()),
        EvaluateDataValue::Boolean(true),
    ]));
}

#[test]
fn nested_list_item_is_request_error() {
    assert_request_error_on_fares(evaluate_fares(vec![EvaluateDataValue::List(vec![
        EvaluateDataValue::Literal("1 eur".to_string()),
    ])]));
}

#[test]
fn nested_multi_unit_map_item_is_request_error() {
    let mut map = IndexMap::new();
    map.insert(
        "eur".to_string(),
        EvaluateDataValue::Literal("1".to_string()),
    );
    map.insert(
        "usd".to_string(),
        EvaluateDataValue::Literal("2".to_string()),
    );
    assert_request_error_on_fares(evaluate_fares(vec![EvaluateDataValue::Map(map)]));
}

const ORDER_SPEC: &str = "spec line\ndata money: measure\n  -> unit eur: 1.00\ndata prices: money list\ndata qty: number\n\nspec order\nuses lines: line list\n";

#[test]
fn nested_item_in_instance_list_row_is_request_error() {
    let mut engine = Engine::new();
    engine
        .load([(SourceType::Volatile, ORDER_SPEC.to_string())])
        .unwrap_or_else(|errs| panic!("load must succeed: {errs:?}"));
    let mut row = IndexMap::new();
    row.insert(
        "qty".to_string(),
        EvaluateDataValue::Literal("1".to_string()),
    );
    row.insert(
        "prices".to_string(),
        EvaluateDataValue::List(vec![EvaluateDataValue::Boolean(true)]),
    );
    let mut data = IndexMap::new();
    data.insert(
        "lines".to_string(),
        EvaluateDataValue::List(vec![EvaluateDataValue::Map(row)]),
    );
    let now = DateTimeValue::now();
    let error = match engine.evaluate(None, "order", Some(&now), data, None, false) {
        Err(error) => error,
        Ok(_) => panic!("nested wrong-shape row field must be a request error"),
    };
    assert_eq!(error.kind(), ErrorKind::Request, "{}", error.message());
    assert!(
        error.message().contains("boolean input is only valid"),
        "{}",
        error.message()
    );
}

#[test]
fn valid_nested_items_still_evaluate() {
    match evaluate_fares(vec![
        unit_map("eur", "1"),
        EvaluateDataValue::Literal("2 eur".to_string()),
    ]) {
        Ok(_) => {}
        Err(error) => panic!("valid fares must evaluate: {}", error.message()),
    }
}

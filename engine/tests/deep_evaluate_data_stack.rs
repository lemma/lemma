//! Deep host run data must come back as a request Error on a small stack, never an abort.
//!
//! Run data is a neutral tree built by host walkers. Nesting depth is unbounded by the host, so
//! building, comparing, debug-formatting, cloning and dropping it must not grow the native stack.

use indexmap::IndexMap;
use lemma::{DateTimeValue, Engine, EvaluateDataValue, SourceType};

const ONE_MIB: usize = 1024 * 1024;
const DEPTH: usize = 1_000_000;

fn pricing_engine() -> Engine {
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Volatile,
            "spec pricing\ndata amount: number\nrule doubled: amount * 2\n".to_string(),
        )])
        .unwrap_or_else(|errs| panic!("load must succeed: {errs:?}"));
    engine
}

fn deep_list(depth: usize) -> EvaluateDataValue {
    let mut value = EvaluateDataValue::Literal("1".to_string());
    for _ in 0..depth {
        value = EvaluateDataValue::List(vec![value]);
    }
    value
}

fn deep_map(depth: usize) -> EvaluateDataValue {
    let mut value = EvaluateDataValue::Literal("1".to_string());
    for _ in 0..depth {
        let mut level = IndexMap::new();
        level.insert("n".to_string(), value);
        value = EvaluateDataValue::Map(level);
    }
    value
}

fn evaluate_amount_on_small_stack(
    engine: Engine,
    build: fn(usize) -> EvaluateDataValue,
) -> lemma::Error {
    let handle = std::thread::Builder::new()
        .stack_size(ONE_MIB)
        .spawn(move || {
            let now = DateTimeValue::now();
            let mut data = IndexMap::new();
            data.insert("amount".to_string(), build(DEPTH));
            match engine.evaluate(None, "pricing", Some(&now), data, None, false) {
                Err(error) => error,
                Ok(_) => panic!("deep amount must be a request error"),
            }
        })
        .unwrap_or_else(|e| panic!("spawn with stack_size={ONE_MIB} failed: {e}"));
    handle
        .join()
        .expect("eval thread panicked or aborted (stack overflow)")
}

#[test]
fn deep_list_amount_is_request_error_on_small_stack() {
    let error = evaluate_amount_on_small_stack(pricing_engine(), deep_list);
    assert_eq!(error.related_data(), Some("amount"));
    assert!(
        error
            .message()
            .contains("array overlay is not valid for scalar data"),
        "{}",
        error.message()
    );
}

#[test]
fn deep_map_amount_is_request_error_on_small_stack() {
    let error = evaluate_amount_on_small_stack(pricing_engine(), deep_map);
    assert_eq!(error.related_data(), Some("amount"));
    assert!(
        error
            .message()
            .contains("object overlay is only valid for measure or ratio data"),
        "{}",
        error.message()
    );
}

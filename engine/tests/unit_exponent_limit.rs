//! Unit exponents stay within `MAX_UNIT_EXPONENT` (64): written exponents, compound units of
//! compound units, and measure products across rules are rejected at load, never overflowed.

use indexmap::IndexMap;
use lemma::DateTimeValue;
use lemma::Engine;
use std::path::PathBuf;
use std::sync::Arc;

fn source() -> lemma::SourceType {
    lemma::SourceType::Path(Arc::new(PathBuf::from("unit_exponent_limit.lemma")))
}

fn load_error(code: &str) -> String {
    let mut engine = Engine::new();
    let errors = engine
        .load([(source(), code.to_string())])
        .expect_err("load must reject the spec");
    errors
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

fn eval_rule(code: &str, rule_name: &str) -> String {
    let mut engine = Engine::new();
    engine
        .load([(source(), code.to_string())])
        .expect("spec must load");
    let now = DateTimeValue::now();
    let response = engine
        .evaluate(None, "physics", Some(&now), IndexMap::new(), None, false)
        .expect("evaluate must complete");
    let result = response.results.get(rule_name).expect("rule in response");
    assert!(!result.vetoed, "rule vetoed: {:?}", result.veto_reason);
    result.result().expect("result").to_string()
}

#[test]
fn written_exponent_above_limit_is_rejected() {
    let joined = load_error(
        r#"
spec physics
data length: measure
  -> unit meter: 1
data hyper: measure
  -> unit hyper_meter: meter^65
"#,
    );
    assert!(
        joined.contains("maximum unit exponent of 64"),
        "got: {joined}"
    );
}

#[test]
fn written_exponent_far_above_i32_product_range_is_rejected() {
    let joined = load_error(
        r#"
spec physics
data length: measure
  -> unit meter: 1
data hyper: measure
  -> unit hyper_meter: meter^1000000000
data hyper_cubed: measure
  -> unit hyper_meter_cubed: hyper_meter^3
data a: 2 hyper_meter_cubed
rule r: a * a
"#,
    );
    assert!(
        joined.contains("maximum unit exponent of 64"),
        "got: {joined}"
    );
}

#[test]
fn repeated_factor_summing_above_limit_is_rejected() {
    let joined = load_error(
        r#"
spec physics
data length: measure
  -> unit meter: 1
data hyper: measure
  -> unit hyper_meter: meter^40 * meter^40
"#,
    );
    assert!(
        joined.contains("maximum unit exponent of 64"),
        "got: {joined}"
    );
}

#[test]
fn compound_of_compound_above_limit_is_rejected() {
    let joined = load_error(
        r#"
spec physics
data length: measure
  -> unit meter: 1
data hyper: measure
  -> unit hyper_meter: meter^32
data hyper_cubed: measure
  -> unit hyper_meter_cubed: hyper_meter^3
"#,
    );
    assert!(
        joined.contains("hyper_meter_cubed") && joined.contains("maximum of 64"),
        "got: {joined}"
    );
}

#[test]
fn rule_products_doubling_exponent_past_limit_are_rejected() {
    let joined = load_error(
        r#"
spec physics
data length: measure
  -> unit meter: 1
data side: 2 meter
rule e2: side * side
rule e4: e2 * e2
rule e8: e4 * e4
rule e16: e8 * e8
rule e32: e16 * e16
rule e64: e32 * e32
rule e128: e64 * e64
"#,
    );
    assert!(
        joined.contains("resulting unit exponent exceeds the maximum of 64"),
        "got: {joined}"
    );
}

#[test]
fn exponent_at_limit_evaluates() {
    let code = r#"
spec physics
data length: measure
  -> unit meter: 1
data hyper: measure
  -> unit hyper_meter: meter^32
data hyper_squared: measure
  -> unit hyper_meter_squared: meter^64
data a: 2 hyper_meter
rule squared: a * a
"#;
    assert_eq!(eval_rule(code, "squared"), "4 hyper_meter_squared");
}

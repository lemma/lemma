//! Main calculation chain across two repositories and four specs.
//!
//! `invoice` (repo `commerce`) imports `packing` (repo `geometry`) and `rates`
//! (repo `commerce`) and wires rule results into their data slots. Every
//! arithmetic and mathematical operator sits on the live path to `total`.
//!
//! Rational backbone (exact): crates 3, net_per_unit 105, leftover 10,
//! crate_gap 1, half_unit 50, discount_gap 5, rounded_ratio 2, geometry_check 0,
//! so `3 * 105 + 10 - 1 + 50 - 5 + 2 + 0 = 371`.
//! Inverse trig terms are checked against closed forms of pi. `sin`, `cos`, `exp` and `ln`
//! are checked against the same `rust_decimal` functions the engine delegates to.

use indexmap::IndexMap;
use lemma::{DateTimeValue, Engine, SourceType, ValueKind};
use rust_decimal::{Decimal, MathematicalOps};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

const MAIN_CALC: &str = r#"
repo geometry

spec dimensions

data length: 3
data width: 4
data height: 5

rule face_diagonal: sqrt(length ^ 2 + width ^ 2)
rule volume: length * width * height
rule excess: volume - face_diagonal


spec packing

uses d: dimensions

data capacity: number

rule crates: ceil(d.volume / capacity)
rule leftover: d.volume mod capacity
rule raw_ratio: d.volume / capacity
rule floored_crates: floor(raw_ratio)
rule rounded_ratio: round(raw_ratio)
rule crate_gap: abs(crates - floored_crates)
rule geometry_check: d.face_diagonal + d.excess - d.volume


repo commerce

spec rates

data unit_price: number
data tax_rate: 10%
data discount: 5

rule taxed: unit_price * tax_rate
rule gross: unit_price + taxed
rule net_per_unit: gross - discount
rule half_unit: unit_price / 2
rule discount_gap: abs(discount - taxed)


spec invoice

uses pack: geometry packing
  -> with capacity: billed_capacity
uses r: rates
  -> with unit_price: pack_unit_price

rule billed_capacity: 25
rule pack_unit_price: 100

rule trig_blend:
  sin(1) * cos(2) + tan(0.5) + asin(0.5) + acos(0.5) + atan(1)

rule growth: exp(0.5) * log(10)

rule crate_count: pack.crates
rule net_unit_price: r.net_per_unit
rule leftover_units: pack.leftover

rule total:
  pack.crates * r.net_per_unit
    + pack.leftover
    - pack.crate_gap
    + r.half_unit
    - r.discount_gap
    + pack.rounded_ratio
    + pack.geometry_check
    + trig_blend
    + growth
"#;

fn d(literal: &str) -> Decimal {
    Decimal::from_str(literal).expect("BUG: test decimal literal must parse")
}

fn pi() -> Decimal {
    d("3.1415926535897932384626433833")
}

fn oracle_trig_blend() -> Decimal {
    let sin_1 = d("1").checked_sin().expect("BUG: sin(1)");
    let cos_2 = d("2").checked_cos().expect("BUG: cos(2)");
    let tan_half = d("0.5").checked_sin().expect("BUG: sin(0.5)")
        / d("0.5").checked_cos().expect("BUG: cos(0.5)");
    sin_1 * cos_2
        + tan_half
        + pi() / Decimal::from(6)
        + pi() / Decimal::from(3)
        + pi() / Decimal::from(4)
}

fn oracle_growth() -> Decimal {
    let exp_half = d("0.5").checked_exp().expect("BUG: exp(0.5)");
    let ln_ten = d("10").checked_ln().expect("BUG: ln(10)");
    exp_half * ln_ten
}

fn oracle_total() -> Decimal {
    let length = d("3");
    let width = d("4");
    let height = d("5");
    let face_diagonal = (length * length + width * width)
        .sqrt()
        .expect("BUG: sqrt(25) must exist");
    let volume = length * width * height;
    let excess = volume - face_diagonal;
    let geometry_check = face_diagonal + excess - volume;

    let capacity = d("25");
    let raw_ratio = volume / capacity;
    let crates = raw_ratio.ceil();
    let leftover = volume % capacity;
    let floored_crates = raw_ratio.floor();
    let rounded_ratio = raw_ratio.round();
    let crate_gap = (crates - floored_crates).abs();

    let unit_price = d("100");
    let taxed = unit_price * d("0.1");
    let gross = unit_price + taxed;
    let net_per_unit = gross - d("5");
    let half_unit = unit_price / d("2");
    let discount_gap = (d("5") - taxed).abs();

    crates * net_per_unit + leftover - crate_gap + half_unit - discount_gap
        + rounded_ratio
        + geometry_check
        + oracle_trig_blend()
        + oracle_growth()
}

fn tol(decimal_places: u32) -> Decimal {
    Decimal::new(1, decimal_places)
}

fn assert_close_decimal(actual: Decimal, expected: Decimal, tolerance: Decimal) {
    let diff = if actual > expected {
        actual - expected
    } else {
        expected - actual
    };
    assert!(
        diff <= tolerance,
        "expected ~{expected} (±{tolerance}), got {actual} (diff {diff})"
    );
}

fn load_main_calc() -> Engine {
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Path(Arc::new(PathBuf::from("main_calc_integration.lemma"))),
            MAIN_CALC.to_string(),
        )])
        .expect("main calc fixture must plan");
    engine
}

fn evaluate_invoice(engine: &Engine) -> lemma::Response {
    let now = DateTimeValue::now();
    engine
        .evaluate(
            Some("commerce"),
            "invoice",
            Some(&now),
            IndexMap::new(),
            None,
            false,
        )
        .expect("invoice must evaluate")
}

fn decimal(response: &lemma::Response, rule: &str) -> Decimal {
    let result = response.results.get(rule).unwrap_or_else(|| {
        panic!(
            "rule '{rule}' missing; have {:?}",
            response.results.keys().collect::<Vec<_>>()
        )
    });
    assert!(
        !result.vetoed,
        "rule '{rule}' vetoed: {:?}",
        result.veto_reason
    );
    let literal = result.to_literal();
    match &literal.value {
        ValueKind::Number(n) => ValueKind::Number(n.clone())
            .as_decimal_magnitude()
            .expect("BUG: number literal must have a decimal magnitude"),
        other => panic!("rule '{rule}' is not a number: {other:?}"),
    }
}

#[test]
fn main_calc_cross_repo_operator_chain() {
    let engine = load_main_calc();
    let response = evaluate_invoice(&engine);

    assert_eq!(decimal(&response, "crate_count"), Decimal::from(3));
    assert_eq!(decimal(&response, "net_unit_price"), Decimal::from(105));
    assert_eq!(decimal(&response, "leftover_units"), Decimal::from(10));

    assert_close_decimal(
        decimal(&response, "trig_blend"),
        oracle_trig_blend(),
        tol(24),
    );
    assert_close_decimal(decimal(&response, "growth"), oracle_growth(), tol(12));
    assert_close_decimal(decimal(&response, "total"), oracle_total(), tol(12));
}

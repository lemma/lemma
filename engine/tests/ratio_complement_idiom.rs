//! TDD tests for the ratio complement idiom: `price * (100% - discount)`,
//! `(100% + rate) ^ year`, and operand-order correctness for ratio/number
//! mixed arithmetic.

use lemma::{DateTimeValue, Engine};
use std::collections::HashMap;

fn eval(code: &str, spec: &str, rule: &str) -> String {
    eval_with_data(code, spec, rule, HashMap::new())
}

fn eval_with_data(code: &str, spec: &str, rule: &str, data: HashMap<String, String>) -> String {
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap_or_else(|errs| {
            panic!(
                "load failed: {}",
                errs.iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        });
    let now = DateTimeValue::now();
    let resp = engine
        .run(None, spec, Some(&now), data, None, false)
        .expect("eval failed");
    let result = resp
        .results
        .get(rule)
        .unwrap_or_else(|| panic!("rule '{}' not found", rule));
    assert!(
        !result.vetoed,
        "rule '{}' vetoed: {:?}",
        rule, result.veto_reason
    );
    result.result().expect("result").to_string()
}

fn expect_plan_error(code: &str, fragment: &str) {
    let mut engine = Engine::new();
    let result = engine.load([(lemma::SourceType::Volatile, code.to_string())]);
    assert!(result.is_err(), "expected planning error, got Ok");
    let msg = result
        .unwrap_err()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ");
    assert!(
        msg.contains(fragment),
        "error should contain '{}', got: {}",
        fragment,
        msg
    );
}

// ═══════════════════════════════════════════════════════════════════
// Complement idiom: ratio ± ratio, then used with number/measure
// ═══════════════════════════════════════════════════════════════════

#[test]
fn complement_discount_number() {
    let code = r#"spec t
data price: 100
data discount: 10%
rule result: price * (100% - discount)"#;
    assert_eq!(eval(code, "t", "result"), "90");
}

#[test]
fn complement_markup_number() {
    let code = r#"spec t
data base: 200
data markup: 25%
rule result: base * (100% + markup)"#;
    assert_eq!(eval(code, "t", "result"), "250");
}

#[test]
fn complement_discount_measure() {
    let code = r#"spec t
uses lemma units
data money: measure -> unit eur: 1.00
data price: 100 eur
data discount: 10%
rule result: price * (100% - discount)"#;
    let val = eval(code, "t", "result");
    assert!(val.contains("90"), "expected 90 eur, got: {}", val);
}

#[test]
fn complement_divide_margin_pricing() {
    let code = r#"spec t
data cost: 65
data margin: 35%
rule sell_price: cost / (100% - margin)"#;
    assert_eq!(eval(code, "t", "sell_price"), "100");
}

#[test]
fn complement_compound_interest() {
    let code = r#"spec t
data principal: number
data annual_rate: ratio
data year: number
rule growth: (100% + annual_rate) ^ year
rule future_value: principal * growth"#;
    let data = HashMap::from([
        ("principal".into(), "1000".into()),
        ("annual_rate".into(), "10%".into()),
        ("year".into(), "3".into()),
    ]);
    assert_eq!(eval_with_data(code, "t", "future_value", data), "1331");
}

// ═══════════════════════════════════════════════════════════════════
// Operand-order correctness: number ^ ratio, ratio ^ number
// ═══════════════════════════════════════════════════════════════════

#[test]
fn number_power_ratio_sqrt() {
    let code = r#"spec t
data x: 4
rule result: x ^ 50%"#;
    assert_eq!(eval(code, "t", "result"), "2");
}

#[test]
fn number_power_ratio_decimal_fallback() {
    use rust_decimal::Decimal;
    let code = r#"spec t
data x: 2
rule result: x ^ 50%"#;
    let val = eval(code, "t", "result");
    let actual: Decimal = val
        .parse()
        .unwrap_or_else(|_| panic!("2^50% display must be decimal, got {val}"));
    let expected = Decimal::new(1414213562373095, 15);
    let tol = Decimal::new(1, 12);
    let diff = if actual > expected {
        actual - expected
    } else {
        expected - actual
    };
    assert!(
        diff <= tol,
        "2^50% ≈ sqrt(2) as Decimal (±{tol}), got {actual} (diff {diff})"
    );
}

#[test]
fn ratio_power_number() {
    let code = r#"spec t
data r: 50%
rule result: r ^ 2"#;
    assert_eq!(eval(code, "t", "result"), "25%");
}

#[test]
fn ratio_power_number_growth() {
    let code = r#"spec t
data r: 110%
rule result: r ^ 3"#;
    assert_eq!(eval(code, "t", "result"), "133.1%");
}

// ═══════════════════════════════════════════════════════════════════
// Operand-order correctness: divide
// ═══════════════════════════════════════════════════════════════════

#[test]
fn ratio_divide_number() {
    let code = r#"spec t
data r: 50%
rule result: r / 2"#;
    assert_eq!(eval(code, "t", "result"), "25%");
}

#[test]
fn number_divide_ratio() {
    let code = r#"spec t
data n: 10
data r: 50%
rule result: n / r"#;
    assert_eq!(eval(code, "t", "result"), "20");
}

// ═══════════════════════════════════════════════════════════════════
// Operand-order correctness: modulo
// ═══════════════════════════════════════════════════════════════════

#[test]
fn number_modulo_ratio() {
    let code = r#"spec t
data n: 10
data r: 30%
rule result: n % r"#;
    assert_eq!(eval(code, "t", "result"), "0.1");
}

#[test]
fn ratio_modulo_number() {
    let code = r#"spec t
data r: 30%
data n: 10
rule result: r % n"#;
    assert_eq!(eval(code, "t", "result"), "30%");
}

// ═══════════════════════════════════════════════════════════════════
// Planning rejections: ratio left with measure for divide/modulo
// ═══════════════════════════════════════════════════════════════════

#[test]
fn ratio_divide_measure_rejected() {
    let code = r#"spec t
uses lemma units
data money: measure -> unit eur: 1.00
data price: 100 eur
data r: 50%
rule bad: r / price"#;
    expect_plan_error(code, "Cannot apply");
}

#[test]
fn ratio_modulo_measure_rejected() {
    let code = r#"spec t
uses lemma units
data money: measure -> unit eur: 1.00
data price: 100 eur
data r: 50%
rule bad: r % price"#;
    expect_plan_error(code, "Cannot apply");
}

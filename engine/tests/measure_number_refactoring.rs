//! Integration tests for Measure/Number arithmetic behavior.
//!
//! Unit-resolution and PerSliceTypeResolver behavior tests live in `src/planning/types.rs`.

use lemma::DateTimeValue;
use lemma::{Engine, Response};
use std::collections::HashMap;

fn rule_value_str(response: &Response, name: &str) -> String {
    let r = response
        .results
        .get(name)
        .unwrap_or_else(|| panic!("rule '{name}' missing from results"));
    assert!(
        !r.vetoed,
        "rule '{name}' must not veto, got {:?}",
        r.veto_reason
    );
    r.result().expect("result").to_string()
}

#[test]
fn test_measure_op_measure_same_type_allowed() {
    // Measure +/- Measure (same type) -> Measure; Measure / Measure (same type) -> Number.
    // Measure * Measure is rejected at plan time (use `(a as number) * (b as number)` instead).
    let code = r#"spec test
data money: measure
  -> unit eur: 1.00
  -> unit usd: 0.84

data price1: money
data price2: money

rule total: price1 + price2
rule difference: price1 - price2
rule quotient: price1 / price2"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Should parse");

    let mut data = HashMap::new();
    data.insert("price1".to_string(), "10 eur".into());
    data.insert("price2".to_string(), "5 eur".into());

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "test", Some(&now), data, None, true)
        .expect("Should evaluate");

    for name in ["total", "difference"] {
        let r = response.results.get(name).expect(name);
        assert!(!r.vetoed, "{name} must not veto for valid measure inputs");
        let v = r
            .explanation
            .as_ref()
            .expect("explanation")
            .result
            .literal_value()
            .unwrap_or_else(|| panic!("{name} must produce a value"));
        assert!(
            matches!(v.value, lemma::ValueKind::Measure(_)),
            "{name} result must stay in measure money type"
        );
        assert!(
            r.result
                .as_ref()
                .and_then(|val| val.measure.as_ref())
                .is_some(),
            "{name} must expose measure map"
        );
    }
    {
        let name = "quotient";
        let r = response.results.get(name).expect(name);
        assert!(!r.vetoed, "{name} must not veto for valid measure inputs");
        let v = r
            .explanation
            .as_ref()
            .expect("explanation")
            .result
            .literal_value()
            .unwrap_or_else(|| panic!("{name} must produce a value"));
        assert!(
            matches!(v.value, lemma::ValueKind::Number(_)),
            "{name} result must be dimensionless number"
        );
    }
    let total_s = response
        .results
        .get("total")
        .unwrap()
        .result()
        .expect("result")
        .to_string();
    assert!(
        total_s.contains("15") && total_s.to_lowercase().contains("eur"),
        "10 eur + 5 eur => ~15 eur, got {total_s}"
    );
    let diff_s = response
        .results
        .get("difference")
        .unwrap()
        .result()
        .expect("result")
        .to_string();
    assert!(
        diff_s.contains("5") && diff_s.to_lowercase().contains("eur"),
        "10 eur - 5 eur => ~5 eur, got {diff_s}"
    );
    let quot_s = response
        .results
        .get("quotient")
        .unwrap()
        .result()
        .expect("result")
        .to_string();
    assert!(
        quot_s.contains("2"),
        "10 eur / 5 eur => ratio 2 in display, got {quot_s}"
    );
}

#[test]
fn test_measure_op_number_allowed() {
    // Test that Measure op Number is allowed
    let code = r#"spec test
data money: measure
  -> unit eur: 1.00

data price: money
data multiplier: number

rule scaled: price * multiplier
rule divided: price / multiplier"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Should parse");

    let mut data = HashMap::new();
    data.insert("price".to_string(), "10 eur".into());
    data.insert("multiplier".to_string(), "2".into());

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "test", Some(&now), data, None, true)
        .expect("Should evaluate");

    let scaled = rule_value_str(&response, "scaled");
    assert!(
        scaled.contains("20") && scaled.to_lowercase().contains("eur"),
        "10 eur * 2 => ~20 eur, got {scaled}"
    );
    let divided = rule_value_str(&response, "divided");
    assert!(
        divided.contains("5") && divided.to_lowercase().contains("eur"),
        "10 eur / 2 => ~5 eur, got {divided}"
    );
}

#[test]
fn test_number_op_measure_allowed() {
    // Test that Number op Measure is allowed
    let code = r#"spec test
data money: measure
  -> unit eur: 1.00

data multiplier: number
data price: money

rule scaled: multiplier * price
rule divided: multiplier / price"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Should parse");

    let mut data = HashMap::new();
    data.insert("multiplier".to_string(), "2".into());
    data.insert("price".to_string(), "10 eur".into());

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "test", Some(&now), data, None, true)
        .expect("Should evaluate");

    let scaled = rule_value_str(&response, "scaled");
    assert!(
        scaled.contains("20") && scaled.to_lowercase().contains("eur"),
        "2 * 10 eur => ~20 eur, got {scaled}"
    );
    let divided = rule_value_str(&response, "divided");
    assert!(
        divided.contains("0.2") || divided.contains("0,2"),
        "2 / 10 eur => dimensionless ~0.2, got {divided}"
    );
}

#[test]
fn test_ratio_op_number_allowed() {
    // Test that Ratio op Number is allowed (result is Number)
    let code = r#"spec test
data ratio_value: ratio
data multiplier: number

rule result: ratio_value * multiplier"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Should parse");

    let mut data = HashMap::new();
    data.insert("ratio_value".to_string(), "50%".into());
    data.insert("multiplier".to_string(), "2".into());

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "test", Some(&now), data, None, true)
        .expect("Should evaluate");

    let s = rule_value_str(&response, "result");
    assert!(s.contains('1'), "0.5 * 2 => 1, got {s}");
}

#[test]
fn test_ratio_op_measure_allowed() {
    // Test that Ratio op Measure is allowed (result is Measure)
    let code = r#"spec test
data money: measure
  -> unit eur: 1.00

data ratio_value: ratio
data price: money

rule result: ratio_value * price"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Should parse");

    let mut data = HashMap::new();
    data.insert("ratio_value".to_string(), "50%".into());
    data.insert("price".to_string(), "10 eur".into());

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "test", Some(&now), data, None, true)
        .expect("Should evaluate");

    let s = rule_value_str(&response, "result");
    assert!(
        s.contains('5') && s.to_lowercase().contains("eur"),
        "0.5 * 10 eur => ~5 eur, got {s}"
    );
}

#[test]
fn test_measure_op_ratio_allowed() {
    // Test that Measure op Ratio is allowed (result is Measure)
    let code = r#"spec test
data money: measure
  -> unit eur: 1.00

data price: money
data ratio_value: ratio

rule result: price * ratio_value"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Should parse");

    let mut data = HashMap::new();
    data.insert("price".to_string(), "10 eur".into());
    data.insert("ratio_value".to_string(), "50%".into());

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "test", Some(&now), data, None, true)
        .expect("Should evaluate");

    let s = rule_value_str(&response, "result");
    assert!(
        s.contains('5') && s.to_lowercase().contains("eur"),
        "10 eur * 0.5 => ~5 eur, got {s}"
    );
}

#[test]
fn test_measure_comparison_same_type_allowed() {
    // Test that comparing same Measure types is allowed
    let code = r#"spec test
data money: measure
  -> unit eur: 1.00

data price1: money
data price2: money

rule is_greater: price1 > price2
rule is_equal: price1 is price2"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Should parse");

    let mut data = HashMap::new();
    data.insert("price1".to_string(), "10 eur".into());
    data.insert("price2".to_string(), "5 eur".into());

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "test", Some(&now), data, None, true)
        .expect("Should evaluate");

    assert_eq!(rule_value_str(&response, "is_greater"), "true");
    assert_eq!(rule_value_str(&response, "is_equal"), "false");
}

#[test]
fn test_all_arithmetic_operators_measure_same_type() {
    // Measure * Measure is rejected at plan time.
    // All other Measure op Measure/Number combinations compile and run correctly.
    // Note: Measure ^ Number requires an integer literal exponent (Step 8 rule).
    let code = r#"spec test
data money: measure
  -> unit eur: 1.00

data a: money
data b: money
data divisor: number

rule add: a + b
rule subtract: a - b
rule divide: a / b
rule modulo: a % divisor
rule power: a ^ 2"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Should parse");

    let mut data = HashMap::new();
    data.insert("a".to_string(), "10 eur".into());
    data.insert("b".to_string(), "3 eur".into());
    data.insert("divisor".to_string(), "3".into());

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "test", Some(&now), data, None, true)
        .expect("Should evaluate");

    let add = rule_value_str(&response, "add");
    assert!(
        add.contains("13") && add.to_lowercase().contains("eur"),
        "add: {add}"
    );
    let sub = rule_value_str(&response, "subtract");
    assert!(
        sub.contains('7') && sub.to_lowercase().contains("eur"),
        "subtract: {sub}"
    );
    let div = rule_value_str(&response, "divide");
    assert!(div.contains('3'), "divide: {div}");
    assert!(
        !div.to_lowercase().contains("eur"),
        "measure / measure should be dimensionless: {div}"
    );
    let modulo = rule_value_str(&response, "modulo");
    assert!(
        modulo.contains('1') && modulo.to_lowercase().contains("eur"),
        "modulo: {modulo}"
    );
    let pow = rule_value_str(&response, "power");
    assert!(pow.contains("100"), "power 10^2: {pow}");

    // Measure * Measure must be rejected at plan time
    let mut engine2 = Engine::new();
    let errs = engine2
        .load([(
            lemma::SourceType::Volatile,
            r#"spec test2
data money: measure -> unit eur: 1.00
data a: money
data b: money
rule multiply: a * b"#
                .to_string(),
        )])
        .expect_err("Measure * Measure must fail at plan time");
    let joined = errs
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("; ");
    assert!(
        joined.contains("anonymous intermediate"),
        "Measure * Measure must reject as anonymous intermediate, got: {joined}"
    );
}

#[test]
fn test_number_operations_all_operators() {
    // Test all arithmetic operators with Number types
    let code = r#"spec test
data a: number
data b: number

rule add: a + b
rule subtract: a - b
rule multiply: a * b
rule divide: a / b
rule modulo: a % b
rule power: a ^ b"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Should parse");

    let mut data = HashMap::new();
    data.insert("a".to_string(), "10".into());
    data.insert("b".to_string(), "3".into());

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "test", Some(&now), data, None, true)
        .expect("Should evaluate");

    assert_eq!(rule_value_str(&response, "add"), "13");
    assert_eq!(rule_value_str(&response, "subtract"), "7");
    assert_eq!(rule_value_str(&response, "multiply"), "30");
    let div = rule_value_str(&response, "divide");
    assert!(
        div.starts_with("3.333") || div == "3.3333333333333333333333333333",
        "divide 10/3: {div}"
    );
    assert_eq!(rule_value_str(&response, "modulo"), "1");
    assert_eq!(rule_value_str(&response, "power"), "1000");
}

#[test]
fn test_complex_mixed_operations() {
    // Test complex expressions with mixed types
    let code = r#"spec test
data money: measure
  -> unit eur: 1.00

data base_price: money
data discount_ratio: ratio
data tax_multiplier: number
data quantity: number

rule discounted: base_price * discount_ratio
rule with_tax: discounted * tax_multiplier
rule total: with_tax * quantity"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Should parse");

    let mut data = HashMap::new();
    data.insert("base_price".to_string(), "100 eur".into());
    data.insert("discount_ratio".to_string(), "90%".into());
    data.insert("tax_multiplier".to_string(), "1.2".into());
    data.insert("quantity".to_string(), "5".into());

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "test", Some(&now), data, None, true)
        .expect("Should evaluate");

    let disc = rule_value_str(&response, "discounted");
    assert!(
        disc.contains("90") && disc.to_lowercase().contains("eur"),
        "100 eur * 0.9 => ~90 eur: {disc}"
    );
    let tax = rule_value_str(&response, "with_tax");
    assert!(
        tax.contains("108") && tax.to_lowercase().contains("eur"),
        "90 eur * 1.2 => ~108 eur: {tax}"
    );
    let tot = rule_value_str(&response, "total");
    assert!(
        tot.contains("540") && tot.to_lowercase().contains("eur"),
        "108 eur * 5 => ~540 eur: {tot}"
    );
}

#[test]
fn test_primitive_measure_and_number_types() {
    // Measure types must declare at least one unit; measure values are unitful.
    // This test uses a proper measure type (money) and unitful data value.
    let code = r#"spec test
data money: measure
  -> unit eur: 1.00
  -> minimum 0 eur

data measure_value: money
data number_value: number

rule result: measure_value * number_value"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Should parse");

    let mut data = HashMap::new();
    data.insert("measure_value".to_string(), "10 eur".into());
    data.insert("number_value".to_string(), "2".into());

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "test", Some(&now), data, None, true)
        .expect("Should evaluate");

    let s = rule_value_str(&response, "result");
    assert!(
        s.contains("20") && s.to_lowercase().contains("eur"),
        "10 eur * 2 => ~20 eur: {s}"
    );
}

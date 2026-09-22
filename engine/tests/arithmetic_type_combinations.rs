//! Comprehensive tests for arithmetic type combinations.
//!
//! Tests every allowed and disallowed combination of types across all
//! arithmetic operators (+, -, *, /, %, ^), verifying both that valid
//! combinations produce the correct result type and that invalid
//! combinations are rejected during validation.

use lemma::DateTimeValue;
use lemma::Engine;
use rust_decimal::Decimal;
use std::collections::{BTreeMap, HashMap};

fn eval_result(
    code: &str,
    spec_name: &str,
    rule_name: &str,
    data: HashMap<String, String>,
) -> lemma::RuleResult {
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Should parse and plan");
    let now = DateTimeValue::now();
    let response = engine
        .run(None, spec_name, Some(&now), data, None, false)
        .expect("Should evaluate");
    let result = response
        .results
        .get(rule_name)
        .unwrap_or_else(|| panic!("Rule '{}' should exist", rule_name));
    if result.vetoed {
        panic!(
            "Rule '{}' should have a value, got veto: {:?}",
            rule_name, result.veto_reason
        );
    }
    result.clone()
}

fn eval_rule(
    code: &str,
    spec_name: &str,
    rule_name: &str,
    data: HashMap<String, String>,
) -> String {
    eval_result(code, spec_name, rule_name, data)
        .result()
        .expect("result")
        .to_string()
}

fn eval_measure_map(code: &str, spec_name: &str, rule_name: &str) -> BTreeMap<String, Decimal> {
    eval_result(code, spec_name, rule_name, HashMap::new())
        .result
        .expect("rule result value")
        .measure
        .expect("measure map")
}

fn expect_plan_error(code: &str, expected_fragment: &str) {
    let mut engine = Engine::new();
    let result = engine.load([(lemma::SourceType::Volatile, code.to_string())]);
    assert!(result.is_err(), "Expected planning error");
    let combined = result
        .unwrap_err()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ");
    assert!(
        combined.contains(expected_fragment),
        "Expected error containing '{}', got: {}",
        expected_fragment,
        combined
    );
}

// ═══════════════════════════════════════════════════════════════════
// Number with Number
// ═══════════════════════════════════════════════════════════════════

#[test]
fn number_add_number() {
    let code = r#"spec t
uses lemma units
data a: 10
data b: 3
rule result: a + b"#;
    assert_eq!(eval_rule(code, "t", "result", HashMap::new()), "13");
}

#[test]
fn number_subtract_number() {
    let code = r#"spec t
uses lemma units
data a: 10
data b: 3
rule result: a - b"#;
    assert_eq!(eval_rule(code, "t", "result", HashMap::new()), "7");
}

#[test]
fn number_multiply_number() {
    let code = r#"spec t
uses lemma units
data a: 10
data b: 3
rule result: a * b"#;
    assert_eq!(eval_rule(code, "t", "result", HashMap::new()), "30");
}

#[test]
fn number_divide_number() {
    let code = r#"spec t
uses lemma units
data a: 12
data b: 4
rule result: a / b"#;
    assert_eq!(eval_rule(code, "t", "result", HashMap::new()), "3");
}

#[test]
fn number_modulo_number() {
    let code = r#"spec t
uses lemma units
data a: 10
data b: 3
rule result: a % b"#;
    assert_eq!(eval_rule(code, "t", "result", HashMap::new()), "1");
}

#[test]
fn number_power_number() {
    let code = r#"spec t
uses lemma units
data a: 2
data b: 3
rule result: a ^ b"#;
    assert_eq!(eval_rule(code, "t", "result", HashMap::new()), "8");
}

// ═══════════════════════════════════════════════════════════════════
// Measure with Number: add/subtract require explicit conversion (as unit)
// ═══════════════════════════════════════════════════════════════════

#[test]
fn measure_add_number_rejected() {
    let code = r#"spec t
uses lemma units
data money: measure -> unit eur: 1.00
data price: 10 eur
data n: 5
rule result: price + n"#;
    expect_plan_error(code, "Cannot apply '+'");
}

#[test]
fn measure_subtract_number_rejected() {
    let code = r#"spec t
uses lemma units
data money: measure -> unit eur: 1.00
data price: 10 eur
data n: 3
rule result: price - n"#;
    expect_plan_error(code, "Cannot apply '-'");
}

#[test]
fn measure_multiply_number() {
    let code = r#"spec t
uses lemma units
data money: measure -> unit eur: 1.00
data price: 10 eur
data n: 3
rule result: price * n"#;
    let val = eval_rule(code, "t", "result", HashMap::new());
    assert!(val.contains("30"), "Expected 30 eur, got: {}", val);
}

#[test]
fn number_multiply_measure() {
    let code = r#"spec t
uses lemma units
data money: measure -> unit eur: 1.00
data n: 3
data price: 10 eur
rule result: n * price"#;
    let val = eval_rule(code, "t", "result", HashMap::new());
    assert!(val.contains("30"), "Expected 30 eur, got: {}", val);
}

#[test]
fn measure_divide_number() {
    let code = r#"spec t
uses lemma units
data money: measure -> unit eur: 1.00
data price: 12 eur
data n: 4
rule result: price / n"#;
    let val = eval_rule(code, "t", "result", HashMap::new());
    assert!(val.contains("3"), "Expected 3 eur, got: {}", val);
}

#[test]
fn measure_modulo_number() {
    let code = r#"spec t
uses lemma units
data money: measure -> unit eur: 1.00
data price: 10 eur
data n: 3
rule result: price % n"#;
    let val = eval_rule(code, "t", "result", HashMap::new());
    assert!(val.contains("1"), "Expected 1 eur, got: {}", val);
}

#[test]
fn measure_power_number() {
    // Exponent must be an integer literal for dimensional types; using a literal 3 directly.
    let code = r#"spec t
uses lemma units
data money: measure -> unit eur: 1.00
data price: 2 eur
rule result: price ^ 3"#;
    let val = eval_rule(code, "t", "result", HashMap::new());
    assert!(val.contains("8"), "Expected 8 eur, got: {}", val);
}

#[test]
fn measure_power_variable_exponent_rejected() {
    // Variable exponent for Measure ^ Number must be rejected at plan time.
    expect_plan_error(
        r#"spec t
data money: measure -> unit eur: 1.00
data price: 2 eur
data n: 3
rule result: price ^ n"#,
        "variable exponent",
    );
}

#[test]
fn measure_power_fractional_exponent_rejected() {
    // Fractional literal exponents for Measure ^ Number must also be rejected.
    expect_plan_error(
        r#"spec t
data money: measure -> unit eur: 1.00
data price: 4 eur
rule result: price ^ 0.5"#,
        "fractional",
    );
}

// ═══════════════════════════════════════════════════════════════════
// Measure ± Ratio → rejected (scale explicitly)
// ═══════════════════════════════════════════════════════════════════

#[test]
fn measure_add_ratio_rejected() {
    let code = r#"spec t
uses lemma units
data money: measure -> unit eur: 1.00
data price: 100 eur
data rate: 10%
rule result: price + rate"#;
    expect_plan_error(code, "scale explicitly");
}

#[test]
fn measure_subtract_ratio_rejected() {
    let code = r#"spec t
uses lemma units
data money: measure -> unit eur: 1.00
data price: 100 eur
data discount: 25%
rule result: price - discount"#;
    expect_plan_error(code, "scale explicitly");
}

#[test]
fn ratio_add_measure_rejected() {
    let code = r#"spec t
uses lemma units
data money: measure -> unit eur: 1.00
data rate: 10%
data price: 100 eur
rule result: rate + price"#;
    expect_plan_error(code, "scale explicitly");
}

#[test]
fn ratio_subtract_measure_rejected() {
    let code = r#"spec t
uses lemma units
data money: measure -> unit eur: 1.00
data discount: 25%
data price: 100 eur
rule result: discount - price"#;
    expect_plan_error(code, "scale explicitly");
}

#[test]
fn measure_multiply_ratio() {
    let code = r#"spec t
uses lemma units
data money: measure -> unit eur: 1.00
data price: 100 eur
data rate: 50%
rule result: price * rate"#;
    let val = eval_rule(code, "t", "result", HashMap::new());
    assert!(val.contains("50"), "Expected 50 eur, got: {}", val);
}

#[test]
fn measure_divide_ratio() {
    let code = r#"spec t
uses lemma units
data money: measure -> unit eur: 1.00
data price: 100 eur
data rate: 50%
rule result: price / rate"#;
    let val = eval_rule(code, "t", "result", HashMap::new());
    assert!(val.contains("200"), "Expected 200 eur, got: {}", val);
}

// ═══════════════════════════════════════════════════════════════════
// Measure with Duration → anonymous intermediate (canonical magnitudes).
// Phase 1 dimensional arithmetic: operands are converted to canonical magnitudes before
// the operation. eur has factor 1 (canonical), hour converts to 3600 second.
// 50 eur * 8 hour → 50 * 28800 second = 1 440 000 (anonymous {money:1, duration:1}).
// ═══════════════════════════════════════════════════════════════════

#[test]
fn measure_multiply_duration_rejected_at_rule_boundary() {
    // Duration * Measure and Measure * Duration produce anonymous intermediates with unresolved
    // dimensions. These are forbidden at rule boundaries; give the rule a named measure type with units.
    expect_plan_error(
        r#"spec t
uses lemma units
data money: measure -> unit eur: 1.00
data rate: 50 eur
data hour: 8 hour
rule result: rate * hour"#,
        "anonymous intermediate",
    );
}

#[test]
fn duration_multiply_measure_rejected_at_rule_boundary() {
    expect_plan_error(
        r#"spec t
uses lemma units
data money: measure -> unit eur: 1.00
data hour: 8 hour
data rate: 50 eur
rule result: hour * rate"#,
        "anonymous intermediate",
    );
}

#[test]
fn measure_divide_duration_rejected_at_rule_boundary() {
    expect_plan_error(
        r#"spec t
uses lemma units
data money: measure -> unit eur: 1.00
data total: 400 eur
data hour: 8 hour
rule result: total / hour"#,
        "anonymous intermediate",
    );
}

// ═══════════════════════════════════════════════════════════════════
// Duration with Number: add/subtract require explicit conversion (as unit)
// ═══════════════════════════════════════════════════════════════════

#[test]
fn duration_add_number_rejected() {
    let code = r#"spec t
uses lemma units
data d: 10 hour
data n: 5
rule result: d + n"#;
    expect_plan_error(code, "Cannot apply '+'");
}

#[test]
fn duration_subtract_number_rejected() {
    let code = r#"spec t
uses lemma units
data d: 10 hour
data n: 3
rule result: d - n"#;
    expect_plan_error(code, "Cannot apply '-'");
}

#[test]
fn duration_multiply_number() {
    let code = r#"spec t
uses lemma units
data d: 10 hour
data n: 3
rule result: d * n"#;
    let map = eval_measure_map(code, "t", "result");
    assert_eq!(map["hour"], Decimal::from(30), "10 hour * 3 = 30 hour");
}

#[test]
fn number_multiply_duration() {
    let code = r#"spec t
uses lemma units
data n: 3
data d: 10 hour
rule result: n * d"#;
    let map = eval_measure_map(code, "t", "result");
    assert_eq!(map["hour"], Decimal::from(30), "3 * 10 hour = 30 hour");
}

#[test]
fn duration_divide_number() {
    let code = r#"spec t
uses lemma units
data d: 12 hour
data n: 4
rule result: d / n"#;
    let map = eval_measure_map(code, "t", "result");
    assert_eq!(map["hour"], Decimal::from(3), "12 hour / 4 = 3 hour");
}

#[test]
fn duration_modulo_number() {
    let code = r#"spec t
uses lemma units
data d: 10 hour
data n: 3
rule result: d % n"#;
    let map = eval_measure_map(code, "t", "result");
    assert_eq!(map["hour"], Decimal::from(1), "10 hour % 3 = 1 hour");
}

#[test]
fn duration_power_number() {
    // Exponent must be an integer literal for dimensional types; using a literal 3 directly.
    let code = r#"spec t
uses lemma units
data d: 2 hour
rule result: d ^ 3"#;
    let val = eval_rule(code, "t", "result", HashMap::new());
    assert!(val.contains("8"), "Expected 8 hour, got: {}", val);
}

#[test]
fn duration_power_variable_exponent_rejected() {
    // Variable exponent for Duration ^ Number must be rejected at plan time.
    expect_plan_error(
        r#"spec t
uses lemma units
data d: 2 hour
data n: 3
rule result: d ^ n"#,
        "variable exponent",
    );
}

// ═══════════════════════════════════════════════════════════════════
// Duration ± Ratio → rejected (scale explicitly)
// ═══════════════════════════════════════════════════════════════════

#[test]
fn duration_add_ratio_rejected() {
    let code = r#"spec t
uses lemma units
data d: 10 hour
data r: 50%
rule result: d + r"#;
    expect_plan_error(code, "scale explicitly");
}

#[test]
fn duration_subtract_ratio_rejected() {
    let code = r#"spec t
uses lemma units
data d: 10 hour
data r: 25%
rule result: d - r"#;
    expect_plan_error(code, "scale explicitly");
}

#[test]
fn ratio_add_duration_rejected() {
    let code = r#"spec t
uses lemma units
data r: 50%
data d: 10 hour
rule result: r + d"#;
    expect_plan_error(code, "scale explicitly");
}

#[test]
fn ratio_subtract_duration_rejected() {
    let code = r#"spec t
uses lemma units
data r: 25%
data d: 10 hour
rule result: r - d"#;
    expect_plan_error(code, "scale explicitly");
}

#[test]
fn duration_multiply_ratio() {
    let code = r#"spec t
uses lemma units
data d: 10 hour
data r: 50%
rule result: d * r"#;
    let map = eval_measure_map(code, "t", "result");
    assert_eq!(map["hour"], Decimal::from(5), "10 hour * 50% = 5 hour");
}

#[test]
fn ratio_multiply_duration() {
    let code = r#"spec t
uses lemma units
data r: 50%
data d: 10 hour
rule result: r * d"#;
    let map = eval_measure_map(code, "t", "result");
    assert_eq!(map["hour"], Decimal::from(5), "50% * 10 hour = 5 hour");
}

#[test]
fn duration_divide_ratio() {
    let code = r#"spec t
uses lemma units
data d: 10 hour
data r: 50%
rule result: d / r"#;
    let map = eval_measure_map(code, "t", "result");
    assert_eq!(map["hour"], Decimal::from(20), "10 hour / 50% = 20 hour");
}

// ═══════════════════════════════════════════════════════════════════
// Calendar ± Ratio → rejected (scale explicitly)
// ═══════════════════════════════════════════════════════════════════

#[test]
fn calendar_add_ratio_rejected() {
    let code = r#"spec t
uses lemma units
data c: 12 month
data r: 50%
rule result: c + r"#;
    expect_plan_error(code, "scale explicitly");
}

#[test]
fn calendar_subtract_ratio_rejected() {
    let code = r#"spec t
uses lemma units
data c: 12 month
data r: 25%
rule result: c - r"#;
    expect_plan_error(code, "scale explicitly");
}

#[test]
fn ratio_add_calendar_rejected() {
    let code = r#"spec t
uses lemma units
data r: 50%
data c: 12 month
rule result: r + c"#;
    expect_plan_error(code, "scale explicitly");
}

#[test]
fn ratio_subtract_calendar_rejected() {
    let code = r#"spec t
uses lemma units
data r: 25%
data c: 12 month
rule result: r - c"#;
    expect_plan_error(code, "scale explicitly");
}

// ═══════════════════════════════════════════════════════════════════
// Ratio with Number → Number (multiply/divide only)
// ═══════════════════════════════════════════════════════════════════

#[test]
fn ratio_multiply_number() {
    let code = r#"spec t
uses lemma units
data r: 50%
data n: 200
rule result: r * n"#;
    assert_eq!(eval_rule(code, "t", "result", HashMap::new()), "100");
}

#[test]
fn ratio_add_number_rejected() {
    let code = r#"spec t
uses lemma units
data r: 10%
data n: 100
rule result: n + r"#;
    expect_plan_error(code, "scale explicitly");
}

#[test]
fn number_subtract_ratio_rejected() {
    let code = r#"spec t
uses lemma units
data n: 100
data r: 10%
rule result: n - r"#;
    expect_plan_error(code, "scale explicitly");
}

#[test]
fn ratio_subtract_number_rejected() {
    let code = r#"spec t
uses lemma units
data r: 10%
data n: 100
rule result: r - n"#;
    expect_plan_error(code, "scale explicitly");
}

// ═══════════════════════════════════════════════════════════════════
// Measure with Measure (same family) → Measure
// ═══════════════════════════════════════════════════════════════════

#[test]
fn measure_add_measure_same_family() {
    let code = r#"spec t
uses lemma units
data money: measure -> unit eur: 1.00
data a: 4 eur
data b: 5 eur
rule result: a + b"#;
    let val = eval_rule(code, "t", "result", HashMap::new());
    assert!(
        val.contains("9") && val.contains("eur"),
        "Expected 9 eur, got: {}",
        val
    );
}

#[test]
fn measure_subtract_measure_same_family() {
    let code = r#"spec t
uses lemma units
data money: measure -> unit eur: 1.00
data a: 10 eur
data b: 3 eur
rule result: a - b"#;
    let val = eval_rule(code, "t", "result", HashMap::new());
    assert!(
        val.contains("7") && val.contains("eur"),
        "Expected 7 eur, got: {}",
        val
    );
}

#[test]
fn measure_add_measure_result_used_in_comparison() {
    let code = r#"spec t
uses lemma units
data money: measure -> unit eur: 1.00
data a: 4 eur
data b: 5 eur
data threshold: 8 eur
rule total: a + b
rule over_threshold: total > threshold"#;
    assert_eq!(
        eval_rule(code, "t", "over_threshold", HashMap::new()),
        "true"
    );
}

#[test]
fn measure_add_measure_result_in_further_arithmetic() {
    let code = r#"spec t
uses lemma units
data money: measure -> unit eur: 1.00
data a: 10 eur
data b: 20 eur
data c: 5 eur
rule subtotal: a + b
rule total: subtotal + c"#;
    let val = eval_rule(code, "t", "total", HashMap::new());
    assert!(
        val.contains("35") && val.contains("eur"),
        "Expected 35 eur, got: {}",
        val
    );
}

// ═══════════════════════════════════════════════════════════════════
// Ratio with Ratio → Ratio
// ═══════════════════════════════════════════════════════════════════

#[test]
fn ratio_add_ratio() {
    let code = r#"spec t
uses lemma units
data a: 10%
data b: 5%
rule result: a + b"#;
    let val = eval_rule(code, "t", "result", HashMap::new());
    assert!(val.contains("15"), "Expected 15 percent, got: {}", val);
}

#[test]
fn ratio_subtract_ratio() {
    let code = r#"spec t
uses lemma units
data a: 25%
data b: 10%
rule result: a - b"#;
    let val = eval_rule(code, "t", "result", HashMap::new());
    assert!(val.contains("15"), "Expected 15 percent, got: {}", val);
}

#[test]
fn ratio_add_ratio_result_used_with_measure() {
    let code = r#"spec t
uses lemma units
data money: measure -> unit eur: 1.00
data base_rate: 10%
data surcharge: 5%
data price: 200 eur
rule combined_rate: base_rate + surcharge
rule discount: price * combined_rate"#;
    let val = eval_rule(code, "t", "discount", HashMap::new());
    assert!(
        val.contains("30"),
        "Expected 30 eur (200 * 15%), got: {}",
        val
    );
}

// ═══════════════════════════════════════════════════════════════════
// Date - Date rejection
// ═══════════════════════════════════════════════════════════════════

#[test]
fn date_subtract_date_result_used_in_comparison_with_duration() {
    let code = r#"spec t
uses lemma units
data start: 2024-01-01
data end: 2024-01-10
data limit: 5 day
rule elapsed: end - start
rule over_limit: elapsed > limit"#;
    expect_plan_error(code, "date range");
}

#[test]
fn date_subtract_date() {
    let code = r#"spec t
uses lemma units
data a: 2024-01-10
data b: 2024-01-01
rule result: (a - b) as day"#;
    expect_plan_error(code, "date range");
}

// ═══════════════════════════════════════════════════════════════════
// Duration with Duration
// ═══════════════════════════════════════════════════════════════════

#[test]
fn duration_add_duration() {
    let code = r#"spec t
uses lemma units
data a: 10 hour
data b: 5 hour
rule result: a + b"#;
    let map = eval_measure_map(code, "t", "result");
    assert_eq!(map["hour"], Decimal::from(15), "10 hour + 5 hour = 15 hour");
}

#[test]
fn duration_subtract_duration() {
    let code = r#"spec t
uses lemma units
data a: 10 hour
data b: 3 hour
rule result: a - b"#;
    let map = eval_measure_map(code, "t", "result");
    assert_eq!(map["hour"], Decimal::from(7), "10 hour - 3 hour = 7 hour");
}

// ═══════════════════════════════════════════════════════════════════
// Date/Time temporal arithmetic
// ═══════════════════════════════════════════════════════════════════

#[test]
fn date_add_duration() {
    let code = r#"spec t
uses lemma units
data d: 2024-01-01
data dur: 7 day
rule result: d + dur"#;
    let val = eval_rule(code, "t", "result", HashMap::new());
    assert!(
        val.contains("2024-01-08"),
        "Expected 2024-01-08, got: {}",
        val
    );
}

#[test]
fn date_subtract_duration() {
    let code = r#"spec t
uses lemma units
data d: 2024-01-08
data dur: 7 day
rule result: d - dur"#;
    let val = eval_rule(code, "t", "result", HashMap::new());
    assert!(
        val.contains("2024-01-01"),
        "Expected 2024-01-01, got: {}",
        val
    );
}

#[test]
fn duration_add_date() {
    let code = r#"spec t
uses lemma units
data dur: 7 day
data d: 2024-01-01
rule result: dur + d"#;
    let val = eval_rule(code, "t", "result", HashMap::new());
    assert!(
        val.contains("2024-01-08"),
        "Expected 2024-01-08, got: {}",
        val
    );
}

// ═══════════════════════════════════════════════════════════════════
// Measure family: parent + child (same family) → Measure
// ═══════════════════════════════════════════════════════════════════

#[test]
fn same_family_parent_plus_child() {
    let code = r#"spec t
uses lemma units
data money: measure -> unit eur: 1.00
data budget: money -> unit jpy: 160.00 -> minimum 0 eur
data price: 10 eur
data allowance: 5 eur
rule result: price + allowance"#;
    let val = eval_rule(code, "t", "result", HashMap::new());
    assert!(
        val.contains("15") && val.contains("eur"),
        "Expected 15 eur, got: {}",
        val
    );
}

#[test]
fn same_family_siblings() {
    let code = r#"spec t
uses lemma units
data money: measure -> unit eur: 1.00
data income: money -> minimum 0 eur
data expense: money -> minimum 0 eur
data salary: 3000 eur
data rent: 1200 eur
rule remaining: salary - rent"#;
    let val = eval_rule(code, "t", "remaining", HashMap::new());
    assert!(
        val.contains("1800") && val.contains("eur"),
        "Expected 1800 eur, got: {}",
        val
    );
}

#[test]
fn same_family_result_used_in_comparison() {
    let code = r#"spec t
uses lemma units
data money: measure -> unit eur: 1.00
data budget: money -> unit jpy: 160.00 -> minimum 0 eur
data price: 4 eur
data fee: 5 eur
data limit: 8 eur
rule total: price + fee
rule over_budget: total > limit"#;
    assert_eq!(eval_rule(code, "t", "over_budget", HashMap::new()), "true");
}

// ═══════════════════════════════════════════════════════════════════
// Measure / Measure → Number (dimensionless)
// ═══════════════════════════════════════════════════════════════════

#[test]
fn measure_divide_measure_returns_number() {
    let code = r#"spec t
uses lemma units
data money: measure -> unit eur: 1.00
data total: 10 eur
data unit_price: 5 eur
rule price_ratio: total / unit_price"#;
    let val = eval_rule(code, "t", "price_ratio", HashMap::new());
    assert_eq!(
        val, "2",
        "10 eur / 5 eur should be dimensionless 2, got: {val}"
    );
    assert!(
        !val.to_lowercase().contains("eur"),
        "10 eur / 5 eur should NOT contain unit, got: {}",
        val
    );
}

#[test]
fn measure_divide_measure_result_usable_as_number() {
    let code = r#"spec t
uses lemma units
data money: measure -> unit eur: 1.00
data revenue: 100 eur
data cost: 50 eur
rule margin_factor: revenue / cost
rule doubled: margin_factor * 10"#;
    let val = eval_rule(code, "t", "doubled", HashMap::new());
    assert_eq!(val, "20", "margin_factor=2, doubled=20, got: {val}");
    assert!(
        !val.to_lowercase().contains("eur"),
        "number * number should not have unit, got: {}",
        val
    );
}

// ═══════════════════════════════════════════════════════════════════
// Number / Measure → Number (dimensionless)
// ═══════════════════════════════════════════════════════════════════

#[test]
fn number_divide_measure_returns_number() {
    let code = r#"spec t
uses lemma units
data money: measure -> unit eur: 1.00
data count: 20
data price: 10 eur
rule units_per_eur: count / price"#;
    let val = eval_rule(code, "t", "units_per_eur", HashMap::new());
    assert_eq!(
        val, "2",
        "20 / 10 eur should be dimensionless 2, got: {val}"
    );
    assert!(
        !val.to_lowercase().contains("eur"),
        "number / measure should NOT contain unit, got: {}",
        val
    );
}

// ═══════════════════════════════════════════════════════════════════
// Measure * Measure → rejected at plan time
// Use `(a as number) * (b as number)` to multiply measure values.
// ═══════════════════════════════════════════════════════════════════

#[test]
fn measure_multiply_measure_rejected_at_plan_time() {
    expect_plan_error(
        r#"spec t
data money: measure -> unit eur: 1.00
data a: 10 eur
data b: 5 eur
rule product: a * b"#,
        "anonymous intermediate",
    );
}

#[test]
fn measure_multiply_measure_via_as_number_produces_number() {
    let code = r#"spec t
uses lemma units
data money: measure -> unit eur: 1.00
data a: 10 eur
data b: 5 eur
rule product: (a as eur as number) * (b as eur as number)"#;
    let val = eval_rule(code, "t", "product", HashMap::new());
    assert!(
        val.contains("50"),
        "(a as eur as number) * (b as eur as number) should be 50, got: {}",
        val
    );
    assert!(
        !val.to_lowercase().contains("eur"),
        "result should not have unit, got: {}",
        val
    );
}

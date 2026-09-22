//! Tests for Measure/Measure and Duration/Duration arithmetic result types,
//! Measure*Measure / Duration*Duration rejection, and `as number` conversion.

use lemma::DateTimeValue;
use lemma::Engine;
use std::collections::HashMap;

fn load_ok(code: &str) -> Engine {
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("Should parse and plan");
    engine
}

fn load_err(code: &str) -> String {
    let mut engine = Engine::new();
    let errs = engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect_err("Should fail to plan");
    errs.iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("; ")
}

fn eval(engine: &Engine, spec: &str, rule: &str, data: HashMap<String, String>) -> String {
    let now = DateTimeValue::now();
    let response = engine
        .run(None, spec, Some(&now), data, None, false)
        .expect("Should evaluate");
    let result = response
        .results
        .get(rule)
        .unwrap_or_else(|| panic!("Rule '{}' should exist", rule));
    if result.vetoed {
        panic!(
            "Rule '{}' vetoed: {}",
            rule,
            result.veto_reason.as_deref().unwrap_or("Vetoed")
        );
    }
    result.result().expect("result").to_string()
}

// ═══════════════════════════════════════════════════════════════════
// Measure / Measure → Number
// ═══════════════════════════════════════════════════════════════════

#[test]
fn measure_divide_measure_same_type_returns_number() {
    let engine = load_ok(
        r#"spec t
uses lemma units
data money: measure -> unit eur: 1.00
data a: 10 eur
data b: 5 eur
rule result: a / b"#,
    );
    let val = eval(&engine, "t", "result", HashMap::new());
    assert_eq!(val, "2", "10 eur / 5 eur = 2 (dimensionless), got: {}", val);
}

#[test]
fn measure_divide_measure_result_is_not_measure() {
    let engine = load_ok(
        r#"spec t
uses lemma units
data money: measure -> unit eur: 1.00
data a: 10 eur
data b: 5 eur
rule result: a / b"#,
    );
    let val = eval(&engine, "t", "result", HashMap::new());
    assert!(
        !val.to_lowercase().contains("eur"),
        "10 eur / 5 eur should be dimensionless, not contain unit, got: {}",
        val
    );
}

// ═══════════════════════════════════════════════════════════════════
// Measure * Measure → rejected by planner
// ═══════════════════════════════════════════════════════════════════

#[test]
fn measure_multiply_measure_same_family_rejected_by_planner() {
    let err = load_err(
        r#"spec t
uses lemma units
data money: measure -> unit eur: 1.00
data a: 10 eur
data b: 5 eur
rule result: a * b"#,
    );
    assert!(
        err.contains("anonymous intermediate"),
        "Measure * Measure should be rejected as anonymous intermediate, got: {err}"
    );
}

#[test]
fn measure_multiply_measure_via_as_number_allowed() {
    let engine = load_ok(
        r#"spec t
uses lemma units
data money: measure -> unit eur: 1.00
data a: 10 eur
data b: 5 eur
rule result: (a as eur as number) * (b as eur as number)"#,
    );
    let val = eval(&engine, "t", "result", HashMap::new());
    assert_eq!(val, "50", "10 * 5 = 50, got: {}", val);
}

// ═══════════════════════════════════════════════════════════════════
// Duration / Duration → Number
// ═══════════════════════════════════════════════════════════════════

#[test]
fn duration_divide_duration_returns_number() {
    let engine = load_ok(
        r#"spec t
uses lemma units
data a: 10 hour
data b: 5 hour
rule result: a / b"#,
    );
    let val = eval(&engine, "t", "result", HashMap::new());
    assert_eq!(val, "2", "10 hour / 5 hour = 2, got: {}", val);
}

#[test]
fn duration_divide_duration_cross_unit_returns_number() {
    let engine = load_ok(
        r#"spec t
uses lemma units
data a: 2 hour
data b: 30 minute
rule result: a / b"#,
    );
    let val = eval(&engine, "t", "result", HashMap::new());
    assert_eq!(val, "4", "2 hour / 30 minute = 4, got: {}", val);
}

#[test]
fn duration_divide_duration_result_is_not_duration() {
    let engine = load_ok(
        r#"spec t
uses lemma units
data a: 10 hour
data b: 5 hour
rule result: a / b"#,
    );
    let val = eval(&engine, "t", "result", HashMap::new());
    assert!(
        !val.to_lowercase().contains("hour"),
        "Duration / Duration should be dimensionless, got: {}",
        val
    );
}

// ═══════════════════════════════════════════════════════════════════
// Duration * Duration → rejected by planner
// ═══════════════════════════════════════════════════════════════════

#[test]
fn duration_multiply_duration_rejected_by_planner() {
    let err = load_err(
        r#"spec t
uses lemma units
data a: 5 hour
data b: 3 hour
rule result: a * b"#,
    );
    assert!(
        err.contains("anonymous intermediate") || err.contains("Cannot apply"),
        "Duration * Duration should be rejected at plan time, got: {err}"
    );
}

// ═══════════════════════════════════════════════════════════════════
// Number / Duration → Number (not Duration)
// ═══════════════════════════════════════════════════════════════════

#[test]
fn number_divide_duration_returns_number_not_duration() {
    let engine = load_ok(
        r#"spec t
uses lemma units
data n: 10
data d: 5 hour
rule result: n / d"#,
    );
    let val = eval(&engine, "t", "result", HashMap::new());
    assert_eq!(val, "2", "10 / 5 hour = 2, got: {}", val);
    assert!(
        !val.to_lowercase().contains("hour"),
        "number / duration should be dimensionless, got: {}",
        val
    );
}

#[test]
fn number_multiply_duration_returns_duration() {
    let engine = load_ok(
        r#"spec t
uses lemma units
data n: 3
data d: 5 hour
rule result: n * d"#,
    );
    let val = eval(&engine, "t", "result", HashMap::new());
    assert!(val.contains("15"), "3 * 5 hour = 15 hour, got: {}", val);
    assert!(
        val.to_lowercase().contains("hour"),
        "number * duration should be duration, got: {}",
        val
    );
}

// ═══════════════════════════════════════════════════════════════════
// Duration op Number → Duration (regression guard)
// ═══════════════════════════════════════════════════════════════════

#[test]
fn duration_divide_number_returns_duration() {
    let engine = load_ok(
        r#"spec t
uses lemma units
data d: 10 hour
data n: 2
rule result: d / n"#,
    );
    let val = eval(&engine, "t", "result", HashMap::new());
    assert!(val.contains("5"), "10 hour / 2 = 5 hour, got: {}", val);
    assert!(
        val.to_lowercase().contains("hour"),
        "duration / number should stay duration, got: {}",
        val
    );
}

#[test]
fn duration_modulo_number_returns_duration() {
    let engine = load_ok(
        r#"spec t
uses lemma units
data d: 7 hour
data n: 3
rule result: d % n"#,
    );
    let val = eval(&engine, "t", "result", HashMap::new());
    assert!(val.contains("1"), "7 hour % 3 = 1 hour, got: {}", val);
    assert!(
        val.to_lowercase().contains("hour"),
        "duration % number should stay duration, got: {}",
        val
    );
}

// ═══════════════════════════════════════════════════════════════════
// Measure ^ Ratio and Measure % Ratio → rejected (crash prevention)
// ═══════════════════════════════════════════════════════════════════

#[test]
fn measure_power_ratio_rejected_by_planner() {
    let err = load_err(
        r#"spec t
uses lemma units
data money: measure -> unit eur: 1.00
data a: 100 eur
data r: 50%
rule result: a ^ r"#,
    );
    assert!(
        err.contains("fractional or variable exponent")
            || err.contains("Cannot apply")
            || err.contains("ratio"),
        "Measure ^ Ratio should be rejected at plan time, got: {err}"
    );
}

#[test]
fn measure_modulo_ratio_rejected_by_planner() {
    let err = load_err(
        r#"spec t
uses lemma units
data money: measure -> unit eur: 1.00
data a: 100 eur
data r: 25%
rule result: a % r"#,
    );
    assert!(
        err.contains("modulo") && err.to_lowercase().contains("ratio"),
        "Measure % Ratio should be rejected at plan time, got: {err}"
    );
}

// ═══════════════════════════════════════════════════════════════════
// `as number` conversion
// ═══════════════════════════════════════════════════════════════════

#[test]
fn measure_as_number_strips_unit() {
    let engine = load_ok(
        r#"spec t
uses lemma units
data money: measure -> unit eur: 1.00
data a: 10 eur
rule result: a as eur as number"#,
    );
    let val = eval(&engine, "t", "result", HashMap::new());
    assert_eq!(val, "10", "10 eur as number = 10, got: {}", val);
}

#[test]
fn measure_as_number_result_is_usable_as_number() {
    let engine = load_ok(
        r#"spec t
uses lemma units
data money: measure -> unit eur: 1.00
data a: 10 eur
data b: 5 eur
rule result: (a as eur as number) * (b as eur as number)"#,
    );
    let val = eval(&engine, "t", "result", HashMap::new());
    assert_eq!(val, "50", "10 * 5 = 50, got: {}", val);
}

#[test]
fn duration_as_number_strips_unit() {
    let engine = load_ok(
        r#"spec t
uses lemma units
data d: 5 hour
rule result: d as hour as number"#,
    );
    let val = eval(&engine, "t", "result", HashMap::new());
    assert_eq!(val, "5", "5 hour as number = 5, got: {}", val);
}

#[test]
fn ratio_as_number_strips_unit() {
    let engine = load_ok(
        r#"spec t
uses lemma units
data r: 25%
rule result: r as number"#,
    );
    let val = eval(&engine, "t", "result", HashMap::new());
    assert_eq!(val, "0.25", "25% as number = 0.25, got: {}", val);
}

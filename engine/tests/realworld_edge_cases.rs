//! Additional edge-case scenarios targeting potential bugs

use lemma::{DateTimeValue, Engine, SourceType};
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

fn src(name: &str) -> SourceType {
    SourceType::Path(Arc::new(PathBuf::from(name)))
}

fn run_spec(engine: &Engine, spec: &str, data: &[(&str, &str)]) -> lemma::Response {
    let now = DateTimeValue::now();
    let data_map: HashMap<String, String> = data
        .iter()
        .map(|(k, v)| (k.to_string(), (*v).to_string()))
        .collect();
    engine
        .run(None, spec, Some(&now), data_map, None, false)
        .unwrap()
}

fn rule_display(response: &lemma::Response, rule_name: &str) -> String {
    response
        .results
        .values()
        .find(|r| r.rule.name == rule_name)
        .unwrap_or_else(|| panic!("rule '{}' not found", rule_name))
        .result()
        .unwrap_or_else(|| panic!("rule '{}' has no display", rule_name))
        .to_string()
}

fn rule_vetoed(response: &lemma::Response, rule_name: &str) -> bool {
    response
        .results
        .values()
        .find(|r| r.rule.name == rule_name)
        .unwrap_or_else(|| panic!("rule '{}' not found", rule_name))
        .vetoed
}

// ===========================================================================
// EDGE CASE 1: Very large exponentiation
// ===========================================================================

#[test]
fn edge_large_exponentiation() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("big.lemma"),
            r#"
spec big_math

data base: number
data exponent: number

rule result: base ^ exponent
"#
            .to_string(),
        )])
        .unwrap();

    // 2^10 = 1024
    let resp = run_spec(&engine, "big_math", &[("base", "2"), ("exponent", "10")]);
    let display = rule_display(&resp, "result");
    assert_eq!(display, "1024", "2^10 = 1024");

    // 10^0 = 1
    let resp = run_spec(&engine, "big_math", &[("base", "10"), ("exponent", "0")]);
    let display = rule_display(&resp, "result");
    assert_eq!(display, "1", "10^0 = 1");
}

// ===========================================================================
// EDGE CASE 2: Negative number arithmetic
// ===========================================================================

#[test]
fn edge_negative_arithmetic() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("neg.lemma"),
            r#"
spec negatives

data a: number
data b: number

rule sum: a + b
rule diff: a - b
rule product: a * b
rule negated: 0 - a
"#
            .to_string(),
        )])
        .unwrap();

    let resp = run_spec(&engine, "negatives", &[("a", "-5"), ("b", "3")]);
    let sum = rule_display(&resp, "sum");
    assert_eq!(sum, "-2", "-5 + 3 = -2");

    let diff = rule_display(&resp, "diff");
    assert_eq!(diff, "-8", "-5 - 3 = -8");

    let product = rule_display(&resp, "product");
    assert_eq!(product, "-15", "-5 * 3 = -15");

    let negated = rule_display(&resp, "negated");
    assert_eq!(negated, "5", "0 - (-5) = 5");
}

// ===========================================================================
// EDGE CASE 3: Deeply nested cross-spec references
// ===========================================================================

#[test]
fn edge_three_level_spec_composition() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("nested.lemma"),
            r#"
spec base
data factor: 2

spec middle
uses b: base
rule doubled: b.factor * 3

spec top
uses m: middle
rule final: m.doubled + 1
"#
            .to_string(),
        )])
        .unwrap();

    let resp = run_spec(&engine, "top", &[]);
    let display = rule_display(&resp, "final");
    assert_eq!(display, "7", "2 * 3 + 1 = 7");
}

// ===========================================================================
// EDGE CASE 4: Chained percentage operations (explicit scaling)
// ===========================================================================

#[test]
fn edge_chained_percentage_reduction() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("chain.lemma"),
            r#"
spec chain_pct

data amount: 1000

rule half: amount - 50% * amount
rule quarter: half - 50% * half
rule eighth: quarter - 50% * quarter
"#
            .to_string(),
        )])
        .unwrap();

    let resp = run_spec(&engine, "chain_pct", &[]);
    assert_eq!(rule_display(&resp, "half"), "500");
    assert_eq!(rule_display(&resp, "quarter"), "250");
    assert_eq!(rule_display(&resp, "eighth"), "125");
}

// ===========================================================================
// EDGE CASE 5: Veto in one unless arm, value in another — last wins
// If veto arm matches AND a later non-veto arm matches, the veto is overridden
// ===========================================================================

#[test]
fn edge_veto_overridden_by_later_clause() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("veto_override.lemma"),
            r#"
spec veto_override

data value: number
data override_flag: boolean

rule result: 0
  unless value > 100 then veto "Too large"
  unless override_flag then 999
"#
            .to_string(),
        )])
        .unwrap();

    // value=200 (triggers veto), override_flag=true (later clause wins)
    let resp = run_spec(
        &engine,
        "veto_override",
        &[("value", "200"), ("override_flag", "true")],
    );
    // Last matching clause wins: override_flag is true → 999
    assert!(
        !rule_vetoed(&resp, "result"),
        "later clause should override veto"
    );
    assert_eq!(rule_display(&resp, "result"), "999");

    // value=200 (triggers veto), override_flag=false (veto is last match)
    let resp = run_spec(
        &engine,
        "veto_override",
        &[("value", "200"), ("override_flag", "false")],
    );
    assert!(
        rule_vetoed(&resp, "result"),
        "veto should win when no later clause matches"
    );
}

// ===========================================================================
// EDGE CASE 6: Zero multiplied by anything
// ===========================================================================

#[test]
fn edge_zero_multiplication() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("zero.lemma"),
            r#"
spec zero_mult

data quantity: number
data price: number

rule total: quantity * price
rule zero_check: 0 * price
"#
            .to_string(),
        )])
        .unwrap();

    let resp = run_spec(
        &engine,
        "zero_mult",
        &[("quantity", "0"), ("price", "9999")],
    );
    assert_eq!(rule_display(&resp, "total"), "0", "0 * anything = 0");
    assert_eq!(
        rule_display(&resp, "zero_check"),
        "0",
        "literal 0 * price = 0"
    );
}

// ===========================================================================
// EDGE CASE 7: Text comparison edge cases
// ===========================================================================

#[test]
fn edge_text_comparison() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("text.lemma"),
            r#"
spec text_cmp

data status: text
  -> option "active"
  -> option "inactive"
  -> option "pending"

rule is_active: status is "active"
rule is_not_pending: status is not "pending"
"#
            .to_string(),
        )])
        .unwrap();

    let resp = run_spec(&engine, "text_cmp", &[("status", "active")]);
    assert_eq!(rule_display(&resp, "is_active"), "true");
    assert_eq!(rule_display(&resp, "is_not_pending"), "true");

    let resp = run_spec(&engine, "text_cmp", &[("status", "pending")]);
    assert_eq!(rule_display(&resp, "is_active"), "false");
    assert_eq!(rule_display(&resp, "is_not_pending"), "false");
}

// ===========================================================================
// EDGE CASE 8: Unless with complex boolean AND condition
// ===========================================================================

#[test]
fn edge_unless_complex_and() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("complex.lemma"),
            r#"
spec complex_and

data a: boolean
data b: boolean
data c: boolean

rule result: "none"
  unless a and b then "a_and_b"
  unless a and b and c then "all_three"
"#
            .to_string(),
        )])
        .unwrap();

    // All true → last matching is "all_three"
    let resp = run_spec(
        &engine,
        "complex_and",
        &[("a", "true"), ("b", "true"), ("c", "true")],
    );
    assert_eq!(rule_display(&resp, "result"), "all_three");

    // a=true, b=true, c=false → only "a_and_b" matches
    let resp = run_spec(
        &engine,
        "complex_and",
        &[("a", "true"), ("b", "true"), ("c", "false")],
    );
    assert_eq!(rule_display(&resp, "result"), "a_and_b");

    // a=true, b=false, c=true → no unless matches → "none"
    let resp = run_spec(
        &engine,
        "complex_and",
        &[("a", "true"), ("b", "false"), ("c", "true")],
    );
    assert_eq!(rule_display(&resp, "result"), "none");
}

// ===========================================================================
// EDGE CASE 9: Ratio type with custom constraints — multiply only
// ===========================================================================

#[test]
fn edge_ratio_operations() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("ratio.lemma"),
            r#"
spec ratio_math

data discount: ratio -> minimum 0% -> maximum 100%
data base: number

rule raw_mult: base * discount
"#
            .to_string(),
        )])
        .unwrap();

    let resp = run_spec(
        &engine,
        "ratio_math",
        &[("discount", "25%"), ("base", "400")],
    );
    assert_eq!(rule_display(&resp, "raw_mult"), "100", "400 * 25% = 100");
}

#[test]
fn edge_ratio_add_subtract_rejected() {
    let mut engine = Engine::new();
    let result = engine.load([(
        src("ratio.lemma"),
        r#"
spec ratio_math
data discount: ratio
data base: number
rule bad: base - discount
"#
        .to_string(),
    )]);
    assert!(result.is_err());
    let msg = result
        .unwrap_err()
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("; ");
    assert!(msg.contains("scale explicitly"), "got: {}", msg);
}

// ===========================================================================
// EDGE CASE 10: Multiple specs same name different effective dates
// with unpinned import (temporal slicing)
// ===========================================================================

#[test]
fn edge_temporal_before_any_version() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("temporal2.lemma"),
            r#"
spec rates 2024-01-01
data rate: 5%

spec rates 2025-01-01
data rate: 7%

spec calculator 2024-01-01
uses r: rates
data amount: number
rule result: amount * r.rate
"#
            .to_string(),
        )])
        .unwrap();

    // Evaluate at 2024-06-01 → rate = 5%
    let effective = DateTimeValue::from_str("2024-06-01").unwrap();
    let data: HashMap<String, String> = [("amount".to_string(), "1000".into())].into();
    let resp = engine
        .run(None, "calculator", Some(&effective), data, None, false)
        .unwrap();
    let display = rule_display(&resp, "result");
    assert_eq!(display, "50", "2024: 1000 * 5% = 50");

    // Evaluate at 2025-06-01 → rate = 7%
    let effective = DateTimeValue::from_str("2025-06-01").unwrap();
    let data: HashMap<String, String> = [("amount".to_string(), "1000".into())].into();
    let resp = engine
        .run(None, "calculator", Some(&effective), data, None, false)
        .unwrap();
    let display = rule_display(&resp, "result");
    assert_eq!(display, "70", "2025: 1000 * 7% = 70");
}

// ===========================================================================
// EDGE CASE 11: sqrt of perfect square
// ===========================================================================

#[test]
fn edge_sqrt_operations() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("sqrt.lemma"),
            r#"
spec sqrt_test

data value: number -> minimum 0

rule root: sqrt(value)
rule root_of_zero: sqrt(0)
rule root_of_one: sqrt(1)
"#
            .to_string(),
        )])
        .unwrap();

    let resp = run_spec(&engine, "sqrt_test", &[("value", "144")]);
    assert_eq!(rule_display(&resp, "root"), "12", "sqrt(144) = 12");
    assert_eq!(rule_display(&resp, "root_of_zero"), "0", "sqrt(0) = 0");
    assert_eq!(rule_display(&resp, "root_of_one"), "1", "sqrt(1) = 1");
}

// ===========================================================================
// EDGE CASE 12: Division yielding non-terminating decimal
// 1/3 should display cleanly
// ===========================================================================

#[test]
fn edge_non_terminating_division() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("repeating.lemma"),
            r#"
spec repeating

data a: number
data b: number

rule third: a / b
"#
            .to_string(),
        )])
        .unwrap();

    let resp = run_spec(&engine, "repeating", &[("a", "1"), ("b", "3")]);
    // Should display with reasonable precision, not crash or produce infinite string
    let display = rule_display(&resp, "third");
    assert!(!display.is_empty(), "1/3 should have a display value");
    // The exact format depends on engine precision handling
}

// ===========================================================================
// EDGE CASE 13: Multiple unless clauses where NONE match
// ===========================================================================

#[test]
fn edge_no_unless_matches() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("fallthrough.lemma"),
            r#"
spec fallthrough

data x: number

rule category: "default"
  unless x > 100 then "high"
  unless x < 0 then "negative"
"#
            .to_string(),
        )])
        .unwrap();

    // x=50: neither >100 nor <0, so default applies
    let resp = run_spec(&engine, "fallthrough", &[("x", "50")]);
    assert_eq!(rule_display(&resp, "category"), "default");
}

// ===========================================================================
// EDGE CASE 14: Rule referencing another rule that references another rule
// (Deep dependency chain)
// ===========================================================================

#[test]
fn edge_deep_rule_chain() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("chain.lemma"),
            r#"
spec chain

data input: number

rule step1: input + 1
rule step2: step1 * 2
rule step3: step2 - 3
rule step4: step3 / 2
rule step5: step4 + 10
"#
            .to_string(),
        )])
        .unwrap();

    // input=5: step1=6, step2=12, step3=9, step4=4.5, step5=14.5
    let resp = run_spec(&engine, "chain", &[("input", "5")]);
    assert_eq!(
        rule_display(&resp, "step5"),
        "14.5",
        "(((5+1)*2)-3)/2+10 = 14.5"
    );
}

// ===========================================================================
// EDGE CASE 15: Unit conversion chain: measure -> as unit -> as number
// ===========================================================================

#[test]
fn edge_measure_to_number_conversion() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("strip.lemma"),
            r#"
spec unit_strip
uses lemma units

data weight: 5 kilogram

rule kg_value: weight as kilogram as number
rule gram_value: weight as gram as number
"#
            .to_string(),
        )])
        .unwrap();

    let resp = run_spec(&engine, "unit_strip", &[]);
    assert_eq!(
        rule_display(&resp, "kg_value"),
        "5",
        "5kg as kg as number = 5"
    );
    assert_eq!(
        rule_display(&resp, "gram_value"),
        "5000",
        "5kg as g as number = 5000"
    );
}

// ===========================================================================
// EDGE CASE 16: `not` operator on comparison
// ===========================================================================

#[test]
fn edge_not_operator() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("not.lemma"),
            r#"
spec not_test

data active: boolean
data score: number

rule is_inactive: not active
rule below_threshold: not score >= 70
"#
            .to_string(),
        )])
        .unwrap();

    let resp = run_spec(&engine, "not_test", &[("active", "true"), ("score", "80")]);
    assert_eq!(
        rule_display(&resp, "is_inactive"),
        "false",
        "not true = false"
    );
    assert_eq!(
        rule_display(&resp, "below_threshold"),
        "false",
        "not (80>=70) = false"
    );

    let resp = run_spec(&engine, "not_test", &[("active", "false"), ("score", "50")]);
    assert_eq!(
        rule_display(&resp, "is_inactive"),
        "true",
        "not false = true"
    );
    assert_eq!(
        rule_display(&resp, "below_threshold"),
        "true",
        "not (50>=70) = true"
    );
}

// ===========================================================================
// EDGE CASE 17: With binding + overriding data from parent
// ===========================================================================

#[test]
fn edge_with_binding_override() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("with.lemma"),
            r#"
spec config
data tax_rate: 10%
data base_price: 100
rule tax_amount: base_price * tax_rate
rule total: base_price + tax_amount

spec custom
uses c: config
  -> with tax_rate: 25%
rule custom_total: c.total
"#
            .to_string(),
        )])
        .unwrap();

    let resp = run_spec(&engine, "custom", &[]);
    // c.total should use overridden tax_rate of 25%
    // tax_amount = 100 * 0.25 = 25, total = 100 + 25 = 125
    assert_eq!(
        rule_display(&resp, "custom_total"),
        "125",
        "with override: 100 + 100*25% = 125"
    );
}

fn main() {}

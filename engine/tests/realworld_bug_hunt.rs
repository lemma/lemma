//! Aggressive bug-hunting scenarios

use lemma::{DateTimeValue, Engine, SourceType};
use std::collections::HashMap;
use std::path::PathBuf;
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
// BUG HUNT 1: compound rate × hours as money
// ===========================================================================

#[test]
fn hunt_compound_rate_times_hours_as_eur() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("as_prec.lemma"),
            r#"
spec as_prec
uses lemma units

data money: measure
  -> unit eur: 1.00

data wage: measure
  -> unit eur_per_hour: eur/hour

data hours_worked: 160 hour
data hourly_rate: wage -> suggest 50 eur_per_hour

rule total_pay: (hourly_rate * hours_worked) as eur
"#
            .to_string(),
        )])
        .unwrap();

    let resp = run_spec(&engine, "as_prec", &[("hourly_rate", "50 eur_per_hour")]);
    let display = rule_display(&resp, "total_pay");
    // 50 eur/hour * 160 hour = 8000 eur
    assert_eq!(display, "8000 eur", "50 eur/hour * 160 hour = 8000 eur");
}

// ===========================================================================
// BUG HUNT 1b: `as` binds tighter than `/`
// Docs: `balance / rate as month` means `balance / (rate as month)`.
// That form is a planning error; suggest `(balance / burn_rate) as month`.
// ===========================================================================

#[test]
fn hunt_as_precedence_with_division() {
    let mut engine = Engine::new();
    let err = engine
        .load([(
            src("as_div_prec.lemma"),
            r#"
spec burn
uses lemma units
data money: measure
  -> unit eur: 1
data rate: measure
  -> unit eur_per_month: eur/month
data balance: 120000 eur
data burn_rate: 10000 eur_per_month
rule runway: balance / burn_rate as month
"#
            .to_string(),
        )])
        .expect_err("unparenthesized / rate as month must fail planning");
    let joined = err
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("; ");
    assert!(
        joined.contains("(balance / burn_rate) as month"),
        "expected parenthesized suggestion in error, got: {joined}"
    );
}

#[test]
fn hunt_as_precedence_division_with_parentheses_evaluates() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("as_div_ok.lemma"),
            r#"
spec burn
uses lemma units
data money: measure
  -> unit eur: 1
data rate: measure
  -> unit eur_per_month: eur/month
data balance: 120000 eur
data burn_rate: 10000 eur_per_month
rule runway: (balance / burn_rate) as month
"#
            .to_string(),
        )])
        .unwrap();

    let resp = run_spec(&engine, "burn", &[]);
    let display = rule_display(&resp, "runway");
    assert_eq!(
        display, "12 month",
        "120000 eur / 10000 eur/month = 12 month"
    );
}

// ===========================================================================
// BUG HUNT 2: Ratio compared to ratio
// 25% > 10% should be true
// ===========================================================================

#[test]
fn hunt_ratio_comparison() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("ratio_cmp.lemma"),
            r#"
spec ratio_cmp

data discount: ratio

rule big_discount: discount > 20%
rule small_discount: discount < 5%
rule exact_ten: discount is 10%
"#
            .to_string(),
        )])
        .unwrap();

    let resp = run_spec(&engine, "ratio_cmp", &[("discount", "25%")]);
    assert_eq!(rule_display(&resp, "big_discount"), "true", "25% > 20%");
    assert_eq!(
        rule_display(&resp, "small_discount"),
        "false",
        "25% not < 5%"
    );
    assert_eq!(rule_display(&resp, "exact_ten"), "false", "25% is not 10%");

    let resp = run_spec(&engine, "ratio_cmp", &[("discount", "10%")]);
    assert_eq!(rule_display(&resp, "exact_ten"), "true", "10% is 10%");
}

// ===========================================================================
// BUG HUNT 3: Veto propagation through boolean check
// If rule A vetoes, and rule B says "A and something_else", does B veto or false?
// ===========================================================================

#[test]
fn hunt_veto_in_boolean_expression() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("veto_bool.lemma"),
            r#"
spec veto_bool

data value: number
data flag: boolean

rule validated: value
  unless value < 0 then veto "negative"

rule combined: validated > 10 and flag
"#
            .to_string(),
        )])
        .unwrap();

    // value=-5 makes validated veto. combined depends on validated > 10.
    // A veto in a comparison should propagate.
    let resp = run_spec(&engine, "veto_bool", &[("value", "-5"), ("flag", "true")]);
    assert!(rule_vetoed(&resp, "validated"), "negative value vetoes");
    assert!(
        rule_vetoed(&resp, "combined"),
        "veto should propagate through comparison"
    );
}

// ===========================================================================
// BUG HUNT 4: Chained `is veto` on deeply nested veto
// ===========================================================================

#[test]
fn hunt_is_veto_on_transitive_veto() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("deep_veto.lemma"),
            r#"
spec deep_veto

data input: number

rule step1: input
  unless input < 0 then veto "bad input"

rule step2: step1 * 2

rule step3: step2 + 1

rule check_step3: step3 is veto
"#
            .to_string(),
        )])
        .unwrap();

    // input=-1: step1 vetoes, step2 vetoes (depends on step1), step3 vetoes
    let resp = run_spec(&engine, "deep_veto", &[("input", "-1")]);
    assert!(
        rule_vetoed(&resp, "step3"),
        "step3 should veto transitively"
    );
    assert_eq!(
        rule_display(&resp, "check_step3"),
        "true",
        "step3 is veto = true"
    );

    // input=5: everything is fine
    let resp = run_spec(&engine, "deep_veto", &[("input", "5")]);
    assert!(!rule_vetoed(&resp, "step3"));
    assert_eq!(
        rule_display(&resp, "check_step3"),
        "false",
        "step3 is not veto"
    );
}

// ===========================================================================
// BUG HUNT 5: Comparison between measure and literal
// 5 kilogram > 3 kilogram (same unit) — should work
// ===========================================================================

#[test]
fn hunt_measure_comparison() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("quantity_cmp.lemma"),
            r#"
spec quantity_cmp
uses lemma units

data weight: units.mass

rule over_five: weight > 5 kilogram
rule under_one: weight < 1 kilogram
rule exactly_three: weight is 3 kilogram
"#
            .to_string(),
        )])
        .unwrap();

    let resp = run_spec(&engine, "quantity_cmp", &[("weight", "3 kilogram")]);
    assert_eq!(rule_display(&resp, "over_five"), "false");
    assert_eq!(rule_display(&resp, "under_one"), "false");
    assert_eq!(rule_display(&resp, "exactly_three"), "true");

    let resp = run_spec(&engine, "quantity_cmp", &[("weight", "10 kilogram")]);
    assert_eq!(rule_display(&resp, "over_five"), "true");
}

// ===========================================================================
// BUG HUNT 6: Money arithmetic with decimals
// Ensure no floating point issues with currency
// ===========================================================================

#[test]
fn hunt_money_precision() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("money.lemma"),
            r#"
spec money_test

data money: measure
  -> decimals 2
  -> unit eur: 1.00

data price: money -> suggest 19.99 eur
data tax_rate: 21%

rule tax_amount: price * tax_rate
rule total: price + tax_amount
rule ten_items: price * 10
"#
            .to_string(),
        )])
        .unwrap();

    let resp = run_spec(&engine, "money_test", &[("price", "19.99 eur")]);
    // 19.99 * 21% = 4.1979 → displayed with 2 decimals? or full precision?
    let tax = rule_display(&resp, "tax_amount");
    // Should be exact: 19.99 * 0.21 = 4.1979
    assert_eq!(
        tax, "4.20 eur",
        "19.99 * 21% = 4.1979, rounded to 2dp = 4.20 eur"
    );

    // 19.99 * 10 = 199.90
    let ten = rule_display(&resp, "ten_items");
    assert_eq!(ten, "199.90 eur", "19.99 * 10 = 199.90 eur");
}

// ===========================================================================
// BUG HUNT 7: Unless condition referencing a rule that vetoes
// Last-unless-first: `is veto` last wins; reverse order Propagates the veto.
// ===========================================================================

#[test]
fn hunt_unless_condition_with_veto_rule() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("unless_veto.lemma"),
            r#"
spec unless_veto_cond

data input: number

rule validated: input
  unless input > 1000 then veto "too large"

rule result: 0
  unless validated > 50 then 100
  unless validated is veto then 0
"#
            .to_string(),
        )])
        .unwrap();

    // input=2000: validated vetoes. Scan last-unless-first: `is veto` wins first;
    // the compare arm above is never evaluated.
    let resp = run_spec(&engine, "unless_veto_cond", &[("input", "2000")]);
    let display = rule_display(&resp, "result");
    assert_eq!(display, "0", "is veto arm must win when validated vetoes");

    // input=100: validated = 100. 100 > 50 → result = 100
    let resp = run_spec(&engine, "unless_veto_cond", &[("input", "100")]);
    assert_eq!(rule_display(&resp, "result"), "100");
}

#[test]
fn hunt_unless_compare_last_propagates_vetoed_condition() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("unless_veto_prop.lemma"),
            r#"
spec unless_veto_prop

data input: number

rule validated: input
  unless input > 1000 then veto "too large"

rule result: 0
  unless validated is veto then 0
  unless validated > 50 then 100
"#
            .to_string(),
        )])
        .unwrap();

    let resp = run_spec(&engine, "unless_veto_prop", &[("input", "2000")]);
    let result = resp
        .results
        .values()
        .find(|r| r.rule.name == "result")
        .expect("result");
    assert!(
        result.vetoed,
        "vetoed compare condition must Propagate when scanned first"
    );
    assert_eq!(result.veto_reason.as_deref(), Some("too large"));
}

// ===========================================================================
// BUG HUNT 8: Operator precedence: multiplication before addition
// rule: a + b * c should be a + (b * c), not (a + b) * c
// ===========================================================================

#[test]
fn hunt_operator_precedence() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("precedence.lemma"),
            r#"
spec precedence

data a: number
data b: number
data c: number

rule add_then_mult: a + b * c
rule explicit_parens: a + (b * c)
rule mult_first: a * b + c
"#
            .to_string(),
        )])
        .unwrap();

    // a=2, b=3, c=4
    // add_then_mult = 2 + 3*4 = 2 + 12 = 14 (if precedence correct)
    //                 vs (2+3)*4 = 20 (if wrong)
    let resp = run_spec(&engine, "precedence", &[("a", "2"), ("b", "3"), ("c", "4")]);
    let result = rule_display(&resp, "add_then_mult");
    assert_eq!(
        result, "14",
        "2 + 3*4 = 14 (multiplication before addition)"
    );

    let explicit = rule_display(&resp, "explicit_parens");
    assert_eq!(explicit, "14", "2 + (3*4) = 14");

    // mult_first: 2*3 + 4 = 10
    let mult = rule_display(&resp, "mult_first");
    assert_eq!(mult, "10", "2*3 + 4 = 10");
}

// ===========================================================================
// BUG HUNT 9: Empty string handling in text option
// ===========================================================================

#[test]
fn hunt_text_empty_string() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("empty.lemma"),
            r#"
spec empty_text

data label: text
  -> option ""
  -> option "hello"

rule is_empty: label is ""
rule is_hello: label is "hello"
"#
            .to_string(),
        )])
        .expect("empty string in -> option must be a valid Spec");

    let resp = run_spec(&engine, "empty_text", &[("label", "")]);
    assert_eq!(rule_display(&resp, "is_empty"), "true");
    assert_eq!(rule_display(&resp, "is_hello"), "false");
}

// ===========================================================================
// BUG HUNT 10: Very small decimal precision
// 0.0000001 * 10000000 should = 1
// ===========================================================================

#[test]
fn hunt_small_decimal_precision() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("precision.lemma"),
            r#"
spec precision

data tiny: number
data large: number

rule product: tiny * large
rule sum: tiny + tiny + tiny + tiny + tiny + tiny + tiny + tiny + tiny + tiny
"#
            .to_string(),
        )])
        .unwrap();

    let resp = run_spec(
        &engine,
        "precision",
        &[("tiny", "0.0000001"), ("large", "10000000")],
    );
    assert_eq!(
        rule_display(&resp, "product"),
        "1",
        "0.0000001 * 10000000 = 1 exactly"
    );

    // 0.0000001 * 10 = 0.000001
    let resp = run_spec(&engine, "precision", &[("tiny", "0.1"), ("large", "3")]);
    // sum = 0.1 * 10 = 1.0 (exact rational arithmetic, no float drift)
    let sum = rule_display(&resp, "sum");
    assert_eq!(sum, "1", "0.1 added 10 times = 1 (exact rational)");
}

// ===========================================================================
// BUG HUNT 11: Self-referential rule name collision
// A rule named same as a data field — which wins?
// ===========================================================================

#[test]
fn hunt_name_collision_data_rule() {
    let mut engine = Engine::new();
    let result = engine.load([(
        src("collision.lemma"),
        r#"
spec collision

data value: 10

rule value: value * 2
"#
        .to_string(),
    )]);

    // This should either be a parse error (name collision) or have clear semantics
    match result {
        Ok(_) => {
            panic!("Expected name collision between data 'value' and rule 'value' to be rejected");
        }
        Err(e) => {
            // Good — Lemma rejects name collision at planning time
            let msg = format!("{:?}", e);
            assert!(
                msg.contains("value")
                    || msg.contains("collision")
                    || msg.contains("already")
                    || msg.contains("duplicate"),
                "Error should mention the conflicting name: {}",
                msg
            );
        }
    }
}

// ===========================================================================
// BUG HUNT 12: Circular dependency detection
// ===========================================================================

#[test]
fn hunt_circular_dependency() {
    let mut engine = Engine::new();
    let result = engine.load([(
        src("circular.lemma"),
        r#"
spec circular

data seed: 1

rule a: b + 1
rule b: a + 1
"#
        .to_string(),
    )]);

    match result {
        Ok(_) => panic!("Circular dependency should be rejected at planning time"),
        Err(e) => {
            let msg = format!("{:?}", e);
            assert!(
                msg.contains("cycle") || msg.contains("circular") || msg.contains("depend"),
                "Error should mention circular dependency: {}",
                msg
            );
        }
    }
}

// ===========================================================================
// BUG HUNT 13: Unless with comparison to measure (mixed units)
// 5 kilogram > 3000 gram — cross-unit comparison
// ===========================================================================

#[test]
fn hunt_cross_unit_comparison_in_unless() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("cross_unit.lemma"),
            r#"
spec cross_unit
uses lemma units

data weight: units.mass

rule category: "normal"
  unless weight > 5 kilogram then "heavy"
  unless weight < 100 gram then "tiny"
"#
            .to_string(),
        )])
        .unwrap();

    // 3 kilogram = 3000 gram, which is > 100g but < 5kg → "normal"
    let resp = run_spec(&engine, "cross_unit", &[("weight", "3 kilogram")]);
    assert_eq!(rule_display(&resp, "category"), "normal");

    // 50 gram < 100 gram → "tiny" (last matching)
    let resp = run_spec(&engine, "cross_unit", &[("weight", "50 gram")]);
    assert_eq!(rule_display(&resp, "category"), "tiny");

    // 10 kilogram > 5 kilogram → "heavy"
    // But also 10kg > 100g so "tiny" would NOT match (10kg is NOT < 100g)
    let resp = run_spec(&engine, "cross_unit", &[("weight", "10 kilogram")]);
    assert_eq!(rule_display(&resp, "category"), "heavy");
}

fn main() {}

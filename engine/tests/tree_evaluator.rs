//! Integration tests for the tree evaluator (planning → NormalForm walk).

use lemma::{DateTimeValue, Engine};
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::str::FromStr;

const PRICING_SPEC: &str = r#"
spec pricing
data quantity: number
data price: 10
rule total: quantity * price
rule discount: 0
  unless quantity >= 10 then 5
  unless quantity >= 50 then 15
"#;

const CALC_SPEC: &str = r#"
spec calc

data money: measure
  -> decimals 2
  -> unit eur: 1

data hourly_rate: 85.00 eur
data hours_worked: 37.5
data is_rush: boolean
data is_super_rush: boolean

rule labor: hourly_rate * hours_worked
rule rush_surcharge: 0 eur
  unless is_rush then labor * 25%
  unless is_super_rush then labor * 50%
rule subtotal: labor + rush_surcharge
rule vat: subtotal * 21%
rule total: subtotal + vat
"#;

fn load_engine(code: &str, path: &str) -> Engine {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(path))),
            code.to_string(),
        )])
        .expect("load spec");
    engine
}

fn run_measure(engine: &Engine, measure: &str) -> lemma::Response {
    let now = DateTimeValue::now();
    let mut data = HashMap::new();
    data.insert("quantity".to_string(), measure.to_string());
    engine
        .run(None, "pricing", Some(&now), data, None, false)
        .expect("run pricing")
}

fn run_calc(data: HashMap<String, String>) -> lemma::Response {
    let engine = load_engine(CALC_SPEC, "calc.lemma");
    let now = DateTimeValue::now();
    engine
        .run(None, "calc", Some(&now), data, None, false)
        .expect("run calc")
}

fn rule_number(response: &lemma::Response, rule: &str) -> Decimal {
    let result = response.get(rule).unwrap_or_else(|_| panic!("rule {rule}"));
    assert!(!result.vetoed, "rule {rule} vetoed");
    Decimal::from_str(
        result
            .value
            .as_ref()
            .expect("rule result value")
            .number
            .as_ref()
            .expect("number payload"),
    )
    .expect("decimal")
}

fn rule_display(response: &lemma::Response, rule: &str) -> String {
    response
        .get(rule)
        .unwrap_or_else(|_| panic!("rule {rule}"))
        .display()
        .expect("display")
        .to_string()
}

fn rule_measure_display(response: &lemma::Response, rule: &str) -> String {
    response
        .get(rule)
        .unwrap_or_else(|_| panic!("rule {rule}"))
        .display()
        .expect("display")
        .to_string()
}

#[test]
fn discount_unless_measure_below_ten() {
    let engine = load_engine(PRICING_SPEC, "pricing.lemma");
    let response = run_measure(&engine, "5");
    assert_eq!(rule_number(&response, "discount"), Decimal::ZERO);
    assert_eq!(rule_number(&response, "total"), Decimal::from(50));
}

#[test]
fn discount_unless_measure_ten() {
    let engine = load_engine(PRICING_SPEC, "pricing.lemma");
    let response = run_measure(&engine, "10");
    assert_eq!(rule_number(&response, "discount"), Decimal::from(5));
}

#[test]
fn discount_unless_measure_fifty() {
    let engine = load_engine(PRICING_SPEC, "pricing.lemma");
    let response = run_measure(&engine, "50");
    assert_eq!(rule_number(&response, "discount"), Decimal::from(15));
}

#[test]
fn discount_unless_measure_between_tiers() {
    let engine = load_engine(PRICING_SPEC, "pricing.lemma");
    let response = run_measure(&engine, "25");
    assert_eq!(rule_number(&response, "discount"), Decimal::from(5));
}

#[test]
fn nested_rule_reference_in_arithmetic() {
    let code = r#"
spec chain
data x: number
rule base: x * 2
rule offset: 1
  unless x >= 5 then 10
rule total: base + offset
"#;
    let engine = load_engine(code, "chain.lemma");
    let now = DateTimeValue::now();
    let mut data = HashMap::new();
    data.insert("x".to_string(), "3".to_string());
    let response = engine
        .run(None, "chain", Some(&now), data, None, false)
        .expect("run");
    assert_eq!(rule_number(&response, "total"), Decimal::from(7));
}

#[test]
fn inlined_piecewise_subexpression_does_not_return_early_from_parent_rule() {
    let mut data = HashMap::new();
    data.insert("is_rush".into(), "true".into());
    data.insert("is_super_rush".into(), "false".into());
    let response = run_calc(data);

    assert_eq!(rule_display(&response, "total"), "4821.09 eur");
    assert_eq!(
        rule_measure_display(&response, "rush_surcharge"),
        "796.88 eur"
    );
    assert_ne!(
        rule_display(&response, "total"),
        rule_measure_display(&response, "rush_surcharge"),
        "total must not equal inlined rush_surcharge alone"
    );
}

#[test]
fn is_veto_detects_transitive_veto() {
    let code = r#"
spec deep_veto
data input: number
rule step1: input
  unless input < 0 then veto "bad input"
rule step2: step1 * 2
rule step3: step2 + 1
rule check_step3: step3 is veto
"#;
    let engine = load_engine(code, "deep_veto.lemma");
    let now = DateTimeValue::now();
    let mut bad = HashMap::new();
    bad.insert("input".to_string(), "-1".to_string());
    let resp = engine
        .run(None, "deep_veto", Some(&now), bad, None, false)
        .expect("run");
    assert_eq!(rule_display(&resp, "check_step3"), "true");

    let mut good = HashMap::new();
    good.insert("input".to_string(), "5".to_string());
    let resp = engine
        .run(None, "deep_veto", Some(&now), good, None, false)
        .expect("run");
    assert_eq!(rule_display(&resp, "check_step3"), "false");
}

#[test]
fn unless_is_veto_arm_last_wins_when_validated_vetoes() {
    // Scan is last-unless-first. `is veto` is last so it is checked first and
    // wins; the compare arm above is never evaluated (a vetoed compare would
    // Propagate, not skip).
    let code = r#"
spec unless_veto_cond
data input: number
rule validated: input
  unless input > 1000 then veto "too large"
rule result: 0
  unless validated > 50 then 100
  unless validated is veto then 0
"#;
    let engine = load_engine(code, "unless_veto.lemma");
    let now = DateTimeValue::now();
    let mut large = HashMap::new();
    large.insert("input".to_string(), "2000".to_string());
    let resp = engine
        .run(None, "unless_veto_cond", Some(&now), large, None, false)
        .expect("run");
    assert_eq!(rule_display(&resp, "result"), "0");

    let mut ok = HashMap::new();
    ok.insert("input".to_string(), "100".to_string());
    let resp = engine
        .run(None, "unless_veto_cond", Some(&now), ok, None, false)
        .expect("run");
    assert_eq!(rule_display(&resp, "result"), "100");
}

#[test]
fn unless_compare_arm_last_propagates_veto_from_validated() {
    // Reverse arm order: compare is last so it is scanned first. Vetoed
    // condition Propagates — does not fall through to `is veto` or default.
    let code = r#"
spec unless_veto_prop
data input: number
rule validated: input
  unless input > 1000 then veto "too large"
rule result: 0
  unless validated is veto then 0
  unless validated > 50 then 100
"#;
    let engine = load_engine(code, "unless_veto_prop.lemma");
    let now = DateTimeValue::now();
    let mut large = HashMap::new();
    large.insert("input".to_string(), "2000".to_string());
    let resp = engine
        .run(None, "unless_veto_prop", Some(&now), large, None, false)
        .expect("run");
    let result = resp.results.get("result").expect("result");
    assert!(
        result.vetoed,
        "vetoed unless condition must Propagate, not fall through"
    );
    assert_eq!(result.veto_reason.as_deref(), Some("too large"));
}

#[test]
fn calc_total_without_rush_is_not_zero_surcharge_path() {
    let mut data = HashMap::new();
    data.insert("is_rush".into(), "false".into());
    data.insert("is_super_rush".into(), "false".into());
    let response = run_calc(data);
    assert_eq!(rule_display(&response, "total"), "3856.88 eur");
    assert_eq!(
        rule_measure_display(&response, "rush_surcharge"),
        "0.00 eur"
    );
}

use lemma::{DateTimeValue, Engine, SourceType};
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::str::FromStr;

fn load(engine: &mut Engine, code: &str) {
    engine
        .load([(SourceType::Volatile, code.to_string())])
        .unwrap();
}

fn run(
    engine: &Engine,
    spec: &str,
    data: HashMap<String, String>,
    explain: bool,
) -> lemma::Response {
    let now = DateTimeValue::now();
    engine
        .run(None, spec, Some(&now), data, None, explain)
        .expect("evaluation must complete")
}

fn rule_result<'response>(
    response: &'response lemma::Response,
    rule_name: &str,
) -> &'response lemma::RuleResult {
    response
        .results
        .values()
        .find(|result| result.rule.name == rule_name)
        .unwrap_or_else(|| panic!("rule '{rule_name}' must be in the response"))
}

#[test]
fn rule_result_rounds_excess_precision_at_output() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec precision
rule third: 1 / 3
"#,
    );

    let response = run(&engine, "precision", HashMap::new(), false);
    let result = rule_result(&response, "third");
    assert!(!result.vetoed);
    assert_eq!(
        result.result.as_ref().expect("rule result value").number,
        Some(Decimal::from_str("0.3333333333333333333333333333").expect("valid decimal"))
    );
}

#[test]
fn rule_result_rounds_tiny_value_to_zero_at_output() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec tiny
rule dust: 1 / (10 ^ 30)
"#,
    );

    let response = run(&engine, "tiny", HashMap::new(), false);
    let result = rule_result(&response, "dust");
    assert!(!result.vetoed);
    assert_eq!(
        result.result.as_ref().expect("rule result value").number,
        Some(Decimal::ZERO)
    );
}

#[test]
fn magnitude_overflow_vetoes_at_output() {
    let max = Decimal::MAX.normalize().to_string();
    let code = format!(
        r#"
spec overflow
data max_val: {max}
data two: 2
rule product: max_val * two
"#
    );

    let mut engine = Engine::new();
    load(&mut engine, &code);

    let response = run(&engine, "overflow", HashMap::new(), false);
    let result = rule_result(&response, "product");
    assert!(result.vetoed);
    assert_eq!(
        result.veto_reason.as_deref(),
        Some("Calculated result exceeds decimal value limit")
    );
}

#[test]
fn intermediate_exceeding_decimal_range_stored_exactly_output_rounds() {
    let max = Decimal::MAX.normalize().to_string();
    let code = format!(
        r#"
spec magnitude_overflow_boundary
data max_val: {max}
data two: 2
rule huge: max_val * two
rule safe: huge / two
"#
    );

    let mut engine = Engine::new();
    load(&mut engine, &code);

    let response = run(&engine, "magnitude_overflow_boundary", HashMap::new(), true);
    let huge = rule_result(&response, "huge");
    assert!(huge.vetoed);
    assert_eq!(
        huge.veto_reason.as_deref(),
        Some("Calculated result exceeds decimal value limit")
    );
    let explanation = huge.explanation.as_ref().expect("huge explanation");
    assert!(
        !explanation.result.vetoed(),
        "rule_results must store exact Q; RuleResultValue veto is separate"
    );

    let response = run(
        &engine,
        "magnitude_overflow_boundary",
        HashMap::new(),
        false,
    );
    let safe = rule_result(&response, "safe");
    assert!(!safe.vetoed);
    assert_eq!(
        safe.result.as_ref().expect("rule result value").number,
        Some(Decimal::from_str(&max).expect("max decimal"))
    );
}

#[test]
fn accepted_measure_data_plans_and_evaluates_without_decimal_limit_rejection() {
    let code = r#"
spec weight
data money: measure
  -> unit eur: 1
  -> unit milli: 0.001
  -> minimum 1000 eur
rule r: money
"#;

    let mut engine = Engine::new();
    load(&mut engine, code);

    let response = run(
        &engine,
        "weight",
        HashMap::from([("money".to_string(), "1000 eur".into())]),
        false,
    );
    let result = rule_result(&response, "r");
    assert!(!result.vetoed);
    assert!(
        result.veto_reason.is_none(),
        "accepted measure input must not veto for decimal limit"
    );
}

#[test]
fn overlay_decimals_constraint_rejects_excess_precision_at_input() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec price_check
data price: number -> decimals 2
rule r: price
"#,
    );

    let rejected = run(
        &engine,
        "price_check",
        HashMap::from([("price".to_string(), "1.234".into())]),
        false,
    );
    let result = rule_result(&rejected, "r");
    assert!(
        result.vetoed,
        "overlay with excess decimal precision must veto"
    );
    let reason = result.veto_reason.as_deref().unwrap_or("");
    assert!(
        reason.contains("exceeds decimals constraint 2"),
        "expected decimals constraint message, got: {reason}"
    );
    assert!(
        reason.contains("1.234"),
        "veto must show rejected scale, not rounded 1.23, got: {reason}"
    );
    assert!(
        !reason.contains("decimal value limit"),
        "must not use RuleResultValue veto message for input constraint, got: {reason}"
    );

    let accepted = run(
        &engine,
        "price_check",
        HashMap::from([("price".to_string(), "1.23".into())]),
        false,
    );
    let result = rule_result(&accepted, "r");
    assert!(!result.vetoed);
    assert_eq!(
        result.result.as_ref().expect("rule result value").number,
        Some(Decimal::from_str("1.23").expect("1.23"))
    );
}

#[test]
fn overlay_measure_decimals_constraint_uses_declared_unit() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec delivery
data cost: measure -> decimals 2 -> unit eur: 1
rule r: cost
"#,
    );

    let rejected = run(
        &engine,
        "delivery",
        HashMap::from([("cost".to_string(), "1.234 eur".into())]),
        false,
    );
    let result = rule_result(&rejected, "r");
    assert!(result.vetoed);
    let reason = result.veto_reason.as_deref().unwrap_or("");
    assert!(
        reason.contains("exceeds decimals constraint 2"),
        "expected decimals constraint message, got: {reason}"
    );
    assert!(
        reason.contains("1.234"),
        "veto must show rejected scale, not rounded 1.23, got: {reason}"
    );

    let accepted = run(
        &engine,
        "delivery",
        HashMap::from([("cost".to_string(), "1.23 eur".into())]),
        false,
    );
    let result = rule_result(&accepted, "r");
    assert!(!result.vetoed);
    let measure = result
        .result
        .as_ref()
        .expect("rule result value")
        .measure
        .as_ref()
        .expect("measure map");
    assert_eq!(
        measure.get("eur").copied(),
        Some(Decimal::from_str("1.23").expect("1.23"))
    );
}

use lemma::DateTimeValue;
use lemma::__test_support::{checked_div, checked_mul, decimal_to_rational, rational_new};
use lemma::{Engine, ValueKind};
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::str::FromStr;

fn decimal_lit(d: &str) -> Decimal {
    Decimal::from_str(d).unwrap()
}

fn run_override_veto_message(
    engine: &Engine,
    spec: &str,
    data: HashMap<String, String>,
    rule_name: &str,
) -> String {
    let now = DateTimeValue::now();
    let resp = engine
        .run(None, spec, Some(&now), data, None, true)
        .unwrap_or_else(|err| panic!("run must complete with veto, not Error: {err}"));
    let rr = resp
        .results
        .get(rule_name)
        .unwrap_or_else(|| panic!("rule '{rule_name}' not found"));
    assert!(
        rr.vetoed,
        "rule '{rule_name}' must veto on invalid override, got {:?}",
        rr.result()
    );
    rr.veto_reason.clone().expect("veto reason")
}

#[test]
fn measure_comparison_converts_units_before_comparing() {
    // unit usd 0.84: 1 USD = 0.84 EUR (canonical).
    // 100 USD = 84 EUR in base, so 150 EUR > 100 USD triggers veto.
    let code = r#"
spec pricing
data money: measure
    -> unit eur: 1
    -> unit usd: 0.84

data price: money

rule check: true
    unless price > 100 usd then veto "This price is too high."
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(
            None,
            "pricing",
            Some(&now),
            HashMap::from([("price".to_string(), "150 eur".into())]),
            None,
            true,
        )
        .unwrap();

    let rule_result = response
        .results
        .values()
        .find(|r| r.rule.name == "check")
        .unwrap();

    assert!(rule_result.vetoed);
    assert_eq!(
        rule_result.veto_reason.as_deref(),
        Some("This price is too high.")
    );
}

#[test]
fn measure_data_value_rejects_unknown_unit() {
    let code = r#"
spec pricing
data money: measure
    -> unit eur: 1
    -> unit usd: 0.84

data price: money

rule check: price
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let msg = run_override_veto_message(
        &engine,
        "pricing",
        HashMap::from([("price".to_string(), "100 btc".into())]),
        "check",
    );
    assert!(msg.contains("btc"), "actual veto: {msg}");
}

#[test]
fn measure_as_operator_converts_units() {
    // unit usd 0.84: 1 USD = 0.84 EUR (canonical). 100 usd as eur = 84 eur.
    let code = r#"
spec pricing
data money: measure
    -> unit eur: 1
    -> unit usd: 0.84

data amount: 100 usd
rule price_eur: amount as eur
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "pricing", Some(&now), HashMap::new(), None, true)
        .unwrap();
    let rule_result = response
        .results
        .values()
        .find(|r| r.rule.name == "price_eur")
        .unwrap();

    assert!(!rule_result.vetoed);
    let lit = rule_result
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("value");
    assert!(
        matches!(lit.value, ValueKind::Measure(_)),
        "Expected measure value, got: {:?}",
        lit.value
    );
    let measure = rule_result
        .result
        .as_ref()
        .and_then(|v| v.measure.as_ref())
        .expect("measure map");
    let amount = measure.get("eur").copied().expect("eur in measure map");
    assert_eq!(amount, Decimal::from(84));
    assert!(measure.contains_key("eur"));
}

#[test]
fn measure_add_subtract_converts_units_when_same_family() {
    // Measure add/subtract with different units (same measure family) must convert, not Veto.
    // Regression: previously returned Veto "Cannot apply '-' to values with different units".
    let code = r#"
spec t
data money: measure -> unit eur: 1.00 -> unit usd: 0.84
data gross: 7600 usd
data pension: 0 eur
rule taxable: gross - pension
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "t", Some(&now), HashMap::new(), None, true)
        .unwrap();

    let rule_result = response
        .results
        .values()
        .find(|r| r.rule.name == "taxable")
        .unwrap();

    assert!(!rule_result.vetoed);
    let measure = rule_result
        .result
        .as_ref()
        .expect("rule result value")
        .measure
        .as_ref()
        .expect("measure map");
    assert_eq!(measure.get("eur").copied(), Some(Decimal::from(6384)));
    assert_eq!(measure.get("usd").copied(), Some(Decimal::from(7600)));
}

#[test]
fn measure_as_operator_rejects_unknown_unit() {
    let code = r#"
spec pricing
data money: measure
    -> unit eur: 1
    -> unit usd: 0.84

rule price_gbp: 100 as gbp
"#;

    let mut engine = Engine::new();
    let load_err = engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap_err();
    let msg = load_err
        .errors
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("; ");

    assert!(msg.contains("Unknown unit 'gbp'"), "actual error: {msg}");
    assert!(msg.contains("Valid units:"), "actual error: {msg}");
}

#[test]
fn named_measure_type_comparison_with_unit_literal() {
    // Regression: planning rejected `package_weight > 1 kilogram` with
    // "Cannot compare different measure types: measure and weight" because it
    // used strict name equality instead of same_measure_family.
    let code = r#"
spec shipping

data weight: measure -> unit kilogram: 1.0

data package_weight: 2.5 kilogram

rule base_shipping: 5.99
    unless package_weight > 1 kilogram then 8.99
    unless package_weight > 5 kilogram then 15.99
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "shipping", Some(&now), HashMap::new(), None, true)
        .unwrap();

    let rule_result = response
        .results
        .values()
        .find(|r| r.rule.name == "base_shipping")
        .unwrap();

    // package_weight = 2.5 kg, which is > 1 kg but not > 5 kg, so second unless wins: 8.99
    assert!(!rule_result.vetoed);
    let lit = rule_result
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("value");
    match &lit.value {
        ValueKind::Number(d) => {
            assert_eq!(
                lemma::ValueKind::Number(d.clone())
                    .as_decimal_magnitude()
                    .unwrap(),
                Decimal::new(899, 2)
            );
        }
        other => panic!("Expected Number value, got {:?}", other),
    }
}

#[test]
fn named_measure_type_arithmetic_within_same_family() {
    // Regression: planning rejected arithmetic between values of the same
    // measure family when their type names differed (e.g. "measure" vs "weight").
    let code = r#"
spec shipping

data money: measure -> unit USD: 1.00

data base_fee: 5.99 USD
data surcharge: 2.00 USD

rule total: base_fee + surcharge
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "shipping", Some(&now), HashMap::new(), None, true)
        .unwrap();

    let rule_result = response
        .results
        .values()
        .find(|r| r.rule.name == "total")
        .unwrap();

    assert!(!rule_result.vetoed);
    let lit = rule_result
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("value");
    match &lit.value {
        ValueKind::Measure(d) => {
            assert_eq!(
                lemma::ValueKind::Number(d.clone())
                    .as_decimal_magnitude()
                    .unwrap(),
                Decimal::new(799, 2)
            );
        }
        other => panic!("Expected Measure value, got {:?}", other),
    }
}

// ═══════════════════════════════════════════════════════════════════
// Regression: convert_unit must use from_factor / to_factor.
// Measure unit factor = canonical units per 1 unit (gram 0.001 => 1 g = 0.001 kg).
// ═══════════════════════════════════════════════════════════════════

#[test]
fn measure_as_converts_kg_to_gram() {
    let code = r#"
spec physics
data mass: measure
    -> unit kilogram: 1.0
    -> unit gram: 0.001
    -> suggest 2 kilogram

rule result: mass as gram as number
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let mut data = HashMap::new();
    data.insert("mass".to_string(), "2 kilogram".into());
    let response = engine
        .run(None, "physics", Some(&now), data, None, true)
        .unwrap();
    let rule = response
        .results
        .values()
        .find(|r| r.rule.name == "result")
        .unwrap();

    assert!(!rule.vetoed);
    let lit = rule
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("value");
    let amount = match &lit.value {
        ValueKind::Number(n) => n.clone(),
        other => panic!("expected Number from unit extraction, got {other:?}"),
    };

    assert_eq!(
        lemma::ValueKind::Number(amount)
            .as_decimal_magnitude()
            .unwrap(),
        Decimal::from(2000),
        "2 kg in gram = 2000"
    );
}

#[test]
fn measure_as_converts_gram_to_kg() {
    let code = r#"
spec physics
data mass: measure
    -> unit kilogram: 1.0
    -> unit gram: 0.001
    -> suggest 500 gram

rule result: mass as kilogram as number
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let mut data = HashMap::new();
    data.insert("mass".to_string(), "500 gram".into());
    let response = engine
        .run(None, "physics", Some(&now), data, None, true)
        .unwrap();
    let rule = response
        .results
        .values()
        .find(|r| r.rule.name == "result")
        .unwrap();

    assert!(!rule.vetoed);
    let lit = rule
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("value");
    let amount = match &lit.value {
        ValueKind::Number(n) => n.clone(),
        other => panic!("expected Number from unit extraction, got {other:?}"),
    };

    assert_eq!(
        lemma::ValueKind::Number(amount)
            .as_decimal_magnitude()
            .unwrap(),
        decimal_lit("0.5"),
        "500 gram in kilogram = 0.5"
    );
}

#[test]
fn measure_as_converts_pound_to_gram() {
    let code = r#"
spec physics
data mass: measure
    -> unit kilogram: 1.0
    -> unit gram: 0.001
    -> unit pound: 0.453592
    -> suggest 1 pound

rule result: mass as gram as number
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let mut data = HashMap::new();
    data.insert("mass".to_string(), "1 pound".into());
    let response = engine
        .run(None, "physics", Some(&now), data, None, true)
        .unwrap();
    let rule = response
        .results
        .values()
        .find(|r| r.rule.name == "result")
        .unwrap();

    assert!(!rule.vetoed);
    let lit = rule
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("value");
    let amount = match &lit.value {
        ValueKind::Number(n) => n.clone(),
        other => panic!("expected Number from unit extraction, got {other:?}"),
    };

    assert_eq!(
        lemma::ValueKind::Number(amount)
            .as_decimal_magnitude()
            .unwrap(),
        decimal_lit("453.592"),
        "1 pound in gram = 453.592"
    );
}

#[test]
fn measure_arithmetic_different_units_converts_correctly() {
    let code = r#"
spec physics
data mass: measure
    -> unit kilogram: 1.0
    -> unit gram: 0.001

data a: 1 kilogram
data b: 500 gram

rule total: a + b
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "physics", Some(&now), HashMap::new(), None, true)
        .unwrap();
    let rule = response
        .results
        .values()
        .find(|r| r.rule.name == "total")
        .unwrap();

    assert!(!rule.vetoed);
    let measure = rule
        .result
        .as_ref()
        .expect("rule result value")
        .measure
        .as_ref()
        .expect("measure map");
    assert_eq!(
        measure.get("kilogram").copied(),
        Some(Decimal::from_str("1.5").expect("1.5"))
    );
    assert_eq!(measure.get("gram").copied(), Some(Decimal::from(1500)));
}

#[test]
fn measure_comparison_different_units_physical() {
    let code = r#"
spec physics
data mass: measure
    -> unit kilogram: 1.0
    -> unit gram: 0.001

data package: 1500 gram
rule heavy: package > 1 kilogram
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "physics", Some(&now), HashMap::new(), None, true)
        .unwrap();
    let rule = response
        .results
        .values()
        .find(|r| r.rule.name == "heavy")
        .unwrap();

    assert_eq!(
        rule.result.as_ref().expect("rule result value").boolean,
        Some(true)
    );
}

// ═══════════════════════════════════════════════════════════════════
// Bug 2: validate_value_against_type compares raw measure value
// against base-unit bounds without converting. A value like
// "1500 gram" with maximum "2 kilogram" (stored as 2.0 base)
// is wrongly rejected because 1500 > 2.
// ═══════════════════════════════════════════════════════════════════

#[test]
fn measure_validation_respects_unit_for_maximum() {
    let code = r#"
spec physics
data mass: measure
    -> unit kilogram: 1.0
    -> unit gram: 0.001
    -> maximum 2 kilogram

data weight: mass

rule result: weight
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    // 1500 gram = 1.5 kg, which is under the 2 kg maximum
    let response = engine.run(
        None,
        "physics",
        Some(&now),
        HashMap::from([("weight".to_string(), "1500 gram".into())]),
        None,
        true,
    );

    assert!(
        response.is_ok(),
        "1500 gram (= 1.5 kg) should pass maximum 2 kg, got: {:?}",
        response.as_ref().err()
    );
    let response = response.expect("run Ok");
    let rule = response.results.get("result").expect("result");
    assert!(
        !rule.vetoed,
        "1500 gram under maximum 2 kg must not veto, got: {:?}",
        rule.veto_reason
    );
    assert_eq!(
        rule.result().expect("result"),
        "1500 gram",
        "display uses caller unit; family map still lists kilogram"
    );
    let measure = rule
        .result
        .as_ref()
        .expect("value")
        .measure
        .as_ref()
        .expect("measure map");
    assert_eq!(
        measure.get("gram").copied(),
        Some(Decimal::from(1500)),
        "accepted overlay magnitude must remain in measure.gram"
    );
}

#[test]
fn measure_validation_respects_unit_for_minimum() {
    let code = r#"
spec physics
data mass: measure
    -> unit kilogram: 1.0
    -> unit gram: 0.001
    -> minimum 1 kilogram

data weight: mass

rule result: weight
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    // 1500 gram = 1.5 kg, which is above the 1 kg minimum
    let response = engine.run(
        None,
        "physics",
        Some(&now),
        HashMap::from([("weight".to_string(), "1500 gram".into())]),
        None,
        true,
    );

    assert!(
        response.is_ok(),
        "1500 gram (= 1.5 kg) should pass minimum 1 kg, got: {:?}",
        response.as_ref().err()
    );
    let response = response.expect("run Ok");
    let rule = response.results.get("result").expect("result");
    assert!(
        !rule.vetoed,
        "1500 gram above minimum 1 kg must not veto, got: {:?}",
        rule.veto_reason
    );
    assert_eq!(
        rule.result().expect("result"),
        "1500 gram",
        "display uses caller unit; family map still lists kilogram"
    );
    let measure = rule
        .result
        .as_ref()
        .expect("value")
        .measure
        .as_ref()
        .expect("measure map");
    assert_eq!(
        measure.get("gram").copied(),
        Some(Decimal::from(1500)),
        "accepted overlay magnitude must remain in measure.gram"
    );
}

#[test]
fn measure_validation_rejects_over_maximum_in_non_base_unit() {
    let code = r#"
spec physics
data mass: measure
    -> unit kilogram: 1.0
    -> unit gram: 0.001
    -> maximum 2 kilogram

data weight: mass

rule result: weight
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    // 3000 gram = 3 kg, which exceeds the 2 kg maximum
    let message = run_override_veto_message(
        &engine,
        "physics",
        HashMap::from([("weight".to_string(), "3000 gram".into())]),
        "result",
    );
    assert!(
        message.contains("maximum") || message.contains("above"),
        "3000 gram (= 3 kg) should fail maximum 2 kg, got: {message}"
    );
    assert!(
        !message.to_lowercase().contains("canonical"),
        "must not mention canonical units, got: {message}"
    );
}

#[test]
fn measure_below_minimum_veto_uses_per_unit_not_canonical() {
    let code = r#"
spec s
data money: measure -> unit eur: 1 -> unit usd: 0.91
data mass: measure -> unit kilogram: 1
data cost_per_unit: measure
  -> unit eur_per_kilo: eur/kilogram
  -> minimum 1.20 eur_per_kilo
rule out: cost_per_unit
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let message = run_override_veto_message(
        &engine,
        "s",
        HashMap::from([("cost_per_unit".to_string(), "1 eur_per_kilo".into())]),
        "out",
    );
    assert!(
        message.contains("below minimum"),
        "expected below minimum, got: {message}"
    );
    assert!(
        message.contains("eur_per_kilo"),
        "expected user unit in message, got: {message}"
    );
    assert!(
        message.contains("1.2") || message.contains("1.20"),
        "expected per-unit minimum magnitude, got: {message}"
    );
    assert!(
        !message.to_lowercase().contains("canonical"),
        "must not mention canonical units, got: {message}"
    );
}

#[test]
fn measure_below_minimum_veto_respects_type_decimals() {
    let code = r#"
spec s
data money: measure -> unit eur: 1 -> unit usd: 0.91
data mass: measure -> unit kilogram: 1 -> unit tonne: 1000
data cost_per_unit: measure
  -> unit eur_per_kilo: eur/kilogram
  -> unit usd_per_tonne: usd/tonne
  -> minimum 1.20 eur_per_kilo
  -> decimals 2
rule out: cost_per_unit
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let message = run_override_veto_message(
        &engine,
        "s",
        HashMap::from([(
            "cost_per_unit".to_string(),
            "2.01 usd_per_tonne".to_string(),
        )]),
        "out",
    );
    assert!(
        message.contains("below minimum"),
        "expected below minimum, got: {message}"
    );
    assert!(
        message.contains("usd_per_tonne"),
        "expected user unit in message, got: {message}"
    );
    assert!(
        message.contains("1318.68"),
        "expected minimum bound rounded to 2 decimals, got: {message}"
    );
    assert!(
        !message.contains("1318.681"),
        "must not show extra decimal precision beyond type decimals, got: {message}"
    );
}

const COMPOUND_COST_VALIDATION_SPEC: &str = r#"
spec s
data money: measure -> unit eur: 1 -> unit usd: 0.91
data mass: measure -> unit kilogram: 1 -> unit tonne: 1000
data cost_per_unit: measure
  -> unit eur_per_kilo: eur/kilogram
  -> unit usd_per_tonne: usd/tonne
  -> maximum 2.00 eur_per_kilo
rule out: cost_per_unit
"#;

#[test]
fn compound_measure_below_maximum_in_other_unit_passes() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Volatile,
            COMPOUND_COST_VALIDATION_SPEC.to_string(),
        )])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine.run(
        None,
        "s",
        Some(&now),
        HashMap::from([(
            "cost_per_unit".to_string(),
            "2.01 usd_per_tonne".to_string(),
        )]),
        None,
        true,
    );

    assert!(
        response.is_ok(),
        "2.01 usd_per_tonne is below converted maximum of 2.00 eur_per_kilo, got: {:?}",
        response.as_ref().err()
    );
    let response = response.expect("run Ok");
    let rule = response.results.get("out").expect("out");
    assert!(
        !rule.vetoed,
        "2.01 usd_per_tonne under converted maximum must not veto, got: {:?}",
        rule.veto_reason
    );
    let display = rule.result().expect("result");
    assert!(
        display.contains("usd_per_tonne"),
        "display uses caller unit, got: {display}"
    );
    let measure = rule
        .result
        .as_ref()
        .expect("value")
        .measure
        .as_ref()
        .expect("measure map");
    assert_eq!(
        measure.get("usd_per_tonne").copied(),
        Some(Decimal::from_str("2.01").expect("2.01")),
        "overlay magnitude must remain in measure.usd_per_tonne"
    );
}

#[test]
fn compound_measure_above_maximum_shows_converted_bound_in_user_unit() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Volatile,
            COMPOUND_COST_VALIDATION_SPEC.to_string(),
        )])
        .unwrap();

    let message = run_override_veto_message(
        &engine,
        "s",
        HashMap::from([(
            "cost_per_unit".to_string(),
            "5000 usd_per_tonne".to_string(),
        )]),
        "out",
    );
    assert!(
        message.contains("above maximum"),
        "expected above maximum, got: {message}"
    );
    assert!(
        message.contains("usd_per_tonne"),
        "expected user unit in message, got: {message}"
    );
    assert!(
        !message.contains("2 usd_per_tonne"),
        "must not compare against unconverted maximum 2 usd_per_tonne, got: {message}"
    );
}

const TRI_COMPOUND_COST_VALIDATION_SPEC: &str = r#"
spec s
uses lemma units

data money: measure
  -> unit eur: 1
  -> unit usd: 0.91

data mass: measure
  -> unit kilogram: 1
  -> unit tonne: 1000

data storage_cost: measure
  -> unit eur_per_kilo_hour: eur/kilogram/hour
  -> unit usd_per_ton_hour: usd/tonne/hour
  -> maximum 2.00 eur_per_kilo_hour

rule out: storage_cost
"#;

#[test]
fn tri_compound_measure_below_maximum_in_other_unit_passes() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Volatile,
            TRI_COMPOUND_COST_VALIDATION_SPEC.to_string(),
        )])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine.run(
        None,
        "s",
        Some(&now),
        HashMap::from([(
            "storage_cost".to_string(),
            "2.01 usd_per_ton_hour".to_string(),
        )]),
        None,
        true,
    );

    assert!(
        response.is_ok(),
        "2.01 usd_per_ton_hour is below converted maximum of 2.00 eur_per_kilo_hour, got: {:?}",
        response.as_ref().err()
    );
    let response = response.expect("run Ok");
    let rule = response.results.get("out").expect("out");
    assert!(
        !rule.vetoed,
        "2.01 usd_per_ton_hour under converted maximum must not veto, got: {:?}",
        rule.veto_reason
    );
    let display = rule.result().expect("result");
    assert!(
        display.contains("usd_per_ton_hour"),
        "display uses caller unit, got: {display}"
    );
    let measure = rule
        .result
        .as_ref()
        .expect("value")
        .measure
        .as_ref()
        .expect("measure map");
    assert_eq!(
        measure.get("usd_per_ton_hour").copied(),
        Some(Decimal::from_str("2.01").expect("2.01")),
        "overlay magnitude must remain in measure.usd_per_ton_hour"
    );
}

#[test]
fn tri_compound_measure_above_maximum_shows_converted_bound_in_user_unit() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Volatile,
            TRI_COMPOUND_COST_VALIDATION_SPEC.to_string(),
        )])
        .unwrap();

    let message = run_override_veto_message(
        &engine,
        "s",
        HashMap::from([(
            "storage_cost".to_string(),
            "5000 usd_per_ton_hour".to_string(),
        )]),
        "out",
    );
    assert!(
        message.contains("above maximum"),
        "expected above maximum, got: {message}"
    );
    assert!(
        message.contains("usd_per_ton_hour"),
        "expected user unit in message, got: {message}"
    );
    assert!(
        !message.contains("2 usd_per_ton_hour"),
        "must not compare against unconverted maximum 2 usd_per_ton_hour, got: {message}"
    );
}

#[test]
fn measure_below_minimum_cross_unit_veto_message() {
    let code = r#"
spec physics
data mass: measure
    -> unit kilogram: 1.0
    -> unit gram: 0.001
    -> minimum 1 kilogram

data weight: mass

rule result: weight
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let message = run_override_veto_message(
        &engine,
        "physics",
        HashMap::from([("weight".to_string(), "500 gram".into())]),
        "result",
    );
    assert!(
        message.contains("500 gram is below minimum 1000 gram"),
        "expected cross-unit bound in user's unit, got: {message}"
    );
    assert!(
        !message.to_lowercase().contains("canonical"),
        "must not mention canonical units, got: {message}"
    );
}

// --- precision stress: prime unit chains from seed 37 ---

fn reference_named_unit_conversion(
    stored: Decimal,
    from_factor: Decimal,
    to_factor: Decimal,
) -> Decimal {
    let magnitude = decimal_to_rational(stored).expect("reference stored magnitude must lift");
    let from = decimal_to_rational(from_factor).expect("reference from factor must lift");
    let to = decimal_to_rational(to_factor).expect("reference to factor must lift");
    let canonical = checked_mul(&magnitude, &from).expect("reference multiply");
    let in_unit = checked_div(&canonical, &to).expect("reference divide");
    in_unit
        .try_to_decimal()
        .expect("reference conversion must convert to decimal")
}

fn eval_rule_measure_magnitude(
    engine: &Engine,
    spec_name: &str,
    rule_name: &str,
) -> (Decimal, String) {
    let now = DateTimeValue::now();
    let response = engine
        .run(None, spec_name, Some(&now), HashMap::new(), None, true)
        .expect("precision stress evaluation must complete");
    let rule = response
        .results
        .values()
        .find(|r| r.rule.name == rule_name)
        .unwrap_or_else(|| panic!("rule '{rule_name}' missing in spec '{spec_name}'"));
    assert!(
        !rule.vetoed,
        "rule '{rule_name}' must not Veto, got {:?}",
        rule.veto_reason
    );
    if let Some(measure) = rule.result.as_ref().and_then(|v| v.measure.as_ref()) {
        let unit = rule
            .rule
            .rule_type
            .measure_binding_unit
            .clone()
            .filter(|u| measure.contains_key(u))
            .or_else(|| {
                rule.result().and_then(|display| {
                    let lower = display.to_lowercase();
                    measure
                        .keys()
                        .find(|u| lower.contains(&u.to_lowercase()))
                        .cloned()
                })
            })
            .or_else(|| measure.keys().next().cloned())
            .unwrap_or_else(|| panic!("measure map empty for '{rule_name}'"));
        let amount = *measure
            .get(&unit)
            .unwrap_or_else(|| panic!("measure map missing unit '{unit}'"));
        return (amount, unit);
    }
    let lit = rule
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("value");
    match &lit.value {
        ValueKind::Measure(amount) => (
            lemma::ValueKind::Number(amount.clone())
                .as_decimal_magnitude()
                .unwrap(),
            String::new(),
        ),
        other => panic!("expected Measure from '{rule_name}', got {other:?}"),
    }
}

fn prime_precision_spec_header(primes: &[u32]) -> String {
    let mut spec = String::from(
        "spec prime_precision\n\
         data seed: measure\n    -> unit base: 1\n",
    );
    for prime in primes {
        spec.push_str(&format!("    -> unit p{prime} {prime}\n"));
    }
    spec.push_str("data anchor: 37 base\nrule step0: anchor\n");
    spec
}

#[test]
fn precision_baseline_roundtrip_base_p2_base() {
    let code = r#"
spec prime_precision
data seed: measure
    -> unit base: 1
    -> unit p2: 2
data anchor: 37 base
rule step0: anchor
rule step1: step0 as p2
rule step2: step1 as base
"#;
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let (amount, unit) = eval_rule_measure_magnitude(&engine, "prime_precision", "step2");
    assert_eq!(unit, "base");
    assert_eq!(amount, Decimal::from(37));
}

#[test]
fn precision_short_prime_chain_matches_stepwise_reference() {
    let primes: &[u32] = &[2, 3, 5];
    let mut code = prime_precision_spec_header(primes);
    let mut last = String::from("step0");
    let mut step_index = 0;
    for prime in primes {
        step_index += 1;
        let rule = format!("step{step_index}");
        code.push_str(&format!("rule {rule}: {last} as p{prime}\n"));
        last = rule;
    }
    let units_reverse: Vec<u32> = primes.iter().rev().copied().collect();
    for index in 0..units_reverse.len() - 1 {
        step_index += 1;
        let rule = format!("step{step_index}");
        code.push_str(&format!(
            "rule {rule}: {last} as p{}\n",
            units_reverse[index + 1]
        ));
        last = rule;
    }
    step_index += 1;
    let final_rule = format!("step{step_index}");
    code.push_str(&format!("rule {final_rule}: {last} as base\n"));

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, &code.to_string())])
        .unwrap();

    let (lemma_amount, unit) = eval_rule_measure_magnitude(&engine, "prime_precision", &final_rule);
    assert_eq!(unit, "base");
    assert_eq!(
        lemma_amount,
        Decimal::from(37),
        "37 base through prime unit chain forward and back must stay exactly 37"
    );
}

#[test]
fn precision_prime_ladder_forward_reverse_matches_reference() {
    let primes: &[u32] = &[2, 3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 41, 43, 47];
    let mut code = prime_precision_spec_header(primes);
    let mut last = String::from("step0");
    let mut step_index = 0;
    for prime in primes {
        step_index += 1;
        let rule = format!("step{step_index}");
        code.push_str(&format!("rule {rule}: {last} as p{prime}\n"));
        last = rule;
    }
    let units_reverse: Vec<u32> = primes.iter().rev().copied().collect();
    for index in 0..units_reverse.len() - 1 {
        step_index += 1;
        let rule = format!("step{step_index}");
        code.push_str(&format!(
            "rule {rule}: {last} as p{}\n",
            units_reverse[index + 1]
        ));
        last = rule;
    }
    step_index += 1;
    let final_rule = format!("step{step_index}");
    code.push_str(&format!("rule {final_rule}: {last} as base\n"));

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, &code.to_string())])
        .unwrap();

    let (lemma_amount, unit) = eval_rule_measure_magnitude(&engine, "prime_precision", &final_rule);
    assert_eq!(unit, "base");
    assert_eq!(
        lemma_amount,
        Decimal::from(37),
        "37 base through fourteen prime units forward and back must stay exactly 37"
    );
}

fn prime_cycles_lemma(cycle_count: usize) -> String {
    let cycle: &[u32] = &[2, 3, 5, 7, 11];
    let mut code = String::from(
        "spec prime_cycles\n\
         data seed: measure\n    -> unit base: 1\n",
    );
    for prime in cycle {
        code.push_str(&format!("    -> unit p{prime} {prime}\n"));
    }
    code.push_str("data anchor: 37 base\nrule step0: anchor\n");
    let mut previous = String::from("step0");
    let mut step_number = 0;
    for _cycle_index in 0..cycle_count {
        for prime in cycle {
            step_number += 1;
            let rule = format!("step{step_number}");
            code.push_str(&format!("rule {rule}: {previous} as p{prime}\n"));
            previous = rule;
        }
    }
    code.push_str(&format!("rule final_step: {previous} as base\n"));
    code
}

#[test]
fn precision_many_prime_cycles_deterministic() {
    let code = prime_cycles_lemma(20);

    let mut engine = Engine::new();
    engine.load([(lemma::SourceType::Volatile, &code)]).unwrap();

    let (first, _) = eval_rule_measure_magnitude(&engine, "prime_cycles", "final_step");
    let (second, _) = eval_rule_measure_magnitude(&engine, "prime_cycles", "final_step");
    assert_eq!(first, second, "repeated evaluation must be deterministic");
    assert!(first > Decimal::ZERO);
}

/// Cold on-demand explain of the tip only — must not recurse O(hops) on the Rust stack.
#[test]
fn precision_many_prime_cycles_explain_tip_only() {
    let code = prime_cycles_lemma(20);

    let mut engine = Engine::new();
    engine.load([(lemma::SourceType::Volatile, &code)]).unwrap();

    let now = DateTimeValue::now();
    let rules = ["final_step".to_string()];
    let response = engine
        .run(
            None,
            "prime_cycles",
            Some(&now),
            HashMap::new(),
            Some(&rules),
            true,
        )
        .expect("tip-only explain of 100-hop chain must complete");

    assert_eq!(response.results.len(), 1);
    let final_step = response.results.get("final_step").expect("final_step");
    assert!(!final_step.vetoed, "final_step must not veto");
    assert!(
        final_step.explanation.is_some(),
        "tip-only run with explain must attach explanation"
    );
    let again = engine
        .run(
            None,
            "prime_cycles",
            Some(&now),
            HashMap::new(),
            Some(&rules),
            true,
        )
        .expect("second tip-only explain must complete");
    let first_display = final_step.result().map(str::to_string);
    let second_display = again
        .results
        .get("final_step")
        .expect("final_step")
        .result()
        .map(str::to_string);
    assert_eq!(
        first_display, second_display,
        "repeated tip-only explain must be deterministic"
    );
}

#[test]
fn precision_ping_pong_p7_p11_forty_hops() {
    let mut code = String::from(
        "spec ping_pong\n\
         data seed: measure\n    -> unit base: 1\n    -> unit p7: 7\n    -> unit p11: 11\n\
         data anchor: 37 base\nrule step0: anchor\n",
    );
    let mut previous = String::from("step0");
    for index in 0..40 {
        let unit = if index % 2 == 0 { "p7" } else { "p11" };
        let rule = format!("step{}", index + 1);
        code.push_str(&format!("rule {rule}: {previous} as {unit}\n"));
        previous = rule;
    }
    code.push_str(&format!("rule final_step: {previous} as base\n"));

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, &code.to_string())])
        .unwrap();
    let (amount, unit) = eval_rule_measure_magnitude(&engine, "ping_pong", "final_step");
    assert_eq!(unit, "base");
    assert!(amount > Decimal::ZERO);
}

#[test]
fn precision_mixed_arithmetic_conversion_completes() {
    let code = r#"
spec mixed_arith
data seed: measure
    -> unit base: 1
    -> unit p3: 3
    -> unit p7: 7
data anchor: 37 base
data five: 5
data two: 2
rule step0: anchor
rule step1: step0 as p3
rule step2: step1 * five
rule step3: step2 as p7
rule step4: step3 / two
rule step5: step4 as base
"#;
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let reference = {
        let mut canonical = decimal_to_rational(Decimal::from(37)).expect("lift anchor");
        canonical = checked_mul(&canonical, &rational_new(5, 1)).expect("multiply by five");
        canonical = checked_div(&canonical, &rational_new(2, 1)).expect("divide by two");
        canonical
            .try_to_decimal()
            .expect("reference must convert to decimal")
    };

    let (lemma_amount, unit) = eval_rule_measure_magnitude(&engine, "mixed_arith", "step5");
    assert_eq!(unit, "base");
    assert_eq!(lemma_amount, reference);
}

#[test]
fn precision_api_display_ping_pong_twenty_unit_toggles() {
    let mut code = String::from(
        "spec api_ping_pong\n\
         data seed: measure\n    -> unit base: 1\n    -> unit p7: 7\n    -> unit p11: 11\n\
         data anchor: 37 base\nrule step0: anchor\n",
    );
    let mut previous = String::from("step0");
    for index in 0..20 {
        let unit = if index % 2 == 0 { "p7" } else { "p11" };
        let rule = format!("step{}", index + 1);
        code.push_str(&format!("rule {rule}: {previous} as {unit}\n"));
        previous = rule;
    }

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, &code.to_string())])
        .unwrap();

    let (in_spec_amount, in_spec_unit) =
        eval_rule_measure_magnitude(&engine, "api_ping_pong", &previous);
    let in_spec_factor = match in_spec_unit.as_str() {
        "p7" => Decimal::from(7),
        "p11" => Decimal::from(11),
        other => panic!("unexpected in-spec unit {other}"),
    };
    assert_eq!(
        in_spec_amount,
        reference_named_unit_conversion(in_spec_amount, in_spec_factor, in_spec_factor),
        "step20 in-spec display must match canonical RuleResultValue decimal in its own unit"
    );

    for index in 0..20 {
        let rule = format!("step{}", index + 1);
        let target_unit = if index % 2 == 0 { "p7" } else { "p11" };
        let target_factor = if index % 2 == 0 {
            Decimal::from(7)
        } else {
            Decimal::from(11)
        };
        let (amount, unit) = eval_rule_measure_magnitude(&engine, "api_ping_pong", &rule);
        assert_eq!(unit, target_unit);
        let expected =
            reference_named_unit_conversion(Decimal::from(37), Decimal::ONE, target_factor);
        assert_eq!(
            amount, expected,
            "in-spec conversion at hop {index} must match reference"
        );
    }
}

#[test]
fn precision_division_by_zero_control_vetoes() {
    let code = r#"
spec div0_control
data ten: 10
data zero: 0
rule quotient: ten / zero
"#;
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let now = DateTimeValue::now();
    let response = engine
        .run(None, "div0_control", Some(&now), HashMap::new(), None, true)
        .expect("evaluation must complete");
    let rule = response
        .results
        .values()
        .find(|r| r.rule.name == "quotient")
        .expect("quotient missing");
    assert!(rule.vetoed, "division by zero path must Veto, not panic");
    let reason = rule.veto_reason.as_deref().expect("veto reason");
    assert!(
        reason.contains("Division by zero") || reason.contains("division"),
        "expected division veto, got: {reason}"
    );
}

#[test]
fn shadowed_duration_rejects_minutes_alias_at_load() {
    let code = r#"
spec s
uses lemma units
data duration: measure
  -> unit minute: 60
rule r: 5 as minutes
"#;

    let mut engine = Engine::new();
    let err = engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect_err("shadowed duration must reject minutes alias at load");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("minutes"),
        "expected unknown minutes at load, got: {msg}"
    );
}

#[test]
fn stdlib_duration_minute_loads_and_runs() {
    let code = r#"
spec t
uses lemma units
data duration: units.duration
data x: number -> suggest 5
rule r: x as minute
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let now = DateTimeValue::now();
    let mut data = HashMap::new();
    data.insert("x".to_string(), "5".into());
    let response = engine
        .run(None, "t", Some(&now), data, None, true)
        .expect("stdlib duration with minute must run");
    let display = response
        .results
        .get("r")
        .expect("rule r")
        .result()
        .expect("result");
    assert_eq!(display, "5 minute");
}

// ═══════════════════════════════════════════════════════════════════
// Cross-family relabel: `5 eur as kg` = 5 kg (no factor).
// `amount as eur as kg` converts in the money family first, then relabels.
// ═══════════════════════════════════════════════════════════════════

#[test]
fn cross_family_relabel_measure_literal_no_factor() {
    // `5 eur as kg` — magnitude carried unchanged, target family is mass.
    let code = r#"
spec t
data money: measure -> unit eur: 1 -> unit usd: 0.91
data mass: measure -> unit kg: 1
rule out: 5 eur as kg
"#;
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let now = DateTimeValue::now();
    let response = engine
        .run(None, "t", Some(&now), HashMap::new(), None, true)
        .unwrap();
    let out = response.results.get("out").unwrap();
    let lit = out
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("value");
    let (magnitude, unit) = match &lit.value {
        ValueKind::Measure(m) => (
            lemma::ValueKind::Number(m.clone())
                .as_decimal_magnitude()
                .unwrap(),
            out.result
                .as_ref()
                .and_then(|v| v.measure.as_ref())
                .and_then(|m| m.keys().next().cloned()),
        ),
        other => panic!("expected Measure, got {other:?}"),
    };
    assert_eq!(
        magnitude,
        Decimal::from(5),
        "5 eur as kg must carry magnitude 5 unchanged"
    );
    assert_eq!(unit.as_deref(), Some("kg"), "target unit must be kg");
    assert!(
        out.result
            .as_ref()
            .and_then(|v| v.measure.as_ref())
            .is_some(),
        "result must be a measure type"
    );
}

#[test]
fn cross_family_relabel_via_convert_then_relabel() {
    // `amount as eur as kg`: 100 usd → 91 eur (money factor), then relabel → 91 kg (no factor).
    let code = r#"
spec t
data money: measure -> unit eur: 1 -> unit usd: 0.91
data mass: measure -> unit kg: 1
data amount: 100 usd
rule out: amount as eur as kg
"#;
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let now = DateTimeValue::now();
    let response = engine
        .run(None, "t", Some(&now), HashMap::new(), None, true)
        .unwrap();
    let out = response.results.get("out").unwrap();
    let lit = out
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("value");
    let (magnitude, unit) = match &lit.value {
        ValueKind::Measure(m) => (
            lemma::ValueKind::Number(m.clone())
                .as_decimal_magnitude()
                .unwrap(),
            out.result
                .as_ref()
                .and_then(|v| v.measure.as_ref())
                .and_then(|m| m.keys().next().cloned()),
        ),
        other => panic!("expected Measure, got {other:?}"),
    };
    // 100 usd × 0.91 = 91 eur (money family factor), then relabel to kg with the same magnitude.
    assert_eq!(magnitude, Decimal::new(91, 0), "100 usd → 91 eur → 91 kg");
    assert_eq!(unit.as_deref(), Some("kg"), "target unit must be kg");
}

#[test]
fn cross_family_extract_then_relabel_number_bridge() {
    // `amount as eur as number as kg`: strip to 91, then construct 91 kg.
    let code = r#"
spec t
data money: measure -> unit eur: 1 -> unit usd: 0.91
data mass: measure -> unit kg: 1
data amount: 100 usd
rule out: amount as eur as number as kg
"#;
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let now = DateTimeValue::now();
    let response = engine
        .run(None, "t", Some(&now), HashMap::new(), None, true)
        .unwrap();
    let out = response.results.get("out").unwrap();
    let lit = out
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("value");
    let (magnitude, unit) = match &lit.value {
        ValueKind::Measure(m) => (
            lemma::ValueKind::Number(m.clone())
                .as_decimal_magnitude()
                .unwrap(),
            out.result
                .as_ref()
                .and_then(|v| v.measure.as_ref())
                .and_then(|m| m.keys().next().cloned()),
        ),
        other => panic!("expected Measure, got {other:?}"),
    };
    assert_eq!(
        magnitude,
        Decimal::new(91, 0),
        "91 eur → 91 (number) → 91 kg"
    );
    assert_eq!(unit.as_deref(), Some("kg"));
}

// ═══════════════════════════════════════════════════════════════════
// Rejections: bare measure `as number` / bare cross-family `as <unit>`
// require a source unit to be named first.
// ═══════════════════════════════════════════════════════════════════

#[test]
fn bare_measure_reference_as_number_rejected() {
    // `amount as number` without naming a unit must be rejected with a suggestion.
    let code = r#"
spec t
data money: measure -> unit eur: 1 -> unit usd: 0.91
data amount: money
rule out: amount as number
"#;
    let mut engine = Engine::new();
    let err = engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap_err();
    let msg = err
        .errors
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("; ");
    assert!(
        msg.contains("eur") || msg.contains("as eur"),
        "error must suggest explicit unit, got: {msg}"
    );
    assert!(
        msg.to_lowercase().contains("number"),
        "error must mention 'number', got: {msg}"
    );
}

#[test]
fn bare_measure_reference_cross_family_as_unit_rejected() {
    // `amount as kg` without naming source unit must be rejected.
    let code = r#"
spec t
data money: measure -> unit eur: 1
data mass: measure -> unit kg: 1
data amount: money
rule out: amount as kg
"#;
    let mut engine = Engine::new();
    let err = engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap_err();
    let msg = err
        .errors
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("; ");
    assert!(
        msg.contains("eur") || msg.to_lowercase().contains("unit"),
        "error must suggest expressing source unit first, got: {msg}"
    );
}

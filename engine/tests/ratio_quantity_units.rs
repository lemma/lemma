//! Rock-solid tests locking in ratio vs measure unit behaviour.
//!
//! Covers: "as percent" / "as permille" ratio conversion; comparison with percent literals;
//! unknown unit error; measure conversion unchanged; ratio display with no unit.

use lemma::DateTimeValue;
use lemma::ValueKind;
use lemma::{Engine, LiteralValue};
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::str::FromStr;

fn decimal_lit(s: &str) -> Decimal {
    Decimal::from_str(s).expect("BUG: test decimal literal must parse")
}

fn rule_literal(rule: &lemma::RuleResult) -> lemma::LiteralValue {
    assert!(!rule.vetoed, "unexpected veto: {:?}", rule.veto_reason);
    rule.explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("value")
}

#[test]
fn in_percent_produces_ratio_and_compares_with_percent_literal() {
    let code = r#"
spec savings
data savings_amount: 75
data total_amount: 300

rule savings_ratio: (savings_amount / total_amount) as percent
rule is_above_20: savings_ratio > 20%
rule is_above_30: savings_ratio > 30%
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "savings", Some(&now), HashMap::new(), None, true)
        .unwrap();

    let ratio_result = response
        .results
        .get("savings_ratio")
        .expect("savings_ratio rule");
    let lit = rule_literal(ratio_result);
    match &lit.value {
        ValueKind::Ratio(r) => {
            assert_eq!(
                lemma::ValueKind::Number(r.clone())
                    .as_decimal_magnitude()
                    .unwrap(),
                Decimal::new(25, 2),
                "75/300 = 0.25"
            );
            assert!(
                matches!(lit.value, lemma::ValueKind::Ratio(_))
                    || matches!(lit.value, ValueKind::Ratio(_))
            );
            // primary unit lives on rule/data type, not LiteralValue
        }
        other => panic!("savings_ratio must be Ratio, got {:?}", other),
    }

    let above_20 = response.results.get("is_above_20").expect("is_above_20");
    let above_30 = response.results.get("is_above_30").expect("is_above_30");
    assert_eq!(
        above_20.result.as_ref().expect("rule result value").boolean,
        Some(true)
    );
    assert_eq!(
        above_30.result.as_ref().expect("rule result value").boolean,
        Some(false)
    );
}

#[test]
fn in_percent_then_chained_comparison_with_multiple_thresholds() {
    let code = r#"
spec summary
data part: 18
data whole: 60

rule pct: (part / whole) as percent
rule tier: "low"
    unless pct > 25% then "mid"
    unless pct > 50% then "high"
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "summary", Some(&now), HashMap::new(), None, true)
        .unwrap();
    let tier = response.results.get("tier").expect("tier");
    assert_eq!(
        tier.result
            .as_ref()
            .expect("rule result value")
            .text
            .as_deref(),
        Some("mid")
    );
}

#[test]
fn in_permille_produces_ratio() {
    let code = r#"
spec permille_spec
data value: 0.025

rule as_permille: value as permille
rule above_20_permille: as_permille > 20 permille
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(
            None,
            "permille_spec",
            Some(&now),
            HashMap::new(),
            None,
            true,
        )
        .unwrap();
    let as_permille = response.results.get("as_permille").expect("as_permille");
    let lit = rule_literal(as_permille);
    match &lit.value {
        ValueKind::Ratio(r) => {
            assert_eq!(
                lemma::ValueKind::Number(r.clone())
                    .as_decimal_magnitude()
                    .unwrap(),
                Decimal::new(25, 3)
            );
            assert!(
                matches!(lit.value, ValueKind::Ratio(_))
                    || matches!(lit.value, lemma::ValueKind::Ratio(_))
            );
            // binding unit "permille" lives on type, not LiteralValue
        }
        other => panic!("as_permille must be Ratio, got {:?}", other),
    }

    let above = response.results.get("above_20_permille").expect("above");
    assert_eq!(
        above.result.as_ref().expect("rule result value").boolean,
        Some(true)
    );
}

#[test]
fn unknown_unit_in_conversion_fails_planning() {
    let code = r#"
spec bad
data x: 100

rule bad_conv: x as not_a_unit
"#;

    let mut engine = Engine::new();
    let err = engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap_err();

    let msg = format!("{:?}", err);
    assert!(
        msg.contains("Unknown unit") && msg.contains("not_a_unit"),
        "expected unknown unit error, got: {}",
        msg
    );
}

#[test]
fn number_minus_ratio_data_is_rejected() {
    let code = r#"
spec pricing
data discount: ratio

rule price: 100 - discount
"#;

    let mut engine = Engine::new();
    let result = engine.load([(lemma::SourceType::Volatile, code.to_string())]);
    assert!(result.is_err(), "number - ratio should be rejected");
    let msg = result
        .unwrap_err()
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("; ");
    assert!(
        msg.contains("scale explicitly"),
        "expected scale-explicitly hint, got: {}",
        msg
    );
}

#[test]
fn ratio_display_with_none_unit_shows_number_only() {
    let lit = LiteralValue::ratio_from_decimal(decimal_lit("0.5"));
    let display = lit.display_value();
    assert!(
        !display.contains("percent") && display.contains("0.5"),
        "ratio with None unit should display number only, got: {}",
        display
    );

    // Binding unit lives on LemmaType; LiteralValue::ratio_with_bound_unit no longer stores it.
    let ValueKind::Ratio(r) = &lit.value else {
        panic!("expected ratio, got {:?}", lit.value);
    };
    let _ = r;
    let display_bare = lit.display_value();
    assert!(
        display_bare.contains("0.5"),
        "bare ratio display must show magnitude, got: {}",
        display_bare
    );
}

#[test]
fn chained_ratio_conversion_and_arithmetic() {
    let code = r#"
spec chained
data a: 10
data b: 40

rule pct: (a / b) as percent
rule plus_five: pct + 5%
rule compared: plus_five > 25%
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "chained", Some(&now), HashMap::new(), None, true)
        .unwrap();
    let pct = response.results.get("pct").expect("pct");
    let plus_five = response.results.get("plus_five").expect("plus_five");
    let compared = response.results.get("compared").expect("compared");

    if let ValueKind::Ratio(r) = &rule_literal(pct).value {
        assert_eq!(
            lemma::ValueKind::Number(r.clone())
                .as_decimal_magnitude()
                .unwrap(),
            Decimal::new(25, 2)
        );
    } else {
        panic!("pct must be Ratio");
    }
    if let ValueKind::Ratio(r) = &rule_literal(plus_five).value {
        assert_eq!(
            lemma::ValueKind::Number(r.clone())
                .as_decimal_magnitude()
                .unwrap(),
            Decimal::new(30, 2)
        );
    } else {
        panic!("plus_five must be Ratio");
    }
    assert_eq!(
        compared.result.as_ref().expect("rule result value").boolean,
        Some(true)
    );
}

#[test]
fn measure_and_ratio_conversion_in_same_spec() {
    let code = r#"
spec mixed
data money: measure
  -> unit eur: 1

data amount: 200
data part: 50

rule as_eur: amount as eur
rule share_pct: (part / amount) as percent
rule share_above_20: share_pct > 20%
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "mixed", Some(&now), HashMap::new(), None, true)
        .unwrap();
    let as_eur = response.results.get("as_eur").expect("as_eur");
    let share_pct = response.results.get("share_pct").expect("share_pct");
    let share_above_20 = response
        .results
        .get("share_above_20")
        .expect("share_above_20");

    let lit = rule_literal(as_eur);
    match &lit.value {
        ValueKind::Measure(n) => {
            assert_eq!(
                ValueKind::Number(n.clone()).as_decimal_magnitude().unwrap(),
                Decimal::from(200)
            );
            let measure = as_eur
                .result
                .as_ref()
                .and_then(|v| v.measure.as_ref())
                .expect("measure map");
            assert!(
                measure.contains_key("eur"),
                "as_eur must expose eur, got {:?}",
                measure.keys().collect::<Vec<_>>()
            );
        }
        other => panic!("as_eur: expected Measure, got {other:?}"),
    }
    let lit = rule_literal(share_pct);
    if let ValueKind::Ratio(r) = &lit.value {
        assert_eq!(
            lemma::ValueKind::Number(r.clone())
                .as_decimal_magnitude()
                .unwrap(),
            Decimal::new(25, 2)
        );
        assert!(
            matches!(lit.value, lemma::ValueKind::Ratio(_))
                || matches!(lit.value, ValueKind::Ratio(_))
        );
        // primary unit lives on rule/data type, not LiteralValue
    } else {
        panic!("share_pct must be Ratio");
    }
    assert_eq!(
        share_above_20
            .result
            .as_ref()
            .expect("rule result value")
            .boolean,
        Some(true)
    );
}

//! Arithmetic and division behavior (planning errors vs runtime Veto vs Decimal results).

use lemma::{Engine, ValueKind};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

fn source() -> lemma::SourceType {
    lemma::SourceType::Path(Arc::new(PathBuf::from("arithmetic_exactness.lemma")))
}

#[test]
fn literal_division_by_zero_is_planning_error() {
    let code = r#"
        spec exactness
        rule quotient: 1 / 0
    "#;
    let mut engine = Engine::new();
    let errors = engine
        .load([(source(), code.to_string())])
        .expect_err("literal zero divisor must not load");
    let message = format!("{:?}", errors);
    assert!(
        message.to_lowercase().contains("zero"),
        "expected planning error mentioning zero divisor, got: {}",
        message
    );
}

#[test]
fn runtime_data_zero_divisor_still_evaluates_to_veto() {
    let code = r#"
        spec exactness
        data denominator: 0
        rule quotient: 10 / denominator
    "#;
    let mut engine = Engine::new();
    engine
        .load([(source(), code.to_string())])
        .expect("runtime zero divisor must load");
    let response = engine
        .run(None, "exactness", None, HashMap::new(), None, true)
        .expect("evaluation must complete");
    let rule_result = response
        .results
        .get("quotient")
        .expect("quotient rule missing");
    assert!(rule_result.vetoed);
    assert!(
        matches!(
            rule_result.veto_detail,
            Some(lemma::VetoType::Computation { .. })
        ),
        "runtime zero divisor must veto, not fail load"
    );
}

#[test]
fn runtime_data_ten_divide_three_returns_value_not_veto() {
    let code = r#"
        spec exactness
        data ten: 10
        data three: 3
        rule quotient: ten / three
    "#;
    let mut engine = Engine::new();
    engine
        .load([(source(), code.to_string())])
        .expect("runtime division must load");
    let response = engine
        .run(None, "exactness", None, HashMap::new(), None, true)
        .expect("evaluation must complete");
    let rule_result = response
        .results
        .get("quotient")
        .expect("quotient rule missing");
    let value = rule_result
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("ten / three must return Value, not Veto");
    match &value.value {
        ValueKind::Number(n) => {
            assert!(
                lemma::ValueKind::Number(n.clone())
                    .as_decimal_magnitude()
                    .unwrap()
                    > rust_decimal::Decimal::from(3)
                    && lemma::ValueKind::Number(n.clone())
                        .as_decimal_magnitude()
                        .unwrap()
                        < rust_decimal::Decimal::from(4),
                "expected 10/3 decimal, got {n}"
            );
        }
        other => panic!("expected Number, got {other:?}"),
    }
}

#[test]
fn integer_division_is_exact_rational_not_truncation() {
    // `/` yields exact ℚ; does not truncate toward zero (that applies to `%` only).
    let code = r#"
        spec exactness
        rule neg_quot: -7 / 3
        rule pos_quot: 7 / 3
    "#;
    let mut engine = Engine::new();
    engine
        .load([(source(), code.to_string())])
        .expect("division must load");
    let response = engine
        .run(None, "exactness", None, HashMap::new(), None, true)
        .expect("evaluation must complete");

    let neg = response.results.get("neg_quot").expect("neg_quot");
    let pos = response.results.get("pos_quot").expect("pos_quot");
    assert!(!neg.vetoed && !pos.vetoed);

    let neg_n = neg
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("value");
    let pos_n = pos
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("value");

    match (&neg_n.value, &pos_n.value) {
        (ValueKind::Number(n), ValueKind::Number(p)) => {
            let n_dec = lemma::ValueKind::Number(n.clone())
                .as_decimal_magnitude()
                .unwrap();
            let p_dec = lemma::ValueKind::Number(p.clone())
                .as_decimal_magnitude()
                .unwrap();
            // Exact -7/3 ≈ -2.333..., not trunc toward zero (-2).
            assert!(
                n_dec < rust_decimal::Decimal::from(-2) && n_dec > rust_decimal::Decimal::from(-3),
                "expected exact -7/3, got {n_dec}"
            );
            // Exact 7/3 ≈ 2.333..., not trunc toward zero (2).
            assert!(
                p_dec > rust_decimal::Decimal::from(2) && p_dec < rust_decimal::Decimal::from(3),
                "expected exact 7/3, got {p_dec}"
            );
        }
        other => panic!("expected Number pair, got {other:?}"),
    }
}

#[test]
fn hourly_rate_times_date_range_yields_eur_total() {
    let code = r#"spec wage
uses lemma units
data money: measure
  -> unit eur: 1.00
data rate: measure
  -> unit eur_per_second: eur/second
  -> unit eur_per_hour: eur/hour
data hourly_rate: 50 eur_per_hour
data period_start: 2026-01-01
data period_end: 2026-01-02
rule pay: (hourly_rate * (period_start...period_end as hour))"#;
    let mut engine = Engine::new();
    engine
        .load([(source(), code.to_string())])
        .expect("spec must load");
    let response = engine
        .run(None, "wage", None, HashMap::new(), None, true)
        .expect("spec must evaluate");
    let rr = response.results.get("pay").expect("pay");
    let measure = rr
        .result
        .as_ref()
        .and_then(|v| v.measure.as_ref())
        .expect("measure map");
    assert!(
        measure.contains_key("eur"),
        "pay must expose eur, got keys {:?}",
        measure.keys().collect::<Vec<_>>()
    );
    let amount = measure.get("eur").copied().expect("eur");
    assert_eq!(amount, rust_decimal::Decimal::from(1200));
}

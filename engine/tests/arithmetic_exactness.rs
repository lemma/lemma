//! Arithmetic and division behavior (planning errors vs runtime Veto vs Decimal results).

use indexmap::IndexMap;
use lemma::{Engine, ValueKind};
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
fn literal_zero_to_negative_power_is_planning_error() {
    let code = r#"
        spec exactness
        rule inverse_of_zero: 0 ^ (-1)
    "#;
    let mut engine = Engine::new();
    let errors = engine
        .load([(source(), code.to_string())])
        .expect_err("literal zero base with negative exponent must not load");
    let message = format!("{:?}", errors);
    assert!(
        message.to_lowercase().contains("zero"),
        "expected planning error mentioning zero, got: {}",
        message
    );
}

fn evaluate_exactness(code: &str, data: IndexMap<String, String>) -> lemma::Response {
    let mut engine = Engine::new();
    engine
        .load([(source(), code.to_string())])
        .expect("spec must load");
    let data = data
        .into_iter()
        .map(|(name, value)| (name, value.into()))
        .collect();
    engine
        .evaluate(None, "exactness", None, data, None, true)
        .expect("evaluation must complete")
}

fn rule_display(response: &lemma::Response, rule: &str) -> String {
    let rule_result = response.results.get(rule).expect("rule missing");
    assert!(
        !rule_result.vetoed,
        "{rule} vetoed: {:?}",
        rule_result.veto_reason
    );
    rule_result
        .result()
        .map(|value| value.to_string())
        .expect("rule value")
}

#[test]
fn rationals_with_magnitude_at_two_pow_127_keep_their_sign() {
    let code = r#"
        spec exactness
        data two: number
        rule constant_positive: 2 ^ 127 > 0
        rule constant_quotient: 2 ^ 32767 / 2 ^ 32766
        rule constant_negative: 0 - 2 ^ 127 < 0
        rule data_positive: two ^ 127 > 0
        rule data_quotient: two ^ 32767 / two ^ 32766
        rule data_negative: 0 - two ^ 127 < 0
        rule data_all_ones_exponents: two ^ 255 > 0 and two ^ 1023 > 0 and two ^ 4095 > 0
    "#;
    let response = evaluate_exactness(code, IndexMap::from([("two".to_string(), "2".to_string())]));
    for rule in [
        "constant_positive",
        "constant_negative",
        "data_positive",
        "data_negative",
        "data_all_ones_exponents",
    ] {
        assert_eq!(rule_display(&response, rule), "true", "{rule}");
    }
    assert_eq!(rule_display(&response, "constant_quotient"), "2");
    assert_eq!(rule_display(&response, "data_quotient"), "2");
}

#[test]
fn i128_min_intermediate_divides_without_out_of_memory() {
    let code = r#"
        spec exactness
        data two: number
        rule constant_reciprocal_negative: 1 / (0 - 2 ^ 126 * 2) < 0
        rule data_reciprocal_negative: 1 / (0 - two ^ 126 * two) < 0
    "#;
    let response = evaluate_exactness(code, IndexMap::from([("two".to_string(), "2".to_string())]));
    assert_eq!(
        rule_display(&response, "constant_reciprocal_negative"),
        "true"
    );
    assert_eq!(rule_display(&response, "data_reciprocal_negative"), "true");
}

const MAGNITUDE_LIMIT_REASON: &str = "The numeric result exceeds the maximum size of 32768 bits.";

fn assert_magnitude_limit_veto(response: &lemma::Response, rule: &str) {
    let rule_result = response.results.get(rule).expect("rule missing");
    assert!(rule_result.vetoed, "{rule} must veto");
    assert_eq!(
        rule_result.veto_reason.as_deref(),
        Some(MAGNITUDE_LIMIT_REASON),
        "{rule} veto reason"
    );
}

#[test]
fn constant_beyond_magnitude_limit_is_planning_error() {
    for expression in [
        "2 ^ 1180591620717411303424",
        "2 ^ 32768",
        "2 ^ (0 - 32768)",
        "(1 / 2) ^ 32768",
        "1 / 2 ^ 32768",
        "2 ^ 32767 * 2",
        "2 ^ 32767 + 2 ^ 32767",
        "0 - 2 ^ 32767 - 2 ^ 32767",
        "(2 ^ 32767) / (1 / 2)",
        "x + 2 ^ 32768",
    ] {
        let code = format!("spec exactness\ndata x: number\nrule huge: {expression}\n");
        let mut engine = Engine::new();
        let errors = engine
            .load([(source(), code)])
            .expect_err(&format!("constant `{expression}` must not load"));
        let message = format!("{:?}", errors);
        assert!(
            message.contains(MAGNITUDE_LIMIT_REASON),
            "`{expression}`: expected planning error with the magnitude limit, got: {message}"
        );
    }
}

#[test]
fn constant_at_magnitude_limit_and_irrational_constants_load() {
    let code = r#"
        spec exactness
        rule at_limit: 2 ^ 32767 > 1
        rule irrational: 2 ^ 0.5 > 1
    "#;
    let response = evaluate_exactness(code, IndexMap::new());
    assert_eq!(rule_display(&response, "at_limit"), "true");
    assert_eq!(rule_display(&response, "irrational"), "true");
}

#[test]
fn data_power_beyond_magnitude_limit_vetoes() {
    let code = r#"
        spec exactness
        data years: number
        rule growth: 2 ^ years
    "#;
    let mut engine = Engine::new();
    engine
        .load([(source(), code.to_string())])
        .expect("spec must load");
    let data = IndexMap::from([("years".to_string(), "8589934592".into())]);
    let response = engine
        .evaluate(None, "exactness", None, data, None, true)
        .expect("evaluation must complete");
    assert_magnitude_limit_veto(&response, "growth");
}

#[test]
fn power_at_magnitude_limit_is_exact_and_one_bit_more_vetoes() {
    let code = r#"
        spec exactness
        data two: number
        rule at_limit: two ^ 32767 > 1
        rule beyond_limit: two ^ 32768 > 1
        rule reciprocal_at_limit: 1 / two ^ 32767 > 0
        rule reciprocal_beyond_limit: 1 / two ^ 32768 > 0
        rule negative_exponent_beyond_limit: two ^ (0 - 32768) > 0
        rule sum_beyond_limit: two ^ 32767 + two ^ 32767 > 1
        rule product_beyond_limit: two ^ 32767 * two > 1
    "#;
    let response = evaluate_exactness(code, IndexMap::from([("two".to_string(), "2".to_string())]));
    assert_eq!(
        rule_display(&response, "at_limit"),
        "true",
        "2^32767 has 32768 bits and must evaluate"
    );
    assert_eq!(rule_display(&response, "reciprocal_at_limit"), "true");
    for rule in [
        "beyond_limit",
        "reciprocal_beyond_limit",
        "negative_exponent_beyond_limit",
        "sum_beyond_limit",
        "product_beyond_limit",
    ] {
        assert_magnitude_limit_veto(&response, rule);
    }
}

#[test]
fn calendar_add_subtract_beyond_magnitude_limit_vetoes() {
    let code = r#"
        spec exactness
        uses lemma units
        data two: number
        rule big: two ^ 32767 * 1 month
        rule at_limit: big + 1 month > big
        rule sum_beyond_limit: big + big
        rule difference_beyond_limit: 0 month - big - big
    "#;
    let response = evaluate_exactness(code, IndexMap::from([("two".to_string(), "2".to_string())]));
    assert_eq!(rule_display(&response, "at_limit"), "true");
    assert_magnitude_limit_veto(&response, "sum_beyond_limit");
    assert_magnitude_limit_veto(&response, "difference_beyond_limit");
}

#[test]
fn measure_times_ratio_beyond_magnitude_limit_vetoes() {
    let code = r#"
        spec exactness
        uses lemma units
        data two: number
        rule big: two ^ 32767 * 1 meter
        rule at_limit: big * 50% < big
        rule scaled_beyond_limit: big * 200%
    "#;
    let response = evaluate_exactness(code, IndexMap::from([("two".to_string(), "2".to_string())]));
    assert_eq!(rule_display(&response, "at_limit"), "true");
    assert_magnitude_limit_veto(&response, "scaled_beyond_limit");
}

#[test]
fn repeated_squaring_across_rules_vetoes_at_magnitude_limit() {
    let mut code = String::from("spec exactness\ndata x: number\nrule r0: x\n");
    for index in 1..=20 {
        let previous = index - 1;
        code.push_str(&format!("rule r{index}: r{previous} * r{previous}\n"));
    }
    let mut engine = Engine::new();
    engine.load([(source(), code)]).expect("spec must load");
    let data = IndexMap::from([("x".to_string(), "3".into())]);
    let response = engine
        .evaluate(None, "exactness", None, data, None, true)
        .expect("evaluation must complete");
    let r14 = response.results.get("r14").expect("r14 missing");
    assert_eq!(
        r14.veto_reason.as_deref(),
        Some("The calculated result exceeds the decimal value limit."),
        "3^16384 fits the magnitude limit and only fails decimal output"
    );
    assert_magnitude_limit_veto(&response, "r15");
    assert!(response.results.get("r20").expect("r20 missing").vetoed);
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
        .evaluate(None, "exactness", None, IndexMap::new(), None, true)
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
        .evaluate(None, "exactness", None, IndexMap::new(), None, true)
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
        .evaluate(None, "exactness", None, IndexMap::new(), None, true)
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
        .evaluate(None, "wage", None, IndexMap::new(), None, true)
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

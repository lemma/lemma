//! Exact ASCII for range / ratio+veto / compound-unit / net_salary explain fixtures.

use indexmap::IndexMap;
use lemma::{format_explanation, DateTimeValue, Engine, EvaluateDataValue, SourceType};
use std::str::FromStr;

fn explain(
    source: &str,
    spec: &str,
    rule: &str,
    data: IndexMap<String, EvaluateDataValue>,
    effective: Option<&str>,
) -> lemma::Explanation {
    let mut engine = Engine::new();
    engine
        .load([(SourceType::Volatile, source.to_string())])
        .expect("gap fixture must load");
    let now = match effective {
        Some(s) => DateTimeValue::from_str(s).expect("effective"),
        None => DateTimeValue::now(),
    };
    let response = engine
        .evaluate(
            None,
            spec,
            Some(&now),
            data,
            Some(&[rule.to_string()]),
            true,
        )
        .expect("eval must succeed");
    response
        .results
        .get(rule)
        .expect("rule")
        .explanation
        .clone()
        .expect("explanation")
}

#[test]
fn ranges_grade_b_ascii() {
    let mut data = IndexMap::new();
    data.insert("score".into(), "85".into());
    let explanation = explain(
        include_str!("fixtures/explain/ranges_explain.lemma"),
        "grading",
        "grade",
        data,
        None,
    );
    assert_eq!(
        format_explanation(&explanation),
        include_str!("fixtures/explain/ranges_grade_b.ascii.txt")
    );
}

#[test]
fn ranges_in_q1_ascii() {
    let mut data = IndexMap::new();
    data.insert("hire_date".into(), "2023-03-15".into());
    let explanation = explain(
        include_str!("fixtures/explain/ranges_explain.lemma"),
        "employment_window",
        "in_q1_2023",
        data,
        None,
    );
    assert_eq!(
        format_explanation(&explanation),
        include_str!("fixtures/explain/ranges_in_q1.ascii.txt")
    );
}

#[test]
fn ratio_veto_invalid_ascii() {
    let mut data = IndexMap::new();
    data.insert("input_value".into(), "-5".into());
    let explanation = explain(
        include_str!("fixtures/explain/ratio_veto_explain.lemma"),
        "validation",
        "is_invalid",
        data,
        None,
    );
    assert_eq!(
        format_explanation(&explanation),
        include_str!("fixtures/explain/ratio_veto_invalid.ascii.txt")
    );
}

#[test]
fn ratio_interest_ascii() {
    let mut data = IndexMap::new();
    data.insert("principal".into(), "1000".into());
    data.insert("annual_rate".into(), "5%".into());
    let explanation = explain(
        include_str!("fixtures/explain/ratio_veto_explain.lemma"),
        "interest",
        "yearly_interest",
        data,
        None,
    );
    assert_eq!(
        format_explanation(&explanation),
        include_str!("fixtures/explain/ratio_interest.ascii.txt")
    );
}

#[test]
fn compound_total_payment_ascii() {
    let mut data = IndexMap::new();
    data.insert("hourly_rate".into(), "85 eur_per_hour".into());
    let explanation = explain(
        include_str!("fixtures/explain/compound_units_explain.lemma"),
        "contractor",
        "total_payment",
        data,
        None,
    );
    assert_eq!(
        format_explanation(&explanation),
        include_str!("fixtures/explain/compound_total_payment.ascii.txt")
    );
}

#[test]
fn net_salary_month_ascii() {
    let mut data = IndexMap::new();
    data.insert("gross_salary".into(), "3500 eur".into());
    data.insert("pay_period".into(), "month".into());
    data.insert("income_source".into(), "employment".into());
    data.insert("pension_contribution".into(), "0 eur".into());
    data.insert("payroll_tax_credit".into(), "true".into());
    let explanation = explain(
        include_str!("fixtures/explain/net_salary_explain.lemma"),
        "net_salary",
        "net_salary",
        data,
        Some("2026-06-01"),
    );
    assert_eq!(
        format_explanation(&explanation),
        include_str!("fixtures/explain/net_salary_month.ascii.txt")
    );
}

//! Exact ASCII explanation trees for dates_explain (from 05_date_handling).

use indexmap::IndexMap;
use lemma::{format_explanation, DateTimeValue, Engine, EvaluateDataValue, SourceType};
use std::str::FromStr;

const SPEC: &str = include_str!("fixtures/explain/dates_explain.lemma");

fn explain(rule: &str, effective: &str, current_date: &str) -> lemma::Explanation {
    let mut engine = Engine::new();
    engine
        .load([(SourceType::Volatile, SPEC.to_string())])
        .expect("dates_explain must load");
    let now = DateTimeValue::from_str(effective).expect("effective");
    let mut data = IndexMap::new();
    data.insert("current_date".into(), EvaluateDataValue::from(current_date));
    data.insert(
        "membership_start".into(),
        EvaluateDataValue::from("2024-01-01"),
    );
    data.insert(
        "membership_length".into(),
        EvaluateDataValue::from("1 year"),
    );
    let response = engine
        .evaluate(
            None,
            "date_handling",
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
fn membership_active_ascii() {
    let explanation = explain("membership_active", "2024-06-15", "2024-06-15");
    assert_eq!(
        format_explanation(&explanation),
        include_str!("fixtures/explain/dates_membership_active.ascii.txt")
    );
}

#[test]
fn is_urgent_ascii() {
    let explanation = explain("is_urgent", "2024-09-28", "2024-09-28");
    assert_eq!(
        format_explanation(&explanation),
        include_str!("fixtures/explain/dates_is_urgent.ascii.txt")
    );
}

#[test]
fn not_overdue_ascii() {
    let explanation = explain("is_overdue", "2024-11-01", "2024-11-01");
    assert_eq!(
        format_explanation(&explanation),
        include_str!("fixtures/explain/dates_not_overdue.ascii.txt")
    );
}

#[test]
fn probation_complete_ascii() {
    let explanation = explain("is_probation_complete", "2024-07-01", "2024-07-01");
    assert_eq!(
        format_explanation(&explanation),
        include_str!("fixtures/explain/dates_probation_complete.ascii.txt")
    );
}

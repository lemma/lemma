//! Tests for all integration test examples
//!
//! Ensures all example files in cli/tests/integrations/examples/ are valid and can be evaluated

use lemma::DateTimeValue;
use lemma::Engine;
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::str::FromStr;

fn decimal_lit(d: &str) -> Decimal {
    Decimal::from_str(d).unwrap()
}
fn load_examples() -> Engine {
    let mut engine = Engine::new();

    // Load all example files - paths relative to lemma/ crate
    let examples = [
        "../cli/tests/integrations/examples/01_simple_data.lemma",
        "../cli/tests/integrations/examples/02_rules_and_unless.lemma",
        "../cli/tests/integrations/examples/03_spec_references.lemma",
        "../cli/tests/integrations/examples/04_unit_conversions.lemma",
        "../cli/tests/integrations/examples/05_date_handling.lemma",
        "../cli/tests/integrations/examples/06_tax_calculation.lemma",
        "../cli/tests/integrations/examples/07_shipping_policy.lemma",
        "../cli/tests/integrations/examples/08_rule_references.lemma",
        "../cli/tests/integrations/examples/09_stress_test.lemma",
        "../cli/tests/integrations/examples/10_compensation_policy.lemma",
        "../cli/tests/integrations/examples/11_spec_composition.lemma",
    ];

    for path in examples {
        let content = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("Failed to read {}: {}", path, e));
        engine
            .load([(
                lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(path))),
                &content.to_string(),
            )])
            .unwrap_or_else(|errs| {
                panic!(
                    "Failed to parse {}: {}",
                    path,
                    errs.iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("; ")
                )
            });
    }

    engine
}

#[test]
fn test_02_rules_and_unless() {
    let engine = load_examples();
    let now = DateTimeValue::now();

    let mut data = std::collections::HashMap::new();
    data.insert("base_price".to_string(), "100.00".into());
    data.insert("quantity".to_string(), "10".into());
    data.insert("is_premium".to_string(), "true".into());
    data.insert("customer_age".to_string(), "17".into());
    let response = engine
        .run(None, "rules_and_unless", Some(&now), data, None, true)
        .expect("Evaluation failed");

    assert_eq!(response.spec_name, "rules_and_unless");

    let final_total = response.results.get("final_total").unwrap();
    assert!(!final_total.vetoed);
    let lit = final_total
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("value");
    match &lit.value {
        lemma::ValueKind::Number(n) => assert_eq!(
            lemma::ValueKind::Number(n.clone())
                .as_decimal_magnitude()
                .unwrap(),
            decimal_lit("800")
        ),
        other => panic!("Expected Number for final_total, got {:?}", other),
    }

    let age_validation = response.results.get("age_validation").unwrap();
    assert!(age_validation.vetoed);
    assert_eq!(
        age_validation.veto_reason.as_deref(),
        Some("Customer must be 18 or older")
    );
}

#[test]
fn test_03_spec_references() {
    let engine = load_examples();
    let now = DateTimeValue::now();

    // specific_employee (references base_employee)
    let response = engine
        .run(
            None,
            "specific_employee",
            Some(&now),
            HashMap::new(),
            None,
            true,
        )
        .expect("Evaluation failed");

    assert_eq!(response.spec_name, "specific_employee");
    let salary_with_bonus = response.results.get("salary_with_bonus").unwrap();
    assert!(!salary_with_bonus.vetoed);
    let lit = salary_with_bonus
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("value");
    match &lit.value {
        lemma::ValueKind::Number(n) => assert_eq!(
            lemma::ValueKind::Number(n.clone())
                .as_decimal_magnitude()
                .unwrap(),
            decimal_lit("99000")
        ),
        other => panic!("Expected Number for salary_with_bonus, got {:?}", other),
    }

    let employee_summary = response.results.get("employee_summary").unwrap();
    assert_eq!(
        employee_summary
            .result
            .as_ref()
            .expect("rule result value")
            .text
            .as_deref(),
        Some("Alice Smith")
    );
}

#[test]
fn test_04_unit_conversions() {
    let engine = load_examples();
    let now = DateTimeValue::now();

    // Spec has all data defined, no type annotations needed
    let response = engine
        .run(
            None,
            "unit_conversions",
            Some(&now),
            HashMap::new(),
            None,
            true,
        )
        .expect("Evaluation failed");

    assert_eq!(response.spec_name, "unit_conversions");

    let duration_hours = response.results.get("duration_hours").unwrap();
    assert!(!duration_hours.vetoed);
    assert_eq!(
        duration_hours
            .result
            .as_ref()
            .and_then(|v| v.measure.as_ref())
            .and_then(|m| m.get("hour"))
            .copied(),
        Some(rust_decimal::Decimal::new(15, 1))
    );

    let duration_seconds = response.results.get("duration_seconds").unwrap();
    assert!(!duration_seconds.vetoed);
    assert_eq!(
        duration_seconds
            .result
            .as_ref()
            .and_then(|v| v.measure.as_ref())
            .and_then(|m| m.get("second"))
            .copied(),
        Some(rust_decimal::Decimal::from(5400))
    );

    let is_quick_processing = response.results.get("is_quick_processing").unwrap();
    assert_eq!(
        is_quick_processing.result().expect("result").to_string(),
        lemma::LiteralValue::from_bool(true).to_string(),
    );
}

#[test]
fn test_05_date_handling() {
    let engine = load_examples();
    let now = DateTimeValue::now();

    let mut data = std::collections::HashMap::new();
    data.insert("current_date".to_string(), "2024-06-15".into());
    let response = engine
        .run(None, "date_handling", Some(&now), data, None, true)
        .expect("Evaluation failed");

    // Spec evaluates successfully
    assert_eq!(response.spec_name, "date_handling");

    let probation_end = response.results.get("probation_end_date").unwrap();
    let date = probation_end
        .result
        .as_ref()
        .expect("rule result value")
        .date
        .as_ref()
        .expect("date");
    assert_eq!(date.year, 2024);
    assert_eq!(date.month, 5);
    assert_eq!(date.day, 30);

    let is_probation_complete = response.results.get("is_probation_complete").unwrap();
    assert_eq!(
        is_probation_complete.result().expect("result").to_string(),
        lemma::LiteralValue::from_bool(true).to_string(),
    );
}

#[test]
fn test_08_rule_references() {
    let engine = load_examples();
    let now = DateTimeValue::now();

    // Test examples/rule_references spec
    let response = engine
        .run(
            None,
            "rule_references",
            Some(&now),
            HashMap::new(),
            None,
            true,
        )
        .expect("Evaluation failed");

    assert_eq!(response.spec_name, "rule_references");
    assert_eq!(
        response
            .results
            .get("can_drive_legally")
            .unwrap()
            .result
            .as_ref()
            .expect("rule result value")
            .boolean,
        Some(true)
    );

    let driving_status = response.results.get("driving_status").unwrap();
    assert_eq!(
        driving_status
            .result
            .as_ref()
            .expect("rule result value")
            .text
            .as_deref(),
        Some("Can drive legally")
    );

    // Test examples/eligibility_check spec (also in the same file)
    let response = engine
        .run(
            None,
            "eligibility_check",
            Some(&now),
            HashMap::new(),
            None,
            true,
        )
        .expect("Evaluation failed");

    assert_eq!(response.spec_name, "eligibility_check");
    let can_travel = response.results.get("can_travel_internationally").unwrap();
    assert!(can_travel.vetoed);
    assert_eq!(
        can_travel.veto_reason.as_deref(),
        Some("Valid travel documents required")
    );

    let eligibility_message = response.results.get("eligibility_message").unwrap();
    assert!(eligibility_message.vetoed);
    assert_eq!(
        eligibility_message.veto_reason.as_deref(),
        Some("Valid travel documents required")
    );
}

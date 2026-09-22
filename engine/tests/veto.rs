//! Veto functionality tests
//!
//! Key behaviors:
//! 1. Veto blocks a rule from producing any valid result
//! 2. Veto applies only when the vetoed rule's value is needed
//! 3. Unless clauses can provide alternative values, so the veto doesn't apply
//! 4. Veto in unless clause conditions or results will apply to the dependent rule

use lemma::DateTimeValue;
use lemma::{Engine, LiteralValue};
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::str::FromStr;

#[test]
fn test_veto_blocks_rule_evaluation() {
    let code = r#"
spec age_check
uses lemma units
data age: 15
rule is_adult: age >= 18
    unless age < 18 then veto "Must be at least 18 year old"
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "age_check", Some(&now), HashMap::new(), None, false)
        .unwrap();
    let rule_result = response
        .results
        .values()
        .find(|r| r.rule.name == "is_adult")
        .unwrap();

    assert!(rule_result.vetoed);
    assert_eq!(
        rule_result.veto_reason.as_deref(),
        Some("Must be at least 18 year old")
    );
}

#[test]
fn test_veto_without_message() {
    let code = r#"
spec validation
data value: -5
rule is_valid: value > 0
    unless value < 0 then veto
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "validation", Some(&now), HashMap::new(), None, false)
        .unwrap();
    let rule_result = response
        .results
        .values()
        .find(|r| r.rule.name == "is_valid")
        .unwrap();

    assert!(rule_result.vetoed);
    assert_eq!(rule_result.veto_reason, None);
}

#[test]
fn test_veto_does_not_trigger_when_condition_false() {
    let code = r#"
spec age_check
uses lemma units
data age: 25
rule is_adult: age >= 18
    unless age < 18 then veto "Must be at least 18 year old"
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "age_check", Some(&now), HashMap::new(), None, false)
        .unwrap();
    let rule_result = response
        .results
        .values()
        .find(|r| r.rule.name == "is_adult")
        .unwrap();

    assert!(!rule_result.vetoed);
    assert_eq!(
        rule_result
            .result
            .as_ref()
            .expect("rule result value")
            .boolean,
        Some(true)
    );
}

#[test]
fn test_multiple_veto_clauses_first_one_triggers() {
    let code = r#"
spec validation
data age: 15
data score: 85
rule eligible: age >= 18 and score >= 80
    unless age < 18 then veto "Age requirement not met"
    unless score < 80 then veto "Score requirement not met"
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "validation", Some(&now), HashMap::new(), None, false)
        .unwrap();
    let rule_result = response
        .results
        .values()
        .find(|r| r.rule.name == "eligible")
        .unwrap();

    assert!(rule_result.vetoed);
    assert_eq!(
        rule_result.veto_reason.as_deref(),
        Some("Age requirement not met")
    );
}

#[test]
fn test_multiple_veto_clauses_second_one_triggers() {
    let code = r#"
spec validation
data age: 25
data score: 65
rule eligible: age >= 18 and score >= 80
    unless age < 18 then veto "Age requirement not met"
    unless score < 80 then veto "Score requirement not met"
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "validation", Some(&now), HashMap::new(), None, false)
        .unwrap();
    let rule_result = response
        .results
        .values()
        .find(|r| r.rule.name == "eligible")
        .unwrap();

    assert!(rule_result.vetoed);
    assert_eq!(
        rule_result.veto_reason.as_deref(),
        Some("Score requirement not met")
    );
}

#[test]
fn test_veto_with_complex_condition() {
    let code = r#"
spec salary_check
data salary: 30000
data experience: 2
rule valid_compensation: salary >= 40000
    unless salary < 40000 and experience < 5 then veto "Insufficient salary for experience level"
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(
            None,
            "salary_check",
            Some(&now),
            HashMap::new(),
            None,
            false,
        )
        .unwrap();
    let rule_result = response
        .results
        .values()
        .find(|r| r.rule.name == "valid_compensation")
        .unwrap();

    assert!(rule_result.vetoed);
    assert_eq!(
        rule_result.veto_reason.as_deref(),
        Some("Insufficient salary for experience level")
    );
}

#[test]
fn test_veto_vs_regular_unless_mixed() {
    let code = r#"
spec mixed_validation
data age: 20
data country: "US"
data has_license: false
rule can_drive: age >= 16
    unless age < 16 then veto "Too young to drive"
    unless country is not "US" then false
    unless not has_license then false
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(
            None,
            "mixed_validation",
            Some(&now),
            HashMap::new(),
            None,
            false,
        )
        .unwrap();
    let rule_result = response
        .results
        .values()
        .find(|r| r.rule.name == "can_drive")
        .unwrap();

    assert!(!rule_result.vetoed);
    assert_eq!(
        rule_result
            .result
            .as_ref()
            .expect("rule result value")
            .boolean,
        Some(false)
    );
}

#[test]
fn test_veto_with_number_comparison() {
    let code = r#"
spec weight_check
data package_weight: 100
rule can_ship: package_weight <= 50
    unless package_weight > 75 then veto "Package exceeds maximum weight limit"
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(
            None,
            "weight_check",
            Some(&now),
            HashMap::new(),
            None,
            false,
        )
        .unwrap();
    let rule_result = response
        .results
        .values()
        .find(|r| r.rule.name == "can_ship")
        .unwrap();

    assert!(rule_result.vetoed);
    assert_eq!(
        rule_result.veto_reason.as_deref(),
        Some("Package exceeds maximum weight limit")
    );
}

#[test]
fn test_veto_with_money_comparison() {
    let code = r#"
spec pricing_check
data price: 5000
rule is_affordable: price <= 1000
    unless price > 4000 then veto "Price exceeds budget limit"
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(
            None,
            "pricing_check",
            Some(&now),
            HashMap::new(),
            None,
            false,
        )
        .unwrap();
    let rule_result = response
        .results
        .values()
        .find(|r| r.rule.name == "is_affordable")
        .unwrap();

    assert!(rule_result.vetoed);
    assert_eq!(
        rule_result.veto_reason.as_deref(),
        Some("Price exceeds budget limit")
    );
}

#[test]
fn test_veto_with_date_comparison() {
    let code = r#"
spec date_validation
data event_date: 2024-01-15
data min_date: 2024-06-01
rule is_valid_date: event_date >= min_date
    unless event_date < 2024-03-01 then veto "Event date is too early in the year"
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(
            None,
            "date_validation",
            Some(&now),
            HashMap::new(),
            None,
            false,
        )
        .unwrap();
    let rule_result = response
        .results
        .values()
        .find(|r| r.rule.name == "is_valid_date")
        .unwrap();

    assert!(rule_result.vetoed);
    assert_eq!(
        rule_result.veto_reason.as_deref(),
        Some("Event date is too early in the year")
    );
}

#[test]
fn test_veto_with_percentage_comparison() {
    let code = r#"
spec completion_check
data completion: 15%
rule is_complete: completion >= 95%
    unless completion < 20% then veto "Project barely started"
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(
            None,
            "completion_check",
            Some(&now),
            HashMap::new(),
            None,
            false,
        )
        .unwrap();
    let rule_result = response
        .results
        .values()
        .find(|r| r.rule.name == "is_complete")
        .unwrap();

    assert!(rule_result.vetoed);
    assert_eq!(
        rule_result.veto_reason.as_deref(),
        Some("Project barely started")
    );
}

#[test]
fn test_veto_with_rule_reference() {
    let code = r#"
spec eligibility
data age: 16
data has_permission: false
rule is_adult: age >= 18
rule eligible: has_permission
    unless not is_adult then veto "Must be adult or have permission"
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "eligibility", Some(&now), HashMap::new(), None, false)
        .unwrap();
    let eligible_result = response
        .results
        .values()
        .find(|r| r.rule.name == "eligible")
        .unwrap();

    assert!(eligible_result.vetoed);
    assert_eq!(
        eligible_result.veto_reason.as_deref(),
        Some("Must be adult or have permission")
    );
}

#[test]
fn test_veto_with_arithmetic_in_condition() {
    let code = r#"
spec budget_check
data expenses: 9500
data income: 10000
rule within_budget: expenses < income
    unless expenses > income * 0.9 then veto "Expenses exceed 90% of income"
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(
            None,
            "budget_check",
            Some(&now),
            HashMap::new(),
            None,
            false,
        )
        .unwrap();
    let rule_result = response
        .results
        .values()
        .find(|r| r.rule.name == "within_budget")
        .unwrap();

    assert!(rule_result.vetoed);
    assert_eq!(
        rule_result.veto_reason.as_deref(),
        Some("Expenses exceed 90% of income")
    );
}

#[test]
fn test_veto_with_string_equality() {
    let code = r#"
spec status_check
data status: "cancelled"
rule is_active: status is "active"
    unless status is "cancelled" then veto "Cannot process cancelled items"
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(
            None,
            "status_check",
            Some(&now),
            HashMap::new(),
            None,
            false,
        )
        .unwrap();
    let rule_result = response
        .results
        .values()
        .find(|r| r.rule.name == "is_active")
        .unwrap();

    assert!(rule_result.vetoed);
    assert_eq!(
        rule_result.veto_reason.as_deref(),
        Some("Cannot process cancelled items")
    );
}

#[test]
fn test_veto_does_not_affect_other_rules() {
    let code = r#"
spec multi_rule
data value: -10
rule check_positive: value > 0
    unless value < 0 then veto "Value must be positive"
rule check_negative: value < 0
rule double_value: value * 2
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "multi_rule", Some(&now), HashMap::new(), None, false)
        .unwrap();

    let check_positive = response
        .results
        .values()
        .find(|r| r.rule.name == "check_positive")
        .unwrap();
    assert!(check_positive.vetoed);
    assert_eq!(
        check_positive.veto_reason.as_deref(),
        Some("Value must be positive")
    );

    let check_negative = response
        .results
        .values()
        .find(|r| r.rule.name == "check_negative")
        .unwrap();
    assert!(!check_negative.vetoed);
    assert_eq!(
        check_negative
            .result
            .as_ref()
            .expect("rule result value")
            .boolean,
        Some(true)
    );

    let double_value = response
        .results
        .values()
        .find(|r| r.rule.name == "double_value")
        .unwrap();
    assert_eq!(
        double_value.result().expect("result").to_string(),
        LiteralValue::number_from_decimal(Decimal::from_str("-20.0").unwrap()).to_string(),
    );
}

#[test]
fn test_veto_with_special_characters_in_message() {
    let code = r#"
spec special_chars
data age: 10
rule valid: age >= 18
    unless age < 18 then veto "Error: Age < 18! Must be 18+. Contact: admin@example.com (555-1234)"
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(
            None,
            "special_chars",
            Some(&now),
            HashMap::new(),
            None,
            false,
        )
        .unwrap();
    let rule_result = response
        .results
        .values()
        .find(|r| r.rule.name == "valid")
        .unwrap();

    assert!(rule_result.vetoed);
    assert_eq!(
        rule_result.veto_reason.as_deref(),
        Some("Error: Age < 18! Must be 18+. Contact: admin@example.com (555-1234)")
    );
}

#[test]
fn test_veto_with_very_long_message() {
    let message = "This is a very long veto message that contains a lot of text to test how the system handles lengthy error messages. It includes multiple sentences and should be properly stored and returned. The system should handle this without any issues regardless of the message length. Testing edge cases is important for robust software.";

    let code = format!(
        r#"
spec long_message
data value: 0
rule valid: value > 0
    unless value is 0 then veto "{}"
"#,
        message
    );

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, &code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(
            None,
            "long_message",
            Some(&now),
            HashMap::new(),
            None,
            false,
        )
        .unwrap();
    let rule_result = response
        .results
        .values()
        .find(|r| r.rule.name == "valid")
        .unwrap();

    assert!(rule_result.vetoed);
    assert_eq!(rule_result.veto_reason.as_deref(), Some(message));
}

#[test]
fn test_veto_priority_over_false_result() {
    let code = r#"
spec priority_test
data value: 5
rule check: value > 10
    unless value < 10 then veto "Value too small"
    unless value is not 5 then false
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(
            None,
            "priority_test",
            Some(&now),
            HashMap::new(),
            None,
            false,
        )
        .unwrap();
    let rule_result = response
        .results
        .values()
        .find(|r| r.rule.name == "check")
        .unwrap();

    assert!(rule_result.vetoed);
    assert_eq!(rule_result.veto_reason.as_deref(), Some("Value too small"));
}

#[test]
fn test_veto_with_multiple_unless_conditions() {
    let code = r#"
spec multi_unless
data age: 30
data has_criminal_record: true
rule eligible: age >= 18
    unless age < 18 then veto "Eligibility criteria not met"
    unless has_criminal_record then veto "Eligibility criteria not met"
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(
            None,
            "multi_unless",
            Some(&now),
            HashMap::new(),
            None,
            false,
        )
        .unwrap();
    let rule_result = response
        .results
        .values()
        .find(|r| r.rule.name == "eligible")
        .unwrap();

    assert!(rule_result.vetoed);
    assert_eq!(
        rule_result.veto_reason.as_deref(),
        Some("Eligibility criteria not met")
    );
}

#[test]
fn test_veto_with_negation() {
    let code = r#"
spec negation_test
data is_verified: false
rule can_proceed: true
    unless not is_verified then veto "Account must be verified"
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(
            None,
            "negation_test",
            Some(&now),
            HashMap::new(),
            None,
            false,
        )
        .unwrap();
    let rule_result = response
        .results
        .values()
        .find(|r| r.rule.name == "can_proceed")
        .unwrap();

    assert!(rule_result.vetoed);
    assert_eq!(
        rule_result.veto_reason.as_deref(),
        Some("Account must be verified")
    );
}

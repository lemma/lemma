use lemma::DateTimeValue;
use lemma::Engine;
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::str::FromStr;

fn decimal_lit(d: &str) -> Decimal {
    Decimal::from_str(d).unwrap()
}
#[test]
fn test_employee_contract_comprehensive() {
    let mut engine = Engine::new();

    let base_contract = r#"
spec base_contract
uses lemma units
data min_salary: 30000
data max_salary: 200000
data standard_vacation_days: 20 day
data probation_period: 90 day
data min_age: 18 year
"#;

    let employment_terms = r#"
spec employment_terms
uses lemma units
uses base: base_contract
data salary: 75000
data bonus_percentage: 10%
data start_date: 2024-01-15
data vacation_days: 20 day
data employee_age: 28 year

rule total_compensation: salary + (salary * bonus_percentage)
rule is_salary_valid: salary >= base.min_salary and salary <= base.max_salary
rule vacation_days_ok: vacation_days >= base.standard_vacation_days
rule is_adult: employee_age >= base.min_age
rule probation_end_date: start_date + base.probation_period

rule contract_valid: is_salary_valid and vacation_days_ok and is_adult
    unless not is_adult then veto "Employee must be 18 or older"
"#;

    engine
        .load([(lemma::SourceType::Volatile, base_contract.to_string())])
        .unwrap();
    engine
        .load([(lemma::SourceType::Volatile, employment_terms.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(
            None,
            "employment_terms",
            Some(&now),
            HashMap::new(),
            None,
            true,
        )
        .unwrap();

    let total_comp = response
        .results
        .values()
        .find(|r| r.rule.name == "total_compensation")
        .unwrap();

    assert!(!total_comp.vetoed);
    let lit = total_comp
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
            decimal_lit("82500")
        ),
        other => panic!("Expected Number for total_compensation, got {:?}", other),
    }

    let contract_valid = response
        .results
        .values()
        .find(|r| r.rule.name == "contract_valid")
        .unwrap();
    assert_eq!(
        contract_valid
            .result
            .as_ref()
            .expect("rule result value")
            .boolean,
        Some(true)
    );

    let _ = engine.remove(None, "employment_terms", Some(&now));
    let _ = engine.remove(None, "base_contract", Some(&now));
}

#[test]
fn test_tax_calculation_with_percentages() {
    let mut engine = Engine::new();

    let tax_spec = r#"
spec tax_calculation
data income: 80000
data deductions: 10000
data tax_rate_low: 10%
data tax_rate_mid: 20%
data tax_rate_high: 30%
data bracket_low: 40000
data bracket_mid: 80000

rule taxable_income: income - deductions
rule in_low_bracket: taxable_income <= bracket_low
rule in_mid_bracket: taxable_income > bracket_low and taxable_income <= bracket_mid
rule in_high_bracket: taxable_income > bracket_mid

rule tax_rate: tax_rate_low
  unless in_mid_bracket then tax_rate_mid
  unless in_high_bracket then tax_rate_high

rule tax_amount: taxable_income * tax_rate
rule net_income: income - tax_amount
rule effective_rate: (tax_amount / income) * 100%
"#;

    engine
        .load([(lemma::SourceType::Volatile, tax_spec.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(
            None,
            "tax_calculation",
            Some(&now),
            HashMap::new(),
            None,
            true,
        )
        .unwrap();

    let taxable = response
        .results
        .values()
        .find(|r| r.rule.name == "taxable_income")
        .unwrap();
    assert!(!taxable.vetoed);
    let lit = taxable
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
            decimal_lit("70000")
        ),
        other => panic!("Expected Number for taxable_income, got {:?}", other),
    }

    let in_mid = response
        .results
        .values()
        .find(|r| r.rule.name == "in_mid_bracket")
        .unwrap();
    assert_eq!(
        in_mid.result.as_ref().expect("rule result value").boolean,
        Some(true)
    );

    let tax_rate = response
        .results
        .values()
        .find(|r| r.rule.name == "tax_rate")
        .unwrap();
    assert!(!tax_rate.vetoed);
    let lit = tax_rate
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("value");
    match &lit.value {
        lemma::ValueKind::Ratio(n) => {
            assert_eq!(
                lemma::ValueKind::Number(n.clone())
                    .as_decimal_magnitude()
                    .unwrap(),
                decimal_lit("0.2")
            );
            assert!(matches!(lit.value, lemma::ValueKind::Ratio(_)));
            // primary unit lives on rule/data type, not LiteralValue
        }
        other => panic!("Expected Ratio for tax_rate, got {:?}", other),
    }

    let _ = engine.remove(None, "tax_calculation", Some(&now));
}

#[test]
fn test_cli_data_values_integration() {
    let mut engine = Engine::new();

    let config_spec = r#"
spec dynamic_config
data threshold: number
data multiplier: number
data base_value: 100

rule calculated_value: base_value * multiplier
rule exceeds_threshold: calculated_value > threshold
rule status: "LOW"
  unless exceeds_threshold then "HIGH"
"#;

    engine
        .load([(lemma::SourceType::Volatile, config_spec.to_string())])
        .unwrap();

    let mut data = std::collections::HashMap::new();
    data.insert("threshold".to_string(), "500".into());
    data.insert("multiplier".to_string(), "2".into());

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "dynamic_config", Some(&now), data, None, true)
        .unwrap();

    let calculated = response
        .results
        .values()
        .find(|r| r.rule.name == "calculated_value")
        .unwrap();
    assert_eq!(calculated.result().expect("result").to_string(), "200");

    let status = response
        .results
        .values()
        .find(|r| r.rule.name == "status")
        .unwrap();
    assert_eq!(status.result().expect("result").to_string(), "LOW");

    let mut data2 = std::collections::HashMap::new();
    data2.insert("threshold".to_string(), "150".into());
    data2.insert("multiplier".to_string(), "2".into());
    let response2 = engine
        .run(None, "dynamic_config", Some(&now), data2, None, true)
        .unwrap();

    let status2 = response2
        .results
        .values()
        .find(|r| r.rule.name == "status")
        .unwrap();
    assert_eq!(status2.result().expect("result").to_string(), "HIGH");

    let _ = engine.remove(None, "dynamic_config", Some(&now));
}

#[test]
fn test_date_arithmetic_comprehensive() {
    let mut engine = Engine::new();

    let timeline_spec = r#"
spec project_timeline
uses lemma units
data project_start: 2024-01-15
data phase1_duration: 30 day
data phase2_duration: 45 day
data phase3_duration: 60 day
data today: 2024-02-15

rule phase1_end: project_start + phase1_duration
rule phase2_end: phase1_end + phase2_duration
rule phase3_end: phase2_end + phase3_duration

rule project_duration: phase1_duration + phase2_duration + phase3_duration
rule elapsed_time: project_start...today as day
rule days_remaining: today...phase3_end as day

rule is_phase1_complete: today > phase1_end
rule is_phase2_complete: today > phase2_end
rule is_on_schedule: elapsed_time <= phase1_duration + phase2_duration
"#;

    engine
        .load([(lemma::SourceType::Volatile, timeline_spec.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(
            None,
            "project_timeline",
            Some(&now),
            HashMap::new(),
            None,
            true,
        )
        .unwrap();

    let phase1_complete = response
        .results
        .values()
        .find(|r| r.rule.name == "is_phase1_complete")
        .unwrap();
    assert_eq!(
        phase1_complete.result().expect("result").to_string(),
        "true"
    );

    let phase2_complete = response
        .results
        .values()
        .find(|r| r.rule.name == "is_phase2_complete")
        .unwrap();
    assert_eq!(
        phase2_complete
            .result
            .as_ref()
            .expect("rule result value")
            .boolean,
        Some(false)
    );

    let _ = engine.remove(None, "project_timeline", Some(&now));
}

// ============================================================================
// Spec reference field access tests
// ============================================================================

#[test]
fn test_spec_ref_field_access_with_units() {
    let mut engine = Engine::new();

    let base_spec = r#"
spec base
data min_salary: 30000
data max_salary: 200000
"#;

    let child_spec = r#"
spec child
uses base_contract: base
data salary: 75000

rule is_valid: salary >= base_contract.min_salary and salary <= base_contract.max_salary
"#;

    engine
        .load([(lemma::SourceType::Volatile, base_spec.to_string())])
        .unwrap();
    engine
        .load([(lemma::SourceType::Volatile, child_spec.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "child", Some(&now), HashMap::new(), None, true)
        .unwrap();

    let is_valid = response
        .results
        .values()
        .find(|r| r.rule.name == "is_valid")
        .unwrap();
    assert_eq!(
        is_valid.result.as_ref().expect("rule result value").boolean,
        Some(true)
    );
}

#[test]
fn test_spec_ref_field_access_arithmetic() {
    let mut engine = Engine::new();

    let base_spec = r#"
spec base
uses lemma units
data project_start: 2024-01-15
data probation_period: 90 day
"#;

    let child_spec = r#"
spec child
uses lemma units
uses base_contract: base

rule probation_end: base_contract.project_start + base_contract.probation_period
"#;

    engine
        .load([(lemma::SourceType::Volatile, base_spec.to_string())])
        .unwrap();
    engine
        .load([(lemma::SourceType::Volatile, child_spec.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "child", Some(&now), HashMap::new(), None, true)
        .unwrap();

    let probation_end = response
        .results
        .values()
        .find(|r| r.rule.name == "probation_end")
        .unwrap();

    assert!(!probation_end.vetoed);
    let date = probation_end
        .result
        .as_ref()
        .expect("rule result value")
        .date
        .as_ref()
        .expect("date");
    assert_eq!(date.year, 2024);
    assert_eq!(date.month, 4);
    assert_eq!(date.day, 14);
}

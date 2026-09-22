use lemma::DateTimeValue;
use lemma::Engine;
use std::collections::HashMap;

/// Test cross-spec data references (should work)
#[test]
fn test_cross_spec_data_reference() {
    let mut engine = Engine::new();

    let base_spec = r#"
spec base
data price: 100
data quantity: 5
"#;

    let derived_spec = r#"
spec derived
uses base_data: base
rule total: base_data.price * base_data.quantity
"#;

    engine
        .load([(lemma::SourceType::Volatile, base_spec.to_string())])
        .unwrap();
    engine
        .load([(lemma::SourceType::Volatile, derived_spec.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "derived", Some(&now), HashMap::new(), None, false)
        .unwrap();
    let total = response
        .results
        .values()
        .find(|r| r.rule.name == "total")
        .unwrap();

    assert_eq!(total.result().expect("result").to_string(), "500");
}

/// Test cross-spec rule reference
#[test]
fn test_cross_spec_rule_reference() {
    let mut engine = Engine::new();

    let base_spec = r#"
spec base
data value: 50
rule doubled: value * 2
"#;

    let derived_spec = r#"
spec derived
uses base_data: base
rule derived_value: base_data.doubled + 10
"#;

    engine
        .load([(lemma::SourceType::Volatile, base_spec.to_string())])
        .unwrap();
    engine
        .load([(lemma::SourceType::Volatile, derived_spec.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "derived", Some(&now), HashMap::new(), None, false)
        .unwrap();
    let derived_value = response
        .results
        .values()
        .find(|r| r.rule.name == "derived_value")
        .unwrap();

    assert_eq!(derived_value.result().expect("result").to_string(), "110");
}

/// Test cross-spec rule reference with dependencies
#[test]
fn test_cross_spec_rule_reference_with_dependencies() {
    let mut engine = Engine::new();

    let base_spec = r#"
spec base_employee
data monthly_salary: 5000
rule annual_salary: monthly_salary * 12
rule with_bonus: annual_salary * 1.1
"#;

    let derived_spec = r#"
spec manager
uses employee: base_employee
rule manager_bonus: employee.annual_salary * 0.15
"#;

    engine
        .load([(lemma::SourceType::Volatile, base_spec.to_string())])
        .unwrap();
    engine
        .load([(lemma::SourceType::Volatile, derived_spec.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "manager", Some(&now), HashMap::new(), None, false)
        .unwrap();
    let bonus = response
        .results
        .values()
        .find(|r| r.rule.name == "manager_bonus")
        .unwrap();

    assert_eq!(bonus.result().expect("result").to_string(), "9000");
}

/// Test data binding with cross-spec rule reference
#[test]
fn test_cross_spec_data_binding_with_rule_reference() {
    let mut engine = Engine::new();

    let base_spec = r#"
spec base
data price: 100
data quantity: 5
rule total: price * quantity
"#;

    let derived_spec = r#"
spec derived
uses config: base
  -> with price: 200
  -> with quantity: 3
rule derived_total: config.total
"#;

    engine
        .load([(lemma::SourceType::Volatile, base_spec.to_string())])
        .unwrap();
    engine
        .load([(lemma::SourceType::Volatile, derived_spec.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "derived", Some(&now), HashMap::new(), None, false)
        .unwrap();
    let total = response
        .results
        .values()
        .find(|r| r.rule.name == "derived_total")
        .unwrap();

    assert_eq!(total.result().expect("result").to_string(), "600");
}

/// Test nested cross-spec rule references
#[test]
fn test_nested_cross_spec_rule_reference() {
    let mut engine = Engine::new();

    let config_spec = r#"
spec config
data base_days: 3
rule standard_processing_days: base_days
rule express_processing_days: 1
"#;

    let order_spec = r#"
spec order
data is_express: false
rule processing_days: 5
"#;

    let derived_spec = r#"
spec derived
uses settings: config
uses order_info: order
rule total_days: settings.standard_processing_days + order_info.processing_days
"#;

    engine
        .load([(lemma::SourceType::Volatile, config_spec.to_string())])
        .unwrap();
    engine
        .load([(lemma::SourceType::Volatile, order_spec.to_string())])
        .unwrap();
    engine
        .load([(lemma::SourceType::Volatile, derived_spec.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "derived", Some(&now), HashMap::new(), None, false)
        .unwrap();
    let total = response
        .results
        .values()
        .find(|r| r.rule.name == "total_days")
        .unwrap();

    assert_eq!(total.result().expect("result").to_string(), "8");
}

/// Test cross-spec rule reference in unless clause
#[test]
fn test_cross_spec_rule_reference_in_unless_clause() {
    let mut engine = Engine::new();

    let base_spec = r#"
spec base
data threshold: 100
data value: 150
rule is_valid: value >= threshold
"#;

    let derived_spec = r#"
spec derived
uses base_data: base
rule status: "invalid"
  unless base_data.is_valid then "valid"
"#;

    engine
        .load([(lemma::SourceType::Volatile, base_spec.to_string())])
        .unwrap();
    engine
        .load([(lemma::SourceType::Volatile, derived_spec.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "derived", Some(&now), HashMap::new(), None, false)
        .unwrap();
    let status = response
        .results
        .values()
        .find(|r| r.rule.name == "status")
        .unwrap();

    assert_eq!(status.result().expect("result").to_string(), "valid");
}

/// Test that we can mix cross-spec data and rule references
#[test]
fn test_cross_spec_mixed_data_and_rule_references() {
    let mut engine = Engine::new();

    let base_spec = r#"
spec base
data input: 50
rule calculated: input * 2
"#;

    let derived_spec = r#"
spec derived
uses base_data: base
rule combined: base_data.input + base_data.calculated
"#;

    engine
        .load([(lemma::SourceType::Volatile, base_spec.to_string())])
        .unwrap();
    engine
        .load([(lemma::SourceType::Volatile, derived_spec.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "derived", Some(&now), HashMap::new(), None, false)
        .unwrap();
    let combined = response
        .results
        .values()
        .find(|r| r.rule.name == "combined")
        .unwrap();

    assert_eq!(combined.result().expect("result").to_string(), "150");
}

/// Test cross-spec data binding with multiple levels (should work)
#[test]
fn test_multi_level_data_binding() {
    let mut engine = Engine::new();

    let base_spec = r#"
spec base
data x: 10
data y: 20
data z: 30
"#;

    let derived_spec = r#"
spec derived
uses b: base
  -> with x: 100
  -> with y: 200
rule sum: b.x + b.y + b.z
"#;

    engine
        .load([(lemma::SourceType::Volatile, base_spec.to_string())])
        .unwrap();
    engine
        .load([(lemma::SourceType::Volatile, derived_spec.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "derived", Some(&now), HashMap::new(), None, false)
        .unwrap();
    let sum = response
        .results
        .values()
        .find(|r| r.rule.name == "sum")
        .unwrap();

    // x=100 (overridden), y=200 (overridden), z=30 (original)
    // 100 + 200 + 30 = 330
    assert_eq!(sum.result().expect("result").to_string(), "330");
}

/// Test simple data binding without rule references (should work)
#[test]
fn test_simple_data_binding() {
    let mut engine = Engine::new();

    let base_spec = r#"
spec base
data price: 100
data quantity: 5
"#;

    let derived_spec = r#"
spec derived
uses config: base
  -> with price: 200
  -> with quantity: 3
rule total: config.price * config.quantity
"#;

    engine
        .load([(lemma::SourceType::Volatile, base_spec.to_string())])
        .unwrap();
    engine
        .load([(lemma::SourceType::Volatile, derived_spec.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "derived", Some(&now), HashMap::new(), None, false)
        .unwrap();
    let total = response
        .results
        .values()
        .find(|r| r.rule.name == "total")
        .unwrap();

    // Should be 200 * 3 = 600 (using overridden data values)
    assert_eq!(total.result().expect("result").to_string(), "600");
}

/// Test that different data paths to the same rule produce different results
/// This is the critical test for the RulePath implementation!
#[test]
fn test_different_data_paths_produce_different_results() {
    let mut engine = Engine::new();

    let example1_spec = r#"
spec example1
data price: 99
rule total: price * 1.21
"#;

    let example2_spec = r#"
spec example2
uses base: example1
"#;

    let example3_spec = r#"
spec example3
uses base: example2
rule total1: base.base.total

uses base2: example2
  -> with base.price: 79
rule total2: base2.base.total
"#;

    engine
        .load([(lemma::SourceType::Volatile, example1_spec.to_string())])
        .unwrap();
    engine
        .load([(lemma::SourceType::Volatile, example2_spec.to_string())])
        .unwrap();
    engine
        .load([(lemma::SourceType::Volatile, example3_spec.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "example3", Some(&now), HashMap::new(), None, false)
        .unwrap();

    let total1 = response
        .results
        .values()
        .find(|r| r.rule.name == "total1")
        .unwrap();

    let total2 = response
        .results
        .values()
        .find(|r| r.rule.name == "total2")
        .unwrap();

    // total1 uses original price: 99 * 1.21 = 119.79
    assert_eq!(total1.result().expect("result").to_string(), "119.79");

    // total2 uses overridden price: 79 * 1.21 = 95.59
    assert_eq!(total2.result().expect("result").to_string(), "95.59");
}

#[test]
fn spec_ref_evaluates_to_referenced_spec() {
    let mut engine = Engine::new();

    let code = r#"
spec pricing
data base_price: 200

spec order
uses p: pricing
rule total: p.base_price
"#;

    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "order", Some(&now), HashMap::new(), None, false)
        .unwrap();
    let total = response
        .results
        .values()
        .find(|r| r.rule.name == "total")
        .unwrap();

    assert_eq!(
        total.result().expect("result").to_string(),
        "200",
        "Spec ref should evaluate against the referenced pricing spec"
    );
}

#[test]
fn cross_spec_dependency_rules_excluded_from_results() {
    let mut engine = Engine::new();

    let base_spec = r#"
spec base_employee
uses lemma units
data monthly_salary: 5000
data employment_duration: 3 year
rule annual_salary: monthly_salary * 12
rule is_eligible_for_bonus: false
  unless employment_duration >= 1 year then true
"#;

    let derived_spec = r#"
spec specific_employee
uses employee: base_employee
rule salary_with_bonus: employee.annual_salary
  unless employee.is_eligible_for_bonus then employee.annual_salary * 1.1
rule employee_summary: employee.monthly_salary
"#;

    engine
        .load([(lemma::SourceType::Volatile, base_spec.to_string())])
        .unwrap();
    engine
        .load([(lemma::SourceType::Volatile, derived_spec.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(
            None,
            "specific_employee",
            Some(&now),
            HashMap::new(),
            None,
            false,
        )
        .unwrap();

    let mut result_names: Vec<&str> = response.results.keys().map(|k| k.as_str()).collect();
    result_names.sort();
    assert_eq!(
        result_names,
        vec!["employee_summary", "salary_with_bonus"],
        "Only local rules should appear in results; cross-spec dependencies \
         (annual_salary, is_eligible_for_bonus) must be excluded"
    );

    assert_eq!(
        response.results.get("salary_with_bonus").unwrap().result(),
        Some("66000")
    );
}

#[test]
fn spec_ref_from_order_to_pricing_evaluates_correctly() {
    let mut engine = Engine::new();

    let code = r#"
spec pricing
data base_price: 150

spec order
uses p: pricing
rule total: p.base_price
"#;

    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "order", Some(&now), HashMap::new(), None, false)
        .unwrap();
    let total = response
        .results
        .values()
        .find(|r| r.rule.name == "total")
        .unwrap();

    assert_eq!(
        total.result().expect("result").to_string(),
        "150",
        "Spec ref should evaluate against the referenced pricing spec"
    );
}

#[test]
fn cross_spec_lemma_duration_unless_short_is_yes_when_one_hour() {
    let code = r#"
spec t
uses lemma units
rule short: yes
  unless units.duration >= 2 hour then no
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("spec must load");
    let now = DateTimeValue::now();
    let mut data = HashMap::new();
    data.insert("units.duration".to_string(), "1 hour".into());
    let resp = engine
        .run(None, "t", Some(&now), data, None, false)
        .expect("evaluation must run");

    let short = resp
        .results
        .get("short")
        .expect("rule 'short' must be present");
    if short.vetoed {
        panic!(
            "expected short = yes when units.duration is 1 hour, got veto: {}",
            short.veto_reason.as_deref().unwrap_or("Vetoed")
        );
    }
    let out = short.result().expect("result").to_string();
    assert!(
        out == "yes" || out == "true",
        "expected short = yes when units.duration is 1 hour, got: {out}"
    );
}

#[test]
fn cross_spec_lemma_duration_unless_short_is_no_when_three_hours() {
    let code = r#"
spec t
uses lemma units
rule short: yes
  unless units.duration >= 2 hour then no
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("spec must load");
    let now = DateTimeValue::now();
    let mut data = HashMap::new();
    data.insert("units.duration".to_string(), "3 hour".into());
    let resp = engine
        .run(None, "t", Some(&now), data, None, false)
        .expect("evaluation must run");

    let short = resp
        .results
        .get("short")
        .expect("rule 'short' must be present");
    if short.vetoed {
        panic!(
            "expected short = no when units.duration is 3 hour, got veto: {}",
            short.veto_reason.as_deref().unwrap_or("Vetoed")
        );
    }
    let out = short.result().expect("result").to_string();
    assert!(
        out == "no" || out == "false",
        "expected short = no when units.duration is 3 hour, got: {out}"
    );
}

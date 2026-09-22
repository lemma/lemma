//! Missing-data propagation when rules reference other rules (normalized expression).
//!
//! These tests encode **intended** behavior for normalized expression evaluation and
//! needs_data derived from live Piecewise arms.

use lemma::DateGranularity;
use lemma::DateTimeValue;
use lemma::Engine;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

fn coffee_example_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/coffee_order.lemma")
}

fn net_salary_example_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/nl/tax/net_salary.lemma")
}

fn effective_2026() -> DateTimeValue {
    DateTimeValue {
        year: 2026,
        month: 6,
        day: 1,
        hour: 12,
        minute: 0,
        second: 0,
        microsecond: 0,
        timezone: None,
        granularity: DateGranularity::DateTime,
    }
}

fn load_volatile(engine: &mut Engine, code: &str) {
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("spec must plan");
}

fn rule_by_name<'a>(response: &'a lemma::Response, name: &str) -> &'a lemma::RuleResult {
    response
        .results
        .get(name)
        .or_else(|| response.results.values().find(|r| r.rule.name == name))
        .unwrap_or_else(|| panic!("rule '{name}' not in results"))
}

fn assert_veto_reason_contains(rr: &lemma::RuleResult, needle: &str) {
    assert!(rr.vetoed, "rule '{}' must veto", rr.rule.name);
    let reason = rr
        .veto_reason
        .as_deref()
        .unwrap_or_else(|| panic!("rule '{}' vetoed without reason", rr.rule.name));
    assert!(
        reason.contains(needle),
        "rule '{}': expected veto reason containing {needle:?}, got {reason:?}",
        rr.rule.name
    );
}

#[test]
fn missing_data_empty_when_all_datas_provided() {
    let mut engine = Engine::new();
    let code = std::fs::read_to_string(coffee_example_path()).expect("read example");
    engine
        .load([(
            lemma::SourceType::Path(Arc::new(PathBuf::from("01_coffee_order.lemma"))),
            &code.to_string(),
        )])
        .expect("load");

    let now = DateTimeValue::now();

    let mut data = HashMap::new();
    data.insert("product".to_string(), "latte".into());
    data.insert("size".to_string(), "medium".into());
    data.insert("number_of_cups".to_string(), "1".into());
    data.insert("has_loyalty_card".to_string(), "false".into());
    data.insert("age".to_string(), "30".into());

    let response = engine
        .run(None, "coffee_order", Some(&now), data, None, false)
        .expect("run");
    for result in response.results.values() {
        assert!(
            result.missing_data().is_empty(),
            "all data provided: {} {:?}",
            result.rule.name,
            result.missing_data()
        );
    }
}

#[test]
fn missing_data_includes_product_when_no_inputs() {
    let mut engine = Engine::new();
    let code = std::fs::read_to_string(coffee_example_path()).expect("read example");
    engine
        .load([(
            lemma::SourceType::Path(Arc::new(PathBuf::from("01_coffee_order.lemma"))),
            &code.to_string(),
        )])
        .expect("load");

    let now = DateTimeValue::now();
    let response = engine
        .run(
            None,
            "coffee_order",
            Some(&now),
            HashMap::new(),
            None,
            false,
        )
        .expect("run");

    let base_price = rule_by_name(&response, "base_price");
    assert!(
        base_price.is_missing_data(),
        "base_price must be MissingData when product is unbound: {:?}",
        base_price.veto_reason
    );
    assert_eq!(
        base_price.missing_data(),
        ["product".to_string()].as_slice(),
        "base_price missing_data must list product, got {:?}",
        base_price.missing_data()
    );
    assert_eq!(
        base_price.veto_reason.as_deref(),
        Some("Missing data: product"),
        "got {:?}",
        base_price.veto_reason
    );
}

// --- F2 (minimal): dependent rule must propagate Missing data, not default veto arm ---

#[test]
fn rule_ref_dependent_propagates_missing_data_not_default_veto_message() {
    let code = r#"
spec pricing
data product: text
  -> option "latte"
rule base_price: veto "Unknown product"
  unless product is "latte" then 3
rule total: base_price * 2
"#;
    let mut engine = Engine::new();
    load_volatile(&mut engine, code);

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "pricing", Some(&now), HashMap::new(), None, false)
        .expect("run");

    assert_veto_reason_contains(rule_by_name(&response, "base_price"), "Missing data");
    assert_veto_reason_contains(rule_by_name(&response, "total"), "Missing data");
}

#[test]
fn coffee_order_dependent_rules_propagate_missing_product_not_default_veto() {
    let mut engine = Engine::new();
    let code = std::fs::read_to_string(coffee_example_path()).expect("read example");
    engine
        .load([(
            lemma::SourceType::Path(Arc::new(PathBuf::from("01_coffee_order.lemma"))),
            &code.to_string(),
        )])
        .expect("load");

    let now = DateTimeValue::now();
    let response = engine
        .run(
            None,
            "coffee_order",
            Some(&now),
            HashMap::new(),
            None,
            false,
        )
        .expect("run");

    assert_veto_reason_contains(rule_by_name(&response, "base_price"), "product");
    for name in ["price_per_cup", "subtotal", "discount_amount", "total"] {
        let rr = rule_by_name(&response, name);
        assert_veto_reason_contains(rr, "Missing data");
        assert!(
            !rr.veto_reason
                .as_deref()
                .is_some_and(|r| r.contains("Unknown type of coffee")),
            "rule '{name}' must not surface default veto arm when dependency lacks data"
        );
    }
}

// --- F1 (minimal): arithmetic on rule ref must not use default unless literal when dependency vetoes ---

#[test]
fn rule_ref_in_product_vetoes_when_periods_rule_missing_data_not_default_twelve() {
    let code = r#"
spec payroll
data gross: number
data pay_period: text
  -> option "month"
  -> option "week"
rule periods_per_year: 12
  unless pay_period is "week" then 52
rule gross_annual: gross * periods_per_year
"#;
    let mut engine = Engine::new();
    load_volatile(&mut engine, code);

    let now = DateTimeValue::now();
    let mut data = HashMap::new();
    data.insert("gross".to_string(), "5000".into());
    let response = engine
        .run(None, "payroll", Some(&now), data, None, false)
        .expect("run");

    assert_veto_reason_contains(rule_by_name(&response, "periods_per_year"), "pay_period");
    let gross_annual = rule_by_name(&response, "gross_annual");
    assert!(
        gross_annual.vetoed,
        "gross_annual must veto when periods_per_year lacks pay_period (must not assume 12)"
    );
    assert!(
        gross_annual.result().is_none(),
        "gross_annual must not produce a numeric value, got {:?}",
        gross_annual.result()
    );
}

#[test]
fn net_salary_per_period_outputs_veto_when_pay_period_missing() {
    let mut engine = Engine::new();
    let code = std::fs::read_to_string(net_salary_example_path()).expect("read example");
    engine
        .load([(
            lemma::SourceType::Path(Arc::new(PathBuf::from("nl/tax/net_salary.lemma"))),
            &code.to_string(),
        )])
        .expect("load");

    let effective = effective_2026();
    let mut data = HashMap::new();
    data.insert("gross_salary".to_string(), "5000 eur".into());
    let response = engine
        .run(None, "net_salary", Some(&effective), data, None, false)
        .expect("run");

    assert_veto_reason_contains(rule_by_name(&response, "periods_per_year"), "pay_period");

    for name in ["net_salary", "payroll_tax_per_period"] {
        let rr = rule_by_name(&response, name);
        assert!(
            rr.vetoed,
            "rule '{name}' must veto when periods_per_year is missing pay_period"
        );
        assert!(
            rr.result().is_none(),
            "rule '{name}' must not emit a per-period amount without pay_period, got {:?}",
            rr.result()
        );
    }
}

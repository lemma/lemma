//! Tests for example files under engine/documentation/examples/
//!
//! Ensures all example files in documentation/examples/ are valid and can be evaluated

use lemma::{DateGranularity, DateTimeValue, Engine, LemmaType, LiteralValue, ValueKind};
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

fn decimal_lit(s: &str) -> Decimal {
    Decimal::from_str(s).expect("BUG: test decimal literal must parse")
}

fn get_rule_value(
    engine: &Engine,
    spec_name: &str,
    rule_name: &str,
    data: HashMap<String, String>,
) -> (lemma::LiteralValue, Arc<LemmaType>) {
    let now = DateTimeValue::now();
    let response = engine
        .run(
            None,
            spec_name,
            Some(&now),
            data,
            Some(&[rule_name.to_string()]),
            true,
        )
        .unwrap();
    let rule_result = response
        .results
        .get(rule_name)
        .unwrap_or_else(|| panic!("rule '{}' not found in {}", rule_name, spec_name));
    let value = rule_result
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("value");
    (value, Arc::clone(&rule_result.rule.rule_type))
}

fn assert_measure(value: &LiteralValue, lemma_type: &LemmaType, magnitude: &str, unit: &str) {
    match &value.value {
        ValueKind::Measure(n) => {
            assert_eq!(
                ValueKind::Number(n.clone()).as_decimal_magnitude().unwrap(),
                decimal_lit(magnitude)
            );
            assert_eq!(
                lemma_type
                    .measure_runtime_signature()
                    .first()
                    .map(|(name, _)| name.as_str()),
                Some(unit)
            );
        }
        other => panic!("expected Measure, got {other:?}"),
    }
}

fn load_specs_folder_examples() -> Engine {
    let mut engine = Engine::new();

    let examples = [
        "../engine/documentation/examples/01_coffee_order.lemma",
        "../engine/documentation/examples/02_library_fees.lemma",
        "../engine/documentation/examples/03_recipe_scaling.lemma",
        "../engine/documentation/examples/04_membership_benefits.lemma",
        "../engine/documentation/examples/05_weather_clothing.lemma",
        "../engine/documentation/examples/nl/tax/net_salary.lemma",
    ];

    for path in examples {
        let full = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path);
        let content = std::fs::read_to_string(&full)
            .unwrap_or_else(|e| panic!("Failed to read {}: {}", full.display(), e));
        engine
            .load([(
                lemma::SourceType::Path(std::sync::Arc::new(full.clone())),
                &content.to_string(),
            )])
            .unwrap_or_else(|errs| {
                panic!(
                    "Failed to parse {}: {}",
                    full.display(),
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
fn test_01_coffee_order() {
    let engine = load_specs_folder_examples();

    let mut data = HashMap::new();
    data.insert("product".to_string(), "latte".into());
    data.insert("size".to_string(), "large".into());
    data.insert("number_of_cups".to_string(), "2".into());
    data.insert("has_loyalty_card".to_string(), "true".into());
    data.insert("age".to_string(), "70".into());

    let (total, total_type) = get_rule_value(&engine, "coffee_order", "total", data);

    // latte base_price (3.50 eur) * large size_multiplier (120%) = 4.20 eur per cup
    // 4.20 eur * 2 cups = 8.40 eur subtotal
    // loyalty_discount 10% + age_discount 10% = combined_discount 20%
    // discount_amount = 8.40 * 20% = 1.68 eur
    // total = 8.40 - 1.68 = 6.72 eur
    assert_measure(&total, &total_type, "6.72", "eur");
}

#[test]
fn test_02_library_fees() {
    let engine = load_specs_folder_examples();

    let mut data = HashMap::new();
    data.insert("due_date".to_string(), "2024-01-01".into());
    data.insert("return_date".to_string(), "2024-01-06".into());
    data.insert("book_type".to_string(), "regular".into());
    data.insert("is_first_offense".to_string(), "false".into());

    let (final_fee, final_fee_type) =
        get_rule_value(&engine, "library_fees", "final_fee", data.clone());
    assert_measure(&final_fee, &final_fee_type, "1.25", "eur");

    let (can_checkout, _) = get_rule_value(&engine, "library_fees", "can_checkout", data);
    assert_eq!(can_checkout.value, lemma::ValueKind::Boolean(true));
}

#[test]
fn test_03_recipe_scaling() {
    let engine = load_specs_folder_examples();

    let mut data = HashMap::new();
    data.insert("original_servings".to_string(), "4".into());
    data.insert("desired_servings".to_string(), "8".into());
    data.insert("recipe_name".to_string(), "chocolate_cake".into());

    let (scaling_factor, _) =
        get_rule_value(&engine, "recipe_scaling", "scaling_factor", data.clone());
    assert_eq!(
        scaling_factor.value,
        LiteralValue::number_from_decimal(decimal_lit("2")).value
    );

    let (baking_time, baking_type) =
        get_rule_value(&engine, "recipe_scaling", "baking_time", data.clone());
    assert_measure(&baking_time, &baking_type, "2400", "minute");

    let (oven_temp, oven_type) =
        get_rule_value(&engine, "recipe_scaling", "oven_temperature", data);
    assert_measure(&oven_temp, &oven_type, "175", "celsius");
}

#[test]
fn test_04_membership_benefits() {
    let engine = load_specs_folder_examples();

    // Test premium_membership spec (has rules, no data needed)
    let (discount_rate, _) = get_rule_value(
        &engine,
        "premium_membership",
        "discount_rate",
        HashMap::new(),
    );
    assert_eq!(
        discount_rate.value,
        LiteralValue::ratio_from_decimal(decimal_lit("0.10")).value
    );

    // Test membership_benefits spec (references premium_membership)
    let mut benefits_data = HashMap::new();
    benefits_data.insert("monthly_spend".to_string(), "150".into());
    let (discount, _) = get_rule_value(
        &engine,
        "membership_benefits",
        "discount",
        benefits_data.clone(),
    );
    assert_eq!(
        discount.value,
        LiteralValue::number_from_decimal(decimal_lit("15")).value
    );

    let (shipping_cost, _) = get_rule_value(
        &engine,
        "membership_benefits",
        "shipping_cost",
        benefits_data.clone(),
    );
    assert_eq!(
        shipping_cost.value,
        LiteralValue::number_from_decimal(decimal_lit("0")).value
    );

    let (total_points, _) = get_rule_value(
        &engine,
        "membership_benefits",
        "total_points",
        benefits_data,
    );
    assert_eq!(
        total_points.value,
        LiteralValue::number_from_decimal(decimal_lit("325")).value
    );
}

#[test]
fn test_05_weather_clothing() {
    let engine = load_specs_folder_examples();

    let mut data = HashMap::new();
    data.insert("temperature".to_string(), "15 celsius".into());
    data.insert("is_raining".to_string(), "false".into());
    data.insert("wind_speed".to_string(), "10".into());

    let (clothing_layer, _) =
        get_rule_value(&engine, "weather_clothing", "clothing_layer", data.clone());
    assert_eq!(clothing_layer.value, ValueKind::Text("light".to_string()));

    let (needs_jacket, _) = get_rule_value(&engine, "weather_clothing", "needs_jacket", data);
    assert_eq!(needs_jacket.value, ValueKind::Boolean(false));
}

#[test]
fn test_nl_tax_net_salary() {
    let engine = load_specs_folder_examples();

    let effective = DateTimeValue {
        year: 2026,
        month: 6,
        day: 1,
        hour: 12,
        minute: 0,
        second: 0,
        microsecond: 0,
        timezone: None,
        granularity: DateGranularity::DateTime,
    };

    let mut data = HashMap::new();
    data.insert("gross_salary".to_string(), "5000 eur".into());
    data.insert("pay_period".to_string(), "month".into());
    data.insert("income_source".to_string(), "employment".into());
    data.insert("pension_contribution".to_string(), "0 eur".into());
    data.insert("payroll_tax_credit".to_string(), "true".into());
    let response = engine
        .run(None, "net_salary", Some(&effective), data, None, true)
        .expect("net_salary run");

    let net = response
        .results
        .get("net_salary")
        .expect("net_salary in results");
    assert!(
        !net.vetoed,
        "net_salary must not veto with all required data provided"
    );

    let gross_annual = response
        .results
        .get("gross_annual_salary")
        .expect("gross_annual_salary in results");
    assert!(!gross_annual.vetoed, "gross_annual_salary must not veto");
}

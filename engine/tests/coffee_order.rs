//! Integration test for coffee_order example
//!
//! Tests `uses`, qualified parent types, inline type declarations with constraints, and complex rule chains

use lemma::DateTimeValue;
use lemma::Engine;
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::str::FromStr;

fn decimal_lit(d: &str) -> Decimal {
    Decimal::from_str(d).unwrap()
}

fn load_coffee_order() -> Engine {
    let mut engine = Engine::new();

    // Load the examples spec first (contains money and priority types)
    let examples = r#"
spec examples

data money: measure
  -> decimals 2
  -> unit eur: 1.00
  -> unit gbp: 1.17
  -> minimum 0 eur

data priority: text
  -> option "low"
  -> option "medium"
  -> option "high"
"#;

    let coffee_order = r#"
spec coffee_order

uses examples

data coffee: text
  -> option "espresso"
  -> option "latte"
  -> option "cappuccino"
  -> option "mocha"

data size: text
  -> option "small"
  -> option "medium"
  -> option "large"
  -> option "extra large"

data price           : examples.money
data priority        : examples.priority
data number_of_cups  : number -> maximum 10
data has_loyalty_card: boolean

rule ordered_priority: veto "Unknown priority"
  unless priority is "low"    then 1
  unless priority is "medium" then 2
  unless priority is "high"   then 3

rule base_price: veto "Unknown type of coffee"
  unless coffee is "espresso"   then 2.50 eur
  unless coffee is "latte"      then 3.50 eur
  unless coffee is "cappuccino" then 3.50 eur
  unless coffee is "mocha"      then 4.00 eur

rule size_multiplier: veto "Unknown size of coffee"
  unless size is "small"  then 0.80
  unless size is "medium" then 1.00
  unless size is "large"  then 1.20

rule price_per_cup: base_price * size_multiplier

rule subtotal: price_per_cup * number_of_cups

rule loyalty_discount: 0.0
  unless has_loyalty_card then 0.10

rule discount_amount: subtotal * loyalty_discount

rule total: subtotal - discount_amount
"#;

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "examples.lemma",
            ))),
            examples.to_string(),
        )])
        .expect("Failed to parse examples");
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "coffee_order.lemma",
            ))),
            coffee_order.to_string(),
        )])
        .expect("Failed to parse coffee_order");

    engine
}

#[test]
fn test_coffee_order_espresso_small_no_loyalty() {
    let engine = load_coffee_order();
    let now = DateTimeValue::now();

    let data_values = HashMap::from([
        ("coffee".to_string(), "espresso".into()),
        ("size".to_string(), "small".into()),
        ("number_of_cups".to_string(), "2".into()),
        ("has_loyalty_card".to_string(), "false".into()),
    ]);
    let response = engine
        .run(None, "coffee_order", Some(&now), data_values, None, true)
        .expect("Evaluation failed");

    // Check base_price: espresso = 2.50 usd
    let base_price = response
        .results
        .values()
        .find(|r| r.rule.name == "base_price")
        .expect("base_price rule not found");

    let base_price_value = base_price
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("base_price should have value");
    // base_price should be Measure with unit "eur"
    match &base_price_value.value {
        lemma::ValueKind::Measure(n) => {
            let unit = base_price
                .result
                .as_ref()
                .and_then(|v| v.measure.as_ref())
                .and_then(|m| m.keys().next().map(|s| s.as_str()))
                .unwrap_or("");
            assert_eq!(
                unit, "eur",
                "base_price should have unit eur in measure map, got: {:?}",
                unit
            );

            // base_price preserves the numeric value as written for the unit.
            assert_eq!(
                lemma::ValueKind::Number(n.clone())
                    .as_decimal_magnitude()
                    .unwrap(),
                decimal_lit("2.50"),
                "base_price should be exactly 2.50 (2.50 eur), got: {}",
                n
            );
        }
        _ => panic!(
            "base_price should be Measure type, got: {:?}",
            base_price_value.value
        ),
    }

    // Check size_multiplier: small = 0.80
    let size_multiplier = response
        .results
        .values()
        .find(|r| r.rule.name == "size_multiplier")
        .expect("size_multiplier rule not found");

    let multiplier_value = size_multiplier
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("size_multiplier should have value");
    // size_multiplier should be Number (no unit)
    match &multiplier_value.value {
        lemma::ValueKind::Number(n) => {
            assert_eq!(
                lemma::ValueKind::Number(n.clone())
                    .as_decimal_magnitude()
                    .unwrap(),
                decimal_lit("0.80"),
                "size_multiplier should be 0.80, got: {}",
                n
            );
        }
        _ => panic!(
            "size_multiplier should be Number type, got: {:?}",
            multiplier_value.value
        ),
    }

    // Check price_per_cup = base_price * size_multiplier
    let price_per_cup = response
        .results
        .values()
        .find(|r| r.rule.name == "price_per_cup")
        .expect("price_per_cup rule not found");

    let cup_price = price_per_cup
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("price_per_cup should have value");
    // price_per_cup should be Measure with unit "eur" (inherited from base_price)
    match &cup_price.value {
        lemma::ValueKind::Measure(n) => {
            let unit = price_per_cup
                .result
                .as_ref()
                .and_then(|v| v.measure.as_ref())
                .and_then(|m| m.keys().next().map(|s| s.as_str()))
                .unwrap_or("");
            assert_eq!(
                unit, "eur",
                "price_per_cup should have unit eur in measure map, got: {:?}",
                unit
            );

            // base_price = 2.50, size_multiplier = 0.80
            // price_per_cup = 2.50 * 0.80 = 2.00
            assert_eq!(
                lemma::ValueKind::Number(n.clone())
                    .as_decimal_magnitude()
                    .unwrap(),
                decimal_lit("2.00"),
                "price_per_cup should be exactly 2.00 (2.50 * 0.80), got: {}",
                n
            );
        }
        _ => panic!(
            "price_per_cup should be Measure type, got: {:?}",
            cup_price.value
        ),
    }

    // Check subtotal = price_per_cup * 2 cups
    let subtotal = response
        .results
        .values()
        .find(|r| r.rule.name == "subtotal")
        .expect("subtotal rule not found");

    let subtotal_value = subtotal
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("subtotal should have value");
    // subtotal should be Measure with unit "eur" (inherited from price_per_cup)
    let subtotal_num = match &subtotal_value.value {
        lemma::ValueKind::Measure(n) => {
            let unit = subtotal
                .result
                .as_ref()
                .and_then(|v| v.measure.as_ref())
                .and_then(|m| m.keys().next().map(|s| s.as_str()))
                .unwrap_or("");
            assert_eq!(
                unit, "eur",
                "subtotal should have unit eur in measure map, got: {:?}",
                unit
            );

            n.clone()
        }
        _ => panic!(
            "subtotal should be Measure type, got: {:?}",
            subtotal_value.value
        ),
    };
    // price_per_cup = 2.00, number_of_cups = 2
    // subtotal = 2.00 * 2 = 4.00
    assert_eq!(
        lemma::ValueKind::Number(subtotal_num)
            .as_decimal_magnitude()
            .unwrap(),
        decimal_lit("4.00"),
        "subtotal should be exactly 4.00 (2.00 * 2)"
    );

    // Check loyalty_discount: false = 0.0
    let loyalty_discount = response
        .results
        .values()
        .find(|r| r.rule.name == "loyalty_discount")
        .expect("loyalty_discount rule not found");

    let discount = loyalty_discount
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("loyalty_discount should have value");
    // loyalty_discount: false = 0.0 (should be Number, not Ratio when 0.0)
    match &discount.value {
        lemma::ValueKind::Number(n) => {
            assert_eq!(
                lemma::ValueKind::Number(n.clone())
                    .as_decimal_magnitude()
                    .unwrap(),
                decimal_lit("0.00"),
                "loyalty_discount should be 0.00, got: {}",
                n
            );
        }
        _ => panic!(
            "loyalty_discount should be Number type when 0.0, got: {:?}",
            discount.value
        ),
    }

    // Check total = subtotal - discount_amount (should equal subtotal when no discount)
    let total = response
        .results
        .values()
        .find(|r| r.rule.name == "total")
        .expect("total rule not found");

    let total_eur = total
        .result
        .as_ref()
        .and_then(|v| v.measure.as_ref())
        .and_then(|m| m.get("eur"))
        .expect("total measure map must include eur");
    let subtotal_eur = response
        .results
        .values()
        .find(|r| r.rule.name == "subtotal")
        .expect("subtotal rule not found")
        .result
        .as_ref()
        .and_then(|v| v.measure.as_ref())
        .and_then(|m| m.get("eur"))
        .expect("subtotal measure map must include eur");
    assert_eq!(
        total_eur, subtotal_eur,
        "total should equal subtotal when discount is 0"
    );
}

#[test]
fn test_coffee_order_latte_large_with_loyalty() {
    let engine = load_coffee_order();
    let now = DateTimeValue::now();

    let data_values = HashMap::from([
        ("coffee".to_string(), "latte".into()),
        ("size".to_string(), "large".into()),
        ("number_of_cups".to_string(), "3".into()),
        ("has_loyalty_card".to_string(), "true".into()),
    ]);
    let response = engine
        .run(None, "coffee_order", Some(&now), data_values, None, true)
        .expect("Evaluation failed");

    // Check base_price: latte = 3.50 usd
    let base_price = response
        .results
        .values()
        .find(|r| r.rule.name == "base_price")
        .expect("base_price rule not found");

    let base_price_value = base_price
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("base_price should have value");
    // base_price should be Measure with unit "eur"
    match &base_price_value.value {
        lemma::ValueKind::Measure(n) => {
            let unit = base_price
                .result
                .as_ref()
                .and_then(|v| v.measure.as_ref())
                .and_then(|m| m.keys().next().map(|s| s.as_str()))
                .unwrap_or("");
            assert_eq!(
                unit, "eur",
                "base_price should have unit eur in measure map, got: {:?}",
                unit
            );

            // base_price preserves the numeric value as written for the unit.
            assert_eq!(
                lemma::ValueKind::Number(n.clone())
                    .as_decimal_magnitude()
                    .unwrap(),
                decimal_lit("3.50"),
                "base_price should be exactly 3.50 (3.50 eur), got: {}",
                n
            );
        }
        _ => panic!(
            "base_price should be Measure type, got: {:?}",
            base_price_value.value
        ),
    }

    // Check size_multiplier: large = 1.20
    let size_multiplier = response
        .results
        .values()
        .find(|r| r.rule.name == "size_multiplier")
        .expect("size_multiplier rule not found");

    let multiplier_value = size_multiplier
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("size_multiplier should have value");
    // size_multiplier should be Number (no unit)
    match &multiplier_value.value {
        lemma::ValueKind::Number(n) => {
            assert_eq!(
                lemma::ValueKind::Number(n.clone())
                    .as_decimal_magnitude()
                    .unwrap(),
                decimal_lit("1.20"),
                "size_multiplier should be 1.20, got: {}",
                n
            );
        }
        _ => panic!(
            "size_multiplier should be Number type, got: {:?}",
            multiplier_value.value
        ),
    }

    // Check loyalty_discount: true = 0.10
    // Note: 0.10 is written as a number literal, not "10%", so it's a Number, not a Ratio
    let loyalty_discount = response
        .results
        .values()
        .find(|r| r.rule.name == "loyalty_discount")
        .expect("loyalty_discount rule not found");

    let discount = loyalty_discount
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("loyalty_discount should have value");
    // loyalty_discount should be Number (since 0.10 is written as number, not percentage)
    match &discount.value {
        lemma::ValueKind::Number(n) => {
            assert_eq!(
                lemma::ValueKind::Number(n.clone())
                    .as_decimal_magnitude()
                    .unwrap(),
                decimal_lit("0.10"),
                "loyalty_discount should be exactly 0.10, got: {}",
                n
            );
        }
        _ => panic!(
            "loyalty_discount should be Number type, got: {:?}",
            discount.value
        ),
    }

    // Check total should be less than subtotal (due to discount)
    let subtotal = response
        .results
        .values()
        .find(|r| r.rule.name == "subtotal")
        .expect("subtotal rule not found");

    let total = response
        .results
        .values()
        .find(|r| r.rule.name == "total")
        .expect("total rule not found");

    let subtotal_value = subtotal
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("subtotal should have value");
    total
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("total should have value");

    // subtotal should be Measure with unit "eur" (inherited from price_per_cup)
    let subtotal_num = match &subtotal_value.value {
        lemma::ValueKind::Measure(n) => {
            let unit = subtotal
                .result
                .as_ref()
                .and_then(|v| v.measure.as_ref())
                .and_then(|m| m.keys().next().map(|s| s.as_str()))
                .unwrap_or("");
            assert_eq!(
                unit, "eur",
                "subtotal should have unit eur in measure map, got: {:?}",
                unit
            );

            n.clone()
        }
        _ => panic!(
            "subtotal should be Measure type, got: {:?}",
            subtotal_value.value
        ),
    };
    // price_per_cup = 3.50 * 1.20 = 4.20, number_of_cups = 3
    // subtotal = 4.20 * 3 = 12.60
    assert_eq!(
        lemma::ValueKind::Number(subtotal_num)
            .as_decimal_magnitude()
            .unwrap(),
        decimal_lit("12.60"),
        "subtotal should be exactly 12.60 (4.20 * 3)"
    );

    let total_eur = total
        .result
        .as_ref()
        .and_then(|v| v.measure.as_ref())
        .and_then(|m| m.get("eur"))
        .expect("total measure map must include eur");
    // discount_amount = 12.60 * 0.10 = 1.26
    // total = 12.60 - 1.26 = 11.34
    assert_eq!(
        *total_eur,
        decimal_lit("11.34"),
        "total should be exactly 11.34 (12.60 - 1.26)"
    );
}

#[test]
fn test_coffee_order_ordered_priority() {
    let engine = load_coffee_order();
    let now = DateTimeValue::now();

    // Test priority mapping
    let priorities = ["low", "medium", "high"];
    let expected_values = ["1", "2", "3"];

    for (priority, expected) in priorities.iter().zip(expected_values.iter()) {
        let data_values = HashMap::from([("priority".to_string(), (*priority).to_string())]);
        let response = engine
            .run(None, "coffee_order", Some(&now), data_values, None, true)
            .expect("Evaluation failed");

        let ordered_priority = response
            .results
            .values()
            .find(|r| r.rule.name == "ordered_priority")
            .expect("ordered_priority rule not found");

        assert_eq!(
            ordered_priority.result().expect("result").to_string(),
            *expected,
            "priority '{}' should map to {}, got: {}",
            priority,
            expected,
            ordered_priority.result().unwrap_or("")
        );
    }
}

#[test]
fn test_coffee_order_invalid_size_veto() {
    let engine = load_coffee_order();
    let now = DateTimeValue::now();

    // Size "extra large" is defined in the inline type constraint, but size_multiplier
    // only handles small/medium/large, so it should veto
    let data_values = HashMap::from([
        ("coffee".to_string(), "espresso".into()),
        ("size".to_string(), "extra large".into()),
        ("number_of_cups".to_string(), "1".into()),
    ]);
    let response = engine
        .run(None, "coffee_order", Some(&now), data_values, None, true)
        .expect("Evaluation should complete (even with veto)");

    let size_multiplier = response
        .results
        .values()
        .find(|r| r.rule.name == "size_multiplier")
        .expect("size_multiplier rule not found");

    // size_multiplier should veto because "extra large" is not handled
    assert!(
        size_multiplier.vetoed,
        "size_multiplier should veto for 'extra large' size"
    );

    // price_per_cup and subsequent rules must veto when size_multiplier vetoes
    let price_per_cup = response
        .results
        .values()
        .find(|r| r.rule.name == "price_per_cup")
        .expect("price_per_cup");
    assert!(
        price_per_cup.vetoed,
        "price_per_cup should fail when size_multiplier vetoes"
    );
    let subtotal = response
        .results
        .values()
        .find(|r| r.rule.name == "subtotal")
        .expect("subtotal");
    assert!(
        subtotal.vetoed,
        "subtotal should fail when price_per_cup vetoes"
    );
    let total = response
        .results
        .values()
        .find(|r| r.rule.name == "total")
        .expect("total");
    assert!(total.vetoed, "total should fail when subtotal vetoes");
}

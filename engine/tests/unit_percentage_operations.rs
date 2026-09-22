use lemma::Engine;
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::str::FromStr;

fn decimal_lit(d: &str) -> Decimal {
    Decimal::from_str(d).unwrap()
}

fn expect_plan_error(code: &str, expected_fragment: &str) {
    let mut engine = Engine::new();
    let result = engine.load([(lemma::SourceType::Volatile, code.to_string())]);
    assert!(result.is_err(), "Expected planning error");
    let combined = result
        .unwrap_err()
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("; ");
    assert!(
        combined.contains(expected_fragment),
        "Error should contain '{}', got: {}",
        expected_fragment,
        combined
    );
}

#[test]
fn test_unit_subtract_percentage_rejected() {
    let code = r#"
        spec pricing

        data quantity: 10
        data discount: 10%

        rule price: 200 - discount
        "#;
    expect_plan_error(code, "scale explicitly");
}

#[test]
fn test_unit_add_percentage_rejected() {
    let code = r#"
        spec tax_calculation

        data base_price: 100
        data tax_rate: 8.5%

        rule price_with_tax: base_price + tax_rate
        "#;
    expect_plan_error(code, "scale explicitly");
}

#[test]
fn test_unit_multiply_percentage() -> Result<(), lemma::Errors> {
    let mut engine = Engine::new();

    engine.load([(
        lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("ops.lemma"))),
        r#"
        spec unit_percentage_ops

        data price: 50
        data increase: 20%

        rule scaled: price * increase
        "#
        .to_string(),
    )])?;

    let now = lemma::DateTimeValue::now();
    let response = engine
        .run(
            None,
            "unit_percentage_ops",
            Some(&now),
            HashMap::new(),
            None,
            true,
        )
        .expect("run should succeed after load");

    let scaled_result = response
        .results
        .values()
        .find(|r| r.rule.name == "scaled")
        .expect("scaled rule not found");

    let lit = scaled_result
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("value");
    if let lemma::ValueKind::Number(_n) = &lit.value {
        assert_eq!(
            lit.value,
            lemma::LiteralValue::number_from_decimal(decimal_lit("10")).value
        );
    } else {
        panic!(
            "Expected number for scaled, got {:?}",
            scaled_result.result()
        );
    }

    Ok(())
}

#[test]
fn test_complex_discount_scenario_rejected() {
    let code = r#"
        spec complex_pricing

        data base_price: 1000
        data bulk_discount: 15%

        rule after_bulk: base_price - bulk_discount
        "#;
    expect_plan_error(code, "scale explicitly");
}

#[test]
fn test_percentage_arithmetic() -> Result<(), lemma::Errors> {
    let mut engine = Engine::new();

    engine.load([(
        lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
            "percentage.lemma",
        ))),
        r#"
        spec percentage_ops

        data discount_a: 5%
        data discount_b: 10%
        data tax_rate: 15%
        data compound_rate: 20%

        rule combined_discount: discount_a + discount_b
        rule net_rate: tax_rate - discount_a
        rule compound: compound_rate * compound_rate
        rule price_ratio: compound_rate / discount_a
        "#
        .to_string(),
    )])?;

    let now = lemma::DateTimeValue::now();
    let response = engine
        .run(
            None,
            "percentage_ops",
            Some(&now),
            HashMap::new(),
            None,
            true,
        )
        .expect("run should succeed after load");

    // ratio + ratio = ratio (5% + 10% = 15%)
    let combined_result = response
        .results
        .values()
        .find(|r| r.rule.name == "combined_discount")
        .expect("combined_discount rule not found");

    let lit = combined_result
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("value");
    if let lemma::ValueKind::Ratio(_r) = &lit.value {
        assert_eq!(
            lit.value,
            lemma::LiteralValue::ratio_from_decimal(decimal_lit("0.15")).value
        );
    } else {
        panic!(
            "Expected percentage for combined_discount, got {:?}",
            combined_result.result()
        );
    }

    // ratio - ratio = ratio (15% - 5% = 10%)
    let net_rate_result = response
        .results
        .values()
        .find(|r| r.rule.name == "net_rate")
        .expect("net_rate rule not found");

    let lit = net_rate_result
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("value");
    if let lemma::ValueKind::Ratio(_r) = &lit.value {
        assert_eq!(
            lit.value,
            lemma::LiteralValue::ratio_from_decimal(decimal_lit("0.10")).value
        );
    } else {
        panic!(
            "Expected percentage for net_rate, got {:?}",
            net_rate_result.result()
        );
    }

    // ratio * ratio = ratio (20% * 20% = 4%)
    let compound_result = response
        .results
        .values()
        .find(|r| r.rule.name == "compound")
        .expect("compound rule not found");

    let lit = compound_result
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("value");
    if let lemma::ValueKind::Ratio(_r) = &lit.value {
        assert_eq!(
            lit.value,
            lemma::LiteralValue::ratio_from_decimal(decimal_lit("0.04")).value
        );
    } else {
        panic!(
            "Expected percentage for compound, got {:?}",
            compound_result.result()
        );
    }

    // ratio / ratio = ratio (20% / 5% = 4)
    let ratio_result = response
        .results
        .values()
        .find(|r| r.rule.name == "price_ratio")
        .expect("price_ratio rule not found");

    let lit = ratio_result
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("value");
    match &lit.value {
        lemma::ValueKind::Ratio(rational_val) => {
            assert_eq!(
                lemma::ValueKind::Number(rational_val.clone())
                    .as_decimal_magnitude()
                    .unwrap(),
                decimal_lit("4")
            );
            // Binding unit is on the rule type after migration, not LiteralValue.
        }
        _ => panic!(
            "Expected ratio for 20% / 5% (ratio / ratio = ratio), got {:?}",
            lit.value
        ),
    }

    Ok(())
}

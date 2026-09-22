use lemma::DateTimeValue;
use lemma::Engine;
use rust_decimal::Decimal;
use std::collections::HashMap;
/// Rule references work through one level of spec reference.
#[test]
fn test_single_level_spec_ref_with_rule_reference() {
    let mut engine = Engine::new();

    let base_spec = r#"
spec pricing
data base_price: 100
data tax_rate: 21%
rule tax_amount: base_price * tax_rate
rule final_price: base_price + tax_amount
"#;

    let line_item_spec = r#"
spec line_item
uses pricing
data quantity: 10
rule line_total: pricing.final_price * quantity
"#;

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "pricing.lemma",
            ))),
            base_spec.to_string(),
        )])
        .unwrap();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "line_item.lemma",
            ))),
            line_item_spec.to_string(),
        )])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "line_item", Some(&now), HashMap::new(), None, true)
        .unwrap();
    let line_total = response
        .results
        .values()
        .find(|r| r.rule.name == "line_total")
        .unwrap();

    // Should be: (100 * 1.21) * 10 = 1210
    assert!(!line_total.vetoed);
    let lit = line_total
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
            Decimal::from(1210)
        ),
        other => panic!("Expected Number for line_total, got {:?}", other),
    }
}

/// Multi-level spec rule references should work correctly.
/// When spec A references spec B which references spec C,
/// rule references through the chain should resolve properly.
#[test]
fn test_multi_level_spec_rule_reference() {
    let mut engine = Engine::new();

    let base_spec = r#"
spec base
data value: 100
rule doubled: value * 2
"#;

    let middle_spec = r#"
spec middle
uses base_ref: base
rule middle_calc: base_ref.doubled + 50
"#;

    let top_spec = r#"
spec top
uses middle_ref: middle
rule top_calc: middle_ref.middle_calc
"#;

    engine
        .load([(lemma::SourceType::Volatile, base_spec.to_string())])
        .unwrap();
    engine
        .load([(lemma::SourceType::Volatile, middle_spec.to_string())])
        .unwrap();
    engine
        .load([(lemma::SourceType::Volatile, top_spec.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "top", Some(&now), HashMap::new(), None, true)
        .unwrap();

    let top_calc = response
        .results
        .values()
        .find(|r| r.rule.name == "top_calc")
        .expect("top_calc rule not found in results");

    assert!(!top_calc.vetoed);
    let lit = top_calc
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
            Decimal::from(250)
        ),
        other => panic!("Expected Number for top_calc, got {:?}", other),
    }
}

/// `data X: spec Y` RHS is rejected; spec imports use `uses`.
#[test]
fn test_data_spec_shorthand_rejected() {
    let mut engine = Engine::new();

    let specs = r#"
spec a
data x: spec other
"#;

    let errs = engine
        .load([(lemma::SourceType::Volatile, specs.to_string())])
        .unwrap_err();
    let msg = errs
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("; ");
    assert!(
        (msg.contains("uses") && msg.contains("spec"))
            || msg.contains("Dotted paths require `with`"),
        "expected spec-import or dotted-LHS rejection, got: {msg}"
    );
}

/// Accessing data through multi-level spec references with nested bindings works correctly.
#[test]
fn test_multi_level_data_access_through_spec_refs() {
    let mut engine = Engine::new();

    let base_spec = r#"
spec base
data value: 50
"#;

    let middle_spec = r#"
spec middle
uses config: base
  -> with value: 100
"#;

    let top_spec = r#"
spec top
uses settings: middle
rule final_value: settings.config.value * 2
"#;

    engine
        .load([(lemma::SourceType::Volatile, base_spec.to_string())])
        .unwrap();
    engine
        .load([(lemma::SourceType::Volatile, middle_spec.to_string())])
        .unwrap();
    engine
        .load([(lemma::SourceType::Volatile, top_spec.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "top", Some(&now), HashMap::new(), None, true)
        .unwrap();
    let final_value = response
        .results
        .values()
        .find(|r| r.rule.name == "final_value")
        .unwrap();

    // Should be: 100 * 2 = 200 (using the overridden value from middle)
    assert!(!final_value.vetoed);
    let lit = final_value
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
            Decimal::from(200)
        ),
        other => panic!("Expected Number for final_value, got {:?}", other),
    }
}

/// Deep nested data bindings through multiple spec layers should work.
/// Overriding data like order.line.pricing.tax_rate through multiple levels.
#[test]
fn test_deep_nested_data_binding() {
    let mut engine = Engine::new();

    let pricing_spec = r#"
spec pricing
data base_price: 100
data tax_rate: 21%
rule tax_amount: base_price * tax_rate
rule final_price: base_price + tax_amount
"#;

    let line_item_spec = r#"
spec line_item
uses pricing
data quantity: 10
rule line_total: pricing.final_price * quantity
"#;

    let order_spec = r#"
spec order
uses line: line_item
  -> with pricing.tax_rate: 10%
  -> with quantity: 5
rule order_total: line.line_total
"#;

    engine
        .load([(lemma::SourceType::Volatile, pricing_spec.to_string())])
        .unwrap();
    engine
        .load([(lemma::SourceType::Volatile, line_item_spec.to_string())])
        .unwrap();
    engine
        .load([(lemma::SourceType::Volatile, order_spec.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "order", Some(&now), HashMap::new(), None, true)
        .unwrap();

    let order_total = response
        .results
        .values()
        .find(|r| r.rule.name == "order_total")
        .expect("order_total rule not found");

    // base_price=100, tax_rate=10% (overridden), quantity=5
    // (100 * 1.10) * 5 = 550
    assert!(!order_total.vetoed);
    let lit = order_total
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
            Decimal::from(550)
        ),
        other => panic!("Expected Number for order_total, got {:?}", other),
    }
}

/// Different data paths to the same base spec should produce different results
/// when bindings are applied. This tests that rule evaluation respects the specific
/// path through spec references.
#[test]
fn test_different_paths_different_results() {
    let mut engine = Engine::new();

    let base_spec = r#"
spec base
data price: 100
rule total: price * 1.21
"#;

    let wrapper_spec = r#"
spec wrapper
uses base
"#;

    let comparison_spec = r#"
spec comparison
uses path1: wrapper
uses path2: wrapper
  -> with base.price: 75
rule total1: path1.base.total
rule total2: path2.base.total
rule difference: total2 - total1
"#;

    engine
        .load([(lemma::SourceType::Volatile, base_spec.to_string())])
        .unwrap();
    engine
        .load([(lemma::SourceType::Volatile, wrapper_spec.to_string())])
        .unwrap();
    engine
        .load([(lemma::SourceType::Volatile, comparison_spec.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "comparison", Some(&now), HashMap::new(), None, true)
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
    let difference = response
        .results
        .values()
        .find(|r| r.rule.name == "difference")
        .unwrap();

    // path1: 100 * 1.21 = 121
    assert!(!total1.vetoed);
    let lit = total1
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
            Decimal::from(121)
        ),
        other => panic!("Expected Number for total1, got {:?}", other),
    }
    // path2: 75 * 1.21 = 90.75
    assert!(!total2.vetoed);
    let lit = total2
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
            Decimal::new(9075, 2)
        ),
        other => panic!("Expected Number for total2, got {:?}", other),
    }
    // difference: 90.75 - 121 = -30.25
    assert!(!difference.vetoed);
    let lit = difference
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
            Decimal::new(-3025, 2)
        ),
        other => panic!("Expected Number for difference, got {:?}", other),
    }
}

/// Multiple independent spec references in a single spec should all work.
/// Each reference should be independently resolvable.
#[test]
fn test_multiple_independent_spec_refs() {
    let mut engine = Engine::new();

    let config1_spec = r#"
spec config1
data value: 100
rule doubled: value * 2
"#;

    let config2_spec = r#"
spec config2
data value: 50
rule tripled: value * 3
"#;

    let combined_spec = r#"
spec combined
uses c1: config1
uses c2: config2
rule sum: c1.doubled + c2.tripled
rule product: c1.value * c2.value
"#;

    engine
        .load([(lemma::SourceType::Volatile, config1_spec.to_string())])
        .unwrap();
    engine
        .load([(lemma::SourceType::Volatile, config2_spec.to_string())])
        .unwrap();
    engine
        .load([(lemma::SourceType::Volatile, combined_spec.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "combined", Some(&now), HashMap::new(), None, true)
        .unwrap();

    let sum = response
        .results
        .values()
        .find(|r| r.rule.name == "sum")
        .unwrap();
    let product = response
        .results
        .values()
        .find(|r| r.rule.name == "product")
        .unwrap();

    // sum: (100 * 2) + (50 * 3) = 200 + 150 = 350
    assert!(!sum.vetoed);
    let lit = sum
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
            Decimal::from(350)
        ),
        other => panic!("Expected Number for sum, got {:?}", other),
    }
    // product: 100 * 50 = 5000
    assert!(!product.vetoed);
    let lit = product
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
            Decimal::from(5000)
        ),
        other => panic!("Expected Number for product, got {:?}", other),
    }
}

/// Referencing rules from a spec that itself has spec references.
/// This tests transitive rule dependencies across spec boundaries.
#[test]
fn test_transitive_rule_dependencies() {
    let mut engine = Engine::new();

    let base_spec = r#"
spec base
data x: 10
rule x_squared: x * x
"#;

    let middle_spec = r#"
spec middle
uses base_config: base
  -> with x: 20
rule x_squared_plus_ten: base_config.x_squared + 10
"#;

    let top_spec = r#"
spec top
uses middle_config: middle
rule final_result: middle_config.x_squared_plus_ten * 2
"#;

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("base.lemma"))),
            base_spec.to_string(),
        )])
        .unwrap();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "middle.lemma",
            ))),
            middle_spec.to_string(),
        )])
        .unwrap();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("top.lemma"))),
            top_spec.to_string(),
        )])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "top", Some(&now), HashMap::new(), None, true)
        .unwrap();

    let final_result = response
        .results
        .values()
        .find(|r| r.rule.name == "final_result")
        .unwrap();

    // x=20 (overridden), x_squared=400, x_squared_plus_ten=410, final=820
    assert!(!final_result.vetoed);
    let lit = final_result
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
            Decimal::from(820)
        ),
        other => panic!("Expected Number for final_result, got {:?}", other),
    }
}

/// Overriding the same spec reference in different ways should produce
/// different results based on the specific binding path.
#[test]
fn test_same_spec_different_bindings() {
    let mut engine = Engine::new();

    let pricing_spec = r#"
spec pricing
data price: 100
data discount: 0%
rule discount_amount: price * discount
rule final_price: price - discount_amount
"#;

    let scenario_spec = r#"
spec scenarios
uses retail: pricing
  -> with discount: 5%

uses wholesale: pricing
  -> with discount: 15%
  -> with price: 80

rule retail_final: retail.final_price
rule wholesale_final: wholesale.final_price
rule price_difference: retail_final - wholesale_final
"#;

    engine
        .load([(lemma::SourceType::Volatile, pricing_spec.to_string())])
        .unwrap();
    engine
        .load([(lemma::SourceType::Volatile, scenario_spec.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "scenarios", Some(&now), HashMap::new(), None, true)
        .unwrap();

    let retail_final = response
        .results
        .values()
        .find(|r| r.rule.name == "retail_final")
        .unwrap();
    let wholesale_final = response
        .results
        .values()
        .find(|r| r.rule.name == "wholesale_final")
        .unwrap();
    let price_difference = response
        .results
        .values()
        .find(|r| r.rule.name == "price_difference")
        .unwrap();

    // retail: 100 * (1 - 0.05) = 95
    assert!(!retail_final.vetoed);
    let lit = retail_final
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
            Decimal::from(95)
        ),
        other => panic!("Expected Number for retail_final, got {:?}", other),
    }
    // wholesale: 80 * (1 - 0.15) = 68
    assert!(!wholesale_final.vetoed);
    let lit = wholesale_final
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
            Decimal::from(68)
        ),
        other => panic!("Expected Number for wholesale_final, got {:?}", other),
    }
    // difference: 95 - 68 = 27
    assert!(!price_difference.vetoed);
    let lit = price_difference
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
            Decimal::from(27)
        ),
        other => panic!("Expected Number for price_difference, got {:?}", other),
    }
}

/// `data …: spec …` RHS is rejected even when the LHS is dotted (use `with`).
#[test]
fn test_data_spec_shorthand_rejected_with_dotted_lhs() {
    let mut engine = Engine::new();

    let specs = r#"
spec a
rule x: 5

spec c
uses aa: a
rule y: aa.x > 1

spec d
uses cc: c
data cc.aa: spec a
rule yy: cc.y
"#;

    let errs = engine
        .load([(lemma::SourceType::Volatile, specs.to_string())])
        .unwrap_err();
    let msg = errs
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("; ");
    assert!(
        (msg.contains("uses") && msg.contains("spec"))
            || msg.contains("Dotted paths require `with`"),
        "expected spec-import or dotted-LHS rejection, got: {msg}"
    );
}

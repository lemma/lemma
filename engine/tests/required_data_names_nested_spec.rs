use indexmap::IndexMap;
use lemma::{DateTimeValue, Engine};

#[test]
fn necessary_data_include_nested_spec_data_for_local_rule_deps() {
    let code = r#"
spec money
data money: measure
  -> unit eur: 1
  -> unit usd: 0.84

spec pricing
uses money
data quantity: 10
data is_member: false
data price: money.money
rule discount: 0%
  unless quantity >= 10 then 10%
  unless quantity >= 50 then 20%
  unless is_member then 15%
rule discount_amount: price * discount
rule total: price - discount_amount
  unless price < 50 eur then price

spec cashier
uses calc: pricing
rule total: calc.total
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let now = DateTimeValue::now();

    let show_all = engine.show(None, "cashier", Some(&now)).unwrap();
    assert!(
        show_all.data.contains_key("calc.price"),
        "Expected show data to include calc.price, got: {:?}",
        show_all.data.keys().collect::<Vec<_>>()
    );
    let price_type = &show_all.data.get("calc.price").unwrap().lemma_type;
    assert!(
        price_type.is_measure(),
        "Expected calc.price to be a measure type, got {:?}",
        price_type.name()
    );

    let response = engine
        .evaluate(
            None,
            "cashier",
            Some(&now),
            IndexMap::new(),
            Some(&["total".to_string()]),
            false,
        )
        .expect("evaluate must succeed");
    let total = response.results.get("total").expect("total rule");
    assert_eq!(
        total.missing_data(),
        &["calc.price"],
        "only unbound nested price (quantity/is_member have defaults)"
    );
    let show_price_type = &show_all.data.get("calc.price").unwrap().lemma_type;
    assert_eq!(
        show_price_type.name(),
        price_type.name(),
        "show must preserve nested measure type for calc.price"
    );
}

#[test]
fn evaluate_errors_on_unknown_rule() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Volatile,
            "spec test\ndata x: 1\nrule y: x".to_string(),
        )])
        .unwrap();
    let now = DateTimeValue::now();

    let result = engine.evaluate(
        None,
        "test",
        Some(&now),
        IndexMap::new(),
        Some(&["nonexistent".to_string()]),
        false,
    );
    assert!(result.is_err(), "Expected error for unknown rule");
    assert!(
        result.unwrap_err().to_string().contains("not found"),
        "Error should mention rule not found"
    );
}

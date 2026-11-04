use indexmap::IndexMap;
use lemma::Engine;

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

fn assert_display(response: &lemma::Response, rule: &str, expected: &str) {
    let result = response
        .results
        .get(rule)
        .unwrap_or_else(|| panic!("rule '{rule}' missing"));
    if result.vetoed {
        panic!(
            "rule '{rule}' vetoed: {}",
            result.veto_reason.as_deref().unwrap_or("Vetoed")
        );
    }
    assert_eq!(result.result().expect("result").to_string(), expected);
}

#[test]
fn test_money_minus_percentage_rejected() {
    let code = r#"
spec test_money_minus_percentage

data base_price: 200
data discount_rate: 25%

rule price_after_discount: base_price - discount_rate
"#;
    expect_plan_error(code, "scale explicitly");
}

#[test]
fn test_money_plus_percentage_rejected() {
    let code = r#"
spec test_money_plus_percentage

data base: 100
data markup: 10%

rule price_with_markup: base + markup
"#;
    expect_plan_error(code, "scale explicitly");
}

#[test]
fn test_number_times_percentage() {
    let mut engine = Engine::new();

    let code = r#"
spec test_number_times_percentage

data amount: 1000
data rate: 15%

rule result: amount * rate
rule expected: 150

rule test_passes: result is expected
"#;

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("test"))),
            code.to_string(),
        )])
        .unwrap();
    let now = lemma::DateTimeValue::now();
    let response = engine
        .evaluate(
            None,
            "test_number_times_percentage",
            Some(&now),
            IndexMap::new(),
            None,
            false,
        )
        .unwrap();

    assert_display(&response, "result", "150");
    assert_display(&response, "test_passes", "true");
}

#[test]
fn test_money_minus_percentage_with_rule_reference() {
    let mut engine = Engine::new();

    let code = r#"
spec test_with_rule_reference

data base_price: 200
data discount_rate: 25%

rule discount_amount: base_price * discount_rate
rule final_price: base_price - discount_amount
rule expected: 150

rule test_passes: final_price is expected
"#;

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("test"))),
            code.to_string(),
        )])
        .unwrap();
    let now = lemma::DateTimeValue::now();
    let response = engine
        .evaluate(
            None,
            "test_with_rule_reference",
            Some(&now),
            IndexMap::new(),
            None,
            false,
        )
        .unwrap();

    assert_display(&response, "discount_amount", "50");
    assert_display(&response, "final_price", "150");
}

#[test]
fn test_chained_percentage_operations_rejected() {
    let code = r#"
spec test_chained_percentages

data original_price: 100
data first_discount: 20%

rule after_first: original_price - first_discount
"#;
    expect_plan_error(code, "scale explicitly");
}

#[test]
fn distinct_measure_families_cannot_compare() {
    expect_plan_error(
        r#"
spec mix
data money: measure
  -> unit eur: 1
data mass: measure
  -> unit kg: 1
data a: 10 eur
data b: 5 kg
rule heavier: a > b
"#,
        "Cannot compare",
    );
}

#[test]
fn measure_siblings_add_generalizes_to_family_root() {
    let mut engine = Engine::new();
    let code = r#"
spec cash
data money: measure
  -> unit eur: 1
data deposit: money -> maximum 1000 eur
data fee: money -> minimum 1 eur
data d: deposit
data f: fee
rule total: d + f
"#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("spec must load");
    let mut inputs = IndexMap::new();
    inputs.insert("d".to_string(), "100 eur".into());
    inputs.insert("f".to_string(), "5 eur".into());
    let response = engine
        .evaluate(None, "cash", None, inputs, None, true)
        .expect("evaluate");
    let rr = response.results.get("total").expect("total");
    assert_eq!(
        rr.rule_type.as_str(),
        "money",
        "deposit + fee must generalize to family root money, got {}",
        rr.rule_type
    );
}

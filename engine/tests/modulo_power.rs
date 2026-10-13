use indexmap::IndexMap;
use lemma::DateTimeValue;
use lemma::*;

fn assert_rule_number(rule: &lemma::RuleResult, expected: rust_decimal::Decimal) {
    assert!(!rule.vetoed, "unexpected veto: {:?}", rule.veto_reason);
    let lit = rule
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("value");
    if let lemma::ValueKind::Number(n) = &lit.value {
        assert_eq!(
            lemma::ValueKind::Number(n.clone())
                .as_decimal_magnitude()
                .unwrap(),
            expected
        );
    } else {
        panic!("Expected number, got {:?}", lit.value);
    }
}

#[test]
fn test_modulo_simple() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("test"))),
            r#"
spec test
data a: 10
data b: 3
rule remainder: a mod b
"#
            .to_string(),
        )])
        .unwrap();
    let now = DateTimeValue::now();
    let response = engine
        .evaluate(None, "test", Some(&now), IndexMap::new(), None, true)
        .unwrap();
    let result = response.results.get("remainder").unwrap();
    assert_rule_number(result, rust_decimal::Decimal::ONE);
}

#[test]
fn test_power_simple() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("test"))),
            r#"
spec test
data base: 2
data exponent: 3
rule result: base ^ exponent
"#
            .to_string(),
        )])
        .unwrap();
    let now = DateTimeValue::now();
    let response = engine
        .evaluate(None, "test", Some(&now), IndexMap::new(), None, true)
        .unwrap();
    let result = response.results.get("result").unwrap();
    assert_rule_number(result, rust_decimal::Decimal::from(8));
}

#[test]
fn test_modulo_in_expression() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("test"))),
            r#"
spec test
data value: 17
rule is_even: (value mod 2) is 0
rule is_odd: (value mod 2) is 1
"#
            .to_string(),
        )])
        .unwrap();
    let now = DateTimeValue::now();
    let response = engine
        .evaluate(None, "test", Some(&now), IndexMap::new(), None, true)
        .unwrap();

    let is_even = response.results.get("is_even").unwrap();
    assert_eq!(
        is_even.result.as_ref().expect("rule result value").boolean,
        Some(false)
    );

    let is_odd = response.results.get("is_odd").unwrap();
    assert_eq!(
        is_odd.result.as_ref().expect("rule result value").boolean,
        Some(true)
    );
}

#[test]
fn test_power_with_fractions() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("test"))),
            r#"
spec test
data base: 4
rule square_root: base ^ 0.5
"#
            .to_string(),
        )])
        .unwrap();
    let now = DateTimeValue::now();
    let response = engine
        .evaluate(None, "test", Some(&now), IndexMap::new(), None, true)
        .unwrap();
    let result = response.results.get("square_root").unwrap();
    assert_rule_number(result, rust_decimal::Decimal::from(2));
}

#[test]
fn test_combined_operations() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("test"))),
            r#"
spec test
data x: 10
data y: 3
rule calculation: (x mod y) + (2 ^ 3)
"#
            .to_string(),
        )])
        .unwrap();
    let now = DateTimeValue::now();
    let response = engine
        .evaluate(None, "test", Some(&now), IndexMap::new(), None, true)
        .unwrap();
    let result = response.results.get("calculation").unwrap();
    assert_rule_number(result, rust_decimal::Decimal::from(9));
}

#[test]
fn test_modulo_negative_dividend_is_floored() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("test"))),
            r#"
spec test
data a: -7
data b: 3
rule remainder: a mod b
"#
            .to_string(),
        )])
        .unwrap();
    let now = DateTimeValue::now();
    let response = engine
        .evaluate(None, "test", Some(&now), IndexMap::new(), None, true)
        .unwrap();
    let result = response.results.get("remainder").unwrap();
    assert_rule_number(result, rust_decimal::Decimal::from(2));
}

#[test]
fn test_modulo_negative_divisor_is_floored() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("test"))),
            r#"
spec test
data a: 7
data b: -3
rule remainder: a mod b
"#
            .to_string(),
        )])
        .unwrap();
    let now = DateTimeValue::now();
    let response = engine
        .evaluate(None, "test", Some(&now), IndexMap::new(), None, true)
        .unwrap();
    let result = response.results.get("remainder").unwrap();
    assert_rule_number(result, rust_decimal::Decimal::from(-2));
}

#[test]
fn test_modulo_floored_identity() {
    // a == floor(a/b)*b + (a mod b) for both sign combinations.
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("test"))),
            r#"
spec test
data a: -7
data b: 3
data c: 7
data d: -3
rule rem_neg_dividend: a mod b
rule rem_neg_divisor: c mod d
"#
            .to_string(),
        )])
        .unwrap();
    let now = DateTimeValue::now();
    let response = engine
        .evaluate(None, "test", Some(&now), IndexMap::new(), None, true)
        .unwrap();
    assert_rule_number(
        response.results.get("rem_neg_dividend").unwrap(),
        rust_decimal::Decimal::from(2),
    );
    assert_rule_number(
        response.results.get("rem_neg_divisor").unwrap(),
        rust_decimal::Decimal::from(-2),
    );
}

use lemma::DateTimeValue;
use lemma::Engine;
use std::collections::HashMap;

#[test]
fn test_equal_operator_numbers() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Volatile,
            r#"
spec test_equal_numbers

data a: 42
data b: 42
data c: 100

rule equal_true: a is b
rule equal_false: a is c
"#
            .to_string(),
        )])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(
            None,
            "test_equal_numbers",
            Some(&now),
            HashMap::new(),
            None,
            false,
        )
        .unwrap();

    let equal_true = response.results.get("equal_true").unwrap();
    assert_eq!(equal_true.result().expect("result").to_string(), "true");

    let equal_false = response.results.get("equal_false").unwrap();
    assert_eq!(equal_false.result().expect("result").to_string(), "false");
}

#[test]
fn test_equal_operator_text() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Volatile,
            r#"
spec test_equal_text

data greeting: "hello"
data other: "world"

rule same_greeting: greeting is "hello"
rule different_greeting: greeting is other
"#
            .to_string(),
        )])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(
            None,
            "test_equal_text",
            Some(&now),
            HashMap::new(),
            None,
            false,
        )
        .unwrap();

    let same = response.results.get("same_greeting").unwrap();
    assert_eq!(same.result().expect("result").to_string(), "true");

    let different = response.results.get("different_greeting").unwrap();
    assert_eq!(different.result().expect("result").to_string(), "false");
}

#[test]
fn test_equal_operator_booleans() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Volatile,
            r#"
spec test_equal_booleans

data flag_a: true
data flag_b: true
data flag_c: false

rule both_true: flag_a is flag_b
rule mixed: flag_a is flag_c
"#
            .to_string(),
        )])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(
            None,
            "test_equal_booleans",
            Some(&now),
            HashMap::new(),
            None,
            false,
        )
        .unwrap();

    let both_true = response.results.get("both_true").unwrap();
    assert_eq!(both_true.result().expect("result").to_string(), "true");

    let mixed = response.results.get("mixed").unwrap();
    assert_eq!(mixed.result().expect("result").to_string(), "false");
}

#[test]
fn test_equal_operator_in_conditions() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Volatile,
            r#"
spec test_equal_conditions

data status: "active"
data count: 10

rule message: "inactive"
  unless status is "active" then "active"
  unless count is 10 then "count is 10"
"#
            .to_string(),
        )])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(
            None,
            "test_equal_conditions",
            Some(&now),
            HashMap::new(),
            None,
            false,
        )
        .unwrap();

    let message = response.results.get("message").unwrap();
    assert_eq!(message.result().expect("result").to_string(), "count is 10");
}

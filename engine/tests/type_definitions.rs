use lemma::DateTimeValue;
use lemma::{Engine, TypeSpecification};
use std::collections::HashMap;

#[test]
fn test_missing_uses_for_data_reference_fails() {
    let mut engine = Engine::new();

    let money_spec = r#"
spec money
data salary: number
"#;

    let test_spec = r#"
spec test
with salary: money.salary
rule total: salary
"#;

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("money.lemma"))),
            money_spec.to_string(),
        )])
        .unwrap();
    let err = engine
        .load([(lemma::SourceType::Volatile, test_spec.to_string())])
        .unwrap_err();

    let err_msg = err.errors[0].to_string();
    assert!(
        err_msg.contains("Standalone") || err_msg.contains("uses"),
        "local with without import path must be rejected at parse: {}",
        err_msg
    );
}

#[test]
fn test_type_system_with_imports_and_extensions() {
    let mut engine = Engine::new();

    let age_spec = r#"
spec age
data age: number
  -> minimum 0
  -> maximum 150
"#;

    let test_types_spec = r#"
spec test_types

uses age_spec: age
data age: age_spec.age

data adult_age: age
  -> minimum 21

data twenties: adult_age -> maximum 30

rule total: age + adult_age + twenties
"#;

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("age.lemma"))),
            age_spec.to_string(),
        )])
        .unwrap();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "test_types.lemma",
            ))),
            test_types_spec.to_string(),
        )])
        .unwrap();
    let now = DateTimeValue::now();

    let mut data = HashMap::new();
    data.insert("age".to_string(), "25".into());
    data.insert("adult_age".to_string(), "30".into());
    data.insert("twenties".to_string(), "25".into());
    let response = engine
        .run(None, "test_types", Some(&now), data, None, false)
        .expect("Evaluation failed");

    assert_eq!(response.spec_name, "test_types");

    let total_rule = response
        .results
        .values()
        .find(|r| r.rule.name == "total")
        .expect("total rule not found");

    // 25 + 30 + 25 = 80
    assert_eq!(total_rule.result().expect("result").to_string(), "80");
}

#[test]
fn test_import_literal_number_via_from_dependency_spec() {
    let mut engine = Engine::new();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "workspace.lemma",
            ))),
            r#"
spec constants
data pi: 3.14

spec finance
uses constants
data pi: constants.pi
rule x: pi
"#
            .to_string(),
        )])
        .expect("loading specs with literal + from-import must succeed");

    let now = DateTimeValue::now();
    let mut data = HashMap::new();
    data.insert("pi".to_string(), "3.14".into());
    let response = engine
        .run(None, "finance", Some(&now), data, None, false)
        .expect("run finance");

    let rule_x = response.results.get("x").expect("rule x");
    assert_eq!(rule_x.result().expect("result").to_string(), "3.14");
}

/// Regression test: measure type with `-> suggest` before `-> unit` must work.
/// Previously, constraints were applied in declaration order, so `default`
/// would fail to find the unit because it hadn't been registered yet.
#[test]
fn test_measure_type_default_before_unit_declarations() {
    let mut engine = Engine::new();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "pricing.lemma",
            ))),
            r#"
        spec pricing
        data money: measure
          -> suggest 4 eur
          -> unit eur: 1
          -> unit usd: 0.84
        data price: money
        rule doubled: price * 2
    "#
            .to_string(),
        )])
        .expect("default before unit should be valid");
    let now = DateTimeValue::now();

    let show = engine.show(None, "pricing", Some(&now)).unwrap();
    let entry = show.data.get("price").expect("price data in show");
    assert!(
        entry.lemma_type.is_measure(),
        "price must be measure money type"
    );
    assert_eq!(entry.lemma_type.extends.parent_name(), Some("money"));
    match &entry.lemma_type.specifications {
        TypeSpecification::Measure { units, .. } => {
            let names: Vec<&str> = units.iter().map(|u| u.name.as_str()).collect();
            assert!(names.contains(&"eur") && names.contains(&"usd"));
        }
        other => panic!("expected Measure, got {:?}", other),
    }
    assert!(
        entry.suggestion.is_some() && entry.fill.is_none(),
        "typedef money default must surface on price as show suggestion"
    );
}

/// Verify that `-> suggest` after `-> unit` (the original order) still works.
#[test]
fn test_measure_type_default_after_unit_declarations() {
    let mut engine = Engine::new();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "pricing.lemma",
            ))),
            r#"
        spec pricing
        data money: measure
          -> unit eur: 1
          -> unit usd: 0.84
          -> suggest 4 eur
        rule doubled: money * 2
    "#
            .to_string(),
        )])
        .expect("default after unit should be valid");
    let now = DateTimeValue::now();

    let show = engine.show(None, "pricing", Some(&now)).unwrap();
    let entry = show.data.get("money").expect("money data in show");
    assert!(
        entry.lemma_type.is_measure(),
        "money must be measure money type"
    );
    assert_eq!(entry.lemma_type.name(), "money");
    match &entry.lemma_type.specifications {
        TypeSpecification::Measure { units, .. } => {
            let names: Vec<&str> = units.iter().map(|u| u.name.as_str()).collect();
            assert!(names.contains(&"eur") && names.contains(&"usd"));
        }
        other => panic!("expected Measure, got {:?}", other),
    }
    assert!(
        entry.suggestion.is_some() && entry.fill.is_none(),
        "typedef money default must surface on price as show suggestion"
    );
}

#[test]
fn test_show_returns_data_in_definition_order() {
    let mut engine = Engine::new();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "ordering.lemma",
            ))),
            r#"
        spec ordering
        data zebra: number
        data alpha: number
        data middle: number
        rule total: zebra + alpha + middle
    "#
            .to_string(),
        )])
        .unwrap();
    let now = DateTimeValue::now();

    let show = engine.show(None, "ordering", Some(&now)).unwrap();
    let data_names: Vec<&String> = show.data.keys().collect();
    assert_eq!(
        data_names,
        vec!["zebra", "alpha", "middle"],
        "Data should be in definition order, not alphabetical"
    );
}

#[test]
fn test_run_returns_data_in_definition_order() {
    let mut engine = Engine::new();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "ordering.lemma",
            ))),
            r#"
        spec ordering
        data zebra: number
        data alpha: number
        data middle: number
        rule total: zebra + alpha + middle
    "#
            .to_string(),
        )])
        .unwrap();
    let now = DateTimeValue::now();

    let response = engine
        .run(
            None,
            "ordering",
            Some(&now),
            std::collections::HashMap::new(),
            Some(&["total".to_string()]),
            false,
        )
        .expect("run must succeed");
    let data_names = response
        .results
        .get("total")
        .expect("total rule")
        .missing_data()
        .to_vec();
    assert_eq!(
        data_names,
        vec!["zebra", "alpha", "middle"],
        "scoped run should preserve evaluation order for rule missing_data"
    );
}

#[test]
fn test_show_splits_prefilled_literal_and_suggestion() {
    let mut engine = Engine::new();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "defaults.lemma",
            ))),
            r#"
        spec defaults
        data quantity: number -> suggest 10
        data name: text
        data price: 99
        rule total: quantity * price
        rule label: name
    "#
            .to_string(),
        )])
        .unwrap();
    let now = DateTimeValue::now();

    let show = engine.show(None, "defaults", Some(&now)).unwrap();

    let quantity = show.data.get("quantity").expect("quantity should exist");
    assert!(
        quantity.suggestion.is_some() && quantity.fill.is_none(),
        "type-level default is a suggestion only"
    );

    let name = show.data.get("name").expect("name should exist");
    assert!(
        name.suggestion.is_none() && name.fill.is_none(),
        "type-only data without default has no fill value or suggestion"
    );

    let price = show.data.get("price").expect("price should exist");
    assert!(
        price.fill.is_some() && price.suggestion.is_none(),
        "explicit literal is fill, not a default suggestion"
    );
}

#[test]
fn test_show_measure_default_is_value() {
    let mut engine = Engine::new();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "salary.lemma",
            ))),
            r#"
        spec salary
        data money: measure
          -> unit eur: 1
          -> unit usd: 0.84
          -> suggest 3000 eur
        data salary: money
        rule doubled: salary * 2
    "#
            .to_string(),
        )])
        .unwrap();
    let now = DateTimeValue::now();

    let show = engine.show(None, "salary", Some(&now)).unwrap();

    let salary = show.data.get("salary").expect("salary should exist");
    assert!(
        salary.suggestion.is_some() && salary.fill.is_none(),
        "measure typedef default must surface as show suggestion on salary"
    );
}

/// Default declared on an inner typedef must propagate through all extending
/// types and land on the data binding's default, without the intermediate
/// types redeclaring it.
#[test]
fn test_typedef_default_inherits_through_extension_chain() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("chain.lemma"))),
            r#"
            spec chain
            data money: measure
              -> unit eur: 1
              -> suggest 4 eur
            data price: money
            data final_price: price
            rule doubled: final_price * 2
            "#
            .to_string(),
        )])
        .unwrap();
    let now = DateTimeValue::now();

    let show = engine.show(None, "chain", Some(&now)).unwrap();
    let final_price = show
        .data
        .get("final_price")
        .expect("final_price should exist");
    assert!(
        final_price.suggestion.is_some() && final_price.fill.is_none(),
        "typedef default declared on ancestor type must inherit as suggestion on leaf binding"
    );
}

#[test]
fn child_measure_cannot_change_inherited_unit_factor() {
    let mut engine = Engine::new();
    let err = engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("units.lemma"))),
            r#"
            spec money_type
            data money: measure
              -> unit eur: 1.00
              -> unit usd: 0.91
            data price: money
              -> unit usd: 1.00
            "#
            .to_string(),
        )])
        .expect_err("child must not change inherited usd factor");
    let joined = err
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("; ");
    assert!(
        joined.contains("usd")
            && (joined.contains("inherited") || joined.contains("cannot change")),
        "expected inherited unit factor rejection, got: {joined}"
    );
}

#[test]
fn child_measure_may_add_new_unit() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("units.lemma"))),
            r#"
            spec money_type
            data money: measure
              -> unit eur: 1.00
            data price: money
              -> unit usd: 0.91
            data amount: 100 usd
            rule r: amount as eur
            "#
            .to_string(),
        )])
        .expect("child may add new unit");
    let now = DateTimeValue::now();
    let response = engine
        .run(None, "money_type", Some(&now), HashMap::new(), None, false)
        .expect("run");
    let display = response
        .results
        .get("r")
        .and_then(|r| r.result().map(str::to_string))
        .expect("result");
    assert!(
        display.contains("eur"),
        "expected eur conversion display, got {display}"
    );
}

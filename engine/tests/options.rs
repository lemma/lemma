//! `-> options` is a constraint on any type, not a text-only list of strings.
//!
//! Inline literals and a single list-valued rule are the two forms. A value
//! must also satisfy minimum, maximum, decimals (number only), and length.

use indexmap::IndexMap;
use lemma::{DateTimeValue, Engine, Error, EvaluateDataValue, Response, SourceType};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

fn lit(value: impl Into<String>) -> EvaluateDataValue {
    EvaluateDataValue::Literal(value.into())
}

fn map(pairs: &[(&str, EvaluateDataValue)]) -> EvaluateDataValue {
    EvaluateDataValue::Map(
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.clone()))
            .collect(),
    )
}

fn list(values: Vec<EvaluateDataValue>) -> EvaluateDataValue {
    EvaluateDataValue::List(values)
}

fn data(pairs: &[(&str, EvaluateDataValue)]) -> IndexMap<String, EvaluateDataValue> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.clone()))
        .collect()
}

fn load_ok(code: &str) -> Engine {
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Path(Arc::new(PathBuf::from("options.lemma"))),
            code.to_string(),
        )])
        .unwrap_or_else(|errors| {
            panic!(
                "expected load to succeed, got: {}",
                errors
                    .iter()
                    .map(|error| error.to_string())
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        });
    engine
}

fn load_planning_err(code: &str) -> String {
    let mut engine = Engine::new();
    let errors = engine
        .load([(
            SourceType::Path(Arc::new(PathBuf::from("options.lemma"))),
            code.to_string(),
        )])
        .expect_err("expected load to fail");
    let parse = errors
        .iter()
        .filter(|error| matches!(error, Error::Parsing(_)))
        .map(|error| error.to_string())
        .collect::<Vec<_>>();
    assert!(
        parse.is_empty(),
        "expected planning Error, got parse Error: {}",
        parse.join("\n")
    );
    errors
        .iter()
        .map(|error| error.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

fn evaluate(engine: &Engine, spec: &str, inputs: IndexMap<String, EvaluateDataValue>) -> Response {
    let now = DateTimeValue::now();
    engine
        .evaluate(None, spec, Some(&now), inputs, None, false)
        .unwrap_or_else(|error| panic!("evaluate {spec}: {error}"))
}

fn value(response: &Response, rule: &str) -> String {
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
    result
        .result()
        .unwrap_or_else(|| panic!("rule '{rule}' has no result"))
        .to_string()
}

fn veto_reason(response: &Response, rule: &str) -> String {
    let result = response
        .results
        .get(rule)
        .unwrap_or_else(|| panic!("rule '{rule}' missing"));
    assert!(result.vetoed, "rule '{rule}' was not vetoed");
    result
        .veto_reason
        .clone()
        .unwrap_or_else(|| panic!("rule '{rule}' missing veto reason"))
}

fn assert_contains(haystack: &str, needle: &str) {
    assert!(
        haystack.contains(needle),
        "expected `{needle}` in `{haystack}`"
    );
}

fn load_parse_err(code: &str) -> String {
    let mut engine = Engine::new();
    let errors = engine
        .load([(
            SourceType::Path(Arc::new(PathBuf::from("options.lemma"))),
            code.to_string(),
        )])
        .expect_err("expected load to fail");
    let parse = errors
        .iter()
        .filter(|error| matches!(error, Error::Parsing(_)))
        .map(|error| error.to_string())
        .collect::<Vec<_>>();
    assert!(
        !parse.is_empty(),
        "expected parse Error, got: {}",
        errors
            .iter()
            .map(|error| error.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    );
    parse.join("\n")
}

fn plan_err(code: &str) -> String {
    load_planning_err(code)
}

fn person() -> &'static str {
    r#"
spec person
data name: text
data score: number
"#
}

fn assert_plans(code: &str) {
    load_ok(code);
}

fn eval_value(code: &str, spec: &str, inputs: &[(&str, lemma::EvaluateDataValue)]) -> String {
    let engine = load_ok(code);
    let response = evaluate(&engine, spec, data(inputs));
    value(&response, "picked")
}

fn eval_veto(code: &str, spec: &str, inputs: &[(&str, lemma::EvaluateDataValue)]) -> String {
    let engine = load_ok(code);
    let response = evaluate(&engine, spec, data(inputs));
    veto_reason(&response, "picked")
}

fn show_doc(code: &str, spec: &str) -> Value {
    let engine = load_ok(code);
    let now = DateTimeValue::now();
    let show = engine.show(None, spec, Some(&now)).expect("show");
    lemma::api::show_document(&engine, &show)
}

fn property<'a>(document: &'a Value, name: &str) -> &'a Value {
    &document["properties"][name]
}

// --- text, unchanged ---

#[test]
fn text_member_evaluates_and_outsider_vetoes() {
    let code = r#"
spec menu
data drink: text
  -> options "espresso", "latte"
rule picked:
  drink
"#;
    assert_eq!(
        eval_value(code, "menu", &[("drink", lit("espresso"))]),
        "espresso"
    );
    assert_contains(
        &eval_veto(code, "menu", &[("drink", lit("mocha"))]),
        "mocha",
    );
}

#[test]
fn text_without_options_accepts_any_string() {
    let code = r#"
spec menu
data drink: text
rule picked:
  drink
"#;
    assert_eq!(
        eval_value(code, "menu", &[("drink", lit("mocha"))]),
        "mocha"
    );
}

#[test]
fn text_option_and_options_both_append() {
    let code = r#"
spec menu
data drink: text
  -> options "espresso", "latte"
  -> option "mocha"
  -> options "a", "b"
rule picked:
  drink
"#;
    assert_eq!(
        eval_value(code, "menu", &[("drink", lit("mocha"))]),
        "mocha"
    );
    assert_eq!(eval_value(code, "menu", &[("drink", lit("a"))]), "a");
    assert_eq!(
        eval_value(code, "menu", &[("drink", lit("espresso"))]),
        "espresso"
    );
}

#[test]
fn text_option_longer_than_length_is_planning_error() {
    let err = plan_err(
        r#"
spec menu
data drink: text
  -> length 1
  -> options "a", "ab"
rule picked:
  drink
"#,
    );
    assert_contains(&err, "ab");
}

#[test]
fn text_length_and_options_both_apply() {
    let code = r#"
spec menu
data drink: text
  -> length 1
  -> options "a", "b"
rule picked:
  drink
"#;
    assert_eq!(eval_value(code, "menu", &[("drink", lit("a"))]), "a");
    assert_contains(&eval_veto(code, "menu", &[("drink", lit("ab"))]), "ab");
    assert_contains(&eval_veto(code, "menu", &[("drink", lit("c"))]), "c");
}

#[test]
fn text_suggest_and_with_must_be_in_options() {
    assert_plans(
        r#"
spec menu
data drink: text
  -> options "espresso", "latte"
  -> suggest "latte"
rule picked:
  drink
"#,
    );
    let suggest = plan_err(
        r#"
spec menu
data drink: text
  -> options "espresso", "latte"
  -> suggest "mocha"
rule picked:
  drink
"#,
    );
    assert_contains(&suggest, "mocha");
    assert_plans(
        r#"
spec menu
data drink: text
  -> options "espresso", "latte"
  -> with "latte"
rule picked:
  drink
"#,
    );
    let committed = plan_err(
        r#"
spec menu
data drink: text
  -> options "espresso", "latte"
  -> with "mocha"
rule picked:
  drink
"#,
    );
    assert_contains(&committed, "mocha");
}

#[test]
fn text_help_plans_beside_options() {
    assert_plans(
        r#"
spec menu
data drink: text
  -> help "Which drink."
  -> options "espresso", "latte"
rule picked:
  drink
"#,
    );
}

#[test]
fn quoted_text_option_is_not_a_rule_of_the_same_name() {
    let code = r#"
spec menu
data names: text list
  -> with "ada"
rule offered:
  names
data drink: text
  -> options "offered"
rule picked:
  drink
"#;
    assert_eq!(
        eval_value(code, "menu", &[("drink", lit("offered"))]),
        "offered"
    );
    assert_contains(&eval_veto(code, "menu", &[("drink", lit("ada"))]), "ada");
}

// --- number, measure, ratio, date, boolean ---

#[test]
fn number_options_accept_a_member_and_reject_an_outsider() {
    let code = r#"
spec scale
data tier: number
  -> options 1, 2
rule picked:
  tier
"#;
    assert_eq!(eval_value(code, "scale", &[("tier", lit("1"))]), "1");
    assert_contains(&eval_veto(code, "scale", &[("tier", lit("3"))]), "3");
}

#[test]
fn number_with_outside_options_is_planning_error() {
    let err = plan_err(
        r#"
spec scale
data tier: number
  -> options 1, 2
  -> with 3
rule picked:
  tier
"#,
    );
    assert_contains(&err, "allowed options");
}

#[test]
fn number_option_of_the_wrong_type_is_planning_error() {
    let err = plan_err(
        r#"
spec scale
data tier: number
  -> options "a"
rule picked:
  tier
"#,
    );
    assert_contains(&err, "a");
}

#[test]
fn number_singular_option_appends() {
    let code = r#"
spec scale
data tier: number
  -> options 1, 2
  -> option 3
rule picked:
  tier
"#;
    assert_eq!(eval_value(code, "scale", &[("tier", lit("3"))]), "3");
}

#[test]
fn number_option_below_minimum_is_planning_error() {
    let err = plan_err(
        r#"
spec scale
data tier: number
  -> minimum 5
  -> options 0, 5, 10
rule picked:
  tier
"#,
    );
    assert_contains(&err, "0");
    assert_contains(&err, "minimum");
}

#[test]
fn number_options_respect_minimum_and_maximum() {
    let code = r#"
spec scale
data tier: number
  -> options 5, 10
  -> minimum 5
  -> maximum 10
rule picked:
  tier
"#;
    assert_plans(
        r#"
spec scale
data tier: number
  -> options 5, 10
  -> minimum 5
  -> maximum 10
  -> with 5
rule picked:
  tier
"#,
    );
    let outside = plan_err(
        r#"
spec scale
data tier: number
  -> options 5, 10
  -> minimum 5
  -> maximum 10
  -> with 7
rule picked:
  tier
"#,
    );
    assert_contains(&outside, "7");
    assert_eq!(eval_value(code, "scale", &[("tier", lit("5"))]), "5");
    assert_contains(&eval_veto(code, "scale", &[("tier", lit("7"))]), "7");
    let below = eval_veto(code, "scale", &[("tier", lit("4"))]);
    assert_contains(&below, "minimum");
    assert_plans(
        r#"
spec scale
data tier: number
  -> options 5, 10
  -> minimum 5
  -> maximum 10
  -> suggest 10
rule picked:
  tier
"#,
    );
    let suggest = plan_err(
        r#"
spec scale
data tier: number
  -> options 5, 10
  -> minimum 5
  -> maximum 10
  -> suggest 0
rule picked:
  tier
"#,
    );
    assert_contains(&suggest, "0");
}

#[test]
fn number_child_minimum_rejects_inherited_option() {
    let err = plan_err(
        r#"
spec scale
data tier: number
  -> options 5, 10
  -> minimum 0
data tight: tier
  -> minimum 10
  -> with 5
rule picked:
  tight
"#,
    );
    assert_contains(&err, "5");
    assert_contains(&err, "minimum");
}

#[test]
fn number_child_option_appends_to_the_parent_set() {
    let code = r#"
spec scale
data tier: number
  -> options 5, 10
  -> minimum 0
data tight: tier
  -> option 7
rule picked:
  tight
"#;
    assert_eq!(eval_value(code, "scale", &[("tight", lit("10"))]), "10");
    assert_eq!(eval_value(code, "scale", &[("tight", lit("5"))]), "5");
    assert_eq!(eval_value(code, "scale", &[("tight", lit("7"))]), "7");
    let duplicate = plan_err(
        r#"
spec scale
data tier: number
  -> options 5, 10
  -> minimum 0
data tight: tier
  -> options 10
rule picked:
  tight
"#,
    );
    assert_contains(&duplicate, "duplicate");
}

#[test]
fn measure_options_combine_with_units_and_bounds() {
    let code = r#"
spec price_list
data money: measure
  -> unit eur: 1
  -> unit cent: 0.01
data price: money
  -> options 10 eur, 20 eur
  -> minimum 0 eur
  -> maximum 50 eur
rule picked:
  price
"#;
    assert_plans(
        r#"
spec price_list
data money: measure
  -> unit eur: 1
  -> unit cent: 0.01
data price: money
  -> options 10 eur, 20 eur
  -> minimum 0 eur
  -> maximum 50 eur
  -> with 10 eur
rule picked:
  price
"#,
    );
    let outside = plan_err(
        r#"
spec price_list
data money: measure
  -> unit eur: 1
data price: money
  -> options 10 eur, 20 eur
  -> with 5 eur
rule picked:
  price
"#,
    );
    assert_contains(&outside, "5");
    assert_eq!(
        eval_value(code, "price_list", &[("price", lit("10 eur"))]),
        "10 eur"
    );
    assert_eq!(
        eval_value(code, "price_list", &[("price", lit("1000 cent"))]),
        "1000 cent"
    );
    assert_contains(
        &eval_veto(code, "price_list", &[("price", lit("11 eur"))]),
        "11",
    );
    assert_contains(
        &eval_veto(code, "price_list", &[("price", lit("10.001 eur"))]),
        "10.001",
    );
    let above = plan_err(
        r#"
spec price_list
data money: measure
  -> unit eur: 1
data price: money
  -> maximum 50 eur
  -> options 60 eur
rule picked:
  price
"#,
    );
    assert_contains(&above, "60");
    assert_contains(&above, "maximum");
}

#[test]
fn ratio_options_respect_minimum_and_maximum() {
    let code = r#"
spec rates
data rate: ratio
  -> options 10%, 20%
  -> minimum 0%
  -> maximum 100%
rule picked:
  rate
"#;
    assert_plans(
        r#"
spec rates
data rate: ratio
  -> options 10%, 20%
  -> minimum 0%
  -> maximum 100%
  -> with 10%
rule picked:
  rate
"#,
    );
    let outside = plan_err(
        r#"
spec rates
data rate: ratio
  -> options 10%, 20%
  -> with 15%
rule picked:
  rate
"#,
    );
    assert_contains(&outside, "15");
    let above = plan_err(
        r#"
spec rates
data rate: ratio
  -> maximum 100%
  -> options 200%
rule picked:
  rate
"#,
    );
    assert_contains(&above, "200");
    assert_contains(&above, "maximum");
    assert_eq!(eval_value(code, "rates", &[("rate", lit("10%"))]), "10%");
}

#[test]
fn date_options_respect_minimum_and_maximum() {
    let code = r#"
spec calendar
data day: date
  -> options 2024-01-01, 2024-06-01
  -> minimum 2024-01-01
  -> maximum 2024-12-31
rule picked:
  day
"#;
    assert_plans(
        r#"
spec calendar
data day: date
  -> options 2024-01-01, 2024-06-01
  -> minimum 2024-01-01
  -> maximum 2024-12-31
  -> with 2024-01-01
rule picked:
  day
"#,
    );
    let outside = plan_err(
        r#"
spec calendar
data day: date
  -> options 2024-01-01, 2024-06-01
  -> with 2024-03-01
rule picked:
  day
"#,
    );
    assert_contains(&outside, "2024-03-01");
    let before = plan_err(
        r#"
spec calendar
data day: date
  -> minimum 2024-01-01
  -> options 2023-12-31, 2024-01-01
rule picked:
  day
"#,
    );
    assert_contains(&before, "2023-12-31");
    assert_contains(&before, "minimum");
    assert_eq!(
        eval_value(code, "calendar", &[("day", lit("2024-06-01"))]),
        "2024-06-01"
    );
}

#[test]
fn boolean_options_reject_the_other_value() {
    let code = r#"
spec flags
data open: boolean
  -> options true
rule picked:
  open
"#;
    assert_plans(
        r#"
spec flags
data open: boolean
  -> options true
  -> with true
rule picked:
  open
"#,
    );
    let committed = plan_err(
        r#"
spec flags
data open: boolean
  -> options true
  -> with false
rule picked:
  open
"#,
    );
    assert_contains(&committed, "false");
    assert_contains(
        &eval_veto(code, "flags", &[("open", lit("false"))]),
        "false",
    );
}

// --- specs and lists ---

fn drinks() -> &'static str {
    r#"
spec drink
data name: text
data price: measure
  -> unit eur: 1.00

spec order
uses espresso: drink
  -> with name: "espresso", price: 2.50 eur
uses latte: drink
  -> with name: "latte", price: 3.50 eur
uses mocha: drink
  -> with name: "mocha", price: 4.00 eur
"#
}

#[test]
fn spec_list_options_are_a_planning_error() {
    let message = "`-> option` and `-> options` do not apply to a list of spec instances";
    let imported = plan_err(&format!(
        r#"
{}
spec contest
uses person
data people: person list
  -> option name: "Ben", score: 54
rule picked:
  people -> count
"#,
        person()
    ));
    assert_contains(&imported, message);
    let nested = plan_err(
        r#"
spec address
data city: text

spec person
data name: text
uses addr: address

spec contest
uses person
data people: person list
  -> option name: "Ben", addr.city: "Utrecht"
rule picked:
  people -> count
"#,
    );
    assert_contains(&nested, message);
    let self_list = plan_err(
        r#"
spec person
data name: text
data score: number
data roster: person list
  -> option name: "Ben", score: 54
rule picked:
  roster -> count
"#,
    );
    assert_contains(&self_list, message);
    let rule = plan_err(&format!(
        r#"
{}
spec contest
uses person
data menu: person list
  -> with name: "Ben", score: 54
rule offered:
  menu
data people: person list
  -> options offered
rule picked:
  people -> count
"#,
        person()
    ));
    assert_contains(&rule, message);
    let aliases = plan_err(&format!(
        "{}\nuses drink\ndata rows: drink list\n  -> options espresso, latte\nrule picked:\n  rows -> count\n",
        drinks()
    ));
    assert_contains(&aliases, message);
    let wrong_spec = plan_err(
        r#"
spec other
data x: number
spec drink
data name: text
spec order
uses other
uses drink
uses foreign: other
data aliens: other list
  -> with x: 1
rule offered:
  aliens
data rows: drink list
  -> options offered
rule picked:
  rows -> count
"#,
    );
    assert_contains(&wrong_spec, message);
}

#[test]
fn spec_options_parentheses_are_a_parse_error() {
    let err = load_parse_err(&format!(
        r#"
{}
spec contest
uses person
data people: person list
  -> options (name: "Ben", score: 54), (name: "Jack", score: 58)
rule picked:
  people -> count
"#,
        person()
    ));
    assert_contains(&err, "value");
}

#[test]
fn spec_list_rejects_minimum_and_accepts_help() {
    let err = plan_err(&format!(
        r#"
{}
spec contest
uses person
data people: person list
  -> minimum 1
rule picked:
  people -> count
"#,
        person()
    ));
    assert_contains(&err, "minimum");
    assert_plans(&format!(
        r#"
{}
spec contest
uses person
data people: person list
  -> help "Contestants."
rule picked:
  people -> count
"#,
        person()
    ));
}

#[test]
fn number_list_options_check_each_row() {
    assert_plans(
        r#"
spec scale
data order: number list
  -> options 1, 2
  -> with 1
rule picked:
  order -> sum
"#,
    );
    assert_plans(
        r#"
spec scale
data order: number list
  -> options 1, 2
  -> with 1, 2
rule picked:
  order -> sum
"#,
    );
    let one_bad = plan_err(
        r#"
spec scale
data order: number list
  -> options 1, 2
  -> with 3
rule picked:
  order -> sum
"#,
    );
    assert_contains(&one_bad, "3");
    let mixed = plan_err(
        r#"
spec scale
data order: number list
  -> options 1, 2
  -> with 1, 3
rule picked:
  order -> sum
"#,
    );
    assert_contains(&mixed, "3");
}

#[test]
fn named_type_options_apply_to_its_list() {
    assert_plans(
        r#"
spec scale
data tier: number
  -> options 1, 2
  -> minimum 0
  -> maximum 10
data order: tier list
  -> with 1
rule picked:
  order -> sum
"#,
    );
    let err = plan_err(
        r#"
spec scale
data tier: number
  -> options 1, 2
  -> minimum 0
  -> maximum 10
data order: tier list
  -> with 3
rule picked:
  order -> sum
"#,
    );
    assert_contains(&err, "3");
    let code = r#"
spec scale
data tier: number
  -> options 1, 2
  -> minimum 0
  -> maximum 10
data order: tier list
rule picked:
  order -> sum
"#;
    assert_eq!(
        eval_value(code, "scale", &[("order", list(vec![lit("1"), lit("2")]))]),
        "3"
    );
    assert_contains(
        &eval_veto(code, "scale", &[("order", list(vec![lit("1"), lit("3")]))]),
        "3",
    );
}

#[test]
fn list_element_options_and_bounds_both_apply() {
    assert_plans(
        r#"
spec scale
data tier: number
  -> options 5, 10
  -> minimum 5
  -> maximum 10
data order: tier list
  -> with 5
rule picked:
  order -> sum
"#,
    );
    let below = plan_err(
        r#"
spec scale
data tier: number
  -> options 5, 10
  -> minimum 5
  -> maximum 10
data order: tier list
  -> with 0
rule picked:
  order -> sum
"#,
    );
    assert_contains(&below, "0");
    let code = r#"
spec scale
data tier: number
  -> options 5, 10
  -> minimum 5
  -> maximum 10
data order: tier list
rule picked:
  order -> sum
"#;
    assert_contains(
        &eval_veto(code, "scale", &[("order", list(vec![lit("10"), lit("4")]))]),
        "minimum",
    );
}

#[test]
fn measure_list_options_and_minimum() {
    assert_plans(
        r#"
spec price_list
data money: measure
  -> unit eur: 1
  -> options 10 eur, 20 eur
  -> minimum 0 eur
data order: money list
  -> with 10 eur
rule picked:
  order -> sum
"#,
    );
    let err = plan_err(
        r#"
spec price_list
data money: measure
  -> unit eur: 1
  -> options 10 eur, 20 eur
  -> minimum 0 eur
data order: money list
  -> with 15 eur
rule picked:
  order -> sum
"#,
    );
    assert_contains(&err, "15");
}

#[test]
fn text_key_outside_options_is_planning_error() {
    let err = plan_err(
        r#"
spec item
data name: text
  -> options "espresso", "latte"
data price: number
spec menu
uses item
data rows: item list
  -> key name
  -> with name: "mocha", price: 1
rule picked:
  rows -> count
"#,
    );
    assert_contains(&err, "mocha");
}

#[test]
fn number_key_outside_options_is_planning_error() {
    let err = plan_err(
        r#"
spec rate
data band: number
  -> options 1, 2
data price: number
spec table
uses rate
data rows: rate list
  -> key band
  -> with band: 3, price: 1
rule picked:
  rows -> count
"#,
    );
    assert_contains(&err, "allowed options");
}

#[test]
fn list_help_plans_beside_options() {
    assert_plans(
        r#"
spec scale
data order: number list
  -> help "Chosen tiers."
  -> options 1, 2
rule picked:
  order -> count
"#,
    );
}

// --- rules ---

#[test]
fn rule_options_use_the_list_from_this_run() {
    let code = r#"
spec scale
data numbers: number list
rule offered:
  numbers
data tier: number
  -> options offered
rule picked:
  tier
"#;
    assert_eq!(
        eval_value(
            code,
            "scale",
            &[
                ("numbers", list(vec![lit("1"), lit("2")])),
                ("tier", lit("1"))
            ]
        ),
        "1"
    );
    assert_contains(
        &eval_veto(
            code,
            "scale",
            &[
                ("numbers", list(vec![lit("1"), lit("2")])),
                ("tier", lit("3")),
            ],
        ),
        "3",
    );
    assert_contains(
        &eval_veto(
            code,
            "scale",
            &[("numbers", list(vec![])), ("tier", lit("1"))],
        ),
        "1",
    );
    let missing = eval_veto(code, "scale", &[("tier", lit("1"))]);
    assert_contains(&missing, "1");
    assert_contains(&missing, "allowed options");
}

#[test]
fn rule_option_row_still_has_to_meet_minimum() {
    let code = r#"
spec scale
data numbers: number list
rule offered:
  numbers
data tier: number
  -> options offered
  -> minimum 5
  -> maximum 10
rule picked:
  tier
"#;
    let rows = ("numbers", list(vec![lit("1"), lit("5")]));
    assert_eq!(
        eval_value(code, "scale", &[rows.clone(), ("tier", lit("5"))]),
        "5"
    );
    assert_contains(
        &eval_veto(code, "scale", &[rows.clone(), ("tier", lit("1"))]),
        "minimum",
    );
    assert_contains(&eval_veto(code, "scale", &[rows, ("tier", lit("6"))]), "6");
}

#[test]
fn rule_options_of_the_wrong_shape_are_planning_errors() {
    let scalar = plan_err(
        r#"
spec scale
rule offered:
  1
data tier: number
  -> options offered
rule picked:
  tier
"#,
    );
    assert_contains(&scalar, "offered");
    let other = plan_err(
        r#"
spec scale
data names: text list
  -> with "a"
rule offered:
  names
data tier: number
  -> options offered
rule picked:
  tier
"#,
    );
    assert_contains(&other, "offered");
}

#[test]
fn literals_and_a_rule_together_are_a_planning_error() {
    let err = plan_err(
        r#"
spec scale
data numbers: number list
rule offered:
  numbers
data tier: number
  -> options 1, offered
rule picked:
  tier
"#,
    );
    assert_contains(&err, "rule");
}

#[test]
fn duplicate_option_literals_are_a_planning_error() {
    let err = plan_err(
        r#"
spec scale
data tier: number
  -> options 1, 1
rule picked:
  tier
"#,
    );
    assert_contains(&err, "duplicate");
}

#[test]
fn bare_name_that_is_both_a_text_option_and_a_rule_is_a_planning_error() {
    let err = plan_err(
        r#"
spec menu
data names: text list
  -> with "ada"
rule offered:
  names
data drink: text
  -> options offered
rule picked:
  drink
"#,
    );
    assert_contains(&err, "offered");
}

#[test]
fn rule_options_add_to_literal_options() {
    let code = r#"
spec scale
data numbers: number list
rule offered:
  numbers
data tier: number
  -> options 1, 2
  -> options offered
rule picked:
  tier
"#;
    let numbers = ("numbers", list(vec![lit("8"), lit("9")]));
    assert_eq!(
        eval_value(code, "scale", &[numbers.clone(), ("tier", lit("8"))]),
        "8"
    );
    assert_eq!(
        eval_value(code, "scale", &[numbers, ("tier", lit("1"))]),
        "1"
    );
    assert_contains(
        &eval_veto(
            code,
            "scale",
            &[("numbers", list(vec![lit("8")])), ("tier", lit("3"))],
        ),
        "3",
    );
}

// --- show ---

fn document_property(code: &str, spec: &str, name: &str) -> Value {
    property(&show_doc(code, spec), name).clone()
}

#[test]
fn show_text_options_stay_enum_beside_help() {
    let drink = document_property(
        r#"
spec menu
data drink: text
  -> help "Which drink."
  -> options "espresso", "latte"
rule picked:
  drink
"#,
        "menu",
        "drink",
    );
    assert_eq!(drink["enum"], json!(["espresso", "latte"]));
    assert_eq!(drink["description"], json!("Which drink."));
    assert!(drink["x-lemma-type"].get("options").is_none());
}

#[test]
fn show_number_options_publish_the_set_beside_bounds() {
    let tier = document_property(
        r#"
spec scale
data tier: number
  -> options 5, 10
  -> minimum 5
  -> maximum 10
rule picked:
  tier
"#,
        "scale",
        "tier",
    );
    assert_eq!(tier["enum"], json!(["5", "10"]));
    assert_eq!(tier["x-lemma-type"]["minimum"], json!("5"));
    assert_eq!(tier["x-lemma-type"]["maximum"], json!("10"));
}

#[test]
fn show_measure_options_publish_the_set_beside_units() {
    let price = document_property(
        r#"
spec price_list
data money: measure
  -> unit eur: 1
  -> unit cent: 0.01
data price: money
  -> options 10 eur, 20 eur
rule picked:
  price
"#,
        "price_list",
        "price",
    );
    assert_eq!(price["enum"], json!(["10 eur", "20 eur"]));
    assert!(price["x-lemma-type"].get("decimals").is_none());
    assert!(price["x-lemma-type"]["units"].is_array());
}

#[test]
fn show_number_list_options_publish_enum_on_items() {
    let order = document_property(
        r#"
spec scale
data order: number list
  -> options 1, 2
rule picked:
  order -> count
"#,
        "scale",
        "order",
    );
    assert_eq!(order["items"]["enum"], json!(["1", "2"]));
}

#[test]
fn show_rule_options_have_no_enum_and_keep_bounds() {
    let tier = document_property(
        r#"
spec scale
data numbers: number list
rule offered:
  numbers
data tier: number
  -> options offered
  -> minimum 5
  -> maximum 10
rule picked:
  tier
"#,
        "scale",
        "tier",
    );
    assert!(tier.get("enum").is_none());
    assert_eq!(tier["x-lemma-type"]["minimum"], json!("5"));
    assert_eq!(tier["x-lemma-type"]["maximum"], json!("10"));
}

fn rule<'a>(response: &'a Response, name: &str) -> &'a lemma::RuleResult {
    response
        .results
        .get(name)
        .unwrap_or_else(|| panic!("rule '{name}' missing"))
}

fn options<'a>(response: &'a Response, name: &str, key: &str) -> &'a [EvaluateDataValue] {
    rule(response, name)
        .missing_data_options()
        .get(key)
        .unwrap_or_else(|| panic!("{name} has no missing_data_options for {key}"))
}

#[test]
fn missing_data_options_lists_a_static_text_set() {
    let code = r#"
spec menu
data drink: text
  -> options "espresso", "latte"
rule picked:
  drink
"#;
    let engine = load_ok(code);
    let response = evaluate(&engine, "menu", data(&[]));
    let picked = rule(&response, "picked");
    assert_eq!(picked.missing_data(), &["drink".to_string()]);
    assert_eq!(
        options(&response, "picked", "drink"),
        &[lit("espresso"), lit("latte")]
    );
    assert_eq!(
        eval_value(code, "menu", &[("drink", lit("espresso"))]),
        "espresso"
    );
    assert_contains(
        &eval_veto(code, "menu", &[("drink", lit("mocha"))]),
        "mocha",
    );
}

#[test]
fn missing_input_without_a_set_has_no_missing_data_options_entry() {
    let code = r#"
spec menu
data drink: text
rule picked:
  drink
"#;
    let engine = load_ok(code);
    let response = evaluate(&engine, "menu", data(&[]));
    let picked = rule(&response, "picked");
    assert_eq!(picked.missing_data(), &["drink".to_string()]);
    assert!(picked.missing_data_options().get("drink").is_none());
}

#[test]
fn missing_data_options_appends_the_rule_list_after_static_options() {
    let code = r#"
spec scale
data low: number
data high: number
data base: number list
rule offered:
  base -> with low -> with high
data tier: number
  -> options 1, 2
  -> options offered
rule picked:
  tier
"#;
    let open = evaluate(&load_ok(code), "scale", data(&[]));
    let open_missing = rule(&open, "picked").missing_data();
    assert!(
        open_missing.iter().any(|key| key == "low"),
        "{open_missing:?}"
    );
    assert!(
        open_missing.iter().any(|key| key == "high"),
        "{open_missing:?}"
    );
    assert!(
        open_missing.iter().all(|key| key != "tier"),
        "{open_missing:?}"
    );
    assert!(rule(&open, "picked")
        .missing_data_options()
        .get("tier")
        .is_none());

    let closed = evaluate(
        &load_ok(code),
        "scale",
        data(&[("low", lit("8")), ("high", lit("9"))]),
    );
    assert_eq!(
        rule(&closed, "picked").missing_data(),
        &["tier".to_string()]
    );
    assert_eq!(
        options(&closed, "picked", "tier"),
        vec![lit("1"), lit("2"), lit("8"), lit("9")]
    );

    let repeated = evaluate(
        &load_ok(code),
        "scale",
        data(&[("low", lit("1")), ("high", lit("8"))]),
    );
    assert_eq!(
        options(&repeated, "picked", "tier"),
        vec![lit("1"), lit("2"), lit("8")]
    );
    assert_eq!(
        eval_value(
            code,
            "scale",
            &[("low", lit("8")), ("high", lit("9")), ("tier", lit("8"))],
        ),
        "8"
    );
}

#[test]
fn missing_data_options_is_empty_when_the_rule_returns_no_rows() {
    let code = r#"
spec scale
data numbers: number list
rule offered:
  numbers
data tier: number
  -> options offered
rule picked:
  tier
"#;
    let response = evaluate(&load_ok(code), "scale", data(&[("numbers", list(vec![]))]));
    assert_eq!(
        rule(&response, "picked").missing_data(),
        &["tier".to_string()]
    );
    assert!(options(&response, "picked", "tier").is_empty());
    assert_contains(
        &eval_veto(
            code,
            "scale",
            &[("numbers", list(vec![])), ("tier", lit("1"))],
        ),
        "1",
    );
}

#[test]
fn missing_data_options_is_per_rule() {
    let code = r#"
spec menu
data drink: text
  -> options "espresso", "latte"
rule picked:
  drink
rule other:
  "ok"
"#;
    let response = evaluate(&load_ok(code), "menu", data(&[]));
    assert_eq!(
        options(&response, "picked", "drink"),
        vec![lit("espresso"), lit("latte")]
    );
    assert!(rule(&response, "other").missing_data().is_empty());
    assert!(rule(&response, "other").missing_data_options().is_empty());
}

#[test]
fn spec_list_is_an_empty_list_not_missing_data() {
    let code = format!(
        r#"
{}
spec contest
uses person
data guests: person list
rule picked:
  (guests -> first).name
"#,
        person()
    );
    let response = evaluate(&load_ok(&code), "contest", data(&[]));
    let picked = rule(&response, "picked");
    assert!(picked.missing_data().is_empty());
    assert!(picked.missing_data_options().is_empty());
    assert_contains(
        &veto_reason(&response, "picked"),
        "Cannot take the first element of an empty list.",
    );
    let row = map(&[("name", lit("Ben")), ("score", lit("54"))]);
    assert_eq!(
        eval_value(&code, "contest", &[("guests", list(vec![row]))]),
        "Ben"
    );
    let nested = format!(
        r#"
{}
spec party
uses person
data guests: person list

spec contest
uses party
rule picked:
  (party.guests -> first).name
"#,
        person()
    );
    let response = evaluate(&load_ok(&nested), "contest", data(&[]));
    let picked = rule(&response, "picked");
    assert!(picked.missing_data().is_empty());
    assert!(picked.missing_data_options().is_empty());
    assert_contains(
        &veto_reason(&response, "picked"),
        "Cannot take the first element of an empty list.",
    );
}

#[test]
fn show_lists_an_options_rule_input_under_the_reading_rule() {
    let document = show_doc(
        r#"
spec trip
data region: number list
rule offered:
  region
data destination: number
  -> option 1
  -> options offered
rule price:
  destination
"#,
        "trip",
    );
    let needed = document["properties"]["region"]["x-lemma-needed-by-rules"]
        .as_array()
        .expect("region needed_by_rules");
    assert!(
        needed.iter().any(|rule| rule == "price"),
        "price should need region, got {needed:?}"
    );
}

#[test]
fn missing_data_options_of_a_uses_field_follow_its_options_rule() {
    let code = r#"
spec trip
data region: number list
data extra: number
rule offered:
  region -> with extra
data destination: number
  -> option 1
  -> options offered
rule price:
  destination

spec booking
uses t: trip
rule cost:
  t.price
"#;
    let closed = evaluate(
        &load_ok(code),
        "booking",
        data(&[("t.region", list(vec![lit("2")])), ("t.extra", lit("3"))]),
    );
    assert_eq!(
        rule(&closed, "cost").missing_data(),
        &["t.destination".to_string()]
    );
    assert_eq!(
        options(&closed, "cost", "t.destination"),
        vec![lit("1"), lit("2"), lit("3")]
    );

    let open = evaluate(&load_ok(code), "booking", data(&[]));
    assert_eq!(rule(&open, "cost").missing_data(), &["t.extra".to_string()]);
    assert!(rule(&open, "cost")
        .missing_data_options()
        .get("t.destination")
        .is_none());
}

#[test]
fn imported_options_rule_input_is_needed_by_the_reading_rule() {
    let document = show_doc(
        r#"
spec trip
data region: number list
rule offered:
  region
data destination: number
  -> option 1
  -> options offered
rule price:
  destination

spec booking
uses t: trip
rule cost:
  t.price
rule offered:
  7
"#,
        "booking",
    );
    let needed = document["properties"]["t.region"]["x-lemma-needed-by-rules"]
        .as_array()
        .expect("t.region needed_by_rules");
    assert!(
        needed.iter().any(|rule| rule == "cost"),
        "cost should need t.region through t.offered, got {needed:?}"
    );
    let response = evaluate(
        &load_ok(
            r#"
spec trip
data region: number list
rule offered:
  region
data destination: number
  -> option 1
  -> options offered
rule price:
  destination

spec booking
uses t: trip
rule cost:
  t.price
rule offered:
  cost + 1
"#,
        ),
        "booking",
        data(&[
            ("t.region", list(vec![lit("2"), lit("3")])),
            ("t.destination", lit("3")),
        ]),
    );
    assert_eq!(value(&response, "cost"), "3");
    assert_eq!(value(&response, "offered"), "4");
}

fn response_json(response: &Response) -> Value {
    serde_json::to_value(lemma::api::Response::from(response)).expect("response JSON")
}

#[test]
fn missing_data_options_for_a_measure_is_the_source_spelling() {
    let code = r#"
spec price_list
data money: measure
  -> unit eur: 1
data price: money
  -> options 10 eur, 20 eur
rule picked:
  price
"#;
    let response = evaluate(&load_ok(code), "price_list", data(&[]));
    assert_eq!(
        rule(&response, "picked").missing_data(),
        &["price".to_string()]
    );
    assert_eq!(
        options(&response, "picked", "price"),
        vec![lit("10 eur"), lit("20 eur")]
    );
    let submitted = evaluate(
        &load_ok(code),
        "price_list",
        data(&[("price", lit("10 eur"))]),
    );
    assert!(
        !rule(&submitted, "picked").vetoed,
        "{:?}",
        rule(&submitted, "picked").veto_reason
    );
}

#[test]
fn missing_data_options_for_a_ratio_is_the_source_spelling() {
    let code = r#"
spec rates
data rate: ratio
  -> options 10%, 20%
rule picked:
  rate
"#;
    let response = evaluate(&load_ok(code), "rates", data(&[]));
    assert_eq!(
        rule(&response, "picked").missing_data(),
        &["rate".to_string()]
    );
    assert_eq!(
        options(&response, "picked", "rate"),
        vec![lit("10%"), lit("20%")]
    );
    assert_eq!(eval_value(code, "rates", &[("rate", lit("10%"))]), "10%");
}

#[test]
fn missing_data_options_for_a_date_is_the_source_spelling() {
    let code = r#"
spec calendar
data day: date
  -> options 2024-01-01, 2024-06-01
rule picked:
  day
"#;
    let response = evaluate(&load_ok(code), "calendar", data(&[]));
    assert_eq!(
        rule(&response, "picked").missing_data(),
        &["day".to_string()]
    );
    assert_eq!(
        options(&response, "picked", "day"),
        vec![lit("2024-01-01"), lit("2024-06-01")]
    );
    assert_eq!(
        eval_value(code, "calendar", &[("day", lit("2024-01-01"))]),
        "2024-01-01"
    );
}

#[test]
fn missing_data_options_for_a_boolean_is_a_boolean() {
    let code = r#"
spec flags
data open: boolean
  -> options true
rule picked:
  open
"#;
    let response = evaluate(&load_ok(code), "flags", data(&[]));
    assert_eq!(
        rule(&response, "picked").missing_data(),
        &["open".to_string()]
    );
    assert_eq!(
        options(&response, "picked", "open"),
        vec![EvaluateDataValue::Boolean(true)]
    );
    assert_eq!(
        response_json(&response)["results"]["picked"]["missing_data_options"]["open"],
        json!([true])
    );
    assert_eq!(
        eval_value(code, "flags", &[("open", EvaluateDataValue::Boolean(true))],),
        "true"
    );
}

#[test]
fn missing_data_options_is_the_same_array_on_every_rule_missing_the_key() {
    let code = r#"
spec menu
data drink: text
  -> options "espresso", "latte"
rule picked:
  drink
rule also:
  drink
"#;
    let response = evaluate(&load_ok(code), "menu", data(&[]));
    let drinks = vec![lit("espresso"), lit("latte")];
    assert_eq!(options(&response, "picked", "drink"), drinks);
    assert_eq!(options(&response, "also", "drink"), drinks);
    let json = response_json(&response);
    assert_eq!(
        json["results"]["picked"]["missing_data_options"]["drink"],
        json!(["espresso", "latte"])
    );
    assert_eq!(
        json["results"]["also"]["missing_data_options"]["drink"],
        json!(["espresso", "latte"])
    );
}

//! Scalar `-> with` replaces `-> fill`. `fill` and deprecated standalone `with` are errors.

use crate::lists::{assert_contains, data, evaluate, lit, load_err, load_ok, value};
use lemma::DateTimeValue;
use rust_decimal::Decimal;

#[test]
fn with_number_evaluates() {
    let engine = load_ok(
        r#"
spec test
data n: number -> with 42
rule r: n
"#,
    );
    assert_eq!(value(&evaluate(&engine, "test", data(&[])), "r"), "42");
}

#[test]
fn with_measure_evaluates() {
    let engine = load_ok(
        r#"
spec test
uses lemma units
data age: units.calendar -> with 25 year
rule r: age as year
"#,
    );
    assert_eq!(value(&evaluate(&engine, "test", data(&[])), "r"), "25 year");
}

#[test]
fn with_overridden_by_caller() {
    let engine = load_ok(
        r#"
spec test
uses lemma units
data age: units.calendar -> with 25 year
rule r: age as year
"#,
    );
    assert_eq!(
        value(
            &evaluate(&engine, "test", data(&[("age", lit("30 year"))])),
            "r"
        ),
        "30 year"
    );
}

#[test]
fn with_boolean() {
    let engine = load_ok(
        r#"
spec test
data active: boolean -> with true
rule r: active
"#,
    );
    assert_eq!(value(&evaluate(&engine, "test", data(&[])), "r"), "true");
}

#[test]
fn with_text_with_options() {
    let engine = load_ok(
        r#"
spec test
data status: text
  -> option "a"
  -> option "b"
  -> with "a"
rule r: status
"#,
    );
    assert_eq!(value(&evaluate(&engine, "test", data(&[])), "r"), "a");
}

#[test]
fn with_violating_minimum_is_planning_error() {
    let joined = load_err(
        r#"
spec test
data n: number -> minimum 10 -> with 5
rule r: n
"#,
    );
    assert_contains(&joined.to_lowercase(), "minimum");
}

#[test]
fn with_twice_on_scalar_is_planning_error() {
    let joined = load_err(
        r#"
spec test
data n: number
  -> with 1
  -> with 2
"#,
    );
    assert_contains(&joined, "with");
}

#[test]
fn with_several_values_on_scalar_is_planning_error() {
    let joined = load_err(
        r#"
spec test
data rate: ratio
  -> with 5%, 6%
"#,
    );
    assert_contains(&joined, "ratio");
}

#[test]
fn fill_is_unknown_command() {
    let joined = load_err(
        r#"
spec test
data n: number -> fill 42
"#,
    );
    assert_contains(&joined, "fill");
}

#[test]
fn with_and_suggest_mutually_exclusive() {
    let joined = load_err(
        r#"
spec test
data n: number -> with 1 -> suggest 2
rule r: n
"#,
    );
    assert_contains(&joined.to_lowercase(), "suggest");
    assert_contains(&joined.to_lowercase(), "with");
}

#[test]
fn deprecated_standalone_with_is_parse_error() {
    let joined = load_err(
        r#"
spec inner
data v: number

spec outer
uses i: inner
with i.v: 42
"#,
    );
    assert_contains(&joined, "with");
}

#[test]
fn several_field_pairs_on_one_line_for_single_instance() {
    let engine = load_ok(
        r#"
spec employee
data name: text
data salary: measure
  -> unit eur: 1.00

spec payroll
uses ada: employee
  -> with name: "Ada", salary: 9000 eur

rule paid:
  ada.salary
"#,
    );
    assert_eq!(
        value(&evaluate(&engine, "payroll", data(&[])), "paid"),
        "9000 eur"
    );
}

#[test]
fn field_pair_after_bare_number_is_not_a_unit() {
    let engine = load_ok(
        r#"
spec line
data qty: number
data sku: text

spec order
uses first_line: line
  -> with qty: 3, sku: "latte"

rule ordered:
  first_line.qty
"#,
    );
    assert_eq!(
        value(&evaluate(&engine, "order", data(&[])), "ordered"),
        "3"
    );
}

#[test]
fn dotted_field_pair_after_bare_number_is_not_a_qualified_unit() {
    let engine = load_ok(
        r#"
spec product_info
data code: text

spec line
data qty: number
uses item: product_info

spec order
uses first_line: line
  -> with qty: 3, item.code: "latte"

rule ordered:
  first_line.qty

rule code:
  first_line.item.code
"#,
    );
    let response = evaluate(&engine, "order", data(&[]));
    assert_eq!(value(&response, "ordered"), "3");
    assert_eq!(value(&response, "code"), "latte");
}

#[test]
fn missing_comma_between_field_pairs_is_parse_error() {
    let joined = load_err(
        r#"
spec line
data qty: number
data sku: text

spec order
uses first_line: line
  -> with qty: 3 sku: "latte"
"#,
    );
    assert_contains(&joined, "comma");
}

#[test]
fn missing_comma_before_dotted_field_pair_is_parse_error() {
    let joined = load_err(
        r#"
spec product_info
data code: text

spec line
data qty: number
uses item: product_info

spec order
uses first_line: line
  -> with qty: 3 item.code: "latte"
"#,
    );
    assert_contains(&joined, "comma");
}

#[test]
fn trailing_comma_after_field_pair_is_parse_error() {
    let joined = load_err(
        r#"
spec line
data qty: number

spec order
uses first_line: line
  -> with qty: 3,
"#,
    );
    assert_contains(&joined, "comma");
}

#[test]
fn spaced_comma_between_numbers_is_not_a_digit_separator() {
    let joined = load_err(
        r#"
spec test
data n: number
  -> with 1, 000
"#,
    );
    assert!(
        !joined.contains("1_000"),
        "spaced comma must not be reported as digit grouping, got: {joined}"
    );
    assert_contains(&joined, "with");
}

#[test]
fn show_exposes_value_not_suggestion() {
    let engine = load_ok(
        r#"
spec test
data n: number -> with 42
rule r: n
"#,
    );
    let now = DateTimeValue::now();
    let entry = engine
        .show(None, "test", Some(&now))
        .expect("show")
        .data
        .get("n")
        .expect("n in show")
        .clone();
    assert!(
        entry.value.is_some(),
        "-> with must surface as value on show"
    );
    assert!(
        entry.suggestion.is_none(),
        "-> with must not surface as suggestion"
    );
    assert_eq!(
        entry.value.as_ref().and_then(|v| v.number),
        Some(Decimal::from(42))
    );
}

#[test]
fn with_ratio() {
    let engine = load_ok(
        r#"
spec test
data rate: ratio -> with 15%
rule r: rate
"#,
    );
    assert_eq!(value(&evaluate(&engine, "test", data(&[])), "r"), "15%");
}

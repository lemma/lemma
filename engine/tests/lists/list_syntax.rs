//! List syntax: commas, options, field-pair lookahead, postfix (expr).field,
//! `list` after a spec pin, reserved keywords.

use crate::lists::{
    assert_contains, data, evaluate, lit, load_err, load_files, load_ok, load_parse_err,
    load_with_dependency, value,
};

const KEYWORDS: &[&str] = &[
    "list", "key", "find", "first", "last", "sum", "avg", "product", "count", "min", "max", "any",
    "all", "where", "sort", "top", "asc", "desc", "is", "past", "future", "permille", "mod",
];

#[test]
fn value_rows_are_comma_separated() {
    let engine = load_ok(
        r#"
spec fares_card
data money: measure
  -> unit eur: 1.00

data fares: money list
  -> with 89 eur, 120 eur, 64 eur

rule total:
  fares -> sum
"#,
    );
    assert_eq!(
        value(&evaluate(&engine, "fares_card", data(&[])), "total"),
        "273 eur"
    );
}

#[test]
fn options_are_comma_separated() {
    let engine = load_ok(
        r#"
spec drink_menu
data drink: text
  -> options "espresso", "latte"

rule chosen:
  drink
"#,
    );
    assert_eq!(
        value(
            &evaluate(&engine, "drink_menu", data(&[("drink", lit("espresso"))])),
            "chosen"
        ),
        "espresso"
    );
}

#[test]
fn key_fields_are_comma_separated() {
    let engine = load_ok(
        r#"
spec freight_rate
data weight: measure
  -> unit kilogram: 1.00
data volume: measure
  -> unit liter: 1.00
data money: measure
  -> unit eur: 1.00
data band: weight range
data volume_band: volume range
data price: money

spec freight
uses rates: freight_rate list
  -> key band, volume_band
  -> with band: 0 kilogram...5 kilogram, volume_band: 0 liter...2 liter, price: 6.95 eur

data parcel_weight: rates.weight
data parcel_volume: rates.volume

rule shipping_cost:
  (rates -> find parcel_weight, parcel_volume).price
"#,
    );
    let response = evaluate(
        &engine,
        "freight",
        data(&[
            ("parcel_weight", lit("4 kilogram")),
            ("parcel_volume", lit("1 liter")),
        ]),
    );
    assert_eq!(value(&response, "shipping_cost"), "6.95 eur");
}

#[test]
fn trailing_comma_is_parse_error() {
    let err = load_err(
        r#"
spec fares_card
data money: measure
  -> unit eur: 1.00
data fares: money list
  -> with 89 eur,
"#,
    );
    assert_contains(&err, "comma");
}

#[test]
fn empty_item_is_parse_error() {
    let err = load_err(
        r#"
spec fares_card
data money: measure
  -> unit eur: 1.00
data fares: money list
  -> with 89 eur,, 120 eur
"#,
    );
    assert_contains(&err, "comma");
}

#[test]
fn missing_comma_between_options_is_parse_error() {
    let err = load_err(
        r#"
spec drink_menu
data drink: text
  -> options "espresso" "latte"
"#,
    );
    assert_contains(&err, "comma");
}

#[test]
fn list_continues_on_next_line_after_comma() {
    let engine = load_ok(
        r#"
spec fares_card
data money: measure
  -> unit eur: 1.00

data fares: money list
  -> with 89 eur,
       120 eur,
       64 eur

rule total:
  fares -> sum
"#,
    );
    assert_eq!(
        value(&evaluate(&engine, "fares_card", data(&[])), "total"),
        "273 eur"
    );
}

#[test]
fn comma_digit_separator_is_parse_error() {
    let err = load_err(
        r#"
spec payroll
data salary: number
  -> with 1,000
"#,
    );
    assert_contains(&err, "1_000");
}

#[test]
fn underscore_digit_separator_still_works() {
    let engine = load_ok(
        r#"
spec payroll
data salary: number
  -> with 1_000

rule paid:
  salary
"#,
    );
    assert_eq!(
        value(&evaluate(&engine, "payroll", data(&[])), "paid"),
        "1000"
    );
}

#[test]
fn with_comma_minus_is_two_rows() {
    let engine = load_ok(
        r#"
spec deltas
data steps: number list
  -> with 10, -2

rule total:
  steps -> sum
"#,
    );
    assert_eq!(value(&evaluate(&engine, "deltas", data(&[])), "total"), "8");
}

#[test]
fn with_space_minus_is_parse_error() {
    let err = load_parse_err(
        r#"
spec deltas
data steps: number list
  -> with 10 -2
"#,
    );
    assert_contains(&err, "comma");
}

fn fares_rule(body: &str) -> String {
    format!(
        r#"
spec fares_card
data money: measure
  -> unit eur: 1.00
data fares: money list
  -> with 89 eur, 120 eur, 64 eur
data take: number
  -> with 2
rule x:
  {body}
"#
    )
}

#[test]
fn command_call_and_bare_forms_on_value_list() {
    for (body, expected) in [
        ("fares -> top 2 -> count", "2"),
        ("fares -> top(2) -> count", "2"),
        ("fares -> top (2) -> count", "2"),
        ("fares -> top take -> count", "2"),
        ("fares -> find 2", "120 eur"),
        ("fares -> find(2)", "120 eur"),
        ("fares -> find(1 + 1)", "120 eur"),
        ("fares -> where >= 100 eur -> count", "1"),
        ("fares -> where(>= 100 eur) -> count", "1"),
        ("fares -> where (>= 100 eur) -> count", "1"),
        ("fares -> where in 60 eur...90 eur -> count", "2"),
        ("fares -> sum * 2", "546 eur"),
        ("fares -> sum - 5 eur", "268 eur"),
        ("fares -> sort desc -> top 2 -> first", "120 eur"),
        ("(fares -> top 1 -> first) + 1 eur", "90 eur"),
    ] {
        let engine = load_ok(&fares_rule(body));
        assert_eq!(
            value(&evaluate(&engine, "fares_card", data(&[])), "x"),
            expected,
            "{body}"
        );
    }
}

#[test]
fn operator_after_bare_argument_is_parse_error() {
    for (body, call) in [
        ("fares -> top take + 1", "top("),
        ("fares -> find 1 + 1", "find("),
        ("staff -> sum bonus * 2", "sum("),
        ("periods -> product 100% + rate", "product("),
        ("staff -> avg salary / 2", "avg("),
        ("staff -> min salary + 1", "min("),
        ("staff -> max salary > 1", "max("),
        ("staff -> sort salary * 2 desc", "sort("),
    ] {
        let err = load_parse_err(&fares_rule(body));
        assert_contains(&err, call);
    }
}

#[test]
fn bare_condition_is_one_comparison() {
    for body in [
        "staff -> where x > 3 + 4",
        "staff -> where x + 1 > 3",
        "staff -> where a > 1 and b < 2",
        "staff -> any x > 3 + 4",
        "staff -> all x + 1 > 3",
    ] {
        let err = load_parse_err(&fares_rule(body));
        assert_contains(&err, "(");
        assert_contains(&err, "one comparison");
    }
}

#[test]
fn argument_less_command_with_argument_is_parse_error() {
    for (body, command) in [
        ("fares -> count fares", "count"),
        ("fares -> count(1)", "count"),
        ("fares -> first 1", "first"),
        ("fares -> last(1)", "last"),
    ] {
        let err = load_parse_err(&fares_rule(body));
        assert_contains(&err, &format!("`{command}` takes no argument"));
    }
}

#[test]
fn comma_after_call_parenthesis_is_parse_error() {
    let err = load_parse_err(&fares_rule("rates -> find (1), 2"));
    assert_contains(&err, "inside the parentheses");
}

#[test]
fn appended_row_is_one_operand() {
    let err = load_parse_err(&fares_rule("fares -> with 64 eur + 1 eur"));
    assert_contains(&err, "with");
}

#[test]
fn field_pair_lookahead_is_not_a_unit() {
    let engine = load_ok(
        r#"
spec line
data qty: number
data sku: text

spec order
uses lines: line list
  -> with qty: 3, sku: "latte"

rule first_qty:
  (lines -> first).qty
"#,
    );
    assert_eq!(
        value(&evaluate(&engine, "order", data(&[])), "first_qty"),
        "3"
    );
}

#[test]
fn dotted_field_pair_lookahead_is_not_a_unit() {
    let engine = load_ok(
        r#"
spec product_info
data price: measure
  -> unit eur: 1.00

spec line
uses item: product_info
data qty: number

spec order
uses lines: line list
  -> with qty: 3, item.price: 2 eur

rule first_price:
  (lines -> first).item.price
"#,
    );
    assert_eq!(
        value(&evaluate(&engine, "order", data(&[])), "first_price"),
        "2 eur"
    );
}

#[test]
fn missing_comma_between_row_field_pairs_is_parse_error() {
    let err = load_err(
        r#"
spec line
data qty: number
data sku: text

spec order
uses lines: line list
  -> with qty: 3 sku: "latte"
"#,
    );
    assert_contains(&err, "comma");
}

#[test]
fn missing_comma_before_dotted_row_field_pair_is_parse_error() {
    let err = load_err(
        r#"
spec product_info
data price: measure
  -> unit eur: 1.00

spec line
uses item: product_info
data qty: number

spec order
uses lines: line list
  -> with qty: 3 item.price: 2 eur
"#,
    );
    assert_contains(&err, "comma");
}

#[test]
fn postfix_dot_after_parenthesis() {
    let engine = load_ok(
        r#"
spec employee
data name: text
data salary: measure
  -> unit eur: 1.00

spec payroll
uses staff: employee list
  -> with name: "Ada", salary: 9000 eur
  -> with name: "Grace", salary: 9500 eur

rule second_highest:
  (staff -> sort salary desc -> find 2).salary
"#,
    );
    assert_eq!(
        value(&evaluate(&engine, "payroll", data(&[])), "second_highest"),
        "9000 eur"
    );
}

#[test]
fn list_follows_pinned_spec_reference() {
    let engine = load_files(&[
        (
            "bracket.lemma",
            r#"
spec tax_bracket 2024-01-01
data rate: ratio
  -> with 36%
"#,
        ),
        (
            "tax.lemma",
            r#"
spec income_tax
uses brackets: tax_bracket 2024 list
  -> with rate: 36%

rule first_rate:
  (brackets -> first).rate
"#,
        ),
    ]);
    assert_eq!(
        value(&evaluate(&engine, "income_tax", data(&[])), "first_rate"),
        "36%"
    );
}

#[test]
fn list_follows_repository_qualified_pin() {
    let engine = load_with_dependency(
        "@carrier/tariffs",
        r#"
spec rate 2025-06-01
data price: measure
  -> unit eur: 1.00
  -> decimals 2
"#,
        &[(
            "shipping.lemma",
            r#"
spec parcel
uses rates: @carrier/tariffs rate 2025-06-01 list
  -> with price: 4.95 eur

rule first_price:
  (rates -> first).price
"#,
        )],
    );
    assert_eq!(
        value(&evaluate(&engine, "parcel", data(&[])), "first_price"),
        "4.95 eur"
    );
}

#[test]
fn reserved_keywords_rejected_as_data_rule_spec_and_alias() {
    for keyword in KEYWORDS {
        let as_data = load_err(&format!(
            r#"
spec sample
data {keyword}: number
"#
        ));
        assert_contains(&as_data, keyword);

        let as_rule = load_err(&format!(
            r#"
spec sample
rule {keyword}: 1
"#
        ));
        assert_contains(&as_rule, keyword);

        let as_spec = load_err(&format!("spec {keyword}\n"));
        assert_contains(&as_spec, keyword);

        let as_alias = load_err(&format!(
            r#"
spec employee
data name: text

spec payroll
uses {keyword}: employee
"#
        ));
        assert_contains(&as_alias, keyword);
    }
}

#[test]
fn reserved_keywords_are_case_insensitive() {
    let as_data = load_err(
        r#"
spec sample
data Sum: number
"#,
    );
    assert_contains(&as_data, "sum");
    let as_rule = load_err(
        r#"
spec sample
rule TOP: 1
"#,
    );
    assert_contains(&as_rule, "top");
}

#[test]
fn reserved_word_cannot_be_a_unit_name() {
    let err = load_err(
        r#"
spec sample
data duration: measure
  -> unit min: 1.00
"#,
    );
    assert_contains(&err, "min");
}

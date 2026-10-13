//! List `-> with` rows: inline, dotted paths, named instances, append, key collision.

use crate::lists::{
    assert_contains, data, evaluate, evaluate_explain, list, lit, load_ok, load_planning_err, map,
    missing_data, value, veto_reason,
};

#[test]
fn inline_row_one_line() {
    let engine = load_ok(
        r#"
spec menu_item
data drink: text
data price: measure
  -> unit eur: 1.00
spec coffee_order
uses menu: menu_item list
  -> with drink: "espresso", price: 2.50 eur
rule first_drink:
  (menu -> first).drink
"#,
    );
    assert_eq!(
        value(&evaluate(&engine, "coffee_order", data(&[])), "first_drink"),
        "espresso"
    );
}

#[test]
fn list_row_with_overrides_literal_fill() {
    let engine = load_ok(
        r#"
spec xx
data z: 5
spec test
uses zz: xx list
  -> with z: 22
  -> with z: 24
rule x:
  zz -> sum z
"#,
    );
    assert_eq!(value(&evaluate(&engine, "test", data(&[])), "x"), "46");
}

#[test]
fn list_row_with_overrides_typed_with_fill() {
    let engine = load_ok(
        r#"
spec xx
data z: number
  -> with 5
spec test
uses zz: xx list
  -> with z: 22
  -> with z: 24
rule x:
  zz -> sum z
"#,
    );
    assert_eq!(value(&evaluate(&engine, "test", data(&[])), "x"), "46");
}

#[test]
fn list_row_with_binds_open_number() {
    let engine = load_ok(
        r#"
spec xx
data z: number
spec test
uses zz: xx list
  -> with z: 22
  -> with z: 24
rule x:
  zz -> sum z
"#,
    );
    let response = evaluate(&engine, "test", data(&[]));
    assert_eq!(
        missing_data(&response, "x"),
        Vec::<String>::new(),
        "source-row `-> with z` binds zz.1.z and zz.2.z"
    );
    assert_eq!(value(&response, "x"), "46");
    let explained = evaluate_explain(&engine, "test", data(&[]));
    assert_eq!(
        missing_data(&explained, "x"),
        Vec::<String>::new(),
        "explain must not report zz.1.z when the source row set z"
    );
    assert_eq!(value(&explained, "x"), "46");
}

#[test]
fn inline_row_one_field_per_line() {
    let engine = load_ok(
        r#"
spec menu_item
data drink: text
data price: measure
  -> unit eur: 1.00
  -> decimals 2
spec coffee_order
uses menu: menu_item list
  -> with
       drink: "latte",
       price: 3.50 eur
rule first_price:
  (menu -> first).price
"#,
    );
    assert_eq!(
        value(&evaluate(&engine, "coffee_order", data(&[])), "first_price"),
        "3.50 eur"
    );
}

#[test]
fn dotted_row_path_through_row_spec_uses() {
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
  -> with item.price: 3 eur, qty: 2
rule unit_price:
  (lines -> first).item.price
"#,
    );
    assert_eq!(
        value(&evaluate(&engine, "order", data(&[])), "unit_price"),
        "3 eur"
    );
}

#[test]
fn unknown_row_path_is_planning_error() {
    let err = load_planning_err(
        r#"
spec employee
data name: text
spec payroll
uses staff: employee list
  -> with bonus: 1
"#,
    );
    assert_contains(&err, "bonus");
}

#[test]
fn same_path_twice_in_one_row_is_planning_error() {
    let err = load_planning_err(
        r#"
spec employee
data name: text
spec payroll
uses staff: employee list
  -> with name: "Ada", name: "Grace"
"#,
    );
    assert_contains(&err, "name");
}

#[test]
fn dotted_key_field_is_planning_error() {
    let err = load_planning_err(
        r#"
spec product_info
data sku: text
spec line
uses item: product_info
spec order
uses lines: line list
  -> key item.sku
"#,
    );
    assert_contains(&err, "key");
}

#[test]
fn named_instance_rows_follow_their_sources() {
    let engine = load_ok(
        r#"
spec employee
data name: text
data salary: measure
  -> unit eur: 1.00
spec payroll
uses ada: employee
  -> with name: "Ada", salary: 9000 eur
uses grace: employee
  -> with name: "Grace", salary: 9500 eur
uses staff: employee list
  -> with ada, grace
rule total:
  staff -> sum salary
"#,
    );
    assert_eq!(
        value(&evaluate(&engine, "payroll", data(&[])), "total"),
        "18500 eur"
    );
}

#[test]
fn named_instance_field_stays_an_ordinary_input() {
    let engine = load_ok(
        r#"
spec employee
data name: text
data salary: measure
  -> unit eur: 1.00
spec payroll
uses ada: employee
  -> with name: "Ada"
uses staff: employee list
  -> with ada
rule paid:
  (staff -> first).salary
"#,
    );
    let response = evaluate(&engine, "payroll", data(&[("ada.salary", lit("9000 eur"))]));
    assert_eq!(value(&response, "paid"), "9000 eur");
}

#[test]
fn rule_head_with_starts_empty() {
    let engine = load_ok(
        r#"
spec fares_card
data money: measure
  -> unit eur: 1.00
data fares: money list
  -> with 89 eur
rule extra:
  money list -> with 64 eur
rule extra_total:
  extra -> sum
"#,
    );
    assert_eq!(
        value(&evaluate(&engine, "fares_card", data(&[])), "extra_total"),
        "64 eur"
    );
}

#[test]
fn expression_with_appends_to_current_including_evaluate_data() {
    let engine = load_ok(
        r#"
spec fares_card
data money: measure
  -> unit eur: 1.00
data fares: money list
  -> with 89 eur
rule extended:
  fares -> with 64 eur
rule total:
  extended -> sum
"#,
    );
    let response = evaluate(
        &engine,
        "fares_card",
        data(&[("fares", list(vec![lit("120 eur")]))]),
    );
    assert_eq!(value(&response, "total"), "184 eur");
}

#[test]
fn expression_with_appends_inline_instance_row() {
    let engine = load_ok(
        r#"
spec menu_item
data drink: text
data price: measure
  -> unit eur: 1.00
spec coffee_order
uses menu: menu_item list
  -> with drink: "espresso", price: 2.50 eur
rule extended:
  menu -> with drink: "mocha", price: 4.00 eur
rule n:
  extended -> count
"#,
    );
    assert_eq!(
        value(&evaluate(&engine, "coffee_order", data(&[])), "n"),
        "2"
    );
}

#[test]
fn data_copy_uses_declaration_defaults_not_evaluate_data() {
    let engine = load_ok(
        r#"
spec fares_card
data money: measure
  -> unit eur: 1.00
data fares: money list
  -> with 89 eur
data extra: fares -> with 64 eur
rule extra_total:
  extra -> sum
"#,
    );
    let response = evaluate(
        &engine,
        "fares_card",
        data(&[("fares", list(vec![lit("120 eur")]))]),
    );
    assert_eq!(value(&response, "extra_total"), "153 eur");
}

#[test]
fn numbered_append_continues_at_n_plus_one() {
    let engine = load_ok(
        r#"
spec fares_card
data money: measure
  -> unit eur: 1.00
data fares: money list
  -> with 89 eur, 120 eur
rule extended:
  fares -> with 64 eur
rule third:
  extended -> find 3
"#,
    );
    assert_eq!(
        value(&evaluate(&engine, "fares_card", data(&[])), "third"),
        "64 eur"
    );
}

#[test]
fn append_key_collision_on_source_rows_is_planning_error() {
    let err = load_planning_err(
        r#"
spec menu_item
data drink: text
data price: measure
  -> unit eur: 1.00
spec coffee_order
uses menu: menu_item list
  -> key drink
  -> with drink: "espresso", price: 2.50 eur
rule extended:
  menu -> with drink: "espresso", price: 3.00 eur
"#,
    );
    assert_contains(&err, "espresso");
}

#[test]
fn append_key_collision_on_evaluate_data_vetoes() {
    let engine = load_ok(
        r#"
spec menu_item
data drink: text
data price: measure
  -> unit eur: 1.00
spec coffee_order
uses menu: menu_item list
  -> key drink
rule extended:
  menu -> with drink: "espresso", price: 3.00 eur
rule n:
  extended -> count
"#,
    );
    let response = evaluate(
        &engine,
        "coffee_order",
        data(&[(
            "menu",
            list(vec![map(&[
                ("drink", lit("espresso")),
                ("price", lit("2.50 eur")),
            ])]),
        )]),
    );
    assert_contains(&veto_reason(&response, "n"), "espresso");
}

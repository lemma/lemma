//! Whole-list evaluation data, shape request Errors, whole-list vetoes, limits.

use crate::lists::{
    assert_contains, boolean, data, evaluate, evaluate_err, list, lit, load_ok,
    load_ok_with_limits, map, missing_data, value, veto_reason,
};
use lemma::ResourceLimits;

fn payroll() -> &'static str {
    r#"
spec employee
data name: text
data salary: measure
  -> unit eur: 1.00
spec payroll
uses employee
data staff: employee list
  -> with name: "Ada", salary: 9000 eur
rule n:
  staff -> count
rule first_name:
  (staff -> first).name
"#
}

#[test]
fn whole_list_replace() {
    let engine = load_ok(payroll());
    let response = evaluate(
        &engine,
        "payroll",
        data(&[(
            "staff",
            list(vec![map(&[
                ("name", lit("Grace")),
                ("salary", lit("9500 eur")),
            ])]),
        )]),
    );
    assert_eq!(value(&response, "n"), "1");
    assert_eq!(value(&response, "first_name"), "Grace");
}

#[test]
fn empty_list_replace() {
    let engine = load_ok(payroll());
    let response = evaluate(&engine, "payroll", data(&[("staff", list(vec![]))]));
    assert_eq!(value(&response, "n"), "0");
}

#[test]
fn per_row_input_key_fills_unbound_field() {
    let engine = load_ok(
        r#"
spec employee
data name: text
data salary: measure
  -> unit eur: 1.00
spec payroll
uses employee
data staff: employee list
  -> with name: "Ada"
  -> with name: "Grace"
rule first_salary:
  (staff -> first).salary
rule second_salary:
  (staff -> find 2).salary
"#,
    );
    let response = evaluate(
        &engine,
        "payroll",
        data(&[("staff.2.salary", lit("8000 eur"))]),
    );
    assert_eq!(value(&response, "second_salary"), "8000 eur");
    assert_eq!(
        missing_data(&response, "first_salary"),
        vec!["staff.1.salary".to_string()]
    );
}

#[test]
fn misspelled_row_field_is_ignored() {
    let engine = load_ok(payroll());
    evaluate(
        &engine,
        "payroll",
        data(&[("staff", list(vec![map(&[("nme", lit("Grace"))])]))]),
    );
}

#[test]
fn sibling_list_name_prefix_is_not_row_key_error() {
    let engine = load_ok(
        r#"
spec item
data n: number
spec s
uses item
data line: item list
rule r: 1
"#,
    );
    let response = evaluate(&engine, "s", data(&[("lineage.1.x", lit("1"))]));
    assert_eq!(value(&response, "r"), "1");
}

#[test]
fn literal_on_list_is_request_error() {
    let engine = load_ok(payroll());
    let err = evaluate_err(&engine, "payroll", data(&[("staff", lit("Ada"))]));
    assert_contains(&err.to_string(), "staff");
}

#[test]
fn list_on_scalar_is_request_error() {
    let engine = load_ok(
        r#"
spec sample
data salary: number
rule r: salary
"#,
    );
    let err = evaluate_err(&engine, "sample", data(&[("salary", list(vec![lit("1")]))]));
    assert_contains(&err.to_string(), "salary");
}

#[test]
fn multi_key_unit_map_is_request_error() {
    let engine = load_ok(
        r#"
spec sample
data money: measure
  -> unit eur: 1.00
  -> unit usd: 1.10
data price: money
rule r: price
"#,
    );
    let err = evaluate_err(
        &engine,
        "sample",
        data(&[("price", map(&[("eur", lit("84")), ("usd", lit("90"))]))]),
    );
    assert_contains(&err.to_string(), "price");
}

#[test]
fn empty_map_on_measure_is_request_error() {
    let engine = load_ok(
        r#"
spec sample
data money: measure
  -> unit eur: 1.00
data price: money
rule r: price
"#,
    );
    let err = evaluate_err(&engine, "sample", data(&[("price", map(&[]))]));
    assert_contains(&err.to_string(), "price");
}

#[test]
fn boolean_on_non_boolean_is_request_error() {
    let engine = load_ok(
        r#"
spec sample
data name: text
rule r: name
"#,
    );
    let err = evaluate_err(&engine, "sample", data(&[("name", boolean(true))]));
    assert_contains(&err.to_string(), "name");
}

#[test]
fn map_as_uses_nesting_level_is_request_error() {
    let engine = load_ok(
        r#"
spec employee
data name: text
spec payroll
uses ada: employee
rule r: ada.name
"#,
    );
    let err = evaluate_err(
        &engine,
        "payroll",
        data(&[("ada", map(&[("name", lit("Ada"))]))]),
    );
    assert_contains(&err.to_string(), "ada");
}

#[test]
fn unknown_row_field_is_ignored() {
    let engine = load_ok(payroll());
    evaluate(
        &engine,
        "payroll",
        data(&[("staff", list(vec![map(&[("bonus", lit("1"))])]))]),
    );
}

#[test]
fn value_list_elements_literal_or_single_unit_map() {
    let engine = load_ok(
        r#"
spec fares_card
data money: measure
  -> unit eur: 1.00
data fares: money list
rule total:
  fares -> sum
"#,
    );
    let response = evaluate(
        &engine,
        "fares_card",
        data(&[(
            "fares",
            list(vec![lit("84 eur"), map(&[("eur", lit("120"))])]),
        )]),
    );
    assert_eq!(value(&response, "total"), "204 eur");
}

#[test]
fn row_keys_are_dotted_paths() {
    let engine = load_ok(
        r#"
spec product_info
data price: measure
  -> unit eur: 1.00
spec line
uses item: product_info
data qty: number
spec order
uses line
data lines: line list
rule first_price:
  (lines -> first).item.price
"#,
    );
    let response = evaluate(
        &engine,
        "order",
        data(&[(
            "lines",
            list(vec![map(&[
                ("item.price", lit("3 eur")),
                ("qty", lit("2")),
            ])]),
        )]),
    );
    assert_eq!(value(&response, "first_price"), "3 eur");
}

#[test]
fn row_parse_failure_vetoes_whole_list() {
    let engine = load_ok(payroll());
    let response = evaluate(
        &engine,
        "payroll",
        data(&[(
            "staff",
            list(vec![map(&[
                ("name", lit("Ada")),
                ("salary", lit("not-money")),
            ])]),
        )]),
    );
    assert_contains(&veto_reason(&response, "n"), "salary");
}

#[test]
fn a_row_with_two_bad_fields_vetoes_on_the_first_supplied() {
    let engine = load_ok(
        r#"
spec item
data weight: number
data price: number
spec order
uses item
data items: item list
rule n:
  items -> count
"#,
    );
    let response = evaluate(
        &engine,
        "order",
        data(&[(
            "items",
            list(vec![map(&[
                ("price", lit("cheap")),
                ("weight", lit("heavy")),
            ])]),
        )]),
    );
    let reason = veto_reason(&response, "n");
    assert_contains(&reason, "price");
    assert!(!reason.contains("weight"), "veto names one field: {reason}");
}

#[test]
fn duplicate_evaluate_data_keys_veto_whole_list() {
    let engine = load_ok(
        r#"
spec menu_item
data drink: text
data price: number
spec coffee_order
uses menu_item
data menu: menu_item list
  -> key drink
rule n:
  menu -> count
"#,
    );
    let response = evaluate(
        &engine,
        "coffee_order",
        data(&[(
            "menu",
            list(vec![
                map(&[("drink", lit("espresso")), ("price", lit("1"))]),
                map(&[("drink", lit("espresso")), ("price", lit("2"))]),
            ]),
        )]),
    );
    assert_contains(&veto_reason(&response, "n"), "espresso");
}

#[test]
fn invalid_option_vetoes_whole_list() {
    let engine = load_ok(
        r#"
spec menu_item
data drink: text
  -> options "espresso", "latte"
data price: number
spec coffee_order
uses menu_item
data menu: menu_item list
  -> key drink
rule n:
  menu -> count
"#,
    );
    let response = evaluate(
        &engine,
        "coffee_order",
        data(&[(
            "menu",
            list(vec![map(&[("drink", lit("mocha")), ("price", lit("1"))])]),
        )]),
    );
    assert_contains(&veto_reason(&response, "n"), "mocha");
}

#[test]
fn range_overlap_vetoes_whole_list() {
    let engine = load_ok(
        r#"
spec rate
data weight: measure
  -> unit kilogram: 1.00
data band: weight range
data price: number
spec parcel
uses rate
data rates: rate list
  -> key band
rule n:
  rates -> count
"#,
    );
    let response = evaluate(
        &engine,
        "parcel",
        data(&[(
            "rates",
            list(vec![
                map(&[
                    ("band", lit("0 kilogram...3 kilogram")),
                    ("price", lit("1")),
                ]),
                map(&[
                    ("band", lit("2 kilogram...10 kilogram")),
                    ("price", lit("2")),
                ]),
            ]),
        )]),
    );
    assert_contains(&veto_reason(&response, "n"), "overlap");
}

#[test]
fn rows_numbered_in_supplied_order() {
    let engine = load_ok(payroll());
    let response = evaluate(
        &engine,
        "payroll",
        data(&[(
            "staff",
            list(vec![
                map(&[("name", lit("Grace")), ("salary", lit("1 eur"))]),
                map(&[("name", lit("Ada")), ("salary", lit("2 eur"))]),
            ]),
        )]),
    );
    assert_eq!(value(&response, "first_name"), "Grace");
}

#[test]
fn empty_literal_is_empty_input_empty_list_is_not() {
    let engine = load_ok(
        r#"
spec sample
data age: number
data fares: number list
rule aged: age
rule n: fares -> count
"#,
    );
    let empty_age = evaluate(&engine, "sample", data(&[("age", lit(""))]));
    assert_contains(&veto_reason(&empty_age, "aged"), "cannot be empty");
    let empty_list = evaluate(&engine, "sample", data(&[("fares", list(vec![]))]));
    assert_eq!(value(&empty_list, "n"), "0");
}

#[test]
fn missing_nested_components_is_empty_list() {
    let engine = load_ok(
        r#"
spec part
data money: measure
  -> unit eur: 1.00
data components: part list
data quantity: number
data own_cost: money
rule unit_cost:
  own_cost + (components -> sum(components.quantity * components.unit_cost))
spec bike
uses frame: part
rule cost:
  frame.unit_cost
"#,
    );
    let response = evaluate(
        &engine,
        "bike",
        data(&[
            ("frame.quantity", lit("1")),
            ("frame.own_cost", lit("40 eur")),
        ]),
    );
    assert_eq!(value(&response, "cost"), "40 eur");
}

#[test]
fn uses_list_alias_binds_as_input_key() {
    let engine = load_ok(payroll());
    let response = evaluate(
        &engine,
        "payroll",
        data(&[(
            "staff",
            list(vec![map(&[("name", lit("Ada")), ("salary", lit("1 eur"))])]),
        )]),
    );
    assert_eq!(value(&response, "n"), "1");
}

fn tight_limits() -> ResourceLimits {
    ResourceLimits {
        max_list_rows: 2,
        max_list_depth: 2,
        max_list_nodes: 3,
        ..ResourceLimits::default()
    }
}

#[test]
fn max_list_rows_is_request_error() {
    let engine = load_ok_with_limits(payroll(), tight_limits());
    let err = evaluate_err(
        &engine,
        "payroll",
        data(&[(
            "staff",
            list(vec![
                map(&[("name", lit("A")), ("salary", lit("1 eur"))]),
                map(&[("name", lit("B")), ("salary", lit("1 eur"))]),
                map(&[("name", lit("C")), ("salary", lit("1 eur"))]),
            ]),
        )]),
    );
    assert_contains(&err.to_string(), "max_list_rows");
}

#[test]
fn max_list_depth_is_request_error() {
    let engine = load_ok_with_limits(
        r#"
spec part
data own_cost: number
  -> with 1
data components: part list
rule unit_cost:
  own_cost + (components -> sum components.unit_cost)
"#,
        tight_limits(),
    );
    let too_deep = list(vec![map(&[(
        "components",
        list(vec![map(&[(
            "components",
            list(vec![map(&[("own_cost", lit("1"))])]),
        )])]),
    )])]);
    let err = evaluate_err(&engine, "part", data(&[("components", too_deep)]));
    assert_contains(&err.to_string(), "max_list_depth");
}

#[test]
fn max_list_depth_counts_a_nested_list_under_a_mixed_case_key() {
    let engine = load_ok_with_limits(
        r#"
spec part
data own_cost: number
  -> with 1
data components: part list
rule unit_cost:
  own_cost + (components -> sum components.unit_cost)
"#,
        tight_limits(),
    );
    let too_deep = list(vec![map(&[(
        "Components",
        list(vec![map(&[(
            "COMPONENTS",
            list(vec![map(&[("own_cost", lit("1"))])]),
        )])]),
    )])]);
    let err = evaluate_err(&engine, "part", data(&[("components", too_deep)]));
    assert_contains(&err.to_string(), "max_list_depth");
}

fn keyed_menu() -> &'static str {
    r#"
spec menu_item
data drink: text
data size: text
data price: number
spec coffee_order
uses menu_item
data menu: menu_item list
  -> key drink, size
rule n:
  menu -> count
"#
}

#[test]
fn a_later_row_without_its_key_field_is_a_request_error_naming_the_row() {
    let engine = load_ok(keyed_menu());
    let err = evaluate_err(
        &engine,
        "coffee_order",
        data(&[(
            "menu",
            list(vec![
                map(&[
                    ("drink", lit("espresso")),
                    ("size", lit("small")),
                    ("price", lit("1")),
                ]),
                map(&[("drink", lit("latte")), ("price", lit("2"))]),
            ]),
        )]),
    );
    assert_contains(&err.to_string(), "row 2 has no value for key field 'size'");
}

#[test]
fn a_two_field_key_finds_duplicates_whatever_the_field_order() {
    let engine = load_ok(keyed_menu());
    let response = evaluate(
        &engine,
        "coffee_order",
        data(&[(
            "menu",
            list(vec![
                map(&[
                    ("drink", lit("latte")),
                    ("size", lit("large")),
                    ("price", lit("1")),
                ]),
                map(&[
                    ("price", lit("2")),
                    ("SIZE", lit("large")),
                    ("Drink", lit("latte")),
                ]),
                map(&[
                    ("drink", lit("latte")),
                    ("size", lit("small")),
                    ("price", lit("3")),
                ]),
            ]),
        )]),
    );
    assert_contains(&veto_reason(&response, "n"), "latte");
}

#[test]
fn max_list_nodes_is_request_error() {
    let engine = load_ok_with_limits(
        r#"
spec part
data own_cost: number
  -> with 1
data components: part list
rule unit_cost:
  own_cost + (components -> sum components.unit_cost)
"#,
        tight_limits(),
    );
    let wide = list(vec![
        map(&[("components", list(vec![map(&[("own_cost", lit("1"))])]))]),
        map(&[("components", list(vec![map(&[("own_cost", lit("1"))])]))]),
    ]);
    let err = evaluate_err(&engine, "part", data(&[("components", wide)]));
    assert_contains(&err.to_string(), "max_list_nodes");
}

#[test]
fn max_list_rows_on_value_list_is_request_error() {
    let engine = load_ok_with_limits(
        r#"
spec fares_card
data fares: number list
rule n:
  fares -> count
"#,
        tight_limits(),
    );
    let err = evaluate_err(
        &engine,
        "fares_card",
        data(&[("fares", list(vec![lit("1"), lit("2"), lit("3")]))]),
    );
    assert_contains(&err.to_string(), "max_list_rows");
}

#[test]
fn evaluate_takes_typed_evaluate_data_value() {
    let engine = load_ok(
        r#"
spec sample
data n: number
rule r: n
"#,
    );
    let mut inputs = indexmap::IndexMap::new();
    inputs.insert("n".to_string(), lit("7"));
    assert_eq!(value(&evaluate(&engine, "sample", inputs), "r"), "7");
}

#[test]
fn empty_evaluate_data_row_reports_missing_field() {
    let engine = load_ok(
        r#"
spec employee
data name: text
spec payroll
uses employee
data staff: employee list
rule first_name:
  (staff -> first).name
"#,
    );
    let response = evaluate(&engine, "payroll", data(&[("staff", list(vec![map(&[])]))]));
    let first = response.results.get("first_name").expect("first_name");
    assert!(first.is_missing_data(), "{first:?}");
    assert_eq!(missing_data(&response, "first_name"), vec!["staff.1.name"]);
}

#[test]
fn nested_evaluate_data_row_reports_full_path() {
    let engine = load_ok(
        r#"
spec extra
data x: number
spec item
uses extra
data extras: extra list
rule first_x:
  (extras -> first).x
spec order
uses item
data items: item list
rule out:
  (items -> first).first_x
"#,
    );
    let response = evaluate(
        &engine,
        "order",
        data(&[(
            "items",
            list(vec![map(&[("extras", list(vec![map(&[])]))])]),
        )]),
    );
    let out = response.results.get("out").expect("out");
    assert!(out.is_missing_data(), "{out:?}");
    assert_eq!(missing_data(&response, "out"), vec!["items.1.extras.1.x"]);
}

#[test]
fn nested_uses_empty_list_filled_by_dotted_evaluate_data() {
    let engine = load_ok(
        r#"
spec cell
data id: text
data qty: number
spec catalog
uses cell
data items: cell list
  -> key id
rule first_qty: (items -> find "a").qty
spec wrapper
uses c: catalog
rule out: c.first_qty
"#,
    );
    let response = evaluate(
        &engine,
        "wrapper",
        data(&[(
            "c.items",
            list(vec![map(&[("id", lit("a")), ("qty", lit("5"))])]),
        )]),
    );
    assert_eq!(value(&response, "out"), "5");
}

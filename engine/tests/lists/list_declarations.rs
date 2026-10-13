//! List declarations: value lists, uses lists, empty defaults, type positions.

use crate::lists::{
    assert_contains, data, evaluate, list, lit, load_ok, load_parse_err, load_planning_err, value,
};

#[test]
fn data_value_list_with_rows() {
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
fn derived_measure_list_rows_use_resolved_unit_factors() {
    let engine = load_ok(
        r#"
spec trip
uses lemma units
data velocity: measure
  -> unit mps: meter/second
  -> unit kmh: kilometer/hour
data speeds: velocity list
  -> with 10 mps, 72 kmh
rule fastest:
  (speeds -> max) as mps
"#,
    );
    assert_eq!(
        value(&evaluate(&engine, "trip", data(&[])), "fastest"),
        "20 mps"
    );
}

#[test]
fn list_type_extending_list_type_keeps_named_element() {
    let engine = load_ok(
        r#"
spec fares_card
data money: measure
  -> unit eur: 1.00
  -> unit cent: 0.01
data fares: money list
data peak_fares: fares
  -> with 89 eur, 1200 cent
rule total:
  (peak_fares -> sum) as eur
"#,
    );
    assert_eq!(
        value(&evaluate(&engine, "fares_card", data(&[])), "total"),
        "101 eur"
    );
}

#[test]
fn rule_value_list_with_rows() {
    let engine = load_ok(
        r#"
spec codes
rule codes:
  text list -> with "A1", "B2"
rule first_code:
  codes -> first
"#,
    );
    assert_eq!(
        value(&evaluate(&engine, "codes", data(&[])), "first_code"),
        "A1"
    );
}

#[test]
fn rule_range_list_with_rows() {
    let engine = load_ok(
        r#"
spec bands
data money: measure
  -> unit eur: 1.00
rule bands:
  money range list -> with 1 eur...2 eur, 3 eur...5 eur
rule n:
  bands -> count
rule last_band:
  bands -> last
"#,
    );
    let response = evaluate(&engine, "bands", data(&[]));
    assert_eq!(value(&response, "n"), "2");
    assert_eq!(value(&response, "last_band"), "3 eur...5 eur");
}

#[test]
fn rule_list_qualified_element_is_the_imported_type() {
    let engine = load_ok(
        r#"
spec pricing
data code: text
  -> option "A"
  -> option "B"

spec shop
uses pricing
data code: text
  -> option "X"
  -> option "Y"
rule codes:
  pricing.code list -> with "A", "B"
rule first_code:
  codes -> first
"#,
    );
    assert_eq!(
        value(&evaluate(&engine, "shop", data(&[])), "first_code"),
        "A"
    );
}

#[test]
fn rule_list_qualified_element_does_not_fall_back_to_local_bare_name() {
    let message = load_planning_err(
        r#"
spec other
data unused: text

spec shop
uses other
data code: text
  -> option "local"
rule codes:
  other.code list -> with "local"
"#,
    );
    assert_contains(
        &message,
        "Type 'code' is not defined in spec 'other' (via import 'other')",
    );
}

#[test]
fn rule_list_imported_element_needs_alias() {
    let engine = load_ok(
        r#"
spec pricing
data money: measure
  -> unit eur: 1.00

spec shop
uses pricing
rule fares:
  pricing.money list -> with 5 eur, 7 eur
rule total:
  fares -> sum
"#,
    );
    assert_eq!(
        value(&evaluate(&engine, "shop", data(&[])), "total"),
        "12 eur"
    );
}

#[test]
fn rule_list_unknown_element_is_planning_error() {
    let message = load_planning_err(
        r#"
spec shop
rule fares:
  money list -> with 5 eur
"#,
    );
    assert_contains(&message, "Unknown parent 'money' for list type in a rule");
}

#[test]
fn rule_list_of_spec_is_planning_error() {
    let message = load_planning_err(
        r#"
spec employee
data name: text

spec payroll
rule staff:
  employee list -> with "Ada"
"#,
    );
    assert_contains(
        &message,
        "'employee' names a spec. Instance lists are declared as data with `uses`, not built in a rule",
    );
}

#[test]
fn list_without_rows_is_empty_never_missing() {
    let engine = load_ok(
        r#"
spec fares_card
data money: measure
  -> unit eur: 1.00
data fares: money list
rule total:
  fares -> sum
rule n:
  fares -> count
"#,
    );
    let response = evaluate(&engine, "fares_card", data(&[]));
    assert_eq!(value(&response, "total"), "0 eur");
    assert_eq!(value(&response, "n"), "0");
}

#[test]
fn uses_instance_list() {
    let engine = load_ok(
        r#"
spec employee
data name: text
spec payroll
uses staff: employee list
  -> with name: "Ada"
rule n:
  staff -> count
"#,
    );
    assert_eq!(value(&evaluate(&engine, "payroll", data(&[])), "n"), "1");
}

#[test]
fn data_spec_list_suggests_uses() {
    let err = load_planning_err(
        r#"
spec employee
data name: text
spec payroll
data staff: employee list
"#,
    );
    assert_contains(&err, "uses staff: employee list");
}

#[test]
fn type_position_through_list_alias() {
    let engine = load_ok(
        r#"
spec menu_item
data drink: text
  -> options "espresso", "latte"
data price: measure
  -> unit eur: 1.00

spec coffee_order
uses menu: menu_item list
  -> key drink
  -> with drink: "espresso", price: 2.50 eur
  -> with drink: "latte", price: 3.50 eur
data ordered: menu.drink
rule chosen:
  ordered
"#,
    );
    assert_eq!(
        value(
            &evaluate(
                &engine,
                "coffee_order",
                data(&[("ordered", lit("espresso"))])
            ),
            "chosen"
        ),
        "espresso"
    );
}

#[test]
fn dot_on_list_alias_outside_command_is_error() {
    let err = load_planning_err(
        r#"
spec employee
data salary: measure
  -> unit eur: 1.00
spec payroll
uses staff: employee list
rule all_salaries:
  staff.salary
"#,
    );
    assert_contains(&err, "staff");
}

#[test]
fn rule_value_list_is_not_overridable() {
    let engine = load_ok(
        r#"
spec codes
rule codes:
  text list -> with "A1", "B2"
rule first_code:
  codes -> first
"#,
    );
    let response = evaluate(&engine, "codes", data(&[("codes", list(vec![lit("Z9")]))]));
    assert_eq!(value(&response, "first_code"), "A1");
}

#[test]
fn rule_instance_list_with_rows_is_rejected() {
    let err = load_planning_err(
        r#"
spec menu_item
data drink: text
spec coffee_order
rule menu: menu_item list
  -> with drink: "espresso"
"#,
    );
    assert_contains(&err, "uses");
}

#[test]
fn row_type_checked_on_instance_list() {
    let err = load_planning_err(
        r#"
spec employee
data name: text
spec payroll
uses staff: employee list
  -> with "Ada"
"#,
    );
    assert_contains(&err, "employee");
}

#[test]
fn query_command_on_data_is_parse_error() {
    let err = load_parse_err(
        r#"
spec payroll
data salaries: number list
  -> min
"#,
    );
    assert_contains(&err, "data supplies values");
}

#[test]
fn query_command_on_uses_is_parse_error() {
    let err = load_parse_err(
        r#"
spec employee
data salary: measure
  -> unit eur: 1.00
spec payroll
uses staff: employee list
  -> min salary
"#,
    );
    assert_contains(&err, "data supplies values");
}

#[test]
fn key_on_value_list_is_parse_error() {
    let err = load_parse_err(
        r#"
spec fares_card
data money: measure
  -> unit eur: 1.00
data fares: money list
  -> key fare
"#,
    );
    assert_contains(&err, "-> key");
}

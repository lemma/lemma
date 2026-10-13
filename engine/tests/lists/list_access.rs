//! Access: find, first, last, row dot, unless branch typing.

use crate::lists::{
    assert_contains, data, evaluate, load_ok, load_planning_err, value, veto_reason,
};

#[test]
fn find_first_last() {
    let engine = load_ok(
        r#"
spec fares_card
data money: measure
  -> unit eur: 1.00
data fares: money list
  -> with 89 eur, 120 eur, 64 eur
rule cheapest:
  fares -> last
rule dearest:
  fares -> sort desc -> first
rule second:
  fares -> find 2
"#,
    );
    let response = evaluate(&engine, "fares_card", data(&[]));
    assert_eq!(value(&response, "cheapest"), "64 eur");
    assert_eq!(value(&response, "dearest"), "120 eur");
    assert_eq!(value(&response, "second"), "120 eur");
}

#[test]
fn first_last_on_empty_veto() {
    let engine = load_ok(
        r#"
spec fares_card
data money: measure
  -> unit eur: 1.00
data fares: money list
rule start:
  fares -> first
rule end:
  fares -> last
"#,
    );
    let response = evaluate(&engine, "fares_card", data(&[]));
    assert_contains(&veto_reason(&response, "start"), "empty");
    assert_contains(&veto_reason(&response, "end"), "empty");
}

#[test]
fn row_dot_reads_field() {
    let engine = load_ok(
        r#"
spec menu_item
data drink: text
  -> options "espresso", "latte"
data price: measure
  -> unit eur: 1.00
  -> decimals 2
spec coffee_order
uses menu: menu_item list
  -> key drink
  -> with drink: "espresso", price: 2.50 eur
  -> with drink: "latte", price: 3.50 eur
data ordered: menu.drink
rule selected_drink:
  menu -> find ordered
rule total:
  selected_drink.price
"#,
    );
    assert_eq!(
        value(
            &evaluate(
                &engine,
                "coffee_order",
                data(&[("ordered", crate::lists::lit("espresso"))])
            ),
            "total"
        ),
        "2.50 eur"
    );
}

#[test]
fn parenthesised_find_then_dot() {
    let engine = load_ok(
        r#"
spec rate
data weight: measure
  -> unit kilogram: 1.00
data money: measure
  -> unit eur: 1.00
data band: weight range
data price: money
spec parcel
uses rates: rate list
  -> key band
  -> with band: 0 kilogram...2 kilogram, price: 4.95 eur
data parcel_weight: rates.weight
rule shipping:
  (rates -> find parcel_weight).price
"#,
    );
    assert_eq!(
        value(
            &evaluate(
                &engine,
                "parcel",
                data(&[("parcel_weight", crate::lists::lit("1 kilogram"))])
            ),
            "shipping"
        ),
        "4.95 eur"
    );
}

#[test]
fn row_valued_rule_mixes_fixed_instance_with_find() {
    let engine = load_ok(
        r#"
spec menu_item
data drink: text
data price: measure
  -> unit eur: 1.00
  -> decimals 2
spec coffee_order
uses featured: menu_item
  -> with drink: "mocha", price: 4.00 eur
uses menu: menu_item list
  -> key drink
  -> with drink: "espresso", price: 2.50 eur
data ordered: text
data special: boolean
rule selected:
  menu -> find ordered
  unless special then featured
rule price:
  selected.price
"#,
    );
    let response = evaluate(
        &engine,
        "coffee_order",
        data(&[
            ("ordered", crate::lists::lit("espresso")),
            ("special", crate::lists::boolean(true)),
        ]),
    );
    assert_eq!(value(&response, "price"), "4.00 eur");
}

#[test]
fn row_valued_rule_wrong_spec_is_planning_error() {
    let err = load_planning_err(
        r#"
spec menu_item
data drink: text
spec employee
data name: text
spec coffee_order
uses featured: employee
  -> with name: "Ada"
uses menu: menu_item list
  -> with drink: "espresso"
data special: boolean
rule selected:
  menu -> find 1
  unless special then featured
"#,
    );
    assert_contains(&err, "menu_item");
}

#[test]
fn list_valued_rule_requires_same_key() {
    let err = load_planning_err(
        r#"
spec rate
data band: text
data price: number
spec parcel
uses standard_rates: rate list
  -> key band
  -> with band: "light", price: 1
uses express_rates: rate list
  -> with band: "light", price: 2
data express: boolean
rule rates:
  standard_rates
  unless express then express_rates
"#,
    );
    assert_contains(&err, "band");
}

#[test]
fn vetoed_row_rule_vetoes_only_where_read() {
    let engine = load_ok(
        r#"
spec employee
data name: text
data salary: measure
  -> unit eur: 1.00
rule bonus:
  salary * 10%
  unless salary < 0 eur then veto "negative salary"
spec payroll
uses staff: employee list
  -> with name: "Ada", salary: 9000 eur
  -> with name: "Neg", salary: -1 eur
rule names:
  staff -> count
rule first_bonus:
  (staff -> first).bonus
rule last_bonus:
  (staff -> last).bonus
"#,
    );
    let response = evaluate(&engine, "payroll", data(&[]));
    assert_eq!(value(&response, "names"), "2");
    assert_eq!(value(&response, "first_bonus"), "900 eur");
    assert_contains(&veto_reason(&response, "last_bonus"), "negative salary");
}

//! Worked examples from engine/documentation/design/lists.md, verbatim.

use crate::lists::{assert_contains, data, evaluate, lit, load_ok, load_planning_err, value};

const COFFEE_ORDER: &str = r#"
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
  -> with drink: "latte",    price: 3.50 eur

data ordered:  menu.drink
data quantity: number


rule selected_drink:
  menu -> find ordered

rule total:
  selected_drink.price * quantity

rule cheapest:
  menu -> min price
"#;

const PARCEL: &str = r#"
spec rate

data weight: measure
  -> unit kilogram: 1.00

data money: measure
  -> unit eur: 1.00
  -> decimals 2

data band:  weight range
data price: money


spec parcel_shipping

uses standard_rates: rate list
  -> key band
  -> with band: 0 kilogram...2 kilogram,     price: 4.95 eur
  -> with band: 2 kilogram...10 kilogram,    price: 6.95 eur
  -> with band: 10 kilogram...23 kilogram,   price: 13.95 eur

uses express_rates: rate list
  -> key band
  -> with band: 0 kilogram...2 kilogram,     price: 9.95 eur
  -> with band: 2 kilogram...10 kilogram,    price: 12.95 eur
  -> with band: 10 kilogram...23 kilogram,   price: 19.95 eur

data parcel_weight: standard_rates.weight
data express:       boolean


rule rates:
  standard_rates
  unless express then express_rates

rule shipping_cost:
  (rates -> find parcel_weight).price
"#;

const FREIGHT: &str = r#"
spec freight_rate

data weight: measure
  -> unit kilogram: 1.00

data volume: measure
  -> unit liter: 1.00

data money: measure
  -> unit eur: 1.00
  -> decimals 2

data band:        weight range
data volume_band: volume range
data price:       money


spec freight

uses rates: freight_rate list
  -> key band, volume_band
  -> with band: 0 kilogram...5 kilogram,    volume_band: 0 liter...2 liter,    price: 6.95 eur
  -> with band: 0 kilogram...5 kilogram,    volume_band: 2 liter...20 liter,   price: 9.95 eur
  -> with band: 5 kilogram...20 kilogram,   volume_band: 0 liter...20 liter,   price: 14.95 eur

data parcel_weight: rates.weight
data parcel_volume: rates.volume


rule shipping_cost:
  (rates -> find parcel_weight, parcel_volume).price
"#;

const PAYROLL: &str = r#"
spec employee

data name: text
data salary: measure
  -> unit eur: 1.00
  -> decimals 2


rule bonus:
  salary * 10%


spec payroll

uses staff: employee list
  -> with name: "Ada",   salary: 9000 eur
  -> with name: "Grace", salary: 9500 eur


rule total_bonus:
  staff -> sum bonus

rule top_bonus:
  (staff -> sort salary desc -> first).bonus
"#;

const INCOME_TAX: &str = r#"
spec tax_bracket

uses finance

data income: finance.money
data band:   finance.money range
data rate:   ratio


rule taxable:
  band as eur
  unless income < upper band  then income - lower band
  unless income <= lower band then 0 eur

rule tax:
  taxable * rate


spec income_tax

uses finance

data income: finance.money

uses brackets: tax_bracket list
  -> with band: 0 eur...38000 eur,       rate: 36%,  income: income
  -> with band: 38000 eur...76000 eur,   rate: 37%,  income: income


rule tax:
  brackets -> sum tax
"#;

const FINANCE: &str = r#"
spec finance
data money: measure
  -> unit eur: 1.00
"#;

#[test]
fn coffee_order() {
    let engine = load_ok(COFFEE_ORDER);
    let response = evaluate(
        &engine,
        "coffee_order",
        data(&[("ordered", lit("espresso")), ("quantity", lit("2"))]),
    );
    assert_eq!(value(&response, "total"), "5.00 eur");
    assert_eq!(value(&response, "cheapest"), "2.50 eur");
}

#[test]
fn parcel_shipping() {
    let engine = load_ok(PARCEL);
    let standard = evaluate(
        &engine,
        "parcel_shipping",
        data(&[
            ("parcel_weight", lit("1 kilogram")),
            ("express", crate::lists::boolean(false)),
        ]),
    );
    assert_eq!(value(&standard, "shipping_cost"), "4.95 eur");
    let express = evaluate(
        &engine,
        "parcel_shipping",
        data(&[
            ("parcel_weight", lit("1 kilogram")),
            ("express", crate::lists::boolean(true)),
        ]),
    );
    assert_eq!(value(&express, "shipping_cost"), "9.95 eur");
}

#[test]
fn dropping_key_from_express_rates_is_planning_error() {
    let err = load_planning_err(
        r#"
spec rate
data band: text
data price: number
spec parcel_shipping
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
fn freight_find_4kg_1l_is_6_95() {
    let engine = load_ok(FREIGHT);
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
fn payroll() {
    let engine = load_ok(PAYROLL);
    let response = evaluate(&engine, "payroll", data(&[]));
    assert_eq!(value(&response, "total_bonus"), "1850.00 eur");
    assert_eq!(value(&response, "top_bonus"), "950.00 eur");
}

#[test]
fn income_tax() {
    let engine = crate::lists::load_files(&[("finance.lemma", FINANCE), ("tax.lemma", INCOME_TAX)]);
    let response = evaluate(&engine, "income_tax", data(&[("income", lit("40000 eur"))]));
    assert_eq!(value(&response, "tax"), "14420 eur");
}

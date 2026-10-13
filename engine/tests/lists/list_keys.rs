//! Field, range, composite, and numbered keys.

use crate::lists::{
    assert_contains, data, date, evaluate, evaluate_at, list, lit, load_files,
    load_files_planning_err, load_ok, load_planning_err, map, value, veto_reason,
};
use indexmap::IndexMap;

#[test]
fn duplicate_source_key_is_planning_error() {
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
  -> with drink: "espresso", price: 3.00 eur
"#,
    );
    assert_contains(&err, "espresso");
}

#[test]
fn default_key_value_counts_as_duplicate() {
    let err = load_planning_err(
        r#"
spec menu_item
data drink: text
  -> with "espresso"
data price: measure
  -> unit eur: 1.00
spec coffee_order
uses menu: menu_item list
  -> key drink
  -> with price: 2.50 eur
  -> with drink: "espresso", price: 3.00 eur
"#,
    );
    assert_contains(&err, "espresso");
}

#[test]
fn source_rows_need_not_cover_every_option() {
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
data ordered: menu.drink
rule selected:
  (menu -> find ordered).price
rule n:
  menu -> count
"#,
    );
    let response = evaluate(&engine, "coffee_order", data(&[("ordered", lit("latte"))]));
    assert_eq!(value(&response, "n"), "1");
    assert_contains(&veto_reason(&response, "selected"), "latte");
}

#[test]
fn empty_keyed_list_skips_options_coverage() {
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
rule n:
  menu -> count
"#,
    );
    assert_eq!(
        value(&evaluate(&engine, "coffee_order", data(&[])), "n"),
        "0"
    );
}

#[test]
fn find_succeeds_when_option_has_a_source_row() {
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
rule selected:
  (menu -> find ordered).price
"#,
    );
    assert_eq!(
        value(
            &evaluate(&engine, "coffee_order", data(&[("ordered", lit("latte"))])),
            "selected"
        ),
        "3.50 eur"
    );
}

#[test]
fn evaluate_data_missing_option_vetoes_only_that_find() {
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
data ordered: menu.drink
rule selected:
  (menu -> find ordered).price
rule n:
  menu -> count
"#,
    );
    let response = evaluate(
        &engine,
        "coffee_order",
        data(&[
            (
                "menu",
                list(vec![map(&[
                    ("drink", lit("espresso")),
                    ("price", lit("2.50 eur")),
                ])]),
            ),
            ("ordered", lit("latte")),
        ]),
    );
    assert_eq!(value(&response, "n"), "1");
    assert_contains(&veto_reason(&response, "selected"), "latte");
}

#[test]
fn open_key_find_missing_vetoes() {
    let engine = load_ok(
        r#"
spec menu_item
data drink: text
data price: measure
  -> unit eur: 1.00
spec coffee_order
uses menu: menu_item list
  -> key drink
  -> with drink: "espresso", price: 2.50 eur
data ordered: menu.drink
rule selected:
  (menu -> find ordered).price
"#,
    );
    let response = evaluate(&engine, "coffee_order", data(&[("ordered", lit("mocha"))]));
    assert_contains(&veto_reason(&response, "selected"), "mocha");
}

#[test]
fn range_key_contains_and_converts_units() {
    let engine = load_ok(
        r#"
spec rate
data weight: measure
  -> unit kilogram: 1.00
  -> unit gram: 0.001
data money: measure
  -> unit eur: 1.00
data band: weight range
data price: money
spec parcel
uses rates: rate list
  -> key band
  -> with band: 0 kilogram...2 kilogram, price: 4.95 eur
  -> with band: 2 kilogram...10 kilogram, price: 6.95 eur
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
                data(&[("parcel_weight", lit("1500 gram"))])
            ),
            "shipping"
        ),
        "4.95 eur"
    );
}

#[test]
fn range_key_half_open_upper_bound_goes_to_next_row() {
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
  -> with band: 2 kilogram...10 kilogram, price: 6.95 eur
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
                data(&[("parcel_weight", lit("2 kilogram"))])
            ),
            "shipping"
        ),
        "6.95 eur"
    );
}

#[test]
fn range_key_overlap_is_planning_error() {
    let err = load_planning_err(
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
  -> with band: 0 kilogram...3 kilogram, price: 4.95 eur
  -> with band: 2 kilogram...10 kilogram, price: 6.95 eur
"#,
    );
    assert_contains(&err, "overlap");
}

#[test]
fn range_key_no_containing_range_vetoes() {
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
    let response = evaluate(
        &engine,
        "parcel",
        data(&[("parcel_weight", lit("9 kilogram"))]),
    );
    assert_contains(&veto_reason(&response, "shipping"), "9 kilogram");
}

#[test]
fn composite_find_wrong_value_count_is_planning_error() {
    let err = load_planning_err(
        r#"
spec freight_rate
data weight: measure
  -> unit kilogram: 1.00
data volume: measure
  -> unit liter: 1.00
data band: weight range
data volume_band: volume range
data price: measure
  -> unit eur: 1.00
spec freight
uses rates: freight_rate list
  -> key band, volume_band
  -> with band: 0 kilogram...5 kilogram, volume_band: 0 liter...2 liter, price: 6.95 eur
data parcel_weight: rates.weight
rule shipping:
  (rates -> find parcel_weight).price
"#,
    );
    assert_contains(&err, "2 key fields");
}

#[test]
fn composite_find_wrong_type_is_planning_error() {
    let err = load_planning_err(
        r#"
spec freight_rate
data weight: measure
  -> unit kilogram: 1.00
data volume: measure
  -> unit liter: 1.00
data band: weight range
data volume_band: volume range
data price: measure
  -> unit eur: 1.00
spec freight
uses rates: freight_rate list
  -> key band, volume_band
  -> with band: 0 kilogram...5 kilogram, volume_band: 0 liter...2 liter, price: 6.95 eur
data parcel_weight: rates.weight
data label: text
rule shipping:
  (rates -> find parcel_weight, label).price
"#,
    );
    assert_contains(&err, "volume");
}

#[test]
fn composite_uniqueness_allows_disjoint_ranges() {
    let engine = load_ok(
        r#"
spec freight_rate
data weight: measure
  -> unit kilogram: 1.00
data volume: measure
  -> unit liter: 1.00
data band: weight range
data volume_band: volume range
data price: measure
  -> unit eur: 1.00
spec freight
uses rates: freight_rate list
  -> key band, volume_band
  -> with band: 0 kilogram...5 kilogram, volume_band: 0 liter...2 liter, price: 6.95 eur
  -> with band: 0 kilogram...5 kilogram, volume_band: 2 liter...20 liter, price: 9.95 eur
data parcel_weight: rates.weight
data parcel_volume: rates.volume
rule shipping:
  (rates -> find parcel_weight, parcel_volume).price
"#,
    );
    assert_eq!(
        value(
            &evaluate(
                &engine,
                "freight",
                data(&[
                    ("parcel_weight", lit("4 kilogram")),
                    ("parcel_volume", lit("1 liter")),
                ])
            ),
            "shipping"
        ),
        "6.95 eur"
    );
}

#[test]
fn composite_source_rows_need_not_cover_every_combination() {
    let engine = load_ok(
        r#"
spec cell
data band: text
  -> options "light", "heavy"
data volume: text
  -> options "small", "large"
data price: measure
  -> unit eur: 1.00
spec freight
uses rates: cell list
  -> key band, volume
  -> with band: "light", volume: "small", price: 6.95 eur
  -> with band: "light", volume: "large", price: 9.95 eur
  -> with band: "heavy", volume: "small", price: 14.95 eur
data band: rates.band
data volume: rates.volume
rule selected:
  (rates -> find band, volume).price
rule n:
  rates -> count
"#,
    );
    let response = evaluate(
        &engine,
        "freight",
        data(&[("band", lit("heavy")), ("volume", lit("large"))]),
    );
    assert_eq!(value(&response, "n"), "3");
    assert_contains(&veto_reason(&response, "selected"), "heavy");
}

#[test]
fn same_field_twice_in_key_is_planning_error() {
    let err = load_planning_err(
        r#"
spec rate
data band: text
spec parcel
uses rates: rate list
  -> key band, band
"#,
    );
    assert_contains(&err, "band");
}

#[test]
fn numbered_find_zero_is_planning_error() {
    let err = load_planning_err(
        r#"
spec fares_card
data fares: number list
  -> with 1, 2
rule picked:
  fares -> find 0
"#,
    );
    assert_contains(&err, "row number");
}

#[test]
fn numbered_find_negative_is_planning_error() {
    let err = load_planning_err(
        r#"
spec fares_card
data fares: number list
  -> with 1, 2
rule picked:
  fares -> find(-1)
"#,
    );
    assert_contains(&err, "-1");
}

#[test]
fn numbered_find_fractional_is_planning_error() {
    let err = load_planning_err(
        r#"
spec fares_card
data fares: number list
  -> with 1, 2
rule picked:
  fares -> find 1.5
"#,
    );
    assert_contains(&err, "1.5");
}

#[test]
fn numbered_find_out_of_range_vetoes() {
    let engine = load_ok(
        r#"
spec fares_card
data fares: number list
  -> with 1, 2
rule third:
  fares -> find 3
"#,
    );
    assert_contains(
        &veto_reason(&evaluate(&engine, "fares_card", data(&[])), "third"),
        "3",
    );
}

#[test]
fn temporal_key_change_breaks_unpinned_consumers() {
    let err = load_files_planning_err(&[
        (
            "menu_item.lemma",
            r#"
spec menu_item
data drink: text
data size: text
data price: number
"#,
        ),
        (
            "menu_card.lemma",
            r#"
spec menu_card 2024-01-01
uses menu: menu_item list
  -> key drink

spec menu_card 2025-01-01
uses menu: menu_item list
  -> key drink, size
"#,
        ),
        (
            "order.lemma",
            r#"
spec coffee_order
uses card: menu_card
rule drinks:
  card.menu -> count
"#,
        ),
    ]);
    assert_contains(&err, "drink");
    assert_contains(&err, "size");
}

#[test]
fn temporal_key_options_change_breaks_unpinned_consumers() {
    let err = load_files_planning_err(&[
        (
            "menu_item.lemma",
            r#"
spec menu_item 2024-01-01
data drink: text
  -> options "espresso", "latte"

spec menu_item 2025-01-01
data drink: text
  -> options "espresso", "latte", "mocha"
"#,
        ),
        (
            "order.lemma",
            r#"
spec coffee_order
uses menu: menu_item list
  -> key drink
  -> with drink: "espresso"
  -> with drink: "latte"
"#,
        ),
    ]);
    assert_contains(&err, "drink");
    assert_contains(&err, "changed");
}

#[test]
fn pinned_row_spec_ignores_later_interface_change() {
    let engine = load_files(&[
        (
            "menu_item.lemma",
            r#"
spec menu_item 2024-01-01
data drink: text
data price: number

spec menu_item 2025-01-01
data drink: text
data size: text
data price: number
"#,
        ),
        (
            "order.lemma",
            r#"
spec coffee_order
uses menu: menu_item 2024 list
  -> key drink
  -> with drink: "espresso", price: 1
rule n:
  menu -> count
"#,
        ),
    ]);
    let response = evaluate_at(
        &engine,
        "coffee_order",
        &date(2026, 1, 1),
        IndexMap::new(),
        None,
        false,
    );
    assert_eq!(value(&response, "n"), "1");
}

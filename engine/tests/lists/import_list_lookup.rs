//! A consumer reads an instance list that lives on an imported spec.
//!
//! Source rows on that list are the list. The consumer does not send the list
//! again. `uses name: spec list` stays a parse error.

use crate::lists::{
    assert_contains, data, evaluate, list, lit, load_err, load_ok, map, missing_data, value,
    veto_reason,
};

const TABLE: &str = r#"
spec rij
data sleutel: text
data bedrag: number

spec tabel
uses rij
data rijen: rij list
  -> key sleutel
  -> with sleutel: "c", bedrag: 1

spec loon
uses tabellen: tabel
data gekozen: text
data schaal: text
rule bedrag:
  (tabellen.rijen -> find gekozen).bedrag
rule uurloon:
  (tabellen.rijen -> find gekozen).bedrag
  unless schaal is "B" then 2
rule vast:
  2
"#;

#[test]
fn uses_rij_list_is_rejected() {
    let err = load_err(
        r#"
spec rij
data sleutel: text
data bedrag: number

spec tabel
uses rijen: rij list
  -> key sleutel
  -> with sleutel: "c", bedrag: 1

spec loon
uses tabellen: tabel
data gekozen: text
rule bedrag:
  (tabellen.rijen -> find gekozen).bedrag
"#,
    );
    assert_contains(&err, "uses");
    assert_contains(&err, "list");
    assert_contains(&err, "data");
}

#[test]
fn imported_source_rows_find_returns_bedrag() {
    let engine = load_ok(TABLE);
    let response = evaluate(
        &engine,
        "loon",
        data(&[("gekozen", lit("c")), ("schaal", lit("A"))]),
    );
    assert!(missing_data(&response, "bedrag").is_empty());
    assert_eq!(value(&response, "bedrag"), "1");
    assert_eq!(value(&response, "uurloon"), "1");
    assert_eq!(value(&response, "vast"), "2");
}

#[test]
fn imported_source_rows_find_miss_is_a_veto() {
    let engine = load_ok(TABLE);
    let response = evaluate(
        &engine,
        "loon",
        data(&[("gekozen", lit("z")), ("schaal", lit("A"))]),
    );
    assert!(missing_data(&response, "bedrag").is_empty());
    assert_contains(&veto_reason(&response, "bedrag"), "No row found");
    assert_contains(&veto_reason(&response, "uurloon"), "No row found");
    assert_eq!(value(&response, "vast"), "2");
}

#[test]
fn unbound_find_key_lists_only_that_key() {
    let engine = load_ok(TABLE);
    let response = evaluate(&engine, "loon", data(&[("schaal", lit("A"))]));
    let bedrag = response.results.get("bedrag").expect("bedrag");
    assert!(bedrag.is_missing_data());
    assert_eq!(
        missing_data(&response, "bedrag"),
        vec!["gekozen".to_string()]
    );
    assert!(!missing_data(&response, "bedrag")
        .iter()
        .any(|key| key.contains("rijen")));
}

#[test]
fn scale_that_skips_the_table_still_answers() {
    let engine = load_ok(TABLE);
    let response = evaluate(&engine, "loon", data(&[("schaal", lit("B"))]));
    assert!(missing_data(&response, "uurloon").is_empty());
    assert_eq!(value(&response, "uurloon"), "2");
    assert_eq!(value(&response, "vast"), "2");
    let bedrag = response.results.get("bedrag").expect("bedrag");
    assert!(bedrag.is_missing_data());
    assert_eq!(
        missing_data(&response, "bedrag"),
        vec!["gekozen".to_string()]
    );
}

#[test]
fn evaluate_data_replaces_imported_source_rows() {
    let engine = load_ok(TABLE);
    let response = evaluate(
        &engine,
        "loon",
        data(&[
            ("gekozen", lit("c")),
            ("schaal", lit("A")),
            (
                "tabellen.rijen",
                list(vec![map(&[("sleutel", lit("c")), ("bedrag", lit("9"))])]),
            ),
        ]),
    );
    assert_eq!(value(&response, "bedrag"), "9");
}

#[test]
fn imported_list_without_rows_counts_as_zero() {
    let engine = load_ok(
        r#"
spec rij
data sleutel: text
data bedrag: number

spec tabel
uses rij
data rijen: rij list
  -> key sleutel

spec loon
uses tabellen: tabel
rule n:
  tabellen.rijen -> count
"#,
    );
    let response = evaluate(&engine, "loon", data(&[]));
    assert!(missing_data(&response, "n").is_empty());
    assert_eq!(value(&response, "n"), "0");
}

#[test]
fn unbound_row_reference_asks_the_owner_field() {
    let engine = load_ok(
        r#"
spec rij
data sleutel: text
data bedrag: number

spec tabel
uses rij
data toeslag: number
data rijen: rij list
  -> key sleutel
  -> with sleutel: "c", bedrag: toeslag

spec loon
uses tabellen: tabel
data gekozen: text
rule bedrag:
  (tabellen.rijen -> find gekozen).bedrag
"#,
    );
    let response = evaluate(&engine, "loon", data(&[("gekozen", lit("c"))]));
    let bedrag = response.results.get("bedrag").expect("bedrag");
    assert!(bedrag.is_missing_data(), "{bedrag:?}");
    assert_eq!(
        missing_data(&response, "bedrag"),
        vec!["tabellen.toeslag".to_string()]
    );
}

#[test]
fn sealed_rule_list_on_an_import_finds_the_row() {
    let engine = load_ok(
        r#"
spec rij
data sleutel: text
data bedrag: number

spec tabel
uses rij
rule rijen:
  rij list
    -> key sleutel
    -> with sleutel: "c", bedrag: 1

spec loon
uses tabellen: tabel
data gekozen: text
rule bedrag:
  (tabellen.rijen -> find gekozen).bedrag
"#,
    );
    let response = evaluate(&engine, "loon", data(&[("gekozen", lit("c"))]));
    assert!(missing_data(&response, "bedrag").is_empty());
    assert_eq!(value(&response, "bedrag"), "1");
}

#[test]
fn imported_list_find_returns_the_second_row() {
    let engine = load_ok(
        r#"
spec entry
data code: text
data amount: number

spec sheet
uses entry
data rows: entry list
  -> key code
  -> with code: "c", amount: 1
  -> with code: "d", amount: 4

spec reader
uses sheet
data chosen: text
rule amount:
  (sheet.rows -> find chosen).amount
"#,
    );
    let response = evaluate(&engine, "reader", data(&[("chosen", lit("d"))]));
    assert!(missing_data(&response, "amount").is_empty());
    assert_eq!(value(&response, "amount"), "4");
}

#[test]
fn imported_list_find_uses_both_key_fields() {
    let engine = load_ok(
        r#"
spec entry
data group: text
data code: text
data amount: number

spec sheet
uses entry
data rows: entry list
  -> key group, code
  -> with group: "a", code: "1", amount: 3
  -> with group: "a", code: "2", amount: 8

spec reader
uses sheet
data group: text
data code: text
rule amount:
  (sheet.rows -> find group, code).amount
"#,
    );
    let response = evaluate(
        &engine,
        "reader",
        data(&[("group", lit("a")), ("code", lit("2"))]),
    );
    assert!(missing_data(&response, "amount").is_empty());
    assert_eq!(value(&response, "amount"), "8");
}

#[test]
fn omitted_imported_cell_is_the_row_field() {
    let engine = load_ok(
        r#"
spec entry
data code: text
data amount: number

spec sheet
uses entry
data rows: entry list
  -> key code
  -> with code: "c"

spec reader
uses sheet
data chosen: text
rule amount:
  (sheet.rows -> find chosen).amount
"#,
    );
    let response = evaluate(&engine, "reader", data(&[("chosen", lit("c"))]));
    let amount = response.results.get("amount").expect("amount");
    assert!(amount.is_missing_data(), "{amount:?}");
    assert_eq!(
        missing_data(&response, "amount"),
        vec!["sheet.rows.1.amount".to_string()]
    );
}

// The owner's `uses entry -> with amount: 1` seals `sheet.entry.amount` only; the
// omitted cell is the row's own key.
#[test]
fn owner_seal_fills_an_omitted_imported_cell() {
    let engine = load_ok(
        r#"
spec entry
data code: text
data amount: number

spec sheet
uses entry
  -> with amount: 1
data rows: entry list
  -> key code
  -> with code: "c"

spec reader
uses sheet
data chosen: text
rule amount:
  (sheet.rows -> find chosen).amount
"#,
    );
    let response = evaluate(&engine, "reader", data(&[("chosen", lit("c"))]));
    assert_eq!(
        missing_data(&response, "amount"),
        vec!["sheet.rows.1.amount".to_string()]
    );
    let filled = evaluate(
        &engine,
        "reader",
        data(&[("chosen", lit("c")), ("sheet.rows.1.amount", lit("1"))]),
    );
    assert!(missing_data(&filled, "amount").is_empty());
    assert_eq!(value(&filled, "amount"), "1");
}

#[test]
fn consumer_seal_does_not_fill_an_imported_cell() {
    let engine = load_ok(
        r#"
spec entry
data code: text
data amount: number

spec sheet
uses entry
data rows: entry list
  -> key code
  -> with code: "c"

spec reader
uses entry
  -> with amount: 99
uses sheet
data chosen: text
rule amount:
  (sheet.rows -> find chosen).amount
"#,
    );
    let response = evaluate(&engine, "reader", data(&[("chosen", lit("c"))]));
    let amount = response.results.get("amount").expect("amount");
    assert!(amount.is_missing_data(), "{amount:?}");
    assert_eq!(
        missing_data(&response, "amount"),
        vec!["sheet.rows.1.amount".to_string()]
    );
}

#[test]
fn owner_seal_wins_over_the_consumer_seal() {
    let engine = load_ok(
        r#"
spec entry
data code: text
data amount: number

spec sheet
uses entry
  -> with amount: 1
data rows: entry list
  -> key code
  -> with code: "c"

spec reader
uses entry
  -> with amount: 99
uses sheet
data chosen: text
rule amount:
  (sheet.rows -> find chosen).amount
"#,
    );
    let response = evaluate(&engine, "reader", data(&[("chosen", lit("c"))]));
    assert_eq!(
        missing_data(&response, "amount"),
        vec!["sheet.rows.1.amount".to_string()],
        "neither seal reaches the row"
    );
}

#[test]
fn supplied_imported_reference_is_the_cell_value() {
    let engine = load_ok(
        r#"
spec entry
data code: text
data amount: number

spec sheet
uses entry
data factor: number
data rows: entry list
  -> key code
  -> with code: "c", amount: factor

spec reader
uses sheet
data chosen: text
rule amount:
  (sheet.rows -> find chosen).amount
"#,
    );
    let response = evaluate(
        &engine,
        "reader",
        data(&[("chosen", lit("c")), ("sheet.factor", lit("6"))]),
    );
    assert!(missing_data(&response, "amount").is_empty());
    assert_eq!(value(&response, "amount"), "6");
}

#[test]
fn imported_list_two_hops_finds_the_source_row() {
    let engine = load_ok(
        r#"
spec entry
data code: text
data amount: number

spec inner
uses entry
data rows: entry list
  -> key code
  -> with code: "c", amount: 1

spec sheet
uses inner

spec reader
uses sheet
data chosen: text
rule amount:
  (sheet.inner.rows -> find chosen).amount
"#,
    );
    let response = evaluate(&engine, "reader", data(&[("chosen", lit("c"))]));
    assert!(missing_data(&response, "amount").is_empty());
    assert_eq!(value(&response, "amount"), "1");
}

const SHEET_ASSIGNED_AMOUNT: &str = r#"
spec entry
data code: text
data amount: number

spec sheet
uses entry
  -> with amount: 1
data rows: entry list
  -> key code
  -> with code: "c"

spec reader
uses entry
  -> with amount: 99
uses sheet
data chosen: text
rule amount:
  (sheet.rows -> find chosen).amount
"#;

#[test]
fn caller_sent_sheet_rows_keep_sheets_assigned_amount() {
    let engine = load_ok(SHEET_ASSIGNED_AMOUNT);
    let response = evaluate(
        &engine,
        "reader",
        data(&[
            ("chosen", lit("d")),
            ("sheet.rows", list(vec![map(&[("code", lit("d"))])])),
        ]),
    );
    assert_eq!(
        missing_data(&response, "amount"),
        vec!["sheet.rows.1.amount".to_string()],
        "the sheet's `uses entry` literal is not a row default"
    );
}

#[test]
fn caller_sent_sheet_rows_cannot_supply_sheets_assigned_amount() {
    let engine = load_ok(SHEET_ASSIGNED_AMOUNT);
    let response = evaluate(
        &engine,
        "reader",
        data(&[
            ("chosen", lit("d")),
            (
                "sheet.rows",
                list(vec![map(&[("code", lit("d")), ("amount", lit("5"))])]),
            ),
        ]),
    );
    assert!(missing_data(&response, "amount").is_empty());
    assert_eq!(
        value(&response, "amount"),
        "5",
        "a caller row's `amount` is its own cell, not the sheet's sealed instance field"
    );
}

//! An instance produced inside a row supplied by evaluation data carries that row:
//! its fields are read in the row's context and it publishes under the row.

use crate::lists::{data, evaluate, evaluate_explain, list, lit, load_ok, map, value};
use lemma::format_explanation;

const ORDER: &str = r#"
spec item
data target: number

spec box
uses item
data items: item list

spec line
data amount: number
uses i1: item
  -> with target: base
uses holder: box
  -> with items: i1
rule base: amount * 10
rule picked: holder.items
rule pick: i1

spec order
uses line
data lines: line list
rule last_picks: (lines -> last).picked
rule last_total: last_picks -> sum last_picks.target
rule first_pick: (lines -> first).pick
rule first_target: first_pick.target
rule totals: lines -> sum (lines.picked -> sum lines.picked.target)
rule high: last_picks -> where target > 15
rule low: last_picks -> where target < 15
"#;

fn two_lines() -> indexmap::IndexMap<String, lemma::EvaluateDataValue> {
    data(&[(
        "lines",
        list(vec![
            map(&[("amount", lit("1"))]),
            map(&[("amount", lit("2"))]),
        ]),
    )])
}

fn row_instances(response: &lemma::Response, rule: &str) -> Vec<(Option<String>, usize)> {
    response
        .results
        .get(rule)
        .unwrap_or_else(|| panic!("rule {rule}"))
        .result
        .as_ref()
        .unwrap_or_else(|| panic!("result of {rule}"))
        .list
        .as_ref()
        .expect("list arm")
        .iter()
        .map(|item| {
            let row = item.row.as_ref().expect("row arm");
            (row.instance.clone(), row.index)
        })
        .collect()
}

#[test]
fn row_field_on_a_row_bound_instance_reads_in_its_row() {
    let engine = load_ok(ORDER);
    let response = evaluate(&engine, "order", two_lines());
    assert_eq!(value(&response, "last_total"), "20");
    assert_eq!(value(&response, "totals"), "30");
}

#[test]
fn member_of_a_row_bound_instance_reads_in_its_row() {
    let engine = load_ok(ORDER);
    let response = evaluate(&engine, "order", two_lines());
    assert_eq!(value(&response, "first_target"), "10");
}

#[test]
fn row_bound_instances_publish_under_their_row() {
    let engine = load_ok(ORDER);
    let response = evaluate(&engine, "order", two_lines());
    let picks = row_instances(&response, "last_picks");
    assert_eq!(picks.len(), 1);
    let (instance, index) = &picks[0];
    assert_eq!(instance, &Some("lines.2.holder.items.1".to_string()));
    let row = &response.rows[*index];
    assert_eq!(row.spec, "line");
    let base = row
        .rules
        .get("base")
        .and_then(|entry| entry.result.as_ref())
        .and_then(|value| value.result.clone())
        .expect("base of the row");
    assert_eq!(base, "20");
}

#[test]
fn where_over_row_bound_instances_keeps_the_matching_rows() {
    let engine = load_ok(ORDER);
    let response = evaluate(&engine, "order", two_lines());
    let high = row_instances(&response, "high");
    assert_eq!(high.len(), 1);
    let (instance, index) = &high[0];
    assert_eq!(instance, &Some("lines.2.holder.items.1".to_string()));
    assert_eq!(response.rows[*index].spec, "line");
    assert!(row_instances(&response, "low").is_empty());
}

#[test]
fn explanation_names_the_row_of_a_row_bound_read() {
    let engine = load_ok(ORDER);
    let response = evaluate_explain(&engine, "order", two_lines());
    assert_eq!(value(&response, "last_total"), "20");
    assert_eq!(value(&response, "totals"), "30");
    assert_eq!(value(&response, "first_target"), "10");
    let explanation = response
        .results
        .get("last_total")
        .expect("last_total")
        .explanation
        .as_ref()
        .expect("explanation");
    let text = format_explanation(explanation);
    assert!(
        text.contains("last_picks: lines.2.holder.items.1"),
        "{text}"
    );
    assert!(text.contains("2. picked: holder.items.1"), "{text}");
    assert!(text.contains("2. base: 20"), "{text}");
    assert!(text.contains("amount: 2"), "{text}");
}

const NESTED: &str = r#"
spec item
data target: number

spec line
data amount: number
uses i1: item
  -> with target: base
rule base: amount * 10
rule pick: i1

spec shipment
uses line
data lines: line list
rule first_pick: (lines -> first).pick

spec order
uses shipment
data shipments: shipment list
rule picked: (shipments -> first).first_pick
rule target: picked.target
"#;

#[test]
fn row_bound_instance_of_a_nested_row_reads_in_that_row() {
    let engine = load_ok(NESTED);
    let response = evaluate(
        &engine,
        "order",
        data(&[(
            "shipments",
            list(vec![map(&[(
                "lines",
                list(vec![map(&[("amount", lit("3"))])]),
            )])]),
        )]),
    );
    assert_eq!(value(&response, "target"), "30");
    let picked = response
        .results
        .get("picked")
        .expect("picked")
        .result
        .as_ref()
        .expect("picked result")
        .row
        .as_ref()
        .expect("row arm");
    assert_eq!(picked.instance, Some("lines.1.i1".to_string()));
    assert_eq!(response.rows[picked.index].spec, "line");
}

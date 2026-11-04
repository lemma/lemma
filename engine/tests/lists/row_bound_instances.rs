//! An instance produced inside a row supplied by evaluation data carries that row:
//! its fields are read in the row's context and it publishes under the row.

use crate::lists::{
    data, evaluate, evaluate_at, evaluate_explain, list, lit, load_ok, map, now, value,
};
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

const DEFAULTED_FIELD_FROM_RULE: &str = r#"
spec pack
data pack_price: number
data units_per_pack: number
rule pack_unit_cost: pack_price / units_per_pack

spec part
data kind: text
data price: number
  -> with 0
data fixed_cost: number
  -> with 0
rule contribution: price
rule fixed: fixed_cost

spec assembly
uses part
data parts: part list
rule fixed_total: parts -> sum fixed
rule kind_b: parts -> where kind is "B" -> sum contribution
rule all_parts: parts -> sum contribution

spec test
uses pack_source: pack
  -> with pack_price: 21
  -> with units_per_pack: 3
uses item_a: part
  -> with kind: "B"
  -> with price: 10
  -> with fixed_cost: 500
uses item_b: part
  -> with kind: "B"
  -> with price: pack_source.pack_unit_cost
uses a: assembly
  -> with parts: item_a, item_b
rule a_fixed_total: a.fixed_total
rule b_kind_b: a.kind_b
rule c_all_parts: a.all_parts
"#;

fn evaluate_rules(engine: &lemma::Engine, rules: &[&str]) -> lemma::Response {
    let rules: Vec<String> = rules.iter().map(|rule| rule.to_string()).collect();
    evaluate_at(
        engine,
        "test",
        &now(),
        indexmap::IndexMap::new(),
        Some(&rules),
        false,
    )
}

#[test]
fn filtered_sum_reads_rule_bound_field_after_a_pass_that_skipped_it() {
    let engine = load_ok(DEFAULTED_FIELD_FROM_RULE);
    let response = evaluate_rules(&engine, &["a_fixed_total", "b_kind_b"]);
    assert_eq!(value(&response, "a_fixed_total"), "500");
    assert_eq!(value(&response, "b_kind_b"), "17");
}

#[test]
fn sum_reads_rule_bound_field_after_a_pass_that_skipped_it() {
    let engine = load_ok(DEFAULTED_FIELD_FROM_RULE);
    let response = evaluate_rules(&engine, &["a_fixed_total", "c_all_parts"]);
    assert_eq!(value(&response, "a_fixed_total"), "500");
    assert_eq!(value(&response, "c_all_parts"), "17");
}

const FILTER_EXCLUDES_FIRST_ROW: &str = r#"
spec pack
data pack_price: number
data units_per_pack: number
rule pack_unit_cost: pack_price / units_per_pack

spec part
data kind: text
data price: number
  -> with 0
data fixed_cost: number
  -> with 0
rule contribution: price
rule fixed: fixed_cost

spec assembly
uses part
data parts: part list
data divisor: number
rule fixed_total: parts -> sum fixed
rule kind_b: parts -> where kind is "B" -> sum contribution
rule guarded:
  (parts -> where kind is "B" -> sum contribution) / divisor
  unless parts -> where kind is "B" -> count is 0
    then 0

spec test
uses pack_source: pack
  -> with pack_price: 21
  -> with units_per_pack: 3
uses item_a: part
  -> with kind: "A"
  -> with price: 10
  -> with fixed_cost: 500
uses item_b: part
  -> with kind: "B"
  -> with price: pack_source.pack_unit_cost
uses a: assembly
  -> with parts: item_a, item_b
  -> with divisor: 1
rule a_fixed_total: a.fixed_total
rule b_kind_b: a.kind_b
rule c_guarded: a.guarded
"#;

#[test]
fn filtered_sum_of_only_rule_bound_row_after_a_pass_that_skipped_it() {
    let engine = load_ok(FILTER_EXCLUDES_FIRST_ROW);
    let response = evaluate_rules(&engine, &["a_fixed_total", "b_kind_b"]);
    assert_eq!(value(&response, "a_fixed_total"), "500");
    assert_eq!(value(&response, "b_kind_b"), "7");
}

#[test]
fn guarded_filtered_sum_of_only_rule_bound_row_after_a_pass_that_skipped_it() {
    let engine = load_ok(FILTER_EXCLUDES_FIRST_ROW);
    let response = evaluate_rules(&engine, &["a_fixed_total", "c_guarded"]);
    assert_eq!(value(&response, "a_fixed_total"), "500");
    assert_eq!(value(&response, "c_guarded"), "7");
}

#[test]
fn filtered_sum_of_only_rule_bound_row_alone() {
    let engine = load_ok(FILTER_EXCLUDES_FIRST_ROW);
    let response = evaluate_rules(&engine, &["b_kind_b"]);
    assert_eq!(value(&response, "b_kind_b"), "7");
}

#[test]
fn only_rule_bound_row_is_order_independent_across_all_rules() {
    let engine = load_ok(FILTER_EXCLUDES_FIRST_ROW);
    let response = evaluate(&engine, "test", indexmap::IndexMap::new());
    assert_eq!(value(&response, "a_fixed_total"), "500");
    assert_eq!(value(&response, "b_kind_b"), "7");
    assert_eq!(value(&response, "c_guarded"), "7");
}

#[test]
fn rule_bound_field_is_order_independent_across_all_rules() {
    let engine = load_ok(DEFAULTED_FIELD_FROM_RULE);
    let response = evaluate(&engine, "test", indexmap::IndexMap::new());
    assert_eq!(value(&response, "a_fixed_total"), "500");
    assert_eq!(value(&response, "b_kind_b"), "17");
    assert_eq!(value(&response, "c_all_parts"), "17");
}

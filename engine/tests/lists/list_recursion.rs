//! Self-lists: bicycle, offer bundle, shift chain, planning Errors, limits.

use crate::lists::{
    assert_contains, data, date, evaluate, evaluate_at, evaluate_err, evaluate_explain, list, lit,
    load_err, load_files, load_ok, load_ok_with_limits, load_planning_err, map, missing_data, now,
    value, veto_reason,
};
use lemma::ResourceLimits;

fn part_spec() -> &'static str {
    r#"
spec part
data money: measure
  -> unit eur: 1.00
uses components: part list
data quantity: number
data own_cost: money
rule unit_cost:
  own_cost + (components -> sum(components.quantity * components.unit_cost))
"#
}

#[test]
fn bicycle_unit_cost_is_361_eur() {
    let engine = load_ok(part_spec());
    let spoke = map(&[("quantity", lit("32")), ("own_cost", lit("0.25 eur"))]);
    let nipple = map(&[("quantity", lit("1")), ("own_cost", lit("22 eur"))]);
    let hub = map(&[("quantity", lit("1")), ("own_cost", lit("18 eur"))]);
    let rim = map(&[("quantity", lit("1")), ("own_cost", lit("5 eur"))]);
    let wheel = map(&[
        ("quantity", lit("2")),
        ("own_cost", lit("0 eur")),
        ("components", list(vec![rim, hub, spoke, nipple])),
    ]);
    let chainring = map(&[("quantity", lit("1")), ("own_cost", lit("25 eur"))]);
    let cassette = map(&[("quantity", lit("1")), ("own_cost", lit("60 eur"))]);
    let crank = map(&[("quantity", lit("1")), ("own_cost", lit("10 eur"))]);
    let drivetrain = map(&[
        ("quantity", lit("1")),
        ("own_cost", lit("0 eur")),
        ("components", list(vec![crank, chainring, cassette])),
    ]);
    let frame = map(&[("quantity", lit("1")), ("own_cost", lit("40 eur"))]);
    let fork = map(&[("quantity", lit("1")), ("own_cost", lit("120 eur"))]);
    let response = evaluate(
        &engine,
        "part",
        data(&[
            ("quantity", lit("1")),
            ("own_cost", lit("0 eur")),
            ("components", list(vec![frame, wheel, fork, drivetrain])),
        ]),
    );
    assert_eq!(value(&response, "unit_cost"), "361 eur");
}

#[test]
fn offer_bundle_is_47_77_eur() {
    let engine = load_ok(
        r#"
spec offer
data money: measure
  -> unit eur: 1.00
  -> decimals 2
uses items: offer list
data list_price: money
data bundle_discount: ratio
rule price:
  list_price
  unless items -> count > 0 then (items -> sum items.price) * (100% - bundle_discount)
"#,
    );
    let internet = map(&[("list_price", lit("40.00 eur"))]);
    let tv_base = map(&[("list_price", lit("10.00 eur"))]);
    let extra = map(&[("list_price", lit("8.00 eur"))]);
    let tv = map(&[
        ("bundle_discount", lit("10%")),
        ("items", list(vec![tv_base, extra])),
    ]);
    let response = evaluate(
        &engine,
        "offer",
        data(&[
            ("bundle_discount", lit("15%")),
            ("items", list(vec![internet, tv])),
        ]),
    );
    assert_eq!(value(&response, "price"), "47.77 eur");
}

#[test]
fn shift_chain_all_rested() {
    let engine = load_ok(
        r#"
spec shift
uses lemma units
uses next: shift list
data start: date
data end: date
rule enough_rest:
  true
  unless next -> count > 0 then (next -> first).start >= end + 11 hour
rule all_rested:
  enough_rest
  unless next -> count > 0 then enough_rest and (next -> all next.all_rested)
"#,
    );
    let later = map(&[("start", lit("2026-10-06")), ("end", lit("2026-10-06"))]);
    let response = evaluate(
        &engine,
        "shift",
        data(&[
            ("start", lit("2026-10-05")),
            ("end", lit("2026-10-05")),
            ("next", list(vec![later])),
        ]),
    );
    assert_eq!(value(&response, "all_rested"), "true");
}

#[test]
fn source_rows_on_self_list_are_planning_error() {
    let err = load_planning_err(
        r#"
spec part
uses components: part list
  -> with quantity: 1
data quantity: number
"#,
    );
    assert_contains(&err, "components");
}

#[test]
fn single_instance_self_uses_is_planning_error() {
    let err = load_err(
        r#"
spec part
uses child: part
"#,
    );
    assert_contains(&err, "itself");
}

#[test]
fn mutual_recursion_is_planning_error() {
    let err = load_planning_err(
        r#"
spec a
uses items: b list
spec b
uses items: a list
"#,
    );
    assert_contains(&err, "cycle");
}

#[test]
fn same_row_cycle_is_planning_error() {
    let err = load_planning_err(
        r#"
spec part
uses components: part list
rule unit_cost:
  unit_cost + 1
"#,
    );
    assert_contains(&err, "Circular dependency (rules: unit_cost)");
}

#[test]
fn recursive_rule_without_seed_is_planning_error() {
    let err = load_planning_err(
        r#"
spec part
uses components: part list
rule unit_cost:
  components -> sum components.unit_cost
"#,
    );
    assert_contains(&err, "non-recursive part");
}

#[test]
fn missing_data_uses_row_number_paths_post_order() {
    let engine = load_ok(part_spec());
    let leaf = map(&[("quantity", lit("1"))]);
    let parent = map(&[
        ("quantity", lit("1")),
        ("own_cost", lit("40 eur")),
        (
            "components",
            list(vec![leaf, map(&[("quantity", lit("1"))])]),
        ),
    ]);
    let response = evaluate(
        &engine,
        "part",
        data(&[
            ("quantity", lit("1")),
            ("own_cost", lit("0 eur")),
            ("components", list(vec![parent])),
        ]),
    );
    let paths = missing_data(&response, "unit_cost");
    let first = paths
        .iter()
        .position(|path| path == "components.1.components.1.own_cost")
        .expect("first missing leaf");
    let second = paths
        .iter()
        .position(|path| path == "components.1.components.2.own_cost")
        .expect("second missing leaf");
    assert!(first < second, "post-order missing_data: {paths:?}");
}

#[test]
fn deep_chain_within_limits_evaluates() {
    let limits = ResourceLimits {
        max_list_depth: 8,
        max_list_nodes: 16,
        ..ResourceLimits::default()
    };
    let engine = load_ok_with_limits(part_spec(), limits);
    let mut tree = map(&[("quantity", lit("1")), ("own_cost", lit("1 eur"))]);
    for _ in 0..3 {
        tree = map(&[
            ("quantity", lit("1")),
            ("own_cost", lit("1 eur")),
            ("components", list(vec![tree])),
        ]);
    }
    let lemma::EvaluateDataValue::Map(ref fields) = tree else {
        unreachable!("BUG: tree is a map");
    };
    let response = evaluate(
        &engine,
        "part",
        data(&[
            ("quantity", lit("1")),
            ("own_cost", lit("1 eur")),
            (
                "components",
                fields.get("components").cloned().expect("components"),
            ),
        ]),
    );
    assert_eq!(value(&response, "unit_cost"), "4 eur");
}

#[test]
fn depth_beyond_limit_is_request_error() {
    let limits = ResourceLimits {
        max_list_depth: 2,
        ..ResourceLimits::default()
    };
    let engine = load_ok_with_limits(part_spec(), limits);
    let too_deep = map(&[(
        "components",
        list(vec![map(&[(
            "components",
            list(vec![map(&[("own_cost", lit("1 eur"))])]),
        )])]),
    )]);
    let lemma::EvaluateDataValue::Map(ref fields) = too_deep else {
        unreachable!("BUG: row is a map");
    };
    let err = evaluate_err(
        &engine,
        "part",
        data(&[(
            "components",
            fields.get("components").cloned().expect("components"),
        )]),
    );
    assert_contains(&err.to_string(), "max_list_depth");
}

#[test]
fn self_list_inside_another_consumer_evaluates() {
    let engine = load_ok(
        r#"
spec part
data money: measure
  -> unit eur: 1.00
uses components: part list
data quantity: number
data own_cost: money
rule unit_cost:
  own_cost + (components -> sum(components.quantity * components.unit_cost))

spec bike
uses parts: part list
rule total:
  parts -> sum(parts.quantity * parts.unit_cost)
"#,
    );
    let spoke = map(&[("quantity", lit("32")), ("own_cost", lit("0.25 eur"))]);
    let nipple = map(&[("quantity", lit("1")), ("own_cost", lit("22 eur"))]);
    let hub = map(&[("quantity", lit("1")), ("own_cost", lit("18 eur"))]);
    let rim = map(&[("quantity", lit("1")), ("own_cost", lit("5 eur"))]);
    let wheel = map(&[
        ("quantity", lit("2")),
        ("own_cost", lit("0 eur")),
        ("components", list(vec![rim, hub, spoke, nipple])),
    ]);
    let chainring = map(&[("quantity", lit("1")), ("own_cost", lit("25 eur"))]);
    let cassette = map(&[("quantity", lit("1")), ("own_cost", lit("60 eur"))]);
    let crank = map(&[("quantity", lit("1")), ("own_cost", lit("10 eur"))]);
    let drivetrain = map(&[
        ("quantity", lit("1")),
        ("own_cost", lit("0 eur")),
        ("components", list(vec![crank, chainring, cassette])),
    ]);
    let frame = map(&[("quantity", lit("1")), ("own_cost", lit("40 eur"))]);
    let fork = map(&[("quantity", lit("1")), ("own_cost", lit("120 eur"))]);
    let response = evaluate(
        &engine,
        "bike",
        data(&[("parts", list(vec![frame, wheel, fork, drivetrain]))]),
    );
    assert_eq!(value(&response, "total"), "361 eur");
}

#[test]
fn self_list_inside_consumer_depth_counts_consumer_as_one() {
    let limits = ResourceLimits {
        max_list_depth: 2,
        ..ResourceLimits::default()
    };
    let engine = load_ok_with_limits(
        r#"
spec part
data money: measure
  -> unit eur: 1.00
uses components: part list
data own_cost: money
rule unit_cost:
  own_cost + (components -> sum components.unit_cost)

spec bike
uses parts: part list
rule total:
  parts -> sum parts.unit_cost
"#,
        limits,
    );
    // bike=1, parts row=2, nested component=3 → over max_list_depth 2
    let err = evaluate_err(
        &engine,
        "bike",
        data(&[(
            "parts",
            list(vec![map(&[
                ("own_cost", lit("1 eur")),
                ("components", list(vec![map(&[("own_cost", lit("1 eur"))])])),
            ])]),
        )]),
    );
    assert_contains(&err.to_string(), "max_list_depth");
}

#[test]
fn keyed_self_list_find_on_children() {
    let engine = load_ok(
        r#"
spec part
data money: measure
  -> unit eur: 1.00
data sku: text
uses components: part list
  -> key sku
data own_cost: money
rule unit_cost:
  own_cost + (components -> sum components.unit_cost)
rule child_cost:
  (components -> find "spoke").unit_cost
"#,
    );
    let response = evaluate(
        &engine,
        "part",
        data(&[
            ("sku", lit("bike")),
            ("own_cost", lit("0 eur")),
            (
                "components",
                list(vec![
                    map(&[("sku", lit("rim")), ("own_cost", lit("5 eur"))]),
                    map(&[("sku", lit("spoke")), ("own_cost", lit("2 eur"))]),
                ]),
            ),
        ]),
    );
    assert_eq!(value(&response, "child_cost"), "2 eur");
    assert_eq!(value(&response, "unit_cost"), "7 eur");
}

#[test]
fn keyed_self_list_nested_duplicate_key_vetoes() {
    let engine = load_ok(
        r#"
spec part
data money: measure
  -> unit eur: 1.00
data sku: text
uses components: part list
  -> key sku
data own_cost: money
rule unit_cost:
  own_cost + (components -> sum components.unit_cost)
"#,
    );
    let response = evaluate(
        &engine,
        "part",
        data(&[
            ("sku", lit("root")),
            ("own_cost", lit("0 eur")),
            (
                "components",
                list(vec![map(&[
                    ("sku", lit("wheel")),
                    ("own_cost", lit("0 eur")),
                    (
                        "components",
                        list(vec![
                            map(&[("sku", lit("spoke")), ("own_cost", lit("1 eur"))]),
                            map(&[("sku", lit("spoke")), ("own_cost", lit("2 eur"))]),
                        ]),
                    ),
                ])]),
            ),
        ]),
    );
    assert_contains(&veto_reason(&response, "unit_cost"), "spoke");
}

#[test]
fn recursion_via_first_member_read_evaluates() {
    let engine = load_ok(
        r#"
spec part
data money: measure
  -> unit eur: 1.00
uses components: part list
data own_cost: money
rule unit_cost:
  own_cost
  unless components -> count > 0 then own_cost + (components -> first).unit_cost
"#,
    );
    let leaf = map(&[("own_cost", lit("1 eur"))]);
    let mid = map(&[("own_cost", lit("1 eur")), ("components", list(vec![leaf]))]);
    let response = evaluate(
        &engine,
        "part",
        data(&[("own_cost", lit("1 eur")), ("components", list(vec![mid]))]),
    );
    assert_eq!(value(&response, "unit_cost"), "3 eur");
}

#[test]
fn data_reference_to_recursive_rule_types_and_evaluates() {
    let engine = load_ok(
        r#"
spec holder
data money: measure
  -> unit eur: 1.00
data amount: money

spec part
uses holder: holder
  -> with amount: unit_cost
uses components: part list
data own_cost: holder.money
rule unit_cost:
  own_cost + (components -> sum components.unit_cost)
rule bill:
  own_cost + (components -> sum components.holder.amount)
"#,
    );
    let response = evaluate(
        &engine,
        "part",
        data(&[
            ("own_cost", lit("1 eur")),
            ("components", list(vec![map(&[("own_cost", lit("2 eur"))])])),
        ]),
    );
    assert_eq!(value(&response, "bill"), "3 eur");
}

#[test]
fn reading_uses_instance_of_row_is_planning_error() {
    let err = load_planning_err(
        r#"
spec product_info
data price: measure
  -> unit eur: 1.00
spec line
uses item: product_info
data qty: number
spec order
uses lines: line list
  -> with qty: 1, item.price: 2 eur
rule first_item:
  (lines -> first).item
"#,
    );
    assert_contains(&err, "`item` is a `uses` instance");
}

#[test]
fn reading_uses_instance_of_row_in_argument_is_planning_error() {
    let err = load_planning_err(
        r#"
spec product_info
data price: measure
  -> unit eur: 1.00
spec line
uses item: product_info
data qty: number
spec order
uses lines: line list
  -> with qty: 1, item.price: 2 eur
rule item_total:
  lines -> sum lines.item
"#,
    );
    assert_contains(&err, "`item` is a `uses` instance on rows of `lines`");
}

#[test]
fn expression_with_named_instance_is_planning_error() {
    let err = load_planning_err(
        r#"
spec employee
data name: text
spec payroll
uses ada: employee
  -> with name: "Ada"
uses staff: employee list
rule extended:
  staff -> with ada
"#,
    );
    assert_contains(&err, "named instances");
}

#[test]
fn binding_replacing_instance_list_is_planning_error() {
    let err = load_planning_err(
        r#"
spec line
data qty: number
spec order
uses lines: line list
spec wrapper
uses o: order
  -> with lines: extras
uses extras: line list
  -> with qty: 1
"#,
    );
    assert_contains(&err, "binding `o.lines` replaces instance list `lines`");
}

#[test]
fn binding_through_instance_list_is_planning_error() {
    let err = load_planning_err(
        r#"
spec line
data qty: number
spec order
uses lines: line list
spec wrapper
uses o: order
  -> with lines.qty: 1
"#,
    );
    assert_contains(&err, "replaces instance list `lines`");
}

#[test]
fn pinned_self_list_uses_pinned_body() {
    // Shared undated money family so 2024 and 2025 part versions can add costs.
    let engine = load_files(&[(
        "part.lemma",
        r#"
spec units
data money: measure
  -> unit eur: 1.00

spec part 2024-01-01
uses units
uses components: part list
data own_cost: units.money
rule unit_cost:
  own_cost * 2

spec part 2025-01-01
uses units
uses components: part 2024-01-01 list
data own_cost: units.money
rule unit_cost:
  own_cost + (components -> sum components.unit_cost)
"#,
    )]);
    let response = evaluate_at(
        &engine,
        "part",
        &date(2025, 6, 1),
        data(&[
            ("own_cost", lit("1 eur")),
            ("components", list(vec![map(&[("own_cost", lit("5 eur"))])])),
        ]),
        None,
        false,
    );
    // child uses 2024 body: 5*2=10; parent 2025: 1+10=11
    assert_eq!(value(&response, "unit_cost"), "11 eur");
}

#[test]
fn mutual_recursion_through_rows_is_planning_error() {
    let err = load_planning_err(
        r#"
spec node
uses children: node list
rule a:
  1 + (children -> sum children.b)
rule b:
  1 + (children -> sum children.a)
"#,
    );
    assert_contains(&err, "Circular dependency");
}

#[test]
fn recursive_seed_type_mismatch_is_planning_error() {
    let err = load_planning_err(
        r#"
spec part
data money: measure
  -> unit eur: 1.00
uses components: part list
data own_cost: number
rule unit_cost:
  (components -> sum components.unit_cost) * 1 eur
  unless components -> count is 0 then own_cost
"#,
    );
    assert_contains(
        &err,
        "recursive rule `unit_cost` is own_cost from its non-recursive part, but money as a whole",
    );
}

#[test]
fn explanation_on_recursive_rule_uses_row_number_labels() {
    let engine = load_ok(part_spec());
    let response = evaluate_explain(
        &engine,
        "part",
        data(&[
            ("quantity", lit("1")),
            ("own_cost", lit("1 eur")),
            (
                "components",
                list(vec![map(&[
                    ("quantity", lit("1")),
                    ("own_cost", lit("2 eur")),
                    (
                        "components",
                        list(vec![map(&[
                            ("quantity", lit("1")),
                            ("own_cost", lit("3 eur")),
                        ])]),
                    ),
                ])]),
            ),
        ]),
    );
    let explanation = response
        .results
        .get("unit_cost")
        .expect("unit_cost")
        .explanation
        .as_ref()
        .expect("explanation");
    let text = lemma::format_explanation(explanation);
    assert_eq!(value(&response, "unit_cost"), "6 eur");
    assert_contains(&text, "quantity: 1");
    assert_contains(&text, "unit_cost: 5 eur");
    assert!(
        text.contains("\n      1.\n"),
        "multi-field row keeps bare number label:\n{text}"
    );
}

#[test]
fn show_on_self_list_spec_lists_components_once() {
    let engine = load_ok(part_spec());
    let show = engine.show(None, "part", Some(&now())).expect("show part");
    assert!(
        show.data.contains_key("components"),
        "show must list components once, got keys {:?}",
        show.data.keys().collect::<Vec<_>>()
    );
    let nested = show
        .data
        .keys()
        .filter(|key| key.starts_with("components."))
        .count();
    assert_eq!(
        nested,
        0,
        "show must not expand self-list rows into sibling keys, got {:?}",
        show.data.keys().collect::<Vec<_>>()
    );
}

#[test]
fn recursive_root_typed_during_seeding_gets_full_body_type() {
    let engine = load_ok(
        r#"
spec part
data money: measure
  -> unit eur: 1.00
uses components: part list
data quantity: number
data own_cost: money
rule unit_cost:
  own_cost + (components -> sum(components.quantity * components.unit_cost))
rule doubled: unit_cost * 2
"#,
    );
    let rim = map(&[("quantity", lit("2")), ("own_cost", lit("3 eur"))]);
    let response = evaluate(
        &engine,
        "part",
        data(&[
            ("quantity", lit("1")),
            ("own_cost", lit("0 eur")),
            ("components", list(vec![rim])),
        ]),
    );
    assert_eq!(value(&response, "unit_cost"), "6 eur");
    assert_eq!(value(&response, "doubled"), "12 eur");

    let show = engine.show(None, "part", Some(&now())).expect("show part");
    assert_eq!(
        show.rules
            .get("unit_cost")
            .expect("unit_cost")
            .lemma_type
            .name(),
        "own_cost"
    );
}

#[test]
fn row_dependent_cell_is_recomputed_per_row() {
    let engine = load_ok(
        r#"
spec part
uses components: part list
data quantity: number
rule weight: quantity * 10
rule total_weight: components -> sum(components.weight)
rule total_doubled: components -> sum(components.weight * 2)
"#,
    );
    let rows = list(vec![
        map(&[("quantity", lit("1"))]),
        map(&[("quantity", lit("2"))]),
        map(&[("quantity", lit("3"))]),
    ]);
    let response = evaluate(
        &engine,
        "part",
        data(&[("quantity", lit("0")), ("components", rows)]),
    );
    assert_eq!(value(&response, "total_weight"), "60");
    assert_eq!(value(&response, "total_doubled"), "120");
}

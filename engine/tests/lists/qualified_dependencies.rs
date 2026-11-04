//! Cross-instance reads must share one planned demand between planning and evaluation.
//! Instance-list row sources are dependency edges. Overlay wires resolve lazily.

use crate::lists::{
    assert_contains, data, evaluate, evaluate_at, evaluate_explain, list, lit, load_ok,
    load_planning_err, map, now, value,
};

fn knoop_spec() -> &'static str {
    r#"
spec knoop
data gewenst: number
data verlies: ratio
data kinderen: knoop list

rule benodigd:
  gewenst / (100% - verlies)
"#
}

#[test]
fn cross_uses_rule_overlay_and_sealed_list_evaluates() {
    let engine = load_ok(&format!(
        "{}
spec test
uses kind: knoop
  -> with gewenst: ouder.benodigd
  -> with verlies: 0%
uses ouder: knoop
  -> with gewenst: 100
  -> with verlies: 2%
  -> with kinderen: kind
rule r:
  kind.benodigd
",
        knoop_spec()
    ));
    let response = evaluate(&engine, "test", data(&[]));
    assert_eq!(value(&response, "r"), "102.0408163265306122448979591");
}

// A row field whose source is the consumer's data (not a rule).
#[test]
fn sealed_list_row_field_with_data_source_evaluates() {
    let engine = load_ok(&format!(
        "{}
spec test
uses kind: knoop
  -> with gewenst: ouder.gewenst
  -> with verlies: 0%
uses ouder: knoop
  -> with gewenst: 100
  -> with verlies: 2%
  -> with kinderen: kind
rule r:
  ouder.kinderen -> sum benodigd
",
        knoop_spec()
    ));
    let response = evaluate(&engine, "test", data(&[]));
    assert_eq!(value(&response, "r"), "100");
}

// `benodigd` branches on the row's own `kinderen -> count`.
fn knoop_count_spec() -> &'static str {
    r#"
spec knoop
data gewenst: number
data verlies: ratio
data kinderen: knoop list

rule benodigd:
  gewenst
  unless kinderen -> count > 0
  then gewenst / (100% - verlies)
"#
}

#[test]
fn sealed_list_row_rule_count_is_not_a_cycle() {
    let engine = load_ok(&format!(
        "{}
spec test
uses kind: knoop
  -> with gewenst: ouder.benodigd
  -> with verlies: 0%
uses ouder: knoop
  -> with gewenst: 100
  -> with verlies: 2%
  -> with kinderen: kind
rule r:
  ouder.kinderen -> sum benodigd
",
        knoop_count_spec()
    ));
    let response = evaluate(&engine, "test", data(&[]));
    assert_eq!(value(&response, "r"), "102.0408163265306122448979591");
}

#[test]
fn consumer_list_read_count_is_not_a_cycle() {
    let engine = load_ok(&format!(
        "{}
spec test
uses kind: knoop
  -> with gewenst: r
  -> with verlies: 0%
uses ouder: knoop
  -> with gewenst: 100
  -> with verlies: 2%
  -> with kinderen: kind
rule r:
  ouder.kinderen -> count
",
        knoop_spec()
    ));
    let response = evaluate(&engine, "test", data(&[]));
    assert_eq!(value(&response, "r"), "1");
}

#[test]
fn second_demand_binds_extra_wire_on_cached_import_context() {
    let engine = load_ok(&format!(
        "{}
spec test
uses kind: knoop
  -> with gewenst: ouder.benodigd
  -> with verlies: 0%
uses ouder: knoop
  -> with gewenst: 100
  -> with verlies: 2%
  -> with kinderen: kind
rule r:
  kind.benodigd
rule n:
  ouder.kinderen -> count
",
        knoop_spec()
    ));
    let response = evaluate(&engine, "test", data(&[]));
    assert_eq!(value(&response, "r"), "102.0408163265306122448979591");
    assert_eq!(value(&response, "n"), "1");
}

fn recipe_spec() -> &'static str {
    r#"
spec recipe
data share: number
  -> suggest 1
data unit_price: number
  -> suggest 0
data desired_out: number
data is_leaf: boolean
  -> suggest false
data children: recipe list

rule sum_share:
  children -> sum children.share

rule required_in:
  desired_out

rule total_in:
  required_in
  unless children -> count is 0
  then required_in

rule material_cost:
  required_in * unit_price
  unless is_leaf is false
  then total_in
    * (children -> sum(children.share * children.price_per_unit))
    / sum_share
  unless is_leaf is false and children -> count is 0
  then 0

rule price_per_unit:
  material_cost / desired_out
"#
}

#[test]
fn recipe_shape_evaluates() {
    let engine = load_ok(&format!(
        "{}
spec test
uses component_1: recipe
  -> with share: 1
  -> with unit_price: 8
  -> with desired_out: out_component_1
  -> with is_leaf: true
uses component_2: recipe
  -> with share: 1
  -> with unit_price: 4
  -> with desired_out: out_component_2
  -> with is_leaf: true
uses mix: recipe
  -> with is_leaf: false
  -> with desired_out: 100
  -> with children: component_1, component_2

rule out_component_1:
  mix.total_in * component_1.share / mix.sum_share

rule out_component_2:
  mix.total_in * component_2.share / mix.sum_share

rule ingredient_cost:
  mix.material_cost -> round 0.01
",
        recipe_spec()
    ));
    let response = evaluate(&engine, "test", data(&[]));
    assert_eq!(value(&response, "ingredient_cost"), "600");
}

#[test]
fn field_read_cycle_is_planning_error() {
    let err = load_planning_err(&format!(
        "{}
spec test
uses c1: recipe
  -> with share: 1
  -> with unit_price: 8
  -> with desired_out: r
  -> with is_leaf: true
rule r:
  c1.price_per_unit
",
        recipe_spec()
    ));
    assert_contains(&err, "Circular dependency");
}

#[test]
fn unread_field_not_evaluated() {
    let engine = load_ok(&format!(
        "{}
spec test
uses c1: recipe
  -> with share: 1
  -> with unit_price: 8
  -> with desired_out: unread_source
  -> with is_leaf: true
uses unused_item: recipe
  -> with share: 1
  -> with unit_price: 8
  -> with desired_out: 10
  -> with is_leaf: true
uses unused: recipe
  -> with is_leaf: false
  -> with desired_out: 10
  -> with children: unused_item
uses mix: recipe
  -> with is_leaf: false
  -> with desired_out: 0
  -> with children: c1

rule unread_source:
  unused.sum_share

rule sum_share:
  mix.sum_share

rule total:
  mix.total_in
",
        recipe_spec()
    ));
    let requested = vec!["sum_share".to_string(), "total".to_string()];
    let response = evaluate_at(&engine, "test", &now(), data(&[]), Some(&requested), false);
    assert_eq!(value(&response, "sum_share"), "1");
    assert_eq!(value(&response, "total"), "0");
}

#[test]
fn unreached_deferred_source_is_not_evaluated() {
    let engine = load_ok(
        r#"
spec inner
data v: number
uses item
data items: item list
rule s:
  items -> sum items.target

spec item
data target: number

spec test
uses a1: item
  -> with target: shared_result
uses a: inner
  -> with v: 5
  -> with items: a1
uses b1: item
  -> with target: b_source
uses b: inner
  -> with items: b1

rule shared_result:
  a.v

rule b_source:
  b.v

rule r:
  a.s
"#,
    );
    let requested = vec!["r".to_string()];
    let response = evaluate_at(&engine, "test", &now(), data(&[]), Some(&requested), false);
    assert_eq!(value(&response, "r"), "5");
}

#[test]
fn unbound_list_wire_is_missing_data_not_panic() {
    let engine = load_ok(&format!(
        "{}
spec test
uses c1: recipe
  -> with share: 1
  -> with unit_price: 8
  -> with desired_out: c1_out
  -> with is_leaf: true
uses mix: recipe
  -> with is_leaf: false
  -> with desired_out: 100
  -> with children: c1
uses mid: recipe
  -> with is_leaf: false
  -> with desired_out: 100

rule c1_out:
  mix.sum_share * 100

rule cost:
  mid.material_cost

rule out:
  mid.required_in
",
        recipe_spec()
    ));
    let requested = vec!["cost".to_string(), "out".to_string()];
    let response = evaluate_at(&engine, "test", &now(), data(&[]), Some(&requested), false);
    assert_eq!(value(&response, "cost"), "0");
    assert_eq!(value(&response, "out"), "100");
}

#[test]
fn deferred_row_intermediate_values() {
    let engine = load_ok(&format!(
        "{}
spec test
uses component_1: recipe
  -> with share: 1
  -> with unit_price: 8
  -> with desired_out: out_component_1
  -> with is_leaf: true
uses component_2: recipe
  -> with share: 1
  -> with unit_price: 4
  -> with desired_out: out_component_2
  -> with is_leaf: true
uses mix: recipe
  -> with is_leaf: false
  -> with desired_out: 100
  -> with children: component_1, component_2

rule out_component_1:
  mix.total_in * component_1.share / mix.sum_share

rule out_component_2:
  mix.total_in * component_2.share / mix.sum_share

rule cost_1:
  component_1.material_cost

rule cost_2:
  component_2.material_cost

rule ingredient_cost:
  mix.material_cost -> round 0.01
",
        recipe_spec()
    ));
    let response = evaluate(&engine, "test", data(&[]));
    assert_eq!(value(&response, "out_component_1"), "50");
    assert_eq!(value(&response, "out_component_2"), "50");
    assert_eq!(value(&response, "cost_1"), "400");
    assert_eq!(value(&response, "cost_2"), "200");
    assert_eq!(value(&response, "ingredient_cost"), "600");
}

#[test]
fn deferred_yield_is_not_double_applied() {
    let engine = load_ok(
        r#"
spec mid
data loss: ratio
data desired_out: number
data unit_price: number
data is_leaf: boolean
data children: mid list

rule yield_rate:
  100% - loss

rule required_in:
  desired_out / yield_rate

rule material_cost:
  required_in * unit_price
  unless is_leaf is false
  then required_in * (children -> sum children.price_per_unit)

rule price_per_unit:
  material_cost / desired_out

spec test
uses leaf_a: mid
  -> with is_leaf: true
  -> with loss: 2%
  -> with unit_price: 8
  -> with desired_out: mix.required_in
uses mix: mid
  -> with is_leaf: false
  -> with loss: 0%
  -> with unit_price: 0
  -> with desired_out: 250
  -> with children: leaf_a

rule required:
  leaf_a.required_in

rule cost:
  leaf_a.material_cost -> round 0.01
"#,
    );
    let response = evaluate(&engine, "test", data(&[]));
    assert_eq!(
        value(&response, "required"),
        "255.1020408163265306122448979"
    );
    assert_eq!(value(&response, "cost"), "2040.82");
}

#[test]
fn rule_returned_rows_field_read() {
    let engine = load_ok(
        r#"
spec item
data naam: text
data prijs: number
spec order
uses item
data regels: item list
  -> with naam: "a", prijs: 3
  -> with naam: "b", prijs: 5
rule eerste:
  regels -> first
rule prijs_eerste:
  eerste.prijs
rule som:
  regels -> sum regels.prijs
"#,
    );
    let response = evaluate(&engine, "order", data(&[]));
    assert_eq!(value(&response, "prijs_eerste"), "3");
    assert_eq!(value(&response, "som"), "8");
}

fn deferred_import_rule_source_flat(consumer: &str) -> String {
    format!(
        r#"
spec node
data amount: number
data children: node list
rule mid:
  amount
rule out:
  mid

spec test
uses src: node
  -> with amount: 10
uses leaf: node
  -> with amount: src.out
uses root: node
  -> with amount: 0
  -> with children: leaf
rule r:
  {consumer}
"#
    )
}

fn deferred_import_rule_source_nested(consumer: &str) -> String {
    format!(
        r#"
spec node
data amount: number
data children: node list
rule mid:
  amount
rule out:
  mid

spec test
uses src: node
  -> with amount: 10
uses leaf: node
  -> with amount: src.out
uses mid_node: node
  -> with amount: 0
  -> with children: leaf
uses root: node
  -> with amount: 0
  -> with children: mid_node
rule r:
  {consumer}
"#
    )
}

// Nested list: leaf.amount is bound to import-qualified src.out, read through
// out -> mid -> amount.
#[test]
fn deferred_import_rule_source_via_transitive_row_rule() {
    let engine = load_ok(&deferred_import_rule_source_nested(
        "root.children -> sum (children -> sum children.out)",
    ));
    let response = evaluate(&engine, "test", data(&[]));
    assert_eq!(value(&response, "r"), "10");
}

// Flat list, direct field read.
#[test]
fn deferred_import_rule_source_direct_field_sum() {
    let engine = load_ok(&deferred_import_rule_source_flat(
        "root.children -> sum amount",
    ));
    let response = evaluate(&engine, "test", data(&[]));
    assert_eq!(value(&response, "r"), "10");
}

#[test]
fn deferred_import_rule_source_nested_direct_field() {
    let engine = load_ok(&deferred_import_rule_source_nested(
        "root.children -> sum (children -> sum children.amount)",
    ));
    let response = evaluate(&engine, "test", data(&[]));
    assert_eq!(value(&response, "r"), "10");
}

// Rows of both nested lists have `amount`: the inner read names its list.
#[test]
fn nested_row_field_on_both_scopes_is_planning_error() {
    let err = load_planning_err(&deferred_import_rule_source_nested(
        "root.children -> sum (children -> sum amount)",
    ));
    assert_contains(&err, "write `children.amount`");
}

/// Field or rule `field` of the first row `rule` returns. Rows written in the spec
/// are planned instances and publish as the `spec` arm.
fn row_field(response: &lemma::Response, rule: &str, field: &str) -> Option<String> {
    let instance = response
        .results
        .get(rule)
        .and_then(|result| result.result.as_ref())
        .and_then(|value| value.list.as_ref())
        .and_then(|rows| rows.first())
        .and_then(|row| row.spec.as_ref())
        .unwrap_or_else(|| panic!("rule '{rule}' has no first row"));
    instance
        .data
        .get(field)
        .and_then(|slot| slot.result.as_ref())
        .and_then(|value| value.result.clone())
        .or_else(|| {
            instance
                .rules
                .get(field)
                .and_then(|slot| slot.result.as_ref())
                .and_then(|value| value.result.clone())
        })
}

// A rule returning sealed rows: each returned row publishes every field and rule
// through its `spec` arm.
#[test]
fn returned_sealed_rows_fill_row_table() {
    let engine = load_ok(&deferred_import_rule_source_flat("root.children"));
    let response = evaluate(&engine, "test", data(&[]));
    assert_eq!(row_field(&response, "r", "amount").as_deref(), Some("10"));
    assert_eq!(row_field(&response, "r", "out").as_deref(), Some("10"));
}

#[test]
fn returned_sealed_rows_fill_row_table_explain() {
    let engine = load_ok(&deferred_import_rule_source_flat("root.children"));
    let response = evaluate_explain(&engine, "test", data(&[]));
    assert_eq!(row_field(&response, "r", "amount").as_deref(), Some("10"));
    assert_eq!(row_field(&response, "r", "out").as_deref(), Some("10"));
}

#[test]
fn deferred_import_rule_source_via_transitive_row_rule_explain() {
    let engine = load_ok(&deferred_import_rule_source_nested(
        "root.children -> sum (children -> sum children.out)",
    ));
    let response = evaluate_explain(&engine, "test", data(&[]));
    assert_eq!(value(&response, "r"), "10");
}

// Rows of a list sealed inside `inner` (owner `inner`) are returned to `test`.
#[test]
fn sealed_rows_escaping_their_owner_scope() {
    let engine = load_ok(
        r#"
spec item
data target: number
rule doubled:
  target * 2

spec box
uses item
data items: item list

spec inner
uses i1: item
  -> with target: base
uses holder: box
  -> with items: i1
rule base:
  5
rule picked:
  holder.items

spec test
uses inner
rule r:
  inner.picked -> sum doubled
"#,
    );
    let response = evaluate(&engine, "test", data(&[]));
    assert_eq!(value(&response, "r"), "10");
}

// A nested template that fails planning is an Error on the consumer, not a panic.
#[test]
fn invalid_nested_template_with_list_binding_is_planning_error() {
    let err = load_planning_err(
        r#"
spec node
data amount: number
data children: node list
rule total:
  children -> sum (children -> sum amount)

spec test
uses leaf: node
  -> with amount: 10
uses root: node
  -> with amount: 0
  -> with children: leaf
rule r:
  root.total
"#,
    );
    assert_contains(&err, "write `children.children`");
}

// Import demand into a template rule that reads a nested sealed list.
#[test]
fn import_demand_template_rule_over_nested_sealed_list() {
    let engine = load_ok(
        r#"
spec node
data amount: number
data children: node list
rule mid:
  amount
rule out:
  mid
rule total:
  children -> sum (children.children -> sum children.children.out)

spec test
uses src: node
  -> with amount: 10
uses leaf: node
  -> with amount: src.out
uses mid_node: node
  -> with amount: 0
  -> with children: leaf
uses root: node
  -> with amount: 0
  -> with children: mid_node
rule r:
  root.total
"#,
    );
    let response = evaluate(&engine, "test", data(&[]));
    assert_eq!(value(&response, "r"), "10");
}

// The row spec has its own `base`; the sealed source is the consumer's `base`.
#[test]
fn sealed_source_owner_is_not_a_same_named_row_rule() {
    let engine = load_ok(
        r#"
spec item
data target: number
data extra: number
rule base:
  99
rule doubled:
  target * 2 + extra

spec box
uses item
data items: item list

spec test
uses i1: item
  -> with target: base
  -> with extra: base
uses holder: box
  -> with items: i1
rule base:
  5
rule r:
  holder.items -> sum doubled
"#,
    );
    let response = evaluate(&engine, "test", data(&[]));
    assert_eq!(value(&response, "r"), "15");
}

#[test]
fn nested_sealed_source_cycle_is_planning_error() {
    let err = load_planning_err(
        r#"
spec node
data amount: number
data children: node list

spec test
uses leaf: node
  -> with amount: cyc
uses mid_node: node
  -> with amount: 0
  -> with children: leaf
uses root: node
  -> with amount: 0
  -> with children: mid_node
rule cyc:
  root.children -> sum (children -> sum children.amount)
"#,
    );
    assert_contains(&err, "Circular dependency");
}

// A row rule read through the row spec's own import (`items.sub.doubled`), whose
// import data is a sealed rule-backed row field.
fn sealed_field_under_row_import(consumer: &str) -> String {
    format!(
        r#"
spec inner
data x: number
rule doubled:
  x * 2

spec item
uses sub: inner
rule total:
  sub.doubled

spec box
uses item
data items: item list

spec src
rule seven:
  7

spec test
uses src
uses it: item
  -> with sub.x: src.seven
uses holder: box
  -> with items: it
rule r:
  {consumer}
"#
    )
}

#[test]
fn row_import_rule_on_sealed_field() {
    let engine = load_ok(&sealed_field_under_row_import(
        "holder.items -> sum holder.items.sub.doubled",
    ));
    let response = evaluate(&engine, "test", data(&[]));
    assert_eq!(value(&response, "r"), "14");
}

#[test]
fn row_import_rule_on_sealed_field_explain() {
    let engine = load_ok(&sealed_field_under_row_import(
        "holder.items -> sum holder.items.sub.doubled",
    ));
    let response = evaluate_explain(&engine, "test", data(&[]));
    assert_eq!(value(&response, "r"), "14");
}

#[test]
fn row_import_rule_on_literal_field_explain() {
    let engine = load_ok(
        &sealed_field_under_row_import("holder.items -> sum holder.items.sub.doubled")
            .replace("-> with sub.x: src.seven", "-> with sub.x: 7"),
    );
    let response = evaluate_explain(&engine, "test", data(&[]));
    assert_eq!(value(&response, "r"), "14");
}

#[test]
fn row_rule_through_row_import_on_sealed_field_explain() {
    let engine = load_ok(&sealed_field_under_row_import(
        "holder.items -> sum holder.items.total",
    ));
    let response = evaluate_explain(&engine, "test", data(&[]));
    assert_eq!(value(&response, "r"), "14");
}

// A row field cannot source a `uses` instance, so no sealed row field ends at an
// import.
fn row_field_sourcing_an_instance(source: &str) -> String {
    format!(
        r#"
spec leaf
data y: number

spec inner
data x: number
uses deep: leaf

spec item
uses sub: inner
data amount: number

spec box
uses item
data items: item list

spec test
uses other: inner
  -> with x: 3
uses it: item
  -> with amount: 1
  -> with {source}
uses holder: box
  -> with items: it
rule r:
  holder.items -> sum holder.items.amount
"#
    )
}

#[test]
fn row_import_overlaid_with_consumer_instance_is_planning_error() {
    let error = load_planning_err(&row_field_sourcing_an_instance("sub: other"));
    assert_contains(&error, "cannot supply spec instance 'other' as a value");
}

#[test]
fn row_field_sourcing_a_nested_instance_is_planning_error() {
    let error = load_planning_err(&row_field_sourcing_an_instance("amount: other.deep"));
    assert_contains(&error, "cannot supply spec instance 'deep' as a value");
}

// A list of the owner's own spec cannot carry sealed rows, so a rule read on
// self-list rows never needs an owner-scope source.
#[test]
fn self_list_with_sealed_rows_is_planning_error() {
    let error = load_planning_err(
        r#"
spec node
data amount: number
data children: node list
  -> with amount: base
rule base:
  5
rule doubled:
  amount * 2
rule total:
  children -> sum doubled
"#,
    );
    assert_contains(
        &error,
        "`children` is a list of `node` inside `node` itself: it cannot have source rows",
    );
}

#[test]
fn self_list_row_rule_reads_run_data() {
    let engine = load_ok(
        r#"
spec node
data amount: number
data children: node list
rule base:
  5
rule doubled:
  amount * 2
rule total:
  children -> sum children.doubled
"#,
    );
    let response = evaluate(
        &engine,
        "node",
        data(&[
            ("amount", lit("1")),
            (
                "children",
                list(vec![
                    map(&[("amount", lit("4"))]),
                    map(&[("amount", lit("6"))]),
                ]),
            ),
        ]),
    );
    assert_eq!(value(&response, "total"), "20");
}

// A self-list row's rule yields rows the child row builds and seals to its own rule.
fn self_list_rule_yielding_sealed_rows() -> &'static str {
    r#"
spec item
data target: number

spec box
uses item
data items: item list

spec node
data amount: number
data children: node list
uses i1: item
  -> with target: base
uses holder: box
  -> with items: i1
rule base:
  amount * 10
rule picked:
  holder.items
rule amounts:
  children -> sum (children.picked -> sum children.picked.target)
"#
}

fn self_list_children() -> indexmap::IndexMap<String, lemma::EvaluateDataValue> {
    data(&[
        ("amount", lit("0")),
        (
            "children",
            list(vec![
                map(&[("amount", lit("1"))]),
                map(&[("amount", lit("2"))]),
            ]),
        ),
    ])
}

#[test]
fn self_list_row_rule_yields_rows_sealed_in_the_row() {
    let engine = load_ok(self_list_rule_yielding_sealed_rows());
    let response = evaluate(&engine, "node", self_list_children());
    assert_eq!(value(&response, "amounts"), "30");
}

#[test]
fn self_list_row_rule_yields_rows_sealed_in_the_row_explain() {
    let engine = load_ok(self_list_rule_yielding_sealed_rows());
    let response = evaluate_explain(&engine, "node", self_list_children());
    assert_eq!(value(&response, "amounts"), "30");
}

// A rule takes the rows a self-list row's rule yields; it walks before that rule.
fn self_list_member_rows() -> &'static str {
    r#"
spec item
data target: number

spec box
uses item
data items: item list

spec node
data amount: number
data children: node list
uses i1: item
  -> with target: base
uses holder: box
  -> with items: i1
rule base:
  amount * 10
rule picked:
  holder.items
rule a_rows:
  (children -> first).picked
rule a_total:
  a_rows -> sum a_rows.target
"#
}

#[test]
fn self_list_member_rows_from_row_rule() {
    let engine = load_ok(self_list_member_rows());
    let response = evaluate(&engine, "node", self_list_children());
    assert_eq!(value(&response, "a_total"), "10");
}

#[test]
fn self_list_member_rows_from_row_rule_explain() {
    let engine = load_ok(self_list_member_rows());
    let response = evaluate_explain(&engine, "node", self_list_children());
    assert_eq!(value(&response, "a_total"), "10");
}

// An appended row's field holds rows sealed to a rule of the owner.
fn appended_row_with_sealed_rows() -> &'static str {
    r#"
spec item
data target: number

spec box
uses item
data items: item list

spec owner
uses box
data crates: box list
uses i1: item
  -> with target: base
uses holder: box
  -> with items: i1
rule base:
  7
rule a_boxes:
  crates -> with items: holder.items
rule a_total:
  a_boxes -> sum (a_boxes.items -> sum a_boxes.items.target)
"#
}

fn no_crates() -> indexmap::IndexMap<String, lemma::EvaluateDataValue> {
    data(&[("crates", list(Vec::new()))])
}

#[test]
fn appended_row_field_holds_sealed_rows() {
    let engine = load_ok(appended_row_with_sealed_rows());
    let response = evaluate(&engine, "owner", no_crates());
    assert_eq!(value(&response, "a_total"), "7");
}

#[test]
fn appended_row_field_holds_sealed_rows_explain() {
    let engine = load_ok(appended_row_with_sealed_rows());
    let response = evaluate_explain(&engine, "owner", no_crates());
    assert_eq!(value(&response, "a_total"), "7");
}

// A rule of the row spec reading a field the appended row binds to rows of an owner
// instance evaluates the sealed rows through the row's bindings: their deferred
// references resolve in the frame that wrote them, as for scalar bindings
// (`appended_row_rule_over_written_numbers_runs_in_the_row`).
fn appended_row_rule_over_owner_instances(reader: &str) -> String {
    format!(
        r#"
spec item
data target: number

spec box
uses item
data items: item list
rule total:
  items -> sum target

spec owner
uses box
data crates: box list
uses i1: item
  -> with target: base
uses holder: box
  -> with items: i1
rule base:
  7
rule a_boxes:
  crates -> with items: holder.items
rule a_total:
  {reader}
"#
    )
}

#[test]
fn appended_row_rule_reading_owner_instances_evaluates() {
    let engine = load_ok(&appended_row_rule_over_owner_instances(
        "a_boxes -> sum total",
    ));
    let response = evaluate(&engine, "owner", no_crates());
    assert_eq!(value(&response, "a_total"), "7");
}

#[test]
fn appended_row_member_rule_reading_owner_instances_evaluates() {
    let engine = load_ok(&appended_row_rule_over_owner_instances(
        "(a_boxes -> first).total",
    ));
    let response = evaluate(&engine, "owner", no_crates());
    assert_eq!(value(&response, "a_total"), "7");
}

#[test]
fn appended_row_rule_over_written_numbers_runs_in_the_row() {
    let engine = load_ok(
        r#"
spec item
data target: number
rule doubled:
  target * 2

spec owner
uses item
data picks: item list
rule base:
  7
rule a_picks:
  picks -> with target: base
rule a_doubled:
  a_picks -> sum doubled
rule first_doubled:
  (a_picks -> first).doubled
"#,
    );
    let response = evaluate(&engine, "owner", data(&[("picks", list(Vec::new()))]));
    assert_eq!(value(&response, "a_doubled"), "14");
    assert_eq!(value(&response, "first_doubled"), "14");
}

// `or` keeps the row on its left operand, so the sealed `target` is never read.
fn where_or_skips_sealed_field() -> &'static str {
    r#"
spec item
data size: number
data target: number

spec box
uses item
data items: item list

spec owner
uses i1: item
  -> with size: 5, target: base
uses holder: box
  -> with items: i1
rule base:
  7
rule picked:
  holder.items -> where(holder.items.size > 1 or holder.items.target > 3)
rule matched:
  picked -> count
"#
}

#[test]
fn where_or_skipping_sealed_field() {
    let engine = load_ok(where_or_skips_sealed_field());
    let response = evaluate(&engine, "owner", data(&[]));
    assert_eq!(value(&response, "matched"), "1");
}

#[test]
fn where_or_skipping_sealed_field_explain() {
    let engine = load_ok(where_or_skips_sealed_field());
    let response = evaluate_explain(&engine, "owner", data(&[]));
    assert_eq!(value(&response, "matched"), "1");
    let explanation = response
        .results
        .get("picked")
        .expect("picked")
        .explanation
        .as_ref()
        .expect("explanation");
    let text = lemma::format_explanation(explanation);
    assert!(text.ends_with("1. size: 5, target"), "{text}");
}

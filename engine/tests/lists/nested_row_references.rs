//! References bound inside nested stored rows (`StoredBinding::Rows` subtrees)
//! resolve lazily: only a read landing on the field forces the referenced rule,
//! in the plan that owns the list. A consumer rule reading the outer list must
//! not drag in rules the read never reaches, and true cycles through nested
//! references stay planning errors.

use crate::lists::{
    assert_contains, data, evaluate, evaluate_at, evaluate_explain, load_ok, load_planning_err,
    now, value,
};

fn recipe_spec() -> &'static str {
    r#"
spec recipe
data name: text
data is_raw: boolean
data unit_price: number
data proportion: number
data loss: ratio
data target_weight: number
data components: recipe list

rule remaining:
  100% - loss

rule required:
  target_weight / remaining

rule proportion_sum:
  components -> sum components.proportion

rule total:
  required
  unless components -> count > 1
  then required * 1.02

rule cost:
  required * unit_price
  unless is_raw is false
  then total * (components -> sum(components.proportion * components.price)) / proportion_sum

rule price:
  cost / target_weight
"#
}

/// `leaf` is nested inside `mid`'s components and references `mid.required`;
/// `mid` references the consumer rule `share`, which reads `mix.total` and
/// `mix.proportion_sum`. Neither read reaches the nested references.
fn blend_order_source(mid_target_weight: &str) -> String {
    format!(
        "{}
spec blend_order
uses leaf: recipe
  -> with name: \"Leaf\"
  -> with is_raw: true
  -> with unit_price: 5
  -> with proportion: 1
  -> with loss: 0%
  -> with target_weight: mid.required

uses mid: recipe
  -> with name: \"Mid\"
  -> with is_raw: false
  -> with unit_price: 0
  -> with proportion: 3
  -> with loss: 0%
  -> with target_weight: {mid_target_weight}
  -> with components: leaf

uses other: recipe
  -> with name: \"Other\"
  -> with is_raw: true
  -> with unit_price: 9
  -> with proportion: 1
  -> with loss: 0%
  -> with target_weight: 25

uses mix: recipe
  -> with name: \"Mix\"
  -> with is_raw: false
  -> with unit_price: 0
  -> with proportion: 1
  -> with loss: 0%
  -> with target_weight: 100
  -> with components: mid, other

rule share:
  mix.total * mid.proportion / mix.proportion_sum

rule result_cost:
  mix.cost

rule result_proportion_sum:
  mix.proportion_sum

rule result_count:
  mix.components -> count
",
        recipe_spec()
    )
}

#[test]
fn nested_reference_to_outer_instance_rule_is_not_a_cycle() {
    let engine = load_ok(&blend_order_source("share"));
    let response = evaluate(&engine, "blend_order", data(&[]));
    assert_eq!(value(&response, "share"), "76.5");
    assert_eq!(value(&response, "result_cost"), "612");
    assert_eq!(value(&response, "result_proportion_sum"), "4");
    assert_eq!(value(&response, "result_count"), "2");
}

#[test]
fn count_and_sum_over_nested_rows_touch_no_nested_references() {
    let engine = load_ok(&blend_order_source("share"));
    let rules = vec![
        "result_proportion_sum".to_string(),
        "result_count".to_string(),
    ];
    let response = evaluate_at(
        &engine,
        "blend_order",
        &now(),
        data(&[]),
        Some(&rules),
        true,
    );
    assert_eq!(value(&response, "result_proportion_sum"), "4");
    assert_eq!(value(&response, "result_count"), "2");
    for rule in &rules {
        let explanation = response.results[rule]
            .explanation
            .as_ref()
            .unwrap_or_else(|| panic!("rule '{rule}' has no explanation"));
        let json = serde_json::to_string(explanation).expect("explanation json");
        assert!(
            !json.contains("required"),
            "evaluating '{rule}' forced the nested reference target `required`: {json}"
        );
    }
}

#[test]
fn nested_reference_cycle_is_planning_error() {
    let err = load_planning_err(&blend_order_source("mix.cost"));
    assert_contains(&err, "Circular dependency");
}

/// The same true cycle as `nested_reference_cycle_is_planning_error`, but the
/// consumer reads through a member expression on a stored row:
/// `share` reads `(mix.components -> first).total` = `mid.total`, and `mid`'s
/// `target_weight` is bound to `share`. Planning must reject it; a miss would
/// loop evaluation.
fn member_read_cycle_source() -> String {
    format!(
        "{}
spec blend_order
uses leaf: recipe
  -> with name: \"Leaf\"
  -> with is_raw: true
  -> with unit_price: 5
  -> with proportion: 1
  -> with loss: 0%
  -> with target_weight: 10

uses mid: recipe
  -> with name: \"Mid\"
  -> with is_raw: false
  -> with unit_price: 0
  -> with proportion: 3
  -> with loss: 0%
  -> with target_weight: share
  -> with components: leaf

uses mix: recipe
  -> with name: \"Mix\"
  -> with is_raw: false
  -> with unit_price: 0
  -> with proportion: 1
  -> with loss: 0%
  -> with target_weight: 100
  -> with components: mid

rule share:
  (mix.components -> first).total
",
        recipe_spec()
    )
}

#[test]
fn nested_reference_cycle_through_member_read_is_planning_error() {
    let err = load_planning_err(&member_read_cycle_source());
    assert_contains(&err, "Circular dependency");
}

/// A true cycle through rows of one spec (`recipe`) nested in stored rows of
/// another (`wrapper`): `back` reads the recipe row's `total`, whose
/// `target_weight` is bound to `back`. Planning must reject it.
fn cross_spec_cycle_source() -> String {
    format!(
        "{}
spec wrapper
data label: text
uses recipe
data items: recipe list

spec holder
uses wrapper
data items: wrapper list

spec cross_cycle
uses r: recipe
  -> with name: \"R\"
  -> with is_raw: true
  -> with unit_price: 1
  -> with proportion: 1
  -> with loss: 0%
  -> with target_weight: back

uses w: wrapper
  -> with label: \"W\"
  -> with items: r

uses h: holder
  -> with items: w

rule back:
  ((h.items -> first).items -> first).total
",
        recipe_spec()
    )
}

#[test]
fn nested_reference_cycle_across_specs_is_planning_error() {
    let err = load_planning_err(&cross_spec_cycle_source());
    assert_contains(&err, "Circular dependency");
}

/// A true cycle where the nested read happens inside the outer row spec's own
/// rule: `back` reads the wrapper row's `first_total`, which reads the nested
/// recipe row's `total`, whose `target_weight` is bound to `back`.
fn wrapper_rule_cycle_source() -> String {
    format!(
        "{}
spec wrapper_total
data label: text
uses recipe
data items: recipe list

rule first_total:
  (items -> first).total

spec total_holder
uses wrapper_total
data items: wrapper_total list

spec wrapper_cycle
uses r: recipe
  -> with name: \"R\"
  -> with is_raw: true
  -> with unit_price: 1
  -> with proportion: 1
  -> with loss: 0%
  -> with target_weight: back

uses w: wrapper_total
  -> with label: \"W\"
  -> with items: r

uses h: total_holder
  -> with items: w

rule back:
  (h.items -> first).first_total
",
        recipe_spec()
    )
}

#[test]
fn nested_reference_cycle_through_row_spec_rule_is_planning_error() {
    let err = load_planning_err(&wrapper_rule_cycle_source());
    assert_contains(&err, "Circular dependency");
}

/// A true cycle where the outer row spec's rule reads a rule of nested rows of
/// another spec through a list command: `priced_total` sums the nested recipe
/// rows' `price`, which forces their deferred `target_weight`, bound to `back`.
fn wrapper_sum_cycle_source() -> String {
    format!(
        "{}
spec wrapper_sum
data label: text
uses recipe
data items: recipe list

rule priced_total:
  items -> sum(items.price)

spec sum_holder
uses wrapper_sum
data items: wrapper_sum list

spec sum_cycle
uses r: recipe
  -> with name: \"R\"
  -> with is_raw: true
  -> with unit_price: 1
  -> with proportion: 1
  -> with loss: 0%
  -> with target_weight: back

uses w: wrapper_sum
  -> with label: \"W\"
  -> with items: r

uses h: sum_holder
  -> with items: w

rule back:
  (h.items -> first).priced_total
",
        recipe_spec()
    )
}

#[test]
fn nested_reference_cycle_through_cross_spec_row_field_is_planning_error() {
    let err = load_planning_err(&wrapper_sum_cycle_source());
    assert_contains(&err, "Circular dependency");
}

/// The same true cycle as `nested_reference_cycle_is_planning_error`, but the
/// consumer's read iterates a filtered list: the cycle edge must follow the
/// filter's operand rows.
#[test]
fn nested_reference_cycle_through_filtered_list_is_planning_error() {
    let err = load_planning_err(&format!(
        "{}
spec blend_order
uses leaf: recipe
  -> with name: \"Leaf\"
  -> with is_raw: true
  -> with unit_price: 5
  -> with proportion: 1
  -> with loss: 0%
  -> with target_weight: 10

uses mid: recipe
  -> with name: \"Mid\"
  -> with is_raw: false
  -> with unit_price: 0
  -> with proportion: 3
  -> with loss: 0%
  -> with target_weight: share
  -> with components: leaf

uses mix: recipe
  -> with name: \"Mix\"
  -> with is_raw: false
  -> with unit_price: 0
  -> with proportion: 1
  -> with loss: 0%
  -> with target_weight: 100
  -> with components: mid

rule share:
  mix.components -> where is_raw is false -> sum required
",
        recipe_spec()
    ));
    assert_contains(&err, "Circular dependency");
}

#[test]
fn three_level_nesting_defers_at_every_depth() {
    let engine = load_ok(&format!(
        "{}
spec deep_blend
uses inner_leaf: recipe
  -> with name: \"Leaf\"
  -> with is_raw: true
  -> with unit_price: 2
  -> with proportion: 1
  -> with loss: 0%
  -> with target_weight: peak.required

uses middle: recipe
  -> with name: \"Middle\"
  -> with is_raw: false
  -> with unit_price: 0
  -> with proportion: 1
  -> with loss: 0%
  -> with target_weight: peak.required
  -> with components: inner_leaf

uses peak: recipe
  -> with name: \"Peak\"
  -> with is_raw: false
  -> with unit_price: 0
  -> with proportion: 1
  -> with loss: 0%
  -> with target_weight: 60
  -> with components: middle

rule result:
  peak.cost
",
        recipe_spec()
    ));
    let response = evaluate(&engine, "deep_blend", data(&[]));
    assert_eq!(value(&response, "result"), "120");
}

/// Both consumers import `pantry`'s plan: one template, so the packed leaf row its
/// `uses` rows hold is one allocation, but its deferred `needed` reference resolves
/// per consumer.
#[test]
fn two_consumers_share_one_store_and_resolve_deferred_independently() {
    let engine = load_ok(&format!(
        "{}
spec pantry
data base_weight: number

uses inner_leaf: recipe
  -> with name: \"Shared leaf\"
  -> with is_raw: true
  -> with unit_price: 4
  -> with proportion: 1
  -> with loss: 0%
  -> with target_weight: needed

uses inner_pack: recipe
  -> with name: \"Shared pack\"
  -> with is_raw: false
  -> with unit_price: 0
  -> with proportion: 1
  -> with loss: 0%
  -> with target_weight: needed
  -> with components: inner_leaf

rule needed:
  base_weight * 2

rule pack_cost:
  inner_pack.cost

spec first_consumer
uses p: pantry
  -> with base_weight: 10
rule result:
  p.pack_cost

spec second_consumer
uses q: pantry
  -> with base_weight: 20
rule result:
  q.pack_cost
",
        recipe_spec()
    ));
    let pantry_template = |consumer: &str| {
        engine
            .debug_row_template_arc_ptrs(None, consumer)
            .into_iter()
            .find(|(holder, identity, _)| holder.is_empty() && identity == "pantry")
            .unwrap_or_else(|| panic!("{consumer} has no `pantry` template"))
            .2
    };
    assert_eq!(
        pantry_template("first_consumer"),
        pantry_template("second_consumer"),
        "both consumers must read one `pantry` template"
    );
    assert_eq!(
        value(&evaluate(&engine, "first_consumer", data(&[])), "result"),
        "80"
    );
    assert_eq!(
        value(&evaluate(&engine, "second_consumer", data(&[])), "result"),
        "160"
    );
}

#[test]
fn deferred_nested_binding_in_explain_and_show() {
    let engine = load_ok(&blend_order_source("share"));
    let response = evaluate_explain(&engine, "blend_order", data(&[]));
    assert_eq!(value(&response, "result_cost"), "612");
    let explanation = response.results["result_cost"]
        .explanation
        .as_ref()
        .expect("result_cost explanation");
    let json = serde_json::to_string(explanation).expect("explanation json");
    // The deferred nested reference resolved: the leaf's target_weight shows the
    // value `share` produced, qualified to its row (`mid`'s rule `required`
    // narrates as well).
    assert_contains(&json, "mix.components.1.components.1.target_weight");
    assert_contains(&json, "76.5");
    assert_contains(&json, "required");
    let show = engine
        .show(None, "blend_order", None)
        .unwrap_or_else(|error| panic!("show blend_order: {error}"));
    assert_contains(&format!("{show:?}"), "components");
}

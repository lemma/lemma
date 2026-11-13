//! Contract for uses / data / rule / lists. Red until the engine matches the plan.
//!
//! - `data` is overridable input
//! - `rule` builds and joins lists; constructed rows are closed: a rule reading a
//!   field a `-> with` row does not set is a planning Error
//! - `uses` is import only; never asks; `-> with` is sealed on that instance and
//!   reaches no row: rows have their own keys (`items.1.ath`)
//! - no `uses x list`; instance lists are `data` or a list-valued rule

use crate::lists::{
    assert_contains, boolean, data, evaluate, evaluate_err, evaluate_explain, list, lit, load_err,
    load_ok, load_planning_err, map, missing_data, value,
};
use indexmap::IndexMap;
use lemma::{DateTimeValue, Engine, SourceType};

const ITEM: &str = r#"
spec item
data name: text
data score: number
data bonus: number
data ath: number
rule beat_ath:
  score + bonus > ath
rule scaled:
  score * bonus
"#;

fn missing_sorted(response: &lemma::Response, rule: &str) -> Vec<String> {
    let mut keys = missing_data(response, rule);
    keys.sort();
    keys
}

fn assert_request_err(err: &lemma::Error, needle: &str) {
    let text = err.to_string();
    assert!(
        text.contains(needle) || matches!(err, lemma::Error::Request { .. }),
        "expected request Error mentioning `{needle}`, got: {text}"
    );
}

// --- uses never asks; item asks ---

#[test]
fn open_singleton_item_asks_score_bonus_ath() {
    let engine = load_ok(&format!(
        "{ITEM}
spec cart
uses item
rule check:
  item.beat_ath
"
    ));
    let response = evaluate(&engine, "cart", data(&[]));
    assert_eq!(
        missing_sorted(&response, "check"),
        vec![
            "item.ath".to_string(),
            "item.bonus".to_string(),
            "item.score".to_string()
        ]
    );
}

#[test]
fn uses_constant_ath_sealed_item_does_not_ask_ath() {
    let engine = load_ok(&format!(
        "{ITEM}
spec cart
uses item
  -> with ath: 1200
rule check:
  item.beat_ath
"
    ));
    let response = evaluate(&engine, "cart", data(&[]));
    assert_eq!(
        missing_sorted(&response, "check"),
        vec!["item.bonus".to_string(), "item.score".to_string()]
    );
}

#[test]
fn sending_sealed_uses_ath_is_request_error() {
    let engine = load_ok(&format!(
        "{ITEM}
spec cart
uses item
  -> with ath: 1200
rule check:
  item.ath
"
    ));
    let err = evaluate_err(&engine, "cart", data(&[("item.ath", lit("1"))]));
    assert_request_err(&err, "ath");
}

#[test]
fn leave_bonus_open_asks_item_bonus_only() {
    let engine = load_ok(&format!(
        "{ITEM}
spec cart
uses item
  -> with ath: 1200
  -> with score: 1500
rule check:
  item.beat_ath
"
    ));
    let response = evaluate(&engine, "cart", data(&[]));
    assert_eq!(
        missing_sorted(&response, "check"),
        vec!["item.bonus".to_string()]
    );
}

#[test]
fn wire_to_local_data_asks_extra_not_item_bonus() {
    let engine = load_ok(&format!(
        "{ITEM}
spec cart
uses item
  -> with bonus: extra
data extra: number
rule check:
  item.beat_ath
"
    ));
    let response = evaluate(&engine, "cart", data(&[]));
    let missing = missing_sorted(&response, "check");
    assert!(
        missing.contains(&"extra".to_string()),
        "expected extra in {missing:?}"
    );
    assert!(
        missing.contains(&"item.score".to_string()),
        "expected item.score in {missing:?}"
    );
    assert!(
        missing.contains(&"item.ath".to_string()),
        "expected item.ath in {missing:?}"
    );
    assert!(
        !missing.contains(&"item.bonus".to_string()),
        "must not ask item.bonus when wired to extra: {missing:?}"
    );
}

#[test]
fn sending_wired_item_bonus_is_request_error() {
    let engine = load_ok(&format!(
        "{ITEM}
spec cart
uses item
  -> with bonus: extra
  -> with score: 1
  -> with ath: 0
data extra: number
rule check:
  item.beat_ath
"
    ));
    let err = evaluate_err(
        &engine,
        "cart",
        data(&[("extra", lit("5")), ("item.bonus", lit("9"))]),
    );
    assert_request_err(&err, "bonus");
}

#[test]
fn wire_shared_across_two_imports_asks_extra_once() {
    let engine = load_ok(&format!(
        "{ITEM}
spec other
data bonus: number
rule doubled:
  bonus * 2

spec cart
uses item
  -> with bonus: extra
uses other
  -> with bonus: extra
data extra: number
rule out:
  item.scaled + other.doubled
"
    ));
    let response = evaluate(&engine, "cart", data(&[]));
    let missing = missing_sorted(&response, "out");
    assert_eq!(
        missing.iter().filter(|k| *k == "extra").count(),
        1,
        "extra once: {missing:?}"
    );
    assert!(
        missing.contains(&"item.score".to_string()),
        "expected item.score in {missing:?}"
    );
}

#[test]
fn wire_to_rule_is_not_input_check_evaluates() {
    let engine = load_ok(&format!(
        "{ITEM}
spec cart
uses item
  -> with bonus: computed
  -> with score: 10
  -> with ath: 0
rule computed:
  5
rule check:
  item.beat_ath
"
    ));
    let response = evaluate(&engine, "cart", data(&[]));
    assert!(
        missing_data(&response, "check").is_empty(),
        "expected no missing, got {:?}",
        missing_data(&response, "check")
    );
    assert_eq!(value(&response, "check"), "true");
}

#[test]
fn sealed_literal_uses_key_absent_from_show_data() {
    let code = r#"
spec inner
data v: number

spec outer
uses i: inner
  -> with v: 42
rule r: i.v
"#;
    let mut engine = Engine::new();
    engine
        .load([(SourceType::Volatile, code.to_string())])
        .unwrap();
    let now = DateTimeValue::now();
    let show = engine.show(None, "outer", Some(&now)).unwrap();
    assert!(
        !show.data.contains_key("i.v"),
        "sealed uses key must not be an input in show.data: {:?}",
        show.data.keys().collect::<Vec<_>>()
    );
}

#[test]
fn evaluate_cannot_override_wired_uses_slot() {
    let code = r#"
spec inner
data a: number -> suggest 1
data b: number

spec outer
uses i: inner
  -> with a: i.b
data input: number
rule r: i.a + input
"#;
    let mut engine = Engine::new();
    engine
        .load([(SourceType::Volatile, code.to_string())])
        .unwrap();
    let now = DateTimeValue::now();
    let response = engine
        .evaluate(
            None,
            "outer",
            Some(&now),
            IndexMap::new(),
            Some(&["r".to_string()]),
            false,
        )
        .expect("evaluate");
    let mut missing = response
        .results
        .get("r")
        .expect("r")
        .missing_data()
        .to_vec();
    missing.sort();
    assert_eq!(
        missing,
        vec!["i.b".to_string(), "input".to_string()],
        "open inputs are wire target and local data, not sealed i.a"
    );
    let show = engine.show(None, "outer", Some(&now)).unwrap();
    assert!(
        !show.data.contains_key("i.a"),
        "sealed wire LHS must not appear for override: {:?}",
        show.data.keys().collect::<Vec<_>>()
    );
}

#[test]
fn sending_sealed_uses_literal_is_request_error_not_override() {
    let code = r#"
spec a
data x: number
  -> minimum 4

spec b
uses a
  -> with x: 6

spec c
uses b
rule z: b.a.x
"#;
    let mut engine = Engine::new();
    engine
        .load([(SourceType::Volatile, code.to_string())])
        .expect("load");
    let now = DateTimeValue::now();
    let mut data = IndexMap::new();
    data.insert("b.a.x".to_string(), "8".into());
    let err = engine
        .evaluate(None, "c", Some(&now), data, None, false)
        .expect_err("sealed uses key is a request error");
    assert_request_err(&err, "x");
}

#[test]
fn wire_to_rule_slot_cannot_be_supplied() {
    let code = r#"
spec source_spec
data x: number
rule computed: x * 2

spec inner
data slot: number

spec outer
uses src: source_spec
uses i: inner
  -> with slot: src.computed
rule r: i.slot
"#;
    let mut engine = Engine::new();
    engine
        .load([(SourceType::Volatile, code.to_string())])
        .expect("loads");
    let mut data = IndexMap::new();
    data.insert("i.slot".to_string(), "10".into());
    let now = DateTimeValue::now();
    let err = engine
        .evaluate(None, "outer", Some(&now), data, None, false)
        .expect_err("sealed uses wire is a request error");
    assert_request_err(&err, "slot");
}

#[test]
fn rule_bound_import_slot_ignores_runtime_override_as_request_error() {
    let code = r#"
spec inner
data slot: number -> maximum 100

spec outer
rule big: 150
uses i: inner
  -> with slot: big
rule direct_read: i.slot
"#;
    let mut engine = Engine::new();
    engine
        .load([(SourceType::Volatile, code.to_string())])
        .expect("plan");
    let now = DateTimeValue::now();
    let mut data = IndexMap::new();
    data.insert("i.slot".to_string(), "50".into());
    let err = engine
        .evaluate(None, "outer", Some(&now), data, None, false)
        .expect_err("sealed uses key is a request error");
    assert_request_err(&err, "slot");
}

#[test]
fn rule_bound_import_slot_without_override_uses_wire() {
    let code = r#"
spec inner
data slot: number

spec outer
rule big: 42
uses i: inner
  -> with slot: big
rule direct_read: i.slot
"#;
    let mut engine = Engine::new();
    engine
        .load([(SourceType::Volatile, code.to_string())])
        .expect("plan");
    let now = DateTimeValue::now();
    let result = engine
        .evaluate(None, "outer", Some(&now), IndexMap::new(), None, false)
        .expect("evaluate");
    assert_eq!(
        result
            .results
            .get("direct_read")
            .expect("direct_read")
            .result(),
        Some("42")
    );
}

// --- no uses x list ---

#[test]
fn uses_items_item_list_is_error() {
    let err = load_err(&format!(
        "{ITEM}
spec cart
uses items: item list
"
    ));
    assert_contains(&err, "uses");
    assert!(
        err.contains("list") || err.contains("data") || err.contains("rule"),
        "error should steer to uses item plus data or a rule list: {err}"
    );
}

#[test]
fn uses_items_item_list_with_rows_is_error() {
    let err = load_err(&format!(
        "{ITEM}
spec cart
uses items: item list
  -> with name: \"Ben\", score: 1500, bonus: 50, ath: 1200
"
    ));
    assert_contains(&err, "uses");
}

#[test]
fn sealed_list_is_a_rule_not_uses_list() {
    let engine = load_ok(&format!(
        "{ITEM}
spec cart
uses item
rule items:
  item list
    -> with name: \"Ben\", score: 1500, bonus: 50, ath: 1200
rule winners:
  items -> where beat_ath -> count
"
    ));
    let response = evaluate(&engine, "cart", data(&[]));
    assert!(
        missing_data(&response, "winners").is_empty(),
        "sealed rule list asks nothing: {:?}",
        missing_data(&response, "winners")
    );
    assert_eq!(value(&response, "winners"), "1");
}

// --- data instance lists ---

#[test]
fn data_spec_list_plans() {
    let engine = load_ok(
        r#"
spec employee
data name: text
spec payroll
uses employee
data staff: employee list
rule n:
  staff -> count
"#,
    );
    assert_eq!(value(&evaluate(&engine, "payroll", data(&[])), "n"), "0");
}

#[test]
fn data_items_caller_rows_ask_per_row_fields() {
    let engine = load_ok(&format!(
        "{ITEM}
spec cart
uses item
data items: item list
rule winners:
  items -> where beat_ath
"
    ));
    let response = evaluate(
        &engine,
        "cart",
        data(&[(
            "items",
            list(vec![map(&[("name", lit("Ben")), ("score", lit("1500"))])]),
        )]),
    );
    assert_eq!(
        missing_sorted(&response, "winners"),
        vec!["items.1.ath".to_string(), "items.1.bonus".to_string()]
    );
}

#[test]
fn per_row_key_fills_unbound_field() {
    let engine = load_ok(&format!(
        "{ITEM}
spec cart
uses item
data items: item list
  -> with name: \"Ben\", score: 1500, ath: 0
rule winners:
  items -> where beat_ath -> count
"
    ));
    let response = evaluate(&engine, "cart", data(&[("items.1.bonus", lit("50"))]));
    assert!(
        missing_data(&response, "winners").is_empty(),
        "{:?}",
        missing_data(&response, "winners")
    );
    assert_eq!(value(&response, "winners"), "1");
}

#[test]
fn dotted_and_whole_list_same_request_is_error() {
    let engine = load_ok(&format!(
        "{ITEM}
spec cart
uses item
  -> with ath: 0
data items: item list
  -> with name: \"Ben\", score: 1500
rule winners:
  items -> where beat_ath -> count
"
    ));
    let err = evaluate_err(
        &engine,
        "cart",
        data(&[
            (
                "items",
                list(vec![map(&[
                    ("name", lit("Ben")),
                    ("score", lit("1500")),
                    ("bonus", lit("50")),
                ])]),
            ),
            ("items.1.bonus", lit("50")),
        ]),
    );
    assert_request_err(&err, "items");
}

#[test]
fn dotted_row_out_of_range_is_error() {
    let engine = load_ok(&format!(
        "{ITEM}
spec cart
uses item
  -> with ath: 0
data items: item list
  -> with name: \"Ben\", score: 1500
rule winners:
  items -> where beat_ath -> count
"
    ));
    let err = evaluate_err(&engine, "cart", data(&[("items.9.bonus", lit("50"))]));
    assert_request_err(&err, "items");
}

#[test]
fn dotted_sealed_field_is_error() {
    let engine = load_ok(&format!(
        "{ITEM}
spec cart
uses item
  -> with ath: 1200
data items: item list
  -> with name: \"Ben\", score: 1500, bonus: 50, ath: 1200
rule winners:
  items -> where beat_ath -> count
"
    ));
    let err = evaluate_err(&engine, "cart", data(&[("items.1.ath", lit("1"))]));
    assert_request_err(&err, "ath");
}

#[test]
fn uses_template_applies_to_data_rows_ath_sealed() {
    let engine = load_ok(&format!(
        "{ITEM}
spec cart
uses item
  -> with ath: 1200
data items: item list
rule winners:
  items -> where beat_ath
"
    ));
    let response = evaluate(
        &engine,
        "cart",
        data(&[(
            "items",
            list(vec![map(&[("name", lit("Ben")), ("score", lit("1500"))])]),
        )]),
    );
    // The `uses item` literal seals `item.ath` only; a row's `ath` is its own key.
    assert_eq!(
        missing_sorted(&response, "winners"),
        vec!["items.1.ath".to_string(), "items.1.bonus".to_string()]
    );
    let with_ath = evaluate(
        &engine,
        "cart",
        data(&[(
            "items",
            list(vec![map(&[
                ("name", lit("Ben")),
                ("score", lit("1500")),
                ("ath", lit("1")),
            ])]),
        )]),
    );
    assert_eq!(
        missing_sorted(&with_ath, "winners"),
        vec!["items.1.bonus".to_string()]
    );
}

#[test]
fn data_list_default_rows_still_replaceable() {
    let engine = load_ok(&format!(
        "{ITEM}
spec cart
uses item
  -> with ath: 1200
data items: item list
  -> with name: \"Ben\", score: 1500
rule winners:
  items -> where beat_ath
rule n:
  winners -> count
"
    ));
    let response = evaluate(&engine, "cart", data(&[]));
    assert_eq!(
        missing_sorted(&response, "winners"),
        vec!["items.1.ath".to_string(), "items.1.bonus".to_string()]
    );
    let cleared = evaluate(&engine, "cart", data(&[("items", list(vec![]))]));
    assert_eq!(value(&cleared, "n"), "0");
}

#[test]
fn two_data_lists_ask_separate_row_keys() {
    let engine = load_ok(&format!(
        "{ITEM}
spec cart
uses item
  -> with ath: 1200
data low: item list
  -> with name: \"Ben\", score: 1500
data high: item list
  -> with name: \"Ben\", score: 1600
rule winners:
  (low -> any beat_ath) and (high -> all beat_ath)
"
    ));
    let response = evaluate(&engine, "cart", data(&[]));
    assert_eq!(
        missing_sorted(&response, "winners"),
        vec![
            "high.1.ath".to_string(),
            "high.1.bonus".to_string(),
            "low.1.ath".to_string(),
            "low.1.bonus".to_string()
        ]
    );
}

#[test]
fn key_on_data_instance_list_plans() {
    let engine = load_ok(&format!(
        "{ITEM}
spec cart
uses item
data items: item list
  -> key name
  -> with name: \"Ben\", score: 1500, bonus: 50, ath: 0
rule picked:
  (items -> find \"Ben\").score
"
    ));
    assert_eq!(
        value(&evaluate(&engine, "cart", data(&[])), "picked"),
        "1500"
    );
}

// --- rule builds lists ---

#[test]
fn rule_instance_list_with_finished_rows_plans() {
    let engine = load_ok(
        r#"
spec menu_item
data drink: text
spec coffee_order
uses menu_item
rule menu:
  menu_item list
    -> with drink: "espresso"
rule first_drink:
  (menu -> first).drink
"#,
    );
    assert_eq!(
        value(&evaluate(&engine, "coffee_order", data(&[])), "first_drink"),
        "espresso"
    );
}

#[test]
fn rule_built_winners_asks_nothing_when_row_complete() {
    let engine = load_ok(&format!(
        "{ITEM}
spec cart
uses item
rule winners:
  item list
    -> with name: \"Ben\", score: 1500, bonus: 50, ath: 1200
    -> where beat_ath
    -> count
"
    ));
    let response = evaluate(&engine, "cart", data(&[]));
    assert!(missing_data(&response, "winners").is_empty());
    assert_eq!(value(&response, "winners"), "1");
}

#[test]
fn rule_built_row_omits_bonus_asks_item_bonus() {
    let err = load_planning_err(&format!(
        "{ITEM}
spec cart
uses item
rule winners:
  item list
    -> with name: \"Ben\", score: 1500, ath: 1200
    -> where beat_ath
"
    ));
    assert_contains(
        &err,
        "`beat_ath` reads `bonus`, which a row appended with `-> with` does not set; add `bonus: ...` to that row",
    );
    assert!(
        !err.contains("winners."),
        "must not invent winners.N.bonus: {err}"
    );
}

#[test]
fn unused_field_may_be_omitted_on_rule_built_row() {
    let engine = load_ok(&format!(
        "{ITEM}
spec cart
uses item
rule average:
  item list
    -> with score: 5
    -> avg score
"
    ));
    let response = evaluate(&engine, "cart", data(&[]));
    assert!(missing_data(&response, "average").is_empty());
    assert_eq!(value(&response, "average"), "5");
}

#[test]
fn downstream_reader_does_not_reopen_built() {
    let engine = load_ok(&format!(
        "{ITEM}
spec cart
uses item
rule built:
  item list -> with score: 5, bonus: 2
rule later:
  built -> avg scaled
"
    ));
    let response = evaluate(&engine, "cart", data(&[]));
    assert!(
        missing_data(&response, "later").is_empty(),
        "no built.1.bonus: {:?}",
        missing_data(&response, "later")
    );
    assert_eq!(value(&response, "later"), "10");
}

#[test]
fn downstream_reader_uses_sealed_uses_bonus() {
    // The `uses item` literal seals `item.bonus` only; the built row sets its own.
    let engine = load_ok(&format!(
        "{ITEM}
spec cart
uses item
  -> with bonus: 2
rule built:
  item list -> with score: 5, bonus: 3
rule later:
  built -> avg scaled
"
    ));
    let response = evaluate(&engine, "cart", data(&[]));
    assert!(missing_data(&response, "later").is_empty());
    assert_eq!(value(&response, "later"), "15");
    let err = load_planning_err(&format!(
        "{ITEM}
spec cart
uses item
  -> with bonus: 2
rule built:
  item list -> with score: 5
rule later:
  built -> avg scaled
"
    ));
    assert_contains(
        &err,
        "`scaled` reads `bonus`, which a row appended with `-> with` does not set",
    );
}

#[test]
fn full_row_also_plans() {
    let engine = load_ok(&format!(
        "{ITEM}
spec cart
uses item
rule average_full:
  item list
    -> with score: 5, bonus: 2, ath: 0, name: \"x\"
    -> avg score
"
    ));
    let response = evaluate(&engine, "cart", data(&[]));
    assert!(missing_data(&response, "average_full").is_empty());
    assert_eq!(value(&response, "average_full"), "5");
}

#[test]
fn unless_arms_are_expressions_flag_is_only_input() {
    let engine = load_ok(&format!(
        "{ITEM}
spec cart
uses item
data flag: boolean
rule total:
  item list -> with score: 1500, bonus: 50, ath: 1200 -> any beat_ath
  unless flag then
    item list -> with score: 300, bonus: 10, ath: 1200 -> any beat_ath
"
    ));
    let response = evaluate(&engine, "cart", data(&[]));
    assert_eq!(missing_sorted(&response, "total"), vec!["flag".to_string()]);
    let on = evaluate(&engine, "cart", data(&[("flag", boolean(true))]));
    assert!(missing_data(&on, "total").is_empty());
    assert_eq!(value(&on, "total"), "false");
    let off = evaluate(&engine, "cart", data(&[("flag", boolean(false))]));
    assert!(missing_data(&off, "total").is_empty());
    assert_eq!(value(&off, "total"), "true");
}

#[test]
fn house_omits_bonus_asks_item_bonus_not_menu_row() {
    let err = load_planning_err(&format!(
        "{ITEM}
spec cart
uses item
data extras: item list
rule menu:
  item list
    -> with name: \"House\", score: 100, ath: 1200
    -> with extras
rule winners:
  menu -> where beat_ath
"
    ));
    assert_contains(
        &err,
        "`beat_ath` reads `bonus`, which a row appended with `-> with` does not set; add `bonus: ...` to that row",
    );
    assert!(!err.contains("menu."), "no menu.1.bonus: {err}");
}

#[test]
fn two_item_lists_in_one_rule_both_finished() {
    let engine = load_ok(&format!(
        "{ITEM}
spec cart
uses item
rule winners:
  (item list -> with score: 1500, bonus: 50, ath: 1200 -> any beat_ath)
  and
  (item list -> with score: 1600, bonus: 10, ath: 1200 -> all beat_ath)
"
    ));
    let response = evaluate(&engine, "cart", data(&[]));
    assert!(missing_data(&response, "winners").is_empty());
    assert_eq!(value(&response, "winners"), "true");
}

#[test]
fn two_item_lists_one_omits_bonus_asks_item_bonus() {
    let err = load_planning_err(&format!(
        "{ITEM}
spec cart
uses item
rule winners:
  (item list -> with score: 1500, ath: 1200 -> any beat_ath)
  and
  (item list -> with score: 1600, bonus: 10, ath: 1200 -> all beat_ath)
"
    ));
    assert_contains(
        &err,
        "`beat_ath` reads `bonus`, which a row appended with `-> with` does not set; add `bonus: ...` to that row",
    );
    assert_eq!(
        err.matches("does not set").count(),
        1,
        "only the row omitting bonus is rejected: {err}"
    );
}

#[test]
fn local_data_does_not_bind_to_local_rule_menu_drink() {
    let err = load_err(
        r#"
spec menu_item
data drink: text
  -> options "espresso", "latte"
data price: number

spec coffee_order
uses menu_item
rule menu:
  menu_item list
    -> key drink
    -> with drink: "espresso", price: 2.50
    -> with drink: "latte",    price: 3.50
data ordered: menu.drink
rule selected_drink:
  menu -> find ordered
"#,
    );
    assert_contains(&err, "menu");
}

#[test]
fn type_position_uses_imported_spec_field() {
    let engine = load_ok(
        r#"
spec menu_item
data drink: text
  -> options "espresso", "latte"
data price: number

spec coffee_order
uses menu_item
rule menu:
  menu_item list
    -> key drink
    -> with drink: "espresso", price: 2.50
    -> with drink: "latte",    price: 3.50
data ordered: menu_item.drink
rule selected_drink:
  (menu -> find ordered).drink
"#,
    );
    assert_eq!(
        value(
            &evaluate(
                &engine,
                "coffee_order",
                data(&[("ordered", lit("espresso"))])
            ),
            "selected_drink"
        ),
        "espresso"
    );
}

#[test]
fn type_position_through_data_list_alias_plans() {
    let engine = load_ok(
        r#"
spec menu_item
data drink: text
  -> options "espresso", "latte"
data price: number

spec coffee_order
uses menu_item
data menu: menu_item list
  -> key drink
data ordered: menu.drink
rule selected_drink:
  (menu -> find ordered).drink
"#,
    );
    assert_eq!(
        value(
            &evaluate(
                &engine,
                "coffee_order",
                data(&[
                    (
                        "menu",
                        list(vec![
                            map(&[("drink", lit("espresso")), ("price", lit("2.50"))]),
                            map(&[("drink", lit("latte")), ("price", lit("3.50"))]),
                        ]),
                    ),
                    ("ordered", lit("latte")),
                ])
            ),
            "selected_drink"
        ),
        "latte"
    );
}

#[test]
fn key_on_rule_built_list() {
    let engine = load_ok(
        r#"
spec rate
data band: number range
data price: number

spec shipping
uses rate
rule rates:
  rate list
    -> key band
    -> with band: 0...2, price: 5
    -> with band: 2...10, price: 7
rule cost:
  (rates -> find 1).price
"#,
    );
    assert_eq!(
        value(&evaluate(&engine, "shipping", data(&[])), "cost"),
        "5"
    );
}

// --- extend and merge ---

#[test]
fn append_caller_rows_to_constructed_list() {
    let engine = load_ok(&format!(
        "{ITEM}
spec cart
uses item
data extras: item list
rule menu:
  item list
    -> with name: \"House\", score: 100, bonus: 0, ath: 1200
    -> with extras
rule winners:
  menu -> where beat_ath
rule n:
  menu -> count
"
    ));
    let empty = evaluate(&engine, "cart", data(&[("extras", list(vec![]))]));
    assert_eq!(value(&empty, "n"), "1");
    let response = evaluate(
        &engine,
        "cart",
        data(&[(
            "extras",
            list(vec![map(&[("name", lit("Guest")), ("score", lit("1500"))])]),
        )]),
    );
    assert_eq!(
        missing_sorted(&response, "winners"),
        vec!["extras.1.ath".to_string(), "extras.1.bonus".to_string()]
    );
}

#[test]
fn join_two_list_valued_rules() {
    let engine = load_ok(&format!(
        "{ITEM}
spec cart
uses item
  -> with ath: 1200
rule house:
  item list -> with score: 100, bonus: 0
rule guests:
  item list -> with score: 1500, bonus: 50
rule menu:
  house -> with guests
rule n:
  menu -> count
"
    ));
    assert_eq!(value(&evaluate(&engine, "cart", data(&[])), "n"), "2");
}

#[test]
fn join_two_data_lists() {
    let engine = load_ok(&format!(
        "{ITEM}
spec cart
uses item
data low: item list
data high: item list
rule joined:
  low -> with high
rule n:
  joined -> count
"
    ));
    let response = evaluate(
        &engine,
        "cart",
        data(&[
            (
                "low",
                list(vec![map(&[
                    ("score", lit("1")),
                    ("bonus", lit("0")),
                    ("ath", lit("0")),
                ])]),
            ),
            (
                "high",
                list(vec![map(&[
                    ("score", lit("2")),
                    ("bonus", lit("0")),
                    ("ath", lit("0")),
                ])]),
            ),
        ]),
    );
    assert_eq!(value(&response, "n"), "2");
}

#[test]
fn named_instances_as_rows() {
    let engine = load_ok(&format!(
        "{ITEM}
spec cart
uses item
uses ada: item
  -> with name: \"Ada\", score: 1500, bonus: 50, ath: 0
uses grace: item
  -> with name: \"Grace\", score: 1600, bonus: 10, ath: 0
rule staff:
  item list -> with ada, grace
rule total:
  staff -> sum score
"
    ));
    assert_eq!(
        value(&evaluate(&engine, "cart", data(&[])), "total"),
        "3100"
    );
}

#[test]
fn named_instance_open_bonus_asks_ada_bonus() {
    let engine = load_ok(&format!(
        "{ITEM}
spec cart
uses item
uses ada: item
  -> with name: \"Ada\", score: 1500, ath: 0
uses grace: item
  -> with name: \"Grace\", score: 1600, bonus: 10, ath: 0
rule staff:
  item list -> with ada, grace
rule total:
  staff -> sum scaled
"
    ));
    let response = evaluate(&engine, "cart", data(&[]));
    assert_eq!(
        missing_sorted(&response, "total"),
        vec!["ada.bonus".to_string()]
    );
}

#[test]
fn uses_wire_list_slot_from_list_valued_rule() {
    let engine = load_ok(
        r#"
spec row
data n: number

spec catalog
uses row
rule rows: row list -> with n: 5

spec consumer
uses row
data rows: row list
rule total: rows -> sum n

spec root
uses catalog
uses c: consumer
  -> with rows: catalog.rows
rule out: c.total
"#,
    );
    assert_eq!(value(&evaluate(&engine, "root", data(&[])), "out"), "5");
}

#[test]
fn uses_wire_list_slot_from_sealed_data_list() {
    let engine = load_ok(
        r#"
spec item
data target: number

spec box
uses item
data items: item list

spec consumer
uses item
data items: item list
rule total: items -> sum target

spec root
uses i1: item
  -> with target: 7
uses holder: box
  -> with items: i1
uses c: consumer
  -> with items: holder.items
rule out: c.total
"#,
    );
    assert_eq!(value(&evaluate(&engine, "root", data(&[])), "out"), "7");
}

#[test]
fn uses_wire_list_slot_wrong_row_spec_is_planning_error() {
    let err = load_planning_err(
        r#"
spec line
data qty: number
spec other
data name: text
spec order
uses line
data lines: line list
spec catalog
uses other
data rows: other list
  -> with name: "x"
spec wrapper
uses catalog
uses o: order
  -> with lines: catalog.rows
"#,
    );
    assert_contains(&err, "type mismatch");
}

#[test]
fn uses_wire_list_slot_rejects_evaluate_data() {
    let engine = load_ok(
        r#"
spec row
data n: number

spec catalog
uses row
rule rows: row list -> with n: 5

spec consumer
uses row
data rows: row list
rule total: rows -> sum n

spec root
uses catalog
uses c: consumer
  -> with rows: catalog.rows
rule out: c.total
"#,
    );
    let err = evaluate_err(
        &engine,
        "root",
        data(&[("c.rows", list(vec![map(&[("n", lit("1"))])]))]),
    );
    assert_request_err(&err, "rows");
}

#[test]
fn uses_wire_list_slot_mixed_alias_and_reference_is_planning_error() {
    let err = load_planning_err(
        r#"
spec item
data target: number

spec box
uses item
data items: item list

spec root
uses i1: item
  -> with target: 1
uses holder: box
  -> with items: i1
uses c: box
  -> with items: i1, holder.items
"#,
    );
    assert_contains(
        &err,
        "list slots take local `uses` aliases or one list-typed",
    );
}

#[test]
fn uses_wire_list_slot_scalar_target_is_planning_error() {
    let err = load_planning_err(
        r#"
spec item
data target: number

spec box
uses item
data items: item list

spec root
uses src: item
  -> with target: 9
uses c: box
  -> with items: src.target
"#,
    );
    assert_contains(&err, "type mismatch");
}

#[test]
fn uses_wire_list_slot_cycle_is_planning_error() {
    let err = load_planning_err(
        r#"
spec item
data target: number

spec box
uses item
data items: item list

spec root
uses a: box
  -> with items: b.items
uses b: box
  -> with items: a.items
rule out: a.items -> count
"#,
    );
    assert_contains(&err, "Circular data reference");
}

#[test]
fn uses_wire_list_slot_open_data_list_asks_row_fields() {
    // `data extras: item list` with no rows is empty, never missing data.
    // Wire copies that list; evaluate data can replace the whole list.
    let engine = load_ok(
        r#"
spec item
data target: number

spec box
uses item
data items: item list
rule total: items -> sum target

spec root
uses item
uses c: box
  -> with items: extras
data extras: item list
rule out: c.total
"#,
    );
    let empty = evaluate(&engine, "root", data(&[]));
    assert!(
        missing_data(&empty, "out").is_empty(),
        "list without rows is empty, not missing: {:?}",
        missing_data(&empty, "out")
    );
    assert_eq!(value(&empty, "out"), "0");
    let response = evaluate(
        &engine,
        "root",
        data(&[("extras", list(vec![map(&[("target", lit("3"))])]))]),
    );
    assert_eq!(value(&response, "out"), "3");
}

#[test]
fn uses_wire_list_slot_empty_list_rule_sums_zero() {
    let engine = load_ok(
        r#"
spec item
data target: number

spec catalog
uses item
rule rows: item list

spec consumer
uses item
data items: item list
rule total: items -> sum target

spec root
uses catalog
uses c: consumer
  -> with items: catalog.rows
rule out: c.total
"#,
    );
    assert_eq!(value(&evaluate(&engine, "root", data(&[])), "out"), "0");
}

#[test]
fn uses_wire_list_slot_multi_row_list_rule() {
    let engine = load_ok(
        r#"
spec row
data n: number

spec catalog
uses row
rule rows:
  row list
  -> with n: 2
  -> with n: 3

spec consumer
uses row
data rows: row list
rule total: rows -> sum n

spec root
uses catalog
uses c: consumer
  -> with rows: catalog.rows
rule out: c.total
"#,
    );
    assert_eq!(value(&evaluate(&engine, "root", data(&[])), "out"), "5");
}

#[test]
fn uses_wire_list_slot_chained_deferred_wires() {
    let engine = load_ok(
        r#"
spec item
data target: number

spec box
uses item
data items: item list
rule total: items -> sum target

spec root
uses i1: item
  -> with target: 7
uses holder: box
  -> with items: i1
uses mid: box
  -> with items: holder.items
uses c: box
  -> with items: mid.items
rule out: c.total
"#,
    );
    assert_eq!(value(&evaluate(&engine, "root", data(&[])), "out"), "7");
}

#[test]
fn uses_wire_list_slot_explain_evaluates() {
    let engine = load_ok(
        r#"
spec item
data target: number

spec box
uses item
data items: item list
rule total: items -> sum target

spec root
uses i1: item
  -> with target: 7
uses holder: box
  -> with items: i1
uses c: box
  -> with items: holder.items
rule out: c.total
"#,
    );
    assert_eq!(
        value(&evaluate_explain(&engine, "root", data(&[])), "out"),
        "7"
    );
}

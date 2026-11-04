//! Show.rules is this spec's local catalog. Reachable imports are not catalog keys;
//! branch Rule leaves may still name them.

use indexmap::IndexMap;
use lemma::RulePath;
use lemma::{DateTimeValue, Engine, ShowExpression, SourceType};
use std::path::PathBuf;
use std::sync::Arc;

fn dep_input_keys(deps: &[RulePath]) -> Vec<String> {
    deps.iter().map(RulePath::input_key).collect()
}

fn engine(code: &str) -> Engine {
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Path(Arc::new(PathBuf::from("show_graph.lemma"))),
            code.to_string(),
        )])
        .expect("load");
    engine
}

#[test]
fn show_rule_default_only_has_one_branch_without_condition() {
    let engine = engine(
        r#"
        spec g
        rule r: 1
        "#,
    );
    let show = engine.show(None, "g", None).expect("show");
    let rule = show.rules.get("r").expect("r");
    assert!(rule.path.is_empty());
    assert_eq!(rule.branches.len(), 1);
    assert!(rule.branches[0].condition.is_none());
    assert!(matches!(
        rule.branches[0].result,
        ShowExpression::Literal(_)
    ));
    assert!(rule.depends_on_rules.is_empty());
}

#[test]
fn show_rule_unless_order_and_rule_leaf_in_depends_on_rules() {
    let engine = engine(
        r#"
        spec g
        data is_vip: boolean
        data quantity: number
        rule vip_rate: 20%
        rule bulk_rate: 10%
        rule discount: 0%
          unless is_vip then vip_rate
          unless quantity >= 50 then bulk_rate
        "#,
    );
    let show = engine.show(None, "g", None).expect("show");
    let discount = show.rules.get("discount").expect("discount");
    assert_eq!(discount.branches.len(), 3);
    assert!(discount.branches[0].condition.is_none());
    assert!(discount.branches[1].condition.is_some());
    assert!(discount.branches[2].condition.is_some());
    assert!(matches!(
        discount.branches[1].result,
        ShowExpression::Rule { ref name } if name == "vip_rate"
    ));
    assert!(matches!(
        discount.branches[2].result,
        ShowExpression::Rule { ref name } if name == "bulk_rate"
    ));
    assert_eq!(
        dep_input_keys(&discount.depends_on_rules),
        vec!["bulk_rate".to_string(), "vip_rate".to_string()]
    );
    assert!(show.rules.contains_key("vip_rate"));
    assert!(show.rules.contains_key("bulk_rate"));
}

#[test]
fn show_local_rule_target_via_with_binding_in_depends_on_rules() {
    let engine = engine(
        r#"
        spec inner
        data slot: number

        spec outer
        uses i: inner
          -> with slot: computed
        rule computed: 1
        rule r: i.slot
        "#,
    );
    let show = engine.show(None, "outer", None).expect("show");
    let r = show.rules.get("r").expect("r");
    assert_eq!(
        dep_input_keys(&r.depends_on_rules),
        vec!["computed".to_string()]
    );
    assert_eq!(r.lemma_type.name(), "slot");
    assert!(matches!(
        &r.branches[0].result,
        ShowExpression::Data { name } if name == "i.slot"
    ));
}

/// After `+ 0` identity elim, bare `rule out: helper` must still plan with
/// matching validation vs normalize types (no named-leaf stamp leak).
#[test]
fn identity_elim_plus_zero_then_bare_rule_passthrough_plans() {
    let engine = engine(
        r#"
        spec s
        data money: number
        rule helper: money + 0
        rule out: helper
        "#,
    );
    let show = engine.show(None, "s", None).expect("show");
    assert_eq!(
        show.rules.get("helper").expect("helper").lemma_type.name(),
        "number"
    );
    assert_eq!(
        show.rules.get("out").expect("out").lemma_type.name(),
        "number"
    );
}

#[test]
fn show_catalog_omits_imported_rules_keeps_branch_leaf() {
    let engine = engine(
        r#"
        spec inner
        rule helper: 1
        rule other: 2

        spec outer
        uses i: inner
        rule r: i.helper
        "#,
    );
    let show = engine.show(None, "outer", None).expect("show");
    assert_eq!(
        show.rules.keys().cloned().collect::<Vec<_>>(),
        vec!["r".to_string()]
    );
    assert!(!show.rules.contains_key("i.helper"));
    assert!(!show.rules.contains_key("i.other"));
    assert!(!show.rules.contains_key("helper"));
    let r = show.rules.get("r").expect("r");
    assert!(r.depends_on_rules.is_empty());
    assert!(matches!(
        &r.branches[0].result,
        ShowExpression::Rule { name } if name == "i.helper"
    ));
}

#[test]
fn show_two_level_import_not_in_parent_catalog() {
    let engine = engine(
        r#"
        spec leaf
        rule target: 1
        rule other: 2

        spec mid
        uses b: leaf
        rule pass: b.target

        spec outer
        uses a: mid
        rule r: a.pass
        "#,
    );
    let show = engine.show(None, "outer", None).expect("show");
    assert_eq!(
        show.rules.keys().cloned().collect::<Vec<_>>(),
        vec!["r".to_string()]
    );
    assert!(!show.rules.contains_key("a.pass"));
    assert!(!show.rules.contains_key("a.b.target"));
    assert!(!show.rules.contains_key("a.b.other"));
    let r = show.rules.get("r").expect("r");
    assert!(r.depends_on_rules.is_empty());
    assert!(matches!(
        &r.branches[0].result,
        ShowExpression::Rule { name } if name == "a.pass"
    ));
}

#[test]
fn show_two_aliases_not_catalog_keys() {
    let engine = engine(
        r#"
        spec shared
        rule x: 1

        spec outer
        uses a: shared
        uses b: shared
        rule left: a.x
        rule right: b.x
        "#,
    );
    let show = engine.show(None, "outer", None).expect("show");
    let mut keys: Vec<_> = show.rules.keys().cloned().collect();
    keys.sort();
    assert_eq!(keys, vec!["left".to_string(), "right".to_string()]);
    assert!(!show.rules.contains_key("a.x"));
    assert!(!show.rules.contains_key("b.x"));
    assert!(show
        .rules
        .get("left")
        .expect("left")
        .depends_on_rules
        .is_empty());
    assert!(show
        .rules
        .get("right")
        .expect("right")
        .depends_on_rules
        .is_empty());
}

#[test]
fn show_with_binding_to_imported_rule_omits_import_from_catalog() {
    let engine = engine(
        r#"
        spec inner
        data slot: number

        spec source_spec
        data x0: number
        rule computed: x0 * 2
        rule unused: 0

        spec outer
        uses i: inner
          -> with slot: src.computed
        uses src: source_spec
        rule r: i.slot
        "#,
    );
    let show = engine.show(None, "outer", None).expect("show");
    let r = show.rules.get("r").expect("r");
    assert!(
        r.depends_on_rules.is_empty(),
        "deps: {:?}",
        dep_input_keys(&r.depends_on_rules)
    );
    assert!(!show.rules.contains_key("src.computed"));
    assert!(!show.rules.contains_key("src.unused"));
}

#[test]
fn public_depends_on_rules_are_show_keys_rule_leaves_may_name_imports() {
    let engine = engine(
        r#"
        spec inner
        rule helper: 1

        spec outer
        uses i: inner
        rule vip: 2
        rule r: vip
          unless true then i.helper
        "#,
    );
    let show = engine.show(None, "outer", None).expect("show");
    let keys: std::collections::HashSet<&str> = show.rules.keys().map(String::as_str).collect();
    for (name, rule) in &show.rules {
        for dep in &rule.depends_on_rules {
            assert!(
                keys.contains(dep.input_key().as_str()),
                "depends_on_rules entry '{}' of '{name}' missing from Show.rules",
                dep.input_key()
            );
        }
    }
    let r = show.rules.get("r").expect("r");
    assert_eq!(dep_input_keys(&r.depends_on_rules), vec!["vip".to_string()]);
    assert!(matches!(
        &r.branches[1].result,
        ShowExpression::Rule { name } if name == "i.helper"
    ));
    assert!(!keys.contains("i.helper"));
}

#[test]
fn evaluate_rejects_imported_rule_key() {
    let engine = engine(
        r#"
        spec inner
        rule helper: 7
        rule unused: 9

        spec outer
        uses i: inner
        rule r: i.helper
        "#,
    );
    let now = DateTimeValue::now();
    let err = engine
        .evaluate(
            None,
            "outer",
            Some(&now),
            IndexMap::new(),
            Some(&["i.helper".to_string()]),
            false,
        )
        .expect_err("imported rule must not be selectable on parent");
    assert!(err.to_string().contains("i.helper"), "error: {err}");

    let err = engine
        .evaluate(
            None,
            "outer",
            Some(&now),
            IndexMap::new(),
            Some(&["i.unused".to_string()]),
            false,
        )
        .expect_err("unreferenced import name must error");
    assert!(err.to_string().contains("i.unused"), "error: {err}");
}

#[test]
fn default_run_returns_only_local_outputs() {
    let engine = engine(
        r#"
        spec inner
        rule helper: 1

        spec outer
        uses i: inner
        rule r: i.helper
        "#,
    );
    let now = DateTimeValue::now();
    let show = engine.show(None, "outer", Some(&now)).expect("show");
    let response = engine
        .evaluate(None, "outer", Some(&now), IndexMap::new(), None, false)
        .expect("evaluate");
    let catalog_keys: Vec<_> = show.rules.keys().cloned().collect();
    let result_keys: Vec<_> = response.results.keys().cloned().collect();
    assert_eq!(result_keys, catalog_keys);
    assert!(!response.results.contains_key("i.helper"));
}

#[test]
fn parent_pass_through_catalog_and_selection_contract() {
    let engine = engine(
        r#"
        spec child
        rule y: 1
        rule hidden: 2

        spec parent
        uses child
        rule x: child.y
        "#,
    );
    let now = DateTimeValue::now();
    let show = engine.show(None, "parent", Some(&now)).expect("show");
    assert_eq!(
        show.rules.keys().cloned().collect::<Vec<_>>(),
        vec!["x".to_string()]
    );
    let x = show.rules.get("x").expect("x");
    assert!(x.depends_on_rules.is_empty());
    assert!(matches!(
        &x.branches[0].result,
        ShowExpression::Rule { name } if name == "child.y"
    ));
    assert!(!show.rules.contains_key("child.y"));
    assert!(!show.rules.contains_key("child.hidden"));

    let err = engine
        .evaluate(
            None,
            "parent",
            Some(&now),
            IndexMap::new(),
            Some(&["child.y".to_string()]),
            false,
        )
        .expect_err("child.y not selectable on parent");
    assert!(err.to_string().contains("child.y"), "error: {err}");

    let response = engine
        .evaluate(
            None,
            "parent",
            Some(&now),
            IndexMap::new(),
            Some(&["x".to_string()]),
            false,
        )
        .expect("evaluate x");
    assert_eq!(response.results.get("x").expect("x").result(), Some("1"));
}

#[test]
fn explain_imported_rule_name_is_input_key() {
    let engine = engine(
        r#"
        spec inner
        rule helper: 3

        spec outer
        uses i: inner
        rule r: i.helper
        "#,
    );
    let now = DateTimeValue::now();
    let response = engine
        .evaluate(None, "outer", Some(&now), IndexMap::new(), None, true)
        .expect("evaluate explain");
    let explanation = response
        .results
        .get("r")
        .expect("r")
        .explanation
        .as_ref()
        .expect("explanation");
    let json = serde_json::to_value(explanation).expect("explanation JSON");
    assert_eq!(json["name"], "r");
    let child_names: Vec<_> = json["children"]
        .as_array()
        .expect("children")
        .iter()
        .filter_map(|child| {
            if child["type"] == "rule_ref" {
                child["name"].as_str().map(str::to_string)
            } else {
                None
            }
        })
        .collect();
    assert!(
        child_names.iter().any(|n| n == "i.helper"),
        "children: {child_names:?}"
    );
    let ascii = lemma::format_explanation(explanation);
    assert!(
        ascii.contains("i.helper"),
        "ASCII must use input_key:\n{ascii}"
    );
}

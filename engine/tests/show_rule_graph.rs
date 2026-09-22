//! Show.rules is the reachable rule graph (local + depends_on_rules closure).

use lemma::{DateTimeValue, Engine, ShowExpression, SourceType};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

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
        discount.depends_on_rules,
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
    assert_eq!(r.depends_on_rules, vec!["computed".to_string()]);
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
fn show_includes_used_imported_rule_omits_unused_sibling() {
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
    let helper = show.rules.get("i.helper").expect("i.helper present");
    assert_eq!(helper.path.len(), 1);
    assert_eq!(helper.path[0].uses, "i");
    assert_eq!(helper.path[0].spec, "inner");
    assert!(helper.path[0].repository.is_none());
    assert!(matches!(
        &helper.branches[0].result,
        ShowExpression::Literal(_)
    ));
    assert!(
        !show.rules.contains_key("i.other"),
        "unused import sibling must be absent"
    );
    assert!(!show.rules.contains_key("helper"));
    let r = show.rules.get("r").expect("r");
    assert_eq!(r.depends_on_rules, vec!["i.helper".to_string()]);
    assert!(matches!(
        &r.branches[0].result,
        ShowExpression::Rule { name } if name == "i.helper"
    ));
}

#[test]
fn show_two_level_reachable_import_rule() {
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
    assert!(show.rules.contains_key("a.pass"));
    assert!(show.rules.contains_key("a.b.target"));
    assert!(
        !show.rules.contains_key("a.b.other"),
        "unused two-level sibling must be absent"
    );
    let r = show.rules.get("r").expect("r");
    assert_eq!(r.depends_on_rules, vec!["a.pass".to_string()]);
    let pass = show.rules.get("a.pass").expect("a.pass");
    assert_eq!(pass.depends_on_rules, vec!["a.b.target".to_string()]);
}

#[test]
fn show_two_aliases_of_same_spec_are_distinct_keys() {
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
    assert!(show.rules.contains_key("a.x"));
    assert!(show.rules.contains_key("b.x"));
    assert_eq!(
        show.rules.get("left").expect("left").depends_on_rules,
        vec!["a.x".to_string()]
    );
    assert_eq!(
        show.rules.get("right").expect("right").depends_on_rules,
        vec!["b.x".to_string()]
    );
}

#[test]
fn show_with_binding_to_imported_rule_in_depends_on_rules() {
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
        r.depends_on_rules.contains(&"src.computed".to_string()),
        "deps: {:?}",
        r.depends_on_rules
    );
    assert!(show.rules.contains_key("src.computed"));
    assert!(
        !show.rules.contains_key("src.unused"),
        "unused import helper must be absent"
    );
}

#[test]
fn every_depends_on_rules_and_rule_leaf_is_a_show_key() {
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
    fn walk(expression: &ShowExpression, keys: &std::collections::HashSet<&str>) {
        match expression {
            ShowExpression::Rule { name } => {
                assert!(
                    keys.contains(name.as_str()),
                    "Rule leaf '{name}' missing from Show.rules"
                );
            }
            ShowExpression::And { left, right }
            | ShowExpression::Arithmetic { left, right, .. }
            | ShowExpression::Comparison { left, right, .. }
            | ShowExpression::RangeLiteral {
                from: left,
                to: right,
            }
            | ShowExpression::RangeContainment {
                value: left,
                range: right,
            } => {
                walk(left, keys);
                walk(right, keys);
            }
            ShowExpression::Not { operand }
            | ShowExpression::UnitConversion { operand, .. }
            | ShowExpression::Math { operand, .. }
            | ShowExpression::DateRelative { operand, .. }
            | ShowExpression::DateCalendar { operand, .. }
            | ShowExpression::PastFutureRange { operand, .. }
            | ShowExpression::IsVeto { operand } => walk(operand, keys),
            ShowExpression::Literal(_)
            | ShowExpression::Data { .. }
            | ShowExpression::Veto { .. }
            | ShowExpression::Now => {}
        }
    }
    let keys: std::collections::HashSet<&str> = show.rules.keys().map(String::as_str).collect();
    for (name, rule) in &show.rules {
        for dep in &rule.depends_on_rules {
            assert!(
                keys.contains(dep.as_str()),
                "depends_on_rules entry '{dep}' of '{name}' missing from Show.rules"
            );
        }
        for branch in &rule.branches {
            if let Some(condition) = &branch.condition {
                walk(condition, &keys);
            }
            walk(&branch.result, &keys);
        }
    }
}

#[test]
fn run_rules_accepts_reachable_imported_key() {
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
    let response = engine
        .run(
            None,
            "outer",
            Some(&now),
            HashMap::new(),
            Some(&["i.helper".to_string()]),
            false,
        )
        .expect("run i.helper");
    assert!(response.results.contains_key("i.helper"));
    assert!(!response.results.contains_key("r"));

    let err = engine
        .run(
            None,
            "outer",
            Some(&now),
            HashMap::new(),
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
        .run(None, "outer", Some(&now), HashMap::new(), None, false)
        .expect("run");
    let local_keys: Vec<_> = show
        .rules
        .iter()
        .filter(|(_, rule)| rule.path.is_empty())
        .map(|(name, _)| name.clone())
        .collect();
    let result_keys: Vec<_> = response.results.keys().cloned().collect();
    assert_eq!(result_keys, local_keys);
    assert!(!response.results.contains_key("i.helper"));
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
        .run(None, "outer", Some(&now), HashMap::new(), None, true)
        .expect("run explain");
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
            if child["type"] == "rule" {
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

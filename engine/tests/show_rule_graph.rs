//! Show.rules is ShowRule with branches and depends_on_rules.

use lemma::{Engine, ShowExpression, SourceType};
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

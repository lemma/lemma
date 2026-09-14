//! Show lists all declared promptable data; needed_by_rules marks intake vs reuse.

use lemma::DateTimeValue;
use lemma::Engine;
use std::collections::BTreeSet;

#[test]
fn show_lists_unused_data_with_empty_needed_by_rules() {
    let code = r#"
spec t
data used: 4
data unused: 5
rule r: used
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("load");
    let show = engine
        .show(None, "t", Some(&DateTimeValue::now()))
        .expect("show");

    let keys: BTreeSet<&str> = show.data.keys().map(String::as_str).collect();
    assert_eq!(keys, BTreeSet::from(["used", "unused"]));
    assert_eq!(
        show.data.get("used").expect("used").needed_by_rules,
        vec!["r".to_string()]
    );
    assert!(
        show.data
            .get("unused")
            .expect("unused")
            .needed_by_rules
            .is_empty(),
        "unused must have empty needed_by_rules"
    );
}

#[test]
fn show_includes_dead_unless_data_with_empty_needed_by_rules() {
    let code = r#"
spec t
data used: 4
data dead: 5
rule r: used * dead
 unless true then 1
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("load");
    let show = engine
        .show(None, "t", Some(&DateTimeValue::now()))
        .expect("show");

    let keys: BTreeSet<&str> = show.data.keys().map(String::as_str).collect();
    assert_eq!(keys, BTreeSet::from(["used", "dead"]));
    assert!(
        show.data
            .get("used")
            .expect("used")
            .needed_by_rules
            .is_empty(),
        "used only on dead primary arm; needed_by_rules empty"
    );
    assert!(
        show.data
            .get("dead")
            .expect("dead")
            .needed_by_rules
            .is_empty(),
        "dead only on dead primary arm; needed_by_rules empty"
    );
}

#[test]
fn show_includes_dead_under_unless_inlined_divide_rule() {
    let code = r#"
spec t
data dead: 5
rule dep: 5 / 2
rule r: dead
 unless dep < 4 then 1
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("load");
    let show = engine
        .show(None, "t", Some(&DateTimeValue::now()))
        .expect("show");

    assert!(
        show.data.contains_key("dead"),
        "dead slot still declared for reuse; keys: {:?}",
        show.data.keys().collect::<Vec<_>>()
    );
    assert!(
        show.data
            .get("dead")
            .expect("dead")
            .needed_by_rules
            .contains(&"r".to_string()),
        "dep < 4 does not fold through rule_ref; default arm stays live so dead is needed by r"
    );
}

#[test]
fn show_includes_dead_under_unless_nested_arithmetic() {
    let code = r#"
spec t
data dead: 5
rule r: dead
 unless (1 + 2) * 3 / 4 < 10 then 1
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("load");
    let show = engine
        .show(None, "t", Some(&DateTimeValue::now()))
        .expect("show");

    assert!(
        show.data.contains_key("dead"),
        "dead slot still declared for reuse; keys: {:?}",
        show.data.keys().collect::<Vec<_>>()
    );
    assert!(
        show.data
            .get("dead")
            .expect("dead")
            .needed_by_rules
            .is_empty(),
        "dead not needed by remaining rules"
    );
}

#[test]
fn show_library_spec_with_no_rules_lists_declared_slots() {
    let code = r#"
spec base_types
data currency: text
  -> option "EUR"
  -> option "USD"
data rate: ratio
  -> maximum 100%
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("load");
    let show = engine
        .show(None, "base_types", Some(&DateTimeValue::now()))
        .expect("show");

    let keys: BTreeSet<&str> = show.data.keys().map(String::as_str).collect();
    assert_eq!(keys, BTreeSet::from(["currency", "rate"]));
    assert!(show.rules.is_empty());
    for name in ["currency", "rate"] {
        assert!(
            show.data.get(name).expect(name).needed_by_rules.is_empty(),
            "{name} must have empty needed_by_rules"
        );
    }
}

/// Transitive needed_by_rules through a linear rule-ref chain.
#[test]
fn show_needed_by_rules_transitive_through_chain_embeds() {
    let code = r#"
spec t
data x0: number
rule r1: x0 + 1
rule r2: r1 + 1
rule r3: r2 + 1
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("load");
    let show = engine
        .show(None, "t", Some(&DateTimeValue::now()))
        .expect("show");

    assert_eq!(
        show.data.get("x0").expect("x0").needed_by_rules,
        vec!["r1".to_string(), "r2".to_string(), "r3".to_string()]
    );
}

/// Diamond: tip needs both branches' data via two embeds.
#[test]
fn show_needed_by_rules_transitive_through_diamond_embeds() {
    let code = r#"
spec t
data a: number
data b: number
rule left: a + 1
rule right: b + 1
rule tip: left + right
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("load");
    let show = engine
        .show(None, "t", Some(&DateTimeValue::now()))
        .expect("show");

    assert_eq!(
        show.data.get("a").expect("a").needed_by_rules,
        vec!["left".to_string(), "tip".to_string()]
    );
    assert_eq!(
        show.data.get("b").expect("b").needed_by_rules,
        vec!["right".to_string(), "tip".to_string()]
    );
}

/// Rule-target data via `uses`/`with` lowers to the same `rule_ref` cut as a
/// direct rule ref: consumer's needed set unions the target rule's set.
#[test]
fn show_needed_by_rules_through_rule_target_with_binding() {
    let code = r#"
spec inner
data slot: number

spec source_spec
data x0: number
rule computed: x0 * 2

spec outer
uses i: inner
  -> with slot: src.computed
uses src: source_spec
rule r: i.slot
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("load");

    let show_src = engine
        .show(None, "source_spec", Some(&DateTimeValue::now()))
        .expect("show source");
    assert_eq!(
        show_src.data.get("x0").expect("x0").needed_by_rules,
        vec!["computed".to_string()]
    );

    // Outer plan embeds src.computed via the with-bound i.slot DataPath; show
    // must still succeed (topo memo handles imported embed targets) and union
    // the target rule's needed set onto the consumer.
    let show_outer = engine
        .show(None, "outer", Some(&DateTimeValue::now()))
        .expect("show outer");
    assert_eq!(
        show_outer
            .data
            .get("src.x0")
            .expect("src.x0 must appear in outer show catalog")
            .needed_by_rules,
        vec!["r".to_string()],
        "outer needed_by_rules must union target rule's set through with-bound rule_ref"
    );
}

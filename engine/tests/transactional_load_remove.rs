//! Load/remove must not leave Engine in a half-committed state.

use lemma::{DateTimeValue, Engine, SourceType};
use std::collections::HashMap;
use std::sync::Arc;

#[test]
fn failed_load_leaves_prior_specs_and_plans_intact() {
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Volatile,
            r#"
spec good
data x: 1
rule y: x
"#
            .to_string(),
        )])
        .expect("good load");

    let now = DateTimeValue::now();
    let before = engine
        .run(None, "good", Some(&now), HashMap::new(), None, false)
        .expect("good runs before");

    let bad = engine.load([(
        SourceType::Path(Arc::new(std::path::PathBuf::from("bad.lemma"))),
        r#"
spec consumer
uses @missing/dep some_spec
data x: number
rule y: x + some_spec.value
"#
        .to_string(),
    )]);
    assert!(bad.is_err(), "load with missing dependency must fail");

    let listed = engine.list();
    let workspace = listed
        .iter()
        .find(|r| r.repository.is_none())
        .expect("workspace");
    assert!(
        workspace.specs.iter().any(|s| s.name == "good"),
        "good must remain listed"
    );
    assert!(
        !workspace.specs.iter().any(|s| s.name == "consumer"),
        "failed consumer must not remain in context"
    );

    let after = engine
        .run(None, "good", Some(&now), HashMap::new(), None, false)
        .expect("good still runs after failed load");
    assert_eq!(
        before.results.keys().collect::<Vec<_>>(),
        after.results.keys().collect::<Vec<_>>()
    );

    assert!(
        engine.show(None, "consumer", Some(&now)).is_err(),
        "show must not serve rolled-back consumer"
    );
}

#[test]
fn remove_rolls_back_when_replan_fails() {
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Volatile,
            r#"
spec dep
data value: 10
rule out: value

spec consumer
uses d: dep
rule total: d.value
"#
            .to_string(),
        )])
        .expect("load dep + consumer");

    let now = DateTimeValue::now();
    engine
        .run(None, "consumer", Some(&now), HashMap::new(), None, false)
        .expect("consumer runs before remove");

    let remove_result = engine.remove(None, "dep", Some(&now));
    assert!(
        remove_result.is_err(),
        "removing dep must fail while consumer still uses it"
    );

    let listed = engine.list();
    let workspace = listed
        .iter()
        .find(|r| r.repository.is_none())
        .expect("workspace");
    assert!(
        workspace.specs.iter().any(|s| s.name == "dep"),
        "dep must be restored after failed remove"
    );

    engine
        .show(None, "dep", Some(&now))
        .expect("dep plan still served");
    engine
        .run(None, "consumer", Some(&now), HashMap::new(), None, false)
        .expect("consumer still runs after failed remove");
}

#[test]
fn update_replaces_spec_with_dependents() {
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Volatile,
            r#"
spec dep
data value: 10
rule out: value

spec consumer
uses d: dep
rule total: d.value
"#
            .to_string(),
        )])
        .expect("load dep + consumer");

    let now = DateTimeValue::now();
    engine
        .update(
            None,
            r#"
spec dep
data value: 20
rule out: value
"#
            .to_string(),
            SourceType::Volatile,
        )
        .expect("update dep while consumer depends on it");

    let response = engine
        .run(None, "consumer", Some(&now), HashMap::new(), None, false)
        .expect("consumer runs after update");
    let total = response
        .results
        .get("total")
        .expect("total rule")
        .result()
        .expect("total display");
    assert_eq!(total, "20", "consumer must see updated dep value");
}

#[test]
fn update_rolls_back_when_new_source_invalid() {
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Volatile,
            r#"
spec dep
data value: 10
rule out: value

spec consumer
uses d: dep
rule total: d.value
"#
            .to_string(),
        )])
        .expect("load dep + consumer");

    let now = DateTimeValue::now();
    let before = engine
        .run(None, "consumer", Some(&now), HashMap::new(), None, false)
        .expect("consumer runs before");

    let bad = engine.update(
        None,
        r#"
spec dep
uses missing: nonexistent_spec
data value: 99
rule out: value + missing.x
"#
        .to_string(),
        SourceType::Volatile,
    );
    assert!(bad.is_err(), "update with broken deps must fail");

    let listed = engine.list();
    let workspace = listed
        .iter()
        .find(|r| r.repository.is_none())
        .expect("workspace");
    assert!(
        workspace.specs.iter().any(|s| s.name == "dep"),
        "dep must remain after failed update"
    );

    let after = engine
        .run(None, "consumer", Some(&now), HashMap::new(), None, false)
        .expect("consumer still runs after failed update");
    assert_eq!(
        before.results.get("total").and_then(|r| r.result()),
        after.results.get("total").and_then(|r| r.result()),
        "old dep value must be restored"
    );
}

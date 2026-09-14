//! Phase-4 rule-target constraint merge must be visible to phase-5 type checks
//! (inference cache cleared between those phases).
//!
//! Positive case: with-binding supplies a rule result into typed data; run must
//! see the merged type. Negative case: incompatible binding must Error at plan
//! time — proves phase 5 consults the post-merge type, not a stale undetermined
//! cache entry from phase 3.

use lemma::{DateTimeValue, Engine, SourceType};
use std::collections::HashMap;

#[test]
fn rule_target_via_with_binding_plans_after_phase4_cache_clear() {
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Volatile,
            r#"
spec inner
data slot: number

spec source_spec
data v: 5
rule computed: v * 2

spec outer
uses i: inner
  -> with slot: src.computed
uses src: source_spec
rule r: i.slot
"#
            .to_string(),
        )])
        .expect("rule-target with-binding must plan with cache cleared after phase 4");

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "outer", Some(&now), HashMap::new(), None, false)
        .expect("run");
    let r = response.results.get("r").expect("r");
    assert!(!r.vetoed);
    assert_eq!(
        r.value.as_ref().and_then(|v| v.display.as_deref()),
        Some("10")
    );

    let show = engine.show(None, "outer", Some(&now)).expect("show");
    assert_eq!(show.rules.get("r").expect("r").lemma_type.name(), "slot");
}

#[test]
fn rule_target_with_binding_type_mismatch_errors_after_phase4_merge() {
    let mut engine = Engine::new();
    let err = engine
        .load([(
            SourceType::Volatile,
            r#"
spec inner
data slot: number

spec source_spec
rule computed: "not-a-number"

spec outer
uses i: inner
  -> with slot: src.computed
uses src: source_spec
rule r: i.slot
"#
            .to_string(),
        )])
        .expect_err("text rule bound into number data must fail planning");
    assert!(
        !err.errors.is_empty(),
        "expected type/validation errors, got empty Errors"
    );
}

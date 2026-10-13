//! Parity targets for WasmEngine-style loading: dependency-tagged [`Engine::load`] bundles
//! tag every repository parsed from that batch as that dependency (see `Engine::add_sources_inner`).
//! Browser stacks often load sources that way; plain CLI `load` keeps workspace `name: None`.
//!
//! These tests exercise `show` at several effective instants (mirrors
//! `WasmEngine::show` defaulting effective to [`DateTimeValue::now`]) and cross-load patterns
//! that stress type resolution after a dependency batch.

use lemma::{DateGranularity, DateTimeValue, Engine, SourceType};
use std::sync::Arc;

fn path_source(path: &str) -> SourceType {
    SourceType::Path(Arc::new(std::path::PathBuf::from(path)))
}

fn wasm_style_instants() -> [DateTimeValue; 4] {
    [
        DateTimeValue::now(),
        DateTimeValue {
            year: 2026,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            microsecond: 0,
            timezone: None,

            granularity: DateGranularity::Full,
        },
        DateTimeValue {
            year: 2024,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            microsecond: 0,
            timezone: None,

            granularity: DateGranularity::Full,
        },
        DateTimeValue {
            year: 2020,
            month: 6,
            day: 15,
            hour: 12,
            minute: 0,
            second: 0,
            microsecond: 0,
            timezone: None,

            granularity: DateGranularity::DateTime,
        },
    ]
}

#[test]
fn dependency_batch_show_via_repo_qualifier() {
    let mut engine = Engine::new();
    let bundle = r#"
spec alpha2
data code: text
  -> option "NL"

spec cashier
uses C: alpha2
  -> with code: "NL"
rule country: C.code
"#;
    engine
        .load([(
            SourceType::Dependency("@iso/countries".to_string()),
            bundle.to_string(),
        )])
        .expect("dependency-tag load should parse and plan");

    for instant in wasm_style_instants() {
        engine
            .show(Some("@iso/countries"), "cashier", Some(&instant))
            .expect("Engine::show same path as WasmEngine.show");
        engine
            .show(Some("@iso/countries"), "alpha2", Some(&instant))
            .expect("alpha2 exposes show inside dependency-qualified repository");
    }
}

#[test]
fn workspace_consumer_after_dependency_batch_resolves_country_type() {
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Dependency("@iso/countries".to_string()),
            r#"spec alpha2
data code: text
  -> option "NL"
  -> option "BE"
"#
            .to_string(),
        )])
        .expect("registry dependency bundle loads under @iso/countries");

    engine
        .load([(
            path_source("kiosk.lemma"),
            r#"spec kiosk
uses @iso/countries alpha2
data country: alpha2.code
rule tally: country
"#
            .to_string(),
        )])
        .expect("workspace kiosk uses registry-style qualifier line like examples/12_registry_references");

    for instant in wasm_style_instants() {
        engine
            .show(None, "kiosk", Some(&instant))
            .expect("kiosk should plan across instants once dependency types resolve");
    }
}

#[test]
fn duplicate_named_finance_across_named_repositories_plan_without_panic() {
    let mut engine = Engine::new();
    engine
        .load([(
            path_source("a.lemma"),
            r#"repo tier_a

spec finance
data x: number
rule gross: x
"#
            .to_string(),
        )])
        .expect("tier_a repository loads");

    engine
        .load([(
            SourceType::Dependency("tier_b_dep".to_string()),
            r#"repo tier_b

spec finance
data y: number
rule gross: y
"#
            .to_string(),
        )])
        .expect("tier_b dependency bundle loads with duplicate bare spec name `finance`");

    let counts: Vec<(Option<String>, usize)> = engine
        .specs()
        .iter()
        .map(|entry| (entry.repository.clone(), entry.specs.len()))
        .collect();

    assert!(
        counts
            .iter()
            .any(|(name, _)| name.as_deref() == Some("tier_a")),
        "expected tier_a repository in {:?}",
        counts
    );
    assert!(
        counts
            .iter()
            .any(|(name, _)| name.as_deref() == Some("tier_b")),
        "expected tier_b repository in {:?}",
        counts
    );

    let now = DateTimeValue::now();
    engine
        .show(Some("tier_a"), "finance", Some(&now))
        .expect("show tier_a finance");
    engine
        .show(Some("tier_b"), "finance", Some(&now))
        .expect("show tier_b finance");
}

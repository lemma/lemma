//! Integration tests for temporal range and spec references.
//!
//! Normative source: [Composing specs](../../cli/documentation/learn/composing_specs.md).

use lemma::{DateGranularity, DateTimeValue, Engine, SourceType};
use std::collections::HashMap;

fn date(year: i32, month: u32, day: u32) -> DateTimeValue {
    DateTimeValue {
        year,
        month,
        day,
        hour: 0,
        minute: 0,
        second: 0,
        microsecond: 0,
        timezone: None,
        granularity: DateGranularity::Full,
    }
}

fn eval(engine: &Engine, spec_name: &str, effective: &DateTimeValue) -> lemma::Response {
    engine
        .run(
            None,
            spec_name,
            Some(effective),
            HashMap::new(),
            None,
            false,
        )
        .unwrap()
}

fn assert_rule_value(response: &lemma::Response, rule: &str, expected: &str) {
    let result = response
        .results
        .get(rule)
        .unwrap_or_else(|| panic!("rule '{}' not in results", rule));
    if result.vetoed {
        panic!(
            "rule '{}' is Veto: {}",
            rule,
            result.veto_reason.as_deref().unwrap_or("Vetoed")
        );
    }
    assert_eq!(
        result.result(),
        Some(expected),
        "rule '{}': expected {}, got {:?}",
        rule,
        expected,
        result.result()
    );
}

// --- Case D (control): unqualified ref requires full consumer temporal range coverage ---

#[test]
fn unqualified_dep_must_cover_consumer_temporal_range_gap_errors() {
    let mut engine = Engine::new();
    let err = engine
        .load([(
            SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("t.lemma"))),
            r#"
spec consumer 2025-01-01
uses d: dep
rule out: d.v

spec dep 2025-08-01
rule v: 42
"#
            .to_string(),
        )])
        .expect_err("unqualified dep with coverage gap must fail planning");

    let joined = err
        .errors
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        joined.contains("consumer") && joined.contains("dep"),
        "expected coverage error naming specs; got: {joined}"
    );
    assert!(
        joined.contains("no version") || joined.contains("active"),
        "expected temporal coverage wording; got: {joined}"
    );
}

// --- Case C: qualified ref must not require full-range coverage of dep ---

#[test]
fn qualified_dep_allows_consumer_starting_before_dep_exists() {
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("t.lemma"))),
            r#"
spec consumer 2025-01-01
uses d: dep 2025-08-01
rule out: d.v

spec dep 2025-08-01
rule v: 42
"#
            .to_string(),
        )])
        .expect("qualified dep at T should not require dep to cover entire consumer range");

    assert_rule_value(&eval(&engine, "consumer", &date(2025, 3, 1)), "out", "42");
}

// --- Case A: qualified ref — nested unqualified subtree at T, not slice.from ---

#[test]
fn qualified_dep_nested_unqualified_child_resolves_at_qualifier_instant() {
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("t.lemma"))),
            r#"
spec consumer 2025-01-01
uses d: dep 2025-10-01
rule out: d.nested_val

spec dep 2025-01-01
uses nested: child
rule nested_val: nested.x

spec dep 2025-12-01
uses nested: child
rule nested_val: nested.x

spec child 2025-01-01
rule x: 1

spec child 2025-06-01
rule x: 2
"#
            .to_string(),
        )])
        .unwrap();

    // Consumer slice start is 2025-01-01; nested `child` must use T=2025-10-01 → x=2.
    assert_rule_value(&eval(&engine, "consumer", &date(2025, 3, 1)), "out", "2");
}

#[test]
fn qualified_dep_nested_resolution_independent_of_run_effective() {
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("t.lemma"))),
            r#"
spec consumer 2025-01-01
uses d: dep 2025-10-01
rule out: d.nested_val

spec dep 2025-01-01
uses nested: child
rule nested_val: nested.x

spec dep 2025-12-01
uses nested: child
rule nested_val: nested.x

spec child 2025-01-01
rule x: 1

spec child 2025-06-01
rule x: 2
"#
            .to_string(),
        )])
        .unwrap();

    assert_rule_value(&eval(&engine, "consumer", &date(2025, 11, 1)), "out", "2");
}

// --- Case B: qualified-only dep edge must not split consumer on dep version boundaries ---

#[test]
fn qualified_only_dep_reference_does_not_split_consumer_temporal_slices() {
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("t.lemma"))),
            r#"
spec consumer 2025-01-01
uses d: dep 2025-06-15
rule out: d.v

spec dep 2025-03-01
rule v: 10

spec dep 2025-09-01
rule v: 20
"#
            .to_string(),
        )])
        .unwrap();

    assert_rule_value(&eval(&engine, "consumer", &date(2025, 2, 1)), "out", "10");
}

// --- Case E: mixed unqualified + qualified deps — only unqualified drives slice boundaries ---

#[test]
fn mixed_unqualified_and_qualified_deps_slice_count_from_unqualified_only() {
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("t.lemma"))),
            r#"
spec consumer 2025-01-01
uses a: dep_a
uses b: dep_b 2025-04-01
rule out_a: a.v
rule out_b: b.v

spec dep_a 2025-01-01
rule v: 1

spec dep_a 2025-06-01
rule v: 2

spec dep_b 2025-01-01
rule v: 10

spec dep_b 2025-08-01
rule v: 20
"#
            .to_string(),
        )])
        .unwrap();

    // Slice 1: [2025-01-01, 2025-06-01) — dep_a Jan, dep_b pinned at T=2025-04-01 → Jan
    let r1 = eval(&engine, "consumer", &date(2025, 3, 1));
    assert_rule_value(&r1, "out_a", "1");
    assert_rule_value(&r1, "out_b", "10");

    // Slice 2: [2025-06-01, +inf) — dep_a June, dep_b still pinned at T=2025-04-01 → Jan
    let r2 = eval(&engine, "consumer", &date(2025, 9, 1));
    assert_rule_value(&r2, "out_a", "2");
    assert_rule_value(&r2, "out_b", "10");
}

// --- Qualified parents: same subtree instant as data-level qualified ref (section 2.1) ---

#[test]
fn qualified_dep_data_import_from_child_uses_qualifier_not_slice_start() {
    let mut engine = Engine::new();
    engine
        .load([(SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("t.lemma"))), r#"
spec consumer 2025-01-01
uses d: dep 2025-10-01
rule out: d.doubled

spec dep 2025-01-01
uses c: child 2025-06-01
data money: c.money
data p: 5 usd
rule doubled: p * 2

spec child 2025-01-01
data money: measure
 -> unit eur: 1.00
 -> decimals 2

spec child 2025-06-01
data money: measure
 -> unit eur: 1.00
 -> unit usd: 0.91
 -> decimals 2
"#.to_string())])
        .expect("qualified parent `c.money` under `uses c: child …` must resolve child at qualifier instant so `usd` exists");

    assert_rule_value(
        &eval(&engine, "consumer", &date(2025, 3, 1)),
        "out",
        "10.00 usd",
    );
}

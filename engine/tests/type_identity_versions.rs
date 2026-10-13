//! Type identity must include the declaring spec version.
//!
//! Two pins to different versions of a measure type are different families.
//! Mixing their values is a planning Error. Same version via two pins shares
//! one family. Unpinned interface drift ignores version in the family key.

use indexmap::IndexMap;
use lemma::{DateGranularity, DateTimeValue, Engine, EvaluateDataValue, SourceType};
use std::path::PathBuf;
use std::sync::Arc;

fn path_source(file: &str) -> SourceType {
    SourceType::Path(Arc::new(PathBuf::from(file)))
}

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

fn expect_plan_error(code: &str) -> String {
    let mut engine = Engine::new();
    match engine.load([(path_source("bad.lemma"), code.to_string())]) {
        Err(err) => err
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; "),
        Ok(()) => panic!("expected planning error, but load succeeded"),
    }
}

fn load_ok(code: &str) -> Engine {
    let mut engine = Engine::new();
    engine
        .load([(path_source("ok.lemma"), code.to_string())])
        .unwrap_or_else(|errs| panic!("load: {errs:?}"));
    engine
}

fn eval_at(
    engine: &Engine,
    spec: &str,
    effective: &DateTimeValue,
    data: Vec<(&str, &str)>,
) -> lemma::Response {
    let map: IndexMap<String, EvaluateDataValue> = data
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.into()))
        .collect();
    engine
        .evaluate(None, spec, Some(effective), map, None, false)
        .expect("evaluate")
}

fn assert_rule(response: &lemma::Response, rule: &str, expected: &str) {
    let result = response
        .results
        .get(rule)
        .unwrap_or_else(|| panic!("rule '{rule}' missing"));
    assert!(
        !result.vetoed,
        "rule '{rule}' vetoed: {:?}",
        result.veto_reason
    );
    assert_eq!(result.result(), Some(expected));
}

/// Different base units across pinned versions: mixing is a planning Error.
#[test]
fn pinned_versions_different_base_units_cannot_add() {
    let msg = expect_plan_error(
        r#"
spec pricing 2024-01-01
data money: measure
  -> unit eur: 1

spec pricing 2025-01-01
data money: measure
  -> unit usd: 1

spec report
uses a: pricing 2024
uses b: pricing 2025
data x: a.money
data y: b.money
rule total: x + y
"#,
    );
    assert!(
        msg.contains("unrelated") || msg.contains("incompatible") || msg.contains("different"),
        "expected versioned family mismatch, got: {msg}"
    );
}

#[test]
fn pinned_versions_different_base_units_cannot_compare() {
    let msg = expect_plan_error(
        r#"
spec pricing 2024-01-01
data money: measure
  -> unit eur: 1

spec pricing 2025-01-01
data money: measure
  -> unit usd: 1

spec report
uses a: pricing 2024
uses b: pricing 2025
data x: a.money
data y: b.money
rule bigger: x > y
"#,
    );
    assert!(
        msg.contains("unrelated") || msg.contains("incompatible") || msg.contains("different"),
        "expected versioned family mismatch, got: {msg}"
    );
}

#[test]
fn pinned_versions_cannot_convert_across_families() {
    let msg = expect_plan_error(
        r#"
spec pricing 2024-01-01
data money: measure
  -> unit eur: 1

spec pricing 2025-01-01
data money: measure
  -> unit usd: 1

spec report
uses a: pricing 2024
uses b: pricing 2025
data x: a.money
rule in_usd: x as usd
"#,
    );
    assert!(
        msg.contains("usd")
            || msg.contains("2024")
            || msg.contains("Unknown")
            || msg.contains("different"),
        "expected conversion across versions rejected, got: {msg}"
    );
}

/// Two pins that land on the same version share one family.
#[test]
fn same_version_via_two_pins_shares_family() {
    let engine = load_ok(
        r#"
spec pricing 2024-01-01
data money: measure
  -> unit eur: 1
  -> decimals 2

spec report
uses a: pricing 2024-03-01
uses b: pricing 2024-06-01
data x: a.money
data y: b.money
rule total: x + y
"#,
    );
    assert_rule(
        &eval_at(
            &engine,
            "report",
            &date(2024, 7, 1),
            vec![("x", "10 eur"), ("y", "5 eur")],
        ),
        "total",
        "15.00 eur",
    );
}

/// Unpinned consumer spanning two identical versions: no false interface drift.
#[test]
fn unpinned_identical_versions_no_false_drift() {
    let engine = load_ok(
        r#"
spec pricing 2024-01-01
data money: measure
  -> unit eur: 1
  -> decimals 2

spec pricing 2025-01-01
data money: measure
  -> unit eur: 1
  -> decimals 2

spec report 2024-01-01
uses p: pricing
data x: p.money
rule doubled: x * 2
"#,
    );
    assert_rule(
        &eval_at(&engine, "report", &date(2024, 6, 1), vec![("x", "10 eur")]),
        "doubled",
        "20.00 eur",
    );
    assert_rule(
        &eval_at(&engine, "report", &date(2025, 6, 1), vec![("x", "10 eur")]),
        "doubled",
        "20.00 eur",
    );
}

/// Unpinned consumer spanning versions that change a unit: interface drift Error.
#[test]
fn unpinned_changed_unit_is_interface_drift() {
    let msg = expect_plan_error(
        r#"
spec pricing 2024-01-01
data money: measure
  -> unit eur: 1

spec pricing 2025-01-01
data money: measure
  -> unit usd: 1

spec report 2024-01-01
uses p: pricing
data x: p.money
rule doubled: x * 2
"#,
    );
    assert!(
        msg.contains("changed") || msg.contains("interface") || msg.contains("money"),
        "expected interface drift, got: {msg}"
    );
}

/// Local mass must not steal units from an imported type named mass.
#[test]
fn local_mass_extension_does_not_claim_imported_kilogram() {
    let engine = load_ok(
        r#"
spec std
data mass: measure
  -> unit kilogram: 1
  -> unit gram: 0.001

spec bag
uses s: std
data mass: measure
  -> unit kg: 1
data bag_weight: mass
data std_weight: s.mass
rule bag_total: bag_weight + 1 kg
rule std_total: std_weight + 1 kilogram
"#,
    );
    assert_rule(
        &eval_at(
            &engine,
            "bag",
            &date(2024, 1, 1),
            vec![("bag_weight", "2 kg"), ("std_weight", "3 kilogram")],
        ),
        "bag_total",
        "3 kg",
    );
    assert_rule(
        &eval_at(
            &engine,
            "bag",
            &date(2024, 1, 1),
            vec![("bag_weight", "2 kg"), ("std_weight", "3 kilogram")],
        ),
        "std_total",
        "4 kilogram",
    );
}

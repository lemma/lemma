//! Dynamic measure unit scales: `-> unit <name>: <ref>` references a `number`
//! data field or rule as the unit's scale factor. Values written in such a unit
//! are stored unscaled and settled inside the dependency-ordered walk.

use indexmap::IndexMap;
use lemma::{DateTimeValue, Engine, EvaluateDataValue, RuleResult, SourceType};
use rust_decimal::Decimal;
use std::collections::BTreeMap;

fn load(engine: &mut Engine, code: &str) {
    engine
        .load([(SourceType::Volatile, code.to_string())])
        .expect("spec must load");
}

fn load_err(engine: &mut Engine, code: &str) -> String {
    engine
        .load([(SourceType::Volatile, code.to_string())])
        .expect_err("spec must fail to load")
        .errors
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("; ")
}

fn run(
    engine: &Engine,
    spec: &str,
    data: IndexMap<String, EvaluateDataValue>,
    explain: bool,
) -> lemma::Response {
    run_in_repo(engine, None, spec, data, explain)
}

fn run_in_repo(
    engine: &Engine,
    repository: Option<&str>,
    spec: &str,
    data: IndexMap<String, EvaluateDataValue>,
    explain: bool,
) -> lemma::Response {
    let now = DateTimeValue::now();
    engine
        .evaluate(repository, spec, Some(&now), data, None, explain)
        .expect("evaluation must succeed")
}

fn rule<'a>(response: &'a lemma::Response, name: &str) -> &'a RuleResult {
    response.results.get(name).unwrap_or_else(|| {
        panic!(
            "rule '{name}' missing from results: {:?}",
            response.results.keys().collect::<Vec<_>>()
        )
    })
}

fn value_of<'a>(response: &'a lemma::Response, name: &str) -> &'a lemma::RuleResultValue {
    rule(response, name).result.as_ref().expect("value")
}

fn measure_map<'a>(response: &'a lemma::Response, name: &str) -> &'a BTreeMap<String, Decimal> {
    value_of(response, name)
        .measure
        .as_ref()
        .expect("measure map")
}

fn data(pairs: &[(&str, &str)]) -> IndexMap<String, EvaluateDataValue> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), EvaluateDataValue::string(*v)))
        .collect()
}

/// Unbound dynamic scale: missing data naming `scale_key`, no successful result.
fn assert_missing_scale(response: &lemma::Response, rule_name: &str, scale_key: &str) {
    let r = rule(response, rule_name);
    assert!(
        r.vetoed,
        "{rule_name}: unbound {scale_key} must veto, got value {:?}",
        r.result
    );
    assert!(
        r.is_missing_data(),
        "{rule_name}: unbound {scale_key} must be missing data, got {:?}",
        r.veto_reason
    );
    assert!(
        r.missing_data().iter().any(|k| k.contains(scale_key)),
        "{rule_name}: missing_data must name {scale_key}, got {:?}",
        r.missing_data()
    );
    assert!(
        r.result.is_none(),
        "{rule_name}: unbound {scale_key} must not yield a value, got {:?}",
        r.result
    );
}

// =============================================================================
// Planning errors
// =============================================================================

#[test]
fn scale_target_must_be_number() {
    let mut engine = Engine::new();
    let code = r#"
spec t
data flag: boolean
data money: measure
    -> unit eur: 1
    -> unit gbp: flag
"#;
    let msg = load_err(&mut engine, code);
    assert!(
        msg.to_lowercase().contains("number"),
        "error must say target must be number, got: {msg}"
    );
}

#[test]
fn uses_qualified_scale_target_must_be_number() {
    let mut engine = Engine::new();
    let code = r#"
repo r

spec rate_src
data rate: boolean

spec consumer
uses rs: rate_src
data money: measure
    -> unit eur: 1
    -> unit gbp: rs.rate
"#;
    let msg = load_err(&mut engine, code);
    assert!(
        msg.to_lowercase().contains("number"),
        "uses-qualified scale target must be rejected when not a number, got: {msg}"
    );
}

#[test]
fn bare_name_ambiguous_as_data_and_unit_is_planning_error() {
    let mut engine = Engine::new();
    let code = r#"
spec t
data meter: number
data length: measure
    -> unit meter: 1
data money: measure
    -> unit eur: 1
    -> unit gbp: meter
"#;
    let msg = load_err(&mut engine, code);
    assert!(
        msg.to_lowercase().contains("ambiguous"),
        "local data that also names a unit path must be rejected as ambiguous, got: {msg}"
    );
}

#[test]
fn unknown_bare_name_keeps_single_unit_diagnostic() {
    let mut engine = Engine::new();
    let code = r#"
spec t
data money: measure
    -> unit eur: 1
    -> unit gbp: no_such_thing
"#;
    let msg = load_err(&mut engine, code);
    assert!(
        msg.contains("gbp") && msg.contains("Unknown unit 'no_such_thing'"),
        "error must name gbp and the unknown factor no_such_thing, got: {msg}"
    );
}

#[test]
fn amounts_money_with_dynamic_units_and_minimum_loads() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec amounts
data usd_rate: number
data gbp_rate: number
data money: measure
    -> unit eur: 1.00
    -> unit usd: usd_rate
    -> unit gbp: gbp_rate
    -> minimum 0 eur
"#,
    );
}

#[test]
fn amounts_price_per_mass_with_dynamic_compound_units_loads() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec amounts
uses lemma units
data usd_rate: number
data gbp_rate: number
data money: measure
    -> unit eur: 1.00
    -> unit usd: usd_rate
    -> unit gbp: gbp_rate
data price_per_mass: measure
    -> unit eur_per_kg: eur/kilogram
    -> unit gbp_per_kg: gbp/kilogram
    -> unit milli_gbp_per_kg: 0.001 gbp/kilogram
    -> minimum 0 eur_per_kg
"#,
    );
}

#[test]
fn dynamic_unit_in_compound_unit_loads() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec t
uses lemma units
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
data price: measure
    -> unit eur_per_kg: eur/kilogram
    -> unit gbp_per_kg: gbp/kilogram
"#,
    );
}

#[test]
fn dynamic_unit_squared_compound_loads() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec t
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
data money_squared: measure
    -> unit gbp_squared: gbp * gbp
"#,
    );
}

#[test]
fn minimum_in_dynamic_unit_loads() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec t
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
    -> minimum 0 gbp
"#,
    );
}

#[test]
fn maximum_in_dynamic_unit_loads() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec t
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
    -> maximum 100 gbp
"#,
    );
}

#[test]
fn suggest_in_dynamic_unit_loads() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec t
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
    -> suggest 0 gbp
"#,
    );
}

#[test]
fn option_in_dynamic_unit_loads() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec t
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
    -> option 10 gbp
"#,
    );
}

#[test]
fn option_in_dynamic_unit_on_data_slot_loads() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec t
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
data amount: money
    -> option 10 gbp
"#,
    );
}

#[test]
fn option_in_dynamic_unit_accepts_matching_gbp() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec fx
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
    -> option 10 gbp
data amount: money
rule out: amount
"#,
    );
    let ok = run(
        &engine,
        "fx",
        data(&[("rate", "0.86"), ("amount", "10 gbp")]),
        false,
    );
    assert!(!rule(&ok, "out").vetoed, "10 gbp must match option 10 gbp");
    let bad = run(
        &engine,
        "fx",
        data(&[("rate", "0.86"), ("amount", "5 gbp")]),
        false,
    );
    let bad_rule = rule(&bad, "out");
    assert!(bad_rule.vetoed, "5 gbp must not match option 10 gbp");
    let reason = bad_rule.veto_reason.as_deref().expect("veto reason");
    assert!(
        reason.contains("not one of the allowed options"),
        "expected options veto, got: {reason}"
    );
}

#[test]
fn option_in_dynamic_unit_accepts_canonical_eur_equivalent() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec fx
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
    -> option 10 gbp
data amount: money
rule out: amount
"#,
    );
    let response = run(
        &engine,
        "fx",
        data(&[("rate", "0.86"), ("amount", "8.6 eur")]),
        false,
    );
    assert!(
        !rule(&response, "out").vetoed,
        "8.6 eur must match option 10 gbp when rate is 0.86"
    );
}

#[test]
fn option_in_dynamic_unit_unbound_rate_is_missing_data() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec fx
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
    -> option 10 gbp
data amount: money
rule out: amount
"#,
    );
    let response = run(&engine, "fx", data(&[("amount", "10 gbp")]), false);
    let r = rule(&response, "out");
    assert!(r.is_missing_data(), "unbound rate must be missing data");
    assert!(
        r.missing_data().iter().any(|k| k.contains("rate")),
        "missing_data must name rate, got {:?}",
        r.missing_data()
    );
}

#[test]
fn child_cannot_change_inherited_scale() {
    let mut engine = Engine::new();
    let code = r#"
spec t
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
data child: money
    -> unit gbp: 0.9
"#;
    let msg = load_err(&mut engine, code);
    assert!(
        msg.contains("gbp") && (msg.contains("already defined") || msg.contains("inherited")),
        "child overriding an inherited scale must name gbp, got: {msg}"
    );
}

#[test]
fn child_identical_scale_redeclare_is_allowed() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec t
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
data child: money
    -> unit gbp: rate
rule total: 100 gbp
"#,
    );
    let response = run(&engine, "t", data(&[("rate", "0.86")]), false);
    assert_eq!(
        measure_map(&response, "total")
            .get("eur")
            .expect("eur")
            .to_string(),
        "86"
    );
}

#[test]
fn cycle_through_local_rule_scale_is_planning_error() {
    let mut engine = Engine::new();
    let code = r#"
spec t
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
rule rate: 1 gbp as eur as number
rule total: 100 gbp
"#;
    let msg = load_err(&mut engine, code);
    assert!(
        msg == "In spec 't': Validation error: Circular dependency (rules: rate) at volatile:6:1",
        "cycle through a local rule scale must be a planning error, got: {msg}"
    );
}

#[test]
fn ratio_unit_rejects_reference_scale() {
    let mut engine = Engine::new();
    let code = r#"
spec t
data rate: number
data pct: ratio
    -> unit percent: rate
"#;
    let msg = load_err(&mut engine, code);
    assert_eq!(
        msg,
        "In spec 't': Validation error: Failed to apply constraint 'unit': unit requires a unit name followed by a numeric conversion factor (e.g., '-> unit percent: 100'). Compound unit expressions are not supported for ratio types. at volatile:4:1"
    );
}

// =============================================================================
// Evaluation: literals and defaults
// =============================================================================

#[test]
fn rule_literal_in_dynamic_unit_scales() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec fx
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
rule total: 100 gbp
"#,
    );
    let response = run(&engine, "fx", data(&[("rate", "0.86")]), false);
    // 100 gbp * 0.86 = 86 eur canonical.
    let eur = measure_map(&response, "total")
        .get("eur")
        .expect("eur column");
    assert_eq!(eur.to_string(), "86");
}

#[test]
fn convert_between_literal_and_dynamic_compound_units() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec amounts
uses lemma units
data gbp_rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: gbp_rate
data price_per_mass: measure
    -> unit eur_per_kg: eur/kilogram
    -> unit gbp_per_kg: gbp/kilogram
rule in_gbp: 2 eur_per_kg as gbp_per_kg
rule in_eur: 1 gbp_per_kg as eur_per_kg
"#,
    );
    let response = run(&engine, "amounts", data(&[("gbp_rate", "0.86")]), false);
    let gbp = measure_map(&response, "in_gbp")
        .get("gbp_per_kg")
        .expect("gbp_per_kg");
    assert_eq!(gbp.to_string(), "2.325581395348837209302325581");
    let eur = measure_map(&response, "in_eur")
        .get("eur_per_kg")
        .expect("eur_per_kg");
    assert_eq!(eur.to_string(), "0.86");
}

#[test]
fn milliscale_dynamic_compound_cancels_rate() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec amounts
uses lemma units
data gbp_rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: gbp_rate
data price_per_mass: measure
    -> unit eur_per_kg: eur/kilogram
    -> unit gbp_per_kg: gbp/kilogram
    -> unit milli_gbp_per_kg: 0.001 gbp/kilogram
rule tiny: 1000 milli_gbp_per_kg as gbp_per_kg
"#,
    );
    let response = run(&engine, "amounts", data(&[("gbp_rate", "0.86")]), false);
    let gbp = measure_map(&response, "tiny")
        .get("gbp_per_kg")
        .expect("gbp_per_kg");
    assert_eq!(gbp.to_string(), "1");
}

#[test]
fn unbound_scale_on_dynamic_compound_is_missing_data() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec amounts
uses lemma units
data gbp_rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: gbp_rate
data price_per_mass: measure
    -> unit eur_per_kg: eur/kilogram
    -> unit gbp_per_kg: gbp/kilogram
rule x: 1 gbp_per_kg
"#,
    );
    let response = run(&engine, "amounts", IndexMap::new(), false);
    assert_missing_scale(&response, "x", "gbp_rate");
}

#[test]
fn zero_scale_on_dynamic_compound_vetoes() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec amounts
uses lemma units
data gbp_rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: gbp_rate
data price_per_mass: measure
    -> unit eur_per_kg: eur/kilogram
    -> unit gbp_per_kg: gbp/kilogram
rule x: 1 gbp_per_kg
"#,
    );
    let response = run(&engine, "amounts", data(&[("gbp_rate", "0")]), false);
    let r = rule(&response, "x");
    assert!(r.vetoed, "zero scale must veto");
    assert!(!r.is_missing_data(), "zero scale is a computation veto");
}

#[test]
fn minimum_on_literal_sibling_enforces_with_dynamic_compounds() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec amounts
uses lemma units
data gbp_rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: gbp_rate
data price_per_mass: measure
    -> unit eur_per_kg: eur/kilogram
    -> unit gbp_per_kg: gbp/kilogram
    -> minimum 1.20 eur_per_kg
data price: price_per_mass
rule out: price
"#,
    );
    let response = run(
        &engine,
        "amounts",
        data(&[("gbp_rate", "0.86"), ("price", "1 eur_per_kg")]),
        false,
    );
    let r = rule(&response, "out");
    assert!(r.vetoed, "below minimum must veto");
    let reason = r.veto_reason.as_deref().expect("veto reason");
    assert!(
        reason.contains("below the minimum"),
        "expected below the minimum, got: {reason}"
    );
}

#[test]
fn minimum_in_dynamic_unit_enforces_once_rate_settled() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec fx
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
    -> minimum 10 gbp
data amount: money
rule out: amount
"#,
    );
    let below = run(
        &engine,
        "fx",
        data(&[("rate", "0.86"), ("amount", "5 gbp")]),
        false,
    );
    let below_rule = rule(&below, "out");
    assert!(below_rule.vetoed, "5 gbp below minimum 10 gbp must veto");
    let ok = run(
        &engine,
        "fx",
        data(&[("rate", "0.86"), ("amount", "10 gbp")]),
        false,
    );
    assert!(!rule(&ok, "out").vetoed, "10 gbp at minimum must succeed");
}

#[test]
fn range_literal_in_dynamic_unit_scales() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec fx
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
rule band: 10 gbp ... 100 gbp
"#,
    );
    let response = run(&engine, "fx", data(&[("rate", "0.86")]), false);
    let band = value_of(&response, "band");
    let range = band.range.as_ref().expect("range");
    let left_eur = range
        .from
        .measure
        .as_ref()
        .and_then(|m| m.get("eur"))
        .expect("from eur");
    let right_eur = range
        .to
        .measure
        .as_ref()
        .and_then(|m| m.get("eur"))
        .expect("to eur");
    assert_eq!(left_eur.to_string(), "8.6");
    assert_eq!(right_eur.to_string(), "86");
}

#[test]
fn spec_default_in_dynamic_unit_scales() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec fx
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
data amount: 50 gbp
rule total: amount
"#,
    );
    let response = run(&engine, "fx", data(&[("rate", "0.86")]), false);
    let eur = measure_map(&response, "total")
        .get("eur")
        .expect("eur column");
    assert_eq!(eur.to_string(), "43");
}

#[test]
fn with_binding_in_dynamic_unit_scales() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
repo fx_repo

spec priced
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
data amount: money

spec fx
uses p: priced
    -> with amount: 50 gbp
rule total: p.amount
"#,
    );
    let response = run_in_repo(
        &engine,
        Some("fx_repo"),
        "fx",
        data(&[("p.rate", "0.86")]),
        false,
    );
    let eur = measure_map(&response, "total")
        .get("eur")
        .expect("eur column");
    assert_eq!(eur.to_string(), "43");
}

#[test]
fn request_input_in_dynamic_unit_scales() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec fx
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
data amount: money
rule total: amount
"#,
    );
    let response = run(
        &engine,
        "fx",
        data(&[("rate", "0.86"), ("amount", "50 gbp")]),
        false,
    );
    let eur = measure_map(&response, "total")
        .get("eur")
        .expect("eur column");
    assert_eq!(eur.to_string(), "43");
}

// =============================================================================
// Evaluation: conversions
// =============================================================================

#[test]
fn as_dynamic_unit_scales() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec fx
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
rule total: 100 eur as gbp
"#,
    );
    let response = run(&engine, "fx", data(&[("rate", "0.86")]), false);
    let gbp = measure_map(&response, "total")
        .get("gbp")
        .expect("gbp column");
    assert_eq!(gbp.to_string(), "116.279069767441860465116279");
}

#[test]
fn as_number_with_dynamic_binding_unit_scales() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec fx
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
data amount: money
rule total: amount as gbp as number
"#,
    );
    let response = run(
        &engine,
        "fx",
        data(&[("rate", "0.86"), ("amount", "86 eur")]),
        false,
    );
    let num = value_of(&response, "total")
        .number
        .as_ref()
        .expect("number");
    assert_eq!(num.to_string(), "100");
}

#[test]
fn abs_works_with_dynamic_unit() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec fx
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
rule total: abs(-50 gbp)
"#,
    );
    let response = run(&engine, "fx", data(&[("rate", "0.86")]), false);
    let eur = measure_map(&response, "total")
        .get("eur")
        .expect("eur column");
    assert_eq!(eur.to_string(), "43");
}

// =============================================================================
// Evaluation: scale outcomes
// =============================================================================

#[test]
fn unbound_scale_gives_missing_data() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec fx
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
rule total: 100 gbp
"#,
    );
    let response = run(&engine, "fx", IndexMap::new(), false);
    assert_missing_scale(&response, "total", "rate");
}

#[test]
fn vetoed_scale_propagates_veto() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec fx2
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
data blocked: true
rule rate: 0.86 unless blocked then veto "rate unavailable"
rule total: 100 gbp
"#,
    );
    let response = run(&engine, "fx2", IndexMap::new(), false);
    let r = rule(&response, "total");
    assert!(r.vetoed, "vetoed scale must propagate");
    assert_eq!(
        r.veto_reason.as_deref(),
        Some("rate unavailable"),
        "veto reason from the scale rule must surface"
    );
}

#[test]
fn zero_scale_gives_computation_veto() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec fx
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
rule total: 100 gbp
"#,
    );
    let response = run(&engine, "fx", data(&[("rate", "0")]), false);
    let r = rule(&response, "total");
    assert!(r.vetoed, "zero scale must veto");
    assert!(
        !r.is_missing_data(),
        "zero scale is a computation veto, not missing data"
    );
    assert_eq!(
        r.veto_reason.as_deref(),
        Some("unit conversion factor must be greater than zero")
    );
    assert!(r.result.is_none(), "zero scale must not yield a value");
}

#[test]
fn negative_scale_gives_computation_veto() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec fx
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
rule total: 100 gbp
"#,
    );
    let response = run(&engine, "fx", data(&[("rate", "-0.5")]), false);
    let r = rule(&response, "total");
    assert!(r.vetoed, "negative scale must veto");
    assert!(!r.is_missing_data(), "negative scale is a computation veto");
    assert_eq!(
        r.veto_reason.as_deref(),
        Some("unit conversion factor must be greater than zero")
    );
    assert!(r.result.is_none(), "negative scale must not yield a value");
}

// =============================================================================
// Response: per-column measure map
// =============================================================================

#[test]
fn flat_fee_omits_dynamic_column_when_scale_missing() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec fx
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
rule flat_fee: 10 eur
"#,
    );
    let response = run(&engine, "fx", IndexMap::new(), false);
    let m = measure_map(&response, "flat_fee");
    // eur is the binding unit and always present; gbp is dynamic and omitted when
    // its scale is missing (the rule's own value still succeeds).
    assert!(m.contains_key("eur"), "eur column must be present");
    assert!(
        !m.contains_key("gbp"),
        "gbp column must be omitted when scale missing"
    );
}

#[test]
fn binding_unit_column_always_present() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec fx
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
rule total: 100 gbp
"#,
    );
    let response = run(&engine, "fx", data(&[("rate", "0.86")]), false);
    let m = measure_map(&response, "total");
    assert!(
        m.contains_key("eur"),
        "eur (binding) column must be present"
    );
    assert!(
        m.contains_key("gbp"),
        "gbp column must be present when scale bound"
    );
}

#[test]
fn missing_data_names_scale_input() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec fx
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
rule total: 100 gbp
"#,
    );
    let response = run(&engine, "fx", IndexMap::new(), false);
    let r = rule(&response, "total");
    assert!(r.is_missing_data());
    assert!(
        r.missing_data().iter().any(|k| k.contains("rate")),
        "missing_data must name the scale input 'rate', got {:?}",
        r.missing_data()
    );
}

// =============================================================================
// All-dynamic family
// =============================================================================

#[test]
fn all_dynamic_family_rule_vetoes_when_scale_missing() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec fx
data rate: number
data money: measure
    -> unit gbp: rate
    -> unit usd: rate
rule total: 100 gbp
"#,
    );
    let response = run(&engine, "fx", IndexMap::new(), false);
    assert_missing_scale(&response, "total", "rate");
}

#[test]
fn all_dynamic_family_rule_succeeds_when_scale_bound() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec fx
data rate: number
data money: measure
    -> unit gbp: rate
    -> unit usd: rate
rule total: 100 gbp
"#,
    );
    let response = run(&engine, "fx", data(&[("rate", "0.86")]), false);
    let r = rule(&response, "total");
    assert!(!r.vetoed, "must succeed when scale bound");
    let m = measure_map(&response, "total");
    assert_eq!(
        m.get("gbp").expect("gbp").to_string(),
        "100",
        "written gbp magnitude"
    );
    assert_eq!(
        m.get("usd").expect("usd").to_string(),
        "100",
        "same rate settles usd column"
    );
}

#[test]
fn all_dynamic_unused_first_unit_scale_does_not_veto_settled_value() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec fx
data usd_rate: number
data gbp_rate: number
data money: measure
    -> unit usd: usd_rate
    -> unit gbp: gbp_rate
rule total: 100 gbp
"#,
    );
    let response = run(&engine, "fx", data(&[("gbp_rate", "0.86")]), false);
    let r = rule(&response, "total");
    assert!(
        !r.vetoed,
        "settled gbp value must not veto when unused first unit usd_rate is unbound, got {:?}",
        r.veto_reason
    );
    let m = measure_map(&response, "total");
    assert!(m.contains_key("gbp"), "gbp column must be present");
    assert!(
        !m.contains_key("usd"),
        "usd column must be omitted when usd_rate is unbound"
    );
    assert_eq!(
        m.get("gbp").expect("gbp").to_string(),
        "100",
        "written gbp magnitude stays until its own scale is used for other columns"
    );
}

#[test]
fn all_dynamic_family_cycle_is_planning_error() {
    let mut engine = Engine::new();
    let code = r#"
spec t
data money: measure
    -> unit gbp: rate
    -> unit usd: rate
rule rate: 1 gbp as gbp as number
rule total: 100 gbp
"#;
    let msg = load_err(&mut engine, code);
    assert!(
        msg == "In spec 't': Validation error: Circular dependency (rules: rate) at volatile:6:1",
        "cycle through all-dynamic family dependency must be a planning error, got: {msg}"
    );
}

// =============================================================================
// uses-qualified scale
// =============================================================================

#[test]
fn uses_qualified_scale_resolves() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
repo fx_repo

spec rate_src
data rate: number

spec consumer
uses rs: rate_src
data money: measure
    -> unit eur: 1
    -> unit gbp: rs.rate
rule total: 100 gbp
"#,
    );
    let response = run_in_repo(
        &engine,
        Some("fx_repo"),
        "consumer",
        data(&[("rs.rate", "0.86")]),
        false,
    );
    let eur = measure_map(&response, "total")
        .get("eur")
        .expect("eur column");
    assert_eq!(eur.to_string(), "86");
}

#[test]
fn two_aliases_with_different_scales_conflict() {
    let mut engine = Engine::new();
    let code = r#"
repo r

spec rates_a
data rate: number

spec rates_b
data rate: number

spec t
uses a: rates_a
uses b: rates_b
data money: measure
    -> unit eur: 1
    -> unit gbp: a.rate
data money_b: money
    -> unit gbp: b.rate
"#;
    let msg = load_err(&mut engine, code);
    assert_eq!(
        msg,
        "In spec 't': Validation error: Failed to apply constraint 'unit': Unit 'gbp' is already defined in this type's inherited units; cannot change factor or decomposition. Add a new unit name instead. at volatile:16:1"
    );
}

#[test]
fn cycle_through_uses_binding_is_planning_error() {
    let mut engine = Engine::new();
    let code = r#"
repo r

spec inner
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
data rate: number
rule out: 100 gbp

spec t
uses i: inner
    -> with rate: out
rule out: i.out as number
"#;
    let msg = load_err(&mut engine, code);
    assert!(
        msg.contains("Circular dependency") && msg.contains("out") && msg.contains("i.out"),
        "cycle through a uses binding must name out and i.out, got: {msg}"
    );
}

#[test]
fn conversion_narration_names_the_scale_reference() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec fx
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
rule total: 100 eur as gbp
"#,
    );
    let response = run(&engine, "fx", data(&[("rate", "0.86")]), true);
    let r = rule(&response, "total");
    let explanation = format!("{:?}", r.explanation);
    assert!(
        explanation.contains("1 gbp is 0.86 eur (rate)"),
        "narration must name the scale reference, got: {explanation}"
    );
}

#[test]
fn as_unit_then_as_number_with_dynamic_unit() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec fx
data rate: number
data money: measure
    -> unit gbp: rate
data amount: money
rule total: amount as gbp as number
"#,
    );
    let response = run(
        &engine,
        "fx",
        data(&[("rate", "0.86"), ("amount", "100 gbp")]),
        false,
    );
    let num = value_of(&response, "total")
        .number
        .as_ref()
        .expect("number");
    assert_eq!(num.to_string(), "100");
}

#[test]
fn as_number_without_unit_on_dynamic_measure_is_planning_error() {
    let mut engine = Engine::new();
    let code = r#"
spec fx
data rate: number
data money: measure
    -> unit gbp: rate
data amount: money
rule total: amount as number
"#;
    let msg = load_err(&mut engine, code);
    assert!(
        msg.contains("as number") && msg.contains("without a unit"),
        "bare as number on a measure must require an explicit unit, got: {msg}"
    );
}

#[test]
fn step_rounding_with_dynamic_unit_step() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec fx
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
rule total: 100 eur -> round 1 gbp
"#,
    );
    let response = run(&engine, "fx", data(&[("rate", "0.86")]), false);
    let gbp = measure_map(&response, "total")
        .get("gbp")
        .expect("gbp column");
    assert_eq!(gbp.to_string(), "116");
}

#[test]
fn data_reference_chain_settles_unscaled_source() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
repo r

spec src
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
data amount: 100 gbp

spec fx
uses s: src
rule total: s.amount
"#,
    );
    let response = run_in_repo(&engine, Some("r"), "fx", data(&[("s.rate", "0.86")]), false);
    let eur = measure_map(&response, "total")
        .get("eur")
        .expect("eur column");
    assert_eq!(eur.to_string(), "86");
}

#[test]
fn spec_written_list_rows_in_dynamic_unit_scale() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec fx
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
data items: money list
    -> with 50 gbp, 50 gbp
rule total: items -> sum
"#,
    );
    let response = run(&engine, "fx", data(&[("rate", "0.86")]), false);
    let eur = measure_map(&response, "total")
        .get("eur")
        .expect("eur column");
    assert_eq!(eur.to_string(), "86");
}

#[test]
fn request_range_in_dynamic_unit_scales() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec fx
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
data band: money range
rule width: upper(band) - lower(band)
"#,
    );
    let response = run(
        &engine,
        "fx",
        data(&[("rate", "0.86"), ("band", "10 gbp ... 100 gbp")]),
        false,
    );
    let eur = measure_map(&response, "width")
        .get("eur")
        .expect("eur column");
    assert_eq!(eur.to_string(), "77.4");
}

#[test]
fn request_list_rows_in_dynamic_unit_scales() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec fx
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
data items: money list
rule total: items -> sum
"#,
    );
    let mut request = data(&[("rate", "0.86")]);
    request.insert(
        "items".to_string(),
        EvaluateDataValue::List(vec![
            EvaluateDataValue::string("50 gbp"),
            EvaluateDataValue::string("50 gbp"),
        ]),
    );
    let response = run(&engine, "fx", request, false);
    let eur = measure_map(&response, "total")
        .get("eur")
        .expect("eur column");
    assert_eq!(eur.to_string(), "86");
}

#[test]
fn settle_scalar_range_and_list_unscaled_values() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec fx
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
data amount: money
data band: money range
data items: money list
rule scalar: amount
rule span: upper(band) - lower(band)
rule total: items -> sum
"#,
    );
    let mut request = data(&[
        ("rate", "0.86"),
        ("amount", "100 gbp"),
        ("band", "10 gbp ... 100 gbp"),
    ]);
    request.insert(
        "items".to_string(),
        EvaluateDataValue::List(vec![
            EvaluateDataValue::string("50 gbp"),
            EvaluateDataValue::string("50 gbp"),
        ]),
    );
    let response = run(&engine, "fx", request, false);
    assert_eq!(
        measure_map(&response, "scalar")
            .get("eur")
            .expect("scalar eur")
            .to_string(),
        "86"
    );
    assert_eq!(
        measure_map(&response, "span")
            .get("eur")
            .expect("span eur")
            .to_string(),
        "77.4"
    );
    assert_eq!(
        measure_map(&response, "total")
            .get("eur")
            .expect("total eur")
            .to_string(),
        "86"
    );
}

#[test]
fn qualified_compound_unit_without_numeric_prefix_stays_compound() {
    let mut engine = Engine::new();
    let code = r#"
spec t
uses lemma units
data pay: measure
    -> unit my_hour: units.duration.hour
"#;
    let msg = load_err(&mut engine, code);
    assert!(
        msg.contains("cannot use a single unit from another measure")
            && msg.contains("hour")
            && msg.contains("duration"),
        "qualified unit path must stay a compound unit, got: {msg}"
    );
    assert!(
        !msg.contains("nested uses hops"),
        "must not treat units.duration.hour as a scale reference, got: {msg}"
    );
}

#[test]
fn options_checked_data_in_dynamic_family() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec fx
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
data amount: money
    -> option 10 eur
    -> option 20 eur
rule total: amount
"#,
    );
    let response = run(
        &engine,
        "fx",
        data(&[("rate", "0.86"), ("amount", "10 eur")]),
        false,
    );
    let eur = measure_map(&response, "total")
        .get("eur")
        .expect("eur column");
    assert_eq!(eur.to_string(), "10");
}

#[test]
fn needed_by_rules_names_the_scale_input() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec fx
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
rule total: 100 gbp
"#,
    );
    let show = engine.show(None, "fx", None).expect("show");
    let rate = show.data.get("rate").expect("rate");
    assert!(
        rate.needed_by_rules.iter().any(|name| name == "total"),
        "rate must be needed by total, got {:?}",
        rate.needed_by_rules
    );
}

/// Uses-qualified dynamic scales must already be in static Show.data (not only after
/// a unit is chosen). Bound `with` scales must not be re-prompted.
#[test]
fn show_lists_unbound_uses_qualified_fx_scales_statically() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
repo r

spec fx
uses lemma units
data usd_rate: number
  -> minimum 0
data gbp_rate: number
  -> minimum 0
data money: measure
  -> unit eur: 1.00
  -> unit usd: usd_rate
  -> unit gbp: gbp_factor
  -> minimum 0 eur
data price_per_mass: measure
  -> unit eur_per_kg: eur/kilogram
  -> unit usd_per_kg: usd/kilogram
  -> unit gbp_per_kg: gbp/kilogram
  -> minimum 0 eur_per_kg
rule gbp_factor: 1 / gbp_rate

spec priced
uses fx
  -> with usd_rate: usd_rate
data usd_rate: number
  -> minimum 0
data price: fx.price_per_mass
rule total: price
"#,
    );
    let show = engine.show(Some("r"), "priced", None).expect("show");
    assert!(
        show.data.contains_key("fx.gbp_rate"),
        "unbound GBP FX must be in static Show.data, got {:?}",
        show.data.keys().collect::<Vec<_>>()
    );
    assert!(
        !show.data.contains_key("fx.usd_rate"),
        "with-bound USD FX must not be a Show prompt, got {:?}",
        show.data.keys().collect::<Vec<_>>()
    );
    assert!(
        show.data.contains_key("usd_rate"),
        "local USD re-export must remain Show data, got {:?}",
        show.data.keys().collect::<Vec<_>>()
    );
    let gbp = show.data.get("fx.gbp_rate").expect("gbp fx");
    assert!(
        !gbp.needed_by_rules.is_empty(),
        "GBP FX must be needed by a rule, got {:?}",
        gbp.needed_by_rules
    );

    let missing = run_in_repo(
        &engine,
        Some("r"),
        "priced",
        data(&[("price", "2 gbp_per_kg")]),
        false,
    );
    assert_missing_scale(&missing, "total", "fx.gbp_rate");
    for key in rule(&missing, "total").missing_data() {
        assert!(
            show.data.contains_key(key.as_str()),
            "invariant: MissingData key '{key}' must be in static Show.data {:?}",
            show.data.keys().collect::<Vec<_>>()
        );
    }

    let ok = run_in_repo(
        &engine,
        Some("r"),
        "priced",
        data(&[
            ("price", "2 gbp_per_kg"),
            ("fx.gbp_rate", "0.86"),
            ("usd_rate", "0.92"),
        ]),
        false,
    );
    assert!(!rule(&ok, "total").vetoed, "gbp with rate must settle");
}

#[test]
fn show_omits_with_bound_gbp_fx_but_still_settles() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
repo r

spec fx
uses lemma units
data gbp_rate: number
  -> minimum 0
data money: measure
  -> unit eur: 1.00
  -> unit gbp: gbp_factor
  -> minimum 0 eur
rule gbp_factor: 1 / gbp_rate

spec quote
uses fx
  -> with gbp_rate: 0.86
data fee: fx.money
rule total: fee
"#,
    );
    let show = engine.show(Some("r"), "quote", None).expect("show");
    assert!(
        !show.data.contains_key("fx.gbp_rate"),
        "with-bound GBP FX must not be a Show prompt, got {:?}",
        show.data.keys().collect::<Vec<_>>()
    );
    let response = run_in_repo(
        &engine,
        Some("r"),
        "quote",
        data(&[("fee", "10 eur")]),
        false,
    );
    let m = measure_map(&response, "total");
    assert_eq!(m.get("eur").expect("eur").to_string(), "10");
}

#[test]
fn show_includes_uses_qualified_data_scale_of_imported_amount() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec priced
uses lemma units
data rate: number
data money: measure
  -> unit eur: 1
  -> unit gbp: rate
data amount: money

spec consumer
uses p: priced
rule out: p.amount
"#,
    );
    let show = engine.show(None, "consumer", None).expect("show");
    assert!(
        show.data.contains_key("p.rate"),
        "uses-qualified scale must be in static Show.data, got {:?}",
        show.data.keys().collect::<Vec<_>>()
    );
    let response = run(&engine, "consumer", data(&[("p.amount", "1 gbp")]), false);
    assert_missing_scale(&response, "out", "p.rate");
    for key in rule(&response, "out").missing_data() {
        assert!(
            show.data.contains_key(key.as_str()),
            "invariant: MissingData key '{key}' must be in static Show.data {:?}",
            show.data.keys().collect::<Vec<_>>()
        );
    }
}

#[test]
fn show_prefill_of_unscaled_spec_value_keeps_written_magnitude() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec fx
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
data amount: 100 gbp
rule total: amount
"#,
    );
    let show = engine.show(None, "fx", None).expect("show");
    let amount = show.data.get("amount").expect("amount");
    let value = amount.value.as_ref().expect("prefill");
    let result = value.result.as_ref().expect("result");
    assert!(
        result.contains("100") && result.contains("gbp"),
        "prefill must show the written magnitude and unit, got: {result}"
    );
}

// =============================================================================
// Scale dependencies run before the rule body
// =============================================================================

fn explanation_text(response: &lemma::Response, name: &str) -> String {
    let explanation = rule(response, name)
        .explanation
        .as_ref()
        .expect("explanation");
    lemma::format_explanation(explanation)
}

const RULE_SCALE_UNUSED: &str = r#"
spec fx
data blocked: boolean
data money: measure
    -> unit eur: 1
    -> unit gbp: gbp_rate
data amount: money
rule gbp_rate: 0.5 unless blocked then veto "no rate"
rule total: amount
"#;

#[test]
fn explain_unused_rule_scale_reads_settled_scale() {
    let mut engine = Engine::new();
    load(&mut engine, RULE_SCALE_UNUSED);
    let response = run(
        &engine,
        "fx",
        data(&[("blocked", "false"), ("amount", "10 eur")]),
        true,
    );
    let m = measure_map(&response, "total");
    assert_eq!(m.get("eur").expect("eur").to_string(), "10");
    assert_eq!(m.get("gbp").expect("gbp").to_string(), "20");
    assert!(explanation_text(&response, "total").contains("10 eur"));
}

#[test]
fn explain_vetoed_unused_rule_scale_omits_its_column() {
    let mut engine = Engine::new();
    load(&mut engine, RULE_SCALE_UNUSED);
    let response = run(
        &engine,
        "fx",
        data(&[("blocked", "true"), ("amount", "10 eur")]),
        true,
    );
    let r = rule(&response, "total");
    assert!(
        !r.vetoed,
        "a vetoed scale the value does not need must not veto"
    );
    let m = measure_map(&response, "total");
    assert_eq!(m.get("eur").expect("eur").to_string(), "10");
    assert!(!m.contains_key("gbp"), "vetoed scale column omitted: {m:?}");
    assert!(explanation_text(&response, "total").contains("10 eur"));
}

#[test]
fn explain_import_binding_in_dynamic_unit() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
repo fx_repo

spec config
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
data price: money
rule price_eur: price

spec fx
uses c: config
    -> with price: 5 gbp, rate: 0.86
rule total: c.price
rule converted: c.price_eur
"#,
    );
    let response = run_in_repo(&engine, Some("fx_repo"), "fx", IndexMap::new(), true);
    for name in ["total", "converted"] {
        let m = measure_map(&response, name);
        assert_eq!(m.get("eur").expect("eur").to_string(), "4.3", "{name}");
        assert_eq!(
            m.get("gbp").expect("gbp").to_string(),
            "5",
            "{name}: gbp settles with c.rate"
        );
    }
    let total = value_of(&response, "total");
    assert_eq!(
        total.result.as_deref(),
        Some("5 gbp"),
        "the written unit displays with the instance's factor"
    );
    assert_eq!(total.unit.as_deref(), Some("gbp"));
    let converted = value_of(&response, "converted");
    assert_eq!(
        converted.result.as_deref(),
        Some("5 gbp"),
        "price_eur reads price and keeps the written unit once the instance's factor is known"
    );
    assert_eq!(converted.unit.as_deref(), Some("gbp"));
    assert!(
        explanation_text(&response, "total").contains("5 gbp"),
        "the value carries the instance's factor into the explanation"
    );
}

/// Shared FX import: money with a dynamic data scale and a dynamic rule scale.
/// A consumer that holds `fx.money` slots must plan without panic.
#[test]
fn uses_fx_money_with_data_and_rule_scales_loads() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
repo r

spec fx
uses lemma units
data usd_rate: number
  -> minimum 0
data gbp_rate: number
  -> minimum 0
data money: measure
  -> unit eur: 1.00
  -> unit usd: usd_rate
  -> unit gbp: gbp_factor
  -> minimum 0 eur
data price_per_mass: measure
  -> unit eur_per_kg: eur/kilogram
  -> unit usd_per_kg: usd/kilogram
  -> unit gbp_per_kg: gbp/kilogram
  -> minimum 0 eur_per_kg
rule gbp_factor: 1 / gbp_rate

spec quote
uses fx
  -> with gbp_rate: 0.86
data fee: fx.money
rule total: fee
"#,
    );
    let response = run_in_repo(
        &engine,
        Some("r"),
        "quote",
        data(&[("fee", "10 eur"), ("fx.usd_rate", "0.92")]),
        false,
    );
    let m = measure_map(&response, "total");
    assert_eq!(m.get("eur").expect("eur").to_string(), "10");
}

/// Parent lists a child that `uses` dynamic FX; parent sums a child money rule.
/// Parent must not force the child's import FX scale as a SpecMember of itself.
#[test]
fn parent_sum_of_child_list_that_uses_dynamic_fx_loads() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
repo r

spec fx
data usd_rate: number
  -> minimum 0
data money: measure
  -> unit eur: 1.00
  -> unit usd: usd_rate
  -> minimum 0 eur

spec line
uses fx
data amount: fx.money
rule line_total: amount

spec order
uses line
data lines: line list
  -> with amount: 10 eur
rule total: lines -> sum line_total
"#,
    );
    let response = run_in_repo(
        &engine,
        Some("r"),
        "order",
        data(&[("lines.1.fx.usd_rate", "0.5")]),
        false,
    );
    let m = measure_map(&response, "total");
    assert_eq!(m.get("eur").expect("eur").to_string(), "10");
    assert_eq!(
        m.get("usd").expect("usd").to_string(),
        "20",
        "row fx rate 0.5 converts 10 eur to 20 usd"
    );
}

/// Consumer and a nested list element both `uses` the same dynamic-FX spec.
/// List nesting must not register the element's import FX scale as a SpecMember
/// on the list name.
#[test]
fn nested_list_and_consumer_both_use_same_dynamic_fx_spec() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
repo r

spec fx
data rate: number
data money: measure
    -> unit eur: 1
    -> unit usd: rate

spec piece
uses fx
    -> with rate: 0.5
data amount: fx.money

spec assembly
uses fx
    -> with rate: 0.92
uses piece
data items: piece list
    -> with amount: 10 eur
rule read: (items -> first).amount
rule local: 10 fx.usd
"#,
    );
    let response = run_in_repo(&engine, Some("r"), "assembly", IndexMap::new(), false);
    let read = measure_map(&response, "read");
    assert_eq!(read.get("eur").expect("eur").to_string(), "10");
    assert_eq!(
        read.get("usd").expect("usd").to_string(),
        "20",
        "row amount's usd column uses the nested piece's fx rate 0.5 (10 eur / 0.5)"
    );
    let local = measure_map(&response, "local");
    assert_eq!(
        local.get("eur").expect("eur").to_string(),
        "9.2",
        "assembly's own usd uses assembly's fx rate 0.92"
    );
    assert_eq!(local.get("usd").expect("usd").to_string(), "10");
}

#[test]
fn explain_list_rows_with_per_row_scale() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
repo fx_repo

spec line
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
data amount: money
rule in_eur: amount

spec order
uses line
data lines: line list
    -> with rate: 0.86, amount: 100 gbp
    -> with rate: 0.5, amount: 100 gbp
rule total: lines -> sum in_eur
rule first_line: (lines -> first).in_eur
rule first_amount: (lines -> first).amount
rule cheap: lines -> where(amount < 60 eur) -> count
rule cheap_by_rule: lines -> where(in_eur < 60 eur) -> count
rule above_ninety: lines -> where(amount > 90 gbp) -> count
"#,
    );
    let response = run_in_repo(&engine, Some("fx_repo"), "order", IndexMap::new(), true);

    let total = measure_map(&response, "total");
    assert_eq!(total.get("eur").expect("eur").to_string(), "136");
    assert!(
        !total.contains_key("gbp"),
        "a sum over rows with different rates has no one gbp factor: {total:?}"
    );

    let first_line = measure_map(&response, "first_line");
    assert_eq!(first_line.get("eur").expect("eur").to_string(), "86");
    assert_eq!(
        first_line.get("gbp").expect("gbp").to_string(),
        "100",
        "the value read from the first row carries that row's factor"
    );
    let value = value_of(&response, "first_line");
    assert_eq!(
        value.unit.as_deref(),
        Some("gbp"),
        "in_eur reads amount and keeps the written unit once the row's factor is known"
    );

    let first_amount = measure_map(&response, "first_amount");
    assert_eq!(first_amount.get("eur").expect("eur").to_string(), "86");
    assert_eq!(
        first_amount.get("gbp").expect("gbp").to_string(),
        "100",
        "the field read from the first row carries that row's factor"
    );
    let value = value_of(&response, "first_amount");
    assert_eq!(
        value.result.as_deref(),
        Some("100 gbp"),
        "a written unit displays with its row's factor"
    );

    for name in ["cheap", "cheap_by_rule"] {
        let count = value_of(&response, name).number.as_ref().expect("number");
        assert_eq!(count.to_string(), "1", "{name}");
        assert!(
            explanation_text(&response, name).contains("2. amount: 100 gbp"),
            "a row presents its own field in its own unit: {name}"
        );
    }
    let above_ninety = value_of(&response, "above_ninety")
        .number
        .as_ref()
        .expect("number");
    assert_eq!(
        above_ninety.to_string(),
        "2",
        "each row compares with its own rate: 100 gbp is 86 and 50 eur, both above 90 gbp"
    );
}

#[test]
fn scale_rule_reading_its_own_family_is_planning_error() {
    let mut engine = Engine::new();
    let code = r#"
spec t
data money: measure
    -> unit eur: 1
    -> unit gbp: gbp_rate
data eur_quote: money
rule gbp_rate: eur_quote / 1 eur
rule total: 100 gbp
"#;
    let msg = load_err(&mut engine, code);
    assert!(
        msg.contains("Circular dependency (rules: gbp_rate) at"),
        "a scale rule touching its own family is a cycle, got: {msg}"
    );
}

#[test]
fn dynamic_unit_literal_settles_with_uses_scale() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
repo r

spec priced
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate

spec fx
uses p: priced
    -> with rate: 0.86
rule total: 10 gbp
"#,
    );
    let response = run_in_repo(&engine, Some("r"), "fx", IndexMap::new(), false);
    let m = measure_map(&response, "total");
    assert_eq!(
        m.get("eur").expect("eur").to_string(),
        "8.6",
        "10 gbp settles with p.rate"
    );
    assert_eq!(m.get("gbp").expect("gbp").to_string(), "10");
    let value = value_of(&response, "total");
    assert_eq!(value.result.as_deref(), Some("10 gbp"));
}

#[test]
fn dynamic_unit_conversion_settles_with_uses_scale() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
repo r

spec priced
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
data amount: money

spec fx
uses p: priced
    -> with amount: 10 eur, rate: 0.86
rule total: p.amount as gbp
"#,
    );
    let response = run_in_repo(&engine, Some("r"), "fx", IndexMap::new(), false);
    let m = measure_map(&response, "total");
    assert_eq!(m.get("eur").expect("eur").to_string(), "10");
    assert_eq!(
        m.get("gbp").expect("gbp").to_string(),
        "11.6279069767441860465116279",
        "10 eur is 10/0.86 gbp with p.rate"
    );
    let value = value_of(&response, "total");
    assert_eq!(value.unit.as_deref(), Some("gbp"));
    assert_eq!(
        value.result.as_deref(),
        Some("11.6279069767441860465116279 gbp"),
        "the conversion target displays in gbp with the instance's factor"
    );
}

#[test]
fn all_dynamic_value_settles_with_uses_scale() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
repo r

spec priced
data rate: number
data money: measure
    -> unit gbp: rate
    -> unit usd: rate
data amount: money
rule doubled: amount * 2

spec fx
uses p: priced
    -> with amount: 5 gbp, rate: 0.86
data lines: p list
    -> with amount: 5 gbp, rate: 0.86
rule total: p.amount
rule twice: p.doubled
rule big: p.amount > p.doubled
rule first_amount: (lines -> first).amount
rule line_count: lines -> count
"#,
    );
    let response = run_in_repo(&engine, Some("r"), "fx", IndexMap::new(), true);
    for name in ["total", "first_amount"] {
        let m = measure_map(&response, name);
        assert_eq!(m.get("gbp").expect("gbp").to_string(), "5", "{name}");
        assert_eq!(m.get("usd").expect("usd").to_string(), "5", "{name}");
        let value = value_of(&response, name);
        assert_eq!(value.result.as_deref(), Some("5 gbp"), "{name}");
    }
    let m = measure_map(&response, "twice");
    assert_eq!(m.get("gbp").expect("gbp").to_string(), "10");
    assert_eq!(m.get("usd").expect("usd").to_string(), "10");
    assert_eq!(
        value_of(&response, "twice").result.as_deref(),
        Some("10 gbp")
    );
    let big = value_of(&response, "big")
        .boolean
        .as_ref()
        .expect("boolean");
    assert!(!*big, "5 gbp is not greater than 10 gbp");
    let count = value_of(&response, "line_count")
        .number
        .as_ref()
        .expect("number");
    assert_eq!(count.to_string(), "1");
}

#[test]
fn two_uses_qualified_dynamic_units_use_each_instance_scale() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
repo r

spec priced
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate

spec fx
uses a: priced
    -> with rate: 0.80
uses b: priced
    -> with rate: 0.90
rule left: 10 a.money.gbp
rule right: 10 b.money.gbp
"#,
    );
    let response = run_in_repo(&engine, Some("r"), "fx", IndexMap::new(), false);
    let left = measure_map(&response, "left");
    assert_eq!(
        left.get("eur").expect("eur").to_string(),
        "8",
        "left settles with a.rate"
    );
    assert_eq!(left.get("gbp").expect("gbp").to_string(), "10");
    let right = measure_map(&response, "right");
    assert_eq!(
        right.get("eur").expect("eur").to_string(),
        "9",
        "right settles with b.rate"
    );
    assert_eq!(right.get("gbp").expect("gbp").to_string(), "10");
}

#[test]
fn two_uses_bare_dynamic_unit_stays_qualify_error() {
    let mut engine = Engine::new();
    let code = r#"
repo r

spec priced
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate

spec fx
uses a: priced
    -> with rate: 0.80
uses b: priced
    -> with rate: 0.90
rule total: 10 gbp
"#;
    let msg = load_err(&mut engine, code);
    assert!(
        msg.contains("Unit 'gbp' matches more than one type. Write one of:"),
        "a bare dynamic unit with two instances must demand qualification, got: {msg}"
    );
}

#[test]
fn nested_uses_compound_dynamic_unit_settles_through_alias() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
repo r

spec amounts
uses lemma units
data gbp_rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: gbp_rate
data price_per_mass: measure
    -> unit eur_per_kg: eur/kilogram
    -> unit gbp_per_kg: gbp/kilogram

spec offer
uses amounts
    -> with gbp_rate: gbp_rate
data gbp_rate: number
data price: amounts.price_per_mass

spec quote
uses off_1: offer
    -> with gbp_rate: 0.86, price: 6.28488 eur_per_kg
rule matches: off_1.price is 7.308 off_1.amounts.gbp_per_kg
"#,
    );
    let response = run_in_repo(&engine, Some("r"), "quote", IndexMap::new(), false);
    let matched = value_of(&response, "matches")
        .boolean
        .as_ref()
        .expect("boolean");
    assert!(
        *matched,
        "7.308 gbp_per_kg with off_1's nested rate 0.86 is 6.28488 eur_per_kg"
    );
}

const ALL_DYNAMIC_EMPTY_SUM: &str = r#"
spec fx
data rate: number
data money: measure
    -> unit gbp: rate
    -> unit usd: rate
data items: money list
rule total: items -> sum
"#;

fn empty_items(pairs: &[(&str, &str)]) -> IndexMap<String, EvaluateDataValue> {
    let mut request = data(pairs);
    request.insert("items".to_string(), EvaluateDataValue::List(Vec::new()));
    request
}

#[test]
fn all_dynamic_empty_sum_with_scale() {
    let mut engine = Engine::new();
    load(&mut engine, ALL_DYNAMIC_EMPTY_SUM);
    let response = run(&engine, "fx", empty_items(&[("rate", "0.86")]), true);
    let m = measure_map(&response, "total");
    assert_eq!(m.get("gbp").expect("gbp").to_string(), "0");
    assert_eq!(m.get("usd").expect("usd").to_string(), "0");
    assert!(explanation_text(&response, "total").contains("0 gbp"));
}

#[test]
fn all_dynamic_empty_sum_without_scale_is_missing_data() {
    let mut engine = Engine::new();
    load(&mut engine, ALL_DYNAMIC_EMPTY_SUM);
    let response = run(&engine, "fx", empty_items(&[]), true);
    assert_missing_scale(&response, "total", "rate");
}

// =============================================================================
// Unbound dynamic scale: every consumer path → missing data (named key, no value)
// =============================================================================

#[test]
fn unbound_scale_on_uses_imported_money_must_not_add_as_eur() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec amounts
uses lemma units
data gbp_rate: number
data money: measure
  -> unit eur: 1
  -> unit gbp: gbp_rate

spec left_side
uses amounts
data amount: amounts.money
rule out: amount

spec right_side
uses amounts
data amount: amounts.money
rule out: amount

spec bundle
uses l: left_side
uses r: right_side
rule total: l.out + r.out
"#,
    );
    let response = run(
        &engine,
        "bundle",
        data(&[("l.amount", "1 gbp"), ("r.amount", "2 gbp")]),
        false,
    );
    assert_missing_scale(&response, "total", "gbp_rate");
}

#[test]
fn unbound_scale_cast_as_literal_unit_must_be_missing_data_not_panic() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec amounts
uses lemma units
data gbp_rate: number
data money: measure
  -> unit eur: 1
  -> unit gbp: gbp_rate
data amount: money
rule in_eur: amount as eur
"#,
    );
    let response = run(&engine, "amounts", data(&[("amount", "1.2 gbp")]), false);
    assert_missing_scale(&response, "in_eur", "gbp_rate");
}

#[test]
fn unbound_scale_same_spec_arithmetic_is_missing_data() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec fx
uses lemma units
data rate: number
data money: measure
  -> unit eur: 1
  -> unit gbp: rate
data a: money
data b: money
rule added: a + b
rule diff: a - b
rule twice: a * 2
rule half: a / 2
"#,
    );
    let response = run(
        &engine,
        "fx",
        data(&[("a", "1 gbp"), ("b", "2 gbp")]),
        false,
    );
    for name in ["added", "diff", "twice", "half"] {
        assert_missing_scale(&response, name, "rate");
    }
}

#[test]
fn unbound_scale_cast_family_is_missing_data() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec fx
uses lemma units
data rate: number
data money: measure
  -> unit eur: 1
  -> unit gbp: rate
data amount: money
rule as_eur: amount as eur
rule as_gbp: amount as gbp
rule as_num: amount as gbp as number
"#,
    );
    let response = run(&engine, "fx", data(&[("amount", "1.2 gbp")]), false);
    for name in ["as_eur", "as_gbp", "as_num"] {
        assert_missing_scale(&response, name, "rate");
    }
}

#[test]
fn unbound_scale_compound_cast_as_literal_unit_is_missing_data() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec amounts
uses lemma units
data gbp_rate: number
data money: measure
  -> unit eur: 1
  -> unit gbp: gbp_rate
data price_per_mass: measure
  -> unit eur_per_kg: eur/kilogram
  -> unit gbp_per_kg: gbp/kilogram
data price: price_per_mass
rule in_eur: price as eur_per_kg
"#,
    );
    let response = run(
        &engine,
        "amounts",
        data(&[("price", "1 gbp_per_kg")]),
        false,
    );
    assert_missing_scale(&response, "in_eur", "gbp_rate");
}

#[test]
fn unbound_scale_uses_read_without_arithmetic_is_missing_data() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec priced
uses lemma units
data rate: number
data money: measure
  -> unit eur: 1
  -> unit gbp: rate
data amount: money

spec consumer
uses p: priced
rule out: p.amount
"#,
    );
    let response = run(&engine, "consumer", data(&[("p.amount", "1 gbp")]), false);
    assert_missing_scale(&response, "out", "rate");
}

#[test]
fn unbound_scale_data_reference_chain_is_missing_data() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
repo r

spec src
uses lemma units
data rate: number
data money: measure
  -> unit eur: 1
  -> unit gbp: rate
data amount: 100 gbp

spec fx
uses s: src
rule total: s.amount
"#,
    );
    let response = run_in_repo(&engine, Some("r"), "fx", IndexMap::new(), false);
    assert_missing_scale(&response, "total", "rate");
}

#[test]
fn unbound_scale_comparison_is_missing_data() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec fx
uses lemma units
data rate: number
data money: measure
  -> unit eur: 1
  -> unit gbp: rate
data amount: money
rule cmp: amount > 1 eur
"#,
    );
    let response = run(&engine, "fx", data(&[("amount", "1 gbp")]), false);
    assert_missing_scale(&response, "cmp", "rate");
}

#[test]
fn maximum_in_dynamic_unit_enforces_once_rate_settled() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec fx
uses lemma units
data rate: number
data money: measure
  -> unit eur: 1
  -> unit gbp: rate
  -> maximum 100 gbp
data amount: money
rule out: amount
"#,
    );
    let over = run(
        &engine,
        "fx",
        data(&[("rate", "0.86"), ("amount", "200 gbp")]),
        false,
    );
    let r = rule(&over, "out");
    assert!(r.vetoed, "amount above maximum 100 gbp must veto");
    assert!(!r.is_missing_data(), "maximum breach is not missing data");

    let ok = run(
        &engine,
        "fx",
        data(&[("rate", "0.86"), ("amount", "50 gbp")]),
        false,
    );
    let eur = measure_map(&ok, "out").get("eur").expect("eur");
    assert_eq!(eur.to_string(), "43");
}

#[test]
fn unbound_scale_with_maximum_in_dynamic_unit_is_missing_data() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec fx
uses lemma units
data rate: number
data money: measure
  -> unit eur: 1
  -> unit gbp: rate
  -> maximum 100 gbp
data amount: money
rule out: amount
"#,
    );
    let response = run(&engine, "fx", data(&[("amount", "50 gbp")]), false);
    assert_missing_scale(&response, "out", "rate");
}

#[test]
fn unbound_scale_with_minimum_in_dynamic_unit_is_missing_data() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec fx
uses lemma units
data rate: number
data money: measure
  -> unit eur: 1
  -> unit gbp: rate
  -> minimum 10 gbp
data amount: money
rule out: amount
"#,
    );
    let response = run(&engine, "fx", data(&[("amount", "50 gbp")]), false);
    assert_missing_scale(&response, "out", "rate");
}

#[test]
fn unbound_scale_abs_is_missing_data() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec fx
uses lemma units
data rate: number
data money: measure
  -> unit eur: 1
  -> unit gbp: rate
rule total: abs(-50 gbp)
"#,
    );
    let response = run(&engine, "fx", IndexMap::new(), false);
    assert_missing_scale(&response, "total", "rate");
}

#[test]
fn unbound_scale_step_rounding_is_missing_data() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec fx
uses lemma units
data rate: number
data money: measure
  -> unit eur: 1
  -> unit gbp: rate
rule total: 100 eur -> round 1 gbp
"#,
    );
    let response = run(&engine, "fx", IndexMap::new(), false);
    assert_missing_scale(&response, "total", "rate");
}

#[test]
fn unbound_scale_nonempty_list_sum_is_missing_data() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec fx
uses lemma units
data rate: number
data money: measure
  -> unit eur: 1
  -> unit gbp: rate
data items: money list
rule total: items -> sum
"#,
    );
    let mut request = IndexMap::new();
    request.insert(
        "items".to_string(),
        EvaluateDataValue::List(vec![
            EvaluateDataValue::string("50 gbp"),
            EvaluateDataValue::string("50 gbp"),
        ]),
    );
    let response = run(&engine, "fx", request, false);
    assert_missing_scale(&response, "total", "rate");
}

#[test]
fn unbound_scale_on_one_list_row_is_missing_data() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
repo fx_repo

spec line
uses lemma units
data rate: number
data money: measure
  -> unit eur: 1
  -> unit gbp: rate
data amount: money
rule in_eur: amount

spec order
uses line
data lines: line list
  -> with rate: 0.86, amount: 100 gbp
  -> with amount: 100 gbp
rule total: lines -> sum in_eur
"#,
    );
    let response = run_in_repo(&engine, Some("fx_repo"), "order", IndexMap::new(), false);
    assert_missing_scale(&response, "total", "rate");
}

#[test]
fn unbound_scale_milliscale_compound_is_missing_data() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec amounts
uses lemma units
data gbp_rate: number
data money: measure
  -> unit eur: 1
  -> unit gbp: gbp_rate
data price_per_mass: measure
  -> unit eur_per_kg: eur/kilogram
  -> unit gbp_per_kg: gbp/kilogram
  -> unit milli_gbp_per_kg: 0.001 gbp/kilogram
rule tiny: 1000 milli_gbp_per_kg
"#,
    );
    let response = run(&engine, "amounts", IndexMap::new(), false);
    assert_missing_scale(&response, "tiny", "gbp_rate");
}

#[test]
fn vetoed_scale_on_cast_propagates_veto() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec amounts
uses lemma units
data money: measure
  -> unit eur: 1
  -> unit gbp: gbp_rate
data blocked: true
rule gbp_rate: 0.86 unless blocked then veto "rate unavailable"
data amount: money
rule in_eur: amount as eur
"#,
    );
    let response = run(&engine, "amounts", data(&[("amount", "1.2 gbp")]), false);
    let r = rule(&response, "in_eur");
    assert!(r.vetoed, "vetoed scale must veto cast");
    assert!(!r.is_missing_data(), "vetoed scale is not missing data");
    assert_eq!(
        r.veto_reason.as_deref(),
        Some("rate unavailable"),
        "veto reason from the scale rule must surface"
    );
    assert!(r.result.is_none(), "vetoed cast must not yield a value");
}

#[test]
fn vetoed_scale_on_uses_imported_add_propagates_veto() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec amounts
uses lemma units
data blocked: true
rule gbp_rate: 0.86 unless blocked then veto "rate unavailable"
data money: measure
  -> unit eur: 1
  -> unit gbp: gbp_rate

spec left_side
uses amounts
data amount: amounts.money
rule out: amount

spec right_side
uses amounts
data amount: amounts.money
rule out: amount

spec bundle
uses l: left_side
uses r: right_side
rule total: l.out + r.out
"#,
    );
    let response = run(
        &engine,
        "bundle",
        data(&[("l.amount", "1 gbp"), ("r.amount", "2 gbp")]),
        false,
    );
    let r = rule(&response, "total");
    assert!(r.vetoed, "vetoed scale must veto uses-add");
    assert!(!r.is_missing_data(), "vetoed scale is not missing data");
    assert_eq!(
        r.veto_reason.as_deref(),
        Some("rate unavailable"),
        "veto reason from the scale rule must surface"
    );
    assert!(r.result.is_none(), "vetoed add must not yield a value");
}

/// Qualified parent `lines.money` when `lines` is an instance list (not a `uses`
/// alias) is a planning Error — not a silent accept with wrong hop rules.
#[test]
fn qualified_parent_on_instance_list_alias_is_planning_error() {
    let mut engine = Engine::new();
    let msg = load_err(
        &mut engine,
        r#"
repo r

spec line
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate

spec order
uses line
data lines: line list
    -> with rate: 0.5, money: 10 eur
data y: lines.money
"#,
    );
    assert!(
        msg.contains("'lines' is not a `uses` import alias"),
        "qualified parent on a list name must Error, got: {msg}"
    );
}

/// Parent converts a row field to a dynamic unit owned by a nested `uses`
/// (`lines.fx.usd`). The scale is not forcible on the parent plan; the factor
/// must come from the row-carried instance factors (20 usd at rate 0.5).
#[test]
fn parent_converts_row_field_to_nested_uses_dynamic_unit() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
repo r

spec fx
data rate: number
data money: measure
    -> unit eur: 1
    -> unit usd: rate

spec line
uses fx
    -> with rate: 0.5
data amount: fx.money

spec order
uses line
data lines: line list
    -> with amount: 10 eur
rule in_usd: (lines -> first).amount as lines.fx.usd
"#,
    );
    let response = run_in_repo(&engine, Some("r"), "order", IndexMap::new(), false);
    let r = rule(&response, "in_usd");
    assert!(
        !r.vetoed,
        "parent conversion through the row's nested fx must settle, veto={:?} reason={:?}",
        r.vetoed, r.veto_reason
    );
    let m = measure_map(&response, "in_usd");
    assert_eq!(m.get("eur").expect("eur").to_string(), "10");
    assert_eq!(
        m.get("usd").expect("usd").to_string(),
        "20",
        "10 eur at row fx rate 0.5 is 20 usd"
    );
    assert_eq!(value_of(&response, "in_usd").unit.as_deref(), Some("usd"));
}

/// Two row reads with different rates drop `gbp` from instance factors. Wire
/// `unit` must match the unit used in the result string (not a stale `gbp` binding).
#[test]
fn cross_row_add_wire_unit_matches_display_when_gbp_factor_dropped() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
repo r

spec line
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
data amount: money

spec order
uses line
data lines: line list
    -> with rate: 0.86, amount: 100 gbp
    -> with rate: 0.5, amount: 100 gbp
rule combined: (lines -> first).amount + (lines -> last).amount
"#,
    );
    let response = run_in_repo(&engine, Some("r"), "order", IndexMap::new(), false);
    let value = value_of(&response, "combined");
    let m = measure_map(&response, "combined");
    assert_eq!(m.get("eur").expect("eur").to_string(), "136");
    assert!(
        !m.contains_key("gbp"),
        "disagreed row rates must drop gbp from the measure map: {m:?}"
    );
    let result = value.result.as_deref().expect("result string");
    assert!(
        result.ends_with(" eur"),
        "display must fall back to a factor-known unit, got {result}"
    );
    assert_ne!(
        value.unit.as_deref(),
        Some("gbp"),
        "wire unit must not stay gbp when gbp has no factor; result={result} unit={:?}",
        value.unit
    );
    if let Some(unit) = value.unit.as_deref() {
        assert!(
            result.ends_with(&format!(" {unit}")),
            "wire unit {unit} must match the unit in result {result}"
        );
    }
}

/// Row-carried factors + local literal (`instance_factors: None`) must not keep
/// the row's dynamic unit as if the local side agreed.
#[test]
fn row_plus_local_literal_does_not_inherit_row_gbp_factor() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
repo r

spec line
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
data amount: money

spec order
uses line
data lines: line list
    -> with rate: 0.86, amount: 100 gbp
rule mixed: (lines -> first).amount + 10 eur
"#,
    );
    let response = run_in_repo(&engine, Some("r"), "order", IndexMap::new(), false);
    let m = measure_map(&response, "mixed");
    assert_eq!(m.get("eur").expect("eur").to_string(), "96");
    assert!(
        !m.contains_key("gbp"),
        "a local literal has no gbp factor; mixed must not inherit the row's gbp: {m:?}"
    );
}

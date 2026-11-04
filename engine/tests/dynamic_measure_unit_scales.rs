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
fn dynamic_unit_rejected_in_compound_unit() {
    let mut engine = Engine::new();
    let code = r#"
spec t
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
data other: measure
    -> unit piece: 1
    -> unit gbp_squared: gbp * gbp
"#;
    let msg = load_err(&mut engine, code);
    assert!(
        msg.contains("gbp") && msg.contains("conversion factor is a reference"),
        "compound unit with dynamic factor must name gbp and reject the reference, got: {msg}"
    );
}

#[test]
fn dynamic_unit_rejected_in_minimum() {
    let mut engine = Engine::new();
    let code = r#"
spec t
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
    -> minimum 0 gbp
"#;
    let msg = load_err(&mut engine, code);
    assert!(
        msg.contains("gbp")
            && msg.contains("minimum")
            && msg.contains("conversion factor is a reference"),
        "minimum written in a dynamic unit must name gbp, got: {msg}"
    );
}

#[test]
fn dynamic_unit_rejected_in_maximum() {
    let mut engine = Engine::new();
    let code = r#"
spec t
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
    -> maximum 100 gbp
"#;
    let msg = load_err(&mut engine, code);
    assert!(
        msg.contains("gbp")
            && msg.contains("maximum")
            && msg.contains("conversion factor is a reference"),
        "maximum written in a dynamic unit must name gbp, got: {msg}"
    );
}

#[test]
fn dynamic_unit_rejected_in_suggest() {
    let mut engine = Engine::new();
    let code = r#"
spec t
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
    -> suggest 0 gbp
"#;
    let msg = load_err(&mut engine, code);
    assert!(
        msg.contains("gbp")
            && msg.contains("suggest")
            && msg.contains("conversion factor is a reference"),
        "suggest written in a dynamic unit must name gbp, got: {msg}"
    );
}

#[test]
fn dynamic_unit_rejected_in_option() {
    let mut engine = Engine::new();
    let code = r#"
spec t
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
    -> option 10 gbp
"#;
    let msg = load_err(&mut engine, code);
    assert!(
        msg.contains("gbp") && msg.contains("conversion factor is a reference"),
        "option written in a dynamic unit must name gbp, got: {msg}"
    );
}

#[test]
fn dynamic_unit_rejected_in_option_on_data_slot() {
    let mut engine = Engine::new();
    let code = r#"
spec t
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
data amount: money
    -> option 10 gbp
"#;
    let msg = load_err(&mut engine, code);
    assert!(
        msg.contains("gbp") && msg.contains("conversion factor is a reference"),
        "option on a data slot written in a dynamic unit must name gbp, got: {msg}"
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
    let r = rule(&response, "total");
    assert!(r.vetoed, "unbound scale must veto");
    assert!(r.is_missing_data(), "must be missing-data veto");
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
    let r = rule(&response, "total");
    assert!(
        r.vetoed,
        "all-dynamic family rule must veto when scale missing"
    );
    assert!(r.is_missing_data());
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
    assert!(m.contains_key("gbp"), "gbp column must be present");
    assert!(m.contains_key("usd"), "usd column must be present");
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
        msg.contains("Circular dependency (rules: out)"),
        "cycle through a uses binding must be a planning error, got: {msg}"
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
        assert!(
            !m.contains_key("gbp"),
            "gbp takes its factor from config, not fx: {name} {m:?}"
        );
        assert!(
            explanation_text(&response, name).contains("4.3 eur"),
            "{name}"
        );
    }
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
"#,
    );
    let response = run_in_repo(&engine, Some("fx_repo"), "order", IndexMap::new(), true);
    let expected = [
        ("total", "136", None),
        ("first_line", "86", Some("eur")),
        ("first_amount", "86", Some("eur")),
    ];
    for (name, eur, unit) in expected {
        let m = measure_map(&response, name);
        assert_eq!(m.get("eur").expect("eur").to_string(), eur, "{name}");
        assert!(
            !m.contains_key("gbp"),
            "each row owns its gbp factor; order has none: {name} {m:?}"
        );
        let value = value_of(&response, name);
        assert_eq!(value.result.as_deref(), Some(format!("{eur} eur").as_str()));
        assert_eq!(
            value.unit.as_deref(),
            unit,
            "a written unit is reported as the unit of the result: {name}"
        );
        assert!(
            explanation_text(&response, name).contains(&format!("{name}: {eur} eur")),
            "{name}"
        );
    }
    for name in ["cheap", "cheap_by_rule"] {
        let count = value_of(&response, name).number.as_ref().expect("number");
        assert_eq!(count.to_string(), "1", "{name}");
        assert!(
            explanation_text(&response, name).contains("2. amount: 100 gbp"),
            "a row presents its own field in its own unit: {name}"
        );
    }
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
fn foreign_dynamic_unit_literal_is_planning_error() {
    let mut engine = Engine::new();
    let code = r#"
repo r

spec priced
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate

spec fx
uses p: priced
rule total: 10 gbp
"#;
    let msg = load_err(&mut engine, code);
    assert!(
        msg.contains("unit 'gbp' takes its conversion factor from spec 'priced' and cannot be used in spec 'fx'"),
        "got: {msg}"
    );
}

#[test]
fn foreign_dynamic_unit_conversion_is_planning_error() {
    let mut engine = Engine::new();
    let code = r#"
repo r

spec priced
data rate: number
data money: measure
    -> unit eur: 1
    -> unit gbp: rate
data amount: money

spec fx
uses p: priced
rule total: p.amount as gbp
"#;
    let msg = load_err(&mut engine, code);
    assert!(
        msg.contains("unit 'gbp' takes its conversion factor from spec 'priced' and cannot be used in spec 'fx'"),
        "got: {msg}"
    );
}

const FOREIGN_ALL_DYNAMIC_VETO: &str =
    "unit 'gbp' takes its conversion factor from spec 'priced' and cannot be used in spec 'fx'";

#[test]
fn foreign_all_dynamic_value_vetoes_in_importing_spec() {
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
    for name in ["total", "twice", "big", "first_amount"] {
        let r = rule(&response, name);
        assert!(r.vetoed, "{name} must veto");
        assert_eq!(
            r.veto_reason.as_deref(),
            Some(FOREIGN_ALL_DYNAMIC_VETO),
            "{name}"
        );
        assert!(
            explanation_text(&response, name).contains(FOREIGN_ALL_DYNAMIC_VETO),
            "{name}"
        );
    }
    let count = value_of(&response, "line_count")
        .number
        .as_ref()
        .expect("number");
    assert_eq!(count.to_string(), "1");
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
    let r = rule(&response, "total");
    assert!(r.vetoed, "an unscalable all-dynamic value must veto");
    assert!(r.is_missing_data(), "got {:?}", r.veto_reason);
}

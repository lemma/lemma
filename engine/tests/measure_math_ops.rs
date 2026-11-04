//! Math on measure operands: `abs` keeps the measure, `-> round/floor/ceil step` rounds it,
//! and `-> round` without a step on a measure or ratio points to a unit step.

use indexmap::IndexMap;
use lemma::{Engine, EvaluateDataValue, SourceType};
use std::path::PathBuf;
use std::sync::Arc;

fn source() -> SourceType {
    SourceType::Path(Arc::new(PathBuf::from("measure_math_ops.lemma")))
}

fn load_err(code: &str) -> String {
    let mut engine = Engine::new();
    match engine.load([(source(), code.to_string())]) {
        Ok(()) => panic!("expected planning error"),
        Err(errors) => errors
            .iter()
            .map(|error| error.to_string())
            .collect::<Vec<_>>()
            .join("; "),
    }
}

fn eval_display(
    code: &str,
    spec: &str,
    rule: &str,
    data: IndexMap<String, EvaluateDataValue>,
) -> String {
    let mut engine = Engine::new();
    engine
        .load([(source(), code.to_string())])
        .unwrap_or_else(|errors| panic!("spec must load: {errors:?}"));
    let response = engine
        .evaluate(None, spec, None, data, None, false)
        .expect("spec must evaluate");
    let result = response
        .results
        .get(rule)
        .unwrap_or_else(|| panic!("rule '{rule}' missing"));
    if result.vetoed {
        panic!(
            "rule '{rule}' vetoed: {}",
            result.veto_reason.as_deref().unwrap_or("Vetoed")
        );
    }
    result
        .result()
        .expect("rule must have display value")
        .to_string()
}

#[test]
fn ceil_duration_to_whole_weeks() {
    let code = r#"spec duration_math
uses lemma units
data storage_duration: 10 day
rule weeks_billed: storage_duration -> ceil 1 week"#;
    assert_eq!(
        eval_display(code, "duration_math", "weeks_billed", IndexMap::new()),
        "2 week"
    );
}

#[test]
fn warehousing_storage_cost_with_ceil_weeks() {
    let code = r#"spec warehousing
uses lemma units
data money: measure -> unit eur: 1
data rate: measure -> unit eur_per_week: eur/week
data storage_per_pallet_per_week: 10 eur_per_week
data storage_duration: 10 day
rule storage_cost_per_pallet:
  storage_per_pallet_per_week * (storage_duration -> ceil 1 week)"#;
    let displayed = eval_display(
        code,
        "warehousing",
        "storage_cost_per_pallet",
        IndexMap::new(),
    );
    assert!(
        displayed.contains("20") && displayed.to_lowercase().contains("eur"),
        "10 eur/week * (10 day -> ceil 1 week) must be 20 eur, got: {displayed}"
    );
}

#[test]
fn floor_round_abs_on_measure() {
    let code = r#"spec mass_math
uses lemma units
data weight: 2.6 kilogram
rule floored: weight -> floor 1 kilogram
rule rounded: weight -> round 1 kilogram
rule absolute: abs (0 kilogram - weight)"#;
    assert_eq!(
        eval_display(code, "mass_math", "floored", IndexMap::new()),
        "2 kilogram"
    );
    assert_eq!(
        eval_display(code, "mass_math", "rounded", IndexMap::new()),
        "3 kilogram"
    );
    assert_eq!(
        eval_display(code, "mass_math", "absolute", IndexMap::new()),
        "2.6 kilogram"
    );
}

#[test]
fn sqrt_on_measure_rejected_at_plan() {
    let code = r#"spec bad_math
uses lemma units
data storage_duration: 10 day
rule bad: sqrt (storage_duration as week)"#;
    let error = load_err(code);
    assert!(
        error.to_lowercase().contains("sqrt") && error.to_lowercase().contains("number"),
        "sqrt on measure must fail at plan time, got: {error}"
    );
}

#[test]
fn ceil_velocity_quotient_to_a_step_of_the_stdlib_unit() {
    let code = r#"spec compound_ceil
uses lemma units
data dist: 100 meter
data elapsed: 20 second
rule speed:
  dist / elapsed -> ceil 1 meter_per_second"#;
    assert_eq!(
        eval_display(code, "compound_ceil", "speed", IndexMap::new()),
        "5 meter_per_second"
    );
}

const MASS: &str = "data mass: measure -> unit kilogram: 1 -> unit gram: 0.001";

fn assert_points_to_step(error: &str, operator: &str, kind: &str, suggestion: &str) {
    assert!(
        error.contains(&format!("Cannot use '{operator}' on {kind} '"))
            && error.contains("Round to a step instead"),
        "'{operator}' on a {kind} must be a planning error, got: {error}"
    );
    assert!(
        error.contains(&format!("for example '{suggestion}'")),
        "error must suggest '{suggestion}', got: {error}"
    );
}

#[test]
fn bare_round_on_measure_data_points_to_step() {
    let error = load_err(&format!(
        "spec s\n{MASS}\ndata weight: mass\nrule r: weight -> round"
    ));
    assert_points_to_step(&error, "round", "measure", "weight -> round 1 kilogram");
}

#[test]
fn bare_floor_on_measure_literal_points_to_step() {
    let error = load_err(&format!(
        "spec s\n{MASS}\ndata weight: 2.6 kilogram\nrule r: weight -> floor"
    ));
    assert_points_to_step(&error, "floor", "measure", "weight -> floor 1 kilogram");
}

#[test]
fn bare_round_on_measure_arithmetic_points_to_step() {
    let error = load_err(&format!(
        "spec s\n{MASS}\ndata weight: mass\nrule r: weight * 100 -> round"
    ));
    assert_points_to_step(
        &error,
        "round",
        "measure",
        "weight * 100 -> round 1 kilogram",
    );
}

#[test]
fn bare_round_on_a_converted_rule_reference_points_to_step() {
    let error = load_err(&format!(
        "spec s\n{MASS}\ndata weight: mass\nrule a: weight as kilogram\nrule r: a -> round"
    ));
    assert_points_to_step(&error, "round", "measure", "a -> round 1 kilogram");
}

#[test]
fn bare_ceil_on_a_compound_quotient_points_to_step() {
    let error = load_err(
        r#"spec compound_ceil
uses lemma units
data dist: 100 meter
data elapsed: 20 second
rule speed: dist / elapsed -> ceil"#,
    );
    assert!(
        error.contains("Cannot use 'ceil' on measure '")
            && error.contains("'dist / elapsed -> ceil 1 "),
        "ceil on a quotient must point to the step form, got: {error}"
    );
}

#[test]
fn bare_ceil_on_a_quotient_with_no_declared_unit_names_its_dimensions() {
    let error = load_err(
        r#"spec s
data length: measure -> unit meter: 1
data duration: measure -> unit second: 1
data dist: 100 meter
data elapsed: 20 second
rule speed: dist / elapsed -> ceil"#,
    );
    assert!(
        error.contains("Cannot use 'ceil' on 'dist / elapsed'. Round to a step instead")
            && error.contains("in a unit with dimensions")
            && !error.contains("<unit>"),
        "ceil on a quotient no declared unit covers must name its dimensions, got: {error}"
    );
}

#[test]
fn bare_round_on_ratio_data_points_to_step() {
    let error = load_err("spec s\ndata rate: ratio\nrule r: rate -> round");
    assert_points_to_step(&error, "round", "ratio", "rate -> round 1 percent");
}

#[test]
fn prefix_round_is_a_parse_error() {
    let error = load_err(&format!(
        "spec s\n{MASS}\ndata weight: mass\nrule r: round(weight)"
    ));
    assert!(
        error.contains("`round` is written after `->`") && error.contains("`weight -> round`"),
        "{error}"
    );
}

#[test]
fn round_measure_arithmetic_to_a_step() {
    let code = format!(
        "spec s\n{MASS}\ndata weight: 0.026 kilogram\nrule r: weight * 100 -> round 1 kilogram"
    );
    assert_eq!(eval_display(&code, "s", "r", IndexMap::new()), "3 kilogram");
}

#[test]
fn round_supplied_data_to_a_step_writes_the_step_unit() {
    let code = format!("spec s\n{MASS}\ndata weight: mass\nrule r: weight -> round 1 kilogram");
    let mut data = IndexMap::new();
    data.insert(
        "weight".to_string(),
        EvaluateDataValue::Literal("2600 gram".to_string()),
    );
    assert_eq!(eval_display(&code, "s", "r", data), "3 kilogram");
}

#[test]
fn round_supplied_data_to_a_gram_step() {
    let code = format!("spec s\n{MASS}\ndata weight: mass\nrule r: weight -> round 1 gram");
    let mut data = IndexMap::new();
    data.insert(
        "weight".to_string(),
        EvaluateDataValue::Literal("2.6 kilogram".to_string()),
    );
    assert_eq!(eval_display(&code, "s", "r", data), "2600 gram");
}

#[test]
fn round_ratio_to_percent_and_permille_steps() {
    let code = "spec s\ndata r: 12.55 percent\n\
                rule in_percent: r -> round(1 percent) is 13 percent\n\
                rule in_permille: r -> round(1 permille) is 126 permille";
    assert_eq!(
        eval_display(code, "s", "in_percent", IndexMap::new()),
        "true"
    );
    assert_eq!(
        eval_display(code, "s", "in_permille", IndexMap::new()),
        "true"
    );
}

#[test]
fn abs_on_measure_arithmetic_needs_no_unit() {
    let code =
        format!("spec s\n{MASS}\ndata weight: 2.6 kilogram\nrule r: abs(weight - 3 kilogram)");
    assert_eq!(
        eval_display(&code, "s", "r", IndexMap::new()),
        "0.4 kilogram"
    );
}

#[test]
fn ceil_negative_duration_to_whole_weeks() {
    let code = r#"spec negative_duration
uses lemma units
data storage_duration: -10 day
rule weeks_billed: storage_duration -> ceil 1 week"#;
    assert_eq!(
        eval_display(code, "negative_duration", "weeks_billed", IndexMap::new()),
        "-1 week"
    );
}

//! Magnitude-preserving math (`ceil`, `floor`, `round`, `abs`) on measure operands.

use lemma::{Engine, SourceType};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

fn source() -> SourceType {
    SourceType::Path(Arc::new(PathBuf::from("measure_math_ops.lemma")))
}

fn load_ok(code: &str) {
    let mut engine = Engine::new();
    engine
        .load([(source(), code.to_string())])
        .unwrap_or_else(|errors| panic!("spec must load: {errors:?}"));
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

fn eval_display(code: &str, spec: &str, rule: &str, data: HashMap<String, String>) -> String {
    let mut engine = Engine::new();
    engine
        .load([(source(), code.to_string())])
        .expect("spec must load");
    let response = engine
        .run(None, spec, None, data, None, false)
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
fn ceil_duration_as_weeks() {
    let code = r#"spec duration_math
uses lemma units
data storage_duration: 10 day
rule weeks_billed: ceil (storage_duration as week)"#;
    load_ok(code);
    let displayed = eval_display(code, "duration_math", "weeks_billed", HashMap::new());
    assert!(
        displayed.contains('2') && displayed.to_lowercase().contains("week"),
        "10 day as week ceiled must be 2 week, got: {displayed}"
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
  storage_per_pallet_per_week * ceil (storage_duration as week)"#;
    load_ok(code);
    let displayed = eval_display(
        code,
        "warehousing",
        "storage_cost_per_pallet",
        HashMap::new(),
    );
    assert!(
        displayed.contains("20") && displayed.to_lowercase().contains("eur"),
        "10 eur/week * ceil(10 day as week) must be 20 eur, got: {displayed}"
    );
}

#[test]
fn floor_round_abs_on_measure() {
    let code = r#"spec mass_math
uses lemma units
data mass: measure -> unit kilogram: 1
data weight: 2.6 kilogram
rule floored: floor weight
rule rounded: round weight
rule absolute: abs (0 kilogram - weight)"#;
    load_ok(code);
    assert_eq!(
        eval_display(code, "mass_math", "floored", HashMap::new()),
        "2 kilogram"
    );
    assert_eq!(
        eval_display(code, "mass_math", "rounded", HashMap::new()),
        "3 kilogram"
    );
    assert_eq!(
        eval_display(code, "mass_math", "absolute", HashMap::new()),
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
fn ceil_velocity_compound_at_rule_boundary_promotes_to_stdlib() {
    let code = r#"spec compound_ceil
uses lemma units
data dist: 100 meter
data elapsed: 20 second
rule speed:
  ceil (dist / elapsed)"#;
    load_ok(code);
    let displayed = eval_display(code, "compound_ceil", "speed", HashMap::new());
    assert!(
        displayed.contains('5') && displayed.to_lowercase().contains("meter_per_second"),
        "ceil(100 meter / 20 second) must be 5 meter_per_second via units.velocity, got: {displayed}"
    );
}

#[test]
fn ceil_negative_duration_as_weeks() {
    let code = r#"spec negative_duration
uses lemma units
data storage_duration: -10 day
rule weeks_billed: ceil (storage_duration as week)"#;
    load_ok(code);
    let displayed = eval_display(code, "negative_duration", "weeks_billed", HashMap::new());
    assert!(
        displayed.contains("-1") && displayed.to_lowercase().contains("week"),
        "-10 day as week ceiled must be -1 week, got: {displayed}"
    );
}

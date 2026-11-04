//! Postfix rounding: `x -> round`, `x -> floor <step>`, `x -> ceil <step>`.

use indexmap::IndexMap;
use lemma::{Engine, EvaluateDataValue, SourceType};
use std::path::PathBuf;
use std::sync::Arc;

fn source() -> SourceType {
    SourceType::Path(Arc::new(PathBuf::from("step_rounding.lemma")))
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

fn eval_display_with(
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

fn eval_display(code: &str, spec: &str, rule: &str) -> String {
    eval_display_with(code, spec, rule, IndexMap::new())
}

const RECIPE: &str = r#"spec recipe
uses lemma units
data money: measure -> unit eur: 1
data weight_price: measure -> unit eur_per_kg: eur/kilogram
data material_cost: 10 eur
data process_cost: 2.5 eur
data mix_cost: 0.3 eur
data desired_out_weight: 3 kilogram
rule price_per_kg:
  (material_cost + process_cost + mix_cost) / desired_out_weight -> round 0.001 eur_per_kg
rule price_per_kg_number:
  ((material_cost + process_cost + mix_cost) / desired_out_weight -> round 0.001 eur_per_kg) as number
rule price_per_kg_down:
  (material_cost + process_cost + mix_cost) / desired_out_weight -> floor 0.01 eur_per_kg
"#;

#[test]
fn round_a_quotient_to_a_step_in_the_step_unit() {
    assert_eq!(
        eval_display(RECIPE, "recipe", "price_per_kg"),
        "4.267 eur_per_kg"
    );
}

#[test]
fn floor_a_quotient_to_a_step_in_the_step_unit() {
    assert_eq!(
        eval_display(RECIPE, "recipe", "price_per_kg_down"),
        "4.26 eur_per_kg"
    );
}

#[test]
fn rounded_result_is_bound_to_the_step_unit() {
    assert_eq!(
        eval_display(RECIPE, "recipe", "price_per_kg_number"),
        "4.267"
    );
}

#[test]
fn round_a_sum_of_measures_to_a_cash_step() {
    let code = r#"spec kassa
data money: measure -> unit eur: 1
data netto: 10.02 eur
data btw: 2.1042 eur
rule totaal: netto + btw -> round(0.05 eur) is 12.10 eur
rule omhoog: netto + btw -> ceil(0.05 eur) is 12.15 eur
"#;
    assert_eq!(eval_display(code, "kassa", "totaal"), "true");
    assert_eq!(eval_display(code, "kassa", "omhoog"), "true");
}

#[test]
fn ceil_a_duration_to_quarter_hours() {
    let code = r#"spec uren
uses lemma units
data gewerkt: 52 minute
rule gefactureerd: gewerkt -> ceil(15 minute) is 1 hour
"#;
    assert_eq!(eval_display(code, "uren", "gefactureerd"), "true");
}

#[test]
fn step_rounding_on_numbers() {
    let code = r#"spec getallen
data x: 2.345
data half: 2.5
data min_half: -2.5
rule omlaag: x -> floor 0.01
rule omhoog: x -> ceil 0.01
rule afgerond: x -> round 0.01
rule half_weg: half -> round 1
rule min_half_weg: min_half -> round 1
rule vijftallen: 17 -> round 5
"#;
    assert_eq!(eval_display(code, "getallen", "omlaag"), "2.34");
    assert_eq!(eval_display(code, "getallen", "omhoog"), "2.35");
    assert_eq!(eval_display(code, "getallen", "afgerond"), "2.35");
    assert_eq!(eval_display(code, "getallen", "half_weg"), "3");
    assert_eq!(eval_display(code, "getallen", "min_half_weg"), "-3");
    assert_eq!(eval_display(code, "getallen", "vijftallen"), "15");
}

#[test]
fn integer_rounding_without_a_step() {
    let code = r#"spec getallen
data half: 2.5
data min_half: -2.5
data x: 2.345
rule afgerond: half -> round
rule min_afgerond: min_half -> round
rule omlaag: x -> floor
rule omhoog: x -> ceil
rule groter: half / 1 -> round > 2
"#;
    assert_eq!(eval_display(code, "getallen", "afgerond"), "3");
    assert_eq!(eval_display(code, "getallen", "min_afgerond"), "-3");
    assert_eq!(eval_display(code, "getallen", "omlaag"), "2");
    assert_eq!(eval_display(code, "getallen", "omhoog"), "3");
    assert_eq!(eval_display(code, "getallen", "groter"), "true");
}

#[test]
fn step_rounding_on_ratios() {
    let code = r#"spec tarief
data r: 12.55 percent
rule hele_procenten: r -> round(1 percent) is 13 percent
rule tienden: r -> round(0.1%) is 12.6 percent
rule promille: r -> floor(1 permille) is 125 permille
"#;
    assert_eq!(eval_display(code, "tarief", "hele_procenten"), "true");
    assert_eq!(eval_display(code, "tarief", "tienden"), "true");
    assert_eq!(eval_display(code, "tarief", "promille"), "true");
}

#[test]
fn step_rounding_on_supplied_data_rounds_in_the_step_unit() {
    let code = r#"spec gewicht
uses lemma units
data weight: units.mass
rule in_kilogram: weight -> round 1 kilogram
rule in_gram: weight -> round 1 gram
"#;
    let mut data = IndexMap::new();
    data.insert(
        "weight".to_string(),
        EvaluateDataValue::Literal("2600.4 gram".to_string()),
    );
    assert_eq!(
        eval_display_with(code, "gewicht", "in_kilogram", data.clone()),
        "3 kilogram"
    );
    assert_eq!(
        eval_display_with(code, "gewicht", "in_gram", data),
        "2600 gram"
    );
}

#[test]
fn step_of_another_measure_family_is_a_planning_error() {
    let error = load_err(
        r#"spec s
uses lemma units
data money: measure -> unit eur: 1
data weight: 2 kilogram
rule r: weight -> round 0.01 eur"#,
    );
    assert!(
        error.contains("Cannot round") && error.contains("to a step of"),
        "{error}"
    );
}

#[test]
fn number_step_on_a_measure_is_a_planning_error() {
    let error = load_err(
        r#"spec s
uses lemma units
data weight: 2 kilogram
rule r: weight -> round 0.01"#,
    );
    assert!(
        error.contains("Cannot round") && error.contains("to a step of"),
        "{error}"
    );
}

#[test]
fn measure_step_on_a_number_is_a_planning_error() {
    let error = load_err(
        r#"spec s
uses lemma units
data x: 2.5
rule r: x -> round 1 kilogram"#,
    );
    assert!(
        error.contains("Cannot round") && error.contains("to a step of"),
        "{error}"
    );
}

#[test]
fn step_on_text_is_a_planning_error() {
    let error = load_err(
        r#"spec s
data name: "ada"
rule r: name -> round 1"#,
    );
    assert!(error.contains("Cannot round"), "{error}");
}

#[test]
fn measure_without_a_step_needs_a_unit() {
    let error = load_err(
        r#"spec s
data money: measure -> unit eur: 1
data price: 2.6 eur
rule r: price -> round"#,
    );
    assert!(
        error.contains("Cannot use 'round' on measure") && error.contains("'price -> round 1 eur'"),
        "{error}"
    );
}

#[test]
fn ratio_without_a_step_needs_a_unit() {
    let error = load_err(
        r#"spec s
data rate: 12.5 percent
rule r: rate -> floor"#,
    );
    assert!(
        error.contains("Cannot use 'floor' on ratio") && error.contains("-> floor 1"),
        "{error}"
    );
}

#[test]
fn zero_step_is_a_planning_error() {
    let error = load_err(
        r#"spec s
data money: measure -> unit eur: 1
data price: 2.5 eur
rule r: price -> round 0 eur"#,
    );
    assert!(error.contains("must be greater than zero"), "{error}");
}

#[test]
fn negative_step_is_a_planning_error() {
    let error = load_err(
        r#"spec s
data money: measure -> unit eur: 1
data price: 2.5 eur
rule r: price -> round(-0.05 eur)"#,
    );
    assert!(error.contains("must be greater than zero"), "{error}");
}

#[test]
fn step_from_data_is_a_planning_error() {
    let error = load_err(
        r#"spec s
data money: measure -> unit eur: 1
data price: 2.5 eur
data step: 0.05 eur
rule r: price -> round step"#,
    );
    assert!(error.contains("must be a literal"), "{error}");
}

#[test]
fn prefix_round_is_a_parse_error() {
    let error = load_err(
        r#"spec s
data x: 2.6
rule r: round(x)"#,
    );
    assert!(
        error.contains("`round` is written after `->`") && error.contains("`x -> round`"),
        "{error}"
    );
}

#[test]
fn prefix_floor_is_a_parse_error() {
    let error = load_err(
        r#"spec s
data x: 2.6
rule r: floor x"#,
    );
    assert!(
        error.contains("`floor` is written after `->`") && error.contains("`x -> floor`"),
        "{error}"
    );
}

#[test]
fn abs_on_a_measure_still_works() {
    let code = r#"spec s
uses lemma units
data weight: 2.6 kilogram
rule r: abs(weight - 3 kilogram)"#;
    assert_eq!(eval_display(code, "s", "r"), "0.4 kilogram");
}

//! Derived measure unit planning: `uses lemma units` duration units, compound-of-compound
//! resolution, topological ordering, and conversion-factor normalization.

use lemma::Engine;
use std::path::PathBuf;
use std::sync::Arc;

fn path_source(file: &str) -> lemma::SourceType {
    lemma::SourceType::Path(Arc::new(PathBuf::from(file)))
}

fn load_ok(code: &str, source_file: &str, message: &str) {
    let mut engine = Engine::new();
    engine
        .load([(path_source(source_file), code.to_string())])
        .expect(message);
}

fn expect_plan_error(code: &str, source_file: &str, expected_fragment: &str) {
    let mut engine = Engine::new();
    let result = engine.load([(path_source(source_file), code.to_string())]);
    assert!(
        result.is_err(),
        "expected planning to fail containing '{expected_fragment}'"
    );
    if expected_fragment.is_empty() {
        return;
    }
    let errors = result.unwrap_err();
    let combined = errors
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ");
    assert!(
        combined.contains(expected_fragment),
        "expected error containing '{expected_fragment}', got: {combined}"
    );
}

/// Mirrors `cli/tests/integrations/examples/03_spec_references.lemma` contractor block:
/// compound wage-rate units reference `second` and `hour` from `units.duration` via `uses lemma units`.
#[test]
fn uses_lemma_compound_wage_rate_units_plan_without_unknown_unit_error() {
    let code = r#"spec contractor
uses lemma units

data money: measure
  -> unit eur: 1.00

data wage_rate: measure
  -> unit eur_per_second: eur/second
  -> unit eur_per_hour: eur/hour

rule smoke: true
"#;
    load_ok(
        code,
        "contractor.lemma",
        "planning must accept eur/second and eur/hour when uses lemma units brings duration units",
    );
}

/// Fleet motor: EUR/hour premium rate, then EUR/hour per insured vehicle. Exercises compound
/// factors that name a unit from another derived measure (`eur_per_hour`) plus `vehicle`.
#[test]
fn uses_lemma_insurance_premium_per_vehicle_compound_plans() {
    let code = r#"spec fleet_motor_quote
uses lemma units

data money: measure
  -> unit eur: 1.00

data fleet_unit: measure
  -> unit vehicle: 1.00

data premium_rate: measure
  -> unit eur_per_second: eur/second
  -> unit eur_per_hour: eur/hour

data premium_per_vehicle: measure
  -> unit eur_hour_per_vehicle: eur_per_hour/vehicle

rule smoke: true
"#;
    load_ok(
        code,
        "fleet_motor_quote.lemma",
        "compound unit may name derived units from an earlier derived measure (eur_per_hour/vehicle)",
    );
}

/// Force = mass * acceleration; local force unit references compound unit `mps2`.
/// Names avoid stdlib `newton` / `units.force`.
#[test]
fn compound_newton_force_unit_plans() {
    let code = r#"spec mechanics
uses lemma units

data mass: measure
  -> unit kg: 1
  -> unit gram: 0.001

data length: measure
  -> unit meter: 1
  -> unit km: 1000

data acceleration: measure
  -> unit mps2: meter/second^2

data force: measure
  -> unit local_newton: kg * mps2

rule smoke: true
"#;
    load_ok(
        code,
        "mechanics_newton.lemma",
        "local_newton = kg * mps2 must plan with compound-of-compound normalization",
    );
}

/// Pressure = force / area; depth-3 compound chain (avoids stdlib `pascal` / `newton`).
#[test]
fn compound_pascal_pressure_unit_plans() {
    let code = r#"spec mechanics_pressure
uses lemma units

data mass: measure
  -> unit kg: 1

data length: measure
  -> unit meter: 1

data acceleration: measure
  -> unit mps2: meter/second^2

data force: measure
  -> unit local_newton: kg * mps2

data area: measure
  -> unit sqm: meter^2

data pressure: measure
  -> unit local_pascal: local_newton/sqm

rule smoke: true
"#;
    load_ok(
        code,
        "mechanics_pascal.lemma",
        "local_pascal = local_newton/sqm must plan across three derived measure layers",
    );
}

/// Population density with two compound units and different area denominators.
#[test]
fn compound_population_density_multi_unit_plans() {
    let code = r#"spec demography
uses lemma units

data population: measure
  -> unit person: 1

data length: measure
  -> unit meter: 1
  -> unit km: 1000

data area: measure
  -> unit sqm: meter^2
  -> unit sqkm: km^2

data density: measure
  -> unit per_sqm: person/sqm
  -> unit per_sqkm: person/sqkm

rule smoke: true
"#;
    load_ok(
        code,
        "demography_density.lemma",
        "person/sqm and person/sqkm must plan with distinct conversion factors",
    );
}

/// Growth rate: compound-of-compound with one unit naturally canonical (factor 1).
#[test]
fn compound_annual_growth_rate_multi_unit_plans() {
    let code = r#"spec growth
uses lemma units

data money: measure
  -> unit eur: 1

data rate: measure
  -> unit eur_per_second: eur/second
  -> unit eur_per_hour: eur/hour

data annual_growth: measure
  -> unit growth_per_hour: eur_per_hour/eur
  -> unit growth_per_second: eur_per_second/eur

rule smoke: true
"#;
    load_ok(
        code,
        "growth_cagr.lemma",
        "eur_per_hour/eur and eur_per_second/eur must plan without spurious normalization",
    );
}

/// Three-level compound chain: each type has a single compound unit requiring normalization.
#[test]
fn compound_three_level_chain_plans() {
    let code = r#"spec chain
uses lemma units

data a: measure
  -> unit au: 1

data b: measure
  -> unit bu: au/second

data c: measure
  -> unit cu: bu/au

data d: measure
  -> unit du: cu/au

rule smoke: true
"#;
    load_ok(
        code,
        "compound_chain.lemma",
        "four-level compound chain must plan with successive normalization",
    );
}

/// Circular derived measure dependency must be rejected at plan time.
#[test]
fn compound_cycle_between_measure_types_rejected() {
    let code = r#"spec cycle
uses lemma units

data x: measure
  -> unit xu: yu/second

data y: measure
  -> unit yu: xu/second

rule smoke: true
"#;
    expect_plan_error(
        code,
        "compound_cycle.lemma",
        "circular compound measure type dependency",
    );
}

/// Explicit prefix on a compound unit alongside another compound unit (normalization).
/// Local names avoid stdlib `newton` / `kilonewton`.
#[test]
fn compound_kilonewton_with_prefix_plans() {
    let code = r#"spec force_prefix
uses lemma units

data mass: measure
  -> unit kg: 1
  -> unit gram: 0.001

data length: measure
  -> unit meter: 1
  -> unit km: 1000

data acceleration: measure
  -> unit mps2: meter/second^2
  -> unit kmh2: km/hour^2

data force: measure
  -> unit local_newton: kg * mps2
  -> unit local_kilonewton: 1000 kg * meter/second^2

rule smoke: true
"#;
    load_ok(
        code,
        "force_prefix.lemma",
        "local_newton and local_kilonewton compound units must plan with normalization",
    );
}

/// Convert 1 local_kilonewton to gram * km/hour^2 via a third compound unit in the same type.
/// 1 local_newton = 1 kg * m/s^2.
/// 1 gram_kmh2 = 0.001 kg * 1000 m / (3600 s)^2 = 1 kg*m/s^2 * (0.001 * 1000 / 3600^2)
///             = 1/12960000 local_newton.
/// 1 local_kilonewton = 1000 local_newton = 1000 * 12960000 gram_kmh2 = 12960000000 gram_kmh2.
#[test]
fn compound_kilonewton_to_gram_kmh2_conversion() {
    let code = r#"spec force_conv
uses lemma units

data mass: measure
  -> unit kg: 1
  -> unit gram: 0.001

data length: measure
  -> unit meter: 1
  -> unit km: 1000

data acceleration: measure
  -> unit mps2: meter/second^2
  -> unit kmh2: km/hour^2

data force: measure
  -> unit local_newton: kg * mps2
  -> unit local_kilonewton: 1000 kg * meter/second^2
  -> unit gram_kmh2: gram * kmh2

data f: 1 local_kilonewton
rule converted: f as gram_kmh2
"#;
    let mut engine = Engine::new();
    engine
        .load([(path_source("force_conv.lemma"), code.to_string())])
        .expect("local_newton/local_kilonewton/gram_kmh2 must plan");
    let now = lemma::DateTimeValue::now();
    let response = engine
        .run(
            None,
            "force_conv",
            Some(&now),
            std::collections::HashMap::new(),
            None,
            false,
        )
        .expect("should evaluate");
    let result = response
        .results
        .get("converted")
        .expect("rule 'converted' not found");
    assert_eq!(
        result
            .result
            .as_ref()
            .and_then(|v| v.measure.as_ref())
            .and_then(|m| m.get("gram_kmh2"))
            .copied(),
        Some(rust_decimal::Decimal::from(12960000000i64))
    );
}

/// Two compound volume units with local names (avoids stdlib `liter` / `cubic_meter`).
#[test]
fn compound_volume_liter_and_cubic_meter_plans() {
    let code = r#"spec volume
uses lemma units

data length: measure
  -> unit meter: 1
  -> unit km: 1000

data volume: measure
  -> unit local_cubic_meter: meter^3
  -> unit local_liter: 0.001 meter^3

rule smoke: true
"#;
    load_ok(
        code,
        "volume.lemma",
        "local_liter and local_cubic_meter must coexist with local_cubic_meter as natural canonical",
    );
}

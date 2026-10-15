//! Derived measure unit planning: `uses lemma units` duration units, compound-of-compound
//! resolution, topological ordering, and conversion-factor normalization.

use indexmap::IndexMap;
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

data mass: units.mass
  -> unit kg: 1

data length: units.length
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

data mass: units.mass
  -> unit kg: 1

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

data length: units.length
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

data mass: units.mass
  -> unit kg: 1

data length: units.length
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

data mass: units.mass
  -> unit kg: 1

data length: units.length
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
        .evaluate(None, "force_conv", Some(&now), IndexMap::new(), None, false)
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

data length: units.length
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

fn evaluate_measure_unit(
    code: &str,
    source_file: &str,
    spec_name: &str,
    rule_name: &str,
    unit: &str,
    data: IndexMap<String, lemma::EvaluateDataValue>,
) -> rust_decimal::Decimal {
    let mut engine = Engine::new();
    engine
        .load([(path_source(source_file), code.to_string())])
        .unwrap_or_else(|errors| {
            panic!(
                "spec must plan ({source_file}): {}",
                errors
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        });
    let now = lemma::DateTimeValue::now();
    let response = engine
        .evaluate(None, spec_name, Some(&now), data, None, false)
        .expect("should evaluate");
    let result = response
        .results
        .get(rule_name)
        .unwrap_or_else(|| panic!("rule '{rule_name}' not found"));
    result
        .result
        .as_ref()
        .and_then(|v| v.measure.as_ref())
        .and_then(|m| m.get(unit))
        .copied()
        .unwrap_or_else(|| panic!("measure map missing unit '{unit}' for '{rule_name}'"))
}

/// Typed import `dimensions.wage_rate` × local `hour` must keep magnitude (not ÷3600).
#[test]
fn imported_compound_wage_rate_times_local_hours_preserves_magnitude() {
    let code = r#"spec dimensions

uses lemma units

data money: measure
  -> unit eur: 1.00
  -> decimals 2

data wage_rate: measure
  -> unit eur_per_hour: eur/hour

spec remuneration

uses lemma units
uses dimensions

data rate: dimensions.wage_rate
data hours_worked: 8 hour

rule shift_pay:
  rate * hours_worked
"#;
    let mut data = IndexMap::new();
    data.insert(
        "rate".to_string(),
        lemma::EvaluateDataValue::string("14.80 eur_per_hour"),
    );
    let eur = evaluate_measure_unit(
        code,
        "imported_wage_rate_hours.lemma",
        "remuneration",
        "shift_pay",
        "eur",
        data,
    );
    assert_eq!(
        eur,
        rust_decimal::Decimal::new(11840, 2),
        "14.80 eur_per_hour * 8 hour must be 118.40 eur when rate is typed dimensions.wage_rate"
    );
}

/// Dimensionless imported compound must keep conversion (not square gram/kilogram → 1e-6).
/// Intended: plan + `1 kilogram_per_kilogram as gram_per_kilogram` → 1000.
/// Today: typed import alias fails planning (inconsistent decompositions after kg/kg cancels).
#[test]
fn imported_dimensionless_compound_alias_preserves_factor() {
    let code = r#"spec dimensions

uses lemma units

data mass_ratio: measure
  -> unit kilogram_per_kilogram: kilogram/kilogram
  -> unit gram_per_kilogram: gram/kilogram

spec consumer

uses lemma units
uses dimensions

data mass_fraction: dimensions.mass_ratio

rule in_grams:
  mass_fraction as gram_per_kilogram
"#;
    let mut data = IndexMap::new();
    data.insert(
        "mass_fraction".to_string(),
        lemma::EvaluateDataValue::string("1 kilogram_per_kilogram"),
    );
    let grams = evaluate_measure_unit(
        code,
        "imported_mass_ratio.lemma",
        "consumer",
        "in_grams",
        "gram_per_kilogram",
        data,
    );
    assert_eq!(
        grams,
        rust_decimal::Decimal::from(1000),
        "1 kilogram_per_kilogram as gram_per_kilogram must be 1000 when mass_ratio is imported"
    );
}

/// Additive compound on an imported measure resolves once; inherited eur_per_hour stays correct.
#[test]
fn imported_wage_rate_additive_eur_per_minute_resolves_once() {
    let code = r#"spec dimensions

uses lemma units

data money: measure
  -> unit eur: 1.00
  -> decimals 2

data wage_rate: measure
  -> unit eur_per_hour: eur/hour

spec remuneration

uses lemma units
uses dimensions

data rate: dimensions.wage_rate
  -> unit eur_per_minute: eur/minute

data minutes_worked: 120 minute

rule shift_pay:
  rate * minutes_worked
"#;
    let mut data = IndexMap::new();
    data.insert(
        "rate".to_string(),
        lemma::EvaluateDataValue::string("0.25 eur_per_minute"),
    );
    let eur = evaluate_measure_unit(
        code,
        "imported_wage_rate_minute.lemma",
        "remuneration",
        "shift_pay",
        "eur",
        data,
    );
    assert_eq!(
        eur,
        rust_decimal::Decimal::new(3000, 2),
        "0.25 eur_per_minute * 120 minute must be 30.00 eur"
    );
}

/// Import alias must not poison later local compounds that name the same bare `hour`.
#[test]
fn imported_wage_rate_alias_does_not_poison_later_local_compound() {
    let code = r#"spec dimensions

uses lemma units

data money: measure
  -> unit eur: 1.00
  -> decimals 2

data wage_rate: measure
  -> unit eur_per_hour: eur/hour

spec travel

uses lemma units
uses dimensions

data imported_rate: dimensions.wage_rate

data length: units.length
  -> unit km: 1000

data velocity: measure
  -> unit km_per_hour: km/hour

data speed: 100 km_per_hour
data hours_worked: 2 hour

rule distance:
  speed * hours_worked
"#;
    let km = evaluate_measure_unit(
        code,
        "imported_alias_lookup.lemma",
        "travel",
        "distance",
        "km",
        IndexMap::new(),
    );
    assert_eq!(
        km,
        rust_decimal::Decimal::from(200),
        "100 km_per_hour * 2 hour must be 200 km with an unused wage_rate import alias present"
    );
}

/// Same-spec compound wage_rate × hour still resolves once from the source prefix.
#[test]
fn same_spec_compound_wage_rate_times_hours_preserves_magnitude() {
    let code = r#"spec remuneration

uses lemma units

data money: measure
  -> unit eur: 1.00
  -> decimals 2

data wage_rate: measure
  -> unit eur_per_hour: eur/hour

data rate: 14.80 eur_per_hour
data hours_worked: 8 hour

rule shift_pay:
  rate * hours_worked
"#;
    let eur = evaluate_measure_unit(
        code,
        "same_spec_wage_rate_hours.lemma",
        "remuneration",
        "shift_pay",
        "eur",
        IndexMap::new(),
    );
    assert_eq!(
        eur,
        rust_decimal::Decimal::new(11840, 2),
        "same-spec 14.80 eur_per_hour * 8 hour must be 118.40 eur"
    );
}

/// Extend `units.duration` with a unit defined as a multiple of an inherited unit.
/// Bare `workday: 8 hour` must plan (not ambiguous signature vs `hour`). Qualified
/// `8 units.duration.hour` must not be required — that was an accidental string escape.
#[test]
fn duration_extension_unit_as_multiple_of_inherited_unit_plans() {
    let code = r#"spec shift
uses lemma units

data worked: units.duration
  -> unit workday: 8 hour

rule smoke: true
"#;
    load_ok(
        code,
        "shift.lemma",
        "extending units.duration with -> unit workday: 8 hour must plan",
    );
}

/// `eur_per_hour * workday` cancels to money: 50 eur_per_hour * 1 workday → 400 eur.
#[test]
fn duration_extension_workday_times_wage_rate_is_money() {
    let code = r#"spec pay

uses lemma units

data money: measure
  -> unit eur: 1.00
  -> decimals 2

data wage_rate: measure
  -> unit eur_per_hour: eur/hour

data worked: units.duration
  -> unit workday: 8 hour

data rate: 50 eur_per_hour
data days: 1 workday

rule shift_pay:
  rate * days
"#;
    let eur = evaluate_measure_unit(
        code,
        "workday_wage_product.lemma",
        "pay",
        "shift_pay",
        "eur",
        IndexMap::new(),
    );
    assert_eq!(
        eur,
        rust_decimal::Decimal::new(40000, 2),
        "50 eur_per_hour * 1 workday must be 400.00 eur"
    );
}

/// Remuneration landmine: `hourly` typed as money × hours is anonymous at the rule boundary.
/// Correct shape is wage_rate × hours (cancels to money). This must stay a planning Error.
#[test]
fn money_times_hours_at_rule_boundary_is_anonymous_intermediate() {
    expect_plan_error(
        r#"spec pay

uses lemma units

data money: measure
  -> unit eur: 1.00
  -> decimals 2

data hourly: 14.80 eur
data hours_worked: 8 hour

rule shift_pay:
  hourly * hours_worked
"#,
        "money_times_hours.lemma",
        "anonymous intermediate",
    );
}

/// Family casts both ways; bare `hour` still binds `duration`, not the extension.
#[test]
fn duration_extension_workday_family_casts() {
    let code = r#"spec shift_conv
uses lemma units

data worktime: units.duration
  -> unit workday: 8 hour

data shift: 1 workday
data eight_hours: 8 hour
rule hours: shift as hour
rule days: eight_hours as workday
rule bare_hour: 5 hour
"#;
    let mut engine = Engine::new();
    engine
        .load([(path_source("shift_conv.lemma"), code.to_string())])
        .expect("workday: 8 hour on units.duration extension must plan");
    let now = lemma::DateTimeValue::now();
    let response = engine
        .evaluate(None, "shift_conv", Some(&now), IndexMap::new(), None, false)
        .expect("should evaluate");
    let measure_unit = |rule: &str, unit: &str| {
        response
            .results
            .get(rule)
            .unwrap_or_else(|| panic!("rule '{rule}' not found"))
            .result
            .as_ref()
            .and_then(|v| v.measure.as_ref())
            .and_then(|m| m.get(unit))
            .copied()
    };
    assert_eq!(
        measure_unit("hours", "hour"),
        Some(rust_decimal::Decimal::from(8))
    );
    assert_eq!(
        measure_unit("days", "workday"),
        Some(rust_decimal::Decimal::from(1))
    );
    assert_eq!(
        response
            .results
            .get("bare_hour")
            .expect("rule 'bare_hour' not found")
            .rule
            .rule_type
            .name
            .as_deref(),
        Some("duration"),
        "bare hour must still bind duration, not worktime"
    );
}

/// Two independent types with the same compound signature stay an ambiguity error.
#[test]
fn duplicate_compound_signature_across_types_still_errors() {
    expect_plan_error(
        r#"spec rates
uses lemma units

data money: measure
  -> unit eur: 1

data wage_a: measure
  -> unit eur_per_hour: eur/hour

data wage_b: measure
  -> unit pay_rate: eur/hour

rule smoke: true
"#,
        "rates.lemma",
        "ambiguous unit signature",
    );
}

/// Family-scale workday beside a true dimensional compound in the same file.
/// Type-wide factor wipe must not break `km/hour * hour` while workday stays identity.
#[test]
fn duration_extension_workday_beside_dimensional_compound() {
    let code = r#"spec mixed

uses lemma units

data worktime: units.duration
  -> unit workday: 8 hour

data length: units.length
  -> unit km: 1000

data velocity: measure
  -> unit km_per_hour: km/hour

data days: 1 workday
data speed: 100 km_per_hour
data elapsed: 2 hour

rule hours_from_day:
  days as hour

rule distance:
  speed * elapsed
"#;
    assert_eq!(
        evaluate_measure_unit(
            code,
            "workday_beside_velocity.lemma",
            "mixed",
            "hours_from_day",
            "hour",
            IndexMap::new(),
        ),
        rust_decimal::Decimal::from(8),
        "1 workday as hour must be 8"
    );
    assert_eq!(
        evaluate_measure_unit(
            code,
            "workday_beside_velocity.lemma",
            "mixed",
            "distance",
            "km",
            IndexMap::new(),
        ),
        rust_decimal::Decimal::from(200),
        "100 km_per_hour * 2 hour must be 200 km beside a workday extension"
    );
}

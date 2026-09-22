//! Per-unit minimum/maximum/default magnitudes on measure and ratio show units.
//! Same-type alias units (e.g. eur/euro) are allowed; cross-type unit clashes are covered in graph unit tests.

use lemma::DateTimeValue;
use lemma::Engine;
use lemma::{MeasureUnit, RatioUnit, TypeSpecification};
use rust_decimal::Decimal;
use std::str::FromStr;

fn decimal_lit(s: &str) -> Decimal {
    Decimal::from_str(s).expect("BUG: test decimal literal must parse")
}

fn load(engine: &mut Engine, code: &str, path: &str) {
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(path))),
            code.to_string(),
        )])
        .unwrap_or_else(|errs| {
            let joined = errs
                .errors
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("\n");
            panic!("expected load to succeed, got: {joined}");
        });
}

fn load_err(engine: &mut Engine, code: &str) -> String {
    let err = engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "show_unit_constraints.lemma",
            ))),
            code.to_string(),
        )])
        .expect_err("expected load failure");
    err.errors
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

fn quantity_unit<'a>(spec: &'a TypeSpecification, name: &str) -> &'a MeasureUnit {
    match spec {
        TypeSpecification::Measure { units, .. } => units
            .get(name)
            .unwrap_or_else(|e| panic!("unit {name}: {e}")),
        other => panic!("expected Measure, got {other:?}"),
    }
}

fn ratio_unit<'a>(spec: &'a TypeSpecification, name: &str) -> &'a RatioUnit {
    match spec {
        TypeSpecification::Ratio { units, .. } => units
            .get(name)
            .unwrap_or_else(|e| panic!("unit {name}: {e}")),
        other => panic!("expected Ratio, got {other:?}"),
    }
}

#[test]
fn measure_minimum_syncs_canonical_and_per_unit_magnitudes() {
    let code = r#"
spec s
data money: measure -> unit eur: 1 -> unit usd: 0.91
data mass: measure -> unit kilogram: 1
data cost_per_unit: measure
  -> unit eur_per_kilo: eur/kilogram
  -> minimum 1.20 eur_per_kilo
  -> maximum 2.00 eur_per_kilo
rule out: cost_per_unit
"#;
    let mut engine = Engine::new();
    load(&mut engine, code, "quantity_min.lemma");

    let now = DateTimeValue::now();
    let show = engine.show(None, "s", Some(&now)).expect("show");
    let entry = show.data.get("cost_per_unit").expect("data");
    match &entry.lemma_type.specifications {
        TypeSpecification::Measure {
            minimum, maximum, ..
        } => {
            let unit = quantity_unit(&entry.lemma_type.specifications, "eur_per_kilo");
            assert_eq!(unit.minimum_decimal(), Some(decimal_lit("1.2")));
            assert_eq!(unit.maximum_decimal(), Some(decimal_lit("2")));
            assert_eq!(minimum.as_ref().unwrap().1, "eur_per_kilo");
            assert_eq!(maximum.as_ref().unwrap().1, "eur_per_kilo");
        }
        other => panic!("expected Measure, got {other:?}"),
    }
}

#[test]
fn measure_second_unit_minimum_converted_from_canonical() {
    let code = r#"
spec s
data mass: measure
  -> unit kilogram: 1
  -> unit gram: 0.001
  -> minimum 1 kilogram
rule out: mass
"#;
    let mut engine = Engine::new();
    load(&mut engine, code, "quantity_gram.lemma");

    let now = DateTimeValue::now();
    let show = engine.show(None, "s", Some(&now)).expect("show");
    let entry = show.data.get("mass").expect("data");
    let gram = quantity_unit(&entry.lemma_type.specifications, "gram");
    assert_eq!(gram.minimum_decimal(), Some(decimal_lit("1000")));
    let kg = quantity_unit(&entry.lemma_type.specifications, "kilogram");
    assert_eq!(kg.minimum_decimal(), Some(decimal_lit("1")));
}

#[test]
fn measure_literal_resolves_unit_index_type_with_synced_minimum() {
    let code = r#"
spec s
data money: measure -> unit eur: 1
data budget: money -> minimum 0 eur
data price: 10 eur
rule out: price
"#;
    let mut engine = Engine::new();
    load(&mut engine, code, "quantity_literal_min.lemma");

    let now = DateTimeValue::now();
    let response = engine
        .run(
            None,
            "s",
            Some(&now),
            std::collections::HashMap::new(),
            None,
            false,
        )
        .expect("eval");
    let result = response
        .results
        .values()
        .next()
        .expect("rule result")
        .result()
        .expect("result")
        .to_string();
    assert!(
        result.contains("10") && result.to_lowercase().contains("eur"),
        "expected 10 eur, got {result}"
    );
}

#[test]
fn ratio_show_json_exposes_per_unit_minimum_string() {
    let code = r#"
spec s
data r: ratio
  -> unit basis_points: 10000
  -> minimum 500 basis_points
rule out: r
"#;
    let mut engine = Engine::new();
    load(&mut engine, code, "ratio_bps.lemma");

    let now = DateTimeValue::now();
    let show = engine.show(None, "s", Some(&now)).expect("show");
    let entry = show.data.get("r").expect("data");
    let unit = ratio_unit(&entry.lemma_type.specifications, "basis_points");
    assert_eq!(unit.minimum_decimal(), Some(decimal_lit("500")));

    let json = serde_json::to_value(lemma::api::LemmaType::from(&entry.lemma_type)).expect("serde");
    let units = json["units"].as_array().expect("units array");
    let bps = units
        .iter()
        .find(|u| u["name"] == "basis_points")
        .expect("basis_points row");
    assert_eq!(bps["minimum"].as_str(), Some("500"));
    assert_eq!(
        entry.lemma_type.specifications.minimum_decimal(),
        Some(decimal_lit("0.05"))
    );
}

#[test]
fn measure_default_populates_each_unit_magnitude() {
    let code = r#"
spec s
data money: measure
  -> unit eur: 1
  -> unit usd: 2
  -> suggest 4 eur
rule out: money
"#;
    let mut engine = Engine::new();
    load(&mut engine, code, "quantity_default.lemma");

    let now = DateTimeValue::now();
    let show = engine.show(None, "s", Some(&now)).expect("show");
    let entry = show.data.get("money").expect("data");
    let eur = quantity_unit(&entry.lemma_type.specifications, "eur");
    let usd = quantity_unit(&entry.lemma_type.specifications, "usd");
    assert_eq!(eur.suggestion_magnitude_decimal(), Some(decimal_lit("4")));
    assert_eq!(usd.suggestion_magnitude_decimal(), Some(decimal_lit("2")));
}

#[test]
fn reference_local_suggestion_populates_per_unit_magnitudes() {
    let code = r#"
spec inner
data base: measure
  -> unit eur: 1
  -> unit usd: 2

spec outer
uses i: inner
data here: i.base -> suggest 10 usd
rule r: here
"#;
    let mut engine = Engine::new();
    load(&mut engine, code, "ref_default.lemma");

    let now = DateTimeValue::now();
    let show = engine.show(None, "outer", Some(&now)).expect("show");
    let entry = show.data.get("here").expect("here");
    let usd = quantity_unit(&entry.lemma_type.specifications, "usd");
    let eur = quantity_unit(&entry.lemma_type.specifications, "eur");
    assert_eq!(usd.suggestion_magnitude_decimal(), Some(decimal_lit("10")));
    assert_eq!(eur.suggestion_magnitude_decimal(), Some(decimal_lit("20")));
}

#[test]
fn precision_constraint_rejected_on_measure_and_number() {
    for (snippet, label) in [
        (
            r#"
spec s
data x: measure -> precision 1
rule r: x
"#,
            "measure",
        ),
        (
            r#"
spec s
data x: number -> precision 1
rule r: x
"#,
            "number",
        ),
    ] {
        let mut engine = Engine::new();
        let msg = load_err(&mut engine, snippet).to_lowercase();
        assert!(
            msg.contains("precision") || msg.contains("unknown constraint"),
            "{label}: expected precision/unknown constraint error, got: {msg}"
        );
    }
}

#[test]
fn show_json_round_trip_preserves_measure_unit_bounds() {
    let code = r#"
spec s
data mass: measure
  -> unit kilogram: 1
  -> unit gram: 0.001
  -> minimum 1 kilogram
rule r: mass
"#;
    let mut engine = Engine::new();
    load(&mut engine, code, "roundtrip.lemma");
    let now = DateTimeValue::now();
    let show = engine.show(None, "s", Some(&now)).expect("show");
    let api_show = lemma::api::Show::from(&show);
    let json = serde_json::to_string(&api_show).expect("serialize");
    let round_tripped: lemma::api::Show = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(api_show, round_tripped);
    let entry = show.data.get("mass").expect("mass");
    let gram = quantity_unit(&entry.lemma_type.specifications, "gram");
    assert_eq!(gram.minimum_decimal(), Some(decimal_lit("1000")));
}

#[test]
fn same_type_alias_units_with_same_factor_must_plan() {
    let code = r#"
spec s
data money: measure
  -> unit eur: 1
  -> unit euro: 1
rule r: 1 eur
"#;
    let mut engine = Engine::new();
    load(&mut engine, code, "quantity_alias.lemma");
}

const COMPOUND_COST_PER_UNIT_SPEC: &str = r#"
spec s
data money: measure -> unit eur: 1 -> unit usd: 0.91
data mass: measure -> unit kilogram: 1 -> unit tonne: 1000
data cost_per_unit: measure
  -> unit eur_per_kilo: eur/kilogram
  -> unit usd_per_tonne: usd/tonne
  -> maximum 2.00 eur_per_kilo
rule out: cost_per_unit
"#;

#[test]
fn compound_measure_maximum_converts_per_unit_across_units() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        COMPOUND_COST_PER_UNIT_SPEC,
        "compound_max.lemma",
    );

    let now = DateTimeValue::now();
    let show = engine.show(None, "s", Some(&now)).expect("show");
    let entry = show.data.get("cost_per_unit").expect("data");

    let eur_per_kilo = quantity_unit(&entry.lemma_type.specifications, "eur_per_kilo");
    let usd_per_tonne = quantity_unit(&entry.lemma_type.specifications, "usd_per_tonne");

    assert_eq!(eur_per_kilo.maximum_decimal(), Some(decimal_lit("2")));
    assert_ne!(
        usd_per_tonne.maximum_decimal(),
        Some(decimal_lit("2")),
        "usd_per_tonne maximum must be converted from eur_per_kilo bound, not copied as 2"
    );

    assert_eq!(
        usd_per_tonne.maximum_canonical_decimal(),
        eur_per_kilo.maximum_canonical_decimal(),
        "per-unit maxima must represent the same canonical bound"
    );

    let json = serde_json::to_value(lemma::api::LemmaType::from(&entry.lemma_type)).expect("serde");
    let units = json["units"].as_array().expect("units array");
    let usd_json = units
        .iter()
        .find(|u| u["name"] == "usd_per_tonne")
        .expect("usd_per_tonne row");
    assert_ne!(
        usd_json["maximum"].as_str(),
        Some("2"),
        "show JSON must not expose stale usd_per_tonne maximum"
    );
}

#[test]
fn compound_measure_maximum_in_bound_unit_stays_literal() {
    let code = r#"
spec s
data money: measure -> unit eur: 1 -> unit usd: 0.91
data mass: measure -> unit kilogram: 1 -> unit tonne: 1000
data cost_per_unit: measure
  -> unit eur_per_kilo: eur/kilogram
  -> unit usd_per_tonne: usd/tonne
  -> maximum 2.00 usd_per_tonne
rule out: cost_per_unit
"#;
    let mut engine = Engine::new();
    load(&mut engine, code, "compound_max_bound_unit.lemma");

    let now = DateTimeValue::now();
    let show = engine.show(None, "s", Some(&now)).expect("show");
    let entry = show.data.get("cost_per_unit").expect("data");

    let usd_per_tonne = quantity_unit(&entry.lemma_type.specifications, "usd_per_tonne");
    assert_eq!(
        usd_per_tonne.maximum_decimal(),
        Some(decimal_lit("2")),
        "maximum declared in usd_per_tonne must stay 2 in that unit"
    );
}

const TRI_COMPOUND_COST_SPEC: &str = r#"
spec s
uses lemma units

data money: measure
  -> unit eur: 1
  -> unit usd: 0.91

data mass: measure
  -> unit kilogram: 1
  -> unit tonne: 1000

data storage_cost: measure
  -> unit eur_per_kilo_hour: eur/kilogram/hour
  -> unit usd_per_ton_hour: usd/tonne/hour
  -> maximum 2.00 eur_per_kilo_hour

rule out: storage_cost
"#;

#[test]
fn tri_compound_measure_maximum_converts_per_unit_across_units() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        TRI_COMPOUND_COST_SPEC,
        "tri_compound_max.lemma",
    );

    let now = DateTimeValue::now();
    let show = engine.show(None, "s", Some(&now)).expect("show");
    let entry = show.data.get("storage_cost").expect("data");

    let eur_per_kilo_hour = quantity_unit(&entry.lemma_type.specifications, "eur_per_kilo_hour");
    let usd_per_ton_hour = quantity_unit(&entry.lemma_type.specifications, "usd_per_ton_hour");

    assert_eq!(eur_per_kilo_hour.maximum_decimal(), Some(decimal_lit("2")));
    assert_ne!(
        usd_per_ton_hour.maximum_decimal(),
        Some(decimal_lit("2")),
        "usd_per_ton_hour maximum must be converted from eur_per_kilo_hour bound, not copied as 2"
    );

    assert_eq!(
        usd_per_ton_hour.maximum_canonical_decimal(),
        eur_per_kilo_hour.maximum_canonical_decimal(),
        "per-unit maxima must represent the same canonical bound across three referenced quantities"
    );
}

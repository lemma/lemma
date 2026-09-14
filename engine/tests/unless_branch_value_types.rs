//! Unless arms must share value type (measure dims), not only base kind.

use lemma::{Engine, SourceType};
use std::path::PathBuf;
use std::sync::Arc;

fn load_ok(code: &str) {
    Engine::new()
        .load([(
            SourceType::Path(Arc::new(PathBuf::from("unless_dims.lemma"))),
            code.to_string(),
        )])
        .expect("must plan");
}

fn load_err(code: &str) -> String {
    match Engine::new().load([(
        SourceType::Path(Arc::new(PathBuf::from("unless_dims.lemma"))),
        code.to_string(),
    )]) {
        Err(e) => format!("{e:?}"),
        Ok(_) => panic!("expected branch value-type error"),
    }
}

#[test]
fn unless_both_money_arms_plans() {
    load_ok(
        r#"
spec cost_price
uses lemma units

data amount: units.mass
data money: measure
  -> unit eur: 1.00
  -> decimals 2
data labor_cost: measure
  -> unit eur_per_hour: eur/hour
  -> suggest 25 eur_per_hour
data product_cost: measure
  -> unit eur_per_kg: eur/kilogram
  -> decimals 2
  -> suggest 4 eur_per_kg
data throughput: measure
  -> unit kg_per_hour: kilogram/hour
  -> suggest 12 kg_per_hour
data flag: boolean

rule cost_price: product_cost + labor_cost / throughput
rule cost_price_per_quantity: cost_price * amount
  unless flag then 4 eur
"#,
    );
}

#[test]
fn unless_money_vs_eur_per_kg_is_rejected() {
    let msg = load_err(
        r#"
spec cost_price
uses lemma units

data amount: units.mass
data money: measure
  -> unit eur: 1.00
  -> unit inr: 0.0092
  -> decimals 2
data labor_cost: measure
  -> unit eur_per_hour: eur/hour
  -> suggest 25 eur_per_hour
data product_cost: measure
  -> unit eur_per_kg: eur/kilogram
  -> decimals 2
  -> suggest 4 eur_per_kg
data throughput: measure
  -> unit kg_per_hour: kilogram/hour
  -> suggest 12 kg_per_hour

rule cost_price: product_cost + labor_cost / throughput
rule dep: 5 / 2
rule cost_price_per_quantity: cost_price * amount
  unless dep < 4 then 4 eur_per_kg
"#,
    );
    assert!(msg.contains("same value type"), "unexpected message: {msg}");
}

#[test]
fn unless_number_after_usd_measure_is_rejected_at_planning() {
    let msg = load_err(
        r#"
spec s
data money: measure
  -> unit usd: 1
  -> unit eur: 1.1
data a: boolean
data b: boolean
rule rate: 10 usd
  unless a then 3 usd as eur
  unless b then 2
"#,
    );
    assert!(msg.contains("same value type"), "unexpected message: {msg}");
}

#[test]
fn unless_measure_range_dim_mismatch_is_rejected() {
    let msg = load_err(
        r#"
spec s
data money: measure
  -> unit eur: 1
data mass: measure
  -> unit kilogram: 1
data money_band: money range
data mass_band: mass range
data flag: boolean
rule r: money_band
  unless flag then mass_band
"#,
    );
    assert!(msg.contains("same value type"), "unexpected message: {msg}");
}

#[test]
fn unless_matching_measure_range_arms_plan() {
    load_ok(
        r#"
spec s
data money: measure
  -> unit eur: 1
data band_a: money range
data band_b: money range
data flag: boolean
rule r: band_a
  unless flag then band_b
"#,
    );
}

#[test]
fn measure_range_reference_family_mismatch_is_rejected() {
    let msg = load_err(
        r#"
spec lib
data money: measure
  -> unit eur: 1
data temperature: measure
  -> unit celsius: 1
data money_band: money range
data temp_band: temperature range

spec sink
uses lib: lib
  -> with money_band: lib.temp_band
"#,
    );
    assert!(
        msg.contains("measure family mismatch"),
        "unexpected message: {msg}"
    );
}

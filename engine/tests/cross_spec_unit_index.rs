//! Cross-spec plans execute dependency unit conversions. Consumer bare unit
//! scope includes each direct import's full unit_index (path-prefixed).

use indexmap::IndexMap;
use lemma::{DateTimeValue, Engine, EvaluateDataValue, SourceType};

const UNITS_SPEC: &str = r#"
spec units
uses lemma units
data money: measure
  -> unit eur: 1
  -> decimals 2
"#;

const WAREHOUSING_SPEC: &str = r#"
spec warehousing
uses units
uses si: lemma units

data units_per_pallet: number
  -> minimum 1
  -> suggest 1

data storage_duration: si.duration
  -> minimum 0 week
  -> suggest 10 day

data interbranch_transport_per_pallet: units.money
  -> minimum 0 eur
  -> suggest 0 eur

data inbound_handling_per_pallet: units.money
  -> minimum 0 eur
  -> suggest 0 eur

data storage_per_pallet_per_week: units.money
  -> minimum 0 eur
  -> suggest 10 eur

data labeling_per_pallet: units.money
  -> minimum 0 eur
  -> suggest 0 eur

data outbound_handling_per_pallet: units.money
  -> minimum 0 eur
  -> suggest 0 eur

rule storage_cost_per_pallet:
  storage_per_pallet_per_week
  * ceil (storage_duration as week as number)

rule total_logistics_per_pallet:
  interbranch_transport_per_pallet
  + inbound_handling_per_pallet
  + storage_cost_per_pallet
  + labeling_per_pallet
  + outbound_handling_per_pallet

rule total_logistics_per_ce:
  total_logistics_per_pallet / units_per_pallet
"#;

const QUOTATION_SPEC: &str = r#"
spec quotation
uses wh: warehousing
rule total: wh.total_logistics_per_ce
"#;

const QUOTATION_MINUTE_SPEC: &str = r#"
spec quotation_minute
uses wh: warehousing
rule via_alias: 5 wh.minute
rule bare: 5 minute
"#;

fn load_specs(engine: &mut Engine) {
    engine
        .load([(SourceType::Volatile, UNITS_SPEC.to_string())])
        .expect("units spec must load");
    engine
        .load([(SourceType::Volatile, WAREHOUSING_SPEC.to_string())])
        .expect("warehousing spec must load");
}

fn warehousing_default_inputs(prefix: &str) -> IndexMap<String, EvaluateDataValue> {
    let key = |name: &str| {
        if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}.{name}")
        }
    };
    IndexMap::from([
        (key("units_per_pallet"), "1".into()),
        (key("storage_duration"), "10 day".into()),
        (key("interbranch_transport_per_pallet"), "0 eur".into()),
        (key("inbound_handling_per_pallet"), "0 eur".into()),
        (key("storage_per_pallet_per_week"), "10 eur".into()),
        (key("labeling_per_pallet"), "0 eur".into()),
        (key("outbound_handling_per_pallet"), "0 eur".into()),
    ])
}

#[test]
fn warehousing_plans_alone() {
    let mut engine = Engine::new();
    load_specs(&mut engine);
    let now = DateTimeValue::now();
    engine
        .show(None, "warehousing", Some(&now))
        .expect("warehousing must plan alone");
    let response = engine
        .evaluate(
            None,
            "warehousing",
            Some(&now),
            warehousing_default_inputs(""),
            None,
            false,
        )
        .expect("warehousing must evaluate");
    let display = response
        .results
        .get("storage_cost_per_pallet")
        .expect("storage_cost_per_pallet must be present")
        .result()
        .expect("storage_cost_per_pallet must have display")
        .to_string();
    assert_eq!(
        display, "20.00 eur",
        "10 eur/week * ceil(10 day as week) must be 20.00 eur, got: {display}"
    );
}

#[test]
fn quotation_evaluates_cross_spec_duration_conversion() {
    let mut engine = Engine::new();
    load_specs(&mut engine);
    engine
        .load([(SourceType::Volatile, QUOTATION_SPEC.to_string())])
        .expect("quotation must load");
    let now = DateTimeValue::now();
    let response = engine
        .evaluate(
            None,
            "quotation",
            Some(&now),
            warehousing_default_inputs("wh"),
            None,
            false,
        )
        .expect("quotation must evaluate");
    let display = response
        .results
        .get("total")
        .expect("rule total must be present")
        .result()
        .expect("total must have display")
        .to_string();
    assert_eq!(
        display, "20.00 eur",
        "10 eur/week * ceil(10 day as week) / 1 CE must be 20.00 eur, got: {display}"
    );
}

#[test]
fn quotation_local_rule_can_use_minutes_via_import_scope() {
    let mut engine = Engine::new();
    load_specs(&mut engine);
    engine
        .load([(SourceType::Volatile, QUOTATION_MINUTE_SPEC.to_string())])
        .expect("minute reaches quotation via wh unit scope");
    let now = DateTimeValue::now();
    let response = engine
        .evaluate(
            None,
            "quotation_minute",
            Some(&now),
            IndexMap::new(),
            None,
            false,
        )
        .expect("evaluate");
    assert_eq!(
        response
            .results
            .get("via_alias")
            .expect("via_alias")
            .result()
            .expect("display")
            .to_string(),
        "5 minute"
    );
    assert_eq!(
        response
            .results
            .get("bare")
            .expect("bare")
            .result()
            .expect("display")
            .to_string(),
        "5 minute"
    );
}

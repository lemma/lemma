//! Family unit maps: rule results and show rule schemas expose the full measure/ratio family.

use lemma::{DateTimeValue, Engine, SourceType};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

fn engine_with_parent_child_money() -> Engine {
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Path(Arc::new(PathBuf::from("family_units.lemma"))),
            r#"
spec family_units
data money: measure
 -> unit eur: 1.00
 -> unit usd: 0.91
 -> decimals 2

data amount: money
 -> suggest 0 eur

data price: money
 -> unit gbp: 1.17

rule total: amount
"#
            .to_string(),
        )])
        .expect("load");
    engine
}

fn run_spec(engine: &Engine, spec: &str) -> lemma::Response {
    run_spec_in_repo(engine, None, spec)
}

fn run_spec_in_repo(engine: &Engine, repository: Option<&str>, spec: &str) -> lemma::Response {
    let now = DateTimeValue::now();
    engine
        .run(repository, spec, Some(&now), HashMap::new(), None, false)
        .expect("run")
}

#[test]
fn rule_result_measure_map_includes_child_unit() {
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Path(Arc::new(PathBuf::from("family_units_eval.lemma"))),
            r#"
spec family_units_eval
data money: measure
 -> unit eur: 1.00
 -> unit usd: 0.91
 -> decimals 2

data price: money
 -> unit gbp: 1.17

rule total: 5 eur
"#
            .to_string(),
        )])
        .expect("load");
    let response = run_spec(&engine, "family_units_eval");
    let total = response.results.get("total").expect("total");
    assert!(!total.vetoed);
    let measure = total
        .value
        .as_ref()
        .expect("value")
        .measure
        .as_ref()
        .expect("measure map");
    assert!(measure.contains_key("eur"));
    assert!(measure.contains_key("usd"));
    assert!(measure.contains_key("gbp"));
}

#[test]
fn show_rule_schema_includes_child_unit() {
    let engine = engine_with_parent_child_money();
    let now = DateTimeValue::now();
    let show = engine.show(None, "family_units", Some(&now)).expect("show");
    let total_type = &show
        .rules
        .get("total")
        .expect("total rule schema")
        .lemma_type;
    let unit_names: Vec<&str> = total_type
        .measure_unit_names()
        .expect("measure rule")
        .into_iter()
        .collect();
    assert!(unit_names.contains(&"eur"));
    assert!(unit_names.contains(&"usd"));
    assert!(unit_names.contains(&"gbp"));
}

#[test]
fn show_data_stays_declared_only() {
    let engine = engine_with_parent_child_money();
    let now = DateTimeValue::now();
    let show = engine.show(None, "family_units", Some(&now)).expect("show");
    let amount_type = &show.data.get("amount").expect("amount data").lemma_type;
    let amount_units: Vec<&str> = amount_type
        .measure_unit_names()
        .expect("amount type")
        .into_iter()
        .collect();
    assert_eq!(amount_units, vec!["eur", "usd"]);
}

#[test]
fn dual_import_alias_consumer_loads_and_rule_result_includes_percent() {
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Path(Arc::new(PathBuf::from("dual_alias_ratio.lemma"))),
            r#"
repo dual_alias

spec margin_spec
data margin: ratio
 -> suggest 10%

spec consumer
uses a: margin_spec
uses b: margin_spec
data rate_slot: ratio
 -> suggest 50%
rule rate_out: rate_slot
"#
            .to_string(),
        )])
        .expect("load");
    let data = HashMap::from([("rate_slot".to_string(), "50%".into())]);
    let now = DateTimeValue::now();
    let response = engine
        .run(
            Some("dual_alias"),
            "consumer",
            Some(&now),
            data,
            None,
            false,
        )
        .expect("run");
    let rate = response.results.get("rate_out").expect("rate_out");
    assert!(!rate.vetoed);
    let ratio = rate
        .value
        .as_ref()
        .expect("value")
        .ratio
        .as_ref()
        .expect("ratio map");
    assert!(ratio.contains_key("percent"));
    assert!(ratio.contains_key("permille"));
}

#[test]
fn ratio_family_rule_and_show_include_child_custom_unit() {
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Path(Arc::new(PathBuf::from("ratio_family.lemma"))),
            r#"
spec ratio_family
data base_rate: ratio
 -> suggest 10%

data fee_rate: base_rate
 -> unit basis_points: 10000

rule out: 10 basis_points
rule need_parent: base_rate
"#
            .to_string(),
        )])
        .expect("load");
    let response = run_spec(&engine, "ratio_family");
    let out = response.results.get("out").expect("out");
    assert!(!out.vetoed);
    let ratio = out
        .value
        .as_ref()
        .expect("value")
        .ratio
        .as_ref()
        .expect("ratio map");
    assert!(ratio.contains_key("percent"));
    assert!(ratio.contains_key("basis_points"));

    let now = DateTimeValue::now();
    let show = engine.show(None, "ratio_family", Some(&now)).expect("show");
    let out_type = &show.rules.get("out").expect("out rule schema").lemma_type;
    let unit_names: Vec<&str> = out_type
        .ratio_unit_names()
        .expect("ratio rule")
        .into_iter()
        .collect();
    assert!(unit_names.contains(&"percent"));
    assert!(unit_names.contains(&"basis_points"));

    let portfolio_type = &show
        .data
        .get("base_rate")
        .expect("base_rate data")
        .lemma_type;
    let portfolio_units: Vec<&str> = portfolio_type
        .ratio_unit_names()
        .expect("base_rate type")
        .into_iter()
        .collect();
    assert_eq!(portfolio_units, vec!["percent", "permille"]);
}

#[test]
fn measure_range_rule_result_expands_family_units() {
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Path(Arc::new(PathBuf::from("money_range.lemma"))),
            r#"
spec money_range
data money: measure
 -> unit eur: 1.00
 -> unit usd: 0.91

data price: money
 -> unit gbp: 1.17

rule band: (1 eur...10 eur)
"#
            .to_string(),
        )])
        .expect("load");
    let response = run_spec(&engine, "money_range");
    let band = response.results.get("band").expect("band");
    assert!(!band.vetoed);
    let range = band
        .value
        .as_ref()
        .expect("value")
        .range
        .as_ref()
        .expect("range");
    let from_measure = range.from.measure.as_ref().expect("from measure map");
    let to_measure = range.to.measure.as_ref().expect("to measure map");
    for map in [from_measure, to_measure] {
        assert!(map.contains_key("eur"));
        assert!(map.contains_key("usd"));
        assert!(map.contains_key("gbp"));
    }
}

#[test]
fn show_rule_units_follow_extension_depth_not_name_only() {
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Path(Arc::new(PathBuf::from("grandchild_units.lemma"))),
            r#"
spec grandchild_units
data money: measure
 -> unit eur: 1.00
 -> unit usd: 0.91

data price: money
 -> unit gbp: 1.17

data wholesale: price
 -> unit chf: 1.05

rule total: 0 eur
"#
            .to_string(),
        )])
        .expect("load");
    let now = DateTimeValue::now();
    let show = engine
        .show(None, "grandchild_units", Some(&now))
        .expect("show");
    let total_type = &show
        .rules
        .get("total")
        .expect("total rule schema")
        .lemma_type;
    let unit_names: Vec<&str> = total_type
        .measure_unit_names()
        .expect("measure rule")
        .into_iter()
        .collect();
    assert_eq!(unit_names, vec!["eur", "usd", "gbp", "chf"]);
}

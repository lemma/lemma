//! Unit rows are not allowed on `measure range` or `ratio range`. Units belong to the
//! element type (`measure` or `ratio`); a range over it is written `<type> range`.

use indexmap::IndexMap;
use lemma::{DateTimeValue, Engine, EvaluateDataValue, SourceType};
use std::path::PathBuf;
use std::sync::Arc;

fn path_source(file: &str) -> SourceType {
    SourceType::Path(Arc::new(PathBuf::from(file)))
}

fn expect_plan_error(code: &str, fragment: &str) {
    let mut engine = Engine::new();
    let result = engine.load([(path_source("range_unit_ban.lemma"), code.to_string())]);
    let errors = result.expect_err("expected planning error, but load succeeded");
    let combined = errors
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ");
    assert!(
        combined.contains(fragment),
        "expected error containing '{fragment}', got: {combined}"
    );
}

fn rule_display(code: &str, spec: &str, rule: &str) -> String {
    let mut engine = Engine::new();
    engine
        .load([(path_source("range_unit_ban.lemma"), code.to_string())])
        .unwrap_or_else(|errs| panic!("load: {errs:?}"));
    let now = DateTimeValue::now();
    let data: IndexMap<String, EvaluateDataValue> = IndexMap::new();
    let response = engine
        .evaluate(None, spec, Some(&now), data, None, false)
        .expect("evaluate");
    let result = response
        .results
        .get(rule)
        .unwrap_or_else(|| panic!("rule {rule}"))
        .clone();
    assert!(
        !result.vetoed,
        "rule {rule} vetoed: {:?}",
        result.veto_reason
    );
    result.result().expect("result").to_string()
}

#[test]
fn measure_range_simple_unit_row_is_rejected() {
    let code = r#"
spec s
data band: measure range
  -> unit eur: 1
"#;
    expect_plan_error(code, "measure range");
}

#[test]
fn measure_range_compound_unit_row_is_rejected() {
    let code = r#"
spec s

uses lemma units

data money: measure
  -> unit eur: 1.00

data band: measure range
  -> unit eur_per_hour: eur/hour
"#;
    expect_plan_error(code, "measure range");
}

#[test]
fn ratio_range_unit_row_is_rejected() {
    let code = r#"
spec s
data band: ratio range
  -> unit basis_points: 10000
"#;
    expect_plan_error(code, "ratio range");
}

#[test]
fn measure_range_over_compound_measure_plans_and_evaluates() {
    let code = r#"
spec s

uses lemma units

data money: measure
  -> unit eur: 1.00

data rate: measure
  -> unit eur_per_hour: eur/hour

data wage_band: rate range
  -> with 10 eur_per_hour...100 eur_per_hour

data wage: 60 eur_per_hour

rule wage_in_band: wage in wage_band
"#;
    assert_eq!(rule_display(code, "s", "wage_in_band"), "true");
}

#[test]
fn ratio_range_over_custom_ratio_plans() {
    let code = r#"
spec s
data bps: ratio
  -> unit basis_points: 10000

data band: bps range
  -> suggest 200 basis_points...3500 basis_points

rule band_suggest_ok: true
"#;
    let mut engine = Engine::new();
    engine
        .load([(path_source("range_unit_ban.lemma"), code.to_string())])
        .unwrap_or_else(|errs| panic!("load: {errs:?}"));
}

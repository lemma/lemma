//! Range endpoint typing: every range kind × endpoint shape × with/suggest/eval/row.
//! Also instance-list row field references must planning-Err on kind/family mismatch.

use indexmap::IndexMap;
use lemma::{DateTimeValue, Engine, EvaluateDataValue, SourceType};
use std::path::PathBuf;
use std::sync::Arc;

fn path_source(file: &str) -> SourceType {
    SourceType::Path(Arc::new(PathBuf::from(file)))
}

fn load_ok(code: &str) -> Engine {
    let mut engine = Engine::new();
    engine
        .load([(path_source("range_endpoint_typing.lemma"), code.to_string())])
        .unwrap_or_else(|errs| {
            let joined = errs
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n");
            panic!("expected load to succeed, got: {joined}");
        });
    engine
}

fn load_err(code: &str) -> String {
    let mut engine = Engine::new();
    let err = engine
        .load([(path_source("range_endpoint_typing.lemma"), code.to_string())])
        .expect_err("expected load failure");
    err.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

fn assert_plan_err_unit_hint(code: &str) {
    let joined = load_err(code);
    assert!(
        joined.contains("hour")
            || joined.contains("unit")
            || joined.contains("cannot use")
            || joined.contains("measure")
            || joined.contains("Provide a range"),
        "expected unit/measure range typing error, got: {joined}"
    );
}

fn assert_plan_err_ratio_hint(code: &str) {
    let joined = load_err(code).to_lowercase();
    assert!(
        joined.contains("ratio")
            && (joined.contains("unit")
                || joined.contains("%")
                || joined.contains("percent")
                || joined.contains("bare")),
        "expected ratio unit syntax error, got: {joined}"
    );
}

fn now() -> DateTimeValue {
    DateTimeValue::now()
}

fn evaluate_rule(
    engine: &Engine,
    spec: &str,
    data: IndexMap<String, EvaluateDataValue>,
) -> Result<lemma::Response, lemma::Error> {
    engine.evaluate(None, spec, Some(&now()), data, None, false)
}

fn hours_type_block() -> &'static str {
    r#"
data hours: measure
  -> unit hour: 1
data money: measure
  -> unit eur: 1
data mass: measure
  -> unit kilogram: 1
  -> unit tonne: 1000
"#
}

// ─── A. Measure range ───────────────────────────────────────────────────────

#[test]
fn measure_range_with_both_units_same_ok() {
    load_ok(&format!(
        r#"
spec s
{hours}
data band: hours range
  -> with 8 hour...12 hour
rule out: band
"#,
        hours = hours_type_block()
    ));
}

#[test]
fn measure_range_suggest_both_units_same_ok() {
    load_ok(&format!(
        r#"
spec s
{hours}
data band: hours range
  -> suggest 8 hour...12 hour
rule out: band
"#,
        hours = hours_type_block()
    ));
}

#[test]
fn measure_range_with_mixed_compatible_units_ok() {
    load_ok(&format!(
        r#"
spec s
{hours}
data band: mass range
  -> with 500 kilogram...1 tonne
rule out: band
"#,
        hours = hours_type_block()
    ));
}

#[test]
fn measure_range_with_unit_left_bare_right_is_err() {
    assert_plan_err_unit_hint(&format!(
        r#"
spec s
{hours}
data band: hours range
  -> with 8 hour...12
rule out: band
"#,
        hours = hours_type_block()
    ));
}

#[test]
fn measure_range_suggest_unit_left_bare_right_is_err() {
    assert_plan_err_unit_hint(&format!(
        r#"
spec s
{hours}
data band: hours range
  -> suggest 8 hour...12
rule out: band
"#,
        hours = hours_type_block()
    ));
}

#[test]
fn measure_range_with_bare_left_unit_right_is_err() {
    assert_plan_err_unit_hint(&format!(
        r#"
spec s
{hours}
data band: hours range
  -> with 12...8 hour
rule out: band
"#,
        hours = hours_type_block()
    ));
}

#[test]
fn measure_range_suggest_bare_left_unit_right_is_err() {
    assert_plan_err_unit_hint(&format!(
        r#"
spec s
{hours}
data band: hours range
  -> suggest 12...8 hour
rule out: band
"#,
        hours = hours_type_block()
    ));
}

#[test]
fn measure_range_with_both_bare_is_err() {
    assert_plan_err_unit_hint(&format!(
        r#"
spec s
{hours}
data band: hours range
  -> with 8...12
rule out: band
"#,
        hours = hours_type_block()
    ));
}

#[test]
fn measure_range_suggest_both_bare_is_err() {
    assert_plan_err_unit_hint(&format!(
        r#"
spec s
{hours}
data band: hours range
  -> suggest 8...12
rule out: band
"#,
        hours = hours_type_block()
    ));
}

#[test]
fn measure_range_with_cross_family_units_is_err() {
    let joined = load_err(&format!(
        r#"
spec s
{hours}
data band: hours range
  -> with 8 hour...12 eur
rule out: band
"#,
        hours = hours_type_block()
    ));
    assert!(
        joined.contains("hour")
            || joined.contains("eur")
            || joined.contains("unit")
            || joined.contains("family")
            || joined.contains("cannot"),
        "expected cross-family/unit error, got: {joined}"
    );
}

#[test]
fn measure_range_with_date_endpoint_is_err() {
    let joined = load_err(&format!(
        r#"
spec s
{hours}
data band: hours range
  -> with 2024-01-01...12 hour
rule out: band
"#,
        hours = hours_type_block()
    ));
    assert!(
        !joined.is_empty(),
        "date endpoint on measure range must Err"
    );
}

#[test]
fn measure_range_with_time_endpoint_is_err() {
    let joined = load_err(&format!(
        r#"
spec s
{hours}
data band: hours range
  -> with 09:00...12 hour
rule out: band
"#,
        hours = hours_type_block()
    ));
    assert!(
        !joined.is_empty(),
        "time endpoint on measure range must Err"
    );
}

#[test]
fn measure_range_eval_overlay_unit_left_bare_right_no_panic() {
    let engine = load_ok(&format!(
        r#"
spec s
{hours}
data band: hours range
rule out: band
"#,
        hours = hours_type_block()
    ));
    let mut data = IndexMap::new();
    data.insert("band".to_string(), "8 hour...12".into());
    let result = evaluate_rule(&engine, "s", data);
    match result {
        Ok(resp) => {
            let rule = resp.results.get("out").expect("out");
            assert!(
                rule.vetoed,
                "bare measure-range endpoint must Veto, got value {:?}",
                rule.result()
            );
        }
        Err(err) => {
            let msg = err.to_string();
            assert!(
                msg.contains("hour")
                    || msg.contains("unit")
                    || msg.contains("cannot")
                    || msg.contains("measure"),
                "request Err must mention typing, got: {msg}"
            );
        }
    }
}

#[test]
fn measure_range_eval_overlay_valid_ok() {
    let engine = load_ok(&format!(
        r#"
spec s
{hours}
data band: hours range
rule out: band
"#,
        hours = hours_type_block()
    ));
    let mut data = IndexMap::new();
    data.insert("band".to_string(), "8 hour...12 hour".into());
    let resp = evaluate_rule(&engine, "s", data).expect("evaluate");
    let rule = resp.results.get("out").expect("out");
    assert!(
        !rule.vetoed,
        "valid measure range must not Veto: {:?}",
        rule.veto_reason
    );
}

#[test]
fn measure_range_instance_list_row_literal_bare_endpoint_is_err() {
    assert_plan_err_unit_hint(&format!(
        r#"
spec shift
{hours}
data band: hours range
spec roster
uses shifts: shift list
  -> with band: 8 hour...12
"#,
        hours = hours_type_block()
    ));
}

#[test]
fn measure_range_instance_list_row_literal_both_units_ok() {
    load_ok(&format!(
        r#"
spec shift
{hours}
data band: hours range
spec roster
uses shifts: shift list
  -> with band: 8 hour...12 hour
rule n: shifts -> count
"#,
        hours = hours_type_block()
    ));
}

// ─── B. Ratio range ─────────────────────────────────────────────────────────

#[test]
fn ratio_range_with_both_percent_ok() {
    load_ok(
        r#"
spec s
data band: ratio range
  -> with 10%...50%
rule out: band
"#,
    );
}

#[test]
fn ratio_range_suggest_both_percent_ok() {
    load_ok(
        r#"
spec s
data band: ratio range
  -> suggest 10%...50%
rule out: band
"#,
    );
}

#[test]
fn ratio_range_with_both_bare_is_err() {
    assert_plan_err_ratio_hint(
        r#"
spec s
data band: ratio range
  -> with 0.1...0.5
rule out: band
"#,
    );
}

#[test]
fn ratio_range_suggest_both_bare_is_err() {
    assert_plan_err_ratio_hint(
        r#"
spec s
data band: ratio range
  -> suggest 0.1...0.5
rule out: band
"#,
    );
}

#[test]
fn ratio_range_with_percent_then_bare_is_err() {
    assert_plan_err_ratio_hint(
        r#"
spec s
data band: ratio range
  -> with 10%...12
rule out: band
"#,
    );
}

#[test]
fn ratio_range_with_bare_then_percent_is_err() {
    assert_plan_err_ratio_hint(
        r#"
spec s
data band: ratio range
  -> with 0.05...10%
rule out: band
"#,
    );
}

#[test]
fn ratio_range_suggest_percent_then_bare_is_err() {
    assert_plan_err_ratio_hint(
        r#"
spec s
data band: ratio range
  -> suggest 10%...12
rule out: band
"#,
    );
}

#[test]
fn ratio_range_with_measure_endpoint_is_err() {
    let joined = load_err(&format!(
        r#"
spec s
{hours}
data band: ratio range
  -> with 10%...5 hour
rule out: band
"#,
        hours = hours_type_block()
    ));
    assert!(
        !joined.is_empty(),
        "measure endpoint on ratio range must Err"
    );
}

#[test]
fn ratio_range_eval_overlay_mixed_bare_ok() {
    let engine = load_ok(
        r#"
spec s
data band: ratio range
rule out: band
"#,
    );
    let mut data = IndexMap::new();
    data.insert("band".to_string(), "10%...50%".into());
    let resp = evaluate_rule(&engine, "s", data).expect("evaluate must not panic");
    let rule = resp.results.get("out").expect("out");
    assert!(
        !rule.vetoed,
        "ratio range overlay must succeed: {:?}",
        rule.veto_reason
    );
}

#[test]
fn ratio_range_eval_overlay_percent_then_bare_no_panic() {
    let engine = load_ok(
        r#"
spec s
data band: ratio range
rule out: band
"#,
    );
    let mut data = IndexMap::new();
    data.insert("band".to_string(), "10%...0.5".into());
    let result = evaluate_rule(&engine, "s", data);
    match result {
        Ok(resp) => {
            let _ = resp.results.get("out").expect("out");
        }
        Err(err) => {
            let msg = err.to_string();
            assert!(!msg.is_empty(), "request Err is fine; must not panic");
        }
    }
}

// ─── C. Number range ────────────────────────────────────────────────────────

#[test]
fn number_range_with_both_bare_ok() {
    load_ok(
        r#"
spec s
data band: number range
  -> with 10...100
rule out: band
"#,
    );
}

#[test]
fn number_range_suggest_both_bare_ok() {
    load_ok(
        r#"
spec s
data band: number range
  -> suggest 10...100
rule out: band
"#,
    );
}

#[test]
fn number_range_with_unit_on_right_is_err() {
    let joined = load_err(&format!(
        r#"
spec s
{hours}
data band: number range
  -> with 10...100 hour
rule out: band
"#,
        hours = hours_type_block()
    ));
    assert!(!joined.is_empty(), "unit on number range must Err");
}

#[test]
fn number_range_with_unit_on_left_is_err() {
    let joined = load_err(&format!(
        r#"
spec s
{hours}
data band: number range
  -> with 10 hour...100
rule out: band
"#,
        hours = hours_type_block()
    ));
    assert!(!joined.is_empty(), "unit on number range must Err");
}

#[test]
fn number_range_with_date_endpoint_is_err() {
    let joined = load_err(
        r#"
spec s
data band: number range
  -> with 2024-01-01...100
rule out: band
"#,
    );
    assert!(!joined.is_empty(), "date on number range must Err");
}

#[test]
fn number_range_with_time_endpoint_is_err() {
    let joined = load_err(
        r#"
spec s
data band: number range
  -> with 09:00...100
rule out: band
"#,
    );
    assert!(!joined.is_empty(), "time on number range must Err");
}

#[test]
fn number_range_eval_overlay_ok() {
    let engine = load_ok(
        r#"
spec s
data band: number range
rule out: band
"#,
    );
    let mut data = IndexMap::new();
    data.insert("band".to_string(), "10...100".into());
    let resp = evaluate_rule(&engine, "s", data).expect("evaluate");
    let rule = resp.results.get("out").expect("out");
    assert!(!rule.vetoed, "{:?}", rule.veto_reason);
}

// ─── D. Date range ──────────────────────────────────────────────────────────

#[test]
fn date_range_with_both_dates_ok() {
    load_ok(
        r#"
spec s
data band: date range
  -> with 2024-01-01...2024-12-31
rule out: band
"#,
    );
}

#[test]
fn date_range_suggest_both_dates_ok() {
    load_ok(
        r#"
spec s
data band: date range
  -> suggest 2024-01-01...2024-12-31
rule out: band
"#,
    );
}

#[test]
fn date_range_with_number_endpoint_is_err() {
    let joined = load_err(
        r#"
spec s
data band: date range
  -> with 2024-01-01...100
rule out: band
"#,
    );
    assert!(!joined.is_empty(), "number on date range must Err");
}

#[test]
fn date_range_with_time_endpoint_is_err() {
    let joined = load_err(
        r#"
spec s
data band: date range
  -> with 2024-01-01...17:00
rule out: band
"#,
    );
    assert!(!joined.is_empty(), "time on date range must Err");
}

#[test]
fn date_range_with_measure_endpoint_is_err() {
    let joined = load_err(&format!(
        r#"
spec s
{hours}
data band: date range
  -> with 2024-01-01...8 hour
rule out: band
"#,
        hours = hours_type_block()
    ));
    assert!(!joined.is_empty(), "measure on date range must Err");
}

// ─── E. Time range ──────────────────────────────────────────────────────────

#[test]
fn time_range_with_both_times_ok() {
    load_ok(
        r#"
spec s
data band: time range
  -> with 09:00...17:00
rule out: band
"#,
    );
}

#[test]
fn time_range_suggest_both_times_ok() {
    load_ok(
        r#"
spec s
data band: time range
  -> suggest 09:00...17:00
rule out: band
"#,
    );
}

#[test]
fn time_range_with_number_endpoint_is_err() {
    let joined = load_err(
        r#"
spec s
data band: time range
  -> with 09:00...100
rule out: band
"#,
    );
    assert!(!joined.is_empty(), "number on time range must Err");
}

#[test]
fn time_range_with_date_endpoint_is_err() {
    let joined = load_err(
        r#"
spec s
data band: time range
  -> with 09:00...2024-01-01
rule out: band
"#,
    );
    assert!(!joined.is_empty(), "date on time range must Err");
}

#[test]
fn time_range_with_measure_endpoint_is_err() {
    let joined = load_err(&format!(
        r#"
spec s
{hours}
data band: time range
  -> with 09:00...8 hour
rule out: band
"#,
        hours = hours_type_block()
    ));
    assert!(!joined.is_empty(), "measure on time range must Err");
}

// ─── F. Cross-schema ────────────────────────────────────────────────────────

#[test]
fn number_range_literal_on_measure_range_data_is_err() {
    assert_plan_err_unit_hint(&format!(
        r#"
spec s
{hours}
data band: hours range
  -> with 8...12
rule out: band
"#,
        hours = hours_type_block()
    ));
}

#[test]
fn measure_range_literal_on_number_range_data_is_err() {
    let joined = load_err(&format!(
        r#"
spec s
{hours}
data band: number range
  -> with 8 hour...12 hour
rule out: band
"#,
        hours = hours_type_block()
    ));
    assert!(
        !joined.is_empty(),
        "measure range literal on number range must Err"
    );
}

#[test]
fn measure_range_literal_on_ratio_range_data_is_err() {
    let joined = load_err(&format!(
        r#"
spec s
{hours}
data band: ratio range
  -> with 8 hour...12 hour
rule out: band
"#,
        hours = hours_type_block()
    ));
    assert!(
        !joined.is_empty(),
        "measure range literal on ratio range must Err"
    );
}

#[test]
fn ratio_range_literal_on_measure_range_data_is_err() {
    let joined = load_err(&format!(
        r#"
spec s
{hours}
data band: hours range
  -> with 10%...50%
rule out: band
"#,
        hours = hours_type_block()
    ));
    assert!(
        !joined.is_empty(),
        "ratio range literal on measure range must Err"
    );
}

// ─── G. Instance-list row references ────────────────────────────────────────

#[test]
fn row_ref_measure_field_to_number_is_err() {
    let joined = load_err(&format!(
        r#"
spec shift
{hours}
data duration: hours

spec roster
data qty: number
uses shifts: shift list
  -> with duration: qty
"#,
        hours = hours_type_block()
    ));
    assert!(
        joined.contains("type mismatch") || joined.contains("mismatch"),
        "number → measure row ref must type-mismatch, got: {joined}"
    );
}

#[test]
fn row_ref_measure_field_to_other_family_is_err() {
    let joined = load_err(
        r#"
spec units
data hours: measure
  -> unit hour: 1
data money: measure
  -> unit eur: 1

spec shift
uses units
data duration: units.hours

spec roster
uses units
data pay: units.money
uses shifts: shift list
  -> with duration: pay
"#,
    );
    assert!(
        joined.contains("family")
            || joined.contains("type mismatch")
            || joined.contains("mismatch"),
        "cross-family measure row ref must Err, got: {joined}"
    );
}

#[test]
fn row_ref_measure_field_to_same_family_ok() {
    load_ok(
        r#"
spec units
data hours: measure
  -> unit hour: 1

spec shift
uses units
data duration: units.hours

spec roster
uses units
data worked: units.hours
uses shifts: shift list
  -> with duration: worked
rule n: shifts -> count
"#,
    );
}

#[test]
fn row_ref_hours_range_field_to_number_is_err() {
    let joined = load_err(&format!(
        r#"
spec shift
{hours}
data band: hours range

spec roster
data qty: number
uses shifts: shift list
  -> with band: qty
"#,
        hours = hours_type_block()
    ));
    assert!(
        joined.contains("type mismatch") || joined.contains("mismatch"),
        "number → hours range row ref must Err, got: {joined}"
    );
}

#[test]
fn row_ref_hours_range_field_to_money_range_is_err() {
    let joined = load_err(
        r#"
spec units
data hours: measure
  -> unit hour: 1
data money: measure
  -> unit eur: 1

spec shift
uses units
data band: units.hours range

spec roster
uses units
data pay_band: units.money range
uses shifts: shift list
  -> with band: pay_band
"#,
    );
    assert!(
        joined.contains("family")
            || joined.contains("type mismatch")
            || joined.contains("mismatch"),
        "money range → hours range row ref must Err, got: {joined}"
    );
}

#[test]
fn row_ref_hours_range_field_to_hours_range_ok() {
    load_ok(
        r#"
spec units
data hours: measure
  -> unit hour: 1

spec shift
uses units
data band: units.hours range

spec roster
uses units
data window: units.hours range
  -> with 8 hour...12 hour
uses shifts: shift list
  -> with band: window
rule n: shifts -> count
"#,
    );
}

#[test]
fn row_ref_number_field_to_text_is_err() {
    let joined = load_err(
        r#"
spec item
data qty: number

spec order
data label: text
uses items: item list
  -> with qty: label
"#,
    );
    assert!(
        joined.contains("type mismatch") || joined.contains("mismatch"),
        "text → number row ref must Err, got: {joined}"
    );
}

#[test]
fn row_ref_text_field_to_number_is_err() {
    let joined = load_err(
        r#"
spec item
data label: text

spec order
data qty: number
uses items: item list
  -> with label: qty
"#,
    );
    assert!(
        joined.contains("type mismatch") || joined.contains("mismatch"),
        "number → text row ref must Err, got: {joined}"
    );
}

//! Qualify units: bare when unique; Type.unit; alias.Type.unit.
//! Import-alias sugar (alias.unit) is rejected.
//! Unique unit names: second independent declarer is a planning Error at definition/merge.

use lemma::{DateTimeValue, Engine, SourceType};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

fn path_source(file: &str) -> SourceType {
    SourceType::Path(Arc::new(PathBuf::from(file)))
}

fn eval_display(code: &str, spec: &str, rule: &str) -> String {
    let mut engine = Engine::new();
    engine
        .load([(path_source("qualified_units.lemma"), code.to_string())])
        .expect("load");
    let now = DateTimeValue::now();
    let response = engine
        .run(None, spec, Some(&now), HashMap::new(), None, false)
        .expect("run");
    response
        .results
        .get(rule)
        .unwrap_or_else(|| panic!("rule {rule}"))
        .result()
        .expect("result")
        .to_string()
}

fn expect_plan_error(code: &str) -> String {
    let mut engine = Engine::new();
    match engine.load([(path_source("bad.lemma"), code.to_string())]) {
        Err(err) => err
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; "),
        Ok(()) => panic!("expected planning error, but load succeeded"),
    }
}

#[test]
fn import_alias_unit_sugar_rejected() {
    let msg = expect_plan_error(
        r#"spec t
uses lemma units
rule r: 5 units.kilogram"#,
    );
    assert!(
        msg.contains("units.kilogram")
            && (msg.contains("Unknown")
                || msg.contains("alias.Type.unit")
                || msg.contains("not alias.unit")),
        "got: {msg}"
    );
}

#[test]
fn import_alias_unit_sugar_rejected_even_when_unique() {
    let msg = expect_plan_error(
        r#"spec t
uses lemma units
rule r: 5 units.kilogram"#,
    );
    assert!(
        !msg.to_lowercase().contains("ambiguous"),
        "must reject sugar as unknown path, not as ambiguity, got: {msg}"
    );
}

#[test]
fn as_import_alias_unit_sugar_rejected() {
    let msg = expect_plan_error(
        r#"spec t
uses lemma units
data x: 10 kilogram
rule r: x as units.kilogram"#,
    );
    assert!(msg.contains("units.kilogram"), "got: {msg}");
}

#[test]
fn compound_factor_import_alias_sugar_rejected() {
    let msg = expect_plan_error(
        r#"spec t
uses lemma units
data money: measure
  -> unit eur: 1
data rate: measure
  -> unit eur_per_kg: eur/units.kilogram"#,
    );
    assert!(
        msg.contains("units.kilogram"),
        "compound factor must reject alias.unit sugar, got: {msg}"
    );
}

#[test]
fn bare_unique_still_ok() {
    let code = r#"spec t
uses lemma units
rule a: 5 kilogram"#;
    assert!(eval_display(code, "t", "a").contains("kilogram"));
}

#[test]
fn three_segment_import_ok() {
    let code = r#"spec t
uses lemma units
rule c: 5 units.mass.kilogram"#;
    assert!(eval_display(code, "t", "c").contains("kilogram"));
}

#[test]
fn local_type_unit_ok() {
    let code = r#"spec t
data money: measure -> unit eur: 1
rule r: 5 money.eur"#;
    assert!(eval_display(code, "t", "r").contains("eur"));
}

#[test]
fn second_kilogram_declarer_vs_stdlib_rejected() {
    let msg = expect_plan_error(
        r#"spec t
uses lemma units
data bag: measure -> unit kilogram: 999
"#,
    );
    assert!(
        msg.contains("kilogram"),
        "local bag + stdlib kilogram must Error at definition, got: {msg}"
    );
    assert!(
        !msg.to_lowercase().contains("qualify as"),
        "must not be bare-use qualify list, got: {msg}"
    );
}

#[test]
fn second_eur_declarer_rejected() {
    let msg = expect_plan_error(
        r#"spec t
data money_a: measure -> unit eur: 1
data money_b: measure -> unit eur: 2
"#,
    );
    assert!(
        msg.contains("eur") && (msg.contains("money_a") || msg.contains("money_b")),
        "second independent eur declarer must Error, got: {msg}"
    );
}

#[test]
fn import_alias_unit_sugar_still_rejected_with_local_clash() {
    let sugar = expect_plan_error(
        r#"spec t
uses lemma units
data bag: measure -> unit kilogram: 999
rule sugar: 1 units.kilogram"#,
    );
    assert!(
        sugar.contains("units.kilogram") || sugar.contains("kilogram"),
        "sugar and/or second declarer must Error, got: {sugar}"
    );
}

#[test]
fn as_extension_unit() {
    let code = r#"spec t
data money_a: measure -> unit eur: 1
data money_b: money_a -> decimals 2
data x: 10 eur
rule r: x as money_b.eur"#;
    let display = eval_display(code, "t", "r");
    assert!(display.contains("eur"), "got {display}");
    assert!(
        display.contains("10.00") || display.contains("10"),
        "got {display}"
    );
}

#[test]
fn local_second_declarer_of_second_rejected() {
    let msg = expect_plan_error(
        r#"spec t
uses lemma units
data tick: measure -> unit second: 1
"#,
    );
    assert!(
        msg.contains("second"),
        "local tick.second + stdlib second must Error, got: {msg}"
    );
}

#[test]
fn dep_later_slice_second_kilogram_declarer_rejected() {
    let mut engine = Engine::new();
    let err = engine
        .load([(
            path_source("b.lemma"),
            r#"
spec B 2025-01-01
data mass: measure
  -> unit kilogram: 1

spec B 2025-07-01
data mass: measure
  -> unit kilogram: 1
data bag: measure
  -> unit kilogram: 999
"#
            .to_string(),
        )])
        .expect_err("B later slice must not declare a second kilogram");

    let joined = err
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ");
    assert!(
        joined.contains("kilogram"),
        "second kilogram declarer on B later row must Error, got: {joined}"
    );
    assert!(
        !joined.contains("BUG:"),
        "must be planning Error, not panic, got: {joined}"
    );
}

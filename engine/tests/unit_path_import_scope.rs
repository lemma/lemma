//! Bare units chain through imports; alias.unit / alias.Type.unit; local Type.unit only.

use indexmap::IndexMap;
use lemma::{DateTimeValue, Engine, SourceType};
use std::path::PathBuf;
use std::sync::Arc;

fn path_source(file: &str) -> SourceType {
    SourceType::Path(Arc::new(PathBuf::from(file)))
}

fn load(code: &str) -> Engine {
    let mut engine = Engine::new();
    engine
        .load([(path_source("unit_path.lemma"), code.to_string())])
        .unwrap_or_else(|errs| panic!("load: {errs:?}"));
    engine
}

fn eval(engine: &mut Engine, spec: &str, rule: &str) -> String {
    let now = DateTimeValue::now();
    let response = engine
        .evaluate(None, spec, Some(&now), IndexMap::new(), None, false)
        .expect("evaluate");
    response
        .results
        .get(rule)
        .expect("rule")
        .result()
        .expect("result")
        .to_string()
}

fn expect_err(code: &str) -> String {
    let mut engine = Engine::new();
    match engine.load([(path_source("bad.lemma"), code.to_string())]) {
        Err(err) => err
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; "),
        Ok(()) => panic!("expected planning error"),
    }
}

#[test]
fn lu_alone_allows_bare_and_path_forms() {
    let code = r#"
spec t
uses lu: lemma units
rule bare: 5 kilogram
rule alias_unit: 5 lu.kilogram
rule alias_type: 5 lu.mass.kilogram
"#;
    let mut engine = load(code);
    assert!(eval(&mut engine, "t", "bare").contains("kilogram"));
    assert!(eval(&mut engine, "t", "alias_unit").contains("kilogram"));
    assert!(eval(&mut engine, "t", "alias_type").contains("kilogram"));
}

#[test]
fn local_extension_of_imported_mass_owns_bare_kilogram() {
    let code = r#"
spec t
uses lu: lemma units
data weight: lu.mass
rule bare: 5 kilogram
rule typed: 5 weight.kilogram
"#;
    let mut engine = load(code);
    assert!(eval(&mut engine, "t", "bare").contains("kilogram"));
    assert!(eval(&mut engine, "t", "typed").contains("kilogram"));
}

#[test]
fn imported_type_unit_without_alias_rejected() {
    let msg = expect_err(
        r#"
spec t
uses lu: lemma units
rule r: 5 mass.kilogram
"#,
    );
    assert!(
        msg.contains("Unknown unit 'mass.kilogram'")
            || msg.contains("lu.mass.kilogram")
            || msg.contains("lu.kilogram"),
        "got: {msg}"
    );
}

#[test]
fn two_different_factor_kilograms_make_bare_ambiguous() {
    let msg = expect_err(
        r#"
spec t
uses lu: lemma units
data bag: measure -> unit kilogram: 999
rule r: 5 kilogram
"#,
    );
    assert!(
        msg.contains("bag.kilogram")
            && (msg.contains("lu.mass.kilogram") || msg.contains("lu.kilogram")),
        "got: {msg}"
    );
}

#[test]
fn show_types_lists_local_weight_units() {
    let code = r#"
spec line_item
uses lu: lemma units
data weight: lu.mass
data qty: number
rule mass: weight
"#;
    let engine = load(code);
    let show = engine
        .show(None, "line_item", Some(&DateTimeValue::now()))
        .expect("show");
    assert!(show.types.contains_key("weight"));
    assert!(show.types.contains_key("qty"));
    assert!(show.data.contains_key("weight"));
    assert!(!show.data.contains_key("qty"));
}

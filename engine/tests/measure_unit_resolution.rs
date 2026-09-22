//! Unique unit names: one declaring owner per unit name in scope.
//! Extensions inherit; bare binds the declarer / family root; qualified/cast bind the extension.

use lemma::{DateTimeValue, Engine, SourceType};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

fn path_source(file: &str) -> SourceType {
    SourceType::Path(Arc::new(PathBuf::from(file)))
}

fn load_and_run(code: &str, spec: &str) -> lemma::Response {
    let mut engine = Engine::new();
    engine
        .load([(path_source("test.lemma"), code.to_string())])
        .unwrap_or_else(|errs| {
            panic!(
                "load failed: {}",
                errs.errors
                    .iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        });
    let now = DateTimeValue::now();
    engine
        .run(None, spec, Some(&now), HashMap::new(), None, true)
        .unwrap_or_else(|e| panic!("run({spec}) failed: {e}"))
}

fn expect_load_error(code: &str) -> String {
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

fn rule_type(response: &lemma::Response, rule: &str) -> Arc<lemma::LemmaType> {
    let rr = response
        .results
        .get(rule)
        .unwrap_or_else(|| panic!("no rule '{rule}'"));
    assert!(!rr.vetoed, "rule '{rule}' vetoed: {:?}", rr.veto_reason);
    Arc::clone(&rr.rule.rule_type)
}

#[test]
fn bare_eur_binds_money_not_price() {
    let response = load_and_run(
        r#"spec test
data money: measure -> unit eur: 1
data price: money -> decimals 2
rule bare: 5 eur
rule qualified: 5 price.eur
"#,
        "test",
    );

    assert_eq!(rule_type(&response, "bare").name(), "money");
    assert_eq!(rule_type(&response, "bare").decimal_places(), None);
    assert_eq!(response.results["bare"].result().unwrap(), "5 eur");

    assert_eq!(rule_type(&response, "qualified").name(), "price");
    assert_eq!(rule_type(&response, "qualified").decimal_places(), Some(2));
    assert_eq!(response.results["qualified"].result().unwrap(), "5.00 eur");
}

#[test]
fn cast_as_price_eur_binds_price() {
    let response = load_and_run(
        r#"spec test
data money: measure -> unit eur: 1
data price: money -> decimals 2
data x: 10 eur
rule r: x as price.eur
"#,
        "test",
    );

    assert_eq!(rule_type(&response, "r").name(), "price");
    assert_eq!(rule_type(&response, "r").decimal_places(), Some(2));
}

#[test]
fn bare_eur_cast_as_extension() {
    let response = load_and_run(
        r#"spec test
data money_a: measure -> unit eur: 1
data money_b: money_a -> decimals 2
rule r: 5 eur as money_b.eur
"#,
        "test",
    );

    assert_eq!(rule_type(&response, "r").name(), "money_b");
    assert_eq!(rule_type(&response, "r").decimal_places(), Some(2));
}

#[test]
fn bare_kilogram_binds_stdlib_mass() {
    let response = load_and_run(
        r#"spec test
uses lemma units
data weight: units.mass -> decimals 2
rule bare: 5 kilogram
rule qualified: 5 weight.kilogram
"#,
        "test",
    );

    assert_eq!(rule_type(&response, "bare").name(), "mass");
    assert_eq!(rule_type(&response, "bare").decimal_places(), None);

    assert_eq!(rule_type(&response, "qualified").name(), "weight");
    assert_eq!(rule_type(&response, "qualified").decimal_places(), Some(2));
    assert_eq!(
        response.results["qualified"].result().unwrap(),
        "5.00 kilogram"
    );
}

#[test]
fn new_kg_binds_weight_inherited_kilogram_binds_mass() {
    let response = load_and_run(
        r#"spec test
uses lemma units
data weight: units.mass -> unit kg: 1
rule new_unit: 5 kg
rule inherited_bare: 5 kilogram
rule inherited_qualified: 5 weight.kilogram
"#,
        "test",
    );

    assert_eq!(rule_type(&response, "new_unit").name(), "weight");
    assert_eq!(rule_type(&response, "inherited_bare").name(), "mass");
    assert_eq!(rule_type(&response, "inherited_qualified").name(), "weight");
}

#[test]
fn units_mass_kilogram_binds_stdlib_mass() {
    let response = load_and_run(
        r#"spec test
uses lemma units
data weight: units.mass -> decimals 2
rule r: 5 units.mass.kilogram
"#,
        "test",
    );

    assert_eq!(rule_type(&response, "r").name(), "mass");
    assert_eq!(rule_type(&response, "r").decimal_places(), None);
}

#[test]
fn second_local_eur_declarer_rejected() {
    let msg = expect_load_error(
        r#"spec test
data money_a: measure -> unit eur: 1
data money_b: measure -> unit eur: 2
"#,
    );
    assert!(
        msg.contains("eur")
            && (msg.contains("money_a") || msg.contains("money_b"))
            && (msg.to_lowercase().contains("defined on both")
                || msg.to_lowercase().contains("declared only once")
                || msg.to_lowercase().contains("ambiguous")
                || msg.to_lowercase().contains("multiple")
                || msg.contains("Defined in")),
        "second independent eur declarer must Error at definition, got: {msg}"
    );
    assert!(
        !msg.contains("Qualify") && !msg.to_lowercase().contains("qualify as"),
        "must not be bare-use qualify-at-use Error, got: {msg}"
    );
}

#[test]
fn second_kilogram_declarer_vs_stdlib_rejected() {
    let msg = expect_load_error(
        r#"spec test
uses lemma units
data bag: measure -> unit kilogram: 999
"#,
    );
    assert!(
        msg.contains("kilogram")
            && (msg.to_lowercase().contains("defined on both")
                || msg.to_lowercase().contains("declared only once")
                || msg.to_lowercase().contains("ambiguous")
                || msg.to_lowercase().contains("multiple")
                || msg.contains("Defined in")
                || msg.contains("mass")
                || msg.contains("bag")),
        "local bag + stdlib mass both declaring kilogram must Error, got: {msg}"
    );
    assert!(
        !msg.to_lowercase().contains("qualify as"),
        "must be duplicate declarer Error, not bare ambiguous list, got: {msg}"
    );
}

#[test]
fn extension_new_unit_ok_duplicate_inherited_name_rejected() {
    let mut engine = Engine::new();
    engine
        .load([(
            path_source("ok.lemma"),
            r#"spec test
uses lemma units
data weight: units.mass -> unit kg: 1
rule r: 5 kg
"#
            .to_string(),
        )])
        .expect("new unit kg on extension must load");

    let msg = expect_load_error(
        r#"spec test
uses lemma units
data weight: units.mass -> unit kg: 1
data other: measure -> unit kilogram: 1
"#,
    );
    assert!(
        msg.contains("kilogram"),
        "second kilogram declarer must Error, got: {msg}"
    );
}

#[test]
fn two_deps_both_declaring_kilogram_rejected() {
    let mut engine = Engine::new();
    engine
        .load([(
            path_source("left.lemma"),
            r#"spec left
data mass: measure -> unit kilogram: 1
"#
            .to_string(),
        )])
        .expect("left alone must load");
    engine
        .load([(
            path_source("right.lemma"),
            r#"spec right
data bag: measure -> unit kilogram: 999
"#
            .to_string(),
        )])
        .expect("right alone must load");

    let err = engine
        .load([(
            path_source("consumer.lemma"),
            r#"spec consumer
uses left
uses right
"#
            .to_string(),
        )])
        .expect_err("consumer merging two kilogram declarers must Error");

    let joined = err
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ");
    assert!(
        joined.contains("kilogram"),
        "two imported declarers of kilogram must Error, got: {joined}"
    );
    assert!(
        !joined.to_lowercase().contains("qualify as"),
        "must be duplicate declarer Error, not bare qualify-at-use, got: {joined}"
    );
}

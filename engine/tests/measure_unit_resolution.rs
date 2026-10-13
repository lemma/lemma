//! Unit names resolve to their declarer in scope. A bare name declared by more than one
//! type must be qualified (`Type.unit`, `alias.Type.unit`).
//! Extensions inherit; bare binds the declarer / family root; qualified/cast bind the extension.

use indexmap::IndexMap;
use lemma::{DateTimeValue, Engine, SourceType};
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
        .evaluate(None, spec, Some(&now), IndexMap::new(), None, true)
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
fn two_local_eur_declarers_qualify_at_use() {
    let response = load_and_run(
        r#"spec test
data money_a: measure -> unit eur: 1
data money_b: measure -> unit eur: 2
rule a: 5 money_a.eur
rule b: 5 money_b.eur
"#,
        "test",
    );
    assert_eq!(rule_type(&response, "a").name.as_deref(), Some("money_a"));
    assert_eq!(rule_type(&response, "b").name.as_deref(), Some("money_b"));
}

#[test]
fn bare_eur_with_two_local_declarers_must_be_qualified() {
    let msg = expect_load_error(
        r#"spec test
data money_a: measure -> unit eur: 1
data money_b: measure -> unit eur: 2
rule r: 5 eur
"#,
    );
    assert!(
        msg.contains("money_a.eur") && msg.contains("money_b.eur"),
        "bare eur must list the qualified forms, got: {msg}"
    );
}

#[test]
fn local_kilogram_beside_stdlib_qualifies_at_use() {
    let response = load_and_run(
        r#"spec test
uses lemma units
data bag: measure -> unit kilogram: 999
rule local: 5 bag.kilogram
rule stdlib: 5 units.kilogram
rule stdlib_alias: 5 units.mass.kilogram
"#,
        "test",
    );
    assert_eq!(rule_type(&response, "local").name.as_deref(), Some("bag"));
    assert_eq!(rule_type(&response, "stdlib").name.as_deref(), Some("mass"));
    assert_eq!(
        rule_type(&response, "stdlib_alias").name.as_deref(),
        Some("mass")
    );
}

#[test]
fn bare_kilogram_beside_stdlib_must_be_qualified() {
    let msg = expect_load_error(
        r#"spec test
uses lemma units
data bag: measure -> unit kilogram: 999
rule r: 5 kilogram
"#,
    );
    assert!(
        msg.contains("bag.kilogram") && msg.contains("units.mass.kilogram"),
        "bare kilogram must list the qualified forms, got: {msg}"
    );
}

#[test]
fn extension_new_unit_beside_independent_inherited_name_declarer() {
    let response = load_and_run(
        r#"spec test
uses lemma units
data weight: units.mass -> unit kg: 1
data other: measure -> unit kilogram: 1
rule new_unit: 5 kg
rule inherited: 5 weight.kilogram
rule independent: 5 other.kilogram
"#,
        "test",
    );
    assert_eq!(
        rule_type(&response, "new_unit").name.as_deref(),
        Some("weight")
    );
    assert_eq!(
        rule_type(&response, "inherited").name.as_deref(),
        Some("weight")
    );
    assert_eq!(
        rule_type(&response, "independent").name.as_deref(),
        Some("other")
    );

    let msg = expect_load_error(
        r#"spec test
uses lemma units
data weight: units.mass -> unit kg: 1
data other: measure -> unit kilogram: 1
rule r: 5 kilogram
"#,
    );
    assert!(
        msg.contains("other.kilogram"),
        "bare kilogram must list the qualified forms, got: {msg}"
    );
}

#[test]
fn two_deps_both_declaring_kilogram_qualify_at_use() {
    let mut engine = Engine::new();
    engine
        .load([(
            path_source("deps.lemma"),
            r#"spec left
data mass: measure -> unit kilogram: 1

spec right
data bag: measure -> unit kilogram: 999
"#
            .to_string(),
        )])
        .expect("deps must load");
    engine
        .load([(
            path_source("consumer.lemma"),
            r#"spec consumer
uses left
uses right
rule from_left: 5 left.mass.kilogram
rule from_right: 5 right.bag.kilogram
"#
            .to_string(),
        )])
        .expect("consumer of two kilogram declarers must load");
    let now = DateTimeValue::now();
    let response = engine
        .evaluate(None, "consumer", Some(&now), IndexMap::new(), None, false)
        .expect("evaluate consumer");
    assert_eq!(
        rule_type(&response, "from_left").name.as_deref(),
        Some("mass")
    );
    assert_eq!(
        rule_type(&response, "from_right").name.as_deref(),
        Some("bag")
    );

    let err = engine
        .load([(
            path_source("bare.lemma"),
            r#"spec bare_consumer
uses left
uses right
rule r: 5 kilogram
"#
            .to_string(),
        )])
        .expect_err("bare kilogram from two imports must be qualified");
    let joined = err
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ");
    assert!(
        joined.contains("left.mass.kilogram") && joined.contains("right.bag.kilogram"),
        "bare kilogram must list the qualified forms, got: {joined}"
    );
}

//! Qualify units: bare when unique in scope; local Type.unit; alias.unit /
//! alias.Type.unit (and nested import paths). Imported Type.unit alone is rejected.
//! A unit name may have several independent declarers; a bare use of it must be qualified.

use indexmap::IndexMap;
use lemma::{DateTimeValue, Engine, EvaluateDataValue, SourceType};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

fn path_source(file: &str) -> SourceType {
    SourceType::Path(Arc::new(PathBuf::from(file)))
}

fn eval_display(code: &str, spec: &str, rule: &str) -> String {
    eval_display_with(code, spec, rule, IndexMap::new())
}

fn evaluate_rule(
    code: &str,
    spec: &str,
    rule: &str,
    data: IndexMap<String, EvaluateDataValue>,
) -> lemma::RuleResult {
    let mut engine = Engine::new();
    engine
        .load([(path_source("qualified_units.lemma"), code.to_string())])
        .unwrap_or_else(|errs| panic!("load: {errs:?}"));
    let now = DateTimeValue::now();
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
    result
}

fn eval_display_with(
    code: &str,
    spec: &str,
    rule: &str,
    data: IndexMap<String, EvaluateDataValue>,
) -> String {
    evaluate_rule(code, spec, rule, data)
        .result()
        .expect("result")
        .to_string()
}

fn rule_type_name(code: &str, spec: &str, rule: &str) -> Option<String> {
    evaluate_rule(code, spec, rule, IndexMap::new())
        .rule
        .rule_type
        .name
        .clone()
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
fn import_alias_unit_resolves() {
    let code = r#"spec t
uses lemma units
rule r: 5 units.kilogram"#;
    assert!(eval_display(code, "t", "r").contains("kilogram"));
}

#[test]
fn as_import_alias_unit_converts() {
    let code = r#"spec t
uses lemma units
data x: 10 kilogram
rule r: x as units.gram"#;
    assert_eq!(eval_display(code, "t", "r"), "10000 gram");
}

#[test]
fn compound_factor_import_alias_unit_binds() {
    let code = r#"spec t
uses lemma units
data money: measure
  -> unit eur: 1
data rate: measure
  -> unit eur_per_kg: eur/units.kilogram
rule cost: 2 eur_per_kg * 3 units.kilogram"#;
    assert_eq!(eval_display(code, "t", "cost"), "6 eur");
}

#[test]
fn imported_type_unit_without_alias_rejected() {
    let msg = expect_plan_error(
        r#"spec t
uses lemma units
rule r: 5 mass.kilogram"#,
    );
    assert!(
        msg.contains("Unknown unit 'mass.kilogram'")
            || msg.contains("units.mass.kilogram")
            || msg.contains("units.kilogram"),
        "got: {msg}"
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

const BAG_BESIDE_STDLIB: &str = r#"spec t
uses lemma units
data bag: measure -> unit kilogram: 999
data load: bag
data std: units.mass
rule load_total: load + 1 bag.kilogram
rule std_total: std + 2 units.mass.kilogram
rule gram_as_kilogram: 2000 gram as units.mass.kilogram
"#;

#[test]
fn typed_input_resolves_unit_against_its_own_type() {
    let data = IndexMap::from([
        ("load".to_string(), EvaluateDataValue::from("3 kilogram")),
        ("std".to_string(), EvaluateDataValue::from("1 kilogram")),
    ]);
    assert_eq!(
        eval_display_with(BAG_BESIDE_STDLIB, "t", "load_total", data.clone()),
        "4 kilogram"
    );
    assert_eq!(
        eval_display_with(BAG_BESIDE_STDLIB, "t", "std_total", data),
        "3 kilogram"
    );
}

#[test]
fn qualified_as_target_converts_within_its_family() {
    let data = IndexMap::from([
        ("load".to_string(), EvaluateDataValue::from("3 kilogram")),
        ("std".to_string(), EvaluateDataValue::from("1 kilogram")),
    ]);
    assert_eq!(
        eval_display_with(BAG_BESIDE_STDLIB, "t", "gram_as_kilogram", data),
        "2 kilogram"
    );
}

#[test]
fn bare_as_target_with_two_declarers_must_be_qualified() {
    let msg = expect_plan_error(
        r#"spec t
uses lemma units
data bag: measure -> unit kilogram: 999
rule r: 2000 gram as kilogram"#,
    );
    assert!(
        msg.contains("bag.kilogram") && msg.contains("units.mass.kilogram"),
        "got: {msg}"
    );
}

#[test]
fn adding_same_named_units_of_different_families_is_rejected() {
    let msg = expect_plan_error(
        r#"spec t
uses lemma units
data bag: measure -> unit kilogram: 999
rule r: 1 bag.kilogram + 1 units.mass.kilogram"#,
    );
    assert!(
        !msg.contains("BUG"),
        "cross-family add must be a planning Error, got: {msg}"
    );
}

#[test]
fn compound_unit_with_qualified_factor_binds_that_family() {
    let code = r#"spec t
uses lemma units
data money: measure -> unit eur: 1
data bag: measure -> unit kilogram: 999
data bag_price: measure -> unit eur_per_bag_kilogram: eur/bag.kilogram
data mass_price: measure -> unit eur_per_kilogram: eur/units.mass.kilogram
rule bag_cost: 2 eur_per_bag_kilogram * 3 bag.kilogram
rule mass_cost: 2 eur_per_kilogram * 3 units.mass.kilogram"#;
    assert_eq!(eval_display(code, "t", "bag_cost"), "6 eur");
    assert_eq!(eval_display(code, "t", "mass_cost"), "6 eur");
    assert_eq!(
        rule_type_name(code, "t", "bag_cost").as_deref(),
        Some("money")
    );
}

#[test]
fn compound_unit_with_ambiguous_bare_factor_must_be_qualified() {
    let msg = expect_plan_error(
        r#"spec t
uses lemma units
data money: measure -> unit eur: 1
data bag: measure -> unit kilogram: 999
data price: measure -> unit eur_per_kilogram: eur/kilogram"#,
    );
    assert!(
        msg.contains("bag.kilogram") && msg.contains("units.mass.kilogram"),
        "got: {msg}"
    );
}

const SQUARED_MASS: &str = r#"spec area_mass
uses lemma units
data squared_mass: measure -> unit kilogram_squared: kilogram^2
"#;

#[test]
fn cross_family_product_is_not_promoted_by_shared_bare_signature() {
    let same = format!(
        "{SQUARED_MASS}
spec t
uses lemma units
uses area_mass
data bag: measure -> unit kilogram: 999
rule same: 1 units.mass.kilogram * 1 units.mass.kilogram"
    );
    assert_eq!(
        rule_type_name(&same, "t", "same").as_deref(),
        Some("squared_mass")
    );

    for rule in [
        "rule mixed: 1 bag.kilogram * 1 units.mass.kilogram",
        "rule mixed: (1 bag.kilogram * 1 units.mass.kilogram) as kilogram_squared",
    ] {
        let msg = expect_plan_error(&format!(
            "{SQUARED_MASS}
spec t
uses lemma units
uses area_mass
data bag: measure -> unit kilogram: 999
{rule}"
        ));
        assert!(
            !msg.contains("BUG"),
            "bag * mass must be a planning Error, not mass^2, for `{rule}`: {msg}"
        );
    }
}

#[test]
fn import_alias_unit_still_ok_beside_local_clash() {
    let code = r#"spec t
uses lemma units
data bag: measure -> unit kilogram: 999
rule sugar: 1 units.kilogram
rule local: 1 bag.kilogram"#;
    assert_eq!(rule_type_name(code, "t", "sugar").as_deref(), Some("mass"));
    assert_eq!(rule_type_name(code, "t", "local").as_deref(), Some("bag"));
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
fn local_second_beside_stdlib_duration_qualifies_at_use() {
    let code = r#"spec t
uses lemma units
data tick: measure -> unit second: 1
rule local: 5 tick.second
rule stdlib: 5 units.duration.second
rule alias_unit: 5 units.second"#;
    assert_eq!(rule_type_name(code, "t", "local").as_deref(), Some("tick"));
    assert_eq!(
        rule_type_name(code, "t", "stdlib").as_deref(),
        Some("duration")
    );
    assert_eq!(
        rule_type_name(code, "t", "alias_unit").as_deref(),
        Some("duration")
    );
    let msg = expect_plan_error(
        r#"spec t
uses lemma units
data tick: measure -> unit second: 1
rule r: 5 second"#,
    );
    assert!(
        msg.contains("tick.second")
            && (msg.contains("units.duration.second") || msg.contains("units.second")),
        "got: {msg}"
    );
}

#[test]
fn dep_later_slice_second_kilogram_declarer_qualifies_at_use() {
    let mut engine = Engine::new();
    engine
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
rule packed: 5 bag.kilogram
"#
            .to_string(),
        )])
        .expect("B later slice may declare a second kilogram");
    let effective = DateTimeValue::from_str("2025-08-01").expect("date");
    let response = engine
        .evaluate(None, "B", Some(&effective), IndexMap::new(), None, false)
        .expect("evaluate B");
    let packed = response.results.get("packed").expect("packed");
    assert_eq!(packed.result(), Some("5 kilogram"));
    assert_eq!(packed.rule.rule_type.name.as_deref(), Some("bag"));
}

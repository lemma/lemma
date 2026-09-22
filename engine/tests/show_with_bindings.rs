//! `with` binding contracts for show vs run `RuleResult.missing_data`.
//!
//! - Rule / data references (`with bag.x: local_rule`): omitted from `missing_data`
//!   (not promptable). Show still documents reachable typed slots.
//! - Literal `with` (`with i.v: 42`): show may list as `fill`; never promptable
//!   required input. Run resolves without overlay.

use lemma::DateTimeValue;
use lemma::Engine;
use std::collections::HashMap;

fn missing_data_union(response: &lemma::Response) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut names = Vec::new();
    for result in response.results.values() {
        for key in result.missing_data() {
            if seen.insert(key.clone()) {
                names.push(key.clone());
            }
        }
    }
    names
}

fn missing_data_for_rule(response: &lemma::Response, rule: &str) -> Vec<String> {
    response
        .results
        .get(rule)
        .unwrap_or_else(|| panic!("rule {rule} missing from results"))
        .missing_data()
        .to_vec()
}

#[test]
fn run_omits_with_bound_rule_references_for_total_price() {
    let code = r#"
spec bag
uses lemma units

data weight: measure
  -> unit kg: 1

data money: measure
  -> unit eur: 1

data price_per_weight: measure
  -> unit eur_per_kg: eur/kg

data item_cost: price_per_weight
data roasting: price_per_weight
data chocolatizing: price_per_weight

rule total_price: weight * (item_cost + roasting + chocolatizing)

spec calc
uses bag
  -> with item_cost: item_cost
  -> with roasting: roasting

data type_of_nut: text -> options "peanut" "cashew"

rule price_peanut: 1.5 eur_per_kg
rule price_peanut_roasting: 0.45 eur_per_kg

rule price_cashew: 2.0 eur_per_kg
rule price_cashew_roasting: 0.55 eur_per_kg

rule item_cost: veto "No item cost"
  unless type_of_nut is "peanut" then price_peanut
  unless type_of_nut is "cashew" then price_cashew

rule roasting: veto "No roasting"
  unless type_of_nut is "peanut" then price_peanut_roasting
  unless type_of_nut is "cashew" then price_cashew_roasting

rule total_price: bag.total_price
"#;

    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "show_with_bindings.lemma",
            ))),
            code.to_string(),
        )])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(
            None,
            "calc",
            Some(&now),
            HashMap::new(),
            Some(&["total_price".to_string()]),
            false,
        )
        .expect("run must succeed");

    let names = missing_data_union(&response);
    assert!(
        !names.contains(&"bag.item_cost".to_string()),
        "with-bound rule ref must not appear in missing_data: {names:?}"
    );
    assert!(
        !names.contains(&"bag.roasting".to_string()),
        "with-bound rule ref must not appear in missing_data"
    );
    assert!(
        names.contains(&"bag.chocolatizing".to_string()),
        "unbound nested data must still appear"
    );
    assert!(
        names.contains(&"bag.weight".to_string()),
        "nested input data must appear"
    );
    assert!(
        names.contains(&"type_of_nut".to_string()),
        "local input for bound rules must appear"
    );

    let data_names: Vec<String> = names
        .into_iter()
        .filter(|k| !k.starts_with("bag.units."))
        .collect();
    assert_eq!(
        data_names,
        vec![
            "bag.weight".to_string(),
            "type_of_nut".to_string(),
            "bag.chocolatizing".to_string()
        ],
        "scoped data must list inputs in evaluation order"
    );
}

#[test]
fn show_lists_literal_with_binding_as_bound_not_promptable() {
    let code = r#"
spec inner
data v: number

spec outer
uses i: inner
  -> with v: 42
rule r: i.v
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let now = DateTimeValue::now();
    let show = engine.show(None, "outer", Some(&now)).unwrap();
    let entry = show
        .data
        .get("i.v")
        .expect("literal with still surfaces in show for documentation");

    assert!(
        entry.fill.is_some(),
        "literal with is fill; CLI skips, not a free input"
    );
}

#[test]
fn run_omits_with_bound_data_reference_for_rule_r() {
    let code = r#"
spec inner
data a: number -> suggest 1
data b: number

spec outer
uses i: inner
  -> with a: i.b
data input: number
rule r: i.a + input
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let now = DateTimeValue::now();
    let response = engine
        .run(
            None,
            "outer",
            Some(&now),
            HashMap::new(),
            Some(&["r".to_string()]),
            false,
        )
        .expect("run must succeed");

    let names = missing_data_union(&response);
    assert!(!names.contains(&"i.a".to_string()));
    assert!(names.contains(&"i.b".to_string()));
    assert!(names.contains(&"input".to_string()));
}

#[test]
fn run_still_resolves_with_bound_fields_without_show_inputs() {
    let code = r#"
spec inner
data v: number

spec outer
uses i: inner
  -> with v: 10
rule r: i.v
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let now = DateTimeValue::now();
    let resp = engine
        .run(None, "outer", Some(&now), HashMap::new(), None, false)
        .unwrap();
    let r = resp.results.values().find(|x| x.rule.name == "r").unwrap();
    assert_eq!(r.result(), Some("10"));
}

/// Spec that has genuinely different data requirements per branch — used to
/// verify that partial unless condition truth (supplied overlay only) prunes unreachable data.
const CHOOSER_LEMMA: &str = r#"
spec chooser

data mode: text -> options "simple" "complex"
data simple_input: number
data complex_input_a: number
data complex_input_b: number

rule result: veto "pick mode"
  unless mode is "simple" then simple_input
  unless mode is "complex" then complex_input_a + complex_input_b
"#;

#[test]
fn run_returns_all_data_on_fresh_plan() {
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, CHOOSER_LEMMA.to_string())])
        .unwrap();
    let now = DateTimeValue::now();
    let response = engine
        .run(
            None,
            "chooser",
            Some(&now),
            HashMap::new(),
            Some(&["result".to_string()]),
            false,
        )
        .expect("run must succeed");

    let names = missing_data_for_rule(&response, "result");
    assert_eq!(
        names,
        vec![
            "mode".to_string(),
            "complex_input_a".to_string(),
            "complex_input_b".to_string(),
            "simple_input".to_string()
        ],
        "fresh run should expose all data in evaluation order"
    );
}

#[test]
fn run_prunes_simple_branch_when_mode_is_complex() {
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, CHOOSER_LEMMA.to_string())])
        .unwrap();
    let now = DateTimeValue::now();
    let mut inputs = HashMap::new();
    inputs.insert("mode".to_string(), "complex".into());
    let response = engine
        .run(
            None,
            "chooser",
            Some(&now),
            inputs,
            Some(&["result".to_string()]),
            false,
        )
        .expect("run must succeed");

    let names = missing_data_for_rule(&response, "result");
    assert!(
        !names.contains(&"mode".to_string()),
        "supplied mode is bound and must not appear in missing_data"
    );
    assert!(names.contains(&"complex_input_a".to_string()));
    assert!(names.contains(&"complex_input_b".to_string()));
    assert!(
        !names.contains(&"simple_input".to_string()),
        "simple_input must be pruned when mode is complex: {names:?}"
    );
    assert_eq!(
        names,
        vec!["complex_input_a".to_string(), "complex_input_b".to_string()]
    );
}

#[test]
fn run_prunes_inactive_nut_branches_when_type_of_nut_supplied() {
    let code = r#"
spec bag
uses lemma units

data weight: measure
  -> unit kg: 1

data money: measure
  -> unit eur: 1

data price_per_weight: measure
  -> unit eur_per_kg: eur/kg

data item_cost: price_per_weight
data roasting: price_per_weight
data chocolatizing: price_per_weight

rule total_price: weight * (item_cost + roasting + chocolatizing)

spec calc
uses bag
  -> with item_cost: item_cost
  -> with roasting: roasting

data type_of_nut: text -> options "peanut" "cashew"

rule price_peanut: 1.5 eur_per_kg
rule price_peanut_roasting: 0.45 eur_per_kg

rule price_cashew: 2.0 eur_per_kg
rule price_cashew_roasting: 0.55 eur_per_kg

rule item_cost: veto "No item cost"
  unless type_of_nut is "peanut" then price_peanut
  unless type_of_nut is "cashew" then price_cashew

rule roasting: veto "No roasting"
  unless type_of_nut is "peanut" then price_peanut_roasting
  unless type_of_nut is "cashew" then price_cashew_roasting

rule total_price: bag.total_price
"#;

    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "calc_nut_prune.lemma",
            ))),
            code.to_string(),
        )])
        .unwrap();

    let now = DateTimeValue::now();
    let mut inputs = HashMap::new();
    inputs.insert("type_of_nut".to_string(), "peanut".into());
    let response = engine
        .run(
            None,
            "calc",
            Some(&now),
            inputs,
            Some(&["total_price".to_string()]),
            false,
        )
        .expect("run must succeed");

    let names = missing_data_union(&response);
    assert!(
        !names.contains(&"type_of_nut".to_string()),
        "supplied type_of_nut is bound and must not appear in missing_data: {names:?}"
    );
    assert!(names.contains(&"bag.weight".to_string()));
    assert!(names.contains(&"bag.chocolatizing".to_string()));
    assert!(
        !names.contains(&"bag.item_cost".to_string()),
        "with-bound item_cost must stay omitted: {names:?}"
    );
    assert!(
        !names.contains(&"bag.roasting".to_string()),
        "with-bound roasting must stay omitted: {names:?}"
    );
}

//! Behaviour of unless chains that planning folds into an ordered dispatch table.
//!
//! The fold is invisible by design: these cases pin the observable behaviour it must
//! not change — selected result, veto propagation, released data and narration.
//! Plan-shape assertions live in `engine/src/tests/transitive_normalization_plan_shape.rs`.

use lemma::{format_explanation, DateTimeValue, Engine, Response, RuleResult};
use std::collections::HashMap;

fn engine_for(code: &str) -> Engine {
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("spec must load");
    engine
}

fn run(engine: &Engine, spec: &str, bindings: &[(&str, &str)], explain: bool) -> Response {
    let now = DateTimeValue::now();
    let data: HashMap<String, String> = bindings
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    engine
        .run(None, spec, Some(&now), data, None, explain)
        .expect("evaluation must succeed")
}

fn rule<'a>(response: &'a Response, name: &str) -> &'a RuleResult {
    response.results.get(name).unwrap_or_else(|| {
        panic!(
            "rule '{name}' missing from results: {:?}",
            response.results.keys().collect::<Vec<_>>()
        )
    })
}

fn display(response: &Response, name: &str) -> String {
    rule(response, name)
        .display()
        .map(|d| d.to_string())
        .unwrap_or_else(|| {
            panic!(
                "rule '{name}' produced no value: {:?}",
                rule(response, name)
            )
        })
}

const LOOKUP: &str = r#"
spec lookup
data code: text
  -> option "NL"
  -> option "BE"
  -> option "DE"
rule name: veto "unknown code"
  unless code is "NL" then "Netherlands"
  unless code is "BE" then "Belgium"
  unless code is "DE" then "Germany"
"#;

/// Same exclusive is-chain without option constraints so an unknown code reaches
/// the veto default instead of failing input validation.
const LOOKUP_OPEN: &str = r#"
spec lookup
data code: text
rule name: veto "unknown code"
  unless code is "NL" then "Netherlands"
  unless code is "BE" then "Belgium"
  unless code is "DE" then "Germany"
"#;

const TIERS: &str = r#"
spec tiers
data quantity: number
rule discount: 0
  unless quantity >= 10 then 5
  unless quantity >= 100 then 10
  unless quantity is 50 then 7
"#;

/// `tip` borrows the Kind of `alias`, which borrows the Kind of the rule that
/// owns the fold: the Piecewise pre-image sits two references away, not one.
const REFERENCE_CHAIN: &str = r#"
spec chain
data code: text
rule country: "other"
  unless code is "NL" then "Netherlands"
  unless code is "BE" then "Belgium"
rule alias: country
rule tip: alias
"#;

/// The dispatch rule is reached through a `uses` binding, so inside the
/// consumer's plan the scrutinee is a rule reference, not a data leaf.
const BOUND_SCRUTINEE: &str = r#"
spec zones
data dest: text
rule zone_of: 2
  unless dest is "100" then 5
  unless dest is "200" then 8

spec rates
data zone: number
data weight: number
rule rate: 0
  unless zone is 5 and weight >= 1 then 14.20
  unless zone is 8 and weight >= 1 then 20.00

spec quote
uses z: zones
  -> with dest: dest
uses r: rates
  -> with zone: zone
  -> with weight: weight
data dest: text
data weight: number
rule zone: z.zone_of
rule total: r.rate
"#;

const RATE_CARD: &str = r#"
spec rates
data zone: number
data weight: number
rule rate: 0
  unless zone is 2 and weight >= 1 then 10
  unless zone is 2 and weight >= 5 then 20
  unless zone is 3 and weight >= 1 then 30
  unless zone is 3 and weight >= 5 then 40
"#;

#[test]
fn conjunctive_rate_card_selects_by_zone_and_weight() {
    let engine = engine_for(RATE_CARD);
    for (zone, weight, expected) in [
        ("1", "10", "0"),
        ("2", "0", "0"),
        ("2", "1", "10"),
        ("2", "4", "10"),
        ("2", "5", "20"),
        ("3", "1", "30"),
        ("3", "5", "40"),
        ("9", "100", "0"),
    ] {
        assert_eq!(
            display(
                &run(
                    &engine,
                    "rates",
                    &[("zone", zone), ("weight", weight)],
                    false
                ),
                "rate"
            ),
            expected,
            "zone={zone} weight={weight}"
        );
    }
}

#[test]
fn conjunctive_rate_card_explain_matches_value_mode() {
    let engine = engine_for(RATE_CARD);
    for (zone, weight) in [("2", "5"), ("3", "1"), ("1", "10")] {
        let without = display(
            &run(
                &engine,
                "rates",
                &[("zone", zone), ("weight", weight)],
                false,
            ),
            "rate",
        );
        let with = display(
            &run(
                &engine,
                "rates",
                &[("zone", zone), ("weight", weight)],
                true,
            ),
            "rate",
        );
        assert_eq!(
            without, with,
            "zone={zone} weight={weight}: explain must match value mode"
        );
    }
}

#[test]
fn conjunctive_rate_card_releases_weight_when_zone_misses() {
    let engine = engine_for(RATE_CARD);
    let response = run(&engine, "rates", &[("zone", "9")], false);
    assert!(
        rule(&response, "rate").missing_data().is_empty(),
        "no zone matches, so weight is not needed: {:?}",
        rule(&response, "rate").missing_data()
    );
}

#[test]
fn conjunctive_rate_card_still_needs_weight_when_zone_hits() {
    let engine = engine_for(RATE_CARD);
    let response = run(&engine, "rates", &[("zone", "2")], false);
    assert_eq!(
        rule(&response, "rate").missing_data(),
        vec!["weight".to_string()],
        "a matching zone still needs the weight residual"
    );
}

#[test]
fn equality_lookup_selects_the_matching_arm() {
    let engine = engine_for(LOOKUP);
    for (code, expected) in [("NL", "Netherlands"), ("BE", "Belgium"), ("DE", "Germany")] {
        assert_eq!(
            display(&run(&engine, "lookup", &[("code", code)], false), "name"),
            expected
        );
    }
}

#[test]
fn ordering_chain_holds_at_below_and_above_every_boundary() {
    let engine = engine_for(TIERS);
    // 50 is an exact hit on the last arm, which wins over the `>= 10` arm below it.
    for (quantity, expected) in [
        ("0", "0"),
        ("9", "0"),
        ("10", "5"),
        ("49", "5"),
        ("50", "7"),
        ("51", "5"),
        ("99", "5"),
        ("100", "10"),
        ("1000", "10"),
    ] {
        assert_eq!(
            display(
                &run(&engine, "tiers", &[("quantity", quantity)], false),
                "discount"
            ),
            expected,
            "quantity {quantity}"
        );
    }
}

#[test]
fn a_negative_scrutinee_lands_below_every_boundary() {
    let engine = engine_for(TIERS);
    assert_eq!(
        display(
            &run(&engine, "tiers", &[("quantity", "-1000")], false),
            "discount"
        ),
        "0"
    );
}

#[test]
fn an_unmatched_scrutinee_falls_through_to_the_default() {
    let engine = engine_for(
        r#"
spec lookup
data code: text
rule name: veto "unknown code"
  unless code is "NL" then "Netherlands"
"#,
    );
    let response = run(&engine, "lookup", &[("code", "ZZ")], false);
    assert!(
        rule(&response, "name").vetoed,
        "an unmatched code must reach the default veto: {:?}",
        rule(&response, "name")
    );
}

#[test]
fn an_unbound_scrutinee_propagates_as_missing_data() {
    let engine = engine_for(LOOKUP);
    let response = run(&engine, "lookup", &[], false);
    let result = rule(&response, "name");
    assert!(
        result.vetoed,
        "an unbound scrutinee must veto, got {result:?}"
    );
    assert_eq!(
        result.missing_data(),
        vec!["code".to_string()],
        "the scrutinee is the only thing the rule still needs"
    );
}

/// Each reference borrows the Kind of the one below it and records no pre-image
/// of its own, so the walk that reports missing data has to follow the chain
/// down to the rule that owns the fold.
#[test]
fn missing_data_travels_through_a_chain_of_references_to_a_dispatch_rule() {
    let engine = engine_for(REFERENCE_CHAIN);
    let response = run(&engine, "chain", &[], false);
    for name in ["country", "alias", "tip"] {
        let result = rule(&response, name);
        assert!(
            result.vetoed,
            "'{name}' has no value without a code: {result:?}"
        );
        assert_eq!(
            result.missing_data(),
            vec!["code".to_string()],
            "'{name}' still needs the scrutinee the references lead to"
        );
    }
}

#[test]
fn a_reference_chain_carries_the_value_it_borrows() {
    let engine = engine_for(REFERENCE_CHAIN);
    let response = run(&engine, "chain", &[("code", "BE")], false);
    for name in ["country", "alias", "tip"] {
        assert_eq!(display(&response, name), "Belgium", "rule '{name}'");
    }
}

#[test]
fn a_bound_rule_scrutinee_reports_the_data_behind_it() {
    let engine = engine_for(BOUND_SCRUTINEE);

    let response = run(&engine, "quote", &[("weight", "1")], false);
    let result = rule(&response, "total");
    assert!(
        result.vetoed,
        "the rule bound to the scrutinee has no value: {result:?}"
    );
    assert_eq!(result.missing_data(), vec!["dest".to_string()]);

    let response = run(&engine, "quote", &[("dest", "100"), ("weight", "1")], false);
    assert_eq!(display(&response, "total"), "14.2");
}

/// The regions the dispatch did not select must release their data, exactly as the
/// untaken arms of a Piecewise do.
#[test]
fn losing_regions_release_their_data() {
    let engine = engine_for(
        r#"
spec routing
data code: text
data dutch_rate: number
data belgian_rate: number
rule rate: 0
  unless code is "NL" then dutch_rate
  unless code is "BE" then belgian_rate
"#,
    );
    let response = run(&engine, "routing", &[("code", "NL")], false);
    let missing = rule(&response, "rate").missing_data();
    assert_eq!(
        missing,
        ["dutch_rate".to_string()].as_slice(),
        "only the selected region's data is still needed, got {missing:?}"
    );
}

#[test]
fn every_region_is_released_when_the_default_wins() {
    let engine = engine_for(
        r#"
spec routing
data code: text
data dutch_rate: number
data belgian_rate: number
rule rate: 0
  unless code is "NL" then dutch_rate
  unless code is "BE" then belgian_rate
"#,
    );
    let response = run(&engine, "routing", &[("code", "ZZ")], false);
    assert!(
        rule(&response, "rate").missing_data().is_empty(),
        "no arm can win, so neither rate is needed: {:?}",
        rule(&response, "rate").missing_data()
    );
}

/// A result shared by several regions stays live when one of them is selected.
#[test]
fn a_result_reachable_from_two_regions_is_not_released() {
    let engine = engine_for(
        r#"
spec shared
data code: text
data special: number
rule rate: 0
  unless code is "NL" then special
  unless code is "BE" then special
"#,
    );
    let response = run(&engine, "shared", &[("code", "BE")], false);
    assert_eq!(
        rule(&response, "rate").missing_data(),
        vec!["special".to_string()],
        "the selected region needs it, so it must not be released"
    );
}

#[test]
fn explain_and_value_modes_agree_at_every_tier_region() {
    let engine = engine_for(TIERS);
    for quantity in ["0", "9", "10", "49", "50", "51", "99", "100", "1000"] {
        let without = display(
            &run(&engine, "tiers", &[("quantity", quantity)], false),
            "discount",
        );
        let with = display(
            &run(&engine, "tiers", &[("quantity", quantity)], true),
            "discount",
        );
        assert_eq!(
            without, with,
            "quantity {quantity}: explain result must match value mode"
        );
    }
}

#[test]
fn explanation_states_the_matched_condition_with_its_data() {
    let engine = engine_for(TIERS);
    let response = run(&engine, "tiers", &[("quantity", "100")], true);
    let explanation = rule(&response, "discount")
        .explanation
        .as_ref()
        .expect("explanation built");

    let matched = explanation
        .causes
        .iter()
        .find(|cause| cause.condition == "quantity >= 100")
        .unwrap_or_else(|| panic!("matched condition missing from {:?}", explanation.causes));
    assert_eq!(matched.value, "true");
    assert!(
        matched
            .children
            .iter()
            .any(|child| format!("{child:?}").contains("100")),
        "the cause must carry the value that drove it, got {:?}",
        matched.children
    );
}

#[test]
fn explanation_states_untaken_conditions_as_the_flipped_fact() {
    let engine = engine_for(TIERS);
    let response = run(&engine, "tiers", &[("quantity", "10")], true);
    let explanation = rule(&response, "discount")
        .explanation
        .as_ref()
        .expect("explanation built");
    let conditions: Vec<&str> = explanation
        .causes
        .iter()
        .map(|cause| cause.condition.as_str())
        .collect();
    assert_eq!(
        conditions,
        vec!["quantity < 100", "quantity is not 50", "quantity >= 10"],
        "later last-wins failures then the winner"
    );
}

#[test]
fn exclusive_point_lookup_mid_hit_states_only_the_winner() {
    let engine = engine_for(LOOKUP);
    let response = run(&engine, "lookup", &[("code", "DE")], true);
    let explanation = rule(&response, "name")
        .explanation
        .as_ref()
        .expect("explanation built");
    let conditions: Vec<&str> = explanation
        .causes
        .iter()
        .map(|cause| cause.condition.as_str())
        .collect();
    assert_eq!(
        conditions,
        vec!["code is DE"],
        "exclusive is-chain keeps only the winning cause, got {:?}",
        explanation.causes
    );
    assert!(
        explanation.causes[0].children.iter().any(|child| {
            matches!(
                child,
                lemma::ExplanationNode::Data { name, display }
                    if name.input_key() == "code" && display == "DE"
            )
        }),
        "structured cause keeps the Data child, got {:?}",
        explanation.causes[0].children
    );
    let ascii = format_explanation(explanation);
    assert!(
        ascii.contains("code is DE"),
        "ASCII must state the winning cause, got {ascii}"
    );
    assert!(
        !ascii.contains("code: DE"),
        "ASCII must not restate the binding under a held is-line, got {ascii}"
    );
}

#[test]
fn inequality_matched_cause_still_expands_data_in_ascii() {
    let engine = engine_for(TIERS);
    let response = run(&engine, "tiers", &[("quantity", "100")], true);
    let explanation = rule(&response, "discount")
        .explanation
        .as_ref()
        .expect("explanation built");
    let ascii = format_explanation(explanation);
    assert!(
        ascii.contains("quantity >= 100"),
        "ASCII must state the matched inequality, got {ascii}"
    );
    assert!(
        ascii.contains("quantity: 100"),
        "ASCII must expand exact magnitude under inequality, got {ascii}"
    );
}

#[test]
fn exclusive_point_lookup_default_attaches_scrutinee_without_is_not_dump() {
    let engine = engine_for(LOOKUP_OPEN);
    let response = run(&engine, "lookup", &[("code", "FR")], true);
    let result = rule(&response, "name");
    assert!(result.vetoed, "unknown code must veto");
    let explanation = result.explanation.as_ref().expect("explanation built");
    assert!(
        explanation.causes.is_empty(),
        "exclusive default must not dump is-not causes, got {:?}",
        explanation.causes
    );
    assert!(
        explanation.children.iter().any(|child| {
            matches!(
                child,
                lemma::ExplanationNode::Data { name, display }
                    if name.input_key() == "code" && display == "FR"
            ) || matches!(
                child,
                lemma::ExplanationNode::Compose { operands, .. }
                    if operands.iter().any(|op| {
                        matches!(
                            op,
                            lemma::ExplanationNode::Data { name, display }
                                if name.input_key() == "code" && display == "FR"
                        )
                    })
            )
        }),
        "exclusive default must attach scrutinee data, got {:?}",
        explanation.children
    );
}

/// Narration walks the pre-image chain, so a vetoed scrutinee still reports the
/// condition it was evaluating — the dispatch table has no conditions to name.
#[test]
fn a_vetoed_scrutinee_narrates_a_condition_from_the_pre_image() {
    let engine = engine_for(LOOKUP);
    let response = run(&engine, "lookup", &[], true);
    let explanation = rule(&response, "name")
        .explanation
        .as_ref()
        .expect("explanation built");
    assert_eq!(
        explanation.body, "code is DE",
        "the reverse scan starts at the last arm and propagates its veto"
    );
}

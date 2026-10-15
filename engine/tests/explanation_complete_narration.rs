//! Integration contracts: explanations narrate everything normalization folds away.
//! Exact `assert_eq!` on typed Explanation/Cause fields and JSON node paths — never `.contains()`.

#[path = "support/explanation_json.rs"]
mod explanation_json;
#[path = "support/explanation_json_walk.rs"]
mod explanation_support;

use indexmap::IndexMap;
use lemma::{DateTimeValue, Engine, EvaluateDataValue, Explanation, ExplanationNode};

fn load(code: &str) -> Engine {
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("spec must load");
    engine
}

fn run(
    engine: &Engine,
    spec: &str,
    data: IndexMap<String, EvaluateDataValue>,
    rules: Option<&[String]>,
    explain: bool,
) -> lemma::Response {
    let now = DateTimeValue::now();
    engine
        .evaluate(None, spec, Some(&now), data, rules, explain)
        .expect("evaluation must succeed")
}

fn out_explanation(response: &lemma::Response) -> &Explanation {
    response
        .results
        .get("out")
        .expect("out in response")
        .explanation
        .as_ref()
        .expect("out explanation")
}

fn cause_pairs(explanation: &Explanation) -> Vec<(&str, &str)> {
    explanation
        .causes
        .iter()
        .map(|c| (c.condition.as_str(), c.value.as_str()))
        .collect()
}

fn explanation_json(explanation: &Explanation) -> serde_json::Value {
    serde_json::to_value(explanation).expect("serialize explanation")
}

fn compose_expressions(value: &serde_json::Value) -> Vec<&str> {
    let mut found = Vec::new();
    fn walk<'a>(value: &'a serde_json::Value, found: &mut Vec<&'a str>) {
        match value {
            serde_json::Value::Object(obj) => {
                if obj.get("type").and_then(|t| t.as_str()) == Some("compose") {
                    if let Some(expr) = obj.get("expression").and_then(|e| e.as_str()) {
                        found.push(expr);
                    }
                }
                for child in obj.values() {
                    walk(child, found);
                }
            }
            serde_json::Value::Array(arr) => {
                for child in arr {
                    walk(child, found);
                }
            }
            _ => {}
        }
    }
    walk(value, &mut found);
    found
}

fn rule_names(value: &serde_json::Value) -> Vec<&str> {
    explanation_support::json_explained_rules(value)
        .into_iter()
        .filter_map(|rule| rule.get("name").and_then(|n| n.as_str()))
        .collect()
}

fn conversion_expressions(value: &serde_json::Value) -> Vec<&str> {
    let mut found = Vec::new();
    fn walk<'a>(value: &'a serde_json::Value, found: &mut Vec<&'a str>) {
        match value {
            serde_json::Value::Object(obj) => {
                if obj.get("type").and_then(|t| t.as_str()) == Some("conversion") {
                    if let Some(expr) = obj.get("expression").and_then(|e| e.as_str()) {
                        found.push(expr);
                    }
                }
                for child in obj.values() {
                    walk(child, found);
                }
            }
            serde_json::Value::Array(arr) => {
                for child in arr {
                    walk(child, found);
                }
            }
            _ => {}
        }
    }
    walk(value, &mut found);
    found
}

// ── Suite 1: piecewise collapse must narrate ─────────────────────────────

#[test]
fn static_false_comparison_unless_explains_flipped_fact() {
    let engine = load(
        r#"
spec flipped_cmp
rule out: true
  unless 5 < 3 then false
"#,
    );
    let response = run(
        &engine,
        "flipped_cmp",
        IndexMap::new(),
        Some(&["out".to_string()]),
        true,
    );
    let out = response.results.get("out").expect("out");
    assert_eq!(out.result(), Some("true"));
    let explanation = out_explanation(&response);
    assert_eq!(explanation.body, "true");
    assert_eq!(cause_pairs(explanation), vec![("5 >= 3", "true")]);
}

#[test]
fn static_false_literal_unless_explains_falsified_arm() {
    let engine = load(
        r#"
spec falsified_literal
rule out: 1 unless false then 2
"#,
    );
    let response = run(
        &engine,
        "falsified_literal",
        IndexMap::new(),
        Some(&["out".to_string()]),
        true,
    );
    assert_eq!(
        response.results.get("out").expect("out").result(),
        Some("1")
    );
    let explanation = out_explanation(&response);
    assert_eq!(explanation.body, "1");
    assert_eq!(cause_pairs(explanation), vec![("false", "false")]);
}

#[test]
fn static_true_unless_explains_held_winning_condition() {
    let engine = load(
        r#"
spec held_true
rule out: 1 unless true then 2
"#,
    );
    let response = run(
        &engine,
        "held_true",
        IndexMap::new(),
        Some(&["out".to_string()]),
        true,
    );
    assert_eq!(
        response.results.get("out").expect("out").result(),
        Some("2")
    );
    let explanation = out_explanation(&response);
    assert_eq!(explanation.body, "2");
    assert_eq!(cause_pairs(explanation), vec![("true", "true")]);
}

#[test]
fn static_true_comparison_unless_explains_winning_comparison() {
    let engine = load(
        r#"
spec held_cmp
rule out: 1 unless 5 > 3 then 2
"#,
    );
    let response = run(
        &engine,
        "held_cmp",
        IndexMap::new(),
        Some(&["out".to_string()]),
        true,
    );
    assert_eq!(
        response.results.get("out").expect("out").result(),
        Some("2")
    );
    let explanation = out_explanation(&response);
    assert_eq!(explanation.body, "2");
    assert_eq!(cause_pairs(explanation), vec![("5 > 3", "true")]);
}

#[test]
fn static_true_winner_omits_shadowed_earlier_arms() {
    let engine = load(
        r#"
spec shadowed
data x: boolean
rule out: 1 unless x then 2 unless true then 3
"#,
    );
    let response = run(
        &engine,
        "shadowed",
        IndexMap::new(),
        Some(&["out".to_string()]),
        true,
    );
    assert_eq!(
        response.results.get("out").expect("out").result(),
        Some("3")
    );
    let explanation = out_explanation(&response);
    assert_eq!(explanation.body, "3");
    assert_eq!(cause_pairs(explanation), vec![("true", "true")]);
}

#[test]
fn partial_dead_arm_among_live_piecewise_narrates_dead() {
    let engine = load(
        r#"
spec partial_dead
data x: boolean
rule out: 1 unless false then 2 unless x then 3
"#,
    );
    let data = IndexMap::from([("x".into(), "true".into())]);
    let response = run(
        &engine,
        "partial_dead",
        data,
        Some(&["out".to_string()]),
        true,
    );
    assert_eq!(
        response.results.get("out").expect("out").result(),
        Some("3")
    );
    let explanation = out_explanation(&response);
    assert_eq!(explanation.body, "3");
    assert_eq!(cause_pairs(explanation), vec![("x", "true")]);
}

#[test]
fn and_false_conjunct_static_states_false_literal() {
    let engine = load(
        r#"
spec and_false_flag
data flag: boolean
rule out: 1 unless flag and false then 2
"#,
    );
    let response = run(
        &engine,
        "and_false_flag",
        IndexMap::new(),
        Some(&["out".to_string()]),
        true,
    );
    assert_eq!(
        response.results.get("out").expect("out").result(),
        Some("1")
    );
    let explanation = out_explanation(&response);
    assert_eq!(explanation.body, "1");
    assert_eq!(
        cause_pairs(explanation),
        vec![("false", "false")],
        "static false conjunct decides; unused flag is not a cause child"
    );
    assert!(
        explanation.causes[0].children.is_empty(),
        "literal false cause has no children, got {:?}",
        explanation.causes[0].children
    );
}

#[test]
fn bound_and_false_states_failing_left_conjunct() {
    let engine = load(
        r#"
spec bound_and
data a: boolean
data b: boolean
rule out: "ok"
  unless a and b then "both"
"#,
    );
    let mut data = IndexMap::new();
    data.insert("a".into(), "false".into());
    data.insert("b".into(), "true".into());
    let response = run(&engine, "bound_and", data, Some(&["out".to_string()]), true);
    assert_eq!(
        response.results.get("out").expect("out").result(),
        Some("ok")
    );
    let explanation = out_explanation(&response);
    assert_eq!(
        cause_pairs(explanation),
        vec![("a is false", "true")],
        "false and states the deciding left conjunct"
    );
    assert!(
        explanation.causes[0].children.is_empty(),
        "bare bool data is fully stated in the cause line, got {:?}",
        explanation.causes[0].children
    );
    let formatted = lemma::format_explanation(explanation);
    assert!(
        formatted.contains("a is false"),
        "ASCII states the failing conjunct, got {formatted}"
    );
    assert!(
        !formatted.contains("a and b"),
        "ASCII must not keep the authored and line, got {formatted}"
    );
}

#[test]
fn and_false_right_conjunct_decides_over_missing_left_sibling() {
    let engine = load(
        r#"
spec and_missing_sibling
data flag: boolean
data model: text
rule out: 0
  unless flag and model is "c" then 3
"#,
    );
    let mut data = IndexMap::new();
    data.insert("model".into(), "b".into());
    let response = run(
        &engine,
        "and_missing_sibling",
        data,
        Some(&["out".to_string()]),
        true,
    );
    assert_eq!(
        response.results.get("out").expect("out").result(),
        Some("0"),
        "model is not c decides the and; missing flag must not veto"
    );
    let explanation = out_explanation(&response);
    let formatted = lemma::format_explanation(explanation);
    assert!(
        formatted.contains("model"),
        "explanation names the false conjunct, got {formatted}"
    );
    assert!(
        !formatted.contains("flag"),
        "explanation must not name the undecided missing sibling, got {formatted}"
    );
}

#[test]
fn and_missing_left_explanation_narrates_deciding_right_operand() {
    let engine = load(
        r#"
spec and_body_narration
data flag: boolean
data model: text
rule r: flag and model is "c"
"#,
    );
    let mut data = IndexMap::new();
    data.insert("model".into(), "b".into());
    let response = run(
        &engine,
        "and_body_narration",
        data,
        Some(&["r".to_string()]),
        true,
    );
    assert_eq!(
        response.results.get("r").expect("r").result(),
        Some("false"),
        "model is c decides the and; missing flag must not veto"
    );
    let explanation = response
        .results
        .get("r")
        .expect("r")
        .explanation
        .as_ref()
        .expect("r explanation");
    let formatted = lemma::format_explanation(explanation);
    assert!(
        formatted.contains("model: b"),
        "the deciding right operand is narrated, got {formatted}"
    );
}

#[test]
fn and_short_circuit_left_fail_omits_right_from_cause() {
    let engine = load(
        r#"
spec short_and
data a: boolean
data b: boolean
rule out: "ok"
  unless a and b then "both"
"#,
    );
    let mut data = IndexMap::new();
    data.insert("a".into(), "false".into());
    let response = run(&engine, "short_and", data, Some(&["out".to_string()]), true);
    assert_eq!(
        response.results.get("out").expect("out").result(),
        Some("ok")
    );
    let explanation = out_explanation(&response);
    assert_eq!(cause_pairs(explanation), vec![("a is false", "true")]);
    let formatted = lemma::format_explanation(explanation);
    assert!(
        formatted.contains("a is false"),
        "ASCII states the failing left, got {formatted}"
    );
    assert!(
        !formatted.contains("b"),
        "short-circuit right must not appear under the cause, got {formatted}"
    );
}

#[test]
fn and_right_comparison_fail_states_flipped_fact() {
    let engine = load(
        r#"
spec rental_view
data active: boolean
data views_consumed: number
data max_views: number
rule can_view: false
  unless active and views_consumed < max_views then true
"#,
    );
    let mut data = IndexMap::new();
    data.insert("active".into(), "true".into());
    data.insert("views_consumed".into(), "6".into());
    data.insert("max_views".into(), "5".into());
    let response = run(
        &engine,
        "rental_view",
        data,
        Some(&["can_view".to_string()]),
        true,
    );
    assert_eq!(
        response.results.get("can_view").expect("can_view").result(),
        Some("false")
    );
    let explanation = response
        .results
        .get("can_view")
        .expect("can_view")
        .explanation
        .as_ref()
        .expect("explanation");
    assert_eq!(
        cause_pairs(explanation),
        vec![("views_consumed >= max_views", "true")],
        "right conjunct decides; flipped comparison, got {:?}",
        explanation.causes
    );
    let formatted = lemma::format_explanation(explanation);
    assert!(
        !formatted.contains(" is false"),
        "ASCII must not tag the and as is false, got {formatted}"
    );
    assert!(
        !formatted.contains(" and "),
        "ASCII must not print the authored and line, got {formatted}"
    );
}

#[test]
fn and_left_comparison_fail_omits_unbound_right() {
    let engine = load(
        r#"
spec left_fail
data code: text
data amount: number
rule out: "no"
  unless code is "NL" and amount > 0 then "yes"
"#,
    );
    let mut data = IndexMap::new();
    data.insert("code".into(), "BE".into());
    let response = run(&engine, "left_fail", data, Some(&["out".to_string()]), true);
    assert_eq!(
        response.results.get("out").expect("out").result(),
        Some("no")
    );
    let explanation = out_explanation(&response);
    assert_eq!(
        cause_pairs(explanation),
        vec![("code is not NL", "true")],
        "left comparison fails first"
    );
    let formatted = lemma::format_explanation(explanation);
    assert!(
        !formatted.contains("amount"),
        "unbound right conjunct must stay out of the cause, got {formatted}"
    );
}

#[test]
fn mixed_static_false_then_static_true_only_held_cause() {
    let engine = load(
        r#"
spec mixed_static
rule out: 0 unless 1 < 0 then 1 unless true then 2
"#,
    );
    let response = run(
        &engine,
        "mixed_static",
        IndexMap::new(),
        Some(&["out".to_string()]),
        true,
    );
    assert_eq!(
        response.results.get("out").expect("out").result(),
        Some("2")
    );
    let explanation = out_explanation(&response);
    assert_eq!(explanation.body, "2");
    assert_eq!(cause_pairs(explanation), vec![("true", "true")]);
}

// ── Suite 2: algebra provenance ──────────────────────────────────────────

#[test]
fn distinct_sqrt_product_preserves_both_sqrt_preimages() {
    let engine = load(
        r#"
spec sqrt_product
rule out: (sqrt 4) * (sqrt 9)
"#,
    );
    let response = run(
        &engine,
        "sqrt_product",
        IndexMap::new(),
        Some(&["out".to_string()]),
        true,
    );
    assert_eq!(
        response.results.get("out").expect("out").result(),
        Some("6")
    );
    let explanation = out_explanation(&response);
    assert_eq!(explanation.body, "sqrt(4) * sqrt(9)");
    let json = explanation_json(explanation);
    let mut sqrts: Vec<&str> = compose_expressions(&json)
        .into_iter()
        .filter(|e| *e == "sqrt 4" || *e == "sqrt 9")
        .collect();
    sqrts.sort_unstable();
    sqrts.dedup();
    assert_eq!(sqrts, vec!["sqrt 4", "sqrt 9"]);
}

#[test]
fn sqrt_of_distinct_literals_fold_still_explains_sqrts() {
    let engine = load(
        r#"
spec sqrt_rules
rule a: sqrt 4
rule b: sqrt 9
rule out: a * b
"#,
    );
    let response = run(
        &engine,
        "sqrt_rules",
        IndexMap::new(),
        Some(&["out".to_string()]),
        true,
    );
    assert_eq!(
        response.results.get("out").expect("out").result(),
        Some("6")
    );
    let explanation = out_explanation(&response);
    let json = explanation_json(explanation);
    let mut names = rule_names(&json);
    names.sort_unstable();
    assert_eq!(names, vec!["a", "b", "out"]);
    let bodies: Vec<&str> = explanation_support::json_explained_rules(&json)
        .into_iter()
        .filter_map(|rule| rule.get("body").and_then(|b| b.as_str()))
        .collect();
    let mut sqrts: Vec<&str> = bodies
        .into_iter()
        .filter(|b| *b == "sqrt 4" || *b == "sqrt 9")
        .collect();
    sqrts.sort_unstable();
    sqrts.dedup();
    assert_eq!(sqrts, vec!["sqrt 4", "sqrt 9"]);
}

#[test]
fn named_compound_measure_literal_explains_named_unit() {
    let engine = load(
        r#"
spec named_rate
uses lemma units
data money: measure
  -> unit eur: 1
data rate: measure
  -> unit eur_per_hour: eur/hour
rule out: 50 eur_per_hour
"#,
    );
    let response = run(
        &engine,
        "named_rate",
        IndexMap::new(),
        Some(&["out".to_string()]),
        true,
    );
    assert_eq!(
        response.results.get("out").expect("out").result(),
        Some("50 eur_per_hour")
    );
    let explanation = out_explanation(&response);
    assert_eq!(explanation.body, "50 eur_per_hour");
}

#[test]
fn nested_identity_elim_keeps_inner_fold_in_explanation() {
    let engine = load(
        r#"
spec nested_identity
rule out: (exp(log(5))) + 0
"#,
    );
    let response = run(
        &engine,
        "nested_identity",
        IndexMap::new(),
        Some(&["out".to_string()]),
        true,
    );
    assert_eq!(
        response.results.get("out").expect("out").result(),
        Some("5")
    );
    let explanation = out_explanation(&response);
    assert_eq!(explanation.body, "exp log 5");
}

// ── Suite 3: conversion and order ────────────────────────────────────────

#[test]
fn number_as_number_conversion_appears_in_explanation() {
    let engine = load(
        r#"
spec as_number
rule out: 100 as number
"#,
    );
    let response = run(
        &engine,
        "as_number",
        IndexMap::new(),
        Some(&["out".to_string()]),
        true,
    );
    assert_eq!(
        response.results.get("out").expect("out").result(),
        Some("100")
    );
    let explanation = out_explanation(&response);
    assert_eq!(explanation.body, "100 as number");
    let json = explanation_json(explanation);
    assert_eq!(conversion_expressions(&json), vec!["100 as number"]);
}

#[test]
fn measure_as_unit_conversion_narrated_when_folded() {
    let engine = load(
        r#"
spec measure_as
uses lemma units
data mass: units.mass
  -> suggest 2 kilogram
rule out: mass as gram
"#,
    );
    let data = IndexMap::from([("mass".into(), "2 kilogram".into())]);
    let response = run(
        &engine,
        "measure_as",
        data,
        Some(&["out".to_string()]),
        true,
    );
    assert_eq!(
        response.results.get("out").expect("out").result(),
        Some("2000 gram")
    );
    let explanation = out_explanation(&response);
    assert_eq!(explanation.body, "mass as gram");
    let json = explanation_json(explanation);
    assert_eq!(conversion_expressions(&json), vec!["mass as gram"]);
}

#[test]
fn sum_reorder_still_explains_source_operand_identity() {
    let engine = load(
        r#"
spec sum_order
rule out: 2 + 1
"#,
    );
    let response = run(
        &engine,
        "sum_order",
        IndexMap::new(),
        Some(&["out".to_string()]),
        true,
    );
    assert_eq!(
        response.results.get("out").expect("out").result(),
        Some("3")
    );
    let explanation = out_explanation(&response);
    assert_eq!(explanation.body, "1 + 2");
    let json = explanation_json(explanation);
    assert_eq!(
        json["body"], "1 + 2",
        "sum fold must narrate ordered sum pre-image"
    );
}

// ── Suite 4: cross-cutting ───────────────────────────────────────────────

#[test]
fn static_unless_explain_false_value_parity() {
    let engine = load(
        r#"
spec parity_static
rule out: true unless 5 < 3 then false
"#,
    );
    let rules = ["out".to_string()];
    let without = run(
        &engine,
        "parity_static",
        IndexMap::new(),
        Some(&rules),
        false,
    );
    let with = run(
        &engine,
        "parity_static",
        IndexMap::new(),
        Some(&rules),
        true,
    );
    let left = without.results.get("out").expect("out");
    let right = with.results.get("out").expect("out");
    assert_eq!(left.vetoed, right.vetoed);
    assert_eq!(left.result(), right.result());
    assert!(left.explanation.is_none());
    assert!(right.explanation.is_some());
}

#[test]
fn explain_lower_unless_condition_veto_does_not_change_value() {
    // Winner is the last unless (true → 2). Exhaustive explain still evaluates
    // the lower condition `1/x > 0` (div-by-zero Veto) and must discard it.
    let engine = load(
        r#"
spec lower_veto
data x: number
rule out: 1
  unless 1 / x > 0 then 99
  unless true then 2
"#,
    );
    let rules = ["out".to_string()];
    let data = IndexMap::from([("x".into(), "0".into())]);
    let without = run(&engine, "lower_veto", data.clone(), Some(&rules), false);
    let with = run(&engine, "lower_veto", data, Some(&rules), true);
    let left = without.results.get("out").expect("out");
    let right = with.results.get("out").expect("out");
    assert!(
        !left.vetoed,
        "plain run must keep winner value, not lower veto"
    );
    assert_eq!(left.result(), Some("2"));
    assert_eq!(left.vetoed, right.vetoed);
    assert_eq!(left.result(), right.result());
    assert!(
        right.explanation.is_some(),
        "explain must narrate without panic"
    );
}

#[test]
fn chained_static_unless_on_rule_ref() {
    let engine = load(
        r#"
spec chained_ref
rule base: 1 unless false then 9
rule out: base
"#,
    );
    let response = run(
        &engine,
        "chained_ref",
        IndexMap::new(),
        Some(&["out".to_string()]),
        true,
    );
    assert_eq!(
        response.results.get("out").expect("out").result(),
        Some("1")
    );
    let explanation = out_explanation(&response);
    let json = explanation_json(explanation);
    assert_eq!(rule_names(&json), vec!["out", "base"]);
    let base_ref = json["children"]
        .as_array()
        .expect("children")
        .iter()
        .find(|n| n.get("name").and_then(|r| r.as_str()) == Some("base"))
        .expect("base rule_ref");
    assert_eq!(base_ref["type"], "rule_ref");
    let base = explanation_json::json_resolve_ref(&json, base_ref);
    assert_eq!(base["causes"][0]["condition"], "false");
    assert_eq!(base["causes"][0]["value"], "false");
}

#[test]
fn runtime_falsified_unless_still_flips() {
    let engine = load(
        r#"
spec runtime_flip
data n: 5
rule out: true unless n < 3 then false
"#,
    );
    let response = run(
        &engine,
        "runtime_flip",
        IndexMap::new(),
        Some(&["out".to_string()]),
        true,
    );
    assert_eq!(
        response.results.get("out").expect("out").result(),
        Some("true")
    );
    let explanation = out_explanation(&response);
    assert_eq!(cause_pairs(explanation), vec![("n >= 3", "true")]);
}

#[test]
fn last_match_wins_static_true_over_earlier_true() {
    let engine = load(
        r#"
spec two_true
rule out: 0 unless true then 1 unless true then 2
"#,
    );
    let response = run(
        &engine,
        "two_true",
        IndexMap::new(),
        Some(&["out".to_string()]),
        true,
    );
    assert_eq!(
        response.results.get("out").expect("out").result(),
        Some("2")
    );
    let explanation = out_explanation(&response);
    assert_eq!(explanation.body, "2");
    assert_eq!(cause_pairs(explanation), vec![("true", "true")]);
}

fn cause_child_names(cause: &lemma::Cause) -> Vec<String> {
    cause
        .children
        .iter()
        .map(|node| match node {
            ExplanationNode::Data { name, .. } | ExplanationNode::DataUnused { name } => {
                name.input_key()
            }
            ExplanationNode::RuleRef { name, .. } => name.input_key(),
            other => panic!("unexpected cause child: {other:?}"),
        })
        .collect()
}

#[test]
fn held_and_cause_stops_at_rule_ref_not_dependency_data() {
    let engine = load(
        r#"
spec premium_membership
uses lemma units
data start: date
data length: units.calendar
rule valid: now in start...start + length

spec film_access
uses premium_membership
data type: text
  -> option "rental"
  -> option "purchase"
data views_consumed: number
data premium_member: boolean
rule max_views: 3
  unless premium_membership.valid then 10
  unless premium_member then 5
rule can_view: no
  unless type is "rental" and views_consumed < max_views then yes
  unless type is "purchase" then yes
"#,
    );
    let mut data = IndexMap::new();
    data.insert("type".into(), "rental".into());
    data.insert("views_consumed".into(), "3".into());
    data.insert("premium_member".into(), "true".into());
    let response = run(
        &engine,
        "film_access",
        data,
        Some(&["can_view".to_string()]),
        true,
    );
    assert_eq!(
        response.results.get("can_view").expect("can_view").result(),
        Some("true")
    );
    let explanation = response
        .results
        .get("can_view")
        .expect("can_view")
        .explanation
        .as_ref()
        .expect("explanation");
    let winning = explanation
        .causes
        .iter()
        .find(|c| c.condition.contains("max_views") && c.value == "true")
        .expect("held rental and cause");
    assert_eq!(
        cause_child_names(winning),
        vec!["views_consumed".to_string(), "max_views".to_string()],
        "held and lists local value + rule_ref cut, not max_views deps or literal is data"
    );
    assert!(
        !cause_child_names(winning).iter().any(|n| n == "type"),
        "type is \"rental\" already states the binding"
    );
    assert!(
        !cause_child_names(winning)
            .iter()
            .any(|n| n == "premium_member"),
        "premium_member belongs under max_views, not can_view's condition"
    );
    let max_views_ref = winning
        .children
        .iter()
        .find(|n| matches!(n, ExplanationNode::RuleRef { name, .. } if name.rule == "max_views"))
        .expect("max_views RuleRef");
    let max_views = match max_views_ref {
        ExplanationNode::RuleRef { id, .. } => explanation
            .rules
            .iter()
            .find(|r| r.id == *id)
            .expect("max_views in rules table"),
        _ => unreachable!(),
    };
    assert_eq!(max_views.result.result.as_deref(), Some("5"));
    assert_eq!(
        lemma::format_explanation(explanation),
        "\
can_view: true
├─ type is not purchase
│  └─ type: rental
└─ type is rental and views_consumed < max_views
   ├─ views_consumed: 3
   └─ max_views: 5
      └─ premium_member"
    );
}

#[test]
fn held_literal_is_cause_omits_restated_data() {
    let engine = load(
        r#"
spec rental_only
data type: text
rule out: no
  unless type is "rental" then yes
"#,
    );
    let mut data = IndexMap::new();
    data.insert("type".into(), "rental".into());
    let response = run(
        &engine,
        "rental_only",
        data,
        Some(&["out".to_string()]),
        true,
    );
    assert_eq!(
        response.results.get("out").expect("out").result(),
        Some("true")
    );
    let explanation = out_explanation(&response);
    let winning = explanation
        .causes
        .iter()
        .find(|c| c.condition.contains("type") && c.value == "true")
        .expect("held type is rental cause");
    assert!(
        winning.children.is_empty(),
        "held data-vs-literal is fully stated in the cause line, got {:?}",
        winning.children
    );
}

#[test]
fn not_taken_literal_is_keeps_actual_data_value() {
    let engine = load(
        r#"
spec film_access
data type: text
  -> option "rental"
  -> option "purchase"
data views_consumed: number
data max_views: number
rule can_view: no
  unless type is "rental" and views_consumed < max_views then yes
  unless type is "purchase" then yes
"#,
    );
    let mut data = IndexMap::new();
    data.insert("type".into(), "rental".into());
    data.insert("views_consumed".into(), "6".into());
    data.insert("max_views".into(), "5".into());
    let response = run(
        &engine,
        "film_access",
        data,
        Some(&["can_view".to_string()]),
        true,
    );
    assert_eq!(
        response.results.get("can_view").expect("can_view").result(),
        Some("false")
    );
    let explanation = response
        .results
        .get("can_view")
        .expect("can_view")
        .explanation
        .as_ref()
        .expect("explanation");
    let not_purchase = explanation
        .causes
        .iter()
        .find(|c| c.condition.contains("is not") && c.value == "true")
        .expect("flipped type is not purchase cause");
    assert_eq!(
        cause_child_names(not_purchase),
        vec!["type".to_string()],
        "is not keeps the actual binding; cause line only names the rejected literal"
    );
    match not_purchase.children.as_slice() {
        [ExplanationNode::Data { name, result }] => {
            assert_eq!(name.input_key(), "type");
            assert_eq!(result.result.as_deref(), Some("rental"));
        }
        other => panic!("expected type: rental data child, got {other:?}"),
    }
}

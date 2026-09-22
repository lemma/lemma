//! Parity tests: the explanation must agree with the authoritative tree-walk
//! result on non-happy paths, and must keep narrating source parts the
//! evaluator logically skipped (e.g. short-circuit).

use lemma::{format_explanation, DateTimeValue, Engine};
use std::collections::HashMap;

fn load(code: &str) -> Engine {
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("spec must load");
    engine
}

fn run_explain(engine: &Engine, spec: &str, inputs: &[(&str, &str)]) -> lemma::Response {
    let now = DateTimeValue::now();
    let data: HashMap<String, String> = inputs
        .iter()
        .map(|(name, value)| (name.to_string(), (*value).to_string()))
        .collect();
    engine
        .run(None, spec, Some(&now), data, None, true)
        .expect("evaluation must succeed")
}

fn rule_result<'response>(
    response: &'response lemma::Response,
    rule_name: &str,
) -> &'response lemma::RuleResult {
    response
        .results
        .values()
        .find(|result| result.rule.name == rule_name)
        .unwrap_or_else(|| panic!("rule '{rule_name}' must be in the response"))
}

#[test]
fn vetoed_unless_condition_explains_the_veto_not_a_branch() {
    let engine = load(
        r#"
spec condition_veto
data input: number
rule guard: input
  unless input > 1000 then veto "too large"
rule r: 1
  unless guard > 50 then 2
"#,
    );

    let response = run_explain(&engine, "condition_veto", &[("input", "2000")]);
    let result = rule_result(&response, "r");
    assert!(
        result.vetoed,
        "the vetoed condition must propagate as the rule result"
    );
    assert_eq!(result.veto_reason.as_deref(), Some("too large"));

    let explanation = result.explanation.as_ref().expect("explanation built");
    assert!(
        explanation.result.vetoed(),
        "explanation.result must agree with the response result"
    );
    assert!(
        explanation.body.contains("guard"),
        "the body must name the vetoing condition, not a branch result: {}",
        explanation.body
    );
    assert_ne!(
        explanation.body, "2",
        "no branch body was selected; the explanation must not claim one"
    );
    let rendered = format_explanation(explanation);
    assert!(
        rendered.contains("too large"),
        "the rendered explanation must surface the veto reason:\n{rendered}"
    );
}

#[test]
fn missing_data_unless_condition_explains_the_missing_data() {
    let engine = load(
        r#"
spec condition_missing
data input: number
rule guard: input
  unless input > 1000 then veto "too large"
rule r: 1
  unless guard > 50 then 2
"#,
    );

    let response = run_explain(&engine, "condition_missing", &[]);
    let result = rule_result(&response, "r");
    assert!(
        result.vetoed,
        "missing data in the condition must propagate as the rule result"
    );

    let explanation = result.explanation.as_ref().expect("explanation built");
    assert!(
        explanation.result.vetoed(),
        "explanation.result must agree with the response result"
    );
    let rendered = format_explanation(explanation);
    assert!(
        rendered.contains("input"),
        "the rendered explanation must point at the missing data:\n{rendered}"
    );
}

#[test]
fn decimal_limit_veto_is_visible_to_the_explanation() {
    let engine = load(
        r#"
spec decimal_limit
rule huge: 10 ^ 100
"#,
    );

    let response = run_explain(&engine, "decimal_limit", &[]);
    let result = rule_result(&response, "huge");
    assert!(
        result.vetoed,
        "a result exceeding the decimal limit must veto"
    );
    assert_eq!(
        result.veto_reason.as_deref(),
        Some("Calculated result exceeds decimal value limit")
    );

    let explanation = result.explanation.as_ref().expect("explanation built");
    assert!(
        !explanation.result.vetoed(),
        "explanation.result reflects exact rule_results storage; RuleResultValue veto is separate"
    );
    assert_eq!(
        serde_json::to_value(explanation).unwrap()["result"].as_str(),
        Some("Calculated result exceeds decimal value limit"),
        "explanation result must match the RuleResult veto reason"
    );
}

#[test]
fn explanation_narrates_operands_the_evaluator_skipped() {
    let engine = load(
        r#"
spec vision
data flag: boolean
data expensive_check: boolean
rule r: flag and expensive_check
"#,
    );

    // `flag` is false, so conjunction short-circuits and the value walk never
    // reads `expensive_check`. The explanation still narrates the skipped
    // operand with its value.
    let response = run_explain(
        &engine,
        "vision",
        &[("flag", "no"), ("expensive_check", "yes")],
    );
    let result = rule_result(&response, "r");
    assert!(!result.vetoed);
    assert_eq!(result.result(), Some("false"));

    let explanation = result.explanation.as_ref().expect("explanation built");
    let rendered = format_explanation(explanation);
    assert!(
        rendered.contains("expensive_check"),
        "the explanation must narrate the source operand the value walk \
         short-circuited past:\n{rendered}"
    );
}

#[test]
fn comparison_explanation_includes_operand_values() {
    let engine = load(
        r#"
spec comparison_operands
data a: number
data b: number
rule active: a >= b
"#,
    );

    let response = run_explain(&engine, "comparison_operands", &[("a", "7"), ("b", "3")]);
    let result = rule_result(&response, "active");
    assert!(!result.vetoed);
    assert_eq!(result.result(), Some("true"));

    let explanation = result.explanation.as_ref().expect("explanation built");
    let rendered = format_explanation(explanation);
    assert!(
        rendered.contains("a: 7") && rendered.contains("b: 3"),
        "the explanation must show the operand values that drove the comparison:\n{rendered}"
    );
}

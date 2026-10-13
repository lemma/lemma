//! Shared-rule explanation graph: linear size, stack-safe format/serialize.

use indexmap::IndexMap;
use lemma::{
    format_explanation, DateTimeValue, Engine, EvaluateDataValue, ExplainedRuleId, SourceType,
};
use std::fmt::Write;

fn now() -> DateTimeValue {
    DateTimeValue::now()
}

fn load(code: &str) -> Engine {
    let mut engine = Engine::new();
    engine
        .load([(SourceType::Volatile, code.to_string())])
        .unwrap_or_else(|errs| panic!("load failed: {errs:?}"));
    engine
}

fn evaluate_explain(engine: &Engine, spec: &str, tip: &str) -> lemma::Response {
    let mut data = IndexMap::new();
    data.insert("x".to_string(), EvaluateDataValue::Literal("1".into()));
    engine
        .evaluate(
            None,
            spec,
            Some(&now()),
            data,
            Some(&[tip.to_string()]),
            true,
        )
        .unwrap_or_else(|e| panic!("run failed: {e:?}"))
}

#[test]
fn diamond_chain_explanation_is_linear() {
    let mut code = String::from("spec diamond\ndata x: number\nrule r1: x\n");
    for i in 2..=40 {
        writeln!(code, "rule r{i}: r{} + r{}", i - 1, i - 1).unwrap();
    }
    let engine = load(&code);
    let response = evaluate_explain(&engine, "diamond", "r40");
    let explanation = response
        .results
        .get("r40")
        .expect("r40")
        .explanation
        .as_ref()
        .expect("explanation");
    assert_eq!(
        explanation.rules.len(),
        39,
        "r1..r39 once each in the rules table (r40 is the root)"
    );
    let text = format_explanation(explanation);
    let lines = text.lines().count();
    assert!(
        lines < 200,
        "ASCII must stay linear in 40 rules, got {lines} lines:\n{text}"
    );
    assert!(
        text.contains("(see above)"),
        "diamond reuse must print see above:\n{text}"
    );
}

#[test]
fn see_above_on_repeated_rule_ref() {
    let engine = load(
        r#"
spec s
data x: number
rule a: x + 1
rule b: a + a
"#,
    );
    let response = evaluate_explain(&engine, "s", "b");
    let explanation = response
        .results
        .get("b")
        .expect("b")
        .explanation
        .as_ref()
        .expect("explanation");
    assert_eq!(explanation.rules.len(), 1);
    assert_eq!(explanation.rules[0].id, ExplainedRuleId(0));
    let text = format_explanation(explanation);
    assert_eq!(
        text.matches("a:").count(),
        2,
        "a appears twice (expand + see above):\n{text}"
    );
    assert!(
        text.contains("(see above)"),
        "second use must say see above:\n{text}"
    );
}

#[test]
fn self_list_double_read_is_linear_not_exponential() {
    const DEPTH: usize = 12;
    let engine = load(
        r#"
spec part
data quantity: number
uses components: part list
rule unit_cost:
  1 + (components -> sum unit_cost) + (components -> max unit_cost)
"#,
    );
    fn nest(depth: usize) -> EvaluateDataValue {
        if depth == 0 {
            return EvaluateDataValue::Map(
                [(
                    "quantity".to_string(),
                    EvaluateDataValue::Literal("1".into()),
                )]
                .into_iter()
                .collect(),
            );
        }
        EvaluateDataValue::Map(
            [
                (
                    "quantity".to_string(),
                    EvaluateDataValue::Literal("1".into()),
                ),
                (
                    "components".to_string(),
                    EvaluateDataValue::List(vec![nest(depth - 1)]),
                ),
            ]
            .into_iter()
            .collect(),
        )
    }
    let mut data = IndexMap::new();
    data.insert(
        "quantity".to_string(),
        EvaluateDataValue::Literal("1".into()),
    );
    data.insert(
        "components".to_string(),
        EvaluateDataValue::List(vec![nest(DEPTH - 1)]),
    );
    let response = engine
        .evaluate(
            None,
            "part",
            Some(&now()),
            data,
            Some(&["unit_cost".to_string()]),
            true,
        )
        .unwrap_or_else(|e| panic!("run failed: {e:?}"));
    let explanation = response
        .results
        .get("unit_cost")
        .expect("unit_cost")
        .explanation
        .as_ref()
        .expect("explanation");
    assert_eq!(
        explanation.rules.len(),
        DEPTH,
        "one rules entry per row unit_cost (depth {DEPTH}), got {}",
        explanation.rules.len()
    );
    let text = format_explanation(explanation);
    assert!(
        text.contains("(see above)"),
        "sum+max of same child must reuse:\n{text}"
    );
    let json = serde_json::to_string(explanation).expect("serialize");
    assert!(
        json.len() < 200_000,
        "JSON must stay linear, len={}",
        json.len()
    );
}

#[test]
fn ten_thousand_rule_chain_explains_on_small_stack() {
    const N: usize = 10_000;
    let mut code = String::with_capacity(N * 24);
    write!(code, "spec deep\ndata x: number\nrule r1: x + 1\n").unwrap();
    for i in 2..=N {
        writeln!(code, "rule r{i}: r{} + 1", i - 1).unwrap();
    }
    let engine = load(&code);
    let tip = format!("r{N}");
    let handle = std::thread::Builder::new()
        .stack_size(1024 * 1024)
        .spawn(move || {
            let response = evaluate_explain(&engine, "deep", &tip);
            let explanation = response
                .results
                .get(&tip)
                .expect("tip")
                .explanation
                .as_ref()
                .expect("explanation")
                .clone();
            let text = format_explanation(&explanation);
            let json = serde_json::to_string(&explanation).expect("json");
            (text.len(), json.len(), explanation.rules.len())
        })
        .expect("spawn");
    let (text_len, json_len, rules_len) = handle.join().expect("join");
    assert_eq!(rules_len, N - 1);
    assert!(text_len > 0 && json_len > 0);
}

#[test]
fn explanation_json_is_deterministic() {
    let engine = load(
        r#"
spec s
data x: number
rule a: x
rule b: a + a
"#,
    );
    let mut first = None;
    for _ in 0..20 {
        let response = evaluate_explain(&engine, "s", "b");
        let explanation = response
            .results
            .get("b")
            .expect("b")
            .explanation
            .as_ref()
            .expect("explanation");
        let json = serde_json::to_string(explanation).expect("json");
        match &first {
            None => first = Some(json),
            Some(prev) => assert_eq!(&json, prev, "explanation JSON must be byte-identical"),
        }
    }
}

#[test]
fn children_are_rule_refs_not_nested_rules() {
    let engine = load(
        r#"
spec s
data x: number
rule a: x
rule b: a + 1
"#,
    );
    let response = evaluate_explain(&engine, "s", "b");
    let explanation = response
        .results
        .get("b")
        .expect("b")
        .explanation
        .as_ref()
        .expect("explanation");
    let json = serde_json::to_value(explanation).expect("json");
    assert_eq!(json["type"], "rule");
    assert!(json["rules"].is_array());
    let children = json["children"].as_array().expect("children");
    let has_rule_ref = children.iter().any(|child| {
        child["type"] == "rule_ref"
            || child
                .get("operands")
                .and_then(|o| o.as_array())
                .is_some_and(|ops| ops.iter().any(|op| op["type"] == "rule_ref"))
    });
    assert!(has_rule_ref, "expected rule_ref in children: {json}");
    let nested = serde_json::to_string(&json["children"]).unwrap()
        + &serde_json::to_string(&json["rules"]).unwrap();
    assert!(
        !nested.contains("\"type\":\"rule\""),
        "nested type:rule must not appear under children/rules: {json}"
    );
}

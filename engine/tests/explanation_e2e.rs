use lemma::{format_explanation, DateTimeValue, Engine, LiteralValue};
use std::collections::HashMap;

#[test]
fn explanation_generated_during_evaluation() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Volatile,
            r#"
spec test_explanation
data base_value: 100
rule doubled: base_value * 2
"#
            .to_string(),
        )])
        .unwrap();
    let now = DateTimeValue::now();
    let response = engine
        .run(
            None,
            "test_explanation",
            Some(&now),
            HashMap::new(),
            None,
            true,
        )
        .unwrap();

    let doubled = response
        .results
        .values()
        .find(|r| r.rule.name == "doubled")
        .expect("doubled rule");

    assert_eq!(
        doubled.display().expect("display").to_string(),
        LiteralValue::number_from_decimal(rust_decimal::Decimal::from(200)).to_string(),
    );

    let explanation = doubled.explanation.as_ref().expect("explanation built");
    assert_eq!(explanation.name.rule, "doubled");
    assert!(format_explanation(explanation).contains("base_value"));
}

#[test]
fn explanation_with_rule_reference() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Volatile,
            r#"
spec test_explanation_ref
data base_value: 50
rule doubled: base_value * 2
rule quadruple: doubled * 2
"#
            .to_string(),
        )])
        .unwrap();
    let now = DateTimeValue::now();
    let response = engine
        .run(
            None,
            "test_explanation_ref",
            Some(&now),
            HashMap::new(),
            None,
            true,
        )
        .unwrap();

    let quadruple = response
        .results
        .values()
        .find(|r| r.rule.name == "quadruple")
        .expect("quadruple rule");

    let explanation = quadruple.explanation.as_ref().expect("explanation");
    let text = format_explanation(explanation);
    assert!(text.contains("doubled * 2"));

    let json: serde_json::Value = serde_json::to_value(explanation).expect("serialize");
    // Literal `2` stays in JSON for parsers; ASCII omits the reprint.
    assert_eq!(json["children"].as_array().expect("children").len(), 2);
    let child_type = json["children"][0]["type"]
        .as_str()
        .expect("embedded rule child type");
    assert_eq!(child_type, "rule");
    assert_eq!(json["children"][0]["name"], "doubled");
    assert_eq!(json["children"][1]["type"], "compose");
    assert_eq!(json["children"][1]["expression"], "2");
    assert!(json["children"][1]["operands"]
        .as_array()
        .expect("operands")
        .is_empty());
    assert!(!text.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed == "├─ 2" || trimmed == "└─ 2"
    }));
}

#[test]
fn explanation_unless_branch_causes() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Volatile,
            r#"
spec test_unless
data flag: false
rule out: 1 unless flag then 2
"#
            .to_string(),
        )])
        .unwrap();
    let now = DateTimeValue::now();
    let response = engine
        .run(None, "test_unless", Some(&now), HashMap::new(), None, true)
        .unwrap();

    let explanation = response
        .results
        .get("out")
        .expect("out rule")
        .explanation
        .as_ref()
        .expect("explanation");
    assert_eq!(explanation.causes.len(), 1);
    // The condition `flag` was false; the cause states the fact that held.
    assert_eq!(explanation.causes[0].condition, "flag is false");
    assert_eq!(explanation.causes[0].value, "true");
}

#[test]
fn explanation_user_veto() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Volatile,
            r#"
spec test_veto
data price: -5
rule validated: price unless price < 0 then veto "negative"
"#
            .to_string(),
        )])
        .unwrap();
    let now = DateTimeValue::now();
    let response = engine
        .run(None, "test_veto", Some(&now), HashMap::new(), None, true)
        .unwrap();

    let result = response.results.get("validated").expect("validated");
    assert!(result.vetoed);
    let explanation = result.explanation.as_ref().expect("explanation");
    assert!(format_explanation(explanation).contains("veto"));
}

#[test]
fn explanation_cross_spec_rule() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Volatile,
            r#"
spec helper
rule helper_value: 10

spec main
uses helper
rule use_cross_spec: helper.helper_value + 1
"#
            .to_string(),
        )])
        .unwrap();
    let now = DateTimeValue::now();
    let response = engine
        .run(None, "main", Some(&now), HashMap::new(), None, true)
        .unwrap();

    let explanation = response
        .results
        .get("use_cross_spec")
        .expect("rule")
        .explanation
        .as_ref()
        .expect("explanation");
    assert!(format_explanation(explanation).contains("helper"));
    assert!(format_explanation(explanation).contains("helper_value"));
}

#[test]
fn explanation_unit_conversion_steps() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Volatile,
            r#"
spec t
data weight: measure -> unit kg: 1 -> unit gram: 0.001
data w: 2 kg
rule in_grams: w as gram
"#
            .to_string(),
        )])
        .unwrap();
    let response = engine
        .run(None, "t", None, HashMap::new(), None, true)
        .unwrap();
    let explanation = response
        .results
        .get("in_grams")
        .expect("rule")
        .explanation
        .as_ref()
        .expect("explanation");

    let json = serde_json::to_value(explanation).unwrap();
    let steps = &json["children"][0]["steps"];
    assert!(steps
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["role"] == "outcome"));
    assert!(steps
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["role"] == "source"));
}

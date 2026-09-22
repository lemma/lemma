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
        doubled.result().expect("result").to_string(),
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
    let children = json["children"].as_array().expect("children");
    assert_eq!(children.len(), 1);
    assert_eq!(children[0]["type"], "compose");
    assert_eq!(children[0]["operator"], "multiply");
    let operands = children[0]["operands"].as_array().expect("operands");
    assert_eq!(operands.len(), 2);
    let child_type = operands[0]["type"]
        .as_str()
        .expect("embedded rule child type");
    assert_eq!(child_type, "rule");
    assert_eq!(operands[0]["name"], "doubled");
    assert_eq!(operands[1]["type"], "compose");
    assert_eq!(operands[1]["expression"], "2");
    assert!(operands[1]["operands"]
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
    let ascii = format_explanation(explanation);
    assert!(
        ascii.contains("helper.helper_value"),
        "ASCII must use input_key:\n{ascii}"
    );
    assert!(
        !ascii.contains('→'),
        "ASCII must not print PathSegment hops:\n{ascii}"
    );
    let json = serde_json::to_value(explanation).expect("serialize");
    let child_names: Vec<_> = json["children"]
        .as_array()
        .expect("children")
        .iter()
        .flat_map(|child| {
            if child["type"] == "rule" {
                vec![child["name"].as_str().expect("name").to_string()]
            } else if child["type"] == "compose" {
                child["operands"]
                    .as_array()
                    .expect("operands")
                    .iter()
                    .filter_map(|op| {
                        (op["type"] == "rule")
                            .then(|| op["name"].as_str().map(str::to_string))
                            .flatten()
                    })
                    .collect()
            } else {
                Vec::new()
            }
        })
        .collect();
    assert!(
        child_names.iter().any(|n| n == "helper.helper_value"),
        "JSON Rule name must be input_key, got {child_names:?}"
    );
    assert!(
        ascii.contains("helper.helper_value + 1")
            || explanation.body.contains("helper.helper_value"),
        "compose body must use input_key, body={:?}, ascii=\n{ascii}",
        explanation.body
    );
}

#[test]
fn explanation_explicit_alias_data_leaf_is_input_key() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Volatile,
            r#"
spec nut_bag
data weight: measure
  -> unit kg: 1

spec calc
uses bag: nut_bag
rule total: bag.weight
"#
            .to_string(),
        )])
        .unwrap();
    let mut data = HashMap::new();
    data.insert("bag.weight".to_string(), "12 kg".into());
    let response = engine
        .run(None, "calc", None, data, None, true)
        .expect("run");
    let explanation = response
        .results
        .get("total")
        .expect("total")
        .explanation
        .as_ref()
        .expect("explanation");
    let ascii = format_explanation(explanation);
    assert!(
        ascii.contains("bag.weight"),
        "data leaf must be alias key:\n{ascii}"
    );
    assert!(
        !ascii.contains("nut_bag") && !ascii.contains('→'),
        "must not print resolved spec or hop:\n{ascii}"
    );
}

#[test]
fn explanation_two_aliases_of_same_spec_stay_distinct() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Volatile,
            r#"
spec src
data x: number

spec main
uses a: src
uses b: src
rule sum: a.x + b.x
"#
            .to_string(),
        )])
        .unwrap();
    let mut data = HashMap::new();
    data.insert("a.x".to_string(), "1".into());
    data.insert("b.x".to_string(), "2".into());
    let response = engine
        .run(None, "main", None, data, None, true)
        .expect("run");
    let explanation = response
        .results
        .get("sum")
        .expect("sum")
        .explanation
        .as_ref()
        .expect("explanation");
    let ascii = format_explanation(explanation);
    assert!(ascii.contains("a.x"), "missing a.x:\n{ascii}");
    assert!(ascii.contains("b.x"), "missing b.x:\n{ascii}");
    assert!(!ascii.contains('→'), "no hops:\n{ascii}");
}

#[test]
fn explanation_missing_nested_data_matches_missing_data_key() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Volatile,
            r#"
spec nut_bag
data weight: measure
  -> unit kg: 1

spec calc
uses bag: nut_bag
rule total: bag.weight
"#
            .to_string(),
        )])
        .unwrap();
    let response = engine
        .run(None, "calc", None, HashMap::new(), None, true)
        .expect("run");
    let result = response.results.get("total").expect("total");
    assert!(result.vetoed);
    let key = "bag.weight";
    assert_eq!(result.missing_data(), &[key.to_string()]);
    assert_eq!(
        result.veto_reason.as_deref(),
        Some("Missing data: bag.weight"),
        "veto_reason must match missing_data key"
    );
    let explanation = result.explanation.as_ref().expect("explanation");
    let ascii = format_explanation(explanation);
    assert!(
        ascii.contains("Missing data: bag.weight") || ascii.contains("bag.weight: Missing data"),
        "explanation must use input_key:\n{ascii}"
    );
    assert!(
        !ascii.contains('→'),
        "no hops in missing explanation:\n{ascii}"
    );
}

#[test]
fn explanation_conversion_source_uses_input_key() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Volatile,
            r#"
spec nut_bag
data weight: measure
  -> unit kg: 1
  -> unit gram: 0.001

spec calc
uses bag: nut_bag
rule in_grams: bag.weight as gram
"#
            .to_string(),
        )])
        .unwrap();
    let mut data = HashMap::new();
    data.insert("bag.weight".to_string(), "2 kg".into());
    let response = engine
        .run(None, "calc", None, data, None, true)
        .expect("run");
    let explanation = response
        .results
        .get("in_grams")
        .expect("in_grams")
        .explanation
        .as_ref()
        .expect("explanation");
    let json = serde_json::to_value(explanation).expect("serialize");
    let steps = json["children"][0]["steps"]
        .as_array()
        .expect("conversion steps");
    let source = steps
        .iter()
        .find(|s| s["role"] == "source")
        .expect("source step");
    let text = source["text"].as_str().expect("source text");
    assert!(
        text.contains("bag.weight"),
        "conversion source must name input_key, got: {text}"
    );
    assert!(
        !text.contains('→') && !text.contains("nut_bag"),
        "conversion source must not print hop/spec, got: {text}"
    );
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

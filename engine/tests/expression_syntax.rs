use lemma::DateTimeValue;
use lemma::Engine;
use std::collections::HashMap;

#[test]
fn parentheses_syntax_evaluates_correctly() {
    // Integration test: parentheses syntax is accepted by parser and behaves correctly in evaluation.
    let code = r#"
spec test
data x: true
data y: false
data num: 16
rule not_x: not(x)
rule sqrt_num: sqrt(num)
rule sin_zero: sin(0)
rule log_ten: log(10)
rule combined: not(x) and sqrt(16) is 4
rule with_spaces: not  (  x  )
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "test", Some(&now), HashMap::new(), None, false)
        .unwrap();

    let not_x_rule = response.results.get("not_x").unwrap();
    assert_eq!(
        not_x_rule
            .result
            .as_ref()
            .expect("rule result value")
            .boolean,
        Some(false)
    );

    let sqrt_rule = response.results.get("sqrt_num").unwrap();
    assert_eq!(sqrt_rule.result(), Some("4"));

    let sin_rule = response.results.get("sin_zero").unwrap();
    assert_eq!(sin_rule.result(), Some("0"));

    let combined_rule = response.results.get("combined").unwrap();
    assert_eq!(
        combined_rule
            .result
            .as_ref()
            .expect("rule result value")
            .boolean,
        Some(false)
    );
}

#[test]
fn accept_and_reject_are_valid_data_and_rule_names() {
    let code = r#"
spec test
data accept: true
rule reject: false
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "test", Some(&now), HashMap::new(), None, false)
        .unwrap();

    assert_eq!(
        response
            .results
            .get("reject")
            .unwrap()
            .result
            .as_ref()
            .expect("rule result value")
            .boolean,
        Some(false)
    );
}

#[test]
fn accept_and_reject_are_not_boolean_runtime_strings() {
    let code = r#"
spec test
data flag: boolean
rule r: flag
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    for invalid in ["accept", "reject"] {
        let response = engine
            .run(
                None,
                "test",
                Some(&now),
                HashMap::from([("flag".to_string(), invalid.to_string())]),
                None,
                false,
            )
            .expect("run must complete");
        assert!(
            response.results.get("r").expect("rule r").vetoed,
            "'{invalid}' must not parse as boolean"
        );
    }
}

#[test]
fn commentary_after_spec_is_accepted() {
    let code = r#"
spec exam 2026-01-01
"""
This spec determines graduation.
"""
data score: number
rule graduates: score > 5
"#;
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("commentary after spec must load");
    let now = DateTimeValue::now();
    let show = engine.show(None, "exam", Some(&now)).expect("show");
    assert_eq!(
        show.commentary.as_deref(),
        Some("This spec determines graduation.")
    );
}

#[test]
fn commentary_after_data_is_parse_error() {
    let code = r#"
spec exam
data score: number
"""
bad place
"""
rule graduates: score > 5
"#;
    let mut engine = Engine::new();
    let err = engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect_err("commentary after data must fail");
    let joined = err
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("; ");
    assert!(
        !joined.is_empty(),
        "expected parse error for misplaced commentary"
    );
}

#[test]
fn commentary_after_rule_is_parse_error() {
    let code = r#"
spec exam
data score: 1
rule graduates: score > 5
"""
bad place
"""
"#;
    let mut engine = Engine::new();
    let err = engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect_err("commentary after rule must fail");
    let joined = err
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("; ");
    assert!(
        !joined.is_empty(),
        "expected parse error for misplaced commentary"
    );
}

#[test]
fn keyword_declaration_order_does_not_change_eval() {
    let ordered = r#"
spec s
uses lemma units
meta title: "Order"
data n: 2
rule doubled: n * 2
"#;
    let reordered = r#"
spec s
rule doubled: n * 2
meta title: "Order"
data n: 2
uses lemma units
"#;
    let mut engine_a = Engine::new();
    let mut engine_b = Engine::new();
    engine_a
        .load([(lemma::SourceType::Volatile, ordered.to_string())])
        .expect("ordered");
    engine_b
        .load([(lemma::SourceType::Volatile, reordered.to_string())])
        .expect("reordered");
    let now = DateTimeValue::now();
    let a = engine_a
        .run(None, "s", Some(&now), HashMap::new(), None, false)
        .expect("run a");
    let b = engine_b
        .run(None, "s", Some(&now), HashMap::new(), None, false)
        .expect("run b");
    assert_eq!(a.results.get("doubled").and_then(|r| r.result()), Some("4"));
    assert_eq!(
        a.results
            .get("doubled")
            .and_then(|r| r.result().map(str::to_string)),
        b.results
            .get("doubled")
            .and_then(|r| r.result().map(str::to_string))
    );
    let show_a = engine_a.show(None, "s", Some(&now)).expect("show a");
    let show_b = engine_b.show(None, "s", Some(&now)).expect("show b");
    assert_eq!(show_a.meta.get("title"), show_b.meta.get("title"));
}

#[test]
fn insignificant_whitespace_one_line_vs_multiline_eval_equal() {
    let multiline = r#"
spec s
data n: 3
rule r: n * 2
"#;
    let oneline = "spec s data n: 3 rule r: n * 2";
    let mut engine_a = Engine::new();
    let mut engine_b = Engine::new();
    engine_a
        .load([(lemma::SourceType::Volatile, multiline.to_string())])
        .expect("multiline");
    engine_b
        .load([(lemma::SourceType::Volatile, oneline.to_string())])
        .expect("oneline");
    let now = DateTimeValue::now();
    let a = engine_a
        .run(None, "s", Some(&now), HashMap::new(), None, false)
        .expect("run a");
    let b = engine_b
        .run(None, "s", Some(&now), HashMap::new(), None, false)
        .expect("run b");
    assert_eq!(
        a.results
            .get("r")
            .and_then(|r| r.result().map(str::to_string)),
        b.results
            .get("r")
            .and_then(|r| r.result().map(str::to_string))
    );
    assert_eq!(a.results.get("r").and_then(|r| r.result()), Some("6"));
}

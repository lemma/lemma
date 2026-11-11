//! Mirrors fuzz target API usage so that compile-breaking changes
//! to the public API are caught by `cargo nextest run` before the
//! nightly-only fuzz job ever runs.

use indexmap::IndexMap;
use lemma::DateTimeValue;
use lemma::EvaluateDataValue;
use lemma::{Engine, SourceType};
use std::collections::HashMap;
use std::str::FromStr;

fn fixed_now() -> DateTimeValue {
    DateTimeValue::from_str("2024-06-15T12:00:00Z").expect("BUG: fixed fuzz datetime")
}

fn engine_with_files(files: HashMap<String, String>) -> Engine {
    let mut engine = Engine::new();
    for (attr, code) in files {
        let src = if attr.trim().is_empty() {
            SourceType::Volatile
        } else {
            SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(attr.as_str())))
        };
        if let Err(error) = engine.load([(src, &code.to_string())]) {
            std::hint::black_box(error);
        }
    }
    engine
}

fn single_file(name: &str, code: &str) -> HashMap<String, String> {
    std::iter::once((name.to_string(), code.into())).collect()
}

#[test]
fn fuzz_deeply_nested_completes_fast() {
    let start = std::time::Instant::now();
    let mut expr = String::from("1");
    for _ in 0..5 {
        expr = format!("({} + 1)", expr);
    }
    let code = format!(
        "spec fuzz_nested\ndata x: 1\nrule deeply_nested: {}\n",
        expr
    );
    engine_with_files(single_file("fuzz_nested", &code));
    let elapsed = start.elapsed();
    assert!(
        elapsed.as_millis() < 500,
        "5-deep nested parse took {}ms, expected <500ms (regression guard)",
        elapsed.as_millis()
    );
}

#[test]
fn fuzz_data_bindings_api_number_too_long_no_panic() {
    let code = "spec fuzz_test\ndata x: number\nrule doubled: x * 2\n";
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "fuzz_binding",
            ))),
            code.to_string(),
        )])
        .unwrap();
    // 30 nines exceeds Decimal::MAX (~7.92e28): unrepresentable input must
    // veto with a parse reason, not panic.
    let mut data = IndexMap::new();
    data.insert("x".to_string(), "999999999999999999999999999999".into());
    let now = fixed_now();
    let response = engine
        .evaluate(None, "fuzz_test", Some(&now), data, None, false)
        .expect("evaluate must complete with veto, not Error");
    let doubled = response.results.get("doubled").expect("doubled");
    assert!(
        doubled.vetoed,
        "expected veto for unrepresentable number, got {:?}",
        doubled.result()
    );
    let reason = doubled.veto_reason.as_deref().expect("veto reason");
    assert!(
        reason.contains("Invalid number"),
        "expected parse-failure veto, got: {reason}"
    );
}

#[test]
fn show_and_response_serialize_infallibly_across_fixture_variants() {
    let cases: &[(&str, &str)] = &[
        (
            "numbers",
            r#"
spec numbers
data n: number -> suggest 1
data t: text -> suggest "hi"
data b: boolean -> suggest true
data d: 2025-03-04
data tm: 12:30:45
rule rn: n
rule rt: t
rule rb: b
rule rd: d
rule rtm: tm
"#,
        ),
        (
            "measures",
            r#"
spec measures
uses lemma units
data money: measure
  -> unit eur: 1
  -> suggest 10 eur
data rate: ratio -> suggest 15%
data window: 1...10
rule rm: money
rule rr: rate
rule rw: window
"#,
        ),
        (
            "veto_and_range",
            r#"
spec veto_and_range
uses lemma units
data mass: measure
  -> unit kilogram: 1
data band: mass range
  -> suggest 1 kilogram...5 kilogram
rule outcome: veto "nope"
rule band_out: band
"#,
        ),
    ];

    for (name, code) in cases {
        let mut engine = Engine::new();
        engine
            .load([(
                SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(format!(
                    "{name}.lemma"
                )))),
                code.to_string(),
            )])
            .unwrap_or_else(|e| panic!("{name} must load: {e:?}"));
        let now = fixed_now();
        let show = engine
            .show(None, name, Some(&now))
            .unwrap_or_else(|e| panic!("{name} show: {e:?}"));
        serde_json::to_string(&lemma::api::show_document(&engine, &show))
            .unwrap_or_else(|e| panic!("{name} Show serialize: {e}"));

        let response = engine
            .evaluate(None, name, Some(&now), IndexMap::new(), None, true)
            .unwrap_or_else(|e| panic!("{name} run: {e:?}"));
        serde_json::to_string(&lemma::api::Response::from(&response))
            .unwrap_or_else(|e| panic!("{name} Response serialize: {e}"));
        let specs = engine.specs();
        serde_json::to_string(&specs).unwrap_or_else(|e| panic!("{name} specs serialize: {e}"));
    }
}

#[test]
fn data_binding_at_max_fractional_digits_evaluates() {
    let scale = rust_decimal::Decimal::MAX_SCALE as usize;
    let frac = "1".repeat(scale);
    let literal = format!("0.{frac}");
    let code = "spec fuzz_test\ndata x: number\nrule doubled: x * 2\n";
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "fuzz_binding",
            ))),
            code.to_string(),
        )])
        .unwrap();
    let mut data = IndexMap::new();
    data.insert("x".to_string(), literal.clone().into());
    let now = fixed_now();
    let response = engine
        .evaluate(None, "fuzz_test", Some(&now), data, None, false)
        .expect("evaluate must complete");
    let doubled = response.results.get("doubled").expect("doubled");
    assert!(
        !doubled.vetoed,
        "max-scale input must evaluate, got {:?}",
        doubled.veto_reason
    );
    let expected = rust_decimal::Decimal::from_str(&format!("0.{}", "2".repeat(scale)))
        .expect("expected doubled decimal");
    assert_eq!(
        doubled.result.as_ref().expect("rule result value").number,
        Some(expected),
        "doubled 0.(1×{scale}) must be 0.(2×{scale})"
    );
}

#[test]
fn fuzz_data_bindings_multi_key_overlay_shapes_no_panic() {
    let code = r#"
spec item
data name: text
data price: measure
  -> unit eur: 1
spec fuzz_test
uses item
data n: number
data b: boolean
data items: item list
  -> key name
  -> with name: "a", price: 1 eur
rule rn: n
rule rb: b
rule total: items -> sum price
"#;
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "fuzz_binding_multi",
            ))),
            code.to_string(),
        )])
        .expect("multi-type fuzz fixture must load");

    let entries = vec![
        ("n".to_string(), EvaluateDataValue::Literal("3".into())),
        ("b".to_string(), EvaluateDataValue::Boolean(true)),
        (
            "items.1.price".to_string(),
            EvaluateDataValue::Map(IndexMap::from([(
                "eur".to_string(),
                EvaluateDataValue::Literal("9".into()),
            )])),
        ),
        (
            "nosuch".to_string(),
            EvaluateDataValue::List(vec![EvaluateDataValue::Boolean(false)]),
        ),
    ];
    let data = lemma::evaluate_data_from_entries(entries).expect("distinct keys");
    let now = fixed_now();
    // Property mirrored from fuzz_data_bindings: overlays and odd keys must not panic.
    // Request Error and Veto are both acceptable outcomes.
    match engine.evaluate(None, "fuzz_test", Some(&now), data, None, false) {
        Ok(response) => {
            assert!(response.results.contains_key("rn"));
            std::hint::black_box(response);
        }
        Err(error) => {
            std::hint::black_box(error);
        }
    }

    // Shape mismatch against a scalar field is a request Error, never a panic.
    let mismatch = lemma::evaluate_data_from_entries(vec![(
        "n".to_string(),
        EvaluateDataValue::Boolean(true),
    )])
    .expect("single key");
    if let Err(error) = engine.evaluate(None, "fuzz_test", Some(&now), mismatch, None, false) {
        std::hint::black_box(error);
    }
}

#[test]
fn fuzz_parser_explain_formats_without_panic() {
    let code = r#"
spec fuzz_explain
data x: 2
rule doubled: x * 2
"#;
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "fuzz_explain",
            ))),
            code.to_string(),
        )])
        .expect("explain fixture must load");
    let now = fixed_now();
    let response = engine
        .evaluate(
            None,
            "fuzz_explain",
            Some(&now),
            IndexMap::new(),
            None,
            true,
        )
        .expect("explain evaluate must succeed");
    serde_json::to_string(&lemma::api::Response::from(&response))
        .expect("Response with explanations must serialize");
    let doubled = response.results.get("doubled").expect("doubled");
    let explanation = doubled
        .explanation
        .as_ref()
        .expect("explain:true must attach an explanation");
    let ascii = lemma::format_explanation(explanation);
    assert!(
        ascii.contains("doubled"),
        "formatted explanation must name the rule, got: {ascii}"
    );
    std::hint::black_box(format!("{explanation:?}"));
}

#[test]
fn fuzz_expressions_typed_operand_loads_and_evaluates() {
    let code = r#"
spec fuzz_test
uses lemma units
data x: 100
data y: 50
data flag: true
data label: "a"
data m: measure
  -> unit kilogram: 1
  -> unit gram: 0.001
  -> suggest 1 kilogram
rule test_expr: (x + y) * 2
"#;
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("fuzz_expr"))),
            code.to_string(),
        )])
        .expect("typed operand fixture must load");
    let now = fixed_now();
    let response = engine
        .evaluate(None, "fuzz_test", Some(&now), IndexMap::new(), None, false)
        .expect("typed operand evaluate must succeed");
    let result = response.results.get("test_expr").expect("test_expr");
    assert!(
        !result.vetoed,
        "typed operand fixture must not veto, got {:?}",
        result.veto_reason
    );
}

#[test]
fn data_binding_with_excess_fractional_digits_vetoes_at_input() {
    let scale = rust_decimal::Decimal::MAX_SCALE as usize;
    let frac = "1".repeat(scale + 1);
    let literal = format!("0.{frac}");
    let code = "spec fuzz_test\ndata x: number\nrule doubled: x * 2\n";
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "fuzz_binding",
            ))),
            code.to_string(),
        )])
        .unwrap();
    let mut data = IndexMap::new();
    data.insert("x".to_string(), literal.clone().into());
    let now = fixed_now();
    let response = engine
        .evaluate(None, "fuzz_test", Some(&now), data, None, false)
        .expect("evaluate must complete with veto, not Error");
    let doubled = response.results.get("doubled").expect("doubled");
    assert!(
        doubled.vetoed,
        "excess fractional digits must veto, got {:?}",
        doubled.result()
    );
    let reason = doubled.veto_reason.as_deref().expect("veto reason");
    assert!(
        reason.contains("Invalid number") && reason.contains("too many fractional digits"),
        "expected parse-failure veto for over-scale input, got: {reason}"
    );
}

#[test]
fn fuzz_promoted_adversarial_literal_soup_no_panic() {
    // Shape promoted from a historical fuzz_literals crash artifact: long
    // percent/calendar soup spliced as a text literal must not panic.
    let soup = "080-000000180-50000percent-percent-6day-60l0.-00/00000000000-50000180-50000181850percent-80calendar-00/00000000ercent-80";
    let code = format!(
        "spec fuzz_test\nuses lemma units\ndata test_value: \"{soup}\"\nrule echo: test_value\n"
    );
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "fuzz_literal_soup",
            ))),
            code,
        )])
        .expect("adversarial text literal must load");
    let now = fixed_now();
    let first = engine
        .evaluate(None, "fuzz_test", Some(&now), IndexMap::new(), None, true)
        .expect("evaluate must complete");
    let second = engine
        .evaluate(None, "fuzz_test", Some(&now), IndexMap::new(), None, true)
        .expect("second evaluate must complete");
    let json_a = serde_json::to_string(&lemma::api::Response::from(&first))
        .expect("Response must serialize");
    let json_b = serde_json::to_string(&lemma::api::Response::from(&second))
        .expect("Response must serialize");
    assert_eq!(json_a, json_b, "deterministic evaluate");
    let echo = first.results.get("echo").expect("echo");
    assert!(
        !echo.vetoed,
        "text literal soup must not veto, got {:?}",
        echo.veto_reason
    );
    assert!(
        echo.explanation.is_some(),
        "explain:true and non-vetoed rule must carry explanation"
    );
}

#[test]
fn fuzz_promoted_power_expression_no_panic() {
    // Theme promoted from a historical fuzz_expressions timeout artifact:
    // power (including a ratio exponent) must not hang or panic.
    let code = r#"
spec fuzz_test
data x: 787
rule powered: (x * 480) ^ 0
rule fractional: 2 ^ 50%
"#;
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("fuzz_power"))),
            code.to_string(),
        )])
        .expect("power fixture must load");
    let now = fixed_now();
    let start = std::time::Instant::now();
    let response = engine
        .evaluate(None, "fuzz_test", Some(&now), IndexMap::new(), None, false)
        .expect("power evaluate must complete");
    assert!(
        start.elapsed().as_millis() < 500,
        "power evaluate took {}ms, expected <500ms",
        start.elapsed().as_millis()
    );
    serde_json::to_string(&lemma::api::Response::from(&response)).expect("Response must serialize");
    assert!(response.results.contains_key("powered"));
    assert!(response.results.contains_key("fractional"));
}

#[test]
fn fuzz_lists_uses_pipelines_and_composition_no_panic() {
    let code = r#"
spec item
data name: text
data qty: number

spec base
data qty: number
  -> suggest 1

spec fuzz_test
uses item
uses base
data flag: true
data extra: 3
data wanted: item.name
  -> with "a"
data items: item list
  -> key name
  -> with name: "a", qty: 1
  -> with name: "b", qty: 2
  -> with name: "c", qty: 0
rule listed: items -> where qty > 0 -> sum qty
rule composed:
  base.qty
  unless flag then base.qty + extra
  unless items -> count > 0 then items -> count
"#;
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "fuzz_lists_uses",
            ))),
            code.to_string(),
        )])
        .expect("lists/uses fixture must load");
    let now = fixed_now();
    let response = engine
        .evaluate(None, "fuzz_test", Some(&now), IndexMap::new(), None, false)
        .expect("lists/uses evaluate must complete");
    let listed = response.results.get("listed").expect("listed");
    assert!(
        !listed.vetoed,
        "list pipeline must not veto, got {:?}",
        listed.veto_reason
    );
    let composed = response.results.get("composed").expect("composed");
    assert!(
        !composed.vetoed,
        "composed uses/unless must not veto, got {:?}",
        composed.veto_reason
    );
}

#[test]
fn fuzz_structured_literal_kinds_load() {
    let cases = [
        ("number", "42"),
        ("text", "\"hello\""),
        ("measure", "1 kilogram"),
        ("ratio", "50%"),
        ("date", "2024-01-01"),
        ("time", "12:30:45"),
        ("boolean", "true"),
        ("range", "1...10"),
    ];
    for (name, literal) in cases {
        let code = format!("spec fuzz_test\nuses lemma units\ndata test_value: {literal}\n");
        let mut engine = Engine::new();
        engine
            .load([(
                SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(format!(
                    "fuzz_literal_{name}"
                )))),
                code,
            )])
            .unwrap_or_else(|e| panic!("structured literal {name} must load: {e:?}"));
        let now = fixed_now();
        engine
            .evaluate(None, "fuzz_test", Some(&now), IndexMap::new(), None, false)
            .unwrap_or_else(|e| panic!("structured literal {name} evaluate: {e:?}"));
    }
}

#[test]
fn fuzz_expressions_unless_and_list_ops_load() {
    let code = r#"
spec item
data name: text
data qty: number

spec fuzz_test
uses lemma units
uses item
data x: 100
data y: 50
data flag: true
data items: item list
  -> key name
  -> with name: "a", qty: 1
  -> with name: "b", qty: 2
rule with_unless:
  x
  unless flag then y
  unless x > 0 then 0
rule with_list: items -> where qty > 0 -> count
"#;
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "fuzz_expr_grown",
            ))),
            code.to_string(),
        )])
        .expect("grown expression fixture must load");
    let now = fixed_now();
    let response = engine
        .evaluate(None, "fuzz_test", Some(&now), IndexMap::new(), None, false)
        .expect("grown expression evaluate must complete");
    assert!(
        !response
            .results
            .get("with_unless")
            .expect("with_unless")
            .vetoed
    );
    assert!(!response.results.get("with_list").expect("with_list").vetoed);
}

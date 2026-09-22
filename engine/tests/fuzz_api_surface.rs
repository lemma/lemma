//! Mirrors fuzz target API usage so that compile-breaking changes
//! to the public API are caught by `cargo nextest run` before the
//! nightly-only fuzz job ever runs.

use lemma::DateTimeValue;
use lemma::{Engine, SourceType};
use std::collections::HashMap;
use std::str::FromStr;

fn engine_with_files(files: HashMap<String, String>) -> Engine {
    let mut engine = Engine::new();
    for (attr, code) in files {
        let src = if attr.trim().is_empty() {
            SourceType::Volatile
        } else {
            SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(attr.as_str())))
        };
        let _ = engine.load([(src, &code.to_string())]);
    }
    engine
}

fn single_file(name: &str, code: &str) -> HashMap<String, String> {
    std::iter::once((name.to_string(), code.to_string())).collect()
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
    let mut data = HashMap::new();
    data.insert("x".to_string(), "999999999999999999999999999999".into());
    let now = DateTimeValue::now();
    let response = engine
        .run(None, "fuzz_test", Some(&now), data, None, false)
        .expect("run must complete with veto, not Error");
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
data band: measure range
  -> unit kilogram: 1
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
        let now = DateTimeValue::now();
        let show = engine
            .show(None, name, Some(&now))
            .unwrap_or_else(|e| panic!("{name} show: {e:?}"));
        serde_json::to_string(&lemma::api::Show::from(&show))
            .unwrap_or_else(|e| panic!("{name} Show serialize: {e}"));

        let response = engine
            .run(None, name, Some(&now), HashMap::new(), None, true)
            .unwrap_or_else(|e| panic!("{name} run: {e:?}"));
        serde_json::to_string(&lemma::api::Response::from(&response))
            .unwrap_or_else(|e| panic!("{name} Response serialize: {e}"));
        let list = engine.list();
        serde_json::to_string(&list).unwrap_or_else(|e| panic!("{name} list serialize: {e}"));
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
    let mut data = HashMap::new();
    data.insert("x".to_string(), literal.clone());
    let now = DateTimeValue::now();
    let response = engine
        .run(None, "fuzz_test", Some(&now), data, None, false)
        .expect("run must complete");
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
    let mut data = HashMap::new();
    data.insert("x".to_string(), literal.clone());
    let now = DateTimeValue::now();
    let response = engine
        .run(None, "fuzz_test", Some(&now), data, None, false)
        .expect("run must complete with veto, not Error");
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

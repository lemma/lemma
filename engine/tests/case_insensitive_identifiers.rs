//! Case-insensitive logical identifiers: parse canonicalization and API boundaries.

use lemma::format_source;
use lemma::DateTimeValue;
use lemma::Engine;
use std::collections::HashMap;
use std::sync::Arc;

fn load_ok(engine: &mut Engine, code: &str) {
    engine
        .load([(
            lemma::SourceType::Path(Arc::new(std::path::PathBuf::from("case_insensitive.lemma"))),
            code.to_string(),
        )])
        .unwrap_or_else(|errs| {
            panic!(
                "expected load to succeed, got: {}",
                errs.iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        });
}

fn load_err_joined(engine: &mut Engine, code: &str) -> String {
    let err = engine
        .load([(
            lemma::SourceType::Path(Arc::new(std::path::PathBuf::from("case_insensitive.lemma"))),
            code.to_string(),
        )])
        .expect_err("expected load to fail");
    err.iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

fn rule_value(result: &lemma::Response, name: &str) -> String {
    let rule_result = result
        .results
        .get(name)
        .unwrap_or_else(|| panic!("rule '{name}' not found"));
    if rule_result.vetoed {
        format!(
            "VETO({})",
            rule_result.veto_reason.as_deref().unwrap_or("Vetoed")
        )
    } else {
        rule_result.result().expect("result").to_string()
    }
}

#[test]
fn parse_lowercases_data_and_reference_names() {
    let code = r#"
spec s
data Price: number -> suggest 10
rule r: price
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let now = DateTimeValue::now();
    let mut data = HashMap::new();
    data.insert("price".to_string(), "10".into());
    let resp = engine
        .run(None, "s", Some(&now), data, None, false)
        .expect("evaluates");
    assert_eq!(rule_value(&resp, "r"), "10");
}

#[test]
fn measure_unit_literal_matches_declared_unit_case() {
    let code = r#"
spec s
data mass: measure -> unit gram: 1
rule r: 500 Gram
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let now = DateTimeValue::now();
    let resp = engine
        .run(None, "s", Some(&now), HashMap::new(), None, false)
        .expect("evaluates");
    assert!(rule_value(&resp, "r").contains("500"));
    assert!(rule_value(&resp, "r").contains("gram"));
}

#[test]
fn get_spec_accepts_mixed_case_api_name() {
    let code = r#"
spec myspec
data x: number -> suggest 7
rule r: x
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let now = DateTimeValue::now();
    let show = engine
        .show(None, "MySpec", Some(&now))
        .expect("spec found with mixed-case API name");
    assert_eq!(show.spec, "myspec");
}

#[test]
fn duplicate_data_names_differing_only_by_case_rejected() {
    let code = r#"
spec s
data age: number
data Age: number
"#;
    let mut engine = Engine::new();
    let msg = load_err_joined(&mut engine, code);
    assert!(
        msg.to_lowercase().contains("duplicate") || msg.to_lowercase().contains("already used"),
        "expected duplicate data error, got: {msg}"
    );
}

#[test]
fn format_source_lowercases_identifiers() {
    let source = r#"spec Test
data Price: number -> suggest 1
rule Total: price
"#;
    let formatted = format_source(source, lemma::SourceType::Volatile).expect("format succeeds");
    assert!(formatted.contains("spec test"));
    assert!(formatted.contains("data price"));
    assert!(formatted.contains("rule total"));
}

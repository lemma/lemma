//! `-> fill` is an unknown data command. Committed values use `-> with`.

use lemma::{Engine, SourceType};
use std::path::PathBuf;
use std::sync::Arc;

fn path_source(file: &str) -> SourceType {
    SourceType::Path(Arc::new(PathBuf::from(file)))
}

fn load_err_joined(code: &str) -> String {
    let mut engine = Engine::new();
    let err = engine
        .load([(path_source("test.lemma"), code.to_string())])
        .expect_err("expected load to fail");
    err.iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn fill_number_is_unknown_command() {
    let joined = load_err_joined(
        r#"spec test
data n: number -> fill 42
rule r: n"#,
    );
    assert!(
        joined.to_lowercase().contains("fill"),
        "expected unknown fill command, got: {joined}"
    );
}

#[test]
fn fill_measure_is_unknown_command() {
    let joined = load_err_joined(
        r#"spec test
uses lemma units
data age: calendar -> fill 25 year
rule r: age as year"#,
    );
    assert!(
        joined.to_lowercase().contains("fill"),
        "expected unknown fill command, got: {joined}"
    );
}

#[test]
fn fill_boolean_is_unknown_command() {
    let joined = load_err_joined(
        r#"spec test
data active: boolean -> fill true
rule r: active"#,
    );
    assert!(
        joined.to_lowercase().contains("fill"),
        "expected unknown fill command, got: {joined}"
    );
}

#[test]
fn fill_text_with_options_is_unknown_command() {
    let joined = load_err_joined(
        r#"spec test
data status: text
  -> option "a"
  -> option "b"
  -> fill "a"
rule r: status"#,
    );
    assert!(
        joined.to_lowercase().contains("fill"),
        "expected unknown fill command, got: {joined}"
    );
}

#[test]
fn fill_and_suggest_is_unknown_command() {
    let joined = load_err_joined(
        r#"spec test
data n: number -> fill 1 -> suggest 2
rule r: n"#,
    );
    assert!(
        joined.to_lowercase().contains("fill"),
        "expected unknown fill command, got: {joined}"
    );
}

#[test]
fn fill_ratio_is_unknown_command() {
    let joined = load_err_joined(
        r#"spec test
data rate: ratio -> fill 15%
rule r: rate"#,
    );
    assert!(
        joined.to_lowercase().contains("fill"),
        "expected unknown fill command, got: {joined}"
    );
}

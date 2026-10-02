//! Regression: with bindings with wrong literal shape must return planning errors, not panic.

use lemma::Engine;
use std::path::PathBuf;
use std::sync::Arc;

fn source() -> lemma::SourceType {
    lemma::SourceType::Path(Arc::new(PathBuf::from("data_bindings_type_mismatch.lemma")))
}

/// Loads `code`, requires exactly one error, and returns its message and source line.
fn load_single_error(engine: &mut Engine, code: &str) -> (String, usize) {
    let err = engine
        .load([(source(), code.to_string())])
        .expect_err("expected load to fail");
    let errors: Vec<_> = err.iter().collect();
    assert_eq!(errors.len(), 1, "{errors:?}");
    let location = errors[0].location().expect("binding error has a source");
    (errors[0].message().to_string(), location.span.line)
}

fn load_ok(engine: &mut Engine, code: &str) {
    engine
        .load([(source(), code.to_string())])
        .unwrap_or_else(|errs| {
            let joined = errs
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("\n");
            panic!("expected load to succeed, got: {joined}");
        });
}

const INNER_SPEC: &str = r#"spec product_structure
data primary_weight: measure
  -> unit kilogram: 1
  -> minimum 0 kilogram
"#;

#[test]
fn fill_bare_number_into_measure_slot_returns_planning_error() {
    let code = format!(
        r#"{INNER_SPEC}
spec almonds
uses product_structure
  -> with primary_weight: 10
"#
    );
    let mut engine = Engine::new();
    let (message, line) = load_single_error(&mut engine, &code);
    assert_eq!(message, "cannot use 10 as measure: expected `<n> kilogram`");
    assert_eq!(line, 8);
}

#[test]
fn fill_text_into_measure_slot_returns_planning_error() {
    let code = format!(
        r#"{INNER_SPEC}
spec almonds
uses product_structure
  -> with primary_weight: "hello"
"#
    );
    let mut engine = Engine::new();
    let (message, line) = load_single_error(&mut engine, &code);
    assert_eq!(
        message,
        "cannot use \"hello\" as measure: expected `<n> kilogram`"
    );
    assert_eq!(line, 8);
}

#[test]
fn fill_number_into_boolean_slot_returns_planning_error() {
    let code = r#"spec inner
data flag: boolean

spec outer
uses inner
  -> with flag: 10
"#;
    let mut engine = Engine::new();
    let (message, line) = load_single_error(&mut engine, code);
    assert_eq!(message, "cannot use 10 as boolean");
    assert_eq!(line, 6);
}

#[test]
fn fill_measure_with_unit_succeeds() {
    let code = format!(
        r#"{INNER_SPEC}
spec almonds
uses product_structure
  -> with primary_weight: 10 kilogram
"#
    );
    let mut engine = Engine::new();
    load_ok(&mut engine, &code);
}

#[test]
fn fill_bare_number_into_literal_measure_slot_returns_planning_error() {
    let code = r#"
spec inner
uses lemma units
data weight: 5 kilogram

spec outer
uses lemma units
uses inner
  -> with weight: 10
"#;
    let mut engine = Engine::new();
    let (message, line) = load_single_error(&mut engine, code);
    assert_eq!(message, "cannot use 10 as measure: expected `<n> kilogram`");
    assert_eq!(line, 9);
}

#[test]
fn fill_text_into_literal_number_slot_returns_planning_error() {
    let code = r#"
spec inner
data rate: 5

spec outer
uses inner
  -> with rate: "fast"
"#;
    let mut engine = Engine::new();
    let (message, line) = load_single_error(&mut engine, code);
    assert_eq!(message, "cannot use \"fast\" as number");
    assert_eq!(line, 7);
}

#[test]
fn fill_compatible_unit_into_literal_measure_slot_succeeds() {
    let code = r#"
spec inner
uses lemma units
data weight: 5 kilogram

spec outer
uses lemma units
uses inner
  -> with weight: 10 kilogram
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
}

#[test]
fn binding_type_errors_follow_with_source_order() {
    let code = r#"
spec inner
data zeta: number
data alpha: number

spec outer
uses inner
  -> with alpha: "no"
  -> with zeta: "also no"
"#;
    let mut engine = Engine::new();
    let err = engine
        .load([(source(), code.to_string())])
        .expect_err("both bindings are the wrong type");
    let errors: Vec<_> = err.iter().collect();
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert_eq!(errors[0].message(), "cannot use \"no\" as number");
    assert_eq!(errors[1].message(), "cannot use \"also no\" as number");
    let first = errors[0]
        .location()
        .expect("first binding error has a source");
    let second = errors[1]
        .location()
        .expect("second binding error has a source");
    assert!(
        first.span.line < second.span.line,
        "-> with errors must follow source order, got lines {} then {}",
        first.span.line,
        second.span.line
    );
}

#[test]
fn fill_foreign_unit_into_literal_measure_slot_returns_planning_error() {
    let code = r#"
spec inner
uses lemma units
data weight: 5 kilogram

spec outer
uses lemma units
uses inner
  -> with weight: 10 eur
"#;
    let mut engine = Engine::new();
    let (message, line) = load_single_error(&mut engine, code);
    assert_eq!(
        message,
        "Unknown unit 'eur' for this measure type. Valid units: kilogram, gram, milligram, microgram, tonne, ounce, pound, ton"
    );
    assert_eq!(line, 9);
}

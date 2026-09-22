use lemma::DateTimeValue;
use lemma::{Engine, Error};
use std::collections::HashMap;

/// Test suite for error messages as documented in ERROR_MESSAGES_IMPLEMENTATION.md
/// Covers parse errors, semantic errors, and runtime errors with proper span tracking

// ============================================================================
// VALIDATION ERRORS - Duplicate Definitions
// ============================================================================

#[test]
fn test_duplicate_data_definition_error() {
    let mut engine = Engine::new();

    let result = engine.load([(
        lemma::SourceType::Volatile,
        r#"
        spec test
        data salary: 50000
        data salary: 60000
    "#
        .to_string(),
    )]);

    let errs = result.unwrap_err();
    let details = errs
        .iter()
        .find_map(|e| match e {
            Error::Validation(d) => Some(d),
            _ => None,
        })
        .expect("expected at least one Validation error");
    let msg = &details.message;
    assert!(
        msg.contains("already used") && msg.contains("salary"),
        "Error should mention duplicate data name, got: {msg}"
    );
}

#[test]
fn test_uses_and_data_same_name_error() {
    let mut engine = Engine::new();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("age.lemma"))),
            r#"
spec age
data age: number
"#
            .to_string(),
        )])
        .expect("age spec");

    let errs = engine
        .load([(
            lemma::SourceType::Volatile,
            r#"
spec test
uses age
data age: age.age
"#
            .to_string(),
        )])
        .unwrap_err();

    let details = errs
        .iter()
        .find_map(|e| match e {
            Error::Validation(d) => Some(d),
            _ => None,
        })
        .expect("validation error");

    assert!(
        details.message.contains("age"),
        "message should name the clash, got: {}",
        details.message
    );
    let suggestion = details.suggestion.as_deref().expect("expected suggestion");
    assert!(
        suggestion.contains("age_spec"),
        "suggestion should show renamed import alias, got: {suggestion}"
    );
}

#[test]
fn test_duplicate_rule_definition_error() {
    let mut engine = Engine::new();

    let result = engine.load([(
        lemma::SourceType::Volatile,
        r#"
        spec test
        data x: 10
        rule total: x * 2
        rule total: x * 3
    "#
        .to_string(),
    )]);

    let errs = result.unwrap_err();
    let details = errs
        .iter()
        .find_map(|e| match e {
            Error::Validation(d) => Some(d),
            _ => None,
        })
        .expect("expected at least one Validation error");
    let msg = &details.message;
    assert!(
        msg.to_lowercase().contains("duplicate") && msg.to_lowercase().contains("rule"),
        "Error should mention duplicate rule, got: {msg}"
    );
    assert!(
        msg.contains("total"),
        "Error should mention rule name, got: {msg}"
    );
}

#[test]
fn test_duplicate_data_shows_name() {
    let mut engine = Engine::new();

    let result = engine.load([(
        lemma::SourceType::Volatile,
        r#"
        spec test
        data name: "Alice"
        data age: 30
        data name: "Bob"
    "#
        .to_string(),
    )]);

    let errs = result.unwrap_err();
    let details = errs
        .iter()
        .find_map(|e| match e {
            Error::Validation(d) => Some(d),
            _ => None,
        })
        .expect("expected at least one Validation error");
    let msg = &details.message;
    assert!(
        msg.contains("already used"),
        "Error should mention duplicate, got: {msg}"
    );
    assert!(
        msg.contains("name"),
        "Error should mention data name, got: {msg}"
    );
}

// ============================================================================
// RUNTIME VETO - Division by Zero
// ============================================================================

#[test]
fn test_runtime_veto_division_by_zero() {
    let mut engine = Engine::new();

    engine
        .load([(
            lemma::SourceType::Volatile,
            r#"
        spec test
        data numerator: 100
        data denominator: 0
        rule result: numerator / denominator
    "#
            .to_string(),
        )])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "test", Some(&now), HashMap::new(), None, false)
        .expect("Division by zero should return Veto, not Error");

    let result_rule = response
        .results
        .values()
        .find(|r| r.rule.name == "result")
        .expect("result rule should exist");

    assert!(
        result_rule.vetoed,
        "Division by zero should return Veto, got veto_reason: {:?}",
        result_rule.veto_reason
    );

    let message = result_rule.veto_reason.as_deref().expect("veto_reason");
    assert!(
        message.to_lowercase().contains("division") || message.to_lowercase().contains("zero"),
        "Veto message should mention division or zero, got: {}",
        message
    );
}

// ============================================================================
// Circular Dependencies
// ============================================================================

#[test]
fn test_transpile_error_self_referencing_rule() {
    let mut engine = Engine::new();

    // Self-referencing rules are caught during transpilation
    let result = engine.load([(
        lemma::SourceType::Volatile,
        r#"
        spec test
        rule x: x + 1
    "#
        .to_string(),
    )]);

    let errs = result.unwrap_err();
    let details = errs
        .iter()
        .find_map(|e| match e {
            Error::Validation(d) => Some(d),
            _ => None,
        })
        .expect("expected at least one Validation error");
    let msg = &details.message;
    assert!(msg.to_lowercase().contains("circular") || msg.to_lowercase().contains("itself"));
    assert!(msg.contains("x"));
}

// ============================================================================
// ERROR MESSAGE FORMATTING - Duplicate Detection
// ============================================================================

#[test]
fn test_duplicate_error_contains_data_name() {
    let mut engine = Engine::new();

    let result = engine.load([(
        lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
            "my_file.lemma",
        ))),
        r#"
        spec my_spec
        data price: 100
        data price: 200
    "#
        .to_string(),
    )]);

    let errs = result.unwrap_err();
    let details = errs
        .iter()
        .find_map(|e| match e {
            Error::Validation(d) => Some(d),
            _ => None,
        })
        .expect("expected at least one Validation error");
    assert!(details.message.contains("already used"));
    assert!(details.message.contains("price"));
}

#[test]
fn test_duplicate_error_is_reported() {
    let mut engine = Engine::new();

    let result = engine.load([(
        lemma::SourceType::Volatile,
        r#"
        spec test
        data x: 10
        data x: 20
    "#
        .to_string(),
    )]);

    let errs = result.unwrap_err();
    let details = errs
        .iter()
        .find_map(|e| match e {
            Error::Validation(d) => Some(d),
            _ => None,
        })
        .expect("expected at least one Validation error");
    assert!(details.message.contains("already used"));
    assert!(details.message.contains("x"));
}

#[test]
fn test_duplicate_in_second_spec_is_caught() {
    let mut engine = Engine::new();

    let result = engine.load([(
        lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("multi.lemma"))),
        r#"
        spec first_spec
        data a: 1

        spec second_spec
        data b: 2
        data b: 3
    "#
        .to_string(),
    )]);

    let errs = result.unwrap_err();
    let details = errs
        .iter()
        .find_map(|e| match e {
            Error::Validation(d) => Some(d),
            _ => None,
        })
        .expect("expected at least one Validation error");
    assert!(details.message.contains("already used"));
    assert!(details.message.contains("b"));
}

// ============================================================================
// ERROR MESSAGE DISPLAY
// ============================================================================

#[test]
fn test_error_display_contains_duplicate_info() {
    let mut engine = Engine::new();

    let result = engine.load([(
        lemma::SourceType::Volatile,
        r#"
        spec test
        data value: 100
        data value: 200
    "#
        .to_string(),
    )]);

    let errs = result.unwrap_err();
    let details = errs
        .iter()
        .find_map(|e| match e {
            Error::Validation(d) => Some(d),
            _ => None,
        })
        .expect("expected at least one Validation error");
    assert!(details.message.contains("already used"));
    assert!(details.message.contains("value"));
}

// ============================================================================
// VETO MESSAGES - Division by Zero
// ============================================================================

#[test]
fn test_division_by_zero_returns_veto_with_message() {
    let mut engine = Engine::new();

    engine
        .load([(
            lemma::SourceType::Volatile,
            r#"
        spec test
        data x: 100
        data y: 0
        rule result: x / y
    "#
            .to_string(),
        )])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "test", Some(&now), HashMap::new(), None, false)
        .expect("Should return Veto, not Error");

    let result_rule = response
        .results
        .values()
        .find(|r| r.rule.name == "result")
        .expect("result rule should exist");

    assert!(
        result_rule.vetoed,
        "Expected Veto, got display: {:?}",
        result_rule.result()
    );
    let message = result_rule.veto_reason.as_deref().expect("veto_reason");
    assert!(
        message.to_lowercase().contains("zero") || message.to_lowercase().contains("division"),
        "Veto message should mention zero or division, got: {}",
        message
    );
}

#[test]
fn test_circular_dependency_has_helpful_suggestion() {
    let mut engine = Engine::new();

    let result = engine.load([(
        lemma::SourceType::Volatile,
        r#"
        spec test
        rule x: y
        rule y: x
    "#
        .to_string(),
    )]);

    let errs = result.unwrap_err();
    let details = errs
        .iter()
        .find_map(|e| match e {
            Error::Validation(d) => Some(d),
            _ => None,
        })
        .expect("expected at least one Validation error");
    let msg = &details.message;
    assert!(msg.to_lowercase().contains("circular") || msg.to_lowercase().contains("cycle"));
    assert!(msg.contains("x") && msg.contains("y"));
}

// ============================================================================
// DUPLICATE DETECTION ACCURACY
// ============================================================================

#[test]
fn test_duplicate_data_is_detected() {
    let mut engine = Engine::new();

    let lemma_code = r#"spec test
data line2: 1
data line3: 2
data line4: 3
data line4: 4"#;

    let result = engine.load([(lemma::SourceType::Volatile, lemma_code.to_string())]);

    let errs = result.unwrap_err();
    let details = errs
        .iter()
        .find_map(|e| match e {
            Error::Validation(d) => Some(d),
            _ => None,
        })
        .expect("expected at least one Validation error");
    assert!(details.message.contains("already used"));
    assert!(details.message.contains("line4"));
}

// ============================================================================
// DUPLICATE DETECTION FROM VARIOUS SOURCES
// ============================================================================

#[test]
fn test_duplicate_detected_from_database_source() {
    let mut engine = Engine::new();

    let result = engine.load([(
        lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
            "db://contracts/123",
        ))),
        r#"
        spec contract
        data amount: 1000
        data amount: 2000
    "#
        .to_string(),
    )]);

    let errs = result.unwrap_err();
    let details = errs
        .iter()
        .find_map(|e| match e {
            Error::Validation(d) => Some(d),
            _ => None,
        })
        .expect("expected at least one Validation error");
    assert!(details.message.contains("already used"));
    assert!(details.message.contains("amount"));
}

// ============================================================================
// MULTI-ERROR COLLECTION - Graph building errors + type checking errors
// ============================================================================

/// Regression test: the engine must report errors from BOTH graph building
/// (e.g. missing reference) and type checking (e.g. branch type
/// mismatch) in a single pass.  Previously, graph building errors caused an
/// early return that prevented type checking from running at all.
#[test]
fn test_multiple_error_phases_reported_together() {
    let mut engine = Engine::new();

    let result = engine.load([(
        lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
            "pricing.lemma",
        ))),
        r#"
        spec pricing

        data money: measure
          -> unit eur: 1
          -> unit usd: 0.84

        data price    : money
        data quantity : number -> minimum 0
        data is_member: false

        rule discount: 0%
          unless quantity >= 10 then 10%
          unless quantity >= 50 then 20%
          unless is_member then 15

        rule total: price * quantity - non_existent_rule
          unless price > 100 usd then veto "This price is too high."
    "#
        .to_string(),
    )]);

    let errs = result.unwrap_err();
    let messages: Vec<String> = errs.iter().map(|e| e.to_string()).collect();
    let has_rule_ref_error = messages
        .iter()
        .any(|m| m.contains("non_existent_rule") && m.contains("not found"));
    let has_type_mismatch = messages
        .iter()
        .any(|m| m.contains("Type mismatch") || m.contains("type mismatch"));
    assert!(
        has_rule_ref_error,
        "Should report missing reference. Got: {messages:?}"
    );
    assert!(
        has_type_mismatch,
        "Should report type mismatch (15 is number, not ratio). Got: {messages:?}"
    );
}

// ============================================================================
// TEMPORAL / EffectiveDate::Origin — error text must stay readable
// ============================================================================

#[test]
fn unversioned_spec_missing_dep_message_names_specs() {
    let mut engine = Engine::new();
    let result = engine.load([(
        lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("t.lemma"))),
        r#"
spec app
uses z: no_such_dep
rule r: 1
"#
        .to_string(),
    )]);
    let errs = result.expect_err("missing dep");
    let joined = errs
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        joined.contains("no_such_dep") && joined.contains("app"),
        "expected naming both specs: {joined}"
    );
    assert!(
        !joined.ends_with("active at "),
        "message must not truncate after 'active at' with empty instant: {joined}"
    );
}

#[test]
fn unversioned_consumer_temporal_coverage_gap_names_consumer_and_dep() {
    let mut engine = Engine::new();
    let result = engine.load([(
        lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("gap.lemma"))),
        r#"
spec app
uses d: dep
rule r: d.x

spec dep 2025-12-01
data x: 1
"#
        .to_string(),
    )]);
    let errs = result.expect_err("coverage gap");
    let joined = errs
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        joined.contains("app") && joined.contains("dep"),
        "expected both spec names in coverage error: {joined}"
    );
}

// ============================================================================
// TYPE NAME CASING - planning errors must use Display (lowercase) not Debug
// ============================================================================
//
// LemmaType implements Display returning the lowercase Lemma type name
// (`number`, `boolean`, `text`, ...). User-facing planning errors must use
// `{}` not `{:?}` so the Rust enum variant names (`Number`, `Boolean`, ...)
// never leak into messages shown to spec authors.

fn assert_lowercase_type_in_errors(joined: &str, expected_lower: &[&str], banned: &[&str]) {
    for token in expected_lower {
        assert!(
            joined.contains(token),
            "expected lowercase '{token}' in error: {joined}"
        );
    }
    for token in banned {
        assert!(
            !joined.contains(token),
            "error must not leak Rust Debug variant '{token}': {joined}"
        );
    }
    assert!(
        !joined.contains("LemmaType {") && !joined.contains("specifications:"),
        "error must not leak LemmaType Debug syntax: {joined}"
    );
}

fn load_and_join_errors(code: &str) -> String {
    let mut engine = Engine::new();
    let errs = engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect_err("expected planning to fail");
    errs.iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(" | ")
}

#[test]
fn compare_number_with_text_uses_lowercase_type_names() {
    let joined = load_and_join_errors(
        r#"
spec test
rule r: 1 > "x"
"#,
    );
    assert!(
        joined.contains("Cannot compare"),
        "expected comparison error: {joined}"
    );
    assert_lowercase_type_in_errors(&joined, &["number", "text"], &["Number", "Text"]);
}

#[test]
fn logical_and_non_boolean_left_uses_lowercase() {
    let joined = load_and_join_errors(
        r#"
spec test
rule r: 1 and true
"#,
    );
    assert!(
        joined.contains("Logical AND requires boolean left operand"),
        "expected AND non-boolean error: {joined}"
    );
    assert_lowercase_type_in_errors(&joined, &["number"], &["Number"]);
}

#[test]
fn logical_and_non_boolean_right_uses_lowercase() {
    let joined = load_and_join_errors(
        r#"
spec test
data flag: boolean
rule r: flag and 5
"#,
    );
    assert!(
        joined.contains("Logical AND requires boolean right operand"),
        "expected AND non-boolean right error: {joined}"
    );
    assert_lowercase_type_in_errors(&joined, &["number"], &["Number"]);
}

#[test]
fn logical_negation_non_boolean_uses_lowercase() {
    let joined = load_and_join_errors(
        r#"
spec test
rule r: not 1
"#,
    );
    assert!(
        joined.contains("Logical negation requires boolean operand"),
        "expected negation error: {joined}"
    );
    assert_lowercase_type_in_errors(&joined, &["number"], &["Number"]);
}

#[test]
fn unless_condition_non_boolean_uses_lowercase() {
    let joined = load_and_join_errors(
        r#"
spec test
rule r: 0
  unless 1 then 1
"#,
    );
    assert!(
        joined.contains("Unless clause condition") && joined.contains("must be boolean"),
        "expected unless-condition error: {joined}"
    );
    assert_lowercase_type_in_errors(&joined, &["number"], &["Number"]);
}

#[test]
fn math_function_requires_number_uses_lowercase() {
    let joined = load_and_join_errors(
        r#"
spec test
rule r: sqrt true
"#,
    );
    assert!(
        joined.contains("Mathematical function 'sqrt' requires number operand, got boolean"),
        "expected math-function operand error: {joined}"
    );
    assert_lowercase_type_in_errors(&joined, &["boolean"], &["Boolean"]);
}

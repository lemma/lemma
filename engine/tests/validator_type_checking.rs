use lemma::Engine;

#[test]
fn test_logical_and_requires_boolean_operands() {
    let code = r#"
spec test
rule result: 5 and true
"#;

    let mut engine = Engine::new();
    let result = engine.load([(lemma::SourceType::Volatile, code.to_string())]);
    assert!(result.is_err(), "Should reject non-boolean in 'and'");
    let errs = result.unwrap_err();
    assert!(errs.iter().any(|e| e.to_string().contains("boolean")));
}

#[test]
fn test_logical_and_requires_boolean_right_operand() {
    let code = r#"
spec test
data flag: boolean
rule result: flag and 5
"#;

    let mut engine = Engine::new();
    let result = engine.load([(lemma::SourceType::Volatile, code.to_string())]);
    assert!(
        result.is_err(),
        "Should reject non-boolean right operand of 'and'"
    );
    let errs = result.unwrap_err();
    assert!(
        errs.iter()
            .any(|e| e.to_string().contains("boolean right operand")),
        "expected right-operand error, got: {errs:?}"
    );
}

#[test]
fn test_mixed_text_and_number_not_allowed() {
    let code = r#"
spec test
data flag: true
rule value: "default"
  unless flag then 42
"#;

    let mut engine = Engine::new();
    let result = engine.load([(lemma::SourceType::Volatile, code.to_string())]);
    assert!(
        result.is_err(),
        "Should reject mixing text and number types"
    );
    let errs = result.unwrap_err();
    let err_msg = errs
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("; ");
    assert!(
        err_msg.contains("incompatible") || err_msg.contains("Type mismatch"),
        "Error message should contain type mismatch info: {}",
        err_msg
    );
}

#[test]
fn test_time_cannot_use_in_logical_operators() {
    let code = r#"
spec test
data time1: 14:30:00
data time2: 15:00:00
rule result: time1 and time2
"#;

    let mut engine = Engine::new();
    let result = engine.load([(lemma::SourceType::Volatile, code.to_string())]);
    assert!(
        result.is_err(),
        "Should reject time values in logical operators"
    );
    let errs = result.unwrap_err();
    assert!(errs.iter().any(|e| e.to_string().contains("boolean")));
}

#[test]
fn test_mathematical_function_requires_number_operand() {
    let code = r#"
spec test
data money: measure -> unit eur: 1.00
data price: 100 eur
rule bad: sqrt price
"#;

    let mut engine = Engine::new();
    let result = engine.load([(lemma::SourceType::Volatile, code.to_string())]);
    assert!(
        result.is_err(),
        "sqrt(measure) should be rejected at planning"
    );
    let errs = result.unwrap_err();
    assert!(
        errs.iter()
            .any(|e| e.to_string().contains("number") || e.to_string().contains("Mathematical")),
        "Error should mention number operand: {:?}",
        errs
    );
}

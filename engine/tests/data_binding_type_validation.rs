/// Comprehensive tests for data binding validation at runtime.
///
/// After planning succeeds, invalid overrides complete evaluation with Veto on
/// affected rules — not `Err(Error)` from caller-data binding. Unknown data keys
/// are ignored.
use lemma::DateTimeValue;
use lemma::Engine;
use std::collections::HashMap;

fn assert_run_completes_with_veto_on_rule(
    result: Result<lemma::Response, lemma::Error>,
    rule_name: &str,
    reason_contains: &str,
) {
    let response = result.unwrap_or_else(|err| {
        panic!("run must complete with veto, not abort with Error — got: {err}")
    });
    let rule = response
        .results
        .get(rule_name)
        .unwrap_or_else(|| panic!("rule '{rule_name}' not in results"));
    assert!(
        rule.vetoed,
        "rule '{rule_name}' must veto on invalid override, got {:?}",
        rule.result()
    );
    if !reason_contains.is_empty() {
        let reason = rule.veto_reason.as_deref().expect("veto reason");
        assert!(
            reason.contains(reason_contains),
            "expected '{reason_contains}' in veto reason, got: {reason}"
        );
    }
}

#[test]
fn test_number_type_validation_rejects_text() {
    let code = r#"
spec test
data age: number
rule doubled: age * 2
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let mut data = HashMap::new();
    data.insert("age".to_string(), "twenty".into());

    let now = DateTimeValue::now();
    let result = engine.run(None, "test", Some(&now), data, None, true);

    assert_run_completes_with_veto_on_rule(result, "doubled", "number");
}

#[test]
fn test_multiple_type_validations() {
    let code = r#"
spec test
data price: number
data quantity: number
data active: boolean
rule total: price * quantity
rule flagged: active
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let mut data = HashMap::new();
    data.insert("price".to_string(), "expensive".into());
    data.insert("quantity".to_string(), "5".into());
    data.insert("active".to_string(), "true".into());

    let now = DateTimeValue::now();
    assert_run_completes_with_veto_on_rule(
        engine.run(None, "test", Some(&now), data, None, true),
        "total",
        "number",
    );

    let mut data = HashMap::new();
    data.insert("price".to_string(), "100".into());
    data.insert("quantity".to_string(), "five".into());
    data.insert("active".to_string(), "true".into());

    assert_run_completes_with_veto_on_rule(
        engine.run(None, "test", Some(&now), data, None, true),
        "total",
        "number",
    );

    let mut data = HashMap::new();
    data.insert("price".to_string(), "100".into());
    data.insert("quantity".to_string(), "5".into());
    data.insert("active".to_string(), "maybe".into());

    assert_run_completes_with_veto_on_rule(
        engine.run(None, "test", Some(&now), data, None, true),
        "flagged",
        "boolean",
    );

    let mut data = HashMap::new();
    data.insert("price".to_string(), "100".into());
    data.insert("quantity".to_string(), "5".into());
    data.insert("active".to_string(), "true".into());
    let response = engine
        .run(None, "test", Some(&now), data, None, true)
        .expect("valid data must evaluate");
    let total = response.results.get("total").expect("total rule");
    assert_eq!(total.result(), Some("500"));
}

#[test]
fn test_literal_data_type_validation() {
    let code = r#"
spec test
data base_price: 50
rule total: base_price * 1.2
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let mut data = HashMap::new();
    data.insert("base_price".to_string(), "sixty".into());

    let now = DateTimeValue::now();
    assert_run_completes_with_veto_on_rule(
        engine.run(None, "test", Some(&now), data, None, true),
        "total",
        "number",
    );

    let mut data = HashMap::new();
    data.insert("base_price".to_string(), "60".into());
    let response = engine
        .run(None, "test", Some(&now), data, None, true)
        .expect("valid base_price must evaluate");
    let total = response.results.get("total").expect("total rule");
    let display = total.result().expect("result");
    assert!(display.starts_with("72"), "60 * 1.2 = 72, got {}", display);
}

#[test]
fn test_unknown_data_binding_ignored() {
    let code = r#"
spec test
data price: number
rule total: price * 1.1
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();

    let mut data = HashMap::new();
    data.insert("price".to_string(), "100".into());
    data.insert("unknown_data".to_string(), "42".into());

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "test", Some(&now), data, None, true)
        .expect("unknown keys must not abort evaluation");
    let total = response.results.get("total").expect("total rule");
    assert_eq!(total.result(), Some("110"));
}

/// Matrix: primitive × applicable constraint × violating user value.
/// Each row asserts the (load accepted, run rejected with constraint name)
/// behavior for a valid constraint-primitive pairing, and the (load rejected)
/// behavior for an incompatible pairing.
///
/// Tests that encode intended behavior stay red when the planner silently
/// accepts an invalid combination — that's the deliverable.

#[test]
fn percent_minimum_violation_on_override() {
    let code = r#"
spec s
data p: ratio -> minimum 10%
rule r: p
"#;
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("m.lemma"))),
            code.to_string(),
        )])
        .unwrap();

    let mut data = HashMap::new();
    data.insert("p".to_string(), "5%".into());

    let now = DateTimeValue::now();
    assert_run_completes_with_veto_on_rule(
        engine.run(None, "s", Some(&now), data, None, true),
        "r",
        "minimum",
    );
}

/// Pin that runtime override `"5%"` parses as `Ratio(0.05, "percent")` exactly.
/// Without this, a 100x regression (storing `Ratio(5, "percent")`) would silently
/// SATISFY a `minimum 10%` constraint (5 > 0.10), making the constraint-violation
/// tests above pass via the wrong path.
#[test]
fn percent_override_value_is_pinned() {
    use lemma::ValueKind;
    use rust_decimal::Decimal;
    use std::str::FromStr;
    fn decimal_lit(d: &str) -> Decimal {
        Decimal::from_str(d).unwrap()
    }
    let code = r#"
spec s
data p: ratio
rule r: p
"#;
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("m.lemma"))),
            code.to_string(),
        )])
        .unwrap();

    let mut data = HashMap::new();
    data.insert("p".to_string(), "5%".into());

    let now = DateTimeValue::now();
    let resp = engine
        .run(None, "s", Some(&now), data, None, true)
        .expect("'5%' must parse on a percent type without constraints");
    let rr = resp.results.get("r").expect("rule 'r' not found");
    assert!(!rr.vetoed, "unexpected veto: {:?}", rr.veto_reason);
    let lit = rr
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("value");
    match &lit.value {
        ValueKind::Ratio(n) => {
            assert_eq!(
                lemma::ValueKind::Number(n.clone())
                    .as_decimal_magnitude()
                    .unwrap(),
                decimal_lit("0.05")
            );
            // Display/binding unit lives on the rule type, not LiteralValue.
            assert!(
                matches!(lit.value, ValueKind::Ratio(_)),
                "expected Ratio value"
            );
        }
        other => panic!("expected Ratio, got: {:?}", other),
    }
}

#[test]
fn percent_maximum_violation_on_override() {
    let code = r#"
spec s
data p: ratio -> maximum 50%
rule r: p
"#;
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("m.lemma"))),
            code.to_string(),
        )])
        .unwrap();

    let mut data = HashMap::new();
    data.insert("p".to_string(), "90%".into());

    let now = DateTimeValue::now();
    assert_run_completes_with_veto_on_rule(
        engine.run(None, "s", Some(&now), data, None, true),
        "r",
        "maximum",
    );
}

#[test]
fn duration_minimum_violation_on_override() {
    let code = r#"
spec s
uses lemma units
data d: units.duration -> minimum 1 day
rule r: d
"#;
    let mut engine = Engine::new();
    let load_result = engine.load([(
        lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("m.lemma"))),
        code.to_string(),
    )]);
    if let Err(errors) = &load_result {
        // If `minimum` with duration literal RHS is not supported, that
        // itself is a landmine — durations can definitely have minimums.
        panic!(
            "duration minimum must be supported; load failed with: {}",
            errors
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    let mut data = HashMap::new();
    data.insert("d".to_string(), "12 hour".into());

    let now = DateTimeValue::now();
    assert_run_completes_with_veto_on_rule(
        engine.run(None, "s", Some(&now), data, None, true),
        "r",
        "minimum",
    );
}

#[test]
fn date_minimum_violation_on_override() {
    let code = r#"
spec s
data when: date -> minimum 2024-01-01
rule r: when
"#;
    let mut engine = Engine::new();
    let load_result = engine.load([(
        lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("m.lemma"))),
        code.to_string(),
    )]);
    if let Err(errors) = &load_result {
        panic!(
            "date minimum must be supported; load failed with: {}",
            errors
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    let mut data = HashMap::new();
    data.insert("when".to_string(), "2023-06-15".into());

    let now = DateTimeValue::now();
    assert_run_completes_with_veto_on_rule(
        engine.run(None, "s", Some(&now), data, None, true),
        "r",
        "minimum",
    );
}

#[test]
fn number_decimals_constraint_vetoes_excess_precision() {
    // `decimals 2` on a number: excess fractional digits veto that data.
    let code = r#"
spec s
data n: number -> decimals 2
rule r: n
"#;
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("m.lemma"))),
            code.to_string(),
        )])
        .unwrap();

    let mut data = HashMap::new();
    data.insert("n".to_string(), "3.14159".into());

    let now = DateTimeValue::now();
    assert_run_completes_with_veto_on_rule(
        engine.run(None, "s", Some(&now), data, None, true),
        "r",
        "decimals",
    );
}

#[test]
fn text_length_exactly_at_boundary_accepted() {
    let code = r#"
spec s
data msg: text -> length 5
rule r: msg
"#;
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("m.lemma"))),
            code.to_string(),
        )])
        .unwrap();

    let mut data = HashMap::new();
    data.insert("msg".to_string(), "exact".into());

    let now = DateTimeValue::now();
    let resp = engine
        .run(None, "s", Some(&now), data, None, true)
        .expect("5-char string must be accepted");
    let rr = resp.results.get("r").expect("rule 'r'");
    assert!(!rr.vetoed, "expected value, got veto: {:?}", rr.veto_reason);
    assert_eq!(
        rr.result
            .as_ref()
            .expect("rule result value")
            .text
            .as_deref(),
        Some("exact")
    );
}

#[test]
fn import_binding_unit_factor_override_errors() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "finance.lemma",
            ))),
            r#"
spec finance
data money: measure
  -> unit eur: 1.00
  -> unit usd: 0.91
"#
            .to_string(),
        )])
        .expect("finance spec must load");

    let result = engine.load([(
        lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
            "pricing.lemma",
        ))),
        r#"
spec pricing
uses fin: finance
data currency: fin.money
  -> unit usd: 0.84
rule r: currency
"#
        .to_string(),
    )]);
    assert!(
        result.is_err(),
        "import binding must not override inherited unit factor"
    );
    let error_msg = format!("{:?}", result.unwrap_err());
    assert!(
        error_msg.contains("usd"),
        "error must name unit usd, got: {error_msg}"
    );
}

#[test]
fn measure_override_with_wrong_unit_rejected() {
    let code = r#"
spec s
data money: measure -> unit eur: 1 -> unit usd: 0.84
data price: money
rule r: price
"#;
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("m.lemma"))),
            code.to_string(),
        )])
        .unwrap();

    let mut data = HashMap::new();
    // `meter` is not a unit of `money`.
    data.insert("price".to_string(), "100 meter".into());

    let now = DateTimeValue::now();
    assert_run_completes_with_veto_on_rule(
        engine.run(None, "s", Some(&now), data, None, true),
        "r",
        "unit",
    );
}

#[test]
fn test_veto_reason_on_invalid_measure_unit_override() {
    let code = r#"
spec bridge
data bridge_height: measure -> unit meter: 1.0
rule span: bridge_height
"#;

    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "workspace.lemma",
            ))),
            code.to_string(),
        )])
        .unwrap();

    let mut data = HashMap::new();
    data.insert("bridge_height".to_string(), "4 mete".into());

    let now = DateTimeValue::now();
    assert_run_completes_with_veto_on_rule(
        engine.run(None, "bridge", Some(&now), data, None, true),
        "span",
        "Unknown unit",
    );
}

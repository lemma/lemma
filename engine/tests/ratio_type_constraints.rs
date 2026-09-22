//! Ratio typedef constraints (`minimum`, `maximum`, `default`) with custom units.
//!
//! Parser emits `N label` as `Value::NumberWithUnit`; planning must resolve against
//! the typedef's `RatioUnits` (same as measure constraints after scale rename).

use lemma::DateTimeValue;
use lemma::Engine;
use lemma::TypeSpecification;
use lemma::ValueKind;
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::str::FromStr;

fn decimal_lit(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn load(engine: &mut Engine, code: &str) {
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "ratio_constraints.lemma",
            ))),
            code.to_string(),
        )])
        .unwrap_or_else(|errs| {
            let joined = errs
                .errors
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("\n");
            panic!("expected load to succeed, got: {joined}");
        });
}

fn bps_spec() -> &'static str {
    r#"
spec s
data r: ratio
  -> unit basis_points: 10000
  -> minimum 500 basis_points
  -> maximum 10000 basis_points
rule out: r
"#
}

#[test]
fn ratio_minimum_custom_unit_loads_with_canonical_bounds() {
    let mut engine = Engine::new();
    load(&mut engine, bps_spec());

    let now = DateTimeValue::now();
    let show = engine.show(None, "s", Some(&now)).expect("show");
    let entry = show.data.get("r").expect("data r");
    match &entry.lemma_type.specifications {
        TypeSpecification::Ratio { units, .. } => {
            assert_eq!(
                entry.lemma_type.specifications.minimum_decimal(),
                Some(decimal_lit("0.05")),
                "500 basis_points / 10000"
            );
            assert_eq!(
                entry.lemma_type.specifications.maximum_decimal(),
                Some(decimal_lit("1")),
                "10000 basis_points / 10000"
            );
            let bps = units.get("basis_points").expect("basis_points unit");
            assert_eq!(bps.minimum_decimal(), Some(decimal_lit("500")));
            assert_eq!(bps.maximum_decimal(), Some(decimal_lit("10000")));
        }
        other => panic!("expected Ratio type, got {:?}", other),
    }
}

#[test]
fn ratio_bare_minimum_rejected_at_load() {
    let code = r#"
spec s
data r: ratio -> minimum 0
rule out: r
"#;
    let mut engine = Engine::new();
    let err = engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "ratio_constraints.lemma",
            ))),
            code.to_string(),
        )])
        .expect_err("bare number minimum on ratio must fail");
    let s = err
        .errors
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase();
    assert!(
        s.contains("ratio") && (s.contains("unit") || s.contains("%") || s.contains("bare")),
        "expected ratio unit syntax error, got: {s}"
    );
}

#[test]
fn ratio_bare_default_rejected_at_load() {
    let code = r#"
spec s
data r: ratio -> suggest 0.015
rule out: r
"#;
    let mut engine = Engine::new();
    let err = engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "ratio_constraints.lemma",
            ))),
            code.to_string(),
        )])
        .expect_err("bare number default on ratio must fail");
    let s = err
        .errors
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase();
    assert!(
        s.contains("suggest") && (s.contains("unit") || s.contains("%")),
        "expected ratio suggest unit syntax error, got: {s}"
    );
}

#[test]
fn ratio_bare_maximum_rejected_at_load() {
    let code = r#"
spec s
data r: ratio -> maximum 1
rule out: r
"#;
    let mut engine = Engine::new();
    let err = engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "ratio_constraints.lemma",
            ))),
            code.to_string(),
        )])
        .expect_err("bare number maximum on ratio must fail");
    let s = err
        .errors
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase();
    assert!(
        s.contains("maximum") && (s.contains("unit") || s.contains("%")),
        "expected ratio maximum unit syntax error, got: {s}"
    );
}

#[test]
fn ratio_default_percent_loads() {
    let code = r#"
spec s
data r: ratio -> suggest 1.5%
rule out: r
"#;
    let mut engine = Engine::new();
    load(&mut engine, code);

    let now = DateTimeValue::now();
    let show = engine.show(None, "s", Some(&now)).expect("show");
    let entry = show.data.get("r").expect("data r");
    let default = entry.suggestion.as_ref().expect("declared default");
    let percent = default
        .ratio
        .as_ref()
        .and_then(|m| m.get("percent"))
        .copied()
        .expect("percent magnitude");
    assert_eq!(percent, decimal_lit("1.5"));
}

#[test]
fn ratio_minimum_percent_constraint_loads() {
    let code = r#"
spec s
data r: ratio -> minimum 10% -> maximum 100%
rule out: r
"#;
    let mut engine = Engine::new();
    load(&mut engine, code);

    let now = DateTimeValue::now();
    let show = engine.show(None, "s", Some(&now)).expect("show");
    let entry = show.data.get("r").expect("data r");
    assert_eq!(
        entry.lemma_type.specifications.minimum_decimal(),
        Some(decimal_lit("0.10"))
    );
}

#[test]
fn ratio_minimum_custom_unit_override_enforced() {
    let mut engine = Engine::new();
    load(&mut engine, bps_spec());

    let mut data = HashMap::new();
    data.insert("r".to_string(), "400 basis_points".into());

    let now = DateTimeValue::now();
    let resp = engine
        .run(None, "s", Some(&now), data, None, true)
        .expect("400 bps < 500 bps minimum must complete with veto");
    let rr = resp.results.get("out").expect("rule out");
    assert!(rr.vetoed, "400 bps < 500 bps minimum must veto");
    let message = rr.veto_reason.clone().expect("veto reason");
    assert!(
        message.contains("minimum") || message.to_lowercase().contains("below"),
        "expected minimum violation, got: {message}"
    );
    assert!(
        message.contains("500 basis_points"),
        "expected per-unit bound in message, got: {message}"
    );
    assert!(
        !message.contains("0.05"),
        "must not show canonical ratio magnitude, got: {message}"
    );
    assert!(
        !message.to_lowercase().contains("canonical"),
        "must not mention canonical units, got: {message}"
    );
}

#[test]
fn ratio_default_custom_unit_loads() {
    let code = r#"
spec s
data r: ratio
  -> unit basis_points: 10000
  -> suggest 500 basis_points
rule out: r
"#;
    let mut engine = Engine::new();
    load(&mut engine, code);

    let now = DateTimeValue::now();
    let show = engine.show(None, "s", Some(&now)).expect("show");
    let entry = show.data.get("r").expect("data r");
    let default = entry.suggestion.as_ref().expect("declared default");
    let basis_points = default
        .ratio
        .as_ref()
        .and_then(|m| m.get("basis_points"))
        .copied()
        .expect("basis_points magnitude");
    assert_eq!(basis_points, decimal_lit("500"));
}

#[test]
fn ratio_minimum_custom_unit_override_accepts_at_bound() {
    let mut engine = Engine::new();
    load(&mut engine, bps_spec());

    let mut data = HashMap::new();
    data.insert("r".to_string(), "500 basis_points".into());

    let now = DateTimeValue::now();
    let resp = engine
        .run(None, "s", Some(&now), data, None, true)
        .expect("500 bps meets minimum");
    let rr = resp.results.get("out").expect("rule out");
    if rr.vetoed {
        panic!(
            "unexpected veto: {}",
            rr.veto_reason.as_deref().unwrap_or("Vetoed")
        );
    }
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
            assert!(
                matches!(lit.value, ValueKind::Ratio(_))
                    || matches!(lit.value, lemma::ValueKind::Ratio(_))
            );
            // binding unit "basis_points" lives on type, not LiteralValue
        }
        other => panic!("expected Ratio, got {:?}", other),
    }
}

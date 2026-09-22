//! QA coverage for `DataValue::Definition` with inline literal RHS (`value: Some(Value)`).
//!
//! One test per primitive literal kind + edge cases. Assertions pin exact
//! values. Tests that encode invariants the implementation may not satisfy
//! are EXPECTED TO FAIL and must remain red. Do not weaken.

use lemma::DateTimeValue;
use lemma::Engine;
use rust_decimal::Decimal;
use std::collections::HashMap;

fn load_ok(engine: &mut Engine, code: &str) {
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "literals.lemma",
            ))),
            code.to_string(),
        )])
        .unwrap_or_else(|errs| {
            let joined = errs
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("\n");
            panic!("expected load to succeed, got: {joined}");
        });
}

fn load_err_joined(engine: &mut Engine, code: &str) -> String {
    let err = engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "literals.lemma",
            ))),
            code.to_string(),
        )])
        .expect_err("expected load to fail");
    err.iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

fn rule_value(result: &lemma::Response, rule_name: &str) -> String {
    let rr = result
        .results
        .get(rule_name)
        .unwrap_or_else(|| panic!("rule '{}' not found", rule_name));
    if rr.vetoed {
        return format!("VETO({})", rr.veto_reason.as_deref().unwrap_or("Vetoed"));
    }
    rr.result().expect("result").to_string()
}

fn rule_measure_unit(result: &lemma::Response, rule_name: &str, unit: &str) -> Decimal {
    let rr = result
        .results
        .get(rule_name)
        .unwrap_or_else(|| panic!("rule '{}' not found", rule_name));
    assert!(!rr.vetoed, "rule '{}' vetoed", rule_name);
    *rr.result
        .as_ref()
        .and_then(|v| v.measure.as_ref())
        .and_then(|map| map.get(unit))
        .unwrap_or_else(|| panic!("measure map missing unit '{unit}' for rule '{rule_name}'"))
}

fn run(engine: &Engine, spec: &str) -> lemma::Response {
    let now = DateTimeValue::now();
    engine
        .run(None, spec, Some(&now), HashMap::new(), None, true)
        .expect("run")
}

fn run_explain(engine: &Engine, spec: &str) -> lemma::Response {
    let now = DateTimeValue::now();
    engine
        .run(None, spec, Some(&now), HashMap::new(), None, true)
        .expect("run")
}

// ─── Number literals ──────────────────────────────────────────────────

#[test]
fn number_literal_integer() {
    let code = r#"
spec s
data n: 42
rule r: n
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    assert_eq!(rule_value(&run(&engine, "s"), "r"), "42");
}

#[test]
fn number_literal_decimal() {
    let code = r#"
spec s
data n: 3.14
rule r: n
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    assert_eq!(rule_value(&run(&engine, "s"), "r"), "3.14");
}

#[test]
fn number_literal_scientific_notation() {
    let code = r#"
spec s
data big: 1.23e10
data small: 5e-3
rule r_big: big
rule r_small: small
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let response = run(&engine, "s");
    assert_eq!(rule_value(&response, "r_big"), "12300000000");
    assert_eq!(rule_value(&response, "r_small"), "0.005");
}

#[test]
fn number_literal_zero_normalizes() {
    let code = r#"
spec s
data n: 0.0
rule r: n
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    assert_eq!(rule_value(&run(&engine, "s"), "r"), "0");
}

#[test]
fn number_literal_negative_via_unary_minus() {
    let code = r#"
spec s
data n: -5
rule r: n
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    assert_eq!(rule_value(&run(&engine, "s"), "r"), "-5");
}

#[test]
fn number_literal_explicit_positive_via_unary_plus() {
    let code = r#"
spec s
data n: +7
rule r: n
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    assert_eq!(rule_value(&run(&engine, "s"), "r"), "7");
}

#[test]
fn number_literal_very_long_decimal_preserves_precision() {
    let code = r#"
spec s
data n: 1.234567890123456789
rule r: n
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    // Pin the exact display. If precision is silently truncated the assertion fails.
    assert_eq!(rule_value(&run(&engine, "s"), "r"), "1.234567890123456789");
}

// ─── Text literals ────────────────────────────────────────────────────

#[test]
fn text_literal_basic() {
    let code = r#"
spec s
data msg: "hello"
rule r: msg
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    assert_eq!(rule_value(&run(&engine, "s"), "r"), "hello");
}

#[test]
fn text_literal_empty() {
    let code = "
spec s
data msg: \"\"
rule r: msg
";
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    assert_eq!(rule_value(&run(&engine, "s"), "r"), "");
}

#[test]
fn text_literal_unicode() {
    let code = "
spec s
data msg: \"日本語 café\"
rule r: msg
";
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    assert_eq!(rule_value(&run(&engine, "s"), "r"), "日本語 café");
}

// ─── Boolean literals ─────────────────────────────────────────────────

#[test]
fn boolean_literal_true() {
    let code = r#"
spec s
data b: true
rule r: b
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    assert_eq!(rule_value(&run(&engine, "s"), "r"), "true");
}

#[test]
fn boolean_literal_false() {
    let code = r#"
spec s
data b: false
rule r: b
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    assert_eq!(rule_value(&run(&engine, "s"), "r"), "false");
}

#[test]
fn boolean_literal_yes() {
    let code = r#"
spec s
data b: yes
rule r: b
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let out = rule_value(&run(&engine, "s"), "r");
    assert!(
        out == "true" || out == "yes",
        "boolean 'yes' must render consistently, got: {out}"
    );
}

#[test]
fn boolean_literal_no() {
    let code = r#"
spec s
data b: no
rule r: b
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let out = rule_value(&run(&engine, "s"), "r");
    assert!(
        out == "false" || out == "no",
        "boolean 'no' must render consistently, got: {out}"
    );
}

// ─── Date / Time literals ─────────────────────────────────────────────

#[test]
fn date_literal_ymd() {
    let code = r#"
spec s
data d: 2024-01-15
rule r: d
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let out = rule_value(&run(&engine, "s"), "r");
    assert!(
        out.starts_with("2024-01-15"),
        "date must round-trip starting with 2024-01-15, got: {out}"
    );
}

#[test]
fn date_literal_invalid_month_rejected() {
    // February 30 does not exist. Must be rejected at parse/plan time, NOT
    // silently clamped or rolled forward.
    let code = r#"
spec s
data d: 2024-02-30
rule r: d
"#;
    let mut engine = Engine::new();
    let joined = load_err_joined(&mut engine, code);
    assert!(
        !joined.is_empty(),
        "invalid date 2024-02-30 must produce a parse/plan error; engine silently accepted it"
    );
}

#[test]
fn date_literal_zero_day_rejected() {
    let code = r#"
spec s
data d: 2024-05-00
rule r: d
"#;
    let mut engine = Engine::new();
    let joined = load_err_joined(&mut engine, code);
    assert!(
        !joined.is_empty(),
        "invalid date 2024-05-00 must produce an error"
    );
}

#[test]
fn time_literal_hh_mm() {
    let code = r#"
spec s
data t: 14:30
rule r: t
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let out = rule_value(&run(&engine, "s"), "r");
    assert!(
        out.starts_with("14:30"),
        "time must render starting with 14:30, got: {out}"
    );
}

#[test]
fn time_literal_hh_mm_ss() {
    let code = r#"
spec s
data t: 14:30:45
rule r: t
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let out = rule_value(&run(&engine, "s"), "r");
    assert!(
        out.starts_with("14:30:45"),
        "time must render starting with 14:30:45, got: {out}"
    );
}

#[test]
fn time_literal_invalid_hour_rejected() {
    let code = r#"
spec s
data t: 25:00
rule r: t
"#;
    let mut engine = Engine::new();
    let joined = load_err_joined(&mut engine, code);
    assert!(
        !joined.is_empty(),
        "invalid time 25:00 must produce an error; engine silently accepted it"
    );
}

// ─── Duration literals ────────────────────────────────────────────────

#[test]
fn duration_literal_years_plural() {
    let code = r#"
spec s
uses lemma units
data d: 5 year
rule r: d
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let out = rule_value(&run(&engine, "s"), "r");
    assert!(out.contains("5") && out.contains("year"), "got: {out}");
}

#[test]
fn duration_literal_year_singular() {
    let code = r#"
spec s
uses lemma units
data d: 1 year
rule r: d
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let out = rule_value(&run(&engine, "s"), "r");
    assert!(out.contains("1") && out.contains("year"), "got: {out}");
}

#[test]
fn duration_literal_months() {
    let code = r#"
spec s
uses lemma units
data d: 3 month
rule r: d
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let out = rule_value(&run(&engine, "s"), "r");
    assert!(out.contains("3") && out.contains("month"), "got: {out}");
}

#[test]
fn duration_literal_weeks() {
    let code = r#"
spec s
uses lemma units
data d: 2 week
rule r: d
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let out = rule_value(&run(&engine, "s"), "r");
    assert!(out.contains("2") && out.contains("week"), "got: {out}");
}

#[test]
fn duration_literal_days() {
    let code = r#"
spec s
uses lemma units
data d: 7 day
rule r: d
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    assert_eq!(
        rule_measure_unit(&run(&engine, "s"), "r", "day"),
        Decimal::from(7)
    );
}

#[test]
fn duration_literal_hours() {
    let code = r#"
spec s
uses lemma units
data d: 12 hour
rule r: d
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let out = rule_value(&run(&engine, "s"), "r");
    assert!(out.contains("12") && out.contains("hour"), "got: {out}");
}

#[test]
fn duration_literal_minutes() {
    let code = r#"
spec s
uses lemma units
data d: 90 minute
rule r: d
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let out = rule_value(&run(&engine, "s"), "r");
    assert!(out.contains("90") && out.contains("minute"), "got: {out}");
}

#[test]
fn duration_literal_seconds() {
    let code = r#"
spec s
uses lemma units
data d: 45 second
rule r: d
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let out = rule_value(&run(&engine, "s"), "r");
    assert!(out.contains("45") && out.contains("second"), "got: {out}");
}

#[test]
fn duration_literal_negative_preserves_sign() {
    let code = r#"
spec s
uses lemma units
data d: -5 day
rule r: d
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let out = rule_value(&run(&engine, "s"), "r");
    assert!(
        out.contains("-5") && out.contains("day"),
        "negative duration literal must preserve sign; got: {out}"
    );
}

// ─── Ratio literals ───────────────────────────────────────────────────

/// Pin a rule's value as `ValueKind::Ratio(decimal)`. Panics
/// on any other shape so a regression in either the numeric magnitude or the
/// unit name fails loudly (substring matches on `Display` cannot — a 100x off
/// value renders as `"5000%"` which still contains `"50"` and `"%"`).
fn rule_ratio(
    result: &lemma::Response,
    rule_name: &str,
) -> (rust_decimal::Decimal, Option<String>) {
    use lemma::ValueKind;
    let rr = result
        .results
        .get(rule_name)
        .unwrap_or_else(|| panic!("rule '{}' not found", rule_name));
    assert!(
        !rr.vetoed,
        "rule '{}' produced veto: {}",
        rule_name,
        rr.veto_reason.as_deref().unwrap_or("Vetoed")
    );
    let lit = rr
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("value");
    match &lit.value {
        ValueKind::Ratio(n) => (
            lemma::ValueKind::Number(n.clone())
                .as_decimal_magnitude()
                .unwrap(),
            rr.result
                .as_ref()
                .and_then(|v| v.ratio.as_ref())
                .and_then(|m| {
                    rr.rule
                        .rule_type
                        .measure_binding_unit
                        .as_ref()
                        .and_then(|u| m.contains_key(u).then(|| u.clone()))
                        .or_else(|| m.keys().next().cloned())
                }),
        ),
        other => panic!("rule '{}' produced non-Ratio value {:?}", rule_name, other),
    }
}

#[test]
fn ratio_literal_percent_sign() {
    let code = r#"
spec s
data r: 50%
rule out: r
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let resp = run_explain(&engine, "s");
    let (value, unit) = rule_ratio(&resp, "out");
    assert_eq!(value, rust_decimal::Decimal::new(50, 2));
    assert_eq!(unit.as_deref(), Some("percent"));
    assert_eq!(rule_value(&resp, "out"), "50%");
}

#[test]
fn ratio_literal_permille_sign() {
    let code = r#"
spec s
data r: 25%%
rule out: r
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let resp = run_explain(&engine, "s");
    let (value, unit) = rule_ratio(&resp, "out");
    assert_eq!(value, rust_decimal::Decimal::new(25, 3));
    assert_eq!(unit.as_deref(), Some("permille"));
    assert_eq!(rule_value(&resp, "out"), "25%%");
}

#[test]
fn ratio_literal_percent_keyword_matches_sigil() {
    let code = r#"
spec s
data r: 50 percent
rule out: r
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let resp = run_explain(&engine, "s");
    let (value, unit) = rule_ratio(&resp, "out");
    assert_eq!(value, rust_decimal::Decimal::new(50, 2));
    assert_eq!(unit.as_deref(), Some("percent"));
    assert_eq!(rule_value(&resp, "out"), "50%");
}

#[test]
fn ratio_literal_permille_keyword_matches_sigil() {
    let code = r#"
spec s
data r: 25 permille
rule out: r
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let resp = run_explain(&engine, "s");
    let (value, unit) = rule_ratio(&resp, "out");
    assert_eq!(value, rust_decimal::Decimal::new(25, 3));
    assert_eq!(unit.as_deref(), Some("permille"));
    assert_eq!(rule_value(&resp, "out"), "25%%");
}

#[test]
fn ratio_literal_negative_percent_sign() {
    let code = r#"
spec s
data r: -50%
rule out: r
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let resp = run_explain(&engine, "s");
    let (value, unit) = rule_ratio(&resp, "out");
    assert_eq!(value, rust_decimal::Decimal::new(-50, 2));
    assert_eq!(unit.as_deref(), Some("percent"));
    assert_eq!(rule_value(&resp, "out"), "-50%");
}

#[test]
fn ratio_literal_bare_number_has_no_unit() {
    let code = r#"
spec s
data r: 0.25
rule out: r
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let resp = run_explain(&engine, "s");
    let rr = resp.results.get("out").expect("rule 'out' not found");
    assert!(!rr.vetoed, "rule 'out' produced veto: {:?}", rr.veto_reason);
    let lit = rr
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("value");
    use lemma::ValueKind;
    match &lit.value {
        ValueKind::Number(n) => {
            assert_eq!(
                lemma::ValueKind::Number(n.clone())
                    .as_decimal_magnitude()
                    .unwrap(),
                rust_decimal::Decimal::new(25, 2)
            );
        }
        ValueKind::Ratio(n) => {
            assert_eq!(
                lemma::ValueKind::Number(n.clone())
                    .as_decimal_magnitude()
                    .unwrap(),
                rust_decimal::Decimal::new(25, 2)
            );
            // bare ratio has no unit on LiteralValue after migration
        }
        other => panic!("expected Number or Ratio, got: {:?}", other),
    }
}

// ─── Measure literals (require user-defined unit) ───────────────────────

#[test]
fn measure_literal_with_defined_unit() {
    let code = r#"
spec s
data money: measure
  -> unit eur: 1
  -> unit usd: 0.84
data price: 10 eur
rule r: price
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let out = rule_value(&run(&engine, "s"), "r");
    assert!(out.contains("10") && out.contains("eur"), "got: {out}");
}

#[test]
fn measure_literal_with_unknown_unit_is_rejected() {
    // No measure type defines `banana` as a unit; the literal must fail.
    let code = r#"
spec s
data money: measure -> unit eur: 1
data price: 10 banana
rule r: price
"#;
    let mut engine = Engine::new();
    let joined = load_err_joined(&mut engine, code);
    assert!(
        joined.contains("unknown unit")
            || joined.contains("Unknown unit")
            || joined.contains("'banana'"),
        "expected unknown-unit error, got: {joined}"
    );
}

#[test]
fn measure_literal_conversion_to_defined_unit() {
    let code = r#"
spec s
data money: measure
  -> unit eur: 1
  -> unit usd: 0.84
data price: 10 usd
rule r: price
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let out = rule_value(&run(&engine, "s"), "r");
    assert!(out.contains("10") && out.contains("usd"), "got: {out}");
}

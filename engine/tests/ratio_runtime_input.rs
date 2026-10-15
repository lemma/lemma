//! Strict runtime-input grammar for Ratio-typed data overrides.
//!
//! Covers `engine.evaluate(None, spec, ..., data: IndexMap<String, EvaluateDataValue>, ...)` where each
//! string flows through `parse_value_from_string` → `parse_number_unit::Ratio` →
//! `RatioLiteral::parse`. Pins exact `ValueKind::Ratio(decimal)`,
//! not substrings of `Display`, so a 100x off value cannot pass.
//!
//! Also includes a Measure-side regression: `"5%"` against a Measure type must veto
//! with the friendly Measure message (no leftover `%`/`%%` handling in the Measure
//! literal parser).

use indexmap::IndexMap;
use lemma::DateTimeValue;
use lemma::Engine;
use lemma::ValueKind;
use rust_decimal::Decimal;
use std::str::FromStr;

fn decimal_lit(d: &str) -> Decimal {
    Decimal::from_str(d).unwrap()
}

fn load(engine: &mut Engine, code: &str) {
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "ratio_in.lemma",
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

fn evaluate_rational(engine: &Engine, spec: &str, raw: &str) -> (Decimal, Option<String>) {
    let mut data = IndexMap::new();
    data.insert("r".to_string(), raw.into());
    let now = DateTimeValue::now();
    let resp = engine
        .evaluate(None, spec, Some(&now), data, None, true)
        .unwrap_or_else(|e| panic!("run failed for input '{raw}': {e}"));
    let rr = resp
        .results
        .get("out")
        .unwrap_or_else(|| panic!("rule 'out' not found"));
    assert!(
        !rr.vetoed,
        "input '{raw}' produced veto: {:?}",
        rr.veto_reason
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
                        .unit()
                        .filter(|u| m.contains_key(*u))
                        .map(str::to_string)
                        .or_else(|| {
                            // Overlay unit identity: name present in the raw input.
                            if (raw.contains("%%") || raw.contains("permille"))
                                && m.contains_key("permille")
                            {
                                Some("permille".to_string())
                            } else if (raw.contains('%') || raw.contains("percent"))
                                && m.contains_key("percent")
                            {
                                Some("percent".to_string())
                            } else {
                                m.keys().find(|u| raw.contains(u.as_str())).cloned()
                            }
                        })
                        .or_else(|| m.keys().next().cloned())
                }),
        ),
        other => panic!("input '{raw}' produced non-Ratio: {:?}", other),
    }
}

fn evaluate_veto_reason(engine: &Engine, spec: &str, raw: &str) -> String {
    let mut data = IndexMap::new();
    data.insert("r".to_string(), raw.into());
    let now = DateTimeValue::now();
    let resp = engine
        .evaluate(None, spec, Some(&now), data, None, true)
        .unwrap_or_else(|e| panic!("run must complete with veto, not Error for '{raw}': {e}"));
    let rr = resp
        .results
        .get("out")
        .unwrap_or_else(|| panic!("rule 'out' not found"));
    assert!(
        rr.vetoed,
        "expected '{raw}' to be rejected via veto, got value {:?}",
        rr.result()
    );
    rr.veto_reason.clone().expect("veto reason")
}

fn assert_data_veto(msg: &str, field: &str, type_label: &str) {
    assert!(
        msg.contains(&format!("Data {field} [{type_label}]:")),
        "expected Data {field} [{type_label}] veto, got: {msg}"
    );
}

fn assert_data_veto_contains(msg: &str, field: &str, type_label: &str, fact: &str) {
    assert_data_veto(msg, field, type_label);
    assert!(
        msg.contains(fact),
        "expected veto to contain '{fact}', got: {msg}"
    );
}

fn percent_spec() -> &'static str {
    r#"
spec s
data r: ratio
rule out: r
"#
}

// ─── Rejected: bare number (ratio runtime input requires a unit) ───────

#[test]
fn rejects_bare_zero() {
    let mut engine = Engine::new();
    load(&mut engine, percent_spec());
    let msg = evaluate_veto_reason(&engine, "s", "0");
    assert_data_veto_contains(&msg, "r", "ratio", "requires a unit");
}

#[test]
fn rejects_bare_decimal() {
    let mut engine = Engine::new();
    load(&mut engine, percent_spec());
    let msg = evaluate_veto_reason(&engine, "s", "0.5");
    assert_data_veto_contains(&msg, "r", "ratio", "requires a unit");
}

#[test]
fn rejects_bare_negative() {
    let mut engine = Engine::new();
    load(&mut engine, percent_spec());
    let msg = evaluate_veto_reason(&engine, "s", "-0.25");
    assert_data_veto_contains(&msg, "r", "ratio", "requires a unit");
}

// ─── Accepted: percent sigil ──────────────────────────────────────────

#[test]
fn accepts_percent_sigil_integer() {
    let mut engine = Engine::new();
    load(&mut engine, percent_spec());
    let (n, u) = evaluate_rational(&engine, "s", "50%");
    assert_eq!(n, decimal_lit("0.50"));
    assert_eq!(u.as_deref(), Some("percent"));
}

#[test]
fn accepts_percent_sigil_decimal() {
    let mut engine = Engine::new();
    load(&mut engine, percent_spec());
    let (n, u) = evaluate_rational(&engine, "s", "50.5%");
    assert_eq!(n, decimal_lit("0.505"));
    assert_eq!(u.as_deref(), Some("percent"));
}

#[test]
fn accepts_percent_sigil_negative() {
    let mut engine = Engine::new();
    load(&mut engine, percent_spec());
    let (n, u) = evaluate_rational(&engine, "s", "-50%");
    assert_eq!(n, decimal_lit("-0.5"));
    assert_eq!(u.as_deref(), Some("percent"));
}

#[test]
fn comma_thousands_separator_in_percent_vetoes() {
    let mut engine = Engine::new();
    load(&mut engine, percent_spec());
    let mut data = IndexMap::new();
    data.insert("r".to_string(), "5,000%".into());
    let now = DateTimeValue::now();
    let resp = engine
        .evaluate(None, "s", Some(&now), data, None, true)
        .unwrap_or_else(|e| panic!("run failed for 5,000%: {e}"));
    let rr = resp.results.get("out").expect("rule 'out' not found");
    assert!(
        rr.vetoed,
        "5,000% must veto after comma digit separator removal"
    );
}

// ─── Accepted: permille sigil ─────────────────────────────────────────

#[test]
fn accepts_permille_sigil() {
    let mut engine = Engine::new();
    load(&mut engine, percent_spec());
    let (n, u) = evaluate_rational(&engine, "s", "25%%");
    assert_eq!(n, decimal_lit("0.025"));
    assert_eq!(u.as_deref(), Some("permille"));
}

#[test]
fn accepts_permille_sigil_negative() {
    let mut engine = Engine::new();
    load(&mut engine, percent_spec());
    let (n, u) = evaluate_rational(&engine, "s", "-25%%");
    assert_eq!(n, decimal_lit("-0.025"));
    assert_eq!(u.as_deref(), Some("permille"));
}

// ─── Accepted: keyword forms (single and multi-space) ─────────────────

#[test]
fn accepts_percent_keyword_single_space() {
    let mut engine = Engine::new();
    load(&mut engine, percent_spec());
    let (n, u) = evaluate_rational(&engine, "s", "50 percent");
    assert_eq!(n, decimal_lit("0.50"));
    assert_eq!(u.as_deref(), Some("percent"));
}

#[test]
fn accepts_percent_keyword_multi_space() {
    let mut engine = Engine::new();
    load(&mut engine, percent_spec());
    let (n, u) = evaluate_rational(&engine, "s", "50    percent");
    assert_eq!(n, decimal_lit("0.50"));
    assert_eq!(u.as_deref(), Some("percent"));
}

#[test]
fn accepts_percent_keyword_tab() {
    let mut engine = Engine::new();
    load(&mut engine, percent_spec());
    let (n, u) = evaluate_rational(&engine, "s", "50\tpercent");
    assert_eq!(n, decimal_lit("0.50"));
    assert_eq!(u.as_deref(), Some("percent"));
}

#[test]
fn accepts_permille_keyword() {
    let mut engine = Engine::new();
    load(&mut engine, percent_spec());
    let (n, u) = evaluate_rational(&engine, "s", "25 permille");
    assert_eq!(n, decimal_lit("0.025"));
    assert_eq!(u.as_deref(), Some("permille"));
}

// ─── Accepted: user-defined ratio unit ────────────────────────────────

#[test]
fn accepts_user_defined_ratio_unit() {
    let code = r#"
spec s
data r: ratio -> unit basis_points: 10000
rule out: r
"#;
    let mut engine = Engine::new();
    load(&mut engine, code);
    let (n, u) = evaluate_rational(&engine, "s", "500 basis_points");
    assert_eq!(n, decimal_lit("0.05"));
    assert_eq!(u.as_deref(), Some("basis_points"));
}

// ─── Cross-form equivalence ───────────────────────────────────────────

#[test]
fn sigil_and_keyword_produce_same_value() {
    let mut engine = Engine::new();
    load(&mut engine, percent_spec());
    let sigil = evaluate_rational(&engine, "s", "50%");
    let keyword = evaluate_rational(&engine, "s", "50 percent");
    assert_eq!(sigil, keyword);
}

#[test]
fn permille_sigil_and_keyword_produce_same_value() {
    let mut engine = Engine::new();
    load(&mut engine, percent_spec());
    let sigil = evaluate_rational(&engine, "s", "25%%");
    let keyword = evaluate_rational(&engine, "s", "25 permille");
    assert_eq!(sigil, keyword);
}

// ─── Rejected: empty / whitespace ─────────────────────────────────────

#[test]
fn rejects_empty() {
    let mut engine = Engine::new();
    load(&mut engine, percent_spec());
    let msg = evaluate_veto_reason(&engine, "s", "");
    assert!(
        msg.contains("Data r [ratio] cannot be empty."),
        "expected field-named empty ratio message, got: {msg}"
    );
    assert!(
        !msg.contains("Ratio value cannot be empty"),
        "must not dump FromStr grammar, got: {msg}"
    );
}

#[test]
fn rejects_whitespace_only() {
    let mut engine = Engine::new();
    load(&mut engine, percent_spec());
    let msg = evaluate_veto_reason(&engine, "s", "   ");
    assert!(
        msg.contains("Data r [ratio] cannot be empty."),
        "expected field-named empty ratio message, got: {msg}"
    );
}

// ─── Rejected: sigil without number ───────────────────────────────────

#[test]
fn rejects_bare_percent_sigil() {
    let mut engine = Engine::new();
    load(&mut engine, percent_spec());
    let msg = evaluate_veto_reason(&engine, "s", "%");
    assert_data_veto_contains(&msg, "r", "ratio", "Invalid ratio");
}

#[test]
fn rejects_bare_permille_sigil() {
    let mut engine = Engine::new();
    load(&mut engine, percent_spec());
    let msg = evaluate_veto_reason(&engine, "s", "%%");
    assert_data_veto_contains(&msg, "r", "ratio", "Invalid ratio");
}

#[test]
fn rejects_sigil_before_number() {
    let mut engine = Engine::new();
    load(&mut engine, percent_spec());
    let percent = evaluate_veto_reason(&engine, "s", "%5");
    assert_data_veto_contains(&percent, "r", "ratio", "Invalid ratio");
    let permille = evaluate_veto_reason(&engine, "s", "%%5");
    assert_data_veto_contains(&permille, "r", "ratio", "Invalid ratio");
}

// ─── Rejected: sigil with separator (strict glue rule) ────────────────

#[test]
fn rejects_percent_sigil_with_space() {
    let mut engine = Engine::new();
    load(&mut engine, percent_spec());
    let msg = evaluate_veto_reason(&engine, "s", "5 %");
    assert_data_veto_contains(&msg, "r", "ratio", "Invalid ratio");
}

#[test]
fn rejects_permille_sigil_with_space() {
    let mut engine = Engine::new();
    load(&mut engine, percent_spec());
    let msg = evaluate_veto_reason(&engine, "s", "5  %%");
    assert_data_veto_contains(&msg, "r", "ratio", "Invalid ratio");
}

// ─── Rejected: digit after sigil / mixed sigil-keyword ────────────────

#[test]
fn rejects_digit_after_percent_sigil() {
    let mut engine = Engine::new();
    load(&mut engine, percent_spec());
    let msg = evaluate_veto_reason(&engine, "s", "5%5");
    assert_data_veto_contains(&msg, "r", "ratio", "Invalid ratio");
}

#[test]
fn rejects_digit_after_permille_sigil() {
    let mut engine = Engine::new();
    load(&mut engine, percent_spec());
    let msg = evaluate_veto_reason(&engine, "s", "5%%5");
    assert_data_veto_contains(&msg, "r", "ratio", "Invalid ratio");
}

#[test]
fn rejects_sigil_glued_to_keyword() {
    let mut engine = Engine::new();
    load(&mut engine, percent_spec());
    let msg = evaluate_veto_reason(&engine, "s", "5%percent");
    assert_data_veto_contains(&msg, "r", "ratio", "Invalid ratio");
}

// ─── Rejected: trailing junk ──────────────────────────────────────────

#[test]
fn rejects_trailing_token_after_keyword() {
    let mut engine = Engine::new();
    load(&mut engine, percent_spec());
    let msg = evaluate_veto_reason(&engine, "s", "50 percent extra");
    assert_data_veto_contains(&msg, "r", "ratio", "Invalid ratio");
}

// ─── Rejected: unknown unit name ──────────────────────────────────────

#[test]
fn rejects_unknown_unit_name() {
    let mut engine = Engine::new();
    load(&mut engine, percent_spec());
    let msg = evaluate_veto_reason(&engine, "s", "50 fictional");
    assert_data_veto_contains(&msg, "r", "ratio", "Unknown unit");
    assert_data_veto_contains(&msg, "r", "ratio", "fictional");
}

// ─── Measure-side regression: `%` is no longer accepted by NumberWithUnit ───

#[test]
fn measure_type_rejects_percent_sigil() {
    let code = r#"
spec s
data r: measure -> unit eur: 1
rule out: r
"#;
    let mut engine = Engine::new();
    load(&mut engine, code);
    let msg = evaluate_veto_reason(&engine, "s", "5%");
    assert_data_veto_contains(&msg, "r", "measure", "must include a unit");
    assert_data_veto_contains(&msg, "r", "measure", "eur");
    assert!(
        !msg.contains("Unknown unit 'percent'"),
        "Measure path must not leak a 'percent' unit lookup, got: {msg}"
    );
}

#[test]
fn measure_type_rejects_permille_sigil() {
    let code = r#"
spec s
data r: measure -> unit eur: 1
rule out: r
"#;
    let mut engine = Engine::new();
    load(&mut engine, code);
    let msg = evaluate_veto_reason(&engine, "s", "5%%");
    assert_data_veto_contains(&msg, "r", "measure", "must include a unit");
    assert_data_veto_contains(&msg, "r", "measure", "eur");
    assert!(
        !msg.contains("Unknown unit 'permille'"),
        "Measure path must not leak a 'permille' unit lookup, got: {msg}"
    );
}

#[test]
fn custom_ratio_unit_result_displays_magnitude_in_that_unit() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        "spec bps\ndata r: ratio -> unit basis_points: 10000\nrule out: r",
    );
    let mut data = IndexMap::new();
    data.insert("r".to_string(), "500 basis_points".into());
    let now = DateTimeValue::now();
    let response = engine
        .evaluate(None, "bps", Some(&now), data, None, false)
        .expect("evaluate bps");
    let result = response.results["out"]
        .result
        .as_ref()
        .expect("out has a value");
    assert_eq!(result.result.as_deref(), Some("500 basis_points"));
    assert_eq!(
        result.ratio.as_ref().expect("ratio map")["basis_points"],
        decimal_lit("500")
    );
}

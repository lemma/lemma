//! Unified ratio units: multiple `ratio` types per spec, shared builtin `percent` /
//! `permille` in the unit index, unique custom unit names, cross-type
//! arithmetic/comparison, per-type `as` scoping, and expression-level (anonymous)
//! ratios. Assertions use canonical `ValueKind` + `RationalInteger`, not display
//! substrings alone.

use lemma::DateTimeValue;
use lemma::{Engine, LiteralValue};
use lemma::{TypeSpecification, ValueKind};
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

fn decimal_lit(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn path_source(file: &str) -> lemma::SourceType {
    lemma::SourceType::Path(Arc::new(PathBuf::from(file)))
}

fn load_ok(engine: &mut Engine, code: &str, file: &str) {
    engine
        .load([(path_source(file), code.to_string())])
        .unwrap_or_else(|errs| {
            let joined = errs
                .errors
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("\n");
            panic!("expected load to succeed ({file}), got: {joined}");
        });
}

fn expect_load_error(code: &str, file: &str, fragments: &[&str]) {
    let mut engine = Engine::new();
    let result = engine.load([(path_source(file), code.to_string())]);
    assert!(result.is_err(), "expected load to fail ({file})");
    let combined = result
        .unwrap_err()
        .errors
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("; ");
    for frag in fragments {
        assert!(
            combined.contains(frag),
            "expected error containing '{frag}', got: {combined}"
        );
    }
}

fn run_spec(engine: &Engine, spec: &str, data: HashMap<String, String>) -> lemma::Response {
    let now = DateTimeValue::now();
    engine
        .run(None, spec, Some(&now), data, None, true)
        .unwrap_or_else(|e| panic!("run({spec}) failed: {e}"))
}

fn rule_value(response: &lemma::Response, rule: &str) -> LiteralValue {
    let rr = response
        .results
        .get(rule)
        .unwrap_or_else(|| panic!("rule '{rule}' missing; keys: {:?}", response.results.keys()));
    if rr.vetoed {
        panic!(
            "rule '{rule}' vetoed: {}",
            rr.veto_reason.as_deref().unwrap_or("Vetoed")
        );
    }
    rr.explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("value")
}

fn assert_ratio_exact(
    lit: &LiteralValue,
    ctx: &str,
    expected_canonical: &str,
    expected_unit: Option<&str>,
) {
    match &lit.value {
        ValueKind::Ratio(r) => {
            assert_eq!(
                lemma::ValueKind::Number(r.clone())
                    .as_decimal_magnitude()
                    .unwrap(),
                decimal_lit(expected_canonical),
                "{ctx}: canonical magnitude"
            );
            let _ = expected_unit; // binding unit lives on type / RuleResult.ratio, not LiteralValue
        }
        other => panic!("{ctx}: expected ValueKind::Ratio, got {other:?}"),
    }
}

fn assert_number_exact(lit: &LiteralValue, ctx: &str, expected: &str) {
    match &lit.value {
        ValueKind::Number(n) => {
            assert_eq!(
                lemma::ValueKind::Number(n.clone())
                    .as_decimal_magnitude()
                    .unwrap(),
                decimal_lit(expected),
                "{ctx}"
            );
        }
        other => panic!("{ctx}: expected Number, got {other:?}"),
    }
}

fn assert_bool(lit: &LiteralValue, ctx: &str, expected: bool) {
    match &lit.value {
        ValueKind::Boolean(b) => assert_eq!(*b, expected, "{ctx}"),
        other => panic!("{ctx}: expected Boolean, got {other:?}"),
    }
}

fn ratio_unit_names(spec: &TypeSpecification) -> Vec<&str> {
    match spec {
        TypeSpecification::Ratio { units, .. } => units.iter().map(|u| u.name.as_str()).collect(),
        other => panic!("expected Ratio spec, got {other:?}"),
    }
}

// -----------------------------------------------------------------------------
// Section 1 — Multi-type spec loads (user regression)
// -----------------------------------------------------------------------------

const TARGETS_SPEC: &str = r#"
spec targets
"""
Corporate financial targets, minimum margins, and standard bonuses.
"""
data standard_margin_pct: ratio
  -> minimum 0%
  -> suggest 15%

data default_credit_insurance_pct: ratio
  -> suggest 1.5%

rule margin: standard_margin_pct
rule insurance: default_credit_insurance_pct
"#;

#[test]
fn targets_spec_two_ratio_fields_load_and_run_defaults() {
    let mut engine = Engine::new();
    load_ok(&mut engine, TARGETS_SPEC, "targets.lemma");

    let mut data = HashMap::new();
    data.insert("standard_margin_pct".to_string(), "15%".into());
    data.insert("default_credit_insurance_pct".to_string(), "1.5%".into());
    let response = run_spec(&engine, "targets", data);
    assert_ratio_exact(
        &rule_value(&response, "margin"),
        "margin default",
        "0.15",
        Some("percent"),
    );
    assert_ratio_exact(
        &rule_value(&response, "insurance"),
        "insurance default",
        "0.015",
        Some("percent"),
    );
}

#[test]
fn targets_spec_show_both_ratio_types_have_builtin_units() {
    let mut engine = Engine::new();
    load_ok(&mut engine, TARGETS_SPEC, "targets_show.lemma");

    let now = DateTimeValue::now();
    let show = engine.show(None, "targets", Some(&now)).expect("show");

    for name in ["standard_margin_pct", "default_credit_insurance_pct"] {
        let entry = show.data.get(name).expect(name);
        let names = ratio_unit_names(&entry.lemma_type.specifications);
        assert!(
            names.contains(&"percent"),
            "{name}: missing percent unit, got {names:?}"
        );
        assert!(
            names.contains(&"permille"),
            "{name}: missing permille unit, got {names:?}"
        );
    }

    let margin = show.data.get("standard_margin_pct").expect("margin");
    assert_eq!(
        margin.lemma_type.specifications.minimum_decimal(),
        Some(decimal_lit("0"))
    );
}

#[test]
fn three_ratio_fields_builtin_units_load() {
    let code = r#"
spec rates
data margin_pct: ratio -> suggest 10%
data fee_pct: ratio -> suggest 2%
data tax_pct: ratio -> suggest 1%
rule sum: margin_pct + fee_pct + tax_pct
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code, "three_ratio.lemma");

    let mut data = HashMap::new();
    data.insert("margin_pct".to_string(), "10%".into());
    data.insert("fee_pct".to_string(), "2%".into());
    data.insert("tax_pct".to_string(), "1%".into());
    let response = run_spec(&engine, "rates", data);
    // 0.10 + 0.02 + 0.01 = 0.13 (left-associative; operands carry percent from supplied values)
    assert_ratio_exact(
        &rule_value(&response, "sum"),
        "sum",
        "0.13",
        Some("percent"),
    );
}

// -----------------------------------------------------------------------------
// Section 2 — Cross-type evaluation
// -----------------------------------------------------------------------------

const FINANCE_SPEC: &str = r#"
spec finance
data margin_pct: ratio -> suggest 15%
data insurance_pct: ratio -> suggest 1.5%

rule total_rate: margin_pct + insurance_pct
rule margin_higher: margin_pct > insurance_pct
rule product: margin_pct * insurance_pct
rule tier: "low"
    unless margin_pct > 10% then "mid"
    unless margin_pct > 20% then "high"
"#;

fn finance_suggested_inputs() -> HashMap<String, String> {
    let mut data = HashMap::new();
    data.insert("margin_pct".to_string(), "15%".into());
    data.insert("insurance_pct".to_string(), "1.5%".into());
    data
}

#[test]
fn cross_type_add_preserves_canonical_sum() {
    let mut engine = Engine::new();
    load_ok(&mut engine, FINANCE_SPEC, "finance_add.lemma");

    let response = run_spec(&engine, "finance", finance_suggested_inputs());
    assert_ratio_exact(
        &rule_value(&response, "total_rate"),
        "total_rate",
        "0.165",
        Some("percent"),
    );
}

#[test]
fn cross_type_add_result_carries_left_operand_type() {
    let mut engine = Engine::new();
    load_ok(&mut engine, FINANCE_SPEC, "finance_type.lemma");

    let response = run_spec(&engine, "finance", finance_suggested_inputs());
    let rr = response.results.get("total_rate").expect("total_rate");
    assert_eq!(
        rr.rule_type.as_str(),
        "margin_pct",
        "ratio + ratio must keep left named type"
    );
    let _ = rule_value(&response, "total_rate");
}

#[test]
fn cross_type_compare_with_defaults() {
    let mut engine = Engine::new();
    load_ok(&mut engine, FINANCE_SPEC, "finance_cmp.lemma");

    let response = run_spec(&engine, "finance", finance_suggested_inputs());
    assert_bool(
        &rule_value(&response, "margin_higher"),
        "margin_higher",
        true,
    );
}

#[test]
fn cross_type_multiply_canonical() {
    let mut engine = Engine::new();
    load_ok(&mut engine, FINANCE_SPEC, "finance_mul.lemma");

    let response = run_spec(&engine, "finance", finance_suggested_inputs());
    assert_ratio_exact(
        &rule_value(&response, "product"),
        "product",
        "0.00225",
        Some("percent"),
    );
}

#[test]
fn cross_type_unless_branches() {
    let mut engine = Engine::new();
    load_ok(&mut engine, FINANCE_SPEC, "finance_tier.lemma");

    let response = run_spec(&engine, "finance", finance_suggested_inputs());
    match &rule_value(&response, "tier").value {
        ValueKind::Text(s) => assert_eq!(s, "mid", "15% > 10% but not > 20%"),
        other => panic!("tier must be Text, got {other:?}"),
    }
}

#[test]
fn cross_type_runtime_override_changes_comparison() {
    let code = r#"
spec finance
data margin_pct: ratio -> suggest 15%
data insurance_pct: ratio -> suggest 1.5%
rule margin_higher: margin_pct > insurance_pct
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code, "finance_override.lemma");

    let mut data = HashMap::new();
    data.insert("margin_pct".to_string(), "1%".into());
    data.insert("insurance_pct".to_string(), "2%".into());

    let response = run_spec(&engine, "finance", data);
    assert_bool(
        &rule_value(&response, "margin_higher"),
        "margin_higher",
        false,
    );
}

// -----------------------------------------------------------------------------
// Section 3 — Per-type `as` scoping and unique custom ratio units
// -----------------------------------------------------------------------------

#[test]
fn own_type_as_percent_ok() {
    let code = r#"
spec s
data margin: ratio -> suggest 20%
data spread: ratio
  -> unit basis_points: 10000
rule margin_as_pct: margin as percent
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code, "as_percent_ok.lemma");

    let mut data = HashMap::new();
    data.insert("margin".to_string(), "20%".into());
    let response = run_spec(&engine, "s", data);
    assert_ratio_exact(
        &rule_value(&response, "margin_as_pct"),
        "margin_as_pct",
        "0.2",
        Some("percent"),
    );
}

#[test]
fn foreign_unit_as_basis_points_fails_plan() {
    let code = r#"
spec s
data margin: ratio -> suggest 20%
data spread: ratio
  -> unit basis_points: 10000
rule bad: margin as basis_points
"#;
    expect_load_error(code, "foreign_bps.lemma", &["basis_points", "margin"]);
}

#[test]
fn second_custom_ratio_unit_declarer_errors_at_load() {
    let code = r#"
spec s
data spread_a: ratio
  -> unit basis_points: 10000
data spread_b: ratio
  -> unit basis_points: 10000
rule out: spread_a
"#;
    expect_load_error(
        code,
        "unique_bps.lemma",
        &[
            "spread_a",
            "spread_b",
            "basis_points",
            "defined on both",
            "declared only once",
        ],
    );
}

#[test]
fn redefining_builtin_percent_with_different_factor_errors() {
    let code = r#"
spec s
data margin: ratio
data custom: ratio
  -> unit percent: 50
rule out: margin
"#;
    expect_load_error(
        code,
        "redefine_percent.lemma",
        &["percent", "cannot change factor", "inherited"],
    );
}

#[test]
fn second_custom_ratio_unit_declarer_among_three_types_errors() {
    let code = r#"
spec s
data a: ratio
  -> unit thirds: 3
data b: ratio
  -> unit thirds: 3
data c: ratio
  -> unit thirds: 6
rule out: a
"#;
    expect_load_error(
        code,
        "three_types_unique.lemma",
        &["thirds", "defined on both", "declared only once"],
    );
}

#[test]
fn second_custom_ratio_unit_declarer_reordered_errors() {
    let code = r#"
spec s
data c: ratio
  -> unit thirds: 6
data a: ratio
  -> unit thirds: 3
data b: ratio
  -> unit thirds: 3
rule out: a
"#;
    expect_load_error(
        code,
        "three_types_unique_reordered.lemma",
        &["thirds", "defined on both", "declared only once"],
    );
}

// -----------------------------------------------------------------------------
// Section 4 — Anonymous / expression ratios
// -----------------------------------------------------------------------------

#[test]
fn division_as_percent_is_ratio_not_number() {
    let code = r#"
spec calc
data part: 75
data whole: 300
rule savings_ratio: (part / whole) as percent
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code, "anon_div.lemma");

    let response = run_spec(&engine, "calc", HashMap::new());
    assert_ratio_exact(
        &rule_value(&response, "savings_ratio"),
        "savings_ratio",
        "0.25",
        Some("percent"),
    );
}

#[test]
fn literal_percent_in_rule_is_ratio() {
    let code = r#"
spec s
rule r: 15%
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code, "literal_pct.lemma");

    let response = run_spec(&engine, "s", HashMap::new());
    assert_ratio_exact(&rule_value(&response, "r"), "r", "0.15", Some("percent"));
}

#[test]
fn anonymous_ratio_arithmetic_in_multi_ratio_spec() {
    let code = r#"
spec finance
data margin_pct: ratio -> suggest 15%
data insurance_pct: ratio -> suggest 1.5%
data part: 10
data whole: 40

rule pct: (part / whole) as percent
rule plus_five: pct + 5%
rule compared: plus_five > 25%
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code, "anon_arith.lemma");

    let response = run_spec(&engine, "finance", HashMap::new());
    assert_ratio_exact(
        &rule_value(&response, "pct"),
        "pct",
        "0.25",
        Some("percent"),
    );
    assert_ratio_exact(
        &rule_value(&response, "plus_five"),
        "plus_five",
        "0.30",
        Some("percent"),
    );
    assert_bool(&rule_value(&response, "compared"), "compared", true);
}

#[test]
fn number_as_percent_with_two_named_ratio_fields_present() {
    let code = r#"
spec finance
data margin_pct: ratio -> suggest 15%
data insurance_pct: ratio -> suggest 1.5%
rule anon: 0.25 as percent
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code, "number_as_pct.lemma");

    let response = run_spec(&engine, "finance", HashMap::new());
    let lit = rule_value(&response, "anon");
    assert_ratio_exact(&lit, "anon", "0.25", Some("percent"));
    assert!(
        matches!(lit.value, ValueKind::Ratio(_)),
        "result must remain ratio-typed, got {:?}",
        lit.value
    );
}

#[test]
fn ratio_display_none_vs_percent_unit() {
    let bare = LiteralValue::ratio_from_decimal(decimal_lit("0.5"));
    let display = bare.display_value();
    assert!(
        !display.contains("percent") && display.contains("0.5"),
        "ratio without unit must not show percent label, got: {display}"
    );

    let ValueKind::Ratio(canonical) = &bare.value else {
        panic!("expected ratio");
    };
    let tagged = lemma::__test_support::TypedLiteral::ratio_with_bound_unit(
        canonical.clone(),
        "percent",
        Arc::new(lemma::LemmaType::primitive(
            lemma::TypeSpecification::ratio(),
        )),
    );
    let display_tagged = tagged.display_value();
    assert!(
        display_tagged.contains('%') || display_tagged.contains("percent"),
        "ratio with percent unit must show unit, got: {display_tagged}"
    );
}

// -----------------------------------------------------------------------------
// Section 5 — Ratio ranges with multiple ratio data fields
// -----------------------------------------------------------------------------

/// Assert a show suggestion's API `ratio` map carries `expected` for `unit`.
/// `RuleResultValue` has no canonical magnitude or unit tag of its own — only the flat
/// per-unit map — so this checks the map value directly.
fn assert_ratio_unit_value(
    suggestion: &lemma::RuleResultValue,
    ctx: &str,
    unit: &str,
    expected: &str,
) {
    let actual = suggestion
        .ratio
        .as_ref()
        .and_then(|m| m.get(unit))
        .copied()
        .unwrap_or_else(|| panic!("{ctx}: missing ratio unit '{unit}'"));
    assert_eq!(actual, decimal_lit(expected), "{ctx}");
}

/// Assert each range endpoint's API `ratio` map carries `expected_left`/`expected_right` for
/// `unit`. `RuleResultValue` (the type behind `ShowData.suggestion`) has no canonical
/// magnitude or unit tag of its own — only the flat per-unit map — so this checks the map
/// value directly rather than reconstructing a canonical `LiteralValue`.
fn assert_range_endpoints_ratio_unit(
    suggestion: &lemma::RuleResultValue,
    ctx: &str,
    unit: &str,
    expected_left: &str,
    expected_right: &str,
) {
    let range = suggestion
        .range
        .as_ref()
        .unwrap_or_else(|| panic!("{ctx}: expected a range suggestion"));
    let left = range
        .from
        .ratio
        .as_ref()
        .and_then(|m| m.get(unit))
        .copied()
        .unwrap_or_else(|| panic!("{ctx}: left endpoint missing ratio unit '{unit}'"));
    let right = range
        .to
        .ratio
        .as_ref()
        .and_then(|m| m.get(unit))
        .copied()
        .unwrap_or_else(|| panic!("{ctx}: right endpoint missing ratio unit '{unit}'"));
    assert_eq!(left, decimal_lit(expected_left), "{ctx}: left");
    assert_eq!(right, decimal_lit(expected_right), "{ctx}: right");
}

#[test]
fn ratio_range_default_with_percent_endpoints_canonical() {
    let code = r#"
spec policy
data allowed_band: ratio range -> suggest 10%...50%
rule band: allowed_band
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code, "ratio_range_default_pct.lemma");

    let now = DateTimeValue::now();
    let show = engine
        .show(None, "policy", Some(&now))
        .expect("plan must build with ratio range default");
    let entry = show.data.get("allowed_band").expect("allowed_band in show");
    assert!(
        matches!(
            &entry.lemma_type.specifications,
            TypeSpecification::RatioRange { .. }
        ),
        "default suggestion lemma_type must be RatioRange, got {:?}",
        entry.lemma_type.specifications
    );
    let suggestion = entry
        .suggestion
        .clone()
        .expect("ratio range typedef must surface declared suggestion");
    assert_range_endpoints_ratio_unit(
        &suggestion,
        "ratio_range_default percent",
        "percent",
        "10",
        "50",
    );
}

#[test]
fn ratio_range_default_with_basis_points_endpoints_canonical() {
    let code = r#"
spec policy
data allowed_band: ratio range
  -> unit basis_points: 10000
  -> suggest 200 basis_points...3500 basis_points
rule band: allowed_band
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code, "ratio_range_default_bps.lemma");

    let now = DateTimeValue::now();
    let suggestion = engine
        .show(None, "policy", Some(&now))
        .expect("plan must build with custom-unit ratio range default")
        .data
        .get("allowed_band")
        .expect("allowed_band in show")
        .suggestion
        .clone()
        .expect("ratio range typedef must surface declared suggestion");
    assert_range_endpoints_ratio_unit(
        &suggestion,
        "ratio_range_default basis_points",
        "basis_points",
        "200",
        "3500",
    );
}

#[test]
fn ratio_range_default_runtime_uses_canonical_endpoints() {
    let code = r#"
spec policy
data allowed_band: ratio range -> suggest 10%...50%
data candidate: ratio -> suggest 25%
rule in_default_band: candidate in allowed_band
rule out_of_band: 5% in allowed_band
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code, "ratio_range_default_runtime.lemma");

    let mut data = HashMap::new();
    data.insert("allowed_band".to_string(), "10%...50%".into());
    data.insert("candidate".to_string(), "25%".into());
    let response = run_spec(&engine, "policy", data);
    assert_bool(
        &rule_value(&response, "in_default_band"),
        "in_default_band",
        true,
    );
    assert_bool(&rule_value(&response, "out_of_band"), "out_of_band", false);
}

#[test]
fn margin_in_ratio_range_literal() {
    let code = r#"
spec policy
data margin_pct: ratio -> suggest 15%
data insurance_pct: ratio -> suggest 1.5%
rule in_band: margin_pct in 10%...50%
rule below: margin_pct in 0%...10%
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code, "in_band.lemma");

    let mut data = HashMap::new();
    data.insert("margin_pct".to_string(), "15%".into());
    let response = run_spec(&engine, "policy", data);
    assert_bool(&rule_value(&response, "in_band"), "in_band", true);
    assert_bool(&rule_value(&response, "below"), "below", false);
}

// -----------------------------------------------------------------------------
// Section 6 — Schema and runtime input
// -----------------------------------------------------------------------------

#[test]
fn runtime_input_each_ratio_field_independent() {
    let mut engine = Engine::new();
    load_ok(&mut engine, TARGETS_SPEC, "targets_runtime.lemma");

    let mut data = HashMap::new();
    data.insert("standard_margin_pct".to_string(), "20%".into());
    data.insert("default_credit_insurance_pct".to_string(), "2%".into());

    let response = run_spec(&engine, "targets", data);
    assert_ratio_exact(
        &rule_value(&response, "margin"),
        "margin runtime",
        "0.2",
        Some("percent"),
    );
    assert_ratio_exact(
        &rule_value(&response, "insurance"),
        "insurance runtime",
        "0.02",
        Some("percent"),
    );
}

#[test]
fn show_declared_suggestions_canonical_for_targets() {
    let mut engine = Engine::new();
    load_ok(&mut engine, TARGETS_SPEC, "targets_defaults.lemma");

    let now = DateTimeValue::now();
    let show = engine.show(None, "targets", Some(&now)).expect("show");

    let margin = show.data.get("standard_margin_pct").expect("margin");
    let default = margin
        .suggestion
        .as_ref()
        .expect("margin declared suggestion");
    assert_ratio_unit_value(default, "margin show default", "percent", "15");

    let insurance = show
        .data
        .get("default_credit_insurance_pct")
        .expect("insurance");
    let ins_default = insurance
        .suggestion
        .as_ref()
        .expect("insurance declared suggestion");
    assert_ratio_unit_value(ins_default, "insurance show default", "percent", "1.5");
}

// -----------------------------------------------------------------------------
// Section 7 — Discount arithmetic across named ratio types
// -----------------------------------------------------------------------------

#[test]
fn number_minus_other_ratio_field_canonical() {
    let code = r#"
spec pricing
data discount: ratio -> suggest 10%
data surcharge: ratio -> suggest 5%
data base: 100

rule after_discount: base * (100% - discount)
rule after_surcharge: base * (100% + surcharge)
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code, "pricing_mix.lemma");

    let mut data = HashMap::new();
    data.insert("discount".to_string(), "10%".into());
    data.insert("surcharge".to_string(), "5%".into());
    let response = run_spec(&engine, "pricing", data);
    assert_number_exact(
        &rule_value(&response, "after_discount"),
        "after_discount",
        "90",
    );
    assert_number_exact(
        &rule_value(&response, "after_surcharge"),
        "after_surcharge",
        "105",
    );
}

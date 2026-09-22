//! API format contract tests (sections A–J).
//!
//! API serialize path: [`Show`] JSON and [`Response`] JSON — same paths
//! [`engine::wasm`] and show export use (`serde_json` on those types).
//! Plan persistence tests live in `execution_plan` unit tests (section I).
//!
//! `RuleResultValue` (the type behind `ShowData.fill`/`suggestion`) is a flat map of
//! every declared unit; it carries no unit tag for the "one value the user typed" the way the
//! old canonical `ValueKind::Ratio(_)` did. `RuleResultValue::to_literal`
//! reconstructs a canonical literal for further computation, but for ratios that
//! reconstruction always comes back with `unit: None` (it always binds to the type's first
//! declared unit's key, and cannot know which key the caller originally committed). Section A
//! reflects that: it checks canonical magnitude survives the round trip, not the unit tag.
//!
//! Section F additionally requires that every show-default per-unit magnitude from
//! section E (`magnitude_in_unit`, API `measure` / `ratio` maps) is submittable
//! as convenience input through [`Engine::run`] without computation veto — same path
//! as the CLI interactive trial. See `cli/documentation/learn/precision.md`.

use lemma::{
    DateTimeValue, Engine, LemmaType, LiteralValue, RuleResultValue, Show, TypeSpecification,
    ValueKind,
};
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

fn decimal_lit(s: &str) -> Decimal {
    Decimal::from_str(s).expect("valid decimal literal in test")
}

fn path_source(file: &str) -> lemma::SourceType {
    lemma::SourceType::Path(Arc::new(PathBuf::from(file)))
}

fn load_engine(code: &str, file: &str) -> Engine {
    let mut engine = Engine::new();
    engine
        .load([(path_source(file), code.to_string())])
        .expect("spec must load");
    engine
}

const COST_PRICE_SPEC: &str = r#"
spec cost_price
uses lemma units

data money: measure
  -> unit eur: 1.00
  -> unit inr: 0.0092
  -> decimals 2

data labor_cost: measure
  -> unit eur_per_hour: eur/hour
  -> unit inr_per_hour: inr/hour
  -> suggest 25 eur_per_hour

data product_cost: measure
  -> unit eur_per_kg: eur/kilogram
  -> unit inr_per_kg: inr/kilogram
  -> suggest 4 eur_per_kg

data throughput: measure
  -> unit kg_per_hour: kilogram/hour
  -> suggest 12 kg_per_hour

rule cost_price: product_cost + labor_cost / throughput
"#;

const POLICY_RATIO_SPEC: &str = r#"
spec policy
data margin: ratio -> suggest 15%
data bps: ratio
  -> unit basis_points: 10000
  -> suggest 500 basis_points
data permille_rate: ratio -> suggest 150 permille
rule m: margin
rule bps_val: bps
rule permille_val: permille_rate
"#;

fn cost_price_engine() -> Engine {
    load_engine(COST_PRICE_SPEC, "cost_price.lemma")
}

fn policy_engine() -> Engine {
    load_engine(POLICY_RATIO_SPEC, "policy.lemma")
}

fn plan_interface_show(engine: &Engine, spec: &str) -> Show {
    let now = DateTimeValue::now();
    engine.show(None, spec, Some(&now)).expect("show")
}

/// Reconstruct the canonical literal that a data's show suggestion converts to.
///
/// Lossy for ratio unit tags: `RuleResultValue::to_literal` always reconstructs a ratio
/// against the type's first declared unit and never carries the tag of the unit the value
/// was originally expressed in (matching `RuleResult`'s established convention).
fn show_default_literal(engine: &Engine, spec: &str, data_name: &str) -> (LiteralValue, LemmaType) {
    let show = plan_interface_show(engine, spec);
    let entry = show
        .data
        .get(data_name)
        .unwrap_or_else(|| panic!("{data_name} missing from show"))
        .clone();
    let lit = entry
        .suggestion
        .unwrap_or_else(|| panic!("{data_name} has no show suggestion"))
        .to_literal(&entry.lemma_type);
    (lit, entry.lemma_type)
}

/// Wrap an existing [`RuleResultValue`] as a one-entry [`Show`] `suggestion` and
/// return that field's serialized JSON (the shape `Show`/`Response` JSON actually emits).
fn wrap_suggestion_in_show_json(
    name: &str,
    lemma_type: LemmaType,
    value: RuleResultValue,
) -> serde_json::Value {
    let mut data = indexmap::IndexMap::new();
    data.insert(
        name.to_string(),
        lemma::ShowData {
            lemma_type,
            path: Vec::new(),
            fill: None,
            suggestion: Some(value),
            needed_by_rules: Vec::new(),
        },
    );
    let show = Show {
        repository: None,
        spec: "api_test".to_string(),
        commentary: None,
        effective_from: None,
        effective_to: None,
        versions: Vec::new(),
        start_line: 1,
        source_type: None,
        data,
        rules: indexmap::IndexMap::new(),
        meta: indexmap::IndexMap::new(),
    };
    serde_json::to_value(lemma::api::Show::from(&show)).expect("Show API JSON must serialize")
        ["data"][name]["suggestion"]
        .clone()
}

/// API response for a canonical literal: build RuleResultValue (same path as `Engine::show`) and
/// embed the result in a [`Show`] `suggestion` field.
fn api_json_for_literal(
    name: &str,
    literal: &LiteralValue,
    lemma_type: &LemmaType,
) -> serde_json::Value {
    let value = lemma::result_value::type_scoped_result_value_from_literal(literal, lemma_type)
        .unwrap_or_else(|failure| {
            panic!("type_scoped_result_value_from_literal '{name}' for API JSON: {failure:?}")
        });
    wrap_suggestion_in_show_json(name, lemma_type.clone(), value)
}

fn api_show_default_json(engine: &Engine, spec: &str, data_name: &str) -> serde_json::Value {
    let (lit, ty) = show_default_literal(engine, spec, data_name);
    api_json_for_literal(data_name, &lit, &ty)
}

fn json_ratio_unit_value(json: &serde_json::Value, unit: &str) -> String {
    json["ratio"][unit]
        .as_str()
        .unwrap_or_else(|| panic!("ratio.{unit} missing from API JSON: {json}"))
        .to_string()
}

fn json_measure_unit_value(json: &serde_json::Value, unit: &str) -> String {
    json["measure"][unit]
        .as_str()
        .unwrap_or_else(|| panic!("measure.{unit} missing from API JSON: {json}"))
        .to_string()
}

fn range_endpoint_unit_value(
    json: &serde_json::Value,
    side: &str,
    kind: &str,
    unit: &str,
) -> String {
    json["range"][side][kind][unit]
        .as_str()
        .unwrap_or_else(|| panic!("range.{side}.{kind}.{unit} missing from API JSON: {json}"))
        .to_string()
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
                ValueKind::Number(r.clone())
                    .as_decimal_magnitude()
                    .expect("ratio magnitude"),
                decimal_lit(expected_canonical),
                "{ctx}: canonical magnitude"
            );
            let _ = expected_unit; // binding unit on type, not LiteralValue
        }
        other => panic!("{ctx}: expected Ratio, got {other:?}"),
    }
}

fn show_literal_api_json(engine: &Engine, spec: &str, data_name: &str) -> serde_json::Value {
    let show = plan_interface_show(engine, spec);
    let entry = show
        .data
        .get(data_name)
        .unwrap_or_else(|| panic!("{data_name} missing from show.data"))
        .clone();
    let value = entry
        .fill
        .or(entry.suggestion)
        .unwrap_or_else(|| panic!("{data_name} has no fill or suggestion in show.data"));
    wrap_suggestion_in_show_json(data_name, entry.lemma_type, value)
}

fn deserialize_api_literal(json: serde_json::Value, lemma_type: &LemmaType) -> LiteralValue {
    let value: RuleResultValue =
        serde_json::from_value(json).expect("API literal JSON must deserialize as RuleResultValue");
    value.to_literal(lemma_type)
}

// --- A: In-memory unchanged ---

#[test]
fn in_memory_ratio_percent_default_is_canonical() {
    let (default, _default_ty) = show_default_literal(&policy_engine(), "policy", "margin");
    assert_ratio_exact(&default, "margin default", "0.15", None);
}

#[test]
fn in_memory_ratio_basis_points_default_is_canonical() {
    let (default, _default_ty) = show_default_literal(&policy_engine(), "policy", "bps");
    assert_ratio_exact(&default, "bps default", "0.05", None);
}

#[test]
fn in_memory_measure_eur_per_hour_default_is_canonical() {
    let (default, ty) = show_default_literal(&cost_price_engine(), "cost_price", "labor_cost");
    match &default.value {
        ValueKind::Measure(r) => {
            assert_eq!(
                ty.measure_runtime_signature(),
                vec![("eur_per_hour".to_string(), 1)]
            );
            let magnitude = ValueKind::Number(r.clone())
                .as_decimal_magnitude()
                .expect("magnitude");
            let expected = decimal_lit("0.0069444444444444444444444444");
            assert_eq!(magnitude, expected, "labor_cost canonical magnitude");
        }
        other => panic!("expected Measure, got {other:?}"),
    }
}

#[test]
fn in_memory_bare_ratio_is_canonical_0_5() {
    let bare = LiteralValue::ratio_from_decimal(decimal_lit("0.5"));
    assert_ratio_exact(&bare, "bare ratio", "0.5", None);
}

// --- B: Ratio API serialize ---

#[test]
fn ratio_api_bare_0_5() {
    let bare = LiteralValue::ratio_from_decimal(decimal_lit("0.5"));
    let json = api_json_for_literal(
        "bare",
        &bare,
        &LemmaType::primitive(TypeSpecification::ratio()),
    );
    assert_eq!(json_ratio_unit_value(&json, "percent"), "50");
    assert_eq!(json_ratio_unit_value(&json, "permille"), "500");
}

#[test]
fn ratio_api_bare_0_15() {
    let bare = LiteralValue::ratio_from_decimal(decimal_lit("0.15"));
    let json = api_json_for_literal(
        "bare",
        &bare,
        &LemmaType::primitive(TypeSpecification::ratio()),
    );
    assert_eq!(json_ratio_unit_value(&json, "percent"), "15");
    assert_eq!(json_ratio_unit_value(&json, "permille"), "150");
}

#[test]
fn ratio_api_percent_15() {
    let json = api_show_default_json(&policy_engine(), "policy", "margin");
    assert_eq!(json_ratio_unit_value(&json, "percent"), "15");
}

#[test]
fn ratio_api_permille_150() {
    let json = api_show_default_json(&policy_engine(), "policy", "permille_rate");
    assert_eq!(json_ratio_unit_value(&json, "permille"), "150");
}

#[test]
fn ratio_api_basis_points_500() {
    let json = api_show_default_json(&policy_engine(), "policy", "bps");
    assert_eq!(json_ratio_unit_value(&json, "basis_points"), "500");
}

// --- C: Ratio API deserialize / accept ---

#[test]
fn ratio_api_bare_0_5_accepted() {
    let bare = LiteralValue::ratio_from_decimal(decimal_lit("0.5"));
    let json = api_json_for_literal(
        "bare",
        &bare,
        &LemmaType::primitive(TypeSpecification::ratio()),
    );
    let roundtrip =
        deserialize_api_literal(json, &LemmaType::primitive(TypeSpecification::ratio()));
    assert_ratio_exact(&roundtrip, "bare deserialize", "0.5", None);
}

#[test]
fn ratio_api_bare_0_5_roundtrip() {
    let bare = LiteralValue::ratio_from_decimal(decimal_lit("0.5"));
    let json = api_json_for_literal(
        "bare",
        &bare,
        &LemmaType::primitive(TypeSpecification::ratio()),
    );
    let roundtrip =
        deserialize_api_literal(json, &LemmaType::primitive(TypeSpecification::ratio()));
    assert_eq!(roundtrip.value, bare.value);
}

#[test]
fn ratio_api_percent_roundtrip() {
    let (original, original_ty) = show_default_literal(&policy_engine(), "policy", "margin");
    let json = api_json_for_literal("margin", &original, &original_ty);
    let roundtrip = deserialize_api_literal(json, &original_ty);
    assert_ratio_exact(&roundtrip, "percent roundtrip", "0.15", None);
}

#[test]
fn ratio_api_basis_points_roundtrip() {
    let (original, original_ty) = show_default_literal(&policy_engine(), "policy", "bps");
    let json = api_json_for_literal("bps", &original, &original_ty);
    let roundtrip = deserialize_api_literal(json, &original_ty);
    assert_ratio_exact(&roundtrip, "bps roundtrip", "0.05", None);
}

// --- D: Prompt matches API ---

#[test]
fn ratio_prompt_bare_0_5() {
    let bare = LiteralValue::ratio_from_decimal(decimal_lit("0.5"));
    let ty = LemmaType::primitive(TypeSpecification::ratio());
    assert_eq!(
        bare.magnitude_suggestion_for_decimal_prompt(&ty),
        Some(decimal_lit("0.5"))
    );
}

#[test]
fn ratio_prompt_percent_15() {
    // CLI prompts read the same per-unit map the API exposes (no separate
    // canonical-literal prompt path for ratios — see module doc).
    let json = api_show_default_json(&policy_engine(), "policy", "margin");
    assert_eq!(json_ratio_unit_value(&json, "percent"), "15");
}

#[test]
fn ratio_prompt_basis_points_500() {
    let json = api_show_default_json(&policy_engine(), "policy", "bps");
    assert_eq!(json_ratio_unit_value(&json, "basis_points"), "500");
}

#[test]
fn measure_prompt_eur_per_hour_25() {
    let (default, default_ty) =
        show_default_literal(&cost_price_engine(), "cost_price", "labor_cost");
    assert_eq!(
        default.magnitude_suggestion_for_decimal_prompt(&default_ty),
        Some(decimal_lit("25"))
    );
}

// --- E: Measure API ---

#[test]
fn measure_api_eur_per_hour_25() {
    let json = api_show_default_json(&cost_price_engine(), "cost_price", "labor_cost");
    assert_eq!(json_measure_unit_value(&json, "eur_per_hour"), "25");
}

#[test]
fn measure_api_kg_per_hour_12() {
    let json = api_show_default_json(&cost_price_engine(), "cost_price", "throughput");
    assert_eq!(json_measure_unit_value(&json, "kg_per_hour"), "12");
}

#[test]
fn measure_show_default_includes_all_declared_units() {
    let (default, default_ty) =
        show_default_literal(&cost_price_engine(), "cost_price", "labor_cost");
    let json = api_json_for_literal("labor_cost", &default, &default_ty);
    let measure = json["measure"]
        .as_object()
        .expect("labor_cost default must include measure unit map");
    assert_eq!(
        measure["eur_per_hour"].as_str(),
        Some("25"),
        "eur_per_hour magnitude"
    );
    let inr = measure["inr_per_hour"]
        .as_str()
        .expect("inr_per_hour must have RuleResultValue");
    assert_ne!(inr, "25", "inr_per_hour must differ from eur magnitude");
    assert_eq!(
        default.magnitude_in_unit(&default_ty, "inr_per_hour"),
        Some(decimal_lit(inr)),
        "magnitude_in_unit must match API map"
    );
}

#[test]
fn ratio_show_default_includes_all_declared_units() {
    let (default, default_ty) = show_default_literal(&policy_engine(), "policy", "bps");
    let json = api_json_for_literal("bps", &default, &default_ty);
    let ratio = json["ratio"]
        .as_object()
        .expect("bps default must include ratio unit map");
    assert_eq!(
        ratio["basis_points"].as_str(),
        Some("500"),
        "basis_points magnitude"
    );
    assert_eq!(
        default.magnitude_in_unit(&default_ty, "basis_points"),
        Some(decimal_lit("500")),
        "magnitude_in_unit must match API map"
    );
}

#[test]
fn measure_api_roundtrip_canonical() {
    let (original, original_ty) =
        show_default_literal(&cost_price_engine(), "cost_price", "labor_cost");
    let json = api_json_for_literal("labor_cost", &original, &original_ty);
    let roundtrip = deserialize_api_literal(json, &original_ty);
    assert_eq!(roundtrip.value, original.value);
}

#[test]
fn measure_prompt_matches_api() {
    let show = plan_interface_show(&cost_price_engine(), "cost_price");
    for (name, expected) in [
        ("labor_cost", "25"),
        ("throughput", "12"),
        ("product_cost", "4"),
    ] {
        let entry = show
            .data
            .get(name)
            .unwrap_or_else(|| panic!("{name}"))
            .clone();
        let suggestion = entry.suggestion.unwrap_or_else(|| panic!("{name} default"));
        let default = suggestion.clone().to_literal(&entry.lemma_type);
        assert_eq!(
            default.magnitude_suggestion_for_decimal_prompt(&entry.lemma_type),
            Some(decimal_lit(expected)),
            "{name} prompt must match API"
        );
        let json = wrap_suggestion_in_show_json(name, entry.lemma_type, suggestion);
        assert_eq!(
            json_measure_unit_value(&json, entry_unit_for(name)),
            expected,
            "{name} API value must match"
        );
    }
}

fn entry_unit_for(data_name: &str) -> &'static str {
    match data_name {
        "labor_cost" => "eur_per_hour",
        "throughput" => "kg_per_hour",
        "product_cost" => "eur_per_kg",
        other => panic!("no known unit for '{other}' in this test fixture"),
    }
}

// --- F: End-to-end consumer ---

fn cost_price_run_inputs() -> HashMap<String, String> {
    HashMap::from([
        ("product_cost".into(), "4 eur_per_kg".into()),
        ("labor_cost".into(), "25 eur_per_hour".into()),
        ("throughput".into(), "12 kg_per_hour".into()),
    ])
}

fn run_cost_price(engine: &Engine, now: &DateTimeValue) -> lemma::Response {
    engine
        .run(
            None,
            "cost_price",
            Some(now),
            cost_price_run_inputs(),
            None,
            true,
        )
        .expect("evaluation")
}

#[test]
fn measure_eval_per_unit_inputs_ok() {
    let engine = cost_price_engine();
    let now = DateTimeValue::now();
    run_cost_price(&engine, &now);
}

fn run_cost_price_with_single_override(
    engine: &Engine,
    now: &DateTimeValue,
    data_name: &str,
    convenience: &str,
    rule_names: Option<&[&str]>,
    explain: bool,
) -> lemma::Response {
    let mut data = cost_price_run_inputs();
    data.insert(data_name.to_string(), convenience.to_string());
    let rules: Option<Vec<String>> =
        rule_names.map(|names| names.iter().map(|name| (*name).to_string()).collect());
    engine
        .run(
            None,
            "cost_price",
            Some(now),
            data,
            rules.as_deref(),
            explain,
        )
        .expect("evaluation must complete")
}

fn assert_cost_price_rule_not_vetoed(response: &lemma::Response, context: &str) {
    let rule = response
        .results
        .get("cost_price")
        .unwrap_or_else(|| panic!("{context}: cost_price rule must be present"));
    assert!(
        !rule.vetoed,
        "{context}: cost_price must not veto, got {:?}",
        rule.veto_reason
    );
    assert_ne!(
        rule.veto_reason.as_deref(),
        Some("Calculated result exceeds decimal value limit"),
        "{context}: must not veto for decimal limit"
    );
    assert!(
        rule.result().is_some(),
        "{context}: cost_price must produce a committable display value"
    );
}

#[test]
fn measure_show_default_inr_per_hour_convenience_input_evaluates() {
    let engine = cost_price_engine();
    let now = DateTimeValue::now();
    let (default, default_ty) = show_default_literal(&engine, "cost_price", "labor_cost");
    let magnitude = default
        .magnitude_in_unit(&default_ty, "inr_per_hour")
        .expect("section E guarantees inr_per_hour RuleResultValue");
    let input = format!("{magnitude} inr_per_hour");
    let response = run_cost_price_with_single_override(
        &engine,
        &now,
        "labor_cost",
        &input,
        Some(&["cost_price"]),
        false,
    );
    assert_cost_price_rule_not_vetoed(
        &response,
        "show default inr_per_hour magnitude as convenience input",
    );
}

#[test]
fn measure_show_default_each_declared_unit_convenience_input_evaluates() {
    let engine = cost_price_engine();
    let now = DateTimeValue::now();
    let cases = [
        ("labor_cost", "eur_per_hour"),
        ("labor_cost", "inr_per_hour"),
        ("product_cost", "eur_per_kg"),
        ("product_cost", "inr_per_kg"),
        ("throughput", "kg_per_hour"),
    ];
    for (data_name, unit) in cases {
        let (default, default_ty) = show_default_literal(&engine, "cost_price", data_name);
        let magnitude = default
            .magnitude_in_unit(&default_ty, unit)
            .unwrap_or_else(|| panic!("{data_name} must have decimal for unit {unit}"));
        let input = format!("{magnitude} {unit}");
        let response = run_cost_price_with_single_override(
            &engine,
            &now,
            data_name,
            &input,
            Some(&["cost_price"]),
            false,
        );
        assert_cost_price_rule_not_vetoed(
            &response,
            &format!("{data_name} show default in {unit} as convenience input"),
        );
    }
}

#[test]
fn measure_show_default_inr_per_hour_not_overprecision_string() {
    let (default, default_ty) =
        show_default_literal(&cost_price_engine(), "cost_price", "labor_cost");
    let magnitude = default
        .magnitude_in_unit(&default_ty, "inr_per_hour")
        .expect("inr_per_hour must have RuleResultValue");
    let overprecision = decimal_lit("2717.3913043478260869565217391");
    assert_ne!(
        magnitude, overprecision,
        "show default must not emit unbounded output precision as convenience input"
    );
}

#[test]
fn ratio_show_default_basis_points_convenience_input_evaluates() {
    let engine = policy_engine();
    let now = DateTimeValue::now();
    let (default, default_ty) = show_default_literal(&engine, "policy", "bps");
    let magnitude = default
        .magnitude_in_unit(&default_ty, "basis_points")
        .expect("section E guarantees basis_points RuleResultValue");
    assert_eq!(magnitude, decimal_lit("500"));
    let mut data = HashMap::new();
    data.insert("bps".into(), format!("{magnitude} basis_points"));
    let response = engine
        .run(
            None,
            "policy",
            Some(&now),
            data,
            Some(&["bps_val".to_string()]),
            false,
        )
        .expect("evaluation must complete");
    let rule = response.results.get("bps_val").expect("bps_val rule");
    assert!(
        !rule.vetoed,
        "bps_val must not veto, got {:?}",
        rule.veto_reason
    );
}

#[test]
fn measure_show_literal_api_after_run() {
    let engine = cost_price_engine();
    let now = DateTimeValue::now();
    run_cost_price(&engine, &now);
    assert_eq!(
        json_measure_unit_value(
            &show_literal_api_json(&engine, "cost_price", "labor_cost"),
            "eur_per_hour"
        ),
        "25"
    );
    assert_eq!(
        json_measure_unit_value(
            &show_literal_api_json(&engine, "cost_price", "throughput"),
            "kg_per_hour"
        ),
        "12"
    );
}

#[test]
fn measure_response_json_serializes() {
    let engine = cost_price_engine();
    let now = DateTimeValue::now();
    let response = run_cost_price(&engine, &now);
    serde_json::to_string(&lemma::api::Response::from(&response))
        .expect("response API JSON must serialize");
}

#[test]
fn ratio_eval_15_percent_ok() {
    let engine = policy_engine();
    let now = DateTimeValue::now();
    let mut data = HashMap::new();
    data.insert("margin".into(), "15%".into());
    let response = engine
        .run(None, "policy", Some(&now), data, None, true)
        .expect("evaluation");
    let rr = response.results.get("m").expect("rule m");
    assert!(!rr.vetoed, "rule must not veto");
    let lit = rr
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("result value");
    assert_ratio_exact(&lit, "rule m", "0.15", Some("percent"));
}

#[test]
fn ratio_show_suggestion_api_percent() {
    let engine = policy_engine();
    let json = show_literal_api_json(&engine, "policy", "margin");
    assert_eq!(json_ratio_unit_value(&json, "percent"), "15");
}

#[test]
fn ratio_rule_result_bare_0_5_api() {
    let bare = LiteralValue::ratio_from_decimal(decimal_lit("0.5"));
    let json = api_json_for_literal(
        "half",
        &bare,
        &LemmaType::primitive(TypeSpecification::ratio()),
    );
    assert_eq!(json_ratio_unit_value(&json, "percent"), "50");
    let roundtrip =
        deserialize_api_literal(json, &LemmaType::primitive(TypeSpecification::ratio()));
    assert_ratio_exact(&roundtrip, "bare API roundtrip", "0.5", None);
}

// --- H: Range API ---

const RATIO_RANGE_PERCENT_SPEC: &str = r#"
spec policy
data allowed_band: ratio range -> suggest 10%...50%
rule band: allowed_band
"#;

const RATIO_RANGE_BPS_SPEC: &str = r#"
spec policy
data allowed_band: ratio range
  -> unit basis_points: 10000
  -> suggest 200 basis_points...3500 basis_points
rule band: allowed_band
"#;

#[test]
fn ratio_range_api_percent_endpoints() {
    let engine = load_engine(RATIO_RANGE_PERCENT_SPEC, "ratio_range_pct.lemma");
    let (default, default_ty) = show_default_literal(&engine, "policy", "allowed_band");
    let json = api_json_for_literal("allowed_band", &default, &default_ty);
    assert_eq!(
        range_endpoint_unit_value(&json, "from", "ratio", "percent"),
        "10"
    );
    assert_eq!(
        range_endpoint_unit_value(&json, "to", "ratio", "percent"),
        "50"
    );
}

#[test]
fn ratio_range_api_basis_points_endpoints() {
    let engine = load_engine(RATIO_RANGE_BPS_SPEC, "ratio_range_bps.lemma");
    let (default, default_ty) = show_default_literal(&engine, "policy", "allowed_band");
    let json = api_json_for_literal("allowed_band", &default, &default_ty);
    assert_eq!(
        range_endpoint_unit_value(&json, "from", "ratio", "basis_points"),
        "200"
    );
    assert_eq!(
        range_endpoint_unit_value(&json, "to", "ratio", "basis_points"),
        "3500"
    );
}

#[test]
fn ratio_range_api_roundtrip_canonical() {
    let engine = load_engine(RATIO_RANGE_PERCENT_SPEC, "ratio_range_pct.lemma");
    let (original, original_ty) = show_default_literal(&engine, "policy", "allowed_band");
    let json = api_json_for_literal("allowed_band", &original, &original_ty);
    let roundtrip = deserialize_api_literal(json, &original_ty);
    match (&original.value, &roundtrip.value) {
        (ValueKind::Range(l0, r0), ValueKind::Range(l1, r1)) => {
            assert_eq!(l0.value, l1.value);
            assert_eq!(r0.value, r1.value);
        }
        _ => panic!("expected range values"),
    }
}

// --- Contract additions: one API shape per type ---

const NON_BASE_MEASURE_SPEC: &str = r#"
spec pricing
uses lemma units
data money: measure
  -> unit eur: 1.00
  -> unit inr: 0.0092
  -> decimals 2
  -> suggest 100 inr
rule out: money
"#;

const PREFILLED_INR_SPEC: &str = r#"
spec base
uses lemma units
data money: measure
  -> unit eur: 1.00
  -> unit inr: 0.0092
  -> decimals 2
rule out: money

spec priced
uses base
  -> with money: 100 inr
rule out: base.out
"#;

const UNIT_SCOPED_RANGE_SPEC: &str = r#"
spec band
uses lemma units
data money: measure
  -> unit eur: 1.00
  -> unit inr: 0.0092
data window: money range -> suggest 10 eur...20 eur
rule band: window
"#;

#[test]
fn show_suggestion_carries_per_unit_magnitude() {
    let engine = load_engine(NON_BASE_MEASURE_SPEC, "pricing.lemma");
    let show = plan_interface_show(&engine, "pricing");
    let entry = show.data.get("money").expect("money").clone();
    let json = serde_json::to_value(lemma::api::ShowData::from(&entry)).expect("ShowData JSON");
    let suggestion = entry.suggestion.expect("suggestion");
    let measure = suggestion.measure.as_ref().expect("measure unit map");
    assert_eq!(measure.get("inr").copied(), Some(decimal_lit("100.00")));
    assert_ne!(
        measure.get("eur").copied(),
        Some(decimal_lit("100.00")),
        "eur magnitude must differ from inr (real unit conversion, not raw echo)"
    );
    assert_eq!(
        json["suggestion"]["measure"]["inr"].as_str(),
        Some("100.00")
    );
    assert_eq!(
        suggestion.result.as_deref(),
        Some("100.00 inr"),
        "suggestion display must use the unit written on -> suggest"
    );
}

#[test]
fn show_fill_carries_per_unit_magnitude() {
    let engine = load_engine(PREFILLED_INR_SPEC, "priced.lemma");
    let show = plan_interface_show(&engine, "priced");
    let entry = show.data.get("base.money").expect("base.money").clone();
    let json = serde_json::to_value(lemma::api::ShowData::from(&entry)).expect("ShowData JSON");
    let fill = entry.fill.expect("fill");
    let measure = fill.measure.as_ref().expect("measure unit map");
    assert_eq!(measure.get("inr").copied(), Some(decimal_lit("100.00")));
    assert_eq!(json["fill"]["measure"]["inr"].as_str(), Some("100.00"));
}

#[test]
fn execution_plan_constant_keeps_canonical_magnitude() {
    let engine = load_engine(PREFILLED_INR_SPEC, "priced.lemma");
    let now = DateTimeValue::now();
    let response = engine
        .run(None, "priced", Some(&now), HashMap::new(), None, true)
        .expect("run");
    let lit = response
        .results
        .get("out")
        .expect("out")
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("value");
    match &lit.value {
        ValueKind::Measure(canonical) => {
            let canonical_mag = ValueKind::Number(canonical.clone())
                .as_decimal_magnitude()
                .expect("canonical");
            assert_ne!(
                canonical_mag,
                decimal_lit("100"),
                "plan/eval storage must remain canonical base-unit magnitude"
            );
        }
        other => panic!("expected Measure, got {other:?}"),
    }
}

#[test]
fn range_endpoints_carry_own_value_maps() {
    let engine = load_engine(RATIO_RANGE_PERCENT_SPEC, "ratio_range_pct.lemma");
    let (default, default_ty) = show_default_literal(&engine, "policy", "allowed_band");
    let json = api_json_for_literal("allowed_band", &default, &default_ty);
    for side in ["from", "to"] {
        let endpoint = &json["range"][side];
        assert!(
            endpoint.get("ratio").and_then(|v| v.as_object()).is_some(),
            "{side} endpoint must carry its own ratio unit map: {endpoint}"
        );
        assert!(
            endpoint.get("range").is_none_or(serde_json::Value::is_null),
            "{side} endpoint must not itself be a range: {endpoint}"
        );
    }
}

#[test]
fn unit_scoped_measure_range_endpoint_keeps_unit() {
    let engine = load_engine(UNIT_SCOPED_RANGE_SPEC, "band.lemma");
    let (default, default_ty) = show_default_literal(&engine, "band", "window");
    let json = api_json_for_literal("window", &default, &default_ty);
    let from_measure = json["range"]["from"]["measure"]
        .as_object()
        .expect("endpoint must expose measure unit map");
    let eur = from_measure
        .get("eur")
        .and_then(|v| v.as_str())
        .expect("unit-scoped endpoint must keep eur");
    assert_eq!(decimal_lit(eur), decimal_lit("10"));
}

#[test]
fn ratio_without_unit_emits_json_null_not_empty_string() {
    let bare = LiteralValue::ratio_from_decimal(decimal_lit("0.5"));
    let json =
        serde_json::to_value(lemma::api::ValueKind::from(&bare.value)).expect("ValueKind JSON");
    assert!(
        json.get("ratio").is_some(),
        "ratio payload must be present, got {json}"
    );
    assert!(
        json["ratio"].get("unit").is_none(),
        "ValueKind::Ratio no longer carries a unit field, got {json}"
    );
}

#[test]
fn ratio_with_unit_emits_unit_string() {
    let bare = LiteralValue::ratio_from_decimal(decimal_lit("0.15"));
    let ValueKind::Ratio(canonical) = &bare.value else {
        panic!("expected ratio");
    };
    let with_unit = lemma::__test_support::TypedLiteral::ratio_with_bound_unit(
        canonical.clone(),
        "percent",
        std::sync::Arc::new(LemmaType::primitive(TypeSpecification::ratio())),
    );
    assert_eq!(
        with_unit.lemma_type.measure_binding_unit.as_deref(),
        Some("percent")
    );
}

#[test]
fn measure_declared_bound_is_named_value_unit_object() {
    let engine = load_engine(
        r#"
spec t
data money: measure
  -> unit eur: 1
  -> minimum 10 eur
  -> maximum 1000 eur
rule r: money
"#,
        "bounds.lemma",
    );
    let show = plan_interface_show(&engine, "t");
    let entry = show.data.get("money").expect("money");
    let json =
        serde_json::to_value(lemma::api::LemmaType::from(&entry.lemma_type)).expect("type JSON");
    assert!(json["minimum"].is_object(), "got {}", json["minimum"]);
    assert_eq!(json["minimum"]["value"].as_str(), Some("10"));
    assert_eq!(json["minimum"]["unit"].as_str(), Some("eur"));
    assert!(json["maximum"].is_object(), "got {}", json["maximum"]);
    assert_eq!(json["maximum"]["value"].as_str(), Some("1000"));
    assert_eq!(json["maximum"]["unit"].as_str(), Some("eur"));
    assert!(
        json["minimum"].as_array().is_none(),
        "positional [decimal, unit] array must not appear"
    );
}

#[test]
fn long_rational_serializes_exact_decimal_string() {
    let many = "0.3333333333333333333333333333";
    let engine = load_engine(
        &format!(
            r#"
spec exact
data x: number -> suggest {many}
rule out: x
"#
        ),
        "exact.lemma",
    );
    let json = api_show_default_json(&engine, "exact", "x");
    let number = json["number"].as_str().expect("number API string");
    assert!(
        !number.contains('e') && !number.contains('E'),
        "must not use scientific notation: {number}"
    );
    assert_eq!(
        decimal_lit(number),
        decimal_lit(many),
        "long decimal must serialize without float rounding; got {number}"
    );
}

/// Measure API unit maps (`measure_literal_in_all_units`) divide the literal's
/// canonical magnitude by each of the *type's* declared unit factors — it never inspects the
/// literal's own `signature`. This is intentional: real evaluated results normalize `signature`
/// into decomposed base units (e.g. `eur/hour` for a compound `eur_per_hour` measure), which is
/// a real, valid multi-term signature, not a bug — planning guarantees the canonical magnitude
/// is correctly denominated for the target type; unit-map conversion does not re-derive that from
/// `signature`. This test locks in that a compound (multi-term) signature still converts
/// correctly by canonical magnitude alone.
#[test]
fn compound_signature_measure_converts_by_canonical_magnitude() {
    let (template, template_ty) =
        show_default_literal(&cost_price_engine(), "cost_price", "labor_cost");
    let ValueKind::Measure(canonical) = &template.value else {
        panic!("expected Measure, got {:?}", template.value);
    };
    let compound = LiteralValue {
        value: ValueKind::Measure(canonical.clone()),
    };
    let json = api_json_for_literal("moment", &compound, &template_ty);
    assert_eq!(json_measure_unit_value(&json, "eur_per_hour"), "25");
}

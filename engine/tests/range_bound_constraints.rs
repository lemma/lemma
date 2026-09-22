//! Range endpoint (`lower`/`upper`) and width (`minimum`/`maximum`) constraints.

use lemma::type_detail_lines;
use lemma::DateTimeValue;
use lemma::Engine;
use lemma::TypeSpecification;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

fn load(engine: &mut Engine, code: &str, path: &str) {
    engine
        .load([(
            lemma::SourceType::Path(Arc::new(PathBuf::from(path))),
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

fn load_err(code: &str) -> String {
    let mut engine = Engine::new();
    let err = engine
        .load([(
            lemma::SourceType::Path(Arc::new(PathBuf::from("range_bound_constraints.lemma"))),
            code.to_string(),
        )])
        .expect_err("expected load failure");
    err.errors
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

fn now() -> DateTimeValue {
    DateTimeValue::now()
}

fn cargo_mass_shipment_ok() -> &'static str {
    r#"
spec s
data cargo_mass: measure
  -> unit kilogram: 1
  -> unit tonne: 1000

data shipment: cargo_mass range
  -> lower 100 kilogram
  -> upper 50 tonne
  -> minimum 500 kilogram
  -> maximum 2 tonne
  -> suggest 1 tonne...3 tonne

rule out: shipment
"#
}

// ─── A. Custom multi-unit measure ───────────────────────────────────────────

#[test]
fn custom_measure_range_accepts_cross_unit_bounds() {
    let mut engine = Engine::new();
    load(&mut engine, cargo_mass_shipment_ok(), "cargo_ok.lemma");

    let show = engine.show(None, "s", Some(&now())).expect("show");
    let entry = show.data.get("shipment").expect("shipment");
    match &entry.lemma_type.specifications {
        TypeSpecification::MeasureRange {
            lower,
            upper,
            minimum,
            maximum,
            ..
        } => {
            assert_eq!(lower.as_ref().unwrap().1, "kilogram");
            assert_eq!(upper.as_ref().unwrap().1, "tonne");
            assert_eq!(minimum.as_ref().unwrap().1, "kilogram");
            assert_eq!(maximum.as_ref().unwrap().1, "tonne");
        }
        other => panic!("expected MeasureRange, got {other:?}"),
    }
}

#[test]
fn custom_measure_range_rejects_lower_above_upper_across_units() {
    let code = r#"
spec s
data cargo_mass: measure
  -> unit kilogram: 1
  -> unit tonne: 1000

data shipment: cargo_mass range
  -> lower 3 tonne
  -> upper 1000 kilogram

rule out: shipment
"#;
    let err = load_err(code);
    assert!(
        err.to_lowercase().contains("lower") || err.to_lowercase().contains("invalid"),
        "expected lower>upper planning error, got: {err}"
    );
}

#[test]
fn custom_measure_range_rejects_width_minimum_above_maximum_across_units() {
    let code = r#"
spec s
data cargo_mass: measure
  -> unit kilogram: 1
  -> unit tonne: 1000

data shipment: cargo_mass range
  -> lower 100 kilogram
  -> upper 50 tonne
  -> minimum 3 tonne
  -> maximum 500 kilogram

rule out: shipment
"#;
    let err = load_err(code);
    assert!(
        err.to_lowercase().contains("minimum") || err.to_lowercase().contains("invalid"),
        "expected width minimum>maximum planning error, got: {err}"
    );
}

#[test]
fn custom_measure_range_default_fails_endpoint() {
    let code = r#"
spec s
data cargo_mass: measure
  -> unit kilogram: 1
  -> unit tonne: 1000

data shipment: cargo_mass range
  -> lower 100 kilogram
  -> upper 50 tonne
  -> minimum 500 kilogram
  -> maximum 2 tonne
  -> suggest 50 kilogram...1 tonne

rule out: shipment
"#;
    let err = load_err(code);
    assert!(
        err.to_lowercase().contains("below")
            || err.to_lowercase().contains("minimum")
            || err.to_lowercase().contains("lower")
            || err.to_lowercase().contains("suggest")
            || err.to_lowercase().contains("suggestion"),
        "expected suggestion endpoint failure, got: {err}"
    );
}

#[test]
fn custom_measure_range_default_fails_width() {
    let code = r#"
spec s
data cargo_mass: measure
  -> unit kilogram: 1
  -> unit tonne: 1000

data shipment: cargo_mass range
  -> lower 100 kilogram
  -> upper 50 tonne
  -> minimum 500 kilogram
  -> maximum 2 tonne
  -> suggest 1 tonne...1.1 tonne

rule out: shipment
"#;
    let err = load_err(code);
    assert!(
        err.to_lowercase().contains("span")
            || err.to_lowercase().contains("minimum")
            || err.to_lowercase().contains("width")
            || err.to_lowercase().contains("suggest")
            || err.to_lowercase().contains("suggestion"),
        "expected suggestion width failure (100kg span < 500kg minimum), got: {err}"
    );
}

#[test]
fn custom_measure_range_overlay_vetoes_endpoint() {
    let mut engine = Engine::new();
    load(&mut engine, cargo_mass_shipment_ok(), "cargo_veto_ep.lemma");

    let mut data = HashMap::new();
    data.insert("shipment".to_string(), "50 kilogram...1 tonne".into());
    let response = engine
        .run(None, "s", Some(&now()), data, None, false)
        .expect("run must complete with veto, not Error");
    let out = response.results.get("out").expect("out");
    assert!(out.vetoed, "overlay below lower must veto rule out");
    let reason = out.veto_reason.as_deref().expect("veto reason");
    assert!(
        reason.contains("is below minimum"),
        "endpoint veto must be element minimum, got: {reason}"
    );
}

#[test]
fn custom_measure_range_overlay_vetoes_width() {
    let mut engine = Engine::new();
    load(&mut engine, cargo_mass_shipment_ok(), "cargo_veto_w.lemma");

    let mut data = HashMap::new();
    // Endpoints inside envelope but span 100kg < minimum 500kg
    data.insert("shipment".to_string(), "1 tonne...1.1 tonne".into());
    let response = engine
        .run(None, "s", Some(&now()), data, None, false)
        .expect("run must complete with veto, not Error");
    let out = response.results.get("out").expect("out");
    assert!(out.vetoed, "overlay below width minimum must veto rule out");
    let reason = out.veto_reason.as_deref().expect("veto reason");
    assert!(
        reason.contains("span is below minimum width"),
        "width veto reason must be span minimum, got: {reason}"
    );
}

#[test]
fn custom_measure_range_rejects_foreign_unit() {
    let code = r#"
spec s
data cargo_mass: measure
  -> unit kilogram: 1
  -> unit tonne: 1000

data shipment: cargo_mass range
  -> lower 10 eur

rule out: shipment
"#;
    let err = load_err(code);
    assert!(
        err.to_lowercase().contains("eur") || err.to_lowercase().contains("unit"),
        "expected foreign unit error, got: {err}"
    );
}

#[test]
fn money_range_cross_unit_bounds() {
    let code = r#"
spec s
data money: measure
  -> unit eur: 1
  -> unit usd: 0.92

data band: money range
  -> lower 0 eur
  -> upper 100000 usd
  -> minimum 100 eur
  -> maximum 10000 usd
  -> suggest 1000 eur...5000 eur

rule out: band
"#;
    let mut engine = Engine::new();
    load(&mut engine, code, "money_range.lemma");
    let show = engine.show(None, "s", Some(&now())).expect("show");
    let entry = show.data.get("band").expect("band");
    match &entry.lemma_type.specifications {
        TypeSpecification::MeasureRange {
            lower,
            upper,
            minimum,
            maximum,
            ..
        } => {
            assert_eq!(lower.as_ref().unwrap().1, "eur");
            assert_eq!(upper.as_ref().unwrap().1, "usd");
            assert_eq!(minimum.as_ref().unwrap().1, "eur");
            assert_eq!(maximum.as_ref().unwrap().1, "usd");
        }
        other => panic!("expected MeasureRange, got {other:?}"),
    }
}

#[test]
fn anonymous_measure_range_cross_unit_bounds() {
    let code = r#"
spec s
data band: measure range
  -> unit kilogram: 1
  -> unit tonne: 1000
  -> lower 100 kilogram
  -> upper 50 tonne
  -> minimum 500 kilogram
  -> maximum 2 tonne
  -> suggest 1 tonne...3 tonne

rule out: band
"#;
    let mut engine = Engine::new();
    load(&mut engine, code, "anon_measure_range.lemma");
}

// ─── B. Accept / reject by range kind ───────────────────────────────────────

#[test]
fn number_range_accepts_all_four_commands() {
    let code = r#"
spec s
data tier: number range
  -> lower 0
  -> upper 100
  -> minimum 10
  -> maximum 50
  -> suggest 20...40

rule out: tier
"#;
    let mut engine = Engine::new();
    load(&mut engine, code, "number_range_ok.lemma");
}

#[test]
fn ratio_range_accepts_all_four_commands() {
    let code = r#"
spec s
data band: ratio range
  -> unit percent: 100
  -> lower 0%
  -> upper 100%
  -> minimum 10%
  -> maximum 50%
  -> suggest 20%...40%

rule out: band
"#;
    let mut engine = Engine::new();
    load(&mut engine, code, "ratio_range_ok.lemma");
}

#[test]
fn date_range_accepts_all_four_commands() {
    let code = r#"
spec s
uses lemma units
data window: date range
  -> lower 2020-01-01
  -> upper 2030-12-31
  -> minimum 1 day
  -> maximum 90 day
  -> suggest 2024-01-01...2024-01-31

rule out: window
"#;
    let mut engine = Engine::new();
    load(&mut engine, code, "date_range_ok.lemma");
}

#[test]
fn time_range_accepts_duration_width() {
    let code = r#"
spec s
uses lemma units
data shift: time range
  -> lower 09:00
  -> upper 17:00
  -> minimum 1 hour
  -> maximum 8 hour
  -> suggest 09:00...12:00

rule out: shift
"#;
    let mut engine = Engine::new();
    load(&mut engine, code, "time_range_ok.lemma");
}

#[test]
fn scalar_rejects_lower_upper() {
    for ty in ["number", "measure", "date", "time", "ratio"] {
        let unit_line = if ty == "measure" {
            "  -> unit kilogram: 1\n"
        } else if ty == "ratio" {
            "  -> unit percent: 100\n"
        } else {
            ""
        };
        let arg = match ty {
            "number" => "0",
            "measure" => "0 kilogram",
            "date" => "2020-01-01",
            "time" => "09:00",
            "ratio" => "0%",
            _ => unreachable!(),
        };
        let code = format!(
            r#"
spec s
data x: {ty}
{unit_line}  -> lower {arg}
rule out: x
"#
        );
        let err = load_err(&code);
        assert!(
            err.to_lowercase().contains("lower") || err.to_lowercase().contains("invalid"),
            "scalar {ty} must reject lower, got: {err}"
        );
    }
}

#[test]
fn date_range_rejects_date_as_width() {
    let code = r#"
spec s
data window: date range
  -> minimum 2020-01-01

rule out: window
"#;
    let err = load_err(code);
    assert!(
        err.to_lowercase().contains("minimum") || err.to_lowercase().contains("invalid"),
        "expected reject date as width, got: {err}"
    );
}

#[test]
fn time_range_rejects_calendar_width() {
    let code = r#"
spec s
uses lemma units
data shift: time range
  -> minimum 1 month

rule out: shift
"#;
    let err = load_err(code);
    assert!(
        err.to_lowercase().contains("month")
            || err.to_lowercase().contains("calendar")
            || err.to_lowercase().contains("invalid")
            || err.to_lowercase().contains("duration"),
        "expected reject calendar on time range width, got: {err}"
    );
}

#[test]
fn number_range_rejects_negative_width() {
    let code = r#"
spec s
data tier: number range
  -> minimum -1

rule out: tier
"#;
    let err = load_err(code);
    assert!(
        err.to_lowercase().contains("negative")
            || err.to_lowercase().contains("minimum")
            || err.to_lowercase().contains("invalid"),
        "expected reject negative width, got: {err}"
    );
}

#[test]
fn number_range_rejects_lower_above_upper() {
    let code = r#"
spec s
data tier: number range
  -> lower 100
  -> upper 0

rule out: tier
"#;
    let err = load_err(code);
    assert!(
        err.to_lowercase().contains("lower") || err.to_lowercase().contains("invalid"),
        "expected lower>upper error, got: {err}"
    );
}

#[test]
fn number_range_rejects_width_minimum_above_maximum() {
    let code = r#"
spec s
data tier: number range
  -> minimum 50
  -> maximum 10

rule out: tier
"#;
    let err = load_err(code);
    assert!(
        err.to_lowercase().contains("minimum") || err.to_lowercase().contains("invalid"),
        "expected width min>max error, got: {err}"
    );
}

// ─── C. Date / time width fork ──────────────────────────────────────────────

#[test]
fn date_range_calendar_width_minimum() {
    let code = r#"
spec s
uses lemma units
data window: date range
  -> minimum 1 month
  -> suggest 2024-01-01...2024-03-01

rule out: window
"#;
    let mut engine = Engine::new();
    load(&mut engine, code, "date_calendar_width.lemma");
}

#[test]
fn date_range_duration_width_minimum() {
    let code = r#"
spec s
uses lemma units
data window: date range
  -> minimum 7 day
  -> suggest 2024-01-01...2024-01-15

rule out: window
"#;
    let mut engine = Engine::new();
    load(&mut engine, code, "date_duration_width.lemma");
}

#[test]
fn date_range_rejects_mixed_calendar_and_duration_width() {
    let code = r#"
spec s
uses lemma units
data window: date range
  -> minimum 1 month
  -> maximum 90 day

rule out: window
"#;
    let err = load_err(code);
    assert!(
        err.to_lowercase().contains("calendar")
            || err.to_lowercase().contains("duration")
            || err.to_lowercase().contains("mixed")
            || err.to_lowercase().contains("incompatible")
            || err.to_lowercase().contains("invalid"),
        "expected reject mixed calendar/duration width, got: {err}"
    );
}

#[test]
fn date_range_calendar_vs_duration_not_interchangeable() {
    // Two full calendar months can be a short duration in days for Feb–Mar;
    // with only calendar minimum, this must load; with only day minimum that
    // exceeds the actual second-span it must fail as default width.
    let calendar_ok = r#"
spec s
uses lemma units
data window: date range
  -> minimum 2 month
  -> suggest 2024-01-15...2024-03-15

rule out: window
"#;
    let mut engine = Engine::new();
    load(&mut engine, calendar_ok, "cal_ok.lemma");

    // Same endpoints: ~60 days. Require 90 day minimum → default must fail.
    let duration_fail = r#"
spec s
uses lemma units
data window: date range
  -> minimum 90 day
  -> suggest 2024-01-15...2024-03-15

rule out: window
"#;
    let err = load_err(duration_fail);
    assert!(
        err.to_lowercase().contains("span")
            || err.to_lowercase().contains("minimum")
            || err.to_lowercase().contains("suggest")
            || err.to_lowercase().contains("suggestion")
            || err.to_lowercase().contains("below"),
        "90 day minimum must reject ~60 day span, got: {err}"
    );
}

// ─── D. Defaults, overlays, coercion ────────────────────────────────────────

#[test]
fn number_range_bad_default_is_planning_error() {
    let code = r#"
spec s
data tier: number range
  -> lower 0
  -> upper 100
  -> minimum 10
  -> suggest 0...5

rule out: tier
"#;
    let err = load_err(code);
    assert!(!err.is_empty(), "bad default must be planning error");
}

#[test]
fn number_range_overlay_vetoes_not_errors() {
    let code = r#"
spec s
data tier: number range
  -> lower 0
  -> upper 100
  -> minimum 10
  -> maximum 50

rule out: tier
"#;
    let mut engine = Engine::new();
    load(&mut engine, code, "number_overlay.lemma");
    let mut data = HashMap::new();
    data.insert("tier".to_string(), "0...5".into());
    let response = engine
        .run(None, "s", Some(&now()), data, None, false)
        .expect("run must complete");
    let out = response.results.get("out").expect("out");
    assert!(
        out.vetoed,
        "out-of-bound number range overlay must veto rule out"
    );
    let reason = out.veto_reason.as_deref().expect("veto reason");
    assert!(
        reason.contains("span is below minimum width"),
        "range overlay veto reason must be span minimum, got: {reason}"
    );
}

// ─── E. Named inheritance ───────────────────────────────────────────────────

#[test]
fn named_number_range_inherits_endpoint_bounds_not_width() {
    let code = r#"
spec s
data score: number
  -> minimum 0
  -> maximum 100

data window: score range
  -> minimum 10

rule out: window
"#;
    let mut engine = Engine::new();
    load(&mut engine, code, "named_score.lemma");
    let show = engine.show(None, "s", Some(&now())).expect("show");
    let entry = show.data.get("window").expect("window");
    match &entry.lemma_type.specifications {
        TypeSpecification::NumberRange {
            lower,
            upper,
            minimum,
            maximum,
            ..
        } => {
            assert!(lower.is_some(), "inherited lower from score.minimum");
            assert!(upper.is_some(), "inherited upper from score.maximum");
            assert!(minimum.is_some(), "width minimum 10");
            assert!(maximum.is_none(), "width maximum unset");
        }
        other => panic!("expected NumberRange, got {other:?}"),
    }
}

#[test]
fn named_money_range_inherits_endpoint_bounds_only() {
    let code = r#"
spec s
data money: measure
  -> unit eur: 1
  -> unit usd: 0.92
  -> minimum 0 eur
  -> maximum 1000000 eur

data band: money range

rule out: band
"#;
    let mut engine = Engine::new();
    load(&mut engine, code, "named_money.lemma");
    let show = engine.show(None, "s", Some(&now())).expect("show");
    let entry = show.data.get("band").expect("band");
    match &entry.lemma_type.specifications {
        TypeSpecification::MeasureRange {
            lower,
            upper,
            minimum,
            maximum,
            ..
        } => {
            assert!(lower.is_some());
            assert!(upper.is_some());
            assert!(minimum.is_none());
            assert!(maximum.is_none());
        }
        other => panic!("expected MeasureRange, got {other:?}"),
    }
}

// ─── F. Show / detail lines ─────────────────────────────────────────────────

#[test]
fn type_detail_lines_emit_range_bounds() {
    let mut engine = Engine::new();
    load(&mut engine, cargo_mass_shipment_ok(), "detail.lemma");
    let show = engine.show(None, "s", Some(&now())).expect("show");
    let entry = show.data.get("shipment").expect("shipment");
    let lines = type_detail_lines(&entry.lemma_type.specifications);
    let joined = lines.join("\n");
    assert!(joined.contains("lower:"), "got: {joined}");
    assert!(joined.contains("upper:"), "got: {joined}");
    assert!(joined.contains("minimum:"), "got: {joined}");
    assert!(joined.contains("maximum:"), "got: {joined}");
}

#[test]
fn measure_range_units_do_not_gain_lower_upper_fields() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        cargo_mass_shipment_ok(),
        "no_unit_fields.lemma",
    );
    let show = engine.show(None, "s", Some(&now())).expect("show");
    let entry = show.data.get("shipment").expect("shipment");
    match &entry.lemma_type.specifications {
        TypeSpecification::MeasureRange { units, .. } => {
            for unit in units.iter() {
                assert!(
                    unit.minimum.is_none(),
                    "units[] must not sync range endpoint/width into minimum"
                );
                assert!(
                    unit.maximum.is_none(),
                    "units[] must not sync range endpoint/width into maximum"
                );
            }
        }
        other => panic!("expected MeasureRange, got {other:?}"),
    }
}

#[test]
fn date_range_temporal_width_uses_exact_rational_path() {
    // Regression: temporal width bounds must not require Decimal conversion for RuleResultValue.
    let code = r#"
spec s
uses lemma units
data window: date range
  -> minimum 7 day
  -> maximum 90 day
  -> suggest 2024-01-01...2024-01-15

rule out: window
"#;
    let mut engine = Engine::new();
    load(&mut engine, code, "temporal_rational_width.lemma");

    let show = engine.show(None, "s", Some(&now())).expect("show");
    let entry = show.data.get("window").expect("window");
    match &entry.lemma_type.specifications {
        TypeSpecification::DateRange {
            minimum, maximum, ..
        } => {
            assert_eq!(minimum.as_ref().unwrap().1, "day");
            assert_eq!(maximum.as_ref().unwrap().1, "day");
        }
        other => panic!("expected DateRange, got {other:?}"),
    }
}

#[test]
fn generic_measure_range_suggest_without_units_is_planning_error() {
    let code = r#"
spec s
data band: measure range
  -> suggest 1 kilogram...5 kilogram

rule out: band
"#;
    let err = load_err(code);
    assert!(
        err.contains("Unknown unit") && err.contains("kilogram"),
        "generic measure range suggest without units must planning Error, not panic or Ok: {err}"
    );
}

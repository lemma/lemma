use lemma::DateTimeValue;
use lemma::{Engine, SourceType};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

fn path_source(file: &str) -> SourceType {
    SourceType::Path(Arc::new(PathBuf::from(file)))
}

fn eval_rule(code: &str, spec_name: &str, rule_name: &str) -> String {
    let mut engine = Engine::new();
    engine
        .load([(path_source("stdlib_lemma_units.lemma"), code.to_string())])
        .expect("load");
    let now = DateTimeValue::now();
    let response = engine
        .run(None, spec_name, Some(&now), HashMap::new(), None, false)
        .expect("run");
    response
        .results
        .get(rule_name)
        .unwrap_or_else(|| panic!("rule {rule_name:?} missing"))
        .result()
        .expect("result")
        .to_string()
}

fn expect_plan_error(code: &str, source_file: &str) -> String {
    let mut engine = Engine::new();
    let err = engine
        .load([(path_source(source_file), code.to_string())])
        .expect_err("expected planning error");
    err.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

#[test]
fn uses_lemma_units_duration_typedef_and_literals() {
    let code = r#"spec consumer
uses lemma units
data age: units.duration
rule hour: age as hour"#;
    let mut engine = Engine::new();
    engine
        .load([(path_source("consumer.lemma"), code.to_string())])
        .expect("plan");
    let now = DateTimeValue::now();
    let mut data = HashMap::new();
    data.insert("age".to_string(), "90 minute".into());
    let response = engine
        .run(None, "consumer", Some(&now), data, None, false)
        .expect("run");
    assert_eq!(
        response
            .results
            .get("hour")
            .expect("hour rule")
            .result
            .as_ref()
            .and_then(|v| v.measure.as_ref())
            .and_then(|m| m.get("hour"))
            .copied(),
        Some(rust_decimal::Decimal::new(15, 1))
    );
}

#[test]
fn uses_lemma_units_length_and_duration_units_for_compound_cast() {
    let code = r#"spec speed_test
uses lemma units
data velocity: measure -> unit mps: meter/second
data dist: 100 meter
data secs: 20 second
rule speed: (dist / secs) as mps"#;
    let out = eval_rule(code, "speed_test", "speed");
    assert!(
        out.contains('5') && out.to_lowercase().contains("mps"),
        "expected 5 mps, got: {out}"
    );
}

#[test]
fn plural_duration_unit_is_unknown() {
    let combined = expect_plan_error(
        r#"spec bad
uses lemma units
rule x: 1 hours"#,
        "plural_hours.lemma",
    );
    assert!(
        !combined.is_empty(),
        "expected planning error for plural hours, got: {combined:?}"
    );
}

#[test]
fn british_metre_unit_is_unknown() {
    let combined = expect_plan_error(
        r#"spec bad
uses lemma units
rule x: 1 metre"#,
        "british_metre.lemma",
    );
    assert!(
        !combined.is_empty(),
        "expected planning error for metre, got: {combined:?}"
    );
}

#[test]
fn american_meter_converts() {
    let out = eval_rule(
        r#"spec length_test
uses lemma units
rule x: 1000 millimeter as meter"#,
        "length_test",
        "x",
    );
    assert!(
        out.contains('1') && out.contains("meter"),
        "expected 1 meter, got: {out}"
    );
}

#[test]
fn inch_as_centimeter() {
    let out = eval_rule(
        r#"spec imperial_length
uses lemma units
rule x: 1 inch as centimeter"#,
        "imperial_length",
        "x",
    );
    assert!(
        out.contains("2.54") && out.contains("centimeter"),
        "expected 2.54 centimeter, got: {out}"
    );
}

#[test]
fn pound_as_kilogram() {
    let out = eval_rule(
        r#"spec imperial_mass
uses lemma units
rule x: 1 pound as kilogram"#,
        "imperial_mass",
        "x",
    );
    assert!(
        out.contains("0.45359237") && out.contains("kilogram"),
        "expected 0.45359237 kilogram, got: {out}"
    );
}

#[test]
fn liter_as_milliliter() {
    let out = eval_rule(
        r#"spec volume_test
uses lemma units
rule x: 1 liter as milliliter"#,
        "volume_test",
        "x",
    );
    assert!(
        out.contains("1000") && out.contains("milliliter"),
        "expected 1000 milliliter, got: {out}"
    );
}

#[test]
fn mass_times_accel_as_newton() {
    let out = eval_rule(
        r#"spec force_test
uses lemma units
data acceleration: measure -> unit mps2: meter/second^2
data m: 10 kilogram
data a: 5 mps2
rule f: (m * a) as newton"#,
        "force_test",
        "f",
    );
    assert!(
        out.contains("50") && out.contains("newton"),
        "expected 50 newton, got: {out}"
    );
}

#[test]
fn kilowatt_as_watt() {
    let out = eval_rule(
        r#"spec power_test
uses lemma units
rule x: 1 kilowatt as watt"#,
        "power_test",
        "x",
    );
    assert!(
        out.contains("1000") && out.contains("watt"),
        "expected 1000 watt, got: {out}"
    );
}

#[test]
fn byte_as_bit() {
    let out = eval_rule(
        r#"spec info_test
uses lemma units
rule x: 1 byte as bit"#,
        "info_test",
        "x",
    );
    assert!(
        out.contains('8') && out.contains("bit"),
        "expected 8 bit, got: {out}"
    );
}

#[test]
fn kilobyte_as_bit() {
    let out = eval_rule(
        r#"spec info_test
uses lemma units
rule x: 1 kilobyte as bit"#,
        "info_test",
        "x",
    );
    assert!(
        out.contains("8000") && out.contains("bit"),
        "expected 8000 bit, got: {out}"
    );
}

#[test]
fn megabyte_differs_from_mebibyte() {
    let mb = eval_rule(
        r#"spec info_test
uses lemma units
rule x: 1 megabyte as byte"#,
        "info_test",
        "x",
    );
    let mib = eval_rule(
        r#"spec info_test
uses lemma units
rule x: 1 mebibyte as byte"#,
        "info_test",
        "x",
    );
    assert!(
        mb.contains("1000000") && mb.contains("byte"),
        "expected 1000000 byte, got: {mb}"
    );
    assert!(
        mib.contains("1048576") && mib.contains("byte"),
        "expected 1048576 byte, got: {mib}"
    );
}

#[test]
fn show_lists_stdlib_typedefs_with_empty_needed_by_rules() {
    let engine = Engine::new();
    let show = engine
        .show(Some("lemma"), "units", None)
        .expect("show lemma units");
    assert!(
        show.rules.is_empty(),
        "units has no rules; rules: {:?}",
        show.rules.keys().collect::<Vec<_>>()
    );
    let expected = [
        "duration",
        "length",
        "mass",
        "force",
        "pressure",
        "energy",
        "power",
        "frequency",
        "area",
        "volume",
        "information",
        "charge",
        "voltage",
        "resistance",
        "capacitance",
        "temperature",
        "current",
        "substance",
        "luminous_intensity",
        "calendar",
    ];
    for name in expected {
        let entry = show.data.get(name).unwrap_or_else(|| {
            panic!(
                "expected typedef {name} in show.data; keys: {:?}",
                show.data.keys().collect::<Vec<_>>()
            )
        });
        assert!(
            entry.needed_by_rules.is_empty(),
            "{name} must have empty needed_by_rules"
        );
    }
    let source = engine
        .source(Some("lemma"), Some("units"), None)
        .expect("source lemma units");
    for name in expected {
        assert!(
            source.contains(name),
            "expected typedef {name} in units source"
        );
    }
}

#[test]
fn uses_lemma_units_kelvin_temperature() {
    let display = eval_rule(
        r#"spec temp
uses lemma units
rule r: 300 kelvin"#,
        "temp",
        "r",
    );
    assert_eq!(display, "300 kelvin");
}

#[test]
fn uses_lemma_calendar_does_not_resolve_standalone_spec() {
    let code = r#"spec bad
uses lemma calendar
rule x: 1 year"#;
    let combined = expect_plan_error(code, "bad.lemma");
    assert!(
        !combined.is_empty(),
        "expected planning error for uses lemma calendar, got: {combined:?}"
    );
}

#[test]
fn bare_uses_lemma_without_units_does_not_resolve_stdlib() {
    let code = r#"spec bad
uses lemma
rule x: 1 hour"#;
    let combined = expect_plan_error(code, "bad.lemma");
    assert!(
        !combined.is_empty(),
        "expected planning error for bare uses lemma, got: {combined:?}"
    );
}

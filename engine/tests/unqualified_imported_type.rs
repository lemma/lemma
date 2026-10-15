//! Imported types need the alias: `data age: units.calendar`.
//! A bare name that an import declares is a planning Error naming that alias.

use indexmap::IndexMap;
use lemma::{DateTimeValue, Engine, EvaluateDataValue, SourceType};
use std::path::PathBuf;
use std::sync::Arc;

fn path_source(file: &str) -> SourceType {
    SourceType::Path(Arc::new(PathBuf::from(file)))
}

fn load_ok(code: &str) -> Engine {
    let mut engine = Engine::new();
    engine
        .load([(path_source("test.lemma"), code.to_string())])
        .unwrap_or_else(|errs| {
            let joined = errs
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("\n");
            panic!("expected load to succeed, got: {joined}");
        });
    engine
}

fn load_err_joined(code: &str) -> String {
    let mut engine = Engine::new();
    let err = engine
        .load([(path_source("test.lemma"), code.to_string())])
        .expect_err("expected load to fail");
    err.iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

fn eval_rule(
    engine: &mut Engine,
    spec: &str,
    data: IndexMap<String, EvaluateDataValue>,
    rule: &str,
) -> String {
    let now = DateTimeValue::now();
    let response = engine
        .evaluate(None, spec, Some(&now), data, None, false)
        .expect("evaluate");
    response
        .results
        .get(rule)
        .unwrap_or_else(|| panic!("rule '{rule}' missing"))
        .result()
        .expect("result")
        .to_string()
}

#[test]
fn bare_imported_type_errors_naming_alias() {
    let joined = load_err_joined(
        r#"spec test
uses lemma units
data age: calendar
rule value: age as year"#,
    );
    assert!(
        joined.contains("units.calendar"),
        "expected alias hint, got: {joined}"
    );
}

#[test]
fn bare_imported_type_error_reported_once_when_spec_is_imported() {
    let mut engine = Engine::new();
    let errors = engine
        .load([(
            path_source("production.lemma"),
            r#"spec production
uses lemma units
data material_weight: mass
rule heavy: material_weight > 0 kilogram

spec manufacturing
uses production
rule ok: production.heavy"#
                .to_string(),
        )])
        .expect_err("expected bare-import error");
    let rendered: Vec<String> = errors.iter().map(|e| e.to_string()).collect();
    assert_eq!(rendered.len(), 1, "expected one error, got: {rendered:#?}");
    assert!(
        rendered[0].contains("In spec 'production'") && rendered[0].contains("units.mass"),
        "expected error attributed to production, got: {}",
        rendered[0]
    );
}

#[test]
fn imported_type_with_constraints_needs_alias() {
    let code = r#"spec test
uses lemma units
data age: units.calendar -> minimum 1 year
rule value: age as year"#;
    let mut engine = load_ok(code);
    let mut data = IndexMap::new();
    data.insert("age".to_string(), "36 month".into());
    let result = eval_rule(&mut engine, "test", data, "value");
    assert_eq!(result, "3 year");
}

#[test]
fn imported_type_range_needs_alias() {
    let code = r#"spec test
uses lemma units
data span: units.calendar range
rule value: span"#;
    load_ok(code);
}

#[test]
fn bare_imported_type_from_two_imports_lists_both_aliases() {
    let lib_a = r#"spec lib_a
data duration: measure
  -> unit second: 1
  -> unit minute: 60"#;
    let lib_b = r#"spec lib_b
data duration: measure
  -> unit tick: 1"#;
    let consumer = r#"spec consumer
uses lib_a
uses lib_b
data elapsed: duration
rule value: elapsed"#;

    let mut engine = Engine::new();
    engine
        .load([(path_source("lib_a.lemma"), lib_a.to_string())])
        .expect("lib_a loads");
    engine
        .load([(path_source("lib_b.lemma"), lib_b.to_string())])
        .expect("lib_b loads");
    let err = engine
        .load([(path_source("consumer.lemma"), consumer.to_string())])
        .expect_err("expected bare-import error");
    let joined = err
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        joined.contains("lib_a.duration") && joined.contains("lib_b.duration"),
        "expected both qualified forms, got: {joined}"
    );
}

// ---------------------------------------------------------------------------
// 5. Local typedef shadows imported type (existing behavior)
// ---------------------------------------------------------------------------
#[test]
fn unqualified_type_shadowed_by_local_typedef() {
    let code = r#"spec test
uses lemma units
data length: measure
  -> unit span: 1
  -> unit rod: 16.5
data height: length
rule value: height as rod"#;
    let mut engine = load_ok(code);
    let mut data = IndexMap::new();
    data.insert("height".to_string(), "33 span".into());
    let result = eval_rule(&mut engine, "test", data, "value");
    assert_eq!(result, "2 rod");
}

// ---------------------------------------------------------------------------
// 6. Import alias name still errors (regression guard)
// ---------------------------------------------------------------------------
#[test]
fn unqualified_type_import_alias_still_errors() {
    let code = r#"spec test
uses lemma units
data x: units
rule r: x"#;
    let joined = load_err_joined(code);
    assert!(
        joined.contains("import alias"),
        "expected 'import alias' hint, got: {joined}"
    );
}

// ---------------------------------------------------------------------------
// 7. Unknown name still errors (regression guard)
// ---------------------------------------------------------------------------
#[test]
fn unqualified_type_still_errors_when_no_import_has_it() {
    let code = r#"spec test
uses lemma units
data x: nonexistent
rule r: x"#;
    let joined = load_err_joined(code);
    assert!(
        joined.contains("Unknown parent"),
        "expected 'Unknown parent', got: {joined}"
    );
}

// ---------------------------------------------------------------------------
// 8. Qualified form still works (regression guard)
// ---------------------------------------------------------------------------
#[test]
fn qualified_form_still_works() {
    let code = r#"spec test
uses lemma units
data age: units.calendar
rule value: age as year"#;
    let mut engine = load_ok(code);
    let mut data = IndexMap::new();
    data.insert("age".to_string(), "36 month".into());
    let result = eval_rule(&mut engine, "test", data, "value");
    assert_eq!(result, "3 year");
}

// ---------------------------------------------------------------------------
// Local type with the same name as an imported type
// ---------------------------------------------------------------------------

const PRICING_AND_SHOP: &str = r#"
spec pricing
data money: measure
  -> unit eur: 1.00

spec shop
uses pricing
data money: measure
  -> unit usd: 1.00
  -> unit cent: 0.01
"#;

#[test]
fn imported_unit_stays_in_scope_beside_same_named_local_type() {
    let mut engine = load_ok(&format!("{PRICING_AND_SHOP}rule fee: 5 eur\n"));
    assert_eq!(
        eval_rule(&mut engine, "shop", IndexMap::new(), "fee"),
        "5 eur"
    );
}

#[test]
fn qualified_imported_type_beside_same_named_local_type_binds_its_units() {
    let mut engine = load_ok(&format!(
        "{PRICING_AND_SHOP}data fee: pricing.money\n  -> with 5 eur\nrule out: fee\n"
    ));
    assert_eq!(
        eval_rule(&mut engine, "shop", IndexMap::new(), "out"),
        "5 eur"
    );
}

#[test]
fn same_named_local_and_imported_types_are_distinct_families() {
    let joined = load_err_joined(&format!(
        "{PRICING_AND_SHOP}data a: pricing.money\n  -> with 5 eur\ndata b: money\n  -> with 3 usd\nrule s: a + b\n"
    ));
    assert!(
        joined.contains("Cannot add unrelated measure types: a and b"),
        "expected a family mismatch error, got: {joined}"
    );
}

#[test]
fn local_unit_conversion_ignores_same_named_imported_family() {
    let mut engine = load_ok(&format!("{PRICING_AND_SHOP}rule c: 250 cent as usd\n"));
    assert_eq!(
        eval_rule(&mut engine, "shop", IndexMap::new(), "c"),
        "2.5 usd"
    );
}

// ---------------------------------------------------------------------------
// Bug: qualified parent first segment must be a uses alias (not bare spec name)
// ---------------------------------------------------------------------------

#[test]
fn qualified_parent_without_uses_is_error() {
    let joined = load_err_joined(
        r#"
spec a
data x: number

spec c
data zz: a.x
rule z: zz
"#,
    );
    assert!(
        joined.contains("uses") && joined.contains("a"),
        "expected uses-alias error for bare a.x, got: {joined}"
    );
}

#[test]
fn qualified_parent_with_matching_uses_alias_ok() {
    let mut engine = load_ok(
        r#"
spec a
data x: number
  -> minimum 4

spec c
uses a
data zz: a.x
rule z: zz
"#,
    );
    let mut data = IndexMap::new();
    data.insert("zz".to_string(), "5".into());
    assert_eq!(eval_rule(&mut engine, "c", data, "z"), "5");
}

#[test]
fn qualified_parent_with_explicit_uses_alias_ok() {
    let mut engine = load_ok(
        r#"
spec a
data x: number

spec c
uses foo: a
data zz: foo.x
rule z: zz
"#,
    );
    let mut data = IndexMap::new();
    data.insert("zz".to_string(), "5".into());
    assert_eq!(eval_rule(&mut engine, "c", data, "z"), "5");
}

#[test]
fn qualified_parent_target_name_instead_of_uses_alias_is_error() {
    let joined = load_err_joined(
        r#"
spec a
data x: number

spec c
uses foo: a
data zz: a.x
rule z: zz
"#,
    );
    assert!(
        joined.contains("uses") && (joined.contains("foo") || joined.contains("a")),
        "expected alias-not-target error for a.x after uses foo: a, got: {joined}"
    );
}

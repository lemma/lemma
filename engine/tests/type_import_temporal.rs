//! Regression test: type-only dependencies must respect temporal versioning.
//!
//! When spec B depends on spec A *only* via qualified parent types (`uses f: A` +
//! `data money: f.money`) without an extra data-level spec ref on `A`, and `A`
//! has multiple temporal versions with different type definitions, B must
//! produce separate temporal slices — one per version of A that falls within
//! B's effective range.

use lemma::{DateGranularity, DateTimeValue, Engine};
use std::collections::HashMap;

fn date(year: i32, month: u32, day: u32) -> DateTimeValue {
    DateTimeValue {
        year,
        month,
        day,
        hour: 0,
        minute: 0,
        second: 0,
        microsecond: 0,
        timezone: None,
        granularity: DateGranularity::Full,
    }
}

fn eval_with(
    engine: &Engine,
    spec_name: &str,
    effective: &DateTimeValue,
    data: Vec<(&str, &str)>,
) -> lemma::Response {
    let map: HashMap<String, String> = data
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    engine
        .run(None, spec_name, Some(effective), map, None, false)
        .unwrap()
}

fn assert_rule_value(response: &lemma::Response, rule: &str, expected: &str) {
    let result = response
        .results
        .get(rule)
        .unwrap_or_else(|| panic!("rule '{}' not in results", rule));
    if result.vetoed {
        panic!(
            "rule '{}' is Veto: {}",
            rule,
            result.veto_reason.as_deref().unwrap_or("Vetoed")
        );
    }
    assert_eq!(
        result.result(),
        Some(expected),
        "rule '{}': expected {}, got {:?}",
        rule,
        expected,
        result.result()
    );
}

/// `uses f: finance 2025-02-01` pins to finance v1 (eur only) regardless of
/// evaluation datetime. The pin freezes the type at that instant.
#[test]
fn qualified_data_import_pins_to_referenced_version() {
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
 -> decimals 2
data base_price: 50.00 eur

spec finance 2025-07-01
data money: measure
 -> unit eur: 1.00
 -> unit usd: 0.91
 -> decimals 2
data base_price: 75.00 eur
"#
            .to_string(),
        )])
        .unwrap();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("shop.lemma"))),
            r#"
spec shop 2025-01-01
uses f: finance 2025-02-01
data money: f.money
data price: money
rule doubled: price * 2
"#
            .to_string(),
        )])
        .unwrap();

    // Pin resolves finance v1 (eur only), works at any eval datetime.
    assert_rule_value(
        &eval_with(
            &engine,
            "shop",
            &date(2025, 3, 1),
            vec![("price", "10.00 eur")],
        ),
        "doubled",
        "20.00 eur",
    );

    // Even after the boundary, the pin keeps us on v1 — still eur only.
    assert_rule_value(
        &eval_with(
            &engine,
            "shop",
            &date(2025, 9, 1),
            vec![("price", "10.00 eur")],
        ),
        "doubled",
        "20.00 eur",
    );
}

/// `uses f: finance 2025-02-01` pins to finance v1 (eur only). Using a unit from
/// v2 (usd) must veto even after the v2 boundary, because the pin freezes the
/// type at the qualified instant.
#[test]
fn qualified_data_import_rejects_unit_from_later_version() {
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
 -> decimals 2

spec finance 2025-07-01
data money: measure
 -> unit eur: 1.00
 -> unit usd: 0.91
 -> decimals 2
"#
            .to_string(),
        )])
        .unwrap();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("shop.lemma"))),
            r#"
spec shop 2025-01-01
uses f: finance 2025-02-01
data money: f.money
data price: money
rule doubled: price * 2
"#
            .to_string(),
        )])
        .unwrap();

    // eur works: finance v1 has eur
    assert_rule_value(
        &eval_with(
            &engine,
            "shop",
            &date(2025, 3, 1),
            vec![("price", "10.00 eur")],
        ),
        "doubled",
        "20.00 eur",
    );

    // usd must veto: pin locks to finance v1 which only has eur
    let effective = date(2025, 9, 1);
    let response = engine
        .run(
            None,
            "shop",
            Some(&effective),
            HashMap::from([("price".to_string(), "10.00 usd".into())]),
            None,
            false,
        )
        .expect("usd override must complete with veto, not Error");
    let doubled = response.results.get("doubled").expect("doubled");
    assert!(doubled.vetoed, "usd should be rejected by pinned v1 type");
    let reason = doubled.veto_reason.as_deref().expect("veto reason");
    assert!(
        reason.contains("Unknown unit") && reason.contains("usd"),
        "veto should mention unknown unit 'usd', got: {reason}"
    );
}

/// Unranged consumer with a type-only dep whose interface changes between
/// temporal slices must be rejected when the reference is not pinned.
#[test]
fn unranged_spec_with_type_only_dep_rejects_incompatible_interface() {
    let mut engine = Engine::new();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("units.lemma"))),
            r#"
spec units
data weight: measure
 -> unit kg: 1.00
 -> decimals 1

spec units 2025-06-01
data weight: measure
 -> unit kg: 1.00
 -> unit lb: 2.205
 -> decimals 1
"#
            .to_string(),
        )])
        .unwrap();

    let result = engine.load([(
        lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
            "warehouse.lemma",
        ))),
        r#"
spec warehouse
uses units
data weight: units.weight
data item_weight: weight
rule heavy: item_weight > 100.0 kg
"#
        .to_string(),
    )]);

    assert!(
        result.is_err(),
        "Unpinned type-only dep with incompatible interfaces must be rejected"
    );
    let errs = result.unwrap_err();
    let combined: String = errs
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("; ");
    assert!(
        combined.contains("changed its interface"),
        "Error should mention interface change, got: {combined}"
    );
}

/// Mixed scenario: consumer has both a data-level spec ref (`uses ref: finance`) and
/// qualified parent types resolving through the same dependency. Consumer needs
/// separate temporal versions to satisfy the cross-spec interface contract (dep's
/// interface changes between slices).
#[test]
fn mixed_spec_ref_and_data_import_to_same_dep() {
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
 -> decimals 2
data base_price: 50.00 eur

spec finance 2025-07-01
data money: measure
 -> unit eur: 1.00
 -> unit usd: 0.91
 -> decimals 2
data base_price: 75.00 eur
"#
            .to_string(),
        )])
        .unwrap();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("shop.lemma"))),
            r#"
spec shop 2025-01-01
uses finance
uses ref: finance
data money: finance.money
data price: money
rule total: ref.base_price + price

spec shop 2025-07-01
uses finance
uses ref: finance
data money: finance.money
data price: money
rule total: ref.base_price + price
"#
            .to_string(),
        )])
        .unwrap();

    // Before boundary: finance v1, base_price=50, only eur
    assert_rule_value(
        &eval_with(
            &engine,
            "shop",
            &date(2025, 3, 1),
            vec![("price", "10.00 eur")],
        ),
        "total",
        "60.00 eur",
    );

    // After boundary: finance v2, base_price=75, eur+usd available
    assert_rule_value(
        &eval_with(
            &engine,
            "shop",
            &date(2025, 9, 1),
            vec![("price", "10.00 eur")],
        ),
        "total",
        "85.00 eur",
    );

    // After boundary: usd must be available from the qualified `f.money` type.
    // unit usd 0.91: 1 USD = 0.91 EUR; 10 USD => 9.10 EUR.
    // 75.00 + 9.10 = 84.10 eur.
    assert_rule_value(
        &eval_with(
            &engine,
            "shop",
            &date(2025, 9, 1),
            vec![("price", "10.00 usd")],
        ),
        "total",
        "84.10 eur",
    );
}

/// Unpinned `uses finance` + `data price: finance.money` must be rejected when the
/// source spec's interface changes across versions.
#[test]
fn inline_data_import_rejects_incompatible_unpinned_dep() {
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
 -> decimals 2
data base_price: 50.00 eur

spec finance 2025-07-01
data money: measure
 -> unit eur: 1.00
 -> unit usd: 0.91
 -> decimals 2
data base_price: 75.00 eur
"#
            .to_string(),
        )])
        .unwrap();

    let result = engine.load([(
        lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("shop.lemma"))),
        r#"
spec shop 2025-01-01
uses finance
data price: finance.money
rule doubled: price * 2
"#
        .to_string(),
    )]);

    assert!(
        result.is_err(),
        "Unpinned uses + qualified parent with incompatible dep interfaces must be rejected"
    );
    let errs = result.unwrap_err();
    let combined: String = errs
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("; ");
    assert!(
        combined.contains("changed its interface"),
        "Error should mention interface change, got: {combined}"
    );
}

/// Qualified `uses` with explicit effective datetime pins resolution to that version.
#[test]
fn data_import_with_effective_datetime_pins_version() {
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
 -> decimals 2
data base_price: 50.00 eur

spec finance 2025-07-01
data money: measure
 -> unit eur: 1.00
 -> unit usd: 0.91
 -> decimals 2
data base_price: 75.00 eur
"#
            .to_string(),
        )])
        .unwrap();

    // Pin `uses f: finance` to a date before the v2 boundary; even when evaluated
    // after 2025-07-01, only eur should be available.
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("shop.lemma"))),
            r#"
spec shop 2025-01-01
uses f: finance 2025-03-01
data money: f.money
data price: money
rule doubled: price * 2
"#
            .to_string(),
        )])
        .unwrap();

    // Evaluate after the finance boundary — but `uses f: finance 2025-03-01`
    // resolves finance v1 (eur only).
    assert_rule_value(
        &eval_with(
            &engine,
            "shop",
            &date(2025, 9, 1),
            vec![("price", "10.00 eur")],
        ),
        "doubled",
        "20.00 eur",
    );
}

/// Regression: qualified pin to early dep version must NOT silently bind to a
/// later body. Two finance versions with incompatible types (v1=eur only,
/// v2=eur+usd). Consumer pins to v1 via `uses f: finance 2025-02-01`.
/// Evaluating with `usd` in a later slice must veto (v1 has no usd).
#[test]
fn qualified_pin_must_not_leak_later_version_types() {
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
 -> decimals 2

spec finance 2025-07-01
data money: measure
 -> unit eur: 1.00
 -> unit usd: 0.91
 -> decimals 2
"#
            .to_string(),
        )])
        .unwrap();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("shop.lemma"))),
            r#"
spec shop 2025-01-01
uses f: finance 2025-02-01
data money: f.money
data price: money
rule doubled: price * 2
"#
            .to_string(),
        )])
        .unwrap();

    // Evaluate after boundary with usd — must veto because pin locks to v1 (no usd).
    let effective = date(2025, 9, 1);
    let response = engine
        .run(
            None,
            "shop",
            Some(&effective),
            HashMap::from([("price".to_string(), "10.00 usd".into())]),
            None,
            false,
        )
        .expect("usd override must complete with veto, not Error");
    let doubled = response.results.get("doubled").expect("doubled");
    assert!(doubled.vetoed, "usd should be rejected by pinned v1 type");
    let reason = doubled.veto_reason.as_deref().expect("veto reason");
    assert!(
        reason.contains("Unknown unit") && reason.contains("usd"),
        "veto should mention unknown unit 'usd', got: {reason}"
    );
}

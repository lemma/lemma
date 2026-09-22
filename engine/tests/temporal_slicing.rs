//! Integration tests for temporal slicing of execution plans.
//!
//! When a spec's active range spans dependency spec boundaries,
//! planning must produce one ExecutionPlan per temporal slice and each
//! slice must independently validate.

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

fn eval(engine: &Engine, spec_name: &str, effective: &DateTimeValue) -> lemma::Response {
    engine
        .run(
            None,
            spec_name,
            Some(effective),
            HashMap::new(),
            None,
            false,
        )
        .unwrap()
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

// ============================================================================
// 1. SINGLE DEPENDENCY — NO VERSIONING
// ============================================================================

#[test]
fn single_unversioned_dependency() {
    let mut engine = Engine::new();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "config.lemma",
            ))),
            "spec config\ndata base_rate: 100".to_string(),
        )])
        .unwrap();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "pricing.lemma",
            ))),
            r#"
spec pricing 2025-01-01
uses cfg: config
rule rate: cfg.base_rate * 2
"#
            .to_string(),
        )])
        .unwrap();

    for d in [date(2025, 1, 15), date(2025, 7, 1), date(2025, 12, 15)] {
        assert_rule_value(&eval(&engine, "pricing", &d), "rate", "200");
    }
}

// ============================================================================
// 2. SINGLE DEPENDENCY — ONE VERSION BOUNDARY
// ============================================================================

#[test]
fn one_boundary_produces_two_slices() {
    let mut engine = Engine::new();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "config.lemma",
            ))),
            r#"
spec config
data base_rate: 100

spec config 2025-04-01
data base_rate: 200
"#
            .to_string(),
        )])
        .unwrap();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "pricing.lemma",
            ))),
            r#"
spec pricing 2025-01-01
uses cfg: config
rule rate: cfg.base_rate
"#
            .to_string(),
        )])
        .unwrap();

    assert_rule_value(&eval(&engine, "pricing", &date(2025, 2, 1)), "rate", "100");
    assert_rule_value(&eval(&engine, "pricing", &date(2025, 3, 31)), "rate", "100");
    assert_rule_value(&eval(&engine, "pricing", &date(2025, 4, 1)), "rate", "200");
    assert_rule_value(&eval(&engine, "pricing", &date(2025, 6, 15)), "rate", "200");
}

#[test]
fn boundary_exactly_at_spec_effective_from_no_split() {
    let mut engine = Engine::new();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "config.lemma",
            ))),
            r#"
spec config
data rate: 50

spec config 2025-01-01
data rate: 75
"#
            .to_string(),
        )])
        .unwrap();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "pricing.lemma",
            ))),
            r#"
spec pricing 2025-01-01
uses cfg: config
rule rate: cfg.rate
"#
            .to_string(),
        )])
        .unwrap();

    assert_rule_value(&eval(&engine, "pricing", &date(2025, 1, 1)), "rate", "75");
    assert_rule_value(&eval(&engine, "pricing", &date(2025, 5, 1)), "rate", "75");
}

// ============================================================================
// 3. SINGLE DEPENDENCY — MULTIPLE VERSION BOUNDARIES
// ============================================================================

#[test]
fn three_versions_produce_three_slices() {
    let mut engine = Engine::new();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("rates.lemma"))),
            r#"
spec rates
data rate: 10

spec rates 2025-03-01
data rate: 20

spec rates 2025-07-01
data rate: 30
"#
            .to_string(),
        )])
        .unwrap();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "pricing.lemma",
            ))),
            r#"
spec pricing 2025-01-01
uses r: rates
data quantity: number
rule total: quantity * r.rate
"#
            .to_string(),
        )])
        .unwrap();

    let cases = [
        (date(2025, 2, 1), "100"), // v1: 10*10
        (date(2025, 5, 1), "200"), // v2: 10*20
        (date(2025, 9, 1), "300"), // v3: 10*30
    ];
    for (d, expected) in &cases {
        assert_rule_value(
            &eval_with(&engine, "pricing", d, vec![("quantity", "10")]),
            "total",
            expected,
        );
    }
}

#[test]
fn four_versions_only_two_boundaries_inside_range() {
    let mut engine = Engine::new();

    // v1: [-∞, Mar), v2: [Mar, Jun), v3: [Jun, Sep), v4: [Sep, +∞)
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("rates.lemma"))),
            r#"
spec rates
data rate: 10

spec rates 2025-03-01
data rate: 20

spec rates 2025-06-01
data rate: 30

spec rates 2025-09-01
data rate: 40
"#
            .to_string(),
        )])
        .unwrap();

    // pricing active [Apr, +∞) → Jun and Sep boundaries are inside
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "pricing.lemma",
            ))),
            r#"
spec pricing 2025-04-01
uses r: rates
rule rate: r.rate
"#
            .to_string(),
        )])
        .unwrap();

    assert_rule_value(&eval(&engine, "pricing", &date(2025, 4, 15)), "rate", "20");
    assert_rule_value(&eval(&engine, "pricing", &date(2025, 5, 31)), "rate", "20");
    assert_rule_value(&eval(&engine, "pricing", &date(2025, 6, 1)), "rate", "30");
    assert_rule_value(&eval(&engine, "pricing", &date(2025, 7, 15)), "rate", "30");
}

// ============================================================================
// 4. MULTIPLE DEPENDENCIES — INDEPENDENT BOUNDARIES
// ============================================================================

#[test]
fn two_deps_boundaries_at_different_times() {
    let mut engine = Engine::new();

    // tax_rates: boundary at April
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "tax_rates.lemma",
            ))),
            r#"
spec tax_rates
data vat: 19

spec tax_rates 2025-04-01
data vat: 21
"#
            .to_string(),
        )])
        .unwrap();

    // shipping_rates: boundary at July
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "shipping_rates.lemma",
            ))),
            r#"
spec shipping_rates
data fee: 5

spec shipping_rates 2025-07-01
data fee: 8
"#
            .to_string(),
        )])
        .unwrap();

    // invoice: depends on both → boundaries at {April, July} → three slices
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "invoice.lemma",
            ))),
            r#"
spec invoice 2025-01-01
uses tax: tax_rates
uses shipping: shipping_rates
data price: number
rule vat_amount: price * tax.vat / 100
rule shipping_fee: shipping.fee
rule total: price + vat_amount + shipping_fee
"#
            .to_string(),
        )])
        .unwrap();

    // Slice 1: [Jan, Apr) — vat=19, fee=5
    let r = eval_with(
        &engine,
        "invoice",
        &date(2025, 2, 1),
        vec![("price", "100")],
    );
    assert_rule_value(&r, "vat_amount", "19");
    assert_rule_value(&r, "shipping_fee", "5");
    assert_rule_value(&r, "total", "124");

    // Slice 2: [Apr, Jul) — vat=21, fee=5
    let r = eval_with(
        &engine,
        "invoice",
        &date(2025, 5, 1),
        vec![("price", "100")],
    );
    assert_rule_value(&r, "vat_amount", "21");
    assert_rule_value(&r, "shipping_fee", "5");
    assert_rule_value(&r, "total", "126");

    // Slice 3: [Jul, +∞) — vat=21, fee=8
    let r = eval_with(
        &engine,
        "invoice",
        &date(2025, 9, 1),
        vec![("price", "100")],
    );
    assert_rule_value(&r, "vat_amount", "21");
    assert_rule_value(&r, "shipping_fee", "8");
    assert_rule_value(&r, "total", "129");
}

#[test]
fn two_deps_boundaries_at_same_time() {
    let mut engine = Engine::new();

    // Both deps change on April 1
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "tax_rates.lemma",
            ))),
            r#"
spec tax_rates
data vat: 19

spec tax_rates 2025-04-01
data vat: 21
"#
            .to_string(),
        )])
        .unwrap();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "shipping_rates.lemma",
            ))),
            r#"
spec shipping_rates
data fee: 5

spec shipping_rates 2025-04-01
data fee: 8
"#
            .to_string(),
        )])
        .unwrap();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "invoice.lemma",
            ))),
            r#"
spec invoice 2025-01-01
uses tax: tax_rates
uses shipping: shipping_rates
rule combined: tax.vat + shipping.fee
"#
            .to_string(),
        )])
        .unwrap();

    // Coincident boundary → two slices, not three
    assert_rule_value(
        &eval(&engine, "invoice", &date(2025, 2, 1)),
        "combined",
        "24",
    );
    assert_rule_value(
        &eval(&engine, "invoice", &date(2025, 6, 1)),
        "combined",
        "29",
    );
}

#[test]
fn one_dep_versioned_one_dep_unversioned() {
    let mut engine = Engine::new();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "constants.lemma",
            ))),
            r#"
spec constants
data pi: 3
"#
            .to_string(),
        )])
        .unwrap();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("rates.lemma"))),
            r#"
spec rates
data multiplier: 2

spec rates 2025-06-01
data multiplier: 4
"#
            .to_string(),
        )])
        .unwrap();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("calc.lemma"))),
            r#"
spec calc 2025-01-01
uses c: constants
uses r: rates
rule result: c.pi * r.multiplier
"#
            .to_string(),
        )])
        .unwrap();

    assert_rule_value(&eval(&engine, "calc", &date(2025, 3, 1)), "result", "6");
    assert_rule_value(&eval(&engine, "calc", &date(2025, 9, 1)), "result", "12");
}

// ============================================================================
// 5. TRANSITIVE DEPENDENCIES
// ============================================================================

#[test]
fn transitive_two_levels_deep() {
    let mut engine = Engine::new();

    // SpecC: two specs
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "base_rates.lemma",
            ))),
            r#"
spec base_rates
data multiplier: 2

spec base_rates 2025-06-01
data multiplier: 3
"#
            .to_string(),
        )])
        .unwrap();

    // SpecB: unversioned, depends on SpecC
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "intermediate.lemma",
            ))),
            r#"
spec intermediate
uses base: base_rates
data value: 10
rule adjusted: value * base.multiplier
"#
            .to_string(),
        )])
        .unwrap();

    // SpecA: depends on SpecB → transitively on SpecC
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("top.lemma"))),
            r#"
spec top 2025-01-01
uses mid: intermediate
rule result: mid.adjusted
"#
            .to_string(),
        )])
        .unwrap();

    assert_rule_value(&eval(&engine, "top", &date(2025, 3, 1)), "result", "20");
    assert_rule_value(&eval(&engine, "top", &date(2025, 9, 1)), "result", "30");
}

#[test]
fn transitive_both_levels_versioned() {
    let mut engine = Engine::new();

    // SpecC: boundary at June
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("deep.lemma"))),
            r#"
spec deep
data factor: 2

spec deep 2025-06-01
data factor: 5
"#
            .to_string(),
        )])
        .unwrap();

    // SpecB: boundary at April, depends on SpecC
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "middle.lemma",
            ))),
            r#"
spec middle
uses d: deep
data base: 10
rule value: base * d.factor

spec middle 2025-04-01
uses d: deep
data base: 100
rule value: base * d.factor
"#
            .to_string(),
        )])
        .unwrap();

    // SpecA: active from Jan → boundaries at {April, June} → three slices
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("top.lemma"))),
            r#"
spec top 2025-01-01
uses m: middle
rule result: m.value
"#
            .to_string(),
        )])
        .unwrap();

    // [Jan, Apr): middle v1 (base=10) * deep v1 (factor=2) = 20
    assert_rule_value(&eval(&engine, "top", &date(2025, 2, 1)), "result", "20");
    // [Apr, Jun): middle v2 (base=100) * deep v1 (factor=2) = 200
    assert_rule_value(&eval(&engine, "top", &date(2025, 5, 1)), "result", "200");
    // [Jun, +∞): middle v2 (base=100) * deep v2 (factor=5) = 500
    assert_rule_value(&eval(&engine, "top", &date(2025, 9, 1)), "result", "500");
}

#[test]
fn diamond_dependency_single_boundary() {
    let mut engine = Engine::new();

    // Shared dep at the bottom
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "shared.lemma",
            ))),
            r#"
spec shared
data value: 10

spec shared 2025-06-01
data value: 20
"#
            .to_string(),
        )])
        .unwrap();

    // Two intermediate specs both depend on shared
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("left.lemma"))),
            r#"
spec left_branch
uses s: shared
rule doubled: s.value * 2
"#
            .to_string(),
        )])
        .unwrap();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("right.lemma"))),
            r#"
spec right_branch
uses s: shared
rule tripled: s.value * 3
"#
            .to_string(),
        )])
        .unwrap();

    // Top depends on both branches (diamond through shared)
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("top.lemma"))),
            r#"
spec top 2025-01-01
uses l: left_branch
uses r: right_branch
rule total: l.doubled + r.tripled
"#
            .to_string(),
        )])
        .unwrap();

    // Before June: 10*2 + 10*3 = 50
    assert_rule_value(&eval(&engine, "top", &date(2025, 3, 1)), "total", "50");
    // After June: 20*2 + 20*3 = 100
    assert_rule_value(&eval(&engine, "top", &date(2025, 9, 1)), "total", "100");
}

#[test]
fn diamond_dependency_boundaries_at_different_levels() {
    let mut engine = Engine::new();

    // Shared base: boundary at August
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "shared.lemma",
            ))),
            r#"
spec shared
data base: 10

spec shared 2025-08-01
data base: 50
"#
            .to_string(),
        )])
        .unwrap();

    // Left branch: boundary at April
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("left.lemma"))),
            r#"
spec left
uses s: shared
data add: 1
rule result: s.base + add

spec left 2025-04-01
uses s: shared
data add: 2
rule result: s.base + add
"#
            .to_string(),
        )])
        .unwrap();

    // Right branch: unversioned
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("right.lemma"))),
            r#"
spec right
uses s: shared
rule result: s.base * 2
"#
            .to_string(),
        )])
        .unwrap();

    // Top: active from Jan → boundaries at {April, August} → three slices
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("top.lemma"))),
            r#"
spec top 2025-01-01
uses l: left
uses r: right
rule total: l.result + r.result
"#
            .to_string(),
        )])
        .unwrap();

    // [Jan, Apr): left v1 (10+1=11) + right (10*2=20) = 31
    assert_rule_value(&eval(&engine, "top", &date(2025, 2, 1)), "total", "31");
    // [Apr, Aug): left v2 (10+2=12) + right (10*2=20) = 32
    assert_rule_value(&eval(&engine, "top", &date(2025, 5, 1)), "total", "32");
    // [Aug, +∞): left v2 (50+2=52) + right (50*2=100) = 152
    assert_rule_value(&eval(&engine, "top", &date(2025, 9, 1)), "total", "152");
}

// ============================================================================
// 6. UNRANGED DOC (−∞ to +∞) WITH VERSIONED DEPENDENCY
// ============================================================================

#[test]
fn unranged_spec_sliced_by_versioned_dep() {
    let mut engine = Engine::new();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("rates.lemma"))),
            r#"
spec rates
data tax: 19

spec rates 2026-01-01
data tax: 21
"#
            .to_string(),
        )])
        .unwrap();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "calculator.lemma",
            ))),
            r#"
spec calculator
uses r: rates
data income: number
rule tax_amount: income * r.tax / 100
"#
            .to_string(),
        )])
        .unwrap();

    assert_rule_value(
        &eval_with(
            &engine,
            "calculator",
            &date(2025, 6, 1),
            vec![("income", "1000")],
        ),
        "tax_amount",
        "190",
    );
    assert_rule_value(
        &eval_with(
            &engine,
            "calculator",
            &date(2026, 6, 1),
            vec![("income", "1000")],
        ),
        "tax_amount",
        "210",
    );
}

// ============================================================================
// 7. DEPENDENCY COVERAGE GAPS — PLANNING ERRORS
// ============================================================================

#[test]
fn three_versions_seamlessly_chained() {
    let mut engine = Engine::new();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "policy.lemma",
            ))),
            r#"
spec policy 2025-01-01
data limit: 1000

spec policy 2025-04-01
data limit: 2000

spec policy 2025-08-01
data limit: 3000
"#
            .to_string(),
        )])
        .unwrap();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "contract.lemma",
            ))),
            r#"
spec contract 2025-01-01
uses p: policy
data amount: number
rule under_limit: amount < p.limit
"#
            .to_string(),
        )])
        .unwrap();

    // 1500 < 1000 = false, 1500 < 2000 = true, 1500 < 3000 = true
    assert_rule_value(
        &eval_with(
            &engine,
            "contract",
            &date(2025, 2, 1),
            vec![("amount", "1500")],
        ),
        "under_limit",
        "false",
    );
    assert_rule_value(
        &eval_with(
            &engine,
            "contract",
            &date(2025, 5, 1),
            vec![("amount", "1500")],
        ),
        "under_limit",
        "true",
    );
    assert_rule_value(
        &eval_with(
            &engine,
            "contract",
            &date(2025, 9, 1),
            vec![("amount", "1500")],
        ),
        "under_limit",
        "true",
    );
}

// ============================================================================
// 9. SAME DOC EVALUATED AT DIFFERENT TIMES — CORRECT SLICE SELECTED
// ============================================================================

#[test]
fn evaluate_at_boundary_instant_uses_new_version() {
    let mut engine = Engine::new();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("dep.lemma"))),
            r#"
spec dep
data val: 1

spec dep 2025-06-01
data val: 2
"#
            .to_string(),
        )])
        .unwrap();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("main.lemma"))),
            r#"
spec main
uses d: dep
rule result: d.val
"#
            .to_string(),
        )])
        .unwrap();

    // Exactly at the boundary: effective_from is inclusive
    assert_rule_value(&eval(&engine, "main", &date(2025, 5, 31)), "result", "1");
    assert_rule_value(&eval(&engine, "main", &date(2025, 6, 1)), "result", "2");
}

// ============================================================================
// 10. DEPENDENT SPEC (SpecC) REFERENCES VERSIONED SPEC (SpecB)
// ============================================================================

#[test]
fn third_level_spec_depends_on_versioned_spec() {
    let mut engine = Engine::new();

    // SpecB: versioned
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("rates.lemma"))),
            r#"
spec rates
data base: 100

spec rates 2025-05-01
data base: 200
"#
            .to_string(),
        )])
        .unwrap();

    // SpecA: unversioned, depends on SpecB
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "policy.lemma",
            ))),
            r#"
spec policy
uses r: rates
rule threshold: r.base * 2
"#
            .to_string(),
        )])
        .unwrap();

    // SpecC: depends on SpecA (which transitively depends on SpecB)
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "contract.lemma",
            ))),
            r#"
spec contract 2025-01-01
uses p: policy
data amount: number
rule is_over_threshold: amount > p.threshold
"#
            .to_string(),
        )])
        .unwrap();

    // Before May: threshold = 100*2 = 200. amount=250 > 200 → true
    assert_rule_value(
        &eval_with(
            &engine,
            "contract",
            &date(2025, 3, 1),
            vec![("amount", "250")],
        ),
        "is_over_threshold",
        "true",
    );
    // After May: threshold = 200*2 = 400. amount=250 > 400 → false
    assert_rule_value(
        &eval_with(
            &engine,
            "contract",
            &date(2025, 9, 1),
            vec![("amount", "250")],
        ),
        "is_over_threshold",
        "false",
    );
}

// ============================================================================
// 11. COMPLEX SCENARIO — REALISTIC MULTI-DOC WITH MULTIPLE BOUNDARIES
// ============================================================================

#[test]
fn realistic_tax_and_labor_law_scenario() {
    let mut engine = Engine::new();

    // Tax rates: change April 1
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("tax.lemma"))),
            r#"
spec tax_law
data income_tax_rate: 30

spec tax_law 2025-04-01
data income_tax_rate: 32
"#
            .to_string(),
        )])
        .unwrap();

    // Labor law: change July 1
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("labor.lemma"))),
            r#"
spec labor_law
data min_wage_hourly: 12
data max_weekly_hours: 40

spec labor_law 2025-07-01
data min_wage_hourly: 15
data max_weekly_hours: 38
"#
            .to_string(),
        )])
        .unwrap();

    // Employment contract: depends on both, active from Jan
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "employment.lemma",
            ))),
            r#"
spec employment 2025-01-01
uses tax: tax_law
uses labor: labor_law
data hourly_rate: number
data weekly_hours: number

rule annual_gross: hourly_rate * weekly_hours * 52
rule annual_tax: annual_gross * tax.income_tax_rate / 100
rule annual_net: annual_gross - annual_tax
rule min_annual_gross: labor.min_wage_hourly * labor.max_weekly_hours * 52
rule meets_minimum: annual_gross >= min_annual_gross
"#
            .to_string(),
        )])
        .unwrap();

    let data = vec![("hourly_rate", "20"), ("weekly_hours", "40")];

    // Slice [Jan, Apr): tax=30%, labor v1 (min=12, max_h=40)
    let r = eval_with(&engine, "employment", &date(2025, 2, 1), data.clone());
    assert_rule_value(&r, "annual_gross", "41600");
    assert_rule_value(&r, "annual_tax", "12480");
    assert_rule_value(&r, "annual_net", "29120");
    assert_rule_value(&r, "min_annual_gross", "24960");
    assert_rule_value(&r, "meets_minimum", "true");

    // Slice [Apr, Jul): tax=32%, labor v1 (min=12, max_h=40)
    let r = eval_with(&engine, "employment", &date(2025, 5, 1), data.clone());
    assert_rule_value(&r, "annual_tax", "13312");
    assert_rule_value(&r, "annual_net", "28288");
    assert_rule_value(&r, "min_annual_gross", "24960");

    // Slice [Jul, +∞): tax=32%, labor v2 (min=15, max_h=38)
    let r = eval_with(&engine, "employment", &date(2025, 9, 1), data.clone());
    assert_rule_value(&r, "annual_tax", "13312");
    assert_rule_value(&r, "min_annual_gross", "29640");
    assert_rule_value(&r, "meets_minimum", "true");
}

// ============================================================================
// 12. SELF-VERSIONED DOC REFERENCING VERSIONED DEP
// ============================================================================

#[test]
fn both_spec_and_dep_are_versioned() {
    let mut engine = Engine::new();

    // dep: boundary at June
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("dep.lemma"))),
            r#"
spec dep
data val: 10

spec dep 2025-06-01
data val: 20
"#
            .to_string(),
        )])
        .unwrap();

    // main v1: [Jan, Apr), main v2: [Apr, +∞)
    // main v1's range [Jan, Apr) has no dep boundary → single slice for v1
    // main v2's range [Apr, +∞) has dep boundary at June → two slices for v2
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("main.lemma"))),
            r#"
spec main 2025-01-01
uses d: dep
data multiplier: 2
rule result: d.val * multiplier

spec main 2025-04-01
uses d: dep
data multiplier: 3
rule result: d.val * multiplier
"#
            .to_string(),
        )])
        .unwrap();

    // main v1 [Jan, Apr): dep v1 (10) * 2 = 20
    assert_rule_value(&eval(&engine, "main", &date(2025, 2, 1)), "result", "20");
    // main v2 [Apr, Jun): dep v1 (10) * 3 = 30
    assert_rule_value(&eval(&engine, "main", &date(2025, 5, 1)), "result", "30");
    // main v2 [Jun, +∞): dep v2 (20) * 3 = 60
    assert_rule_value(&eval(&engine, "main", &date(2025, 9, 1)), "result", "60");
}

// ============================================================================
// 13. INTERFACE CONSISTENCY — TEMPORAL VALIDATION
// ============================================================================
// Different specs in a dependency set CAN have different interfaces.
// Every slice must resolve to a spec that satisfies what the
// dependent spec actually references (per-slice interface validation).

#[test]
fn dep_version_removes_referenced_data_rejected() {
    let mut engine = Engine::new();

    // config v1 has base_rate. config v2 renames it to cost.
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "config.lemma",
            ))),
            r#"
spec config
data base_rate: 100

spec config 2025-04-01
data cost: 200
"#
            .to_string(),
        )])
        .unwrap();

    // pricing references config.base_rate → v2 slice fails (no base_rate in v2)
    let result = engine.load([(
        lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
            "pricing.lemma",
        ))),
        r#"
spec pricing 2025-01-01
uses cfg: config
rule rate: cfg.base_rate
"#
        .to_string(),
    )]);

    assert!(
        result.is_err(),
        "Must reject: config v2 (April+) removed 'base_rate' that pricing references"
    );
    let errs = result.unwrap_err();
    assert!(
        errs.iter().any(|e| e.to_string().contains("base_rate")),
        "Error should mention missing 'base_rate'. Got: {}",
        errs.iter()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
}

#[test]
fn dep_version_adds_rule_that_other_spec_doesnt_use_accepted() {
    let mut engine = Engine::new();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "policy.lemma",
            ))),
            r#"
spec policy
data base: 100
rule discount: 10

spec policy 2025-06-01
data base: 200
rule discount: 20
rule bonus: 5
"#
            .to_string(),
        )])
        .unwrap();

    // contract only references policy.discount — bonus is irrelevant
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "contract.lemma",
            ))),
            r#"
spec contract 2025-01-01
uses p: policy
rule applied_discount: p.discount
"#
            .to_string(),
        )])
        .unwrap();

    assert_rule_value(
        &eval(&engine, "contract", &date(2025, 3, 1)),
        "applied_discount",
        "10",
    );
    assert_rule_value(
        &eval(&engine, "contract", &date(2025, 9, 1)),
        "applied_discount",
        "20",
    );
}

// ============================================================================
// SLICE INTERFACE — CATEGORY 1: COMPATIBLE INTERFACES (planning succeeds)
// ============================================================================

#[test]
fn slice_compat_dep_adds_unreferenced_data() {
    let mut engine = Engine::new();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "settings.lemma",
            ))),
            r#"
spec settings
data limit: 10

spec settings 2025-05-01
data limit: 20
data description: "updated settings"
data extra_number: 999
"#
            .to_string(),
        )])
        .unwrap();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("app.lemma"))),
            r#"
spec app 2025-01-01
uses s: settings
rule max: s.limit
"#
            .to_string(),
        )])
        .unwrap();

    assert_rule_value(&eval(&engine, "app", &date(2025, 3, 1)), "max", "10");
    assert_rule_value(&eval(&engine, "app", &date(2025, 8, 1)), "max", "20");
}

#[test]
fn slice_compat_dep_adds_unreferenced_rules() {
    let mut engine = Engine::new();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("calc.lemma"))),
            r#"
spec calc
rule base_fee: 100

spec calc 2025-04-01
rule base_fee: 150
rule surcharge: 25
"#
            .to_string(),
        )])
        .unwrap();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "invoice.lemma",
            ))),
            r#"
spec invoice 2025-01-01
uses c: calc
rule fee: c.base_fee
"#
            .to_string(),
        )])
        .unwrap();

    assert_rule_value(&eval(&engine, "invoice", &date(2025, 2, 1)), "fee", "100");
    assert_rule_value(&eval(&engine, "invoice", &date(2025, 6, 1)), "fee", "150");
}

/// Dep changes data interface across time (number → text). Dependent adds temporal specs
/// so each era only binds to the matching dep slice. Target: load + eval succeed; if not,
/// validation is too strict (e.g. one consumer spec checked against every dep slice).
#[test]
fn dependent_versions_track_dep_interface_change() {
    let mut engine = Engine::new();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("t.lemma"))),
            r#"
spec dep
data x: 10

spec dep 2025-06-01
data x: "hello"

spec consumer 2025-01-01
uses d: dep
rule val: d.x

spec consumer 2025-06-01
uses d: dep
rule val: d.x
"#
            .to_string(),
        )])
        .unwrap();

    assert_rule_value(&eval(&engine, "consumer", &date(2025, 3, 1)), "val", "10");
    assert_rule_value(
        &eval(&engine, "consumer", &date(2025, 9, 1)),
        "val",
        "hello",
    );
}

#[test]
fn app_temporal_versions_distinct_rules_per_dep_interface_era() {
    let mut engine = Engine::new();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "app_dep.lemma",
            ))),
            r#"
spec dep
data x: 10

spec dep 2025-06-01
data x: "hello"

spec app 2025-01-01
uses d: dep
rule total: d.x + 2

spec app 2025-06-01
uses d: dep
rule greeting: d.x
"#
            .to_string(),
        )])
        .unwrap();

    assert_rule_value(&eval(&engine, "app", &date(2025, 3, 1)), "total", "12");
    assert_rule_value(
        &eval(&engine, "app", &date(2025, 9, 1)),
        "greeting",
        "hello",
    );
}

#[test]
fn slice_compat_dep_rule_value_changes_but_type_same() {
    let mut engine = Engine::new();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "policy.lemma",
            ))),
            r#"
spec policy
rule discount: 10

spec policy 2025-05-01
rule discount: 25
"#
            .to_string(),
        )])
        .unwrap();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("shop.lemma"))),
            r#"
spec shop 2025-01-01
uses p: policy
rule d: p.discount
"#
            .to_string(),
        )])
        .unwrap();

    assert_rule_value(&eval(&engine, "shop", &date(2025, 2, 1)), "d", "10");
    assert_rule_value(&eval(&engine, "shop", &date(2025, 8, 1)), "d", "25");
}

#[test]
fn slice_compat_dep_data_type_annotation_identical() {
    let mut engine = Engine::new();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("cfg.lemma"))),
            r#"
spec cfg
data threshold: number

spec cfg 2025-04-01
data threshold: number
"#
            .to_string(),
        )])
        .unwrap();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "consumer.lemma",
            ))),
            r#"
spec consumer 2025-01-01
uses c: cfg
  -> with threshold: 50
rule t: c.threshold
"#
            .to_string(),
        )])
        .unwrap();

    assert_rule_value(&eval(&engine, "consumer", &date(2025, 2, 1)), "t", "50");
    assert_rule_value(&eval(&engine, "consumer", &date(2025, 6, 1)), "t", "50");
}

// ============================================================================
// SLICE INTERFACE — CATEGORY 2: INCOMPATIBLE INTERFACES (planning rejects)
// ============================================================================

#[test]
fn slice_incompat_type_change_skipping_middle_slice() {
    // The unified-surface check must catch a type change between two slices
    // even when an intermediate slice does not expose the name at all:
    // pairwise adjacent comparison would miss it (compatibility of shared
    // names is not transitive through a slice that shares nothing).
    let mut engine = Engine::new();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "skipping.lemma",
            ))),
            r#"
spec skipping_dep
data x: 10

spec skipping_dep 2025-06-01
data y: 5

spec skipping_dep 2025-09-01
data x: "ten"
data y: 5
"#
            .to_string(),
        )])
        .unwrap();

    let result = engine.load([(
        lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
            "consumer.lemma",
        ))),
        r#"
spec consumer 2025-01-01
uses d: skipping_dep
rule sy: d.y
"#
        .to_string(),
    )]);

    assert!(
        result.is_err(),
        "Must reject: skipping_dep.x changes from number to text across the \
         middle slice that does not expose x"
    );
    let errs = result.unwrap_err();
    let joined = errs
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(" | ");
    assert!(
        joined.contains("changed its interface between temporal slices"),
        "Error must come from SliceInterface validation. Got: {}",
        joined
    );
}

#[test]
fn slice_incompat_multiple_deps_one_unstable() {
    let mut engine = Engine::new();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "stable.lemma",
            ))),
            r#"
spec stable_dep
data x: 10

spec stable_dep 2025-06-01
data x: 20
"#
            .to_string(),
        )])
        .unwrap();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "unstable.lemma",
            ))),
            r#"
spec unstable_dep
data y: 5

spec unstable_dep 2025-06-01
data y: "five"
"#
            .to_string(),
        )])
        .unwrap();

    let result = engine.load([(
        lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
            "consumer.lemma",
        ))),
        r#"
spec consumer 2025-01-01
uses a: stable_dep
uses b: unstable_dep
rule sx: a.x
rule sy: b.y
"#
        .to_string(),
    )]);

    assert!(
        result.is_err(),
        "Must reject: unstable_dep.y changes from number to text"
    );
    let errs = result.unwrap_err();
    let joined = errs
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(" | ");
    assert!(
        joined.contains("changed its interface between temporal slices"),
        "Error must come from SliceInterface validation. Got: {}",
        joined
    );
    assert!(
        joined.contains("unstable_dep"),
        "Error must identify unstable_dep as the changed dependency. Got: {}",
        joined
    );
}

// ============================================================================
// SLICE INTERFACE — CATEGORY 3: EDGE CASES & BOUNDARY CONDITIONS
// ============================================================================

#[test]
fn slice_edge_data_referenced_only_in_unless_branch() {
    let mut engine = Engine::new();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("dep.lemma"))),
            r#"
spec dep
data main_val: 10
data alt_val: 20

spec dep 2025-06-01
data main_val: "ten"
data alt_val: "twenty"
"#
            .to_string(),
        )])
        .unwrap();

    // alt_val only appears in the unless then-branch, not the default.
    // Both slices are individually valid (branches have matching types within
    // each slice), but the interface changes across slices.
    let result = engine.load([(
        lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
            "caller.lemma",
        ))),
        r#"
spec caller 2025-01-01
uses d: dep
data use_alt: boolean
rule result: d.main_val
 unless use_alt then d.alt_val
"#
        .to_string(),
    )]);

    assert!(
        result.is_err(),
        "Must reject: dep data change from number to text across slices"
    );
    let errs = result.unwrap_err();
    let joined = errs
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(" | ");
    assert!(
        joined.contains("changed its interface between temporal slices"),
        "Error must come from SliceInterface validation. Got: {}",
        joined
    );
}

#[test]
fn slice_edge_data_in_both_condition_and_expression() {
    let mut engine = Engine::new();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "amounts.lemma",
            ))),
            r#"
spec amounts
data amount: 100

spec amounts 2025-05-01
data amount: 200
"#
            .to_string(),
        )])
        .unwrap();

    // amount appears in both the unless condition and the then expression
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("calc.lemma"))),
            r#"
spec calc 2025-01-01
uses d: amounts
rule result: 0
 unless d.amount > 50 then d.amount * 2
"#
            .to_string(),
        )])
        .unwrap();

    assert_rule_value(&eval(&engine, "calc", &date(2025, 3, 1)), "result", "200");
    assert_rule_value(&eval(&engine, "calc", &date(2025, 8, 1)), "result", "400");
}

#[test]
fn slice_edge_dep_removes_referenced_rule_caught_by_per_slice() {
    let mut engine = Engine::new();

    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("svc.lemma"))),
            r#"
spec svc
rule compute: 42

spec svc 2025-06-01
rule other_compute: 99
"#
            .to_string(),
        )])
        .unwrap();

    let result = engine.load([(
        lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
            "caller.lemma",
        ))),
        r#"
spec caller 2025-01-01
uses s: svc
rule val: s.compute
"#
        .to_string(),
    )]);

    assert!(
        result.is_err(),
        "Must reject: svc v2 no longer has rule 'compute' that caller references"
    );
    let errs = result.unwrap_err();
    let joined = errs
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(" | ");
    assert!(
        joined.contains("compute"),
        "Error should mention the missing rule 'compute'. Got: {}",
        joined
    );
}

// ============================================================================
// ADVERSARIAL — half-open range boundaries (ranges_overlap semantics)
// ============================================================================

/// Consumer `[2025-01-01, 2025-06-01)` must not treat dep body starting exactly at
/// consumer end as covering the consumer.
#[test]
fn adversarial_consumer_range_end_exclusive_dep_starting_at_end_not_covering() {
    let mut engine = Engine::new();
    let err = engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("adv.lemma"))),
            r#"
spec consumer 2025-01-01
uses d: dep
rule x: d.v

spec dep 2025-06-01
data v: 1
"#
            .to_string(),
        )])
        .expect_err("dep must not cover consumer range ending at 2025-06-01");

    let joined = err
        .errors
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        joined.contains("consumer") && joined.contains("dep"),
        "expected coverage error; got {joined}"
    );
}

/// Consumer starts before any dep body exists (half-open coverage).
#[test]
fn adversarial_consumer_starts_before_dep_first_effective_errors() {
    let mut engine = Engine::new();
    let err = engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("adv2.lemma"))),
            r#"
spec consumer 2025-03-01
uses d: dep
rule x: d.v

spec dep 2025-08-01
data v: 1
"#
            .to_string(),
        )])
        .expect_err("gap before dep exists");

    let joined = err
        .errors
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(joined.contains("dep"), "got {joined}");
}

// ============================================================================
// ADVERSARIAL — slice interface matrix (distinct failure shapes)
// ============================================================================

#[test]
fn slice_incompat_data_field_type_changes_number_to_text() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("dep.lemma"))),
            r#"
spec dep 2025-01-01
data rate: number

spec dep 2025-07-01
data rate: text
"#
            .to_string(),
        )])
        .unwrap();

    let err = engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("app.lemma"))),
            r#"
spec app 2025-01-01
uses d: dep
rule r: d.rate
"#
            .to_string(),
        )])
        .expect_err("interface change");

    let joined = err
        .errors
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        joined.contains("interface") || joined.contains("temporal") || joined.contains("dep"),
        "got {joined}"
    );
}

#[test]
fn slice_incompat_rule_result_type_changes() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("dep.lemma"))),
            r#"
spec dep 2025-01-01
rule discount: 5

spec dep 2025-07-01
rule discount: true
"#
            .to_string(),
        )])
        .unwrap();

    let err = engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("app.lemma"))),
            r#"
spec app 2025-01-01
uses d: dep
rule out: d.discount
"#
            .to_string(),
        )])
        .expect_err("rule type mismatch across slices");

    let joined = err
        .errors
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        joined.contains("interface") || joined.contains("dep"),
        "got {joined}"
    );
}

#[test]
fn slice_incompat_named_type_adds_unit_across_slices() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("dep.lemma"))),
            r#"
spec dep 2025-01-01
data money: measure
 -> unit eur: 1.0

spec dep 2025-07-01
data money: measure
 -> unit eur: 1.0
 -> unit usd: 1.1
"#
            .to_string(),
        )])
        .unwrap();

    let err = engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("app.lemma"))),
            r#"
spec app 2025-01-01
uses dep
data m: dep.money
rule x: 1
"#
            .to_string(),
        )])
        .expect_err("type shape change");

    let joined = err
        .errors
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        joined.contains("interface") || joined.contains("dep") || joined.contains("money"),
        "got {joined}"
    );
}

#[test]
fn slice_incompat_three_versions_middle_breaks_adjacent_pair() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("dep.lemma"))),
            r#"
spec dep 2025-01-01
data rate: number

spec dep 2025-04-01
data rate: text

spec dep 2025-10-01
data rate: number
"#
            .to_string(),
        )])
        .unwrap();

    let err = engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("app.lemma"))),
            r#"
spec app 2025-01-01
uses d: dep
rule r: d.rate
"#
            .to_string(),
        )])
        .expect_err("middle slice incompatible");

    let joined = err
        .errors
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(!joined.is_empty(), "expected errors");
}

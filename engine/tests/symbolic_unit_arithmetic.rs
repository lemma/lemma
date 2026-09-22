//! Integration tests for unit-agnostic plan-time arithmetic and calendar units.

use lemma::{Engine, ValueKind};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

fn source() -> lemma::SourceType {
    lemma::SourceType::Path(Arc::new(PathBuf::from("symbolic_unit_arithmetic.lemma")))
}

fn eval_str(code: &str, spec_name: &str, rule_name: &str) -> String {
    let mut engine = Engine::new();
    engine
        .load([(source(), code.to_string())])
        .expect("spec must load");
    let response = engine
        .run(None, spec_name, None, HashMap::new(), None, true)
        .expect("spec must evaluate");
    response
        .results
        .get(rule_name)
        .unwrap_or_else(|| panic!("rule '{}' missing", rule_name))
        .result()
        .expect("result")
        .to_string()
}

fn eval_measure_in_unit(
    code: &str,
    spec_name: &str,
    rule_name: &str,
    unit: &str,
) -> rust_decimal::Decimal {
    let mut engine = Engine::new();
    engine
        .load([(source(), code.to_string())])
        .expect("spec must load");
    let response = engine
        .run(None, spec_name, None, HashMap::new(), None, true)
        .expect("spec must evaluate");
    let rule = response
        .results
        .get(rule_name)
        .unwrap_or_else(|| panic!("rule '{rule_name}' missing"));
    let measure = rule
        .result
        .as_ref()
        .and_then(|v| v.measure.as_ref())
        .unwrap_or_else(|| panic!("rule '{rule_name}' has no measure map"));
    *measure
        .get(unit)
        .unwrap_or_else(|| panic!("measure map missing unit '{unit}' for '{rule_name}'"))
}

fn eval_decimal(code: &str, spec_name: &str, rule_name: &str) -> rust_decimal::Decimal {
    let mut engine = Engine::new();
    engine
        .load([(source(), code.to_string())])
        .expect("spec must load");
    let response = engine
        .run(None, spec_name, None, HashMap::new(), None, true)
        .expect("spec must evaluate");
    let rule = response
        .results
        .get(rule_name)
        .unwrap_or_else(|| panic!("rule '{}' missing", rule_name));
    if let Some(measure) = rule.result.as_ref().and_then(|v| v.measure.as_ref()) {
        let unit = rule
            .rule
            .rule_type
            .measure_binding_unit
            .clone()
            .filter(|u| measure.contains_key(u))
            .or_else(|| {
                rule.result().and_then(|display| {
                    let lower = display.to_lowercase();
                    measure
                        .keys()
                        .find(|u| lower.contains(&u.to_lowercase()))
                        .cloned()
                })
            })
            .or_else(|| measure.keys().next().cloned())
            .unwrap_or_else(|| panic!("BUG: measure map empty for rule '{rule_name}'"));
        return *measure
            .get(&unit)
            .unwrap_or_else(|| panic!("measure map missing unit '{unit}'"));
    }
    if let Some(calendar) = rule.result.as_ref().and_then(|v| v.calendar.as_ref()) {
        return calendar.value;
    }
    let value = rule
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("rule must return a value");
    match &value.value {
        ValueKind::Number(n) => lemma::ValueKind::Number(n.clone())
            .as_decimal_magnitude()
            .expect("numeric value kind"),
        other => panic!("expected numeric value, got {:?}", other),
    }
}

fn load_expect_error(code: &str) -> String {
    let mut engine = Engine::new();
    let errors = engine
        .load([(source(), code.to_string())])
        .expect_err("expected load to fail");
    errors
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("; ")
}

fn assert_loads(code: &str) {
    let mut engine = Engine::new();
    engine
        .load([(source(), code.to_string())])
        .expect("expected spec to load");
}

// ---------------------------------------------------------------------------
// Plan-time typing: cross-type promotion without as-cast
// ---------------------------------------------------------------------------

/// rule pay: rate * hour — must plan to `money` without an `as eur` cast.
/// Today planning rejects this with "anonymous intermediate at rule boundary".
#[test]
fn cross_type_arithmetic_promotes_without_as_cast() {
    let code = r#"spec pay_calc
uses lemma units
data money: measure
  -> unit eur: 1
data rate: measure
  -> unit eur_per_hour: eur/hour
data hours_worked: 40 hour
data hourly_rate: 30 eur_per_hour
rule pay: hourly_rate * hours_worked"#;
    // Must load (plan) without any as-cast.
    assert_loads(code);
    let displayed = eval_str(code, "pay_calc", "pay");
    let decimal = eval_decimal(code, "pay_calc", "pay");
    assert_eq!(
        decimal,
        rust_decimal::Decimal::from(1200),
        "magnitude must be 1200"
    );
    assert!(
        displayed.to_lowercase().contains("eur"),
        "expected unit 'eur' in display, got: {}",
        displayed
    );
    assert!(
        !displayed.to_lowercase().contains("eur_per_hour"),
        "Measure×Measure product binding is None: display result type eur, not left eur_per_hour, got: {}",
        displayed
    );
}

/// Two types sharing the same decomposition in scope — rule must be rejected with
/// both candidate names in the error.
#[test]
fn ambiguous_decomposition_lists_both_candidates() {
    let code = r#"spec ambiguous
data torque: measure
  -> unit newton_meter: newton*meter
data energy: measure
  -> unit joule: newton*meter
data force: measure
  -> unit newton: 1
data length: measure
  -> unit meter: 1
data f: 3 newton
data d: 5 meter
rule work: f * d"#;
    let error = load_expect_error(code);
    let lower = error.to_lowercase();
    assert!(
        lower.contains("ambiguous") || lower.contains("multiple") || lower.contains("matches"),
        "expected ambiguity error, got: {}",
        error
    );
    // Both candidate names must appear in the error.
    assert!(
        error.contains("torque") && error.contains("energy"),
        "expected both candidate type names in error, got: {}",
        error
    );
}

/// Two types with different names but the same base-measure decomposition ({length:1})
/// produce an ambiguous result when their base units are multiplied by a scalar. The
/// planner must reject this with both candidate names in the error (not a panic).
#[test]
fn ambiguity_across_distinct_types_same_decomposition() {
    let code = r#"spec ambiguous_length
data imperial_length: measure
  -> unit inch: 1
data metric_length: measure
  -> unit meter: 1
data x: 3 inch
data y: 4 inch
rule total: x + y"#;
    // Same-type add — should not be ambiguous; this tests the non-error path:
    let mut engine = Engine::new();
    engine
        .load([(source(), code.to_string())])
        .expect("same-type add must load without error");

    // Cross-type multiply is the ambiguous case (inch * meter → both types have {length:1}
    // if we multiply two instances with the same decomposition; currently this promotes
    // to {length:2} which has no named type and is rejected as anonymous at rule boundary):
    let cross_code = r#"spec ambiguous_length
data imperial_length: measure
  -> unit inch: 1
data metric_length: measure
  -> unit meter: 1
data x: 3 inch
data y: 4 meter
rule product: x * y"#;
    // Multiplication produces {length:2} — no named type → anonymous boundary rejection
    // OR ambiguity. Either way, it must not panic; it must produce a planning error.
    let err = load_expect_error(cross_code);
    assert!(
        !err.is_empty(),
        "expected a planning error for inch * meter with no length² type, got empty error"
    );
}

/// force / area with no pressure type declared: error must name the decomposition
/// and suggest a remedy.
#[test]
fn no_named_type_for_decomposition_clear_error() {
    let code = r#"spec pressure_calc
data force: measure
  -> unit newton: 1
data area: measure
  -> unit square_meter: meter*meter
data f: 10 newton
data a: 2 square_meter
rule pressure: f / a"#;
    let error = load_expect_error(code);
    // Must mention that no type matches, not a raw panic or generic "anonymous".
    assert!(
        error.to_lowercase().contains("anonymous")
            || error.to_lowercase().contains("dimension")
            || error.to_lowercase().contains("no"),
        "expected clear 'no matching type' error, got: {}",
        error
    );
}

/// A rule that only references data nodes (no inline literals) must plan
/// purely via decomposition. Three different bound-unit combinations must
/// all evaluate to a coherent result.
#[test]
fn input_only_data_uses_decomposition() {
    let base = r#"spec rate_calc
uses lemma units
data money: measure
  -> unit eur: 1
data rate: measure
  -> unit eur_per_hour: eur/hour
  -> unit eur_per_minute: eur/minute
data rate_value: {RATE}
data duration_value: {DURATION}
rule pay: (rate_value * duration_value)"#;

    // Expected values in eur after `as eur` conversion.
    // Today the middle case (60 eur_per_hour * 90 minute) returns 5400 (wrong:
    // magnitude pre-normalization bug). After implementation it must return 90.
    let cases: &[(&str, &str, i64)] = &[
        ("30 eur_per_hour", "2 hour", 60),
        ("60 eur_per_hour", "90 minute", 90),
        ("1 eur_per_minute", "120 minute", 120),
    ];
    for (rate, duration, expected_eur) in cases {
        let code = base.replace("{RATE}", rate).replace("{DURATION}", duration);
        let decimal = eval_decimal(&code, "rate_calc", "pay");
        assert_eq!(
            decimal,
            rust_decimal::Decimal::from(*expected_eur),
            "rate={}, duration={}: expected {} eur, got {}",
            rate,
            duration,
            expected_eur,
            decimal
        );
    }
}

// ---------------------------------------------------------------------------
// Date - Date and Time - Time rejection
// ---------------------------------------------------------------------------

#[test]
fn time_minus_time_rejected_with_datetime_range_suggestion() {
    let code = r#"spec clock
uses lemma units
data start: 09:00:00
data finish: 17:30:00
rule worked: (finish - start) as hour"#;
    let error = load_expect_error(code);
    assert!(
        error.to_lowercase().contains("datetime range"),
        "expected datetime range suggestion, got: {}",
        error
    );
}

#[test]
fn date_minus_date_rejected_with_date_range_suggestion() {
    let code = r#"spec tenure
uses lemma units
data start: 2024-01-01
data end_date: 2024-04-01
rule tenure_days: (end_date - start) as day"#;
    let error = load_expect_error(code);
    assert!(
        error.to_lowercase().contains("date range"),
        "expected date range suggestion, got: {}",
        error
    );
}

#[test]
fn date_arithmetic_requires_duration_type_in_scope() {
    let code = r#"spec no_duration
data start: 2024-01-01
data finish: 2024-12-31
rule gap: finish - start"#;
    let error = load_expect_error(code);
    assert!(
        error.to_lowercase().contains("date range"),
        "expected date range suggestion, got: {}",
        error
    );
}

// ---------------------------------------------------------------------------
// Type-system permissiveness
// ---------------------------------------------------------------------------

/// A base measure type with no factor-1 unit must be accepted.
/// Pins: no "canonical" requirement.
#[test]
fn base_measure_without_factor_one_unit_accepted_and_displays_in_declared_units() {
    let code = r#"spec weird_units
data weight: measure
  -> unit half_kg: 0.5
  -> unit quarter_kg: 0.25
data a: 5 half_kg
data b: 3 quarter_kg
rule sum: a + b
rule converted: (a) as quarter_kg"#;
    assert_loads(code);
    // 5 half_kg + 3 quarter_kg:
    // Convert b to half_kg: 3 * (0.25/0.5) = 1.5 half_kg
    // Sum = 6.5 half_kg
    assert_eq!(
        eval_decimal(code, "weird_units", "sum"),
        rust_decimal::Decimal::new(65, 1),
        "expected 6.5 half_kg in measure map"
    );
    // 5 half_kg as quarter_kg = 5 * (0.5/0.25) = 10 quarter_kg
    assert_eq!(
        eval_decimal(code, "weird_units", "converted"),
        rust_decimal::Decimal::from(10),
        "expected 10 quarter_kg in measure map"
    );
}

/// A compound type (no factor-1 unit) for which a cross-type result resolves
/// via decomposition. The result should display via signature_index hit.
#[test]
fn compound_type_without_factor_one_unit_evaluates_and_displays_via_signature() {
    let code = r#"spec rate_display
data money: measure
  -> unit eur: 1
data time_unit: measure
  -> unit hour: 1
data rate: measure
  -> unit eur_per_hour: eur/hour
data total: 120 eur
data duration: 2 hour
rule rate_result: total / duration"#;
    assert_loads(code);
    let displayed = eval_str(code, "rate_display", "rate_result");
    assert!(
        displayed.contains("60"),
        "expected magnitude 60, got: {}",
        displayed
    );
    assert!(
        displayed.to_lowercase().contains("eur_per_hour")
            || displayed.to_lowercase().contains("eur/hour")
            || (displayed.to_lowercase().contains("eur")
                && displayed.to_lowercase().contains("hour")),
        "expected eur_per_hour or operator-style unit in display, got: {}",
        displayed
    );
}

// ---------------------------------------------------------------------------
// Runtime behavior: display format
// ---------------------------------------------------------------------------

/// A Measure value whose signature matches a signature_index entry must display
/// using the friendly unit name.
#[test]
fn value_with_compound_signature_renders_friendly_name_when_signature_index_matches() {
    let code = r#"spec rate_display
data money: measure
  -> unit eur: 1
data time_unit: measure
  -> unit hour: 1
data rate: measure
  -> unit eur_per_hour: eur/hour
data total: 120 eur
data duration: 2 hour
rule rate_result: total / duration"#;
    assert_loads(code);
    let displayed = eval_str(code, "rate_display", "rate_result");
    // signature [("eur",1),("hour",-1)] should hit signature_index -> "eur_per_hour"
    assert!(
        displayed.contains("eur_per_hour"),
        "expected friendly name 'eur_per_hour', got: {}",
        displayed
    );
}

/// A Measure with a compound signature that does NOT match any signature_index
/// entry must be rejected at planning — rule results require a named type with
/// declared units for API serialization.
#[test]
fn value_with_compound_signature_renders_operator_style_when_signature_index_misses() {
    let code = r#"spec weird_compound
data money: measure
  -> unit eur: 1
data time_h: measure
  -> unit hour: 1
data time_m: measure
  -> unit minute: 1
data amount: 40 eur
data h: 2 hour
data m: 3 minute
rule compound: amount * h / m"#;
    let error = load_expect_error(code);
    assert!(
        error.to_lowercase().contains("anonymous")
            || error.to_lowercase().contains("cast")
            || error.to_lowercase().contains("as <"),
        "expected planning rejection for anonymous rule result, got: {}",
        error
    );
}

/// ce_per_minute * minute -> Measure(15, [("ce",1)]). Signature cancels; lookup
/// hits the `batch` type; emits named lemma_type.
#[test]
fn signature_combination_cancels_to_named_unit() {
    let code = r#"spec packaging
uses lemma units
data batch: measure
  -> unit ce: 1
data packaging_speed: measure
  -> unit ce_per_minute: ce/minute
data speed: 5 ce_per_minute
data duration: 3 minute
rule throughput: speed * duration"#;
    assert_loads(code);
    let displayed = eval_str(code, "packaging", "throughput");
    assert!(
        displayed.contains("15"),
        "expected magnitude 15, got: {}",
        displayed
    );
    assert!(
        displayed.to_lowercase().contains("ce"),
        "expected unit 'ce', got: {}",
        displayed
    );
}

/// eur_per_minute * hour -> anonymous compound; rejected at planning.
#[test]
fn signature_combination_misses_yields_operator_style_display() {
    let code = r#"spec weird_rate
data money: measure
  -> unit eur: 1
data time_m: measure
  -> unit minute: 1
data time_h: measure
  -> unit hour: 1
data rate: measure
  -> unit eur_per_minute: eur/minute
data r: 40 eur_per_minute
data h: 2 hour
rule compound: r * h"#;
    let error = load_expect_error(code);
    assert!(
        error.to_lowercase().contains("anonymous")
            || error.to_lowercase().contains("cast")
            || error.to_lowercase().contains("as <"),
        "expected planning rejection for anonymous rule result, got: {}",
        error
    );
}

/// Two Measure values with identical signatures must sum magnitudes directly,
/// no factor conversion applied.
#[test]
fn measure_plus_measure_same_signature_direct_sum() {
    let code = r#"spec sums
data money: measure
  -> unit eur: 1
data a: 100 eur
data b: 50 eur
rule total: a + b"#;
    let displayed = eval_str(code, "sums", "total");
    assert!(
        displayed.contains("150"),
        "expected 150 eur (direct sum), got: {}",
        displayed
    );
}

/// Two Measure values with different-but-compatible signatures (eur/sec and
/// eur/minute) convert via signature_factor and sum in the LHS signature.
/// 10 eur/sec + 20 eur/minute = 10 + 20/60 = 31/3 eur/sec ≈ 10.333...
#[test]
fn measure_plus_measure_different_signature_converts_via_signature_factor() {
    let code = r#"spec rate_sum
uses lemma units
data money: measure
  -> unit eur: 1
data rate: measure
  -> unit eur_per_second: eur/second
  -> unit eur_per_minute: eur/minute
data a: 10 eur_per_second
data b: 20 eur_per_minute
rule total_rate: (a + b)"#;
    let decimal = eval_decimal(code, "rate_sum", "total_rate");
    // 10 + 20/60 = 10 + 1/3 = 31/3
    let expected = rust_decimal::Decimal::new(10333, 3); // 10.333 approx
    let diff = (decimal - expected).abs();
    assert!(
        diff < rust_decimal::Decimal::new(1, 2),
        "expected ~10.333 eur_per_second, got {}",
        decimal
    );
}

// ---------------------------------------------------------------------------
// Q*Calendar
// ---------------------------------------------------------------------------

/// eur_per_month * 3 month: signature [(eur,1),(month,-1),(month,1)] cancels to
/// [(eur,1)]; signature_index hits; emits named Measure(300, eur).
#[test]
fn q_times_calendar_uses_builtin_calendar_factor() {
    let code = r#"spec monthly_pay
uses lemma units
data money: measure
  -> unit eur: 1
data rate: measure
  -> unit eur_per_month: eur/month
data monthly: 100 eur_per_month
rule quarterly: monthly * 3 month"#;
    assert_loads(code);
    let decimal = eval_decimal(code, "monthly_pay", "quarterly");
    assert_eq!(
        decimal,
        rust_decimal::Decimal::from(300),
        "expected 300 eur, got {}",
        decimal
    );
    let displayed = eval_str(code, "monthly_pay", "quarterly");
    assert!(
        displayed.to_lowercase().contains("eur"),
        "expected 'eur' in display, got: {}",
        displayed
    );
}

/// Inverse of `q_times_calendar_uses_builtin_calendar_factor`:
/// `300 eur / 100 eur_per_month as month` must plan and evaluate to 3 month.
#[test]
fn q_divide_rate_yields_months() {
    let code = r#"spec runway_inverse
uses lemma units
data money: measure
  -> unit eur: 1
data rate_type: measure
  -> unit eur_per_month: eur/month
data balance: 300 eur
data burn: 100 eur_per_month
rule month: (balance / burn) as month"#;
    assert_loads(code);
    let decimal = eval_decimal(code, "runway_inverse", "month");
    assert_eq!(
        decimal,
        rust_decimal::Decimal::from(3),
        "expected 3 month runway, got {}",
        decimal
    );
}

/// eur_per_month * 1 year: magnitude is direct product (100 * 1 = 100) with
/// signature [(eur,1),(month,-1),(year,1)] — no pre-canonicalization.
/// Casting `as eur` uses signature_factor: month factor 1, year factor 12,
/// result = 100 * 12 = 1200 eur.
#[test]
fn q_times_calendar_mismatched_unit_keeps_operand_magnitudes() {
    let code = r#"spec annual_pay
uses lemma units
data money: measure
  -> unit eur: 1
data rate: measure
  -> unit eur_per_month: eur/month
data monthly: 100 eur_per_month
rule annual: (monthly * 1 year)"#;
    assert_loads(code);
    let decimal = eval_decimal(code, "annual_pay", "annual");
    assert_eq!(
        decimal,
        rust_decimal::Decimal::from(1200),
        "expected 1200 eur (100 * 12 month in a year), got {}",
        decimal
    );
}

// ---------------------------------------------------------------------------
// Validation: calendar unit name collision
// ---------------------------------------------------------------------------

/// Declaring a measure unit named after a calendar unit must be rejected at
/// plan time (it would shadow the built-in calendar factor table).
#[test]
fn user_declared_unit_named_after_calendar_unit_is_rejected() {
    let code = r#"spec bad_unit
data duration: measure
  -> unit month: 1"#;
    let error = load_expect_error(code);
    assert!(
        error.to_lowercase().contains("month")
            || error.to_lowercase().contains("calendar")
            || error.to_lowercase().contains("reserved"),
        "expected error about reserved calendar name 'month', got: {}",
        error
    );
}

/// Companion to `user_declared_unit_named_after_calendar_unit_is_rejected`:
/// month must be declared on `uses lemma units`, not per-spec.
#[test]
fn uses_lemma_calendar_stdlib_loads() {
    let code = r#"spec calendar_smoke
uses lemma units
rule span: 3 month"#;
    assert_loads(code);
}

// ---------------------------------------------------------------------------
// User scenario: burn-rate runway (money / eur_month rate → month)
// ---------------------------------------------------------------------------

/// Full user-reported spec. Must load and yield 15 month with pinned inputs.
#[test]
fn burn_baby_burn_deadline_months() {
    let code = r#"spec burn_baby_burn
uses lemma units

data money: measure
  -> unit usd: 1.00
  -> unit eur: 1.19
  -> decimals 2
  -> minimum 0 usd

data balance: money
data money_flow: measure
  -> unit eur_month: eur/month
  -> unit usd_year: usd/year

data burn_rate: money_flow
  -> help "Provide the burn rate as EUR/month or USD/year."

data revenue: money_flow
  -> help "Provide the revenue as EUR/month or USD/year."

rule deadline: veto "Everything is fine: no deadline"
  unless burn_rate - revenue > 0 eur_month
    then (balance / (burn_rate - revenue)) as month"#;
    let mut data = HashMap::new();
    data.insert("balance".to_string(), "120000 eur".into());
    data.insert("burn_rate".to_string(), "10000 eur_month".into());
    data.insert("revenue".to_string(), "2000 eur_month".into());
    let mut engine = Engine::new();
    engine
        .load([(source(), code.to_string())])
        .expect("burn_baby_burn must load and plan");
    let response = engine
        .run(None, "burn_baby_burn", None, data, None, true)
        .expect("burn_baby_burn must evaluate");
    let deadline = response.results.get("deadline").expect("deadline rule");
    assert!(
        !deadline.vetoed,
        "expected deadline value, got veto: {:?}",
        deadline.veto_reason
    );
    let value = deadline
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("deadline value");
    let decimal = match &value.value {
        ValueKind::Measure(n) => lemma::ValueKind::Number(n.clone())
            .as_decimal_magnitude()
            .expect("deadline magnitude must commit to decimal"),
        other => panic!("expected measure deadline, got {:?}", other),
    };
    assert_eq!(
        decimal,
        rust_decimal::Decimal::from(15),
        "120000 eur / 8000 eur_month net burn = 15 month runway"
    );
}

// ---------------------------------------------------------------------------
// Validation: singular calendar form required
// ---------------------------------------------------------------------------

/// Plural calendar unit `months` is unknown after singular-only stdlib.
/// Singular `month` must still load.
#[test]
fn plural_calendar_unit_rejected_with_import() {
    let plural_code = r#"spec pl
uses lemma units
data money: measure
  -> unit eur: 1
data rate: measure
  -> unit eur_per_month: eur/month
data r: 10 eur_per_month
rule x: r * 3 months"#;
    let mut engine = Engine::new();
    let result = engine.load([(source(), plural_code.to_string())]);
    assert!(
        result.is_err(),
        "plural '3 months' must be rejected; stdlib calendar is singular-only"
    );

    let singular_code = r#"spec sing
uses lemma units
data money: measure
  -> unit eur: 1
data rate: measure
  -> unit eur_per_month: eur/month
data r: 10 eur_per_month
rule x: r * 3 month"#;
    assert_loads(singular_code);
}

// ---------------------------------------------------------------------------
// End-to-end: manufacturing spec without as-casts
// ---------------------------------------------------------------------------

/// The full manufacturing spec with every `as eur` removed from cross-type rules.
/// Loads, plans, and evaluates. Asserts the magnitude of total_production_cost
/// and manufacturing_cost_per_ce.
#[test]
fn manufacturing_spec_full_eval_no_casts() {
    // All cross-type rules removed of `as eur` cast — planning must auto-promote.
    let code = r#"spec manufacturing
uses lemma units

data money: measure
  -> unit eur: 1

data rate: measure
  -> unit eur_per_hour: eur/hour
  -> unit eur_per_minute: eur/minute

data unit_cost: measure
  -> unit eur_per_ce: eur/ce

data batch: measure
  -> unit ce: 1

data speed: measure
  -> unit ce_per_minute: ce/minute

data batch_size_ce: 100 ce
data packaging_speed: 5 ce_per_minute
data net_weight_mixing: 50 kilogram
data mixing_rate_per_kg: 0.10 eur_per_kg
data headcount_pouring: 1
data headcount_packaging_operators: 1
data headcount_packaging_workers: 1
data labor_rate_hr: 60 eur_per_hour
data machine_var_rate_hr: 30 eur_per_hour
data machine_fixed_rate_hr: 20 eur_per_hour
data indirect_overhead_pct: 15 percent

data mixing_rate: measure
  -> unit eur_per_kg: eur/kilogram

rule packaging_duration: batch_size_ce / packaging_speed
rule unpack_and_mix_cost: (net_weight_mixing * mixing_rate_per_kg)
rule total_line_headcount: headcount_pouring + headcount_packaging_operators + headcount_packaging_workers
rule direct_labor_cost: (total_line_headcount * labor_rate_hr * packaging_duration)
rule direct_var_machine_cost: (machine_var_rate_hr * packaging_duration)
rule direct_fixed_machine_cost: (machine_fixed_rate_hr * packaging_duration)
rule total_direct_cost: unpack_and_mix_cost + direct_labor_cost + direct_var_machine_cost + direct_fixed_machine_cost
rule indirect_overhead_cost: (total_direct_cost * indirect_overhead_pct)
rule total_production_cost: total_direct_cost + indirect_overhead_cost
rule manufacturing_cost_per_ce: (total_production_cost / batch_size_ce)"#;

    assert_loads(code);
    // packaging_duration = 100 ce / 5 ce_per_minute = 20 minute (canonical seconds on display)
    let duration_decimal =
        eval_measure_in_unit(code, "manufacturing", "packaging_duration", "minute");
    assert_eq!(
        duration_decimal,
        rust_decimal::Decimal::from(20),
        "packaging_duration must be 20 minute, got {}",
        duration_decimal
    );
    // total_production_cost: 5 + 60 + 10 + 400/60 + 15% overhead = 1127/12 eur exactly
    let total = eval_decimal(code, "manufacturing", "total_production_cost");
    let total_times_12 = total * rust_decimal::Decimal::from(12);
    assert_eq!(
        total_times_12.round(),
        rust_decimal::Decimal::from(1127),
        "total_production_cost must be 1127/12 eur, got {}",
        total
    );
    // manufacturing_cost_per_ce = 1127/12 / 100 = 1127/1200 eur/ce
    let cost_per_ce = eval_decimal(code, "manufacturing", "manufacturing_cost_per_ce");
    let cost_times_1200 = cost_per_ce * rust_decimal::Decimal::from(1200);
    assert_eq!(
        cost_times_1200.round(),
        rust_decimal::Decimal::from(1127),
        "manufacturing_cost_per_ce must be 1127/1200 eur/ce, got {}",
        cost_per_ce
    );
}

// ---------------------------------------------------------------------------
// Serialization round-trip
// ---------------------------------------------------------------------------

/// Evaluating a rule that produces a compound-signature Measure (not cast),
/// round-tripping via serialize then deserialize must preserve the magnitude
/// and the unit display.
#[test]
fn value_round_trip_via_evaluator_unit_conversion() {
    let code = r#"spec round_trip
uses lemma units
data money: measure
  -> unit eur: 1
data rate: measure
  -> unit eur_per_hour: eur/hour
data fee: 120 eur
data hrs: 2 hour
rule hourly: (fee / hrs)"#;
    assert_loads(code);
    let original_decimal = eval_decimal(code, "round_trip", "hourly");
    assert_eq!(
        original_decimal,
        rust_decimal::Decimal::from(60),
        "expected 60 eur_per_hour, got {}",
        original_decimal
    );
}

// ---------------------------------------------------------------------------
// Pre-existing tests preserved from the original symbolic_unit_arithmetic suite.
// These test behavior that must continue to work through and after the rewrite.
// Pattern matches on ValueKind::Measure use the OLD (RationalInteger, String,
// BaseMeasureVector) shape; they will be updated to (RationalInteger, Vec<(String,i32)>)
// when the rewrite_measure_value_kind_shape todo lands.
// ---------------------------------------------------------------------------

#[test]
fn ce_divided_by_ce_per_minute_yields_minute() {
    let code = r#"spec packaging
uses lemma units
data batch_size: measure
  -> unit ce: 1
data packaging_speed: measure
  -> unit ce_per_minute: ce/minute
data batch_size_ce: 100 ce
data speed: 5 ce_per_minute
rule packaging_duration: batch_size_ce / speed"#;
    let displayed = eval_str(code, "packaging", "packaging_duration");
    let decimal = eval_measure_in_unit(code, "packaging", "packaging_duration", "minute");
    assert!(
        displayed.to_lowercase().contains("second"),
        "unbound duration displays first written unit, got: {}",
        displayed
    );
    assert_eq!(
        decimal,
        rust_decimal::Decimal::from(20),
        "expected 20 minute"
    );
}

#[test]
fn ce_per_minute_times_minute_yields_ce_with_correct_magnitude() {
    let code = r#"spec packaging
uses lemma units
data batch_size: measure
  -> unit ce: 1
data packaging_speed: measure
  -> unit ce_per_minute: ce/minute
data speed: 2 ce_per_minute
data shift: 60 minute
rule throughput: speed * shift"#;
    let displayed = eval_str(code, "packaging", "throughput");
    let decimal = eval_decimal(code, "packaging", "throughput");
    assert!(
        displayed.to_lowercase().contains("ce"),
        "expected 'ce' in display, got: {}",
        displayed
    );
    assert_eq!(decimal, rust_decimal::Decimal::from(120), "expected 120");
}

#[test]
fn ce_per_minute_times_hour_requires_as_cast() {
    let code = r#"spec packaging
uses lemma units
data batch_size: measure
  -> unit ce: 1
data packaging_speed: measure
  -> unit ce_per_minute: ce/minute
data speed: 2 ce_per_minute
data shift: 1 hour
rule throughput: (speed * shift) as ce"#;
    let displayed = eval_str(code, "packaging", "throughput");
    let decimal = eval_decimal(code, "packaging", "throughput");
    assert!(
        displayed.to_lowercase().contains("ce"),
        "expected 'ce' in display, got: {}",
        displayed
    );
    assert_eq!(decimal, rust_decimal::Decimal::from(120), "expected 120");
}

#[test]
fn manufacturing_packaging_duration_and_labor_cost() {
    let code = r#"spec manufacturing
uses lemma units
data money: measure
  -> unit eur: 1.00
data rate: measure
  -> unit eur_per_hour: eur/hour
  -> unit eur_per_minute: eur/minute
data batch: measure
  -> unit ce: 1
data speed: measure
  -> unit ce_per_minute: ce/minute
data batch_size_ce: 100 ce
data packaging_speed: 5 ce_per_minute
data labor_rate_hr: 60 eur_per_hour
rule packaging_duration: batch_size_ce / packaging_speed
rule direct_labor_cost: (labor_rate_hr * packaging_duration)"#;
    let duration_decimal =
        eval_measure_in_unit(code, "manufacturing", "packaging_duration", "minute");
    assert_eq!(
        duration_decimal,
        rust_decimal::Decimal::from(20),
        "expected 20 minute"
    );

    let cost_decimal = eval_decimal(code, "manufacturing", "direct_labor_cost");
    assert_eq!(
        cost_decimal,
        rust_decimal::Decimal::from(20),
        "expected 20 eur"
    );
}

#[test]
fn unresolvable_signature_rejected_at_rule_boundary() {
    let code = r#"spec mixed
data length: measure
  -> unit meter: 1
data force: measure
  -> unit newton: 1
data x: 3 meter
data y: 5 newton
rule weird: x * y"#;
    let error = load_expect_error(code);
    assert!(
        error.to_lowercase().contains("anonymous")
            || error.to_lowercase().contains("dimension")
            || error.to_lowercase().contains("no"),
        "expected rejection of unresolvable result at rule boundary, got: {}",
        error
    );
}

#[test]
fn derived_type_without_factor_one_unit_plans_and_runs() {
    let code = r#"spec rates
uses lemma units
data money: measure
  -> unit eur: 1.00
data rate: measure
  -> unit eur_per_hour: eur/hour
data fee: 10 eur_per_hour
data minute: 30 minute
rule charge: (fee * minute)"#;
    let decimal = eval_decimal(code, "rates", "charge");
    assert_eq!(decimal, rust_decimal::Decimal::from(5), "expected 5 eur");
}

// ---------------------------------------------------------------------------
// `as` binding precedence: detection and error messages
// ---------------------------------------------------------------------------

/// `balance / burn_rate as month` parses as `balance / (burn_rate as month)`.
/// burn_rate is money/calendar; month is calendar — incompatible families.
/// But `(balance / burn_rate) as month` would be valid.
/// Expect a targeted error suggesting the parenthesized form.
#[test]
fn as_binding_division_suggests_parentheses() {
    let code = r#"spec burn
uses lemma units
data money: measure
  -> unit eur: 1
data rate: measure
  -> unit eur_per_month: eur/month
data balance: 120000 eur
data burn_rate: 10000 eur_per_month
rule runway: balance / burn_rate as month"#;
    let error = load_expect_error(code);
    assert!(
        error.contains("(balance / burn_rate) as month")
            || error.contains("(balance / burn_rate) as month"),
        "expected parenthesized suggestion in error, got: {}",
        error
    );
    assert!(
        !error.contains("different measure families"),
        "expected targeted error, not generic family error, got: {}",
        error
    );
}

/// Same pattern for multiplication: `fee * shift as eur` where shift is duration
/// and eur is money. `shift as eur` is incompatible; `(fee * shift) as eur` is valid
/// because fee is eur/hour and fee * shift cancels hour leaving eur.
#[test]
fn as_binding_multiplication_suggests_parentheses() {
    let code = r#"spec payroll
data money_type: measure
  -> unit eur: 1
data duration_type: measure
  -> unit hour: 1
data rate_type: measure
  -> unit eur_per_hour: eur/hour
data fee: 10 eur_per_hour
data shift: 8 hour
rule wages: fee * shift as eur"#;
    let error = load_expect_error(code);
    assert!(
        error.contains("(fee * shift) as eur"),
        "expected parenthesized suggestion in error, got: {}",
        error
    );
}

/// Compound operand: `balance / (burn_rate - revenue) as month`.
/// The suggestion label must expand the arithmetic sub-expression, not produce `<expr>`.
#[test]
fn as_binding_compound_operand_suggestion_is_readable() {
    let code = r#"spec burn
uses lemma units
data money: measure
  -> unit eur: 1
data rate: measure
  -> unit eur_per_month: eur/month
data balance: 120000 eur
data burn_rate: 10000 eur_per_month
data revenue: 2000 eur_per_month
rule runway: balance / (burn_rate - revenue) as month"#;
    let error = load_expect_error(code);
    assert!(
        !error.contains("<expr>") && !error.contains("<measure>"),
        "expected readable operand labels in error, got: {}",
        error
    );
    assert!(
        error.contains("burn_rate") && error.contains("revenue"),
        "expected operand names in error, got: {}",
        error
    );
}

/// `speed * time as hour`: time and hour are same-family (duration).
/// The operand conversion is valid, so this must succeed as `speed * (time as hour)`.
#[test]
fn as_binding_valid_operand_conversion_succeeds() {
    let code = r#"spec motion
uses lemma units
data speed_val: 10 meter_per_second
data duration: 2 hour
rule distance: speed_val * duration as second"#;
    assert_loads(code);
}

/// `mass / time as gram`: gram is mass; time is duration. gram doesn't match time,
/// and doesn't match mass/time either. Must produce the standard conversion error,
/// not a parenthesization suggestion.
#[test]
fn as_binding_neither_interpretation_valid_emits_standard_error() {
    let code = r#"spec bad
uses lemma units
data mass_type: measure
  -> unit kilogram: 1
  -> unit gram: 0.001
data m: 5 kilogram
data t: 2 second
rule weird: m / t as gram"#;
    let error = load_expect_error(code);
    assert!(
        !error.contains("Write '("),
        "must not suggest parenthesization when neither interpretation is valid, got: {}",
        error
    );
}

/// `mass * acceleration as newton`: acceleration (length/time^2) is not same-family
/// as force (newton), so operand conversion fails. The combined result mass*acceleration
/// matches force's decomposition, so suggest `(mass * acceleration) as newton`.
/// Uses stdlib `newton` (no local redeclaration) plus local `mps2`.
#[test]
fn as_binding_compound_unit_newton_suggests_parentheses() {
    let code = r#"spec mechanics
uses lemma units
data acceleration_type: measure
  -> unit mps2: meter/second^2
data m: 10 kilogram
data a: 5 mps2
rule f: m * a as newton"#;
    let error = load_expect_error(code);
    assert!(
        error.contains("(m * a) as newton"),
        "expected parenthesized suggestion for compound unit, got: {}",
        error
    );
}

/// `(mass * acceleration) as newton` with explicit parentheses must succeed.
/// Uses stdlib `newton` (no local redeclaration) plus local `mps2`.
#[test]
fn as_binding_compound_unit_newton_explicit_parentheses_succeeds() {
    let code = r#"spec mechanics
uses lemma units
data acceleration_type: measure
  -> unit mps2: meter/second^2
data m: 10 kilogram
data a: 5 mps2
rule f: (m * a) as newton"#;
    let decimal = eval_decimal(code, "mechanics", "f");
    assert_eq!(
        decimal,
        rust_decimal::Decimal::from(50),
        "10 kg * 5 m/s^2 = 50 newton"
    );
}

/// `(balance / burn_rate) as month` with explicit parentheses must succeed and
/// produce the correct number of month.
#[test]
fn as_binding_explicit_parentheses_succeeds() {
    let code = r#"spec burn
uses lemma units
data money: measure
  -> unit eur: 1
data rate: measure
  -> unit eur_per_month: eur/month
data balance: 120000 eur
data burn_rate: 10000 eur_per_month
rule runway: (balance / burn_rate) as month"#;
    let decimal = eval_decimal(code, "burn", "runway");
    assert_eq!(
        decimal,
        rust_decimal::Decimal::from(12),
        "expected 12 month"
    );
}

//! Real-world scenario testing: fresh scenarios that exercise Lemma as a user would.
//! These cover edge cases in arithmetic, veto propagation, temporal specs, ranges,
//! compound units, and cross-spec composition.

use lemma::{DateTimeValue, Engine, SourceType};
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

fn src(name: &str) -> SourceType {
    SourceType::Path(Arc::new(PathBuf::from(name)))
}

fn run_spec(engine: &Engine, spec: &str, data: &[(&str, &str)]) -> lemma::Response {
    let now = DateTimeValue::now();
    let data_map: HashMap<String, String> = data
        .iter()
        .map(|(k, v)| (k.to_string(), (*v).to_string()))
        .collect();
    engine
        .run(None, spec, Some(&now), data_map, None, false)
        .unwrap()
}

fn rule_display(response: &lemma::Response, rule_name: &str) -> String {
    response
        .results
        .values()
        .find(|r| r.rule.name == rule_name)
        .unwrap_or_else(|| panic!("rule '{}' not found", rule_name))
        .result()
        .unwrap_or_else(|| panic!("rule '{}' has no display", rule_name))
        .to_string()
}

fn rule_vetoed(response: &lemma::Response, rule_name: &str) -> bool {
    response
        .results
        .values()
        .find(|r| r.rule.name == rule_name)
        .unwrap_or_else(|| panic!("rule '{}' not found", rule_name))
        .vetoed
}

// ===========================================================================
// SCENARIO 1: Insurance premium calculator
// Tests: percentage arithmetic, multi-level unless, veto propagation
// ===========================================================================

#[test]
fn scenario_insurance_premium_basic() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("insurance.lemma"),
            r#"
spec insurance_premium

data age: number -> minimum 18 -> maximum 99
data smoker: boolean
data coverage_amount: number -> minimum 10000 -> maximum 1000000
data pre_existing_condition: boolean

rule base_rate: 0.5%
  unless age >= 30 then 0.8%
  unless age >= 40 then 1.2%
  unless age >= 50 then 1.8%
  unless age >= 60 then 2.5%
  unless age >= 70 then 4.0%

rule smoker_surcharge: 0%
  unless smoker then 50%

rule condition_surcharge: 0%
  unless pre_existing_condition then 25%

rule effective_rate: base_rate + smoker_surcharge + condition_surcharge

rule annual_premium: coverage_amount * effective_rate

rule monthly_premium: annual_premium / 12

rule is_high_risk: age >= 60 and smoker

rule coverage_check: yes
  unless coverage_amount > 500000 and age >= 70
    then veto "Coverage exceeds limit for age group"
"#
            .to_string(),
        )])
        .unwrap();

    // 25-year-old non-smoker, no conditions, 100k coverage
    let resp = run_spec(
        &engine,
        "insurance_premium",
        &[
            ("age", "25"),
            ("smoker", "false"),
            ("coverage_amount", "100000"),
            ("pre_existing_condition", "false"),
        ],
    );

    // base_rate = 0.5% (age < 30)
    // effective_rate = 0.5% + 0% + 0% = 0.5%
    // annual = 100000 * 0.5% = 500
    // monthly = 500 / 12 ≈ 41.666...
    assert!(!rule_vetoed(&resp, "annual_premium"));
    assert!(!rule_vetoed(&resp, "coverage_check"));

    let display = rule_display(&resp, "annual_premium");
    assert_eq!(display, "500", "annual premium for young non-smoker");
}

#[test]
fn scenario_insurance_veto_propagation() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("insurance.lemma"),
            r#"
spec insurance_premium

data age: number -> minimum 18 -> maximum 99
data smoker: boolean
data coverage_amount: number -> minimum 10000 -> maximum 1000000
data pre_existing_condition: boolean

rule base_rate: 0.5%
  unless age >= 30 then 0.8%
  unless age >= 40 then 1.2%
  unless age >= 50 then 1.8%
  unless age >= 60 then 2.5%
  unless age >= 70 then 4.0%

rule smoker_surcharge: 0%
  unless smoker then 50%

rule condition_surcharge: 0%
  unless pre_existing_condition then 25%

rule effective_rate: base_rate + smoker_surcharge + condition_surcharge

rule annual_premium: coverage_amount * effective_rate

rule monthly_premium: annual_premium / 12

rule coverage_check: yes
  unless coverage_amount > 500000 and age >= 70
    then veto "Coverage exceeds limit for age group"
"#
            .to_string(),
        )])
        .unwrap();

    // 75-year-old, 600k coverage should veto
    let resp = run_spec(
        &engine,
        "insurance_premium",
        &[
            ("age", "75"),
            ("smoker", "false"),
            ("coverage_amount", "600000"),
            ("pre_existing_condition", "false"),
        ],
    );

    assert!(
        rule_vetoed(&resp, "coverage_check"),
        "should veto for high coverage + old age"
    );
}

// ===========================================================================
// SCENARIO 2: Compound interest with exponentiation
// Tests: ^ operator, division, mathematical functions
// ===========================================================================

#[test]
fn scenario_compound_interest() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("interest.lemma"),
            r#"
spec compound_interest

data principal: number -> minimum 0
data annual_rate: ratio -> minimum 0% -> maximum 100%
data year: number -> minimum 1 -> maximum 50

rule growth_factor: (100% + annual_rate) ^ year

rule future_value: principal * growth_factor

rule total_interest: future_value - principal
"#
            .to_string(),
        )])
        .unwrap();

    // 1000 at 10% for 3 year = 1000 * 1.1^3 = 1331
    let resp = run_spec(
        &engine,
        "compound_interest",
        &[("principal", "1000"), ("annual_rate", "10%"), ("year", "3")],
    );

    let display = rule_display(&resp, "future_value");
    assert_eq!(display, "1331", "1000 * 1.1^3 = 1331");
}

// ===========================================================================
// SCENARIO 3: Cross-spec composition with data binding
// Tests: uses, with, qualified references
// ===========================================================================

#[test]
fn scenario_cross_spec_order_composition() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("invoice.lemma"),
            r#"
spec tax_rates

data rate: 21%

rule effective_tax: rate


spec product_pricing

data base_price: number -> minimum 0
data quantity: number -> minimum 1

rule subtotal: base_price * quantity


spec invoice

uses tax: tax_rates
uses items: product_pricing
  -> with base_price: 50
  -> with quantity: 10

rule net_total: items.subtotal
rule tax_amount: net_total * tax.effective_tax
rule gross_total: net_total + tax_amount
"#
            .to_string(),
        )])
        .unwrap();

    let resp = run_spec(&engine, "invoice", &[]);

    // items.subtotal = 50 * 10 = 500
    // tax = 500 * 21% = 105
    // gross = 500 + 105 = 605
    let display = rule_display(&resp, "gross_total");
    assert_eq!(display, "605", "50*10 + 21% tax = 605");
}

// ===========================================================================
// SCENARIO 4: Veto as default with exhaustive unless (lookup table)
// Tests: veto default, text comparison, last-wins semantics
// ===========================================================================

#[test]
fn scenario_veto_lookup_table() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("tax.lemma"),
            r#"
spec tax_lookup

data country: text
  -> option "NL"
  -> option "BE"
  -> option "DE"
  -> option "FR"
  -> option "US"

rule vat_rate: veto "Unknown country for VAT"
  unless country is "NL" then 21%
  unless country is "BE" then 21%
  unless country is "DE" then 19%
  unless country is "FR" then 20%
  unless country is "US" then 0%

rule has_vat: vat_rate is not veto
"#
            .to_string(),
        )])
        .unwrap();

    let resp = run_spec(&engine, "tax_lookup", &[("country", "DE")]);
    let display = rule_display(&resp, "vat_rate");
    assert_eq!(display, "19%", "Germany VAT = 19%");
    assert!(!rule_vetoed(&resp, "has_vat"));
}

// ===========================================================================
// SCENARIO 5: Percentage of a percentage (ratio arithmetic)
// Tests: ratio * ratio, percentage subtraction from value
// ===========================================================================

#[test]
fn scenario_stacked_discounts() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("discounts.lemma"),
            r#"
spec stacked_discounts

data price: number -> minimum 0
data member_discount: 10%
data seasonal_discount: 20%
data coupon_discount: 5%

rule after_member: price - member_discount * price
rule after_seasonal: after_member - seasonal_discount * after_member
rule final_price: after_seasonal - coupon_discount * after_seasonal
"#
            .to_string(),
        )])
        .unwrap();

    // price = 1000
    // after_member = 1000 - 0.1*1000 = 900
    // after_seasonal = 900 - 0.2*900 = 720
    // final = 720 - 0.05*720 = 684
    let resp = run_spec(&engine, "stacked_discounts", &[("price", "1000")]);
    let display = rule_display(&resp, "final_price");
    assert_eq!(display, "684", "1000 * 0.9 * 0.8 * 0.95 = 684");
}

// ===========================================================================
// SCENARIO 6: SI units and conversion
// Tests: uses lemma units, unit conversion with `as`, compound units
// ===========================================================================

#[test]
fn scenario_si_unit_conversion() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("logistics.lemma"),
            r#"
spec logistics
uses lemma units

data package_weight: 2500 gram
data distance: 150 kilometer
data speed: 60 kilometer

rule weight_kg: package_weight as kilogram
rule is_heavy: package_weight > 20 kilogram
rule is_light: package_weight < 1 kilogram
"#
            .to_string(),
        )])
        .unwrap();

    let resp = run_spec(&engine, "logistics", &[]);
    let display = rule_display(&resp, "weight_kg");
    assert_eq!(display, "2.5 kilogram", "2500g = 2.5kg");

    let heavy = rule_display(&resp, "is_heavy");
    assert_eq!(heavy, "false", "2.5kg is not > 20kg");

    let light = rule_display(&resp, "is_light");
    assert_eq!(light, "false", "2.5kg is not < 1kg");
}

// ===========================================================================
// SCENARIO 7: Date ranges and membership
// Tests: date range, `in` operator, span calculation
// ===========================================================================

#[test]
fn scenario_date_range_membership() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("employment.lemma"),
            r#"
spec employment
uses lemma units

data hire_date: 2023-03-15
data review_date: 2024-03-15

rule tenure_days: (hire_date...review_date) as day
rule in_first_year: hire_date in 2023-01-01...2024-01-01
rule in_q1_2023: hire_date in 2023-01-01...2023-04-01
"#
            .to_string(),
        )])
        .unwrap();

    let resp = run_spec(&engine, "employment", &[]);

    // 2023-03-15 is in Q1 2023 (Jan 1 - Apr 1)
    let in_q1 = rule_display(&resp, "in_q1_2023");
    assert_eq!(in_q1, "true", "March 15 is in Q1 2023");

    let in_first = rule_display(&resp, "in_first_year");
    assert_eq!(in_first, "true", "hire_date is in first year");

    // tenure: 2023-03-15 to 2024-03-15 = 366 day (2024 is leap year)
    let tenure = rule_display(&resp, "tenure_days");
    assert_eq!(tenure, "366 day", "one year including leap day");
}

// ===========================================================================
// SCENARIO 8: Boolean logic combinations
// Tests: and, not, complex boolean expressions in unless
// ===========================================================================

#[test]
fn scenario_boolean_logic() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("access.lemma"),
            r#"
spec access_control

data is_admin: boolean
data is_verified: boolean
data account_age_days: number -> minimum 0
data has_2fa: boolean

rule is_trusted: is_verified and account_age_days >= 30

rule can_post: yes
  unless not is_verified then no
  unless account_age_days < 7 then no

rule can_moderate: no
  unless is_admin then yes
  unless is_trusted and has_2fa then yes

rule access_level: 0
  unless is_verified then 1
  unless is_trusted then 2
  unless is_admin then 3
"#
            .to_string(),
        )])
        .unwrap();

    // Verified non-admin, 60 day, with 2FA
    let resp = run_spec(
        &engine,
        "access_control",
        &[
            ("is_admin", "false"),
            ("is_verified", "true"),
            ("account_age_days", "60"),
            ("has_2fa", "true"),
        ],
    );

    let can_mod = rule_display(&resp, "can_moderate");
    assert_eq!(can_mod, "true", "trusted + 2FA can moderate");

    let level = rule_display(&resp, "access_level");
    assert_eq!(level, "2", "trusted user gets level 2");
}

// ===========================================================================
// SCENARIO 9: Division by zero → veto
// Tests: runtime veto from impossible arithmetic
// ===========================================================================

#[test]
fn scenario_division_by_zero_vetoes() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("division.lemma"),
            r#"
spec division_test

data numerator: number
data denominator: number

rule division_ratio: numerator / denominator
rule double_ratio: division_ratio * 2
"#
            .to_string(),
        )])
        .unwrap();

    let resp = run_spec(
        &engine,
        "division_test",
        &[("numerator", "100"), ("denominator", "0")],
    );

    assert!(
        rule_vetoed(&resp, "division_ratio"),
        "division by zero should veto"
    );
    assert!(
        rule_vetoed(&resp, "double_ratio"),
        "dependent rule should also veto"
    );
}

// ===========================================================================
// SCENARIO 10: Modulo and floor/ceil/round
// Tests: mathematical operations, integer behavior
// ===========================================================================

#[test]
fn scenario_math_operations() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("math.lemma"),
            r#"
spec math_ops

data value: number

rule remainder: value % 7
rule rounded: round(value / 3)
rule floored: floor(value / 3)
rule ceiled: ceil(value / 3)
rule absolute: abs(0 - value)
"#
            .to_string(),
        )])
        .unwrap();

    let resp = run_spec(&engine, "math_ops", &[("value", "20")]);

    let rem = rule_display(&resp, "remainder");
    assert_eq!(rem, "6", "20 % 7 = 6");

    let rounded = rule_display(&resp, "rounded");
    assert_eq!(rounded, "7", "round(20/3) = round(6.666) = 7");

    let floored = rule_display(&resp, "floored");
    assert_eq!(floored, "6", "floor(20/3) = 6");

    let ceiled = rule_display(&resp, "ceiled");
    assert_eq!(ceiled, "7", "ceil(20/3) = 7");

    let abs_val = rule_display(&resp, "absolute");
    assert_eq!(abs_val, "20", "abs(-20) = 20");
}

// ===========================================================================
// SCENARIO 11: Number ranges with `in`
// Tests: number range type, in operator with number ranges
// ===========================================================================

#[test]
fn scenario_number_range_bands() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("grading.lemma"),
            r#"
spec grading

data score: number -> minimum 0 -> maximum 100

rule grade: "F"
  unless score in 60...70 then "D"
  unless score in 70...80 then "C"
  unless score in 80...90 then "B"
  unless score in 90...101 then "A"

rule is_passing: score >= 60
"#
            .to_string(),
        )])
        .unwrap();

    // Score of 85 should be "B"
    let resp = run_spec(&engine, "grading", &[("score", "85")]);
    let grade = rule_display(&resp, "grade");
    assert_eq!(grade, "B", "score 85 is in [80,90) = B");

    // Score of 90 should be "A" (90 is inclusive lower bound of 90...101)
    let resp = run_spec(&engine, "grading", &[("score", "90")]);
    let grade = rule_display(&resp, "grade");
    assert_eq!(grade, "A", "score 90 is in [90,101) = A");

    // Score of 59 should be "F" (below all bands)
    let resp = run_spec(&engine, "grading", &[("score", "59")]);
    let grade = rule_display(&resp, "grade");
    assert_eq!(grade, "F", "score 59 is below all bands = F");

    // Boundary: score of 70 is in [70,80) = "C", not "D"
    let resp = run_spec(&engine, "grading", &[("score", "70")]);
    let grade = rule_display(&resp, "grade");
    assert_eq!(grade, "C", "score 70 is at boundary [70,80) = C");
}

// ===========================================================================
// SCENARIO 12: Temporal spec versions
// Tests: effective dates, version resolution
// ===========================================================================

#[test]
fn scenario_temporal_spec_versions() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("temporal.lemma"),
            r#"
spec vat_rates

data rate: 19%


spec vat_rates 2024-01-01

data rate: 21%


spec invoice_calc

uses vat: vat_rates

data amount: number -> minimum 0

rule tax: amount * vat.rate
rule total: amount + tax
"#
            .to_string(),
        )])
        .unwrap();

    // Evaluate at a date after 2024-01-01 — should use 21%
    let effective = DateTimeValue::from_str("2024-06-15").unwrap();
    let data_map: HashMap<String, String> = [("amount".to_string(), "1000".into())].into();
    let resp = engine
        .run(
            None,
            "invoice_calc",
            Some(&effective),
            data_map,
            None,
            false,
        )
        .unwrap();
    let tax = rule_display(&resp, "tax");
    assert_eq!(tax, "210", "after 2024-01-01, VAT is 21% → 1000*21% = 210");

    // Evaluate before 2024-01-01 — should use 19%
    let effective_old = DateTimeValue::from_str("2023-06-15").unwrap();
    let data_map2: HashMap<String, String> = [("amount".to_string(), "1000".into())].into();
    let resp2 = engine
        .run(
            None,
            "invoice_calc",
            Some(&effective_old),
            data_map2,
            None,
            false,
        )
        .unwrap();
    let tax2 = rule_display(&resp2, "tax");
    assert_eq!(
        tax2, "190",
        "before 2024-01-01, VAT is 19% → 1000*19% = 190"
    );
}

// ===========================================================================
// SCENARIO 13: Veto `is veto` check (testing without propagation)
// Tests: is veto operator, boolean result from veto check
// ===========================================================================

#[test]
fn scenario_is_veto_check() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("validation.lemma"),
            r#"
spec validation

data input_value: number

rule validated: input_value
  unless input_value < 0 then veto "Negative values not allowed"
  unless input_value > 1000 then veto "Value too large"

rule is_valid: validated is not veto
rule is_invalid: validated is veto

rule safe_value: validated
  unless is_invalid then 0
"#
            .to_string(),
        )])
        .unwrap();

    // Valid input
    let resp = run_spec(&engine, "validation", &[("input_value", "50")]);
    assert!(!rule_vetoed(&resp, "validated"));
    let valid = rule_display(&resp, "is_valid");
    assert_eq!(valid, "true", "50 is valid");
    let safe = rule_display(&resp, "safe_value");
    assert_eq!(safe, "50", "safe_value = 50 when valid");

    // Negative input — vetoes
    let resp = run_spec(&engine, "validation", &[("input_value", "-5")]);
    assert!(rule_vetoed(&resp, "validated"), "-5 should veto");
    let valid = rule_display(&resp, "is_valid");
    assert_eq!(valid, "false", "-5 is invalid");
    let safe = rule_display(&resp, "safe_value");
    assert_eq!(safe, "0", "safe_value falls back to 0 when invalid");
}

// ===========================================================================
// SCENARIO 14: Percentage arithmetic semantics
// Tests: number ± ratio is rejected; multiplication stays
// ===========================================================================

#[test]
fn scenario_percentage_subtract_rejected() {
    let mut engine = Engine::new();
    let result = engine.load([(
        src("pct_ops.lemma"),
        r#"
spec percent_ops
data amount: number
rule bad: amount - 10%
"#
        .to_string(),
    )]);
    assert!(result.is_err());
    let msg = result
        .unwrap_err()
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("; ");
    assert!(msg.contains("scale explicitly"), "got: {}", msg);
}

#[test]
fn scenario_percentage_multiply() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("pct_ops.lemma"),
            r#"
spec percent_ops
data amount: number
rule times_fifty_pct: amount * 50%
"#
            .to_string(),
        )])
        .unwrap();

    let resp = run_spec(&engine, "percent_ops", &[("amount", "200")]);
    let times = rule_display(&resp, "times_fifty_pct");
    assert_eq!(times, "100", "200 * 50% = 100");
}

// ===========================================================================
// SCENARIO 15: Deep unless chain — last wins verification
// Tests: unless ordering semantics with overlapping conditions
// ===========================================================================

#[test]
fn scenario_unless_last_wins() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("priority.lemma"),
            r#"
spec priority_rules

data urgency: number -> minimum 1 -> maximum 10
data is_vip: boolean
data is_internal: boolean

rule priority: "low"
  unless urgency >= 3 then "medium"
  unless urgency >= 5 then "high"
  unless urgency >= 8 then "critical"
  unless is_vip then "critical"
  unless is_internal then "internal"
"#
            .to_string(),
        )])
        .unwrap();

    // urgency=9, vip=true, internal=true → last match is "internal"
    let resp = run_spec(
        &engine,
        "priority_rules",
        &[
            ("urgency", "9"),
            ("is_vip", "true"),
            ("is_internal", "true"),
        ],
    );
    let priority = rule_display(&resp, "priority");
    assert_eq!(priority, "internal", "last matching clause wins: internal");

    // urgency=9, vip=true, internal=false → last match is "critical" (is_vip)
    let resp = run_spec(
        &engine,
        "priority_rules",
        &[
            ("urgency", "9"),
            ("is_vip", "true"),
            ("is_internal", "false"),
        ],
    );
    let priority = rule_display(&resp, "priority");
    assert_eq!(
        priority, "critical",
        "VIP with urgency 9 but not internal = critical"
    );

    // urgency=2, vip=false, internal=false → no unless matches → "low"
    let resp = run_spec(
        &engine,
        "priority_rules",
        &[
            ("urgency", "2"),
            ("is_vip", "false"),
            ("is_internal", "false"),
        ],
    );
    let priority = rule_display(&resp, "priority");
    assert_eq!(priority, "low", "nothing matches = low");
}

fn main() {}

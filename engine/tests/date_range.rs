use lemma::{DateGranularity, ValueKind};
use lemma::{DateTimeValue, Engine, LiteralValue, TimezoneValue};
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

fn source() -> lemma::SourceType {
    lemma::SourceType::Path(Arc::new(PathBuf::from("test.lemma")))
}

fn effective(y: i32, m: u32, d: u32, h: u32, min: u32, s: u32) -> DateTimeValue {
    DateTimeValue {
        year: y,
        month: m,
        day: d,
        hour: h,
        minute: min,
        second: s,
        microsecond: 0,
        timezone: Some(TimezoneValue {
            offset_hours: 0,
            offset_minutes: 0,
        }),
        granularity: DateGranularity::DateTime,
    }
}

fn default_effective() -> DateTimeValue {
    effective(2026, 3, 7, 12, 0, 0)
}

fn eval_literal_with_data(
    code: &str,
    spec_name: &str,
    rule_name: &str,
    effective: &DateTimeValue,
    data: HashMap<String, String>,
) -> LiteralValue {
    let mut engine = Engine::new();
    engine
        .load([(source(), code.to_string())])
        .expect("Should parse and plan");
    let response = engine
        .run(
            None,
            spec_name,
            Some(effective),
            data,
            Some(&[rule_name.to_string()]),
            true,
        )
        .expect("Should evaluate");
    let rule = response
        .results
        .get(rule_name)
        .unwrap_or_else(|| panic!("Rule '{}' not found", rule_name));
    if rule.vetoed {
        panic!(
            "Rule '{}' vetoed: {}",
            rule_name,
            rule.veto_reason.as_deref().unwrap_or("Vetoed")
        );
    }
    rule.explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("BUG: non-vetoed rule missing value")
        .clone()
}

fn eval_rule(code: &str, spec_name: &str, rule_name: &str) -> String {
    eval_rule_with_effective(code, spec_name, rule_name, &default_effective())
}

fn eval_rule_number(code: &str, spec_name: &str, rule_name: &str) -> Decimal {
    let mut engine = Engine::new();
    engine
        .load([(source(), code.to_string())])
        .expect("Should parse and plan");
    let effective = default_effective();
    let response = engine
        .run(
            None,
            spec_name,
            Some(&effective),
            HashMap::new(),
            Some(&[rule_name.to_string()]),
            true,
        )
        .expect("Should evaluate");
    *response
        .results
        .get(rule_name)
        .unwrap_or_else(|| panic!("Rule '{}' not found", rule_name))
        .result
        .as_ref()
        .expect("rule result value")
        .number
        .as_ref()
        .expect("number result")
}

fn eval_rule_with_effective(
    code: &str,
    spec_name: &str,
    rule_name: &str,
    effective: &DateTimeValue,
) -> String {
    let mut engine = Engine::new();
    engine
        .load([(source(), code.to_string())])
        .expect("Should parse and plan");
    let response = engine
        .run(
            None,
            spec_name,
            Some(effective),
            HashMap::new(),
            Some(&[rule_name.to_string()]),
            true,
        )
        .expect("Should evaluate");
    response
        .results
        .get(rule_name)
        .unwrap_or_else(|| panic!("Rule '{}' not found", rule_name))
        .result()
        .expect("result")
        .to_string()
}

fn eval_bool(code: &str, spec_name: &str, rule_name: &str, effective: &DateTimeValue) -> bool {
    match eval_literal_with_data(code, spec_name, rule_name, effective, HashMap::new()).value {
        ValueKind::Boolean(val) => val,
        other => panic!("Expected Boolean, got {:?}", other),
    }
}

fn expect_plan_error(code: &str, expected_fragment: &str) {
    let mut engine = Engine::new();
    let result = engine.load([(source(), code.to_string())]);
    assert!(result.is_err(), "Expected planning error");
    let combined = result
        .unwrap_err()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ");
    assert!(
        combined.contains(expected_fragment),
        "Expected error containing '{}', got: {}",
        expected_fragment,
        combined
    );
}

fn assert_contains_all(actual: &str, expected_parts: &[&str]) {
    assert!(
        !actual.contains("..."),
        "Expected scalar/date output, got range-like output '{}'",
        actual
    );
    let lower = actual.to_lowercase();
    for part in expected_parts {
        assert!(
            contains_expected_fragment(&lower, &part.to_lowercase()),
            "Expected '{}' to contain '{}'",
            actual,
            part
        );
    }
}

fn contains_expected_fragment(haystack: &str, needle: &str) -> bool {
    if is_numeric_fragment(needle) {
        contains_numeric_fragment(haystack, needle)
    } else {
        haystack.contains(needle)
    }
}

fn contains_numeric_fragment(haystack: &str, needle: &str) -> bool {
    let mut search_from = 0;
    while let Some(relative_index) = haystack[search_from..].find(needle) {
        let index = search_from + relative_index;
        let mut start = index;
        while start > 0 {
            let previous = haystack[..start].chars().next_back().unwrap();
            if !is_numeric_context_character(previous) {
                break;
            }
            start -= previous.len_utf8();
        }

        let mut end = index + needle.len();
        while end < haystack.len() {
            let next = haystack[end..].chars().next().unwrap();
            if !is_numeric_context_character(next) {
                break;
            }
            end += next.len_utf8();
        }

        let candidate = &haystack[start..end];
        if candidate == needle {
            return true;
        }
        if let (Ok(candidate_decimal), Ok(needle_decimal)) = (
            candidate.parse::<rust_decimal::Decimal>(),
            needle.parse::<rust_decimal::Decimal>(),
        ) {
            if candidate_decimal == needle_decimal {
                return true;
            }
        }
        if start == index && end == index + needle.len() {
            return true;
        }
        search_from = index + needle.len();
    }
    false
}

fn is_numeric_fragment(fragment: &str) -> bool {
    let mut has_digit = false;
    for character in fragment.chars() {
        if character.is_ascii_digit() {
            has_digit = true;
            continue;
        }
        if character == '-' || character == '.' {
            continue;
        }
        return false;
    }
    has_digit
}

fn is_numeric_context_character(character: char) -> bool {
    character.is_ascii_digit() || character == '.' || character == '-'
}

// =============================================================================
// A. Date range conversions via `...`
// =============================================================================

#[test]
fn a1_date_range_in_years() {
    let code = r#"spec test
uses lemma units
rule age: 1990-05-20...2024-06-15 as year as number"#;
    let val = eval_rule(code, "test", "age");
    assert_contains_all(&val, &["34"]);
}

#[test]
fn a2_date_range_in_months() {
    let code = r#"spec test
uses lemma units
rule month: 2024-01-15...2024-06-15 as month as number"#;
    let val = eval_rule(code, "test", "month");
    assert_contains_all(&val, &["5"]);
}

#[test]
fn a3_date_range_in_days() {
    let code = r#"spec test
uses lemma units
rule r: 2024-06-01...2024-06-15 as day as number"#;
    let val = eval_rule(code, "test", "r");
    assert_contains_all(&val, &["14"]);
}

#[test]
fn a4_date_range_in_hours() {
    let code = r#"spec test
uses lemma units
rule hour: 2024-01-01...2024-01-02 as hour as number"#;
    let val = eval_rule(code, "test", "hour");
    assert_contains_all(&val, &["24"]);
}

// =============================================================================
// B. Indirect rule references
// =============================================================================

#[test]
fn b1_indirect_in_years() {
    let code = r#"spec test
uses lemma units
data d1: 1990-05-20
data d2: 2024-06-15
rule span: d1...d2
rule age: span as year as number"#;
    let val = eval_rule(code, "test", "age");
    assert_contains_all(&val, &["34"]);
}

#[test]
fn b2_indirect_in_days() {
    let code = r#"spec test
uses lemma units
data d1: 2024-06-01
data d2: 2024-06-15
rule span: d1...d2
rule day: span as day as number"#;
    let val = eval_rule(code, "test", "day");
    assert_contains_all(&val, &["14"]);
}

#[test]
fn b3_chained_refs() {
    let code = r#"spec test
uses lemma units
data d1: 1990-05-20
data d2: 2024-06-15
rule span: d1...d2
rule mid: span
rule age: mid as year as number"#;
    let val = eval_rule(code, "test", "age");
    assert_contains_all(&val, &["34"]);
}

#[test]
fn b4_indirect_comparison() {
    let code = r#"spec test
uses lemma units
data d1: 2000-01-01
data d2: 2026-01-01
rule span: d1...d2
rule adult: span as year as number >= 18"#;
    assert!(eval_bool(code, "test", "adult", &default_effective()));
}

// =============================================================================
// C. DateRange +/- Duration -> Duration span
// =============================================================================

#[test]
fn c1_date_range_plus_duration_uses_span() {
    let code = r#"spec test
uses lemma units
rule value: ((2024-02-15...2024-03-15) + 1 day) as day as number"#;
    let val = eval_rule(code, "test", "value");
    assert_contains_all(&val, &["30"]);
}

#[test]
fn c2_duration_plus_number_rejected() {
    let code = r#"spec test
uses lemma units
rule value: 1 day + (2024-02-15...2024-03-15) as day as number"#;
    expect_plan_error(code, "Cannot apply '+'");
}

#[test]
fn c4_date_range_times_number() {
    let code = r#"spec test
uses lemma units
rule scaled: (((2024-02-28...2024-03-01 as day) * 3) as day) as number"#;
    let val = eval_rule(code, "test", "scaled");
    assert_contains_all(&val, &["6"]);
}

// =============================================================================
// D. DateRange +/- Calendar -> Duration span
// =============================================================================

#[test]
fn d1_plus_calendar_months_uses_span() {
    let code = r#"spec test
uses lemma units
rule value: ((2024-01-01...2024-06-15) + 1 month) as month as number"#;
    let val = eval_rule(code, "test", "value");
    assert_contains_all(&val, &["6"]);
}

#[test]
fn d2_plus_calendar_years_uses_span() {
    let code = r#"spec test
uses lemma units
rule value: ((2024-01-01...2024-06-15) + 1 year) as month as number"#;
    let val = eval_rule(code, "test", "value");
    assert_contains_all(&val, &["17"]);
}

#[test]
fn d3_plus_calendar_in_days_uses_span() {
    let code = r#"spec test
uses lemma units
rule value: ((2024-01-01...2024-06-15) + 1 month) as day as number"#;
    let val = eval_rule(code, "test", "value");
    assert_contains_all(&val, &["196"]);
}

#[test]
fn d4_minus_calendar_uses_span() {
    let code = r#"spec test
uses lemma units
rule value: ((2024-01-01...2024-06-15) - 2 month) as month as number"#;
    let val = eval_rule(code, "test", "value");
    assert_contains_all(&val, &["3"]);
}

// =============================================================================
// E. DateRange comparison
// =============================================================================

#[test]
fn e1_gte_duration() {
    let code = r#"spec test
uses lemma units
rule ok: (2024-06-01...2024-06-15) >= 7 day"#;
    assert!(eval_bool(code, "test", "ok", &default_effective()));
}

#[test]
fn e2_lt_duration() {
    let code = r#"spec test
uses lemma units
rule ok: (2024-06-01...2024-06-15) < 7 day"#;
    assert!(!eval_bool(code, "test", "ok", &default_effective()));
}

#[test]
fn e3_gte_calendar() {
    let code = r#"spec test
uses lemma units
rule ok: (2024-01-01...2024-06-15) >= 3 month"#;
    assert!(eval_bool(code, "test", "ok", &default_effective()));
}

#[test]
fn e4_gte_number_after_conversion() {
    let code = r#"spec test
uses lemma units
rule ok: (2024-06-01...2024-06-15) as day as number >= 7"#;
    assert!(eval_bool(code, "test", "ok", &default_effective()));
}

// =============================================================================
// F. Measure * DateRange
// =============================================================================

#[test]
fn f1_hours() {
    let code = r#"spec test
uses lemma units
data money: measure -> unit eur: 1.00
data rate: measure
  -> unit eur_per_second: eur/second
  -> unit eur_per_hour: eur/hour
data hourly_rate: 50 eur_per_hour
rule cost: (hourly_rate * (2024-01-01...2024-01-02 as hour))"#;
    let val = eval_rule(code, "test", "cost");
    assert_contains_all(&val, &["1200", "eur"]);
}

#[test]
fn f2_months() {
    let code = r#"spec test
uses lemma units
data money: measure -> unit eur: 1.00
data monthly_rate: measure -> unit eur_per_month: eur/month
data rate: 50 eur_per_month
rule cost: (rate * (2024-01-01...2024-06-15 as month))"#;
    let val = eval_rule(code, "test", "cost");
    assert_contains_all(&val, &["250", "eur"]);
}

// =============================================================================
// G. Calendar edge cases
// =============================================================================

#[test]
fn g1_same_date_zero_years() {
    let code = r#"spec test
uses lemma units
rule span: 2024-06-15...2024-06-15 as year as number"#;
    let val = eval_rule(code, "test", "span");
    assert_contains_all(&val, &["0"]);
}

#[test]
fn g2_same_date_zero_months() {
    let code = r#"spec test
uses lemma units
rule span: 2024-06-15...2024-06-15 as month as number"#;
    let val = eval_rule(code, "test", "span");
    assert_contains_all(&val, &["0"]);
}

#[test]
fn g3_one_day_short_of_year() {
    let code = r#"spec test
uses lemma units
rule span: 2024-01-15...2025-01-14 as year as number"#;
    // 364-day span is 11 calendar month → 11/12 year when de-canonicalized to year.
    assert_eq!(
        eval_rule_number(code, "test", "span"),
        Decimal::from_str("0.9166666666666666666666666667").expect("span decimal")
    );
}

#[test]
fn g4_exactly_one_year() {
    let code = r#"spec test
uses lemma units
rule span: 2024-01-15...2025-01-15 as year as number"#;
    let val = eval_rule(code, "test", "span");
    assert_contains_all(&val, &["1"]);
}

#[test]
fn g5_leap_year_boundary() {
    let code = r#"spec test
uses lemma units
rule span: 2023-03-01...2024-02-29 as month as number"#;
    let val = eval_rule(code, "test", "span");
    assert_contains_all(&val, &["11"]);
}

#[test]
fn g6_reversed_range_span_is_absolute_months() {
    let code = r#"spec test
uses lemma units
rule span: 2024-06-01...2024-01-01 as month as number"#;
    let val = eval_rule(code, "test", "span");
    assert_contains_all(&val, &["5"]);
}

#[test]
fn g6b_reversed_date_range_span_as_days_matches_forward() {
    let code = r#"spec test
uses lemma units
rule forward: (2024-01-01...2024-01-03) as day as number
rule reversed: (2024-01-03...2024-01-01) as day as number"#;
    assert_eq!(
        eval_rule(code, "test", "forward"),
        eval_rule(code, "test", "reversed")
    );
}

#[test]
fn g7_reversed_range_span_is_absolute_years() {
    let code = r#"spec test
uses lemma units
rule span: 2024-01-01...2020-01-01 as year as number"#;
    let val = eval_rule(code, "test", "span");
    assert_contains_all(&val, &["4"]);
}

#[test]
fn g8_partial_month_floor() {
    let code = r#"spec test
uses lemma units
rule span: 2024-01-16...2024-02-15 as month as number"#;
    let val = eval_rule(code, "test", "span");
    assert_contains_all(&val, &["0"]);
}

#[test]
fn g9_exactly_one_month() {
    let code = r#"spec test
uses lemma units
rule span: 2024-01-16...2024-02-16 as month as number"#;
    let val = eval_rule(code, "test", "span");
    assert_contains_all(&val, &["1"]);
}

// =============================================================================
// H. Regression: date arithmetic remains readable
// =============================================================================

#[test]
fn h1_date_plus_months_plus_hours() {
    let code = r#"spec test
uses lemma units
rule end: 2024-01-15 + 2 month + 12 hour"#;
    let val = eval_rule(code, "test", "end");
    assert_contains_all(&val, &["2024-03-15", "12:00:00"]);
}

#[test]
fn h2_date_plus_years_plus_days() {
    let code = r#"spec test
uses lemma units
rule end: 2024-01-01 + 1 year + 30 day"#;
    let val = eval_rule(code, "test", "end");
    assert_contains_all(&val, &["2025-01-31"]);
}

#[test]
fn h3_date_plus_months_minus_days() {
    let code = r#"spec test
uses lemma units
rule end: 2024-06-15 + 3 month - 10 day"#;
    let val = eval_rule(code, "test", "end");
    assert_contains_all(&val, &["2024-09-05"]);
}

// =============================================================================
// I. Integration: 05_date_handling patterns rewritten with `...`
// =============================================================================

#[test]
fn i1_employee_age() {
    let code = r#"spec test
uses lemma units
data employee_birth_date: 1990-05-20
data current_date: 2024-06-15
rule employee_age: employee_birth_date...current_date as year as number"#;
    let val = eval_rule(code, "test", "employee_age");
    assert_contains_all(&val, &["34"]);
}

#[test]
fn i2_years_employed_indirect() {
    let code = r#"spec test
uses lemma units
data hire_date: 2024-03-01
data current_date: 2026-03-01
rule time_employed: hire_date...current_date
rule years_employed: time_employed as year as number"#;
    let val = eval_rule(code, "test", "years_employed");
    assert_contains_all(&val, &["2"]);
}

#[test]
fn i3_age_comparison() {
    let code = r#"spec test
uses lemma units
data d1: 2000-01-01
data d2: 2026-01-01
rule age: d1...d2 as year as number
rule adult: age >= 18"#;
    assert!(eval_bool(code, "test", "adult", &default_effective()));
}

#[test]
fn i4_candrive_pattern() {
    let code = r#"spec test
uses lemma units
data dob: 2000-01-01
rule candrive: dob...now >= 16 year"#;
    assert!(eval_bool(
        code,
        "test",
        "candrive",
        &effective(2026, 1, 1, 0, 0, 0)
    ));
}

// =============================================================================
// J. Regression: duration/calendar split
// =============================================================================

#[test]
fn j1_time_plus_time() {
    let code = r#"spec test
uses lemma units
rule total: (1 day + 12 hour) as hour as number"#;
    let val = eval_rule(code, "test", "total");
    assert_contains_all(&val, &["36"]);
}

#[test]
fn j2_calendar_plus_calendar() {
    let code = r#"spec test
uses lemma units
rule total: (1 year + 6 month) as month"#;
    let val = eval_rule(code, "test", "total");
    assert_contains_all(&val, &["18"]);
}

#[test]
fn j3_time_plus_calendar_rejected() {
    let code = r#"spec test
uses lemma units
rule total: 1 week + 1 month"#;
    expect_plan_error(code, "duration and calendar");
}

#[test]
fn j4_duration_ref_as_number_rejected() {
    let code = r#"spec test
uses lemma units
data elapsed: units.duration
data d: elapsed -> suggest 90 day
rule total: d as number"#;
    expect_plan_error(code, "Cannot use 'as number' on measure");
}

#[test]
fn j5_calendar_ref_as_days_rejected() {
    let code = r#"spec test
uses lemma units
data span: units.calendar -> suggest 6 month
rule total: span as day as number"#;
    expect_plan_error(code, "as day");
}

#[test]
fn j6_duration_slot_rejects_calendar() {
    let code = r#"spec test
uses lemma units
data duration: units.duration
data x: duration -> suggest 1 month"#;
    expect_plan_error(code, "Unit 'month' is for calendar data");
    expect_plan_error(code, "Valid 'duration' units are");
}

// =============================================================================
// K. DateRange vs duration slots (no implicit coercion)
// =============================================================================

#[test]
fn k1_duration_slot_rejects_date_range_default() {
    let code = r#"spec test
uses lemma units
data elapsed: duration -> suggest 2024-01-01...2024-01-02"#;
    expect_plan_error(code, "duration");
}

#[test]
fn k1_date_range_literal_span_as_days() {
    let code = r#"spec test
uses lemma units
rule span: 2024-01-01...2024-01-02 as day as number"#;
    let val = eval_rule(code, "test", "span");
    assert_contains_all(&val, &["1"]);
}

#[test]
fn k2_rejects_calendar_slot() {
    let code = r#"spec test
uses lemma units
data elapsed: units.calendar -> suggest 2024-01-01...2024-06-15"#;
    expect_plan_error(code, "calendar");
}

// =============================================================================
// L. Literal syntax and declared type
// =============================================================================

#[test]
fn l1_literal_basic() {
    let code = r#"spec test
uses lemma units
rule span: 2024-01-01...2024-06-15 as day as number"#;
    let val = eval_rule(code, "test", "span");
    assert_contains_all(&val, &["166"]);
}

#[test]
fn l2_literal_with_spaces() {
    let code = r#"spec test
uses lemma units
rule span: 2024-01-01 ... 2024-06-15 as month as number"#;
    let val = eval_rule(code, "test", "span");
    assert_contains_all(&val, &["5"]);
}

#[test]
fn l3_literal_in_years() {
    let code = r#"spec test
uses lemma units
rule span: 1990-05-20 ... 2024-06-15 as year as number"#;
    let val = eval_rule(code, "test", "span");
    assert_contains_all(&val, &["34"]);
}

#[test]
fn l4_literal_containment() {
    let code = r#"spec test
rule check: 2024-03-15 in 2024-01-01...2024-06-15"#;
    assert!(eval_bool(code, "test", "check", &default_effective()));
}

#[test]
fn l4b_reversed_date_range_containment() {
    let code = r#"spec test
rule check: 2028-06-15 in 2030-01-01...2024-01-01"#;
    assert!(eval_bool(code, "test", "check", &default_effective()));
}

#[test]
fn l4c_reversed_date_range_upper_endpoint_excluded() {
    let code = r#"spec test
rule check: 2028-01-01 in 2028-01-01...2024-01-01"#;
    assert!(!eval_bool(code, "test", "check", &default_effective()));
}

#[test]
fn l5_user_declared_type() {
    let code = r#"spec test
uses lemma units
data period: date range -> suggest 2024-01-01...2025-01-01
rule month: period as month as number"#;
    let mut data = HashMap::new();
    data.insert("period".to_string(), "2024-01-01...2025-01-01".into());
    let val = eval_literal_with_data(code, "test", "month", &default_effective(), data).to_string();
    assert_contains_all(&val, &["12"]);
}

#[test]
fn l6_user_declared_no_default() {
    let code = r#"spec test
data period: date range
data event: 2024-03-15
rule check: event in period"#;
    let mut data = HashMap::new();
    data.insert("period".to_string(), "2024-01-01...2024-12-31".into());
    let lit = eval_literal_with_data(code, "test", "check", &default_effective(), data);
    match lit.value {
        ValueKind::Boolean(val) => assert!(val),
        other => panic!("Expected Boolean, got {:?}", other),
    }
}

// =============================================================================
// M. Containment and past/future range
// =============================================================================

#[test]
fn m1_past_containment() {
    let code = r#"spec test
uses lemma units
data event: 2026-03-05
rule check: event in past 7 day"#;
    assert!(eval_bool(code, "test", "check", &default_effective()));
}

#[test]
fn m2_past_outside() {
    let code = r#"spec test
uses lemma units
data event: 2026-02-15
rule check: event in past 7 day"#;
    assert!(!eval_bool(code, "test", "check", &default_effective()));
}

#[test]
fn m3_future_containment() {
    let code = r#"spec test
uses lemma units
data event: 2026-03-20
rule check: event in future 30 day"#;
    assert!(eval_bool(code, "test", "check", &default_effective()));
}

#[test]
fn m4_past_as_standalone_rule() {
    let code = r#"spec test
uses lemma units
data event: 2026-03-05
rule window: past 7 day
rule check: event in window"#;
    assert!(eval_bool(code, "test", "check", &default_effective()));
}

#[test]
fn m5_containment_via_rule() {
    let code = r#"spec test
data start: 2024-01-01
data end_date: 2024-06-15
data event: 2024-03-15
rule window: start...end_date
rule check: event in window"#;
    assert!(eval_bool(code, "test", "check", &default_effective()));
}

#[test]
fn m6_past_calendar() {
    let code = r#"spec test
uses lemma units
data event: 2026-02-15
rule check: event in past 3 month"#;
    assert!(eval_bool(code, "test", "check", &default_effective()));
}

// =============================================================================
// N. Date - Date rejection
// =============================================================================

#[test]
fn n1_date_minus_date_rejected() {
    let code = r#"spec test
rule range: 2024-06-15 - 2024-01-01"#;
    expect_plan_error(code, "date range");
}

#[test]
fn n2_with_conversion_rejected() {
    let code = r#"spec test
data dob: 2000-01-01
rule age: (now - dob) as number"#;
    expect_plan_error(code, "date range");
}

#[test]
fn n3_indirect_rejected() {
    let code = r#"spec test
data d1: date
data d2: date
rule range: d1 - d2"#;
    expect_plan_error(code, "date range");
}

// =============================================================================
// Additional challenging coverage around `now` and boundaries
// =============================================================================

#[test]
fn date_range_can_cross_march_first_boundary() {
    let code = r#"spec test
uses lemma units
rule span: 2024-02-28...2024-03-01 as day as number"#;
    let val = eval_rule(code, "test", "span");
    assert_contains_all(&val, &["2"]);
}

#[test]
fn date_range_upper_bound_is_excluded_at_march_first() {
    let code = r#"spec test
rule check: 2024-03-01 in 2024-02-28...2024-03-01"#;
    assert!(!eval_bool(code, "test", "check", &default_effective()));
}

#[test]
fn date_range_just_inside_right_boundary() {
    let code = r#"spec test
rule check: 2024-02-29 in 2024-02-28...2024-03-01"#;
    assert!(eval_bool(code, "test", "check", &default_effective()));
}

#[test]
fn past_range_is_window_relative_to_effective_time() {
    let code = r#"spec test
uses lemma units
data event: 2026-02-28T12:00:00Z
rule check: event in past 7 day"#;
    let val = eval_bool(code, "test", "check", &effective(2026, 3, 7, 12, 0, 0));
    assert!(val);
}

#[test]
fn future_range_is_window_relative_to_effective_time() {
    let code = r#"spec test
uses lemma units
data event: 2026-03-14T11:00:00Z
rule check: event in future 7 day"#;
    let val = eval_bool(code, "test", "check", &effective(2026, 3, 7, 12, 0, 0));
    assert!(val);
}

#[test]
fn candrive_pattern_reads_cleanly_with_now() {
    let code = r#"spec test
uses lemma units
data dob: 2012-03-07
rule candrive: dob...now >= 16 year"#;
    let val = eval_bool(code, "test", "candrive", &effective(2026, 3, 7, 12, 0, 0));
    assert!(!val);
}

#[test]
fn readable_shifted_range_expression() {
    let code = r#"spec test
uses lemma units
data dob: 2000-01-01
rule older: (dob + 12 year)...now as year as number"#;
    let val = eval_rule_with_effective(code, "test", "older", &effective(2026, 1, 1, 0, 0, 0));
    assert_contains_all(&val, &["14"]);
}

// =============================================================================
// N. premium_membership containment
// =============================================================================

const PREMIUM_MEMBERSHIP_SPEC: &str = r#"spec premium_membership
uses lemma units
data start: date
data length: units.duration
rule valid: now in start...start + length
"#;

#[test]
fn premium_membership_outside_period() {
    let mut data = HashMap::new();
    data.insert("start".to_string(), "2026-07-01".into());
    data.insert("length".to_string(), "10 day".into());
    let val = eval_bool_with_data(
        PREMIUM_MEMBERSHIP_SPEC,
        "premium_membership",
        "valid",
        &effective(2027, 2, 14, 12, 0, 0),
        data,
    );
    assert!(!val, "2027-02-14 is outside [2026-07-01, 2026-07-11)");
}

#[test]
fn premium_membership_inside_period() {
    let mut data = HashMap::new();
    data.insert("start".to_string(), "2026-07-01".into());
    data.insert("length".to_string(), "10 day".into());
    let val = eval_bool_with_data(
        PREMIUM_MEMBERSHIP_SPEC,
        "premium_membership",
        "valid",
        &effective(2026, 7, 5, 12, 0, 0),
        data,
    );
    assert!(val, "2026-07-05 is inside [2026-07-01, 2026-07-11)");
}

#[test]
fn premium_membership_rejects_parenthesized_span_add_with_in() {
    let code = r#"spec premium_membership_bad
uses lemma units
data start: date
data length: units.duration
rule bad: now in (start...start) + length
"#;
    expect_plan_error(code, "Right side of 'in' must be a range");
}

fn eval_bool_with_data(
    code: &str,
    spec_name: &str,
    rule_name: &str,
    effective: &DateTimeValue,
    data: HashMap<String, String>,
) -> bool {
    match eval_literal_with_data(code, spec_name, rule_name, effective, data).value {
        ValueKind::Boolean(val) => val,
        other => panic!("Expected Boolean, got {:?}", other),
    }
}

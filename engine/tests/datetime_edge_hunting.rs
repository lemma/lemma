use lemma::{DateGranularity, DateTimeValue, Engine, TimezoneValue, ValueKind};
use std::collections::HashMap;

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

fn effective_us(y: i32, m: u32, d: u32, h: u32, min: u32, s: u32, us: u32) -> DateTimeValue {
    DateTimeValue {
        year: y,
        month: m,
        day: d,
        hour: h,
        minute: min,
        second: s,
        microsecond: us,
        timezone: Some(TimezoneValue {
            offset_hours: 0,
            offset_minutes: 0,
        }),
        granularity: DateGranularity::DateTime,
    }
}

fn eval_bool(engine: &Engine, spec_name: &str, rule: &str, eff: &DateTimeValue) -> bool {
    eval_bool_with_datas(engine, spec_name, rule, eff, HashMap::new())
}

fn eval_bool_with_datas(
    engine: &Engine,
    spec_name: &str,
    rule: &str,
    eff: &DateTimeValue,
    data: HashMap<String, String>,
) -> bool {
    let response = engine
        .run(None, spec_name, Some(eff), data, None, true)
        .unwrap();
    let rr = response
        .results
        .values()
        .find(|r| r.rule.name == rule)
        .unwrap_or_else(|| panic!("rule '{}' not found", rule));
    if rr.vetoed {
        panic!(
            "rule '{}' vetoed: {}",
            rule,
            rr.veto_reason.as_deref().unwrap_or("Vetoed")
        );
    }
    rr.result
        .as_ref()
        .expect("rule result value")
        .boolean
        .expect("boolean rule result")
}

fn eval_value(
    engine: &Engine,
    spec_name: &str,
    rule: &str,
    eff: &DateTimeValue,
) -> lemma::LiteralValue {
    let response = engine
        .run(None, spec_name, Some(eff), HashMap::new(), None, true)
        .unwrap();
    let rr = response
        .results
        .values()
        .find(|r| r.rule.name == rule)
        .unwrap_or_else(|| panic!("rule '{}' not found", rule));
    if rr.vetoed {
        panic!(
            "rule '{}' vetoed: {}",
            rule,
            rr.veto_reason.as_deref().unwrap_or("Vetoed")
        );
    }
    rr.explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("BUG: non-vetoed rule missing value")
        .clone()
}

// =============================================================================
// now with itself: self-referential edge cases
// =============================================================================

#[test]
fn now_in_past_is_always_false() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
rule check: now in past
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    assert!(!eval_bool(
        &engine,
        "test",
        "check",
        &effective(2026, 3, 7, 12, 0, 0)
    ));
}

#[test]
fn now_in_future_is_always_false() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
rule check: now in future
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    assert!(!eval_bool(
        &engine,
        "test",
        "check",
        &effective(2026, 3, 7, 12, 0, 0)
    ));
}

#[test]
fn now_in_past_0_days_excludes_now() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
rule check: now in past 0 day
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    assert!(!eval_bool(
        &engine,
        "test",
        "check",
        &effective(2026, 3, 7, 12, 0, 0)
    ));
}

#[test]
fn now_in_future_0_days_excludes_now() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
rule check: now in future 0 day
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    assert!(!eval_bool(
        &engine,
        "test",
        "check",
        &effective(2026, 3, 7, 12, 0, 0)
    ));
}

// =============================================================================
// ISO week boundary: year crossover
// =============================================================================

#[test]
fn calendar_week_iso_year_boundary_dec_31_2025() {
    // 2025-12-29 is Monday of ISO week 1 of 2026
    // So 2025-12-31 (Wednesday) is in ISO week 1 of 2026
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data event_date: 2025-12-31
rule check: event_date in calendar week
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    // effective on 2025-12-30 (Tuesday) is also ISO week 1 of 2026
    let eff = effective(2025, 12, 30, 12, 0, 0);
    assert!(eval_bool(&engine, "test", "check", &eff));
}

#[test]
fn calendar_week_iso_week_53() {
    // 2026-12-31 is Thursday. ISO week: let's verify
    // 2026-12-28 is Monday → ISO week 53 of 2026
    // 2027-01-03 is Sunday → still ISO week 53 of 2026
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data event_date: 2027-01-01
rule check: event_date in calendar week
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    // effective is 2026-12-31 (Thursday), ISO week 53 of 2026
    let eff = effective(2026, 12, 31, 12, 0, 0);
    assert!(eval_bool(&engine, "test", "check", &eff));
}

#[test]
fn past_calendar_week_from_week_2_wraps_to_week_1() {
    // now is 2026-01-05 (Monday), ISO week 2 of 2026
    // past week = ISO week 1 of 2026, which starts 2025-12-29
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data event_date: 2025-12-30
rule check: event_date in past calendar week
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let eff = effective(2026, 1, 5, 12, 0, 0);
    assert!(eval_bool(&engine, "test", "check", &eff));
}

#[test]
fn past_calendar_week_from_week_1_wraps_to_previous_year() {
    // now is 2025-12-31 (Wednesday), ISO week 1 of 2026
    // past week = ISO week 52 of 2025: Mon 2025-12-22 through Sun 2025-12-28
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data event_date: 2025-12-25
rule check: event_date in past calendar week
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let eff = effective(2025, 12, 31, 12, 0, 0);
    assert!(eval_bool(&engine, "test", "check", &eff));
}

#[test]
fn future_calendar_week_from_week_52_wraps_to_next_year() {
    // now is 2025-12-24 (Wednesday), ISO week 52 of 2025
    // future week = ISO week 1 of 2026: Mon 2025-12-29 through Sun 2026-01-04
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data event_date: 2026-01-02
rule check: event_date in future calendar week
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let eff = effective(2025, 12, 24, 12, 0, 0);
    assert!(eval_bool(&engine, "test", "check", &eff));
}

// =============================================================================
// date sugar with computed expressions
// =============================================================================

#[test]
fn in_past_with_date_arithmetic() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data start_date: 2026-02-01
rule check: (start_date + 30 day) in past
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    // start_date + 30 day = 2026-03-03, which is before 2026-03-07
    let eff = effective(2026, 3, 7, 12, 0, 0);
    assert!(eval_bool(&engine, "test", "check", &eff));
}

#[test]
fn in_future_with_date_arithmetic() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data start_date: 2026-03-01
rule check: (start_date + 30 day) in future
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    // start_date + 30 day = 2026-03-31, which is after 2026-03-07
    let eff = effective(2026, 3, 7, 12, 0, 0);
    assert!(eval_bool(&engine, "test", "check", &eff));
}

#[test]
fn now_plus_duration_produces_future_date() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
rule deadline: now + 30 day
rule is_future: deadline in future
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let eff = effective(2026, 3, 7, 12, 0, 0);
    assert!(eval_bool(&engine, "test", "is_future", &eff));
}

#[test]
fn now_minus_duration_produces_past_date() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
rule past_point: now - 30 day
rule is_past: past_point in past
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let eff = effective(2026, 3, 7, 12, 0, 0);
    assert!(eval_bool(&engine, "test", "is_past", &eff));
}

// =============================================================================
// veto propagation through date sugar
// =============================================================================

#[test]
fn vetoed_date_dependency_propagates_through_in_past() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
rule event_date: veto "no event"
rule check: event_date in past
"#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("pure-veto date sugar must plan");
    let eff = effective(2026, 3, 7, 12, 0, 0);
    let response = engine
        .run(None, "test", Some(&eff), HashMap::new(), None, false)
        .expect("run");
    let check = response
        .results
        .values()
        .find(|r| r.rule.name == "check")
        .expect("check");
    assert!(
        check.vetoed,
        "check must veto when event_date vetoes: {:?}",
        check.veto_reason
    );
    assert!(
        check
            .veto_reason
            .as_deref()
            .is_some_and(|r| r.contains("no event")),
        "veto reason must propagate through date sugar: {:?}",
        check.veto_reason
    );
}

#[test]
fn vetoed_date_dependency_propagates_through_calendar_and_tolerance_sugar() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
rule event_date: veto "no event"
rule calendar_check: event_date in past calendar week
rule tolerance_check: event_date in past 1 day
"#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("pure-veto calendar/tolerance sugar must plan");
    let eff = effective(2026, 3, 7, 12, 0, 0);
    let response = engine
        .run(None, "test", Some(&eff), HashMap::new(), None, false)
        .expect("run");
    for name in ["calendar_check", "tolerance_check"] {
        let rule = response
            .results
            .values()
            .find(|r| r.rule.name == name)
            .unwrap_or_else(|| panic!("{name}"));
        assert!(
            rule.vetoed,
            "{name} must veto when event_date vetoes: {:?}",
            rule.veto_reason
        );
        assert!(
            rule.veto_reason
                .as_deref()
                .is_some_and(|r| r.contains("no event")),
            "{name} veto reason must propagate: {:?}",
            rule.veto_reason
        );
    }
}

// =============================================================================
// tolerance unit varieties
// =============================================================================

#[test]
fn in_past_weeks_tolerance() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data event: 2026-03-01
rule check: event in past 2 week
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    // 2 week = 14 day, window starts 2026-02-21T12:00:00Z
    let eff = effective(2026, 3, 7, 12, 0, 0);
    assert!(eval_bool(&engine, "test", "check", &eff));
}

#[test]
fn in_past_hours_tolerance() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data event: 2026-03-07T08:00:00Z
rule check: event in past 6 hour
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let eff = effective(2026, 3, 7, 12, 0, 0);
    assert!(eval_bool(&engine, "test", "check", &eff));
}

#[test]
fn in_past_minutes_tolerance() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data event: 2026-03-07T11:30:00Z
rule check: event in past 45 minute
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let eff = effective(2026, 3, 7, 12, 0, 0);
    assert!(eval_bool(&engine, "test", "check", &eff));
}

#[test]
fn in_future_weeks_tolerance() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data event: 2026-03-15
rule check: event in future 2 week
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let eff = effective(2026, 3, 7, 12, 0, 0);
    assert!(eval_bool(&engine, "test", "check", &eff));
}

// =============================================================================
// date without time vs now with time
// =============================================================================

#[test]
fn date_only_compared_with_now_with_time() {
    // data is 2026-03-07 (no time = midnight), now is 2026-03-07T12:00
    // midnight < noon, so date is "in past"
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data event: 2026-03-07
rule check: event in past
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let eff = effective(2026, 3, 7, 12, 0, 0);
    assert!(eval_bool(&engine, "test", "check", &eff));
}

#[test]
fn date_only_at_midnight_vs_now_at_midnight() {
    // Both are exactly midnight → date == now → "in past" is false (strict <)
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data event: 2026-03-07
rule check: event in past
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let eff = effective(2026, 3, 7, 0, 0, 0);
    assert!(!eval_bool(&engine, "test", "check", &eff));
}

// =============================================================================
// timezone cross-comparisons
// =============================================================================

#[test]
fn timezone_date_east_vs_utc_now() {
    // event at 2026-03-07T01:00:00+05:00 = 2026-03-06T20:00:00Z
    // now at 2026-03-06T21:00:00Z
    // event (UTC 20:00) < now (UTC 21:00) → in past
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data event: 2026-03-07T01:00:00+05:00
rule check: event in past
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let eff = effective(2026, 3, 6, 21, 0, 0);
    assert!(eval_bool(&engine, "test", "check", &eff));
}

#[test]
fn timezone_date_west_vs_utc_now() {
    // event at 2026-03-06T20:00:00-05:00 = 2026-03-07T01:00:00Z
    // now at 2026-03-07T00:00:00Z
    // event (UTC 01:00) > now (UTC 00:00) → in future
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data event: 2026-03-06T20:00:00-05:00
rule check: event in future
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let eff = effective(2026, 3, 7, 0, 0, 0);
    assert!(eval_bool(&engine, "test", "check", &eff));
}

#[test]
fn calendar_year_different_timezones_same_utc_instant() {
    // event at 2027-01-01T00:30:00+02:00 = 2026-12-31T22:30:00Z
    // now at 2026-06-15T12:00:00Z
    // Calendar year boundary for now (UTC) is 2026-01-01T00:00:00Z to 2026-12-31T23:59:59.999999Z
    // event in UTC is 2026-12-31T22:30:00Z which is inside
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data event: 2027-01-01T00:30:00+02:00
rule check: event in calendar year
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let eff = effective(2026, 6, 15, 12, 0, 0);
    assert!(eval_bool(&engine, "test", "check", &eff));
}

// =============================================================================
// fractional second round-trip
// =============================================================================

#[test]
fn fractional_seconds_in_datetime_literal() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data event: 2026-03-07T12:00:00.500000Z
data event2: 2026-03-07T12:00:00.499999Z
rule check: event > event2
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let eff = effective(2026, 3, 7, 12, 0, 0);
    assert!(eval_bool(&engine, "test", "check", &eff));
}

#[test]
fn fractional_seconds_in_now_effective() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data event: 2026-03-07T12:00:00.000001Z
rule check: event in past
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    // now has microsecond 2 → event (us=1) < now (us=2) → in past
    let eff = effective_us(2026, 3, 7, 12, 0, 0, 2);
    assert!(eval_bool(&engine, "test", "check", &eff));
}

#[test]
fn fractional_seconds_same_second_different_microsecond() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data event: 2026-03-07T12:00:00.000001Z
rule check: event in future
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    // now has microsecond 0, event has microsecond 1 → event > now → in future
    let eff = effective(2026, 3, 7, 12, 0, 0);
    assert!(eval_bool(&engine, "test", "check", &eff));
}

// =============================================================================
// date sugar in complex rule chains
// =============================================================================

#[test]
fn date_sugar_result_used_in_unless() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data order_date: 2026-03-05
data amount: 100
rule discount: 0
  unless order_date in past 3 day then 10
  unless order_date in past 3 day and amount > 50 then 20
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let eff = effective(2026, 3, 7, 12, 0, 0);
    let lit = eval_value(&engine, "test", "discount", &eff);
    if let ValueKind::Number(n) = &lit.value {
        assert_eq!(
            lemma::ValueKind::Number(n.clone())
                .as_decimal_magnitude()
                .unwrap(),
            rust_decimal::Decimal::from(20)
        );
    } else {
        panic!("expected Number, got {:?}", lit.value);
    }
}

#[test]
fn now_arithmetic_in_rule_chain() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
rule thirty_days_ago: now - 30 day
rule sixty_days_ago: now - 60 day
rule window_size: sixty_days_ago...thirty_days_ago as second as number
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let eff = effective(2026, 3, 7, 12, 0, 0);
    let lit = eval_value(&engine, "test", "window_size", &eff);
    if let ValueKind::Number(second) = &lit.value {
        assert_eq!(
            lemma::ValueKind::Number(second.clone())
                .as_decimal_magnitude()
                .unwrap(),
            rust_decimal::Decimal::from(2_592_000)
        );
    } else {
        panic!("expected Number, got {:?}", lit.value);
    }
}

// =============================================================================
// not operator combined with date sugar
// =============================================================================

#[test]
fn not_in_past_is_equivalent_to_not_past() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data event: 2026-04-01
rule check: not (event in past)
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let eff = effective(2026, 3, 7, 12, 0, 0);
    assert!(eval_bool(&engine, "test", "check", &eff));
}

#[test]
fn not_in_future_on_past_date() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data event: 2026-01-01
rule check: not (event in future)
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let eff = effective(2026, 3, 7, 12, 0, 0);
    assert!(eval_bool(&engine, "test", "check", &eff));
}

// =============================================================================
// formatting round-trip
// =============================================================================

#[test]
fn format_round_trip_now_keyword() {
    let input = "spec test
uses lemma units\n\nrule current: now\n";
    let formatted = lemma::format_source(input, lemma::SourceType::Volatile).unwrap();
    assert!(formatted.contains("now"), "got: {}", formatted);
    let reparsed = lemma::format_source(&formatted, lemma::SourceType::Volatile).unwrap();
    assert_eq!(formatted, reparsed, "double format should be idempotent");
}

// =============================================================================
// very old and very future dates
// =============================================================================

#[test]
fn very_old_date_in_past() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data event: 1900-01-01
rule check: event in past
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let eff = effective(2026, 3, 7, 12, 0, 0);
    assert!(eval_bool(&engine, "test", "check", &eff));
}

#[test]
fn very_old_date_not_in_calendar_year() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data event: 1900-01-01
rule check: event not in calendar year
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let eff = effective(2026, 3, 7, 12, 0, 0);
    assert!(eval_bool(&engine, "test", "check", &eff));
}

// =============================================================================
// leap year calendar month boundary
// =============================================================================

#[test]
fn calendar_month_feb_29_not_in_march() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data event: 2024-02-29
rule check: event in calendar month
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    // now is March 2024 → Feb 29 should NOT be in current calendar month
    let eff = effective(2024, 3, 7, 12, 0, 0);
    assert!(!eval_bool(&engine, "test", "check", &eff));
}

#[test]
fn past_calendar_month_feb_from_march_leap_year() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data event: 2024-02-29
rule check: event in past calendar month
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let eff = effective(2024, 3, 7, 12, 0, 0);
    assert!(eval_bool(&engine, "test", "check", &eff));
}

// =============================================================================
// large tolerance values
// =============================================================================

#[test]
fn in_past_365_days_tolerance() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data event: 2025-06-15
rule check: event in past 365 day
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let eff = effective(2026, 3, 7, 12, 0, 0);
    assert!(eval_bool(&engine, "test", "check", &eff));
}

#[test]
fn in_past_365_days_outside() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data event: 2024-01-01
rule check: event in past 365 day
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let eff = effective(2026, 3, 7, 12, 0, 0);
    assert!(!eval_bool(&engine, "test", "check", &eff));
}

// =============================================================================
// multiple calendar checks on different dates
// =============================================================================

#[test]
fn two_dates_different_calendar_checks() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data start_date: 2026-03-05
data end_date: 2025-09-15
rule start_this_month: start_date in calendar month
rule end_last_year: end_date in past calendar year
rule both: start_this_month and end_last_year
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let eff = effective(2026, 3, 7, 12, 0, 0);
    assert!(eval_bool(&engine, "test", "both", &eff));
}

// =============================================================================
// edge: effective at exact year/month/week boundaries
// =============================================================================

#[test]
fn effective_at_jan_1_midnight_calendar_year_boundary() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data event: 2025-12-31T23:59:59Z
rule check: event in past calendar year
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    // now = 2026-01-01T00:00:00Z → past calendar year = 2025
    let eff = effective(2026, 1, 1, 0, 0, 0);
    assert!(eval_bool(&engine, "test", "check", &eff));
}

#[test]
fn effective_at_month_1_midnight_calendar_month_boundary() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data event: 2026-02-28T23:59:59Z
rule check: event in past calendar month
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let eff = effective(2026, 3, 1, 0, 0, 0);
    assert!(eval_bool(&engine, "test", "check", &eff));
}

// =============================================================================
// ensuring `in past N day` with future date returns false
// =============================================================================

#[test]
fn in_past_tolerance_with_future_date() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data event: 2026-04-01
rule check: event in past 7 day
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let eff = effective(2026, 3, 7, 12, 0, 0);
    assert!(!eval_bool(&engine, "test", "check", &eff));
}

#[test]
fn in_future_tolerance_with_past_date() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data event: 2026-01-01
rule check: event in future 7 day
    "#;
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .unwrap();
    let eff = effective(2026, 3, 7, 12, 0, 0);
    assert!(!eval_bool(&engine, "test", "check", &eff));
}

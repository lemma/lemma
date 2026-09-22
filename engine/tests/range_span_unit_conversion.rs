//! Range span projection: `(lo...hi) as <unit> as number` = **width** of the interval.
//! All ranges require an explicit unit before `as number`; bare `as number` on
//! date/measure ranges is rejected.
//!
//! | Range type | Valid `as` targets | Must reject |
//! |------------|--------------------|-------------|
//! | **DateRange** | `as <duration_unit> as number`, `as year/month as number` | bare `as number`, mass units |
//! | **NumberRange** | `as number` (no unit needed) | mass, duration units |
//! | **MeasureRange** (any family) | `as <same-family unit> as number` | cross-family units, bare `as number` |
//! | **RatioRange** | `as <ratio_unit>` | duration, mass |
//!
//! Calendar measure-range span `as` is not supported.

use lemma::{DateGranularity, DateTimeValue, Engine, TimezoneValue};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

fn source() -> lemma::SourceType {
    lemma::SourceType::Path(Arc::new(PathBuf::from("range_span_unit_conversion.lemma")))
}

fn default_effective() -> DateTimeValue {
    DateTimeValue {
        year: 2026,
        month: 3,
        day: 7,
        hour: 12,
        minute: 0,
        second: 0,
        microsecond: 0,
        timezone: Some(TimezoneValue {
            offset_hours: 0,
            offset_minutes: 0,
        }),
        granularity: DateGranularity::DateTime,
    }
}

fn eval_rule(code: &str, spec_name: &str, rule_name: &str) -> String {
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
    response
        .results
        .get(rule_name)
        .unwrap_or_else(|| panic!("Rule '{}' not found", rule_name))
        .result()
        .expect("result")
        .to_string()
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
        combined
            .to_lowercase()
            .contains(&expected_fragment.to_lowercase()),
        "Expected error containing '{}', got: {}",
        expected_fragment,
        combined
    );
}

fn assert_no_range_syntax(actual: &str) {
    assert!(
        !actual.contains("..."),
        "Expected scalar output, got range-like '{}'",
        actual
    );
}

fn assert_contains_parts(actual: &str, parts: &[&str]) {
    assert_no_range_syntax(actual);
    let lower = actual.to_lowercase();
    for part in parts {
        assert!(
            lower.contains(&part.to_lowercase()),
            "Expected '{}' to contain '{}'",
            actual,
            part
        );
    }
}

const USES_UNITS: &str = "uses lemma units";

const MONEY: &str = r#"data money: measure
  -> unit eur: 1.00
  -> unit usd: 0.91"#;

const WEIGHT: &str = r#"data weight: measure
  -> unit stone: 1
  -> unit lb: 14"#;

// =============================================================================
// DateRange
// =============================================================================

mod date_range {
    use super::*;

    #[test]
    fn span_as_days() {
        let code = format!(
            r#"spec test
{USES_UNITS}
rule span: 2024-01-01...2024-01-11 as day as number"#
        );
        assert_contains_parts(&eval_rule(&code, "test", "span"), &["10"]);
    }

    #[test]
    fn span_as_seconds() {
        let code = format!(
            r#"spec test
{USES_UNITS}
rule span: 2024-01-01...2024-01-02 as second as number"#
        );
        assert_contains_parts(&eval_rule(&code, "test", "span"), &["86400"]);
    }

    #[test]
    fn span_as_years_calendar_unit() {
        let code = r#"spec test
uses lemma units
rule span: 2024-01-15...2025-01-15 as year as number"#;
        assert_contains_parts(&eval_rule(code, "test", "span"), &["1"]);
    }

    #[test]
    fn span_as_months_calendar_unit() {
        let code = r#"spec test
uses lemma units
rule span: 2024-01-16...2024-02-16 as month as number"#;
        assert_contains_parts(&eval_rule(code, "test", "span"), &["1"]);
    }

    #[test]
    fn rejects_bare_as_number_on_date_range() {
        // Date range `as number` without a unit is not allowed.
        let code = format!(
            r#"spec test
{USES_UNITS}
rule bad: 2024-01-01...2024-01-11 as number"#
        );
        expect_plan_error(&code, "unit");
    }

    #[test]
    fn rejects_span_as_mass_unit() {
        let code = format!(
            r#"spec test
{USES_UNITS}
{WEIGHT}
rule bad: 2024-01-01...2024-01-11 as stone as number"#
        );
        expect_plan_error(&code, "convert");
    }

    #[test]
    fn rejects_span_as_ratio_unit() {
        let code = r#"spec test
rule bad: 2024-01-01...2024-01-11 as percent as number"#;
        expect_plan_error(code, "convert");
    }
}

// =============================================================================
// NumberRange
// =============================================================================

mod number_range {
    use super::*;

    // Number ranges have no unit, so `as number` is unambiguous and valid.

    #[test]
    fn span_as_number() {
        let code = r#"spec test
rule span: (0...100) as number
rule scalar: 100 as number"#;
        let span = eval_rule(code, "test", "span");
        let scalar = eval_rule(code, "test", "scalar");
        assert_contains_parts(&span, &["100"]);
        assert_eq!(span, scalar, "span and scalar cast should match");
    }

    #[test]
    fn span_48_as_number() {
        let code = r#"spec test
rule span: (0...48) as number"#;
        assert_contains_parts(&eval_rule(code, "test", "span"), &["48"]);
    }

    #[test]
    fn reversed_endpoints_span_as_number() {
        let code = r#"spec test
rule span: (100...0) as number"#;
        assert_contains_parts(&eval_rule(code, "test", "span"), &["100"]);
    }

    #[test]
    fn rejects_span_as_mass_unit() {
        // Number range `as <measure unit>` is rejected — no unit basis for span.
        let code = format!(
            r#"spec test
{USES_UNITS}
{WEIGHT}
rule bad: (0...100) as stone"#
        );
        expect_plan_error(&code, "convert");
    }

    #[test]
    fn rejects_span_as_ratio_unit() {
        let code = r#"spec test
rule bad: (0...100) as percent"#;
        expect_plan_error(code, "convert");
    }
}

// =============================================================================
// MeasureRange (trait duration / units.duration)
// =============================================================================

mod duration_measure_range {
    use super::*;

    #[test]
    fn span_as_days() {
        let code = format!(
            r#"spec test
{USES_UNITS}
rule span: (7 day...14 day) as day as number"#
        );
        assert_contains_parts(&eval_rule(&code, "test", "span"), &["7"]);
    }

    #[test]
    fn span_as_hours() {
        let code = format!(
            r#"spec test
{USES_UNITS}
rule span: (2 hour...5 hour) as hour as number"#
        );
        assert_contains_parts(&eval_rule(&code, "test", "span"), &["3"]);
    }

    #[test]
    fn span_as_seconds() {
        let code = format!(
            r#"spec test
{USES_UNITS}
rule span: (7 day...14 day) as second as number"#
        );
        assert_contains_parts(&eval_rule(&code, "test", "span"), &["604800"]);
    }

    #[test]
    fn span_as_weeks_mixed_day_endpoint() {
        let code = format!(
            r#"spec test
{USES_UNITS}
rule span: (7 day...2 week) as day as number"#
        );
        assert_contains_parts(&eval_rule(&code, "test", "span"), &["7"]);
    }

    #[test]
    fn rejects_bare_as_number_on_measure_range() {
        // Duration measure range `as number` without explicit unit is rejected.
        let code = format!(
            r#"spec test
{USES_UNITS}
rule bad: (7 day...14 day) as number"#
        );
        expect_plan_error(&code, "unit");
    }

    #[test]
    fn rejects_span_as_mass_unit() {
        let code = format!(
            r#"spec test
{USES_UNITS}
{WEIGHT}
rule bad: (7 day...14 day) as stone as number"#
        );
        expect_plan_error(&code, "convert");
    }

    #[test]
    fn rejects_span_as_ratio_unit() {
        let code = format!(
            r#"spec test
{USES_UNITS}
rule bad: (7 day...14 day) as percent as number"#
        );
        expect_plan_error(&code, "convert");
    }
}

// =============================================================================
// MeasureRange (mass / money — not a duration)
// =============================================================================

mod mass_measure_range {
    use super::*;

    #[test]
    fn rejects_span_as_duration_with_mass_endpoints() {
        // Mass measure range cannot produce a duration span.
        let code = format!(
            r#"spec test
{USES_UNITS}
{WEIGHT}
rule bad: (3 stone...5 stone) as day as number"#
        );
        expect_plan_error(&code, "convert");
    }

    #[test]
    fn span_as_same_mass_unit() {
        let code = format!(
            r#"spec test
{WEIGHT}
rule span: (3 stone...5 stone) as stone as number"#
        );
        assert_contains_parts(&eval_rule(&code, "test", "span"), &["2"]);
    }

    #[test]
    fn span_as_lb_cross_unit_within_family() {
        let code = format!(
            r#"spec test
{WEIGHT}
rule span: (1 stone...3 stone) as lb as number"#
        );
        assert_contains_parts(&eval_rule(&code, "test", "span"), &["28"]);
    }

    #[test]
    fn rejects_span_as_duration_nonsensical_unit_pairing() {
        let code = format!(
            r#"spec test
{USES_UNITS}
data cargo: measure -> unit crate: 1
rule bad: (3 crate...5 crate) as day as number"#
        );
        expect_plan_error(&code, "convert");
    }
}

mod money_measure_range {
    use super::*;

    #[test]
    fn rejects_span_as_duration_with_money_endpoints() {
        let code = format!(
            r#"spec test
{USES_UNITS}
{MONEY}
rule bad: (10 eur...50 eur) as day as number"#
        );
        expect_plan_error(&code, "convert");
    }

    #[test]
    fn span_as_same_money_unit() {
        let code = format!(
            r#"spec test
{MONEY}
rule span: (10 eur...50 eur) as eur as number"#
        );
        assert_contains_parts(&eval_rule(&code, "test", "span"), &["40"]);
    }

    #[test]
    fn span_as_usd_cross_unit_within_family() {
        let code = format!(
            r#"spec test
{MONEY}
rule span: (10 eur...50 eur) as usd as number"#
        );
        assert_contains_parts(&eval_rule(&code, "test", "span"), &["43.956"]);
    }
}

// =============================================================================
// RatioRange
// =============================================================================

mod ratio_range {
    use super::*;

    #[test]
    fn span_as_percent() {
        // Ratio range span in same ratio unit.
        let code = r#"spec test
rule span: (10%...50%) as percent"#;
        assert_contains_parts(&eval_rule(code, "test", "span"), &["40", "%"]);
    }

    #[test]
    fn span_as_permille() {
        let code = r#"spec test
rule span: (100 permille...500 permille) as permille"#;
        assert_contains_parts(&eval_rule(code, "test", "span"), &["400", "%%"]);
    }

    #[test]
    fn rejects_span_as_duration_unit() {
        let code = format!(
            r#"spec test
{USES_UNITS}
rule bad: (10%...50%) as day as number"#
        );
        expect_plan_error(&code, "convert");
    }

    #[test]
    fn rejects_span_as_mass_unit() {
        let code = format!(
            r#"spec test
{USES_UNITS}
{WEIGHT}
rule bad: (10%...50%) as stone as number"#
        );
        expect_plan_error(&code, "convert");
    }
}

// =============================================================================
// Calendar interval — span `as` not supported
// =============================================================================

mod calendar_range_span {
    use super::*;

    #[test]
    fn rejects_span_as_duration_unit() {
        // Calendar range span in duration units is not supported.
        let code = format!(
            r#"spec test
{USES_UNITS}
rule bad: (18 year...67 year) as day as number"#
        );
        expect_plan_error(&code, "convert");
    }

    #[test]
    fn rejects_bare_as_number() {
        let code = r#"spec test
uses lemma units
rule bad: (18 year...67 year) as number"#;
        expect_plan_error(code, "measure range");
    }
}

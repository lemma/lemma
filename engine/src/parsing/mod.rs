use crate::error::Error;
use crate::limits::ResourceLimits;

pub mod ast;
pub mod lexer;
pub mod parser;
pub mod source;

pub use ast::*;
pub use parser::{parse_repository_qualifier_str, ParseResult};

pub fn parse(
    content: &str,
    source_type: source::SourceType,
    limits: &ResourceLimits,
) -> Result<ParseResult, Error> {
    parser::parse(content, source_type, limits)
}

#[cfg(test)]
mod assignment_continuation_tests;

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::{parse, ArithmeticComputation, Expression, ExpressionKind};
    use crate::formatting::format_parse_result;
    use crate::Error;
    use crate::ResourceLimits;

    #[test]
    fn parse_empty_input_returns_no_specs() {
        let result = parse(
            "",
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap()
        .into_flattened_specs();
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn parse_workspace_file_yields_expected_spec_datas_and_rules() {
        let input = r#"spec person
data name: "John Doe"
rule adult: true"#;
        let result = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap()
        .into_flattened_specs();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "person");
        assert_eq!(result[0].data.len(), 1);
        assert_eq!(result[0].rules.len(), 1);
        assert_eq!(result[0].rules[0].name, "adult");
    }

    #[test]
    fn mixing_data_and_rules_is_collected_into_spec() {
        let input = r#"spec test
data name: "John"
rule is_adult: age >= 18
data age: 25
rule can_drink: age >= 21
data status: "active"
rule is_eligible: is_adult and status is "active""#;

        let result = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap()
        .into_flattened_specs();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].data.len(), 3);
        assert_eq!(result[0].rules.len(), 3);
    }

    #[test]
    fn parse_simple_spec_collects_data() {
        let input = r#"spec person
data name: "John"
data age: 25"#;
        let result = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap()
        .into_flattened_specs();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "person");
        assert_eq!(result[0].data.len(), 2);
    }

    #[test]
    fn parse_dotted_spec_name() {
        let input = r#"spec contracts.employment.jack
data name: "Jack""#;
        let result = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap()
        .into_flattened_specs();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "contracts.employment.jack");
    }

    #[test]
    fn parse_slashed_spec_name() {
        let input = "spec contracts/employment/jack\ndata x: 1";
        let result = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap()
        .into_flattened_specs();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "contracts/employment/jack");
    }

    #[test]
    fn parse_spec_name_no_version_tag() {
        let input = "spec myspec\nrule x: 1";
        let result = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap()
        .into_flattened_specs();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "myspec");
        assert_eq!(result[0].effective_from(), None);
    }

    #[test]
    fn parse_commentary_block_is_attached_to_spec() {
        let input = r#"spec person
"""
This is a markdown comment
uses **bold** text
"""
data name: "John""#;
        let result = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap()
        .into_flattened_specs();
        assert_eq!(result.len(), 1);
        assert!(result[0].commentary.is_some());
        assert!(result[0].commentary.as_ref().unwrap().contains("**bold**"));
    }

    #[test]
    fn parse_spec_with_rule_collects_rule() {
        let input = r#"spec person
rule is_adult: age >= 18"#;
        let result = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap()
        .into_flattened_specs();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].rules.len(), 1);
        assert_eq!(result[0].rules[0].name, "is_adult");
    }

    #[test]
    fn parse_multiple_specs_returns_all_specs() {
        let input = r#"spec person
data name: "John"

spec company
data name: "Acme Corp""#;
        let result = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap()
        .into_flattened_specs();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].name, "person");
        assert_eq!(result[1].name, "company");
    }

    #[test]
    fn parse_allows_duplicate_data_names() {
        let input = r#"spec person
data name: "John"
data name: "Jane""#;
        let result = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        );
        assert!(
            result.is_ok(),
            "Parser should succeed even with duplicate data"
        );
    }

    #[test]
    fn parse_allows_duplicate_rule_names() {
        let input = r#"spec person
rule is_adult: age >= 18
rule is_adult: age >= 21"#;
        let result = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        );
        assert!(
            result.is_ok(),
            "Parser should succeed even with duplicate rules"
        );
    }

    #[test]
    fn parse_rejects_malformed_input() {
        let input = "invalid syntax here";
        let result = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn parse_handles_whitespace_variants_in_expressions() {
        let test_cases = vec![
            ("spec test\nrule test: 2+3", "no spaces in arithmetic"),
            ("spec test\nrule test: age>=18", "no spaces in comparison"),
            (
                "spec test\nrule test: age >= 18 and salary>50000",
                "spaces around and keyword",
            ),
            (
                "spec test\nrule test: age  >=  18  and  salary  >  50000",
                "extra spaces",
            ),
            (
                "spec test\nrule test: \n  age >= 18 \n  and \n  salary > 50000",
                "newlines in expression",
            ),
        ];

        for (input, description) in test_cases {
            let result = parse(
                input,
                crate::parsing::source::SourceType::Volatile,
                &ResourceLimits::default(),
            );
            assert!(
                result.is_ok(),
                "Failed to parse {} ({}): {:?}",
                input,
                description,
                result.err()
            );
        }
    }

    #[test]
    fn parse_error_cases_are_rejected() {
        let error_cases = vec![
            (
                "spec test\ndata name: \"unclosed string",
                "unclosed string literal",
            ),
            ("spec test\nrule test: (2 + 3", "unclosed parenthesis"),
            ("spec test\nrule test: 2 + 3)", "extra closing paren"),
            ("spec test\ndata spec: 123", "reserved keyword as data name"),
            (
                "spec test\nrule rule: true",
                "reserved keyword as rule name",
            ),
        ];

        for (input, description) in error_cases {
            let result = parse(
                input,
                crate::parsing::source::SourceType::Volatile,
                &ResourceLimits::default(),
            );
            assert!(
                result.is_err(),
                "Expected error for {} but got success",
                description
            );
        }
    }

    #[test]
    fn parse_duration_literals_in_rules() {
        let test_cases = vec![
            ("2 year", "year"),
            ("6 month", "month"),
            ("52 week", "week"),
            ("365 day", "day"),
            ("24 hour", "hour"),
            ("60 minute", "minute"),
            ("3600 second", "second"),
            ("1000 millisecond", "millisecond"),
            ("500000 microsecond", "microsecond"),
            ("50 percent", "percent"),
        ];

        for (expr, description) in test_cases {
            let input = format!("spec test\nrule test: {}", expr);
            let result = parse(
                &input,
                crate::parsing::source::SourceType::Volatile,
                &ResourceLimits::default(),
            );
            assert!(
                result.is_ok(),
                "Failed to parse literal {} ({}): {:?}",
                expr,
                description,
                result.err()
            );
        }
    }

    #[test]
    fn parse_comparisons_with_duration_unit_conversions() {
        let test_cases = vec![
            (
                "(duration as hour) > 2",
                "duration conversion in comparison with parens",
            ),
            (
                "(meeting_time as minute) >= 30",
                "duration conversion with gte",
            ),
            (
                "(project_length as day) < 100",
                "duration conversion with lt",
            ),
            (
                "(delay as second) is 60",
                "duration conversion with equality",
            ),
            (
                "(1 hour) > (30 minute)",
                "duration conversions on both sides",
            ),
            ("duration as hour > 2", "duration conversion without parens"),
            (
                "meeting_time as second > 3600",
                "variable duration conversion in comparison",
            ),
            (
                "project_length as day > deadline_days",
                "two variables with duration conversion",
            ),
            (
                "duration as hour >= 1 and duration as hour <= 8",
                "multiple duration comparisons",
            ),
            (
                "(2024-06-01...2024-06-15) as day as number >= 7",
                "chained as conversion before comparison",
            ),
            ("duration as hour as number > 2", "chained as on duration"),
        ];

        for (expr, description) in test_cases {
            let input = format!("spec test\nrule test: {}", expr);
            let result = parse(
                &input,
                crate::parsing::source::SourceType::Volatile,
                &ResourceLimits::default(),
            );
            assert!(
                result.is_ok(),
                "Failed to parse {} ({}): {:?}",
                expr,
                description,
                result.err()
            );
        }
    }

    #[test]
    fn parse_rejects_token_after_unit_conversion() {
        let result = parse(
            "spec test\nuses lemma units\nrule ok: (2024-06-01...2024-06-15) as day foo",
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        );
        let err = result.expect_err("expected parse error");
        let msg = err.to_string();
        assert!(
            msg.contains("Unexpected token") && msg.contains("foo"),
            "expected error at 'foo', got: {}",
            msg
        );
        assert!(
            !msg.contains("Expected 'data'"),
            "should not defer to spec-level error, got: {}",
            msg
        );
    }

    #[test]
    fn parse_unit_conversion_before_next_spec() {
        let result = parse(
            r#"spec pricing
rule hourly_rate: 150 eur
  unless loyalty is "silver" then 140 eur
  unless loyalty is "gold" then 125 usd as eur

spec other
rule x: 1"#,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        );
        assert!(
            result.is_ok(),
            "unless branch ending with 'as' must parse before next spec: {:?}",
            result.err()
        );
    }

    #[test]
    fn parse_unit_conversion_before_sibling_rule() {
        let result = parse(
            r#"spec s
rule a: 100 usd as eur
rule b: 1"#,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        );
        assert!(
            result.is_ok(),
            "rule ending with 'as' must parse before sibling rule: {:?}",
            result.err()
        );
    }

    #[test]
    fn parse_unit_conversion_before_uses() {
        let result = parse(
            r#"spec s
rule rate: 10 usd as eur
uses lemma units
rule hour: 1 hour"#,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        );
        assert!(
            result.is_ok(),
            "rule ending with 'as' must parse before uses: {:?}",
            result.err()
        );
    }

    /// Every token that may follow a completed rule / unless expression ending in `as`.
    #[test]
    fn parse_unit_conversion_before_expression_boundaries() {
        let cases: &[(&str, &str)] = &[
            (
                "sibling data",
                r#"spec s
rule rate: 10 usd as eur
data price: 100 eur"#,
            ),
            (
                "sibling uses",
                r#"spec s
rule rate: 10 usd as eur
uses other"#,
            ),
            (
                "sibling meta",
                r#"spec s
rule rate: 10 usd as eur
meta version: 1"#,
            ),
            (
                "another unless",
                r#"spec s
rule rate: 10 usd
  unless active then 5 usd as eur
  unless premium then 3 usd as eur"#,
            ),
            (
                "eof",
                r#"spec s
rule rate: 10 usd as eur"#,
            ),
            (
                "next repo",
                r#"spec s
rule rate: 10 usd as eur

repo other
spec t
rule x: 1"#,
            ),
            (
                "unless then before next unless",
                r#"spec s
rule rate: 10 usd
  unless a then 1 usd as eur
  unless b then 2"#,
            ),
            (
                "chained as before sibling rule",
                r#"spec s
rule rate: (2024-01-01...2024-01-02) as day as number
rule other: 1"#,
            ),
        ];

        for (label, source) in cases {
            let result = parse(
                source,
                crate::parsing::source::SourceType::Volatile,
                &ResourceLimits::default(),
            );
            assert!(
                result.is_ok(),
                "unit conversion before {label} must parse: {:?}",
                result.err()
            );
        }
    }

    #[test]
    fn parse_rejects_plain_number_plus_converted_operand() {
        let result = parse(
            r#"spec test
data c: measure
  -> unit eur: 1
  -> unit usd: 0.84
rule z: 5 + c as usd"#,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        );
        let err = result.expect_err("expected parse error for 5 + c as usd");
        let msg = err.to_string();
        assert!(
            msg.contains("plain number") || msg.contains("each operand"),
            "expected conversion-before-+ error, got: {msg}"
        );
    }

    #[test]
    fn parse_accepts_conversion_on_each_additive_operand() {
        let cases: &[(&str, &str)] = &[
            (
                "money",
                r#"spec test
data c: measure
  -> unit eur: 1
  -> unit usd: 0.84
rule z: 5 as usd + c as usd"#,
            ),
            (
                "duration + literal",
                r#"spec test
uses lemma units
rule z: duration as hour + 1"#,
            ),
            (
                "duration + comparison",
                r#"spec test
uses lemma units
data duration: units.duration
  -> suggest 1 hour
rule z: duration as hour + 1 > 0"#,
            ),
            (
                "date range + ref",
                r#"spec test
uses lemma units
data age: date range
data c: measure
  -> unit eur: 1
rule z: age as day + c"#,
            ),
        ];
        for (label, source) in cases {
            let result = parse(
                source,
                crate::parsing::source::SourceType::Volatile,
                &ResourceLimits::default(),
            );
            assert!(
                result.is_ok(),
                "expected {label} to parse, got: {:?}",
                result.err()
            );
        }
    }

    fn rule_expression(source: &str, rule_name: &str) -> Expression {
        let parsed = parse(
            source,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .expect("expected parse");
        let spec = parsed
            .flatten_specs()
            .into_iter()
            .next()
            .expect("expected one spec");
        spec.rules
            .iter()
            .find(|rule| rule.name == rule_name)
            .unwrap_or_else(|| panic!("rule '{rule_name}' not found"))
            .expression
            .clone()
    }

    fn assert_multiply_range_side(expression: &Expression, range_on_left: bool, label: &str) {
        let ExpressionKind::Arithmetic(left, ArithmeticComputation::Multiply, right) =
            &expression.kind
        else {
            panic!("{label}: expected Multiply, got {:?}", expression.kind);
        };
        let (range, other) = if range_on_left {
            (left.as_ref(), right.as_ref())
        } else {
            (right.as_ref(), left.as_ref())
        };
        assert!(
            matches!(range.kind, ExpressionKind::RangeLiteral(..)),
            "{label}: expected RangeLiteral on {} of *, got {:?}",
            if range_on_left { "left" } else { "right" },
            range.kind
        );
        assert!(
            !matches!(other.kind, ExpressionKind::RangeLiteral(..)),
            "{label}: expected non-range on other side of *"
        );
    }

    #[test]
    fn parse_range_binds_tighter_than_multiply() {
        let base = r#"spec test
uses lemma units
data rate: measure -> unit eur: 1
data period_start: 2026-01-01
data period_end: 2026-01-02
"#;
        assert_multiply_range_side(
            &rule_expression(
                &format!("{base}rule rhs: rate * period_start...period_end"),
                "rhs",
            ),
            false,
            "rate * period_start...period_end",
        );
        assert_multiply_range_side(
            &rule_expression(
                &format!("{base}rule lhs: period_start...period_end * rate"),
                "lhs",
            ),
            true,
            "period_start...period_end * rate",
        );
    }

    #[test]
    fn parse_range_additive_right_endpoint_binds_inside_literal() {
        let expression = rule_expression(
            r#"spec test
uses lemma units
data start: date
data length: units.duration
rule valid: now in start...start + length"#,
            "valid",
        );
        let ExpressionKind::RangeContainment(value, range) = &expression.kind else {
            panic!("expected RangeContainment, got {:?}", expression.kind);
        };
        assert!(
            matches!(value.kind, ExpressionKind::Now),
            "expected now as containment value, got {:?}",
            value.kind
        );
        let ExpressionKind::RangeLiteral(left, right) = &range.kind else {
            panic!(
                "expected RangeLiteral as containment range, got {:?}",
                range.kind
            );
        };
        assert!(
            matches!(left.kind, ExpressionKind::Reference(..)),
            "expected start reference as left endpoint, got {:?}",
            left.kind
        );
        let ExpressionKind::Arithmetic(add_left, ArithmeticComputation::Add, add_right) =
            &right.kind
        else {
            panic!(
                "expected start + length as right endpoint, got {:?}",
                right.kind
            );
        };
        assert!(
            matches!(add_left.kind, ExpressionKind::Reference(..)),
            "expected start reference in right endpoint add, got {:?}",
            add_left.kind
        );
        assert!(
            matches!(add_right.kind, ExpressionKind::Reference(..)),
            "expected length reference in right endpoint add, got {:?}",
            add_right.kind
        );
    }

    #[test]
    fn parse_range_multiply_with_conversion_without_inner_parens() {
        let expression = rule_expression(
            r#"spec test
uses lemma units
data money: measure -> unit eur: 1
data rate: measure -> unit eur_per_hour: eur/hour
data hourly_rate: 50 eur_per_hour
data period_start: 2026-01-01
data period_end: 2026-01-02
rule pay: (hourly_rate * period_start...period_end) as eur"#,
            "pay",
        );
        let ExpressionKind::UnitConversion(inner, _) = &expression.kind else {
            panic!("expected UnitConversion, got {:?}", expression.kind);
        };
        assert_multiply_range_side(inner, false, "pay");
    }

    #[test]
    fn parse_error_includes_attribute_and_parse_error_spec_name() {
        let result = parse(
            r#"
spec test
data name: "Unclosed string
data age: 25
"#,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        );

        match result {
            Err(Error::Parsing(details)) => {
                let src = details
                    .source
                    .as_ref()
                    .expect("BUG: parsing errors always have source");
                assert_eq!(
                    src.source_type,
                    crate::parsing::source::SourceType::Volatile
                );
            }
            Err(e) => panic!("Expected Parse error, got: {e:?}"),
            Ok(_) => panic!("Expected parse error for unclosed string"),
        }
    }

    #[test]
    fn parse_single_spec_file() {
        let input = r#"spec somespec
data name: "Alice""#;
        let parsed = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap();
        let specs = parsed.flatten_specs();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].name, "somespec");
    }

    #[test]
    fn parse_uses_registry_spec_explicit_alias() {
        let input = r#"spec example
uses external: @user/workspace somespec"#;
        let specs = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap()
        .into_flattened_specs();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].data.len(), 1);
        match &specs[0].data[0].value {
            crate::parsing::ast::DataValue::Import {
                spec_ref,
                bindings: _,
            } => {
                assert_eq!(spec_ref.name, "somespec");
                let repository_hdr = spec_ref
                    .repository
                    .as_ref()
                    .expect("expected repository qualifier");
                assert_eq!(repository_hdr.name, "@user/workspace");
            }
            other => panic!("Expected Import, got: {:?}", other),
        }
    }

    #[test]
    fn parse_multiple_specs_cross_reference_in_file() {
        let input = r#"spec spec_a
data x: 10

spec spec_b
data y: 20
uses a: spec_a"#;
        let parsed = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap();
        let specs = parsed.flatten_specs();
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].name, "spec_a");
        assert_eq!(specs[1].name, "spec_b");
    }

    #[test]
    fn parse_uses_registry_spec_default_alias() {
        let input = "spec example\nuses @owner/repo somespec";
        let specs = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap()
        .into_flattened_specs();
        match &specs[0].data[0].value {
            crate::parsing::ast::DataValue::Import {
                spec_ref,
                bindings: _,
            } => {
                assert_eq!(spec_ref.name, "somespec");
                let repository_hdr = spec_ref
                    .repository
                    .as_ref()
                    .expect("expected repository qualifier");
                assert_eq!(repository_hdr.name, "@owner/repo");
            }
            other => panic!("Expected Import, got: {:?}", other),
        }
    }

    #[test]
    fn parse_uses_local_spec_default_alias() {
        let input = "spec example\nuses myspec";
        let specs = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap()
        .into_flattened_specs();
        match &specs[0].data[0].value {
            crate::parsing::ast::DataValue::Import {
                spec_ref,
                bindings: _,
            } => {
                assert_eq!(spec_ref.name, "myspec");
                assert!(
                    spec_ref.repository.is_none(),
                    "same-repository reference must omit repository qualifier"
                );
            }
            other => panic!("Expected Import, got: {:?}", other),
        }
    }

    #[test]
    fn parse_spec_name_with_trailing_dot_is_error() {
        let input = "spec myspec.\ndata x: 1";
        let result = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        );
        assert!(
            result.is_err(),
            "Trailing dot after spec name should be a parse error"
        );
    }

    #[test]
    fn parse_multiple_specs_in_same_file() {
        let input = "spec myspec_a\nrule x: 1\n\nspec myspec_b\nrule x: 2";
        let result = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap()
        .into_flattened_specs();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].name, "myspec_a");
        assert_eq!(result[1].name, "myspec_b");
    }

    #[test]
    fn parse_uses_accepts_name_only() {
        let input = "spec consumer\nuses other";
        let result = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        );
        assert!(result.is_ok(), "uses name should parse");
        let specs = result.unwrap().into_flattened_specs();
        let spec_ref = match &specs[0].data[0].value {
            crate::parsing::ast::DataValue::Import { spec_ref, .. } => spec_ref,
            _ => panic!("expected Import"),
        };
        assert_eq!(spec_ref.name, "other");
    }

    #[test]
    fn parse_uses_bare_year_effective() {
        let input = "spec consumer\nuses other 2026";
        let result = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap();
        let specs = result.into_flattened_specs();
        let spec_ref = match &specs[0].data[0].value {
            crate::parsing::ast::DataValue::Import { spec_ref, .. } => spec_ref,
            _ => panic!("expected Import"),
        };
        assert_eq!(spec_ref.name, "other");
        let eff = spec_ref.effective.as_ref().expect("effective");
        assert_eq!(eff.year, 2026);
        assert_eq!(eff.month, 1);
        assert_eq!(eff.day, 1);
    }

    #[test]
    fn parse_uses_registry_spec_ref_records_repository_and_target_spans() {
        let input = "spec consumer\nuses @iso/countries alpha2 2026";
        let result = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap();
        let spec = &result.flatten_specs()[0];
        let sr = match &spec.data[0].value {
            crate::parsing::ast::DataValue::Import { spec_ref, .. } => spec_ref,
            _ => panic!("expected Import"),
        };
        let rs = sr
            .repository_span
            .as_ref()
            .expect("repository_span should be set for @-qualified uses");
        let ts = sr
            .target_span
            .as_ref()
            .expect("target_span should cover spec name and effective");
        assert_eq!(&input[rs.start..rs.end], "@iso/countries");
        assert_eq!(&input[ts.start..ts.end], "alpha2 2026");
    }

    #[test]
    fn parse_uses_alias_no_comma_continuation() {
        let input = "spec consumer\nuses alias: pricing retail\ndata x: 1";
        let result = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap();
        let data = &result.flatten_specs()[0].data;
        assert_eq!(data.len(), 2);
        assert_eq!(data[0].reference.name, "alias");
        let sr = match &data[0].value {
            crate::parsing::ast::DataValue::Import { spec_ref, .. } => spec_ref,
            _ => panic!("expected Import"),
        };
        assert_eq!(sr.name, "retail");
        let repository_hdr = sr
            .repository
            .as_ref()
            .expect("expected repository qualifier");
        assert_eq!(repository_hdr.name, "pricing");
    }

    #[test]
    fn parse_data_qualified_type_with_effective_and_repository_on_uses() {
        let input = "spec consumer\nuses @iso/countries alpha2 2026-06-01\ndata country: alpha2.code -> option \"NL\"";
        let result = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap()
        .into_flattened_specs();
        let spec_ref = match &result[0].data[0].value {
            crate::parsing::ast::DataValue::Import { spec_ref: sr, .. } => sr,
            other => panic!("expected Import on uses row, got: {:?}", other),
        };
        assert_eq!(spec_ref.name, "alpha2");

        let eff = spec_ref
            .effective
            .as_ref()
            .expect("expected effective datetime");
        assert_eq!(eff.year, 2026);
        assert_eq!(eff.month, 6);

        let qualifier = spec_ref
            .repository
            .as_ref()
            .expect("expected repository qualifier");
        assert_eq!(qualifier.name, "@iso/countries");

        match &result[0].data[1].value {
            crate::parsing::ast::DataValue::Definition {
                base,
                constraints,
                value,
            } => {
                assert!(value.is_none());
                assert_eq!(
                    base.as_ref().expect("expected base"),
                    &crate::parsing::ast::ParentType::Qualified {
                        spec_alias: "alpha2".into(),
                        inner: Box::new(crate::parsing::ast::ParentType::Custom {
                            name: "code".into(),
                        }),
                    }
                );

                let cs = constraints
                    .as_ref()
                    .expect("expected trailing constraint chain");
                assert_eq!(cs.len(), 1);
            }
            other => panic!("expected Definition, got: {:?}", other),
        }
    }

    #[test]
    fn parse_qualified_type_with_slashed_opaque_spec_name() {
        let input = r#"
spec logistics/terms
data incoterm_type: text

spec logistics/shipping
uses logistics/terms
data incoterm: logistics/terms.incoterm_type
"#;
        let result = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap()
        .into_flattened_specs();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].name, "logistics/terms");
        assert_eq!(result[1].name, "logistics/shipping");
        assert_eq!(result[1].data[0].reference.name, "logistics/terms");
        let import = match &result[1].data[0].value {
            crate::parsing::ast::DataValue::Import { spec_ref, .. } => spec_ref,
            other => panic!("expected Import, got: {other:?}"),
        };
        assert_eq!(import.name, "logistics/terms");
        assert!(import.repository.is_none());
        match &result[1].data[1].value {
            crate::parsing::ast::DataValue::Definition {
                base: Some(base), ..
            } => {
                assert_eq!(
                    base,
                    &crate::parsing::ast::ParentType::Qualified {
                        spec_alias: "logistics/terms".into(),
                        inner: Box::new(crate::parsing::ast::ParentType::Custom {
                            name: "incoterm_type".into(),
                        }),
                    }
                );
            }
            other => panic!("expected Definition with qualified base, got: {other:?}"),
        }
    }

    #[test]
    fn parse_uses_slashed_name_alias_is_full_opaque_name() {
        let input = "spec consumer\nuses finance/units\ndata x: 1";
        let result = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap()
        .into_flattened_specs();
        assert_eq!(result[0].data[0].reference.name, "finance/units");
        let sr = match &result[0].data[0].value {
            crate::parsing::ast::DataValue::Import { spec_ref, .. } => spec_ref,
            other => panic!("expected Import, got: {other:?}"),
        };
        assert_eq!(sr.name, "finance/units");
        assert!(sr.repository.is_none());
    }

    #[test]
    fn parse_uses_terms_is_spec_named_terms_not_sibling_of_logistics() {
        let input = "spec logistics/shipping\nuses terms\ndata x: 1";
        let result = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap()
        .into_flattened_specs();
        assert_eq!(result[0].data[0].reference.name, "terms");
        let sr = match &result[0].data[0].value {
            crate::parsing::ast::DataValue::Import { spec_ref, .. } => spec_ref,
            other => panic!("expected Import, got: {other:?}"),
        };
        assert_eq!(sr.name, "terms");
        assert!(sr.repository.is_none());
    }

    #[test]
    fn parse_error_is_returned_for_garbage_input() {
        let result = parse(
            r#"
spec test
this is not valid lemma syntax @#$%
"#,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        );

        assert!(result.is_err(), "Should fail on malformed input");
        match result {
            Err(Error::Parsing { .. }) => {
                // Expected
            }
            Err(e) => panic!("Expected Parse error, got: {e:?}"),
            Ok(_) => panic!("Expected parse error"),
        }
    }

    // ─── Parser-level pins for DataValue variants ────────────────────

    #[test]
    fn parse_local_with_literal_rejected() {
        let err = parse(
            r#"spec s
with x: 42"#,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("Standalone") || msg.contains("uses"),
            "expected standalone with rejection, got: {msg}"
        );
    }

    #[test]
    fn parse_local_with_import_reference_rejected() {
        let err = parse(
            r#"spec s
uses i: inner
with copy: i.v"#,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("Standalone") || msg.contains("uses"),
            "expected standalone with rejection, got: {msg}"
        );
    }

    #[test]
    fn parse_local_with_dotted_rhs_rejected() {
        let err = parse(
            r#"spec s
with x: a.something"#,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("Standalone") || msg.contains("uses"),
            "expected standalone with rejection, got: {msg}"
        );
    }

    #[test]
    fn parse_local_with_multi_segment_rhs_rejected() {
        let err = parse(
            r#"spec s
with x: alpha.beta.gamma.delta"#,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("Standalone") || msg.contains("uses"),
            "expected standalone with rejection, got: {msg}"
        );
    }

    /// `data x: notdotted` (local LHS, non-dotted RHS) MUST stay a
    /// Definition with an explicit custom parent — not silently reinterpreted as a Reference.
    #[test]
    fn parse_local_non_dotted_rhs_stays_definition_with_custom_base() {
        let input = r#"spec s
data x: myothertype"#;
        let result = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap()
        .into_flattened_specs();
        let value = &result[0].data[0].value;
        assert!(
            matches!(
                value,
                crate::parsing::ast::DataValue::Definition {
                    base: Some(crate::parsing::ast::ParentType::Custom { .. }),
                    ..
                }
            ),
            "non-dotted local RHS must stay Definition with custom base, got: {:?}",
            value
        );
    }

    /// `uses` block `-> with path: reference` stores reference in binding rhs.
    #[test]
    fn parse_uses_binding_non_dotted_rhs_is_reference() {
        let input = r#"spec s
uses child: other
  -> with slot: somename"#;
        let result = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap()
        .into_flattened_specs();
        let bindings = match &result[0].data[0].value {
            crate::parsing::ast::DataValue::Import { bindings, .. } => bindings,
            other => panic!("expected Import, got: {:?}", other),
        };
        assert_eq!(bindings.len(), 1);
        assert!(
            matches!(
                &bindings[0].rhs,
                crate::parsing::ast::WithRhs::Reference { .. }
            ),
            "non-dotted RHS must yield Reference; got: {:?}",
            bindings[0].rhs
        );
    }

    /// `data x: spec …` is invalid; spec imports use `uses`.
    #[test]
    fn parse_data_colon_spec_rhs_is_rejected() {
        let result = parse(
            r#"
spec s
data x: spec other
"#,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        );
        match result {
            Ok(_) => panic!("`data x: spec other` must fail to parse"),
            Err(err) => {
                let msg = err.to_string();
                assert!(
                    msg.contains("uses") && msg.contains("spec"),
                    "error must direct to `uses` for spec import, got: {msg}"
                );
            }
        }
    }

    /// `uses` block dotted path and dotted reference rhs preserved.
    #[test]
    fn parse_uses_binding_with_dotted_rhs_preserves_both_sides() {
        let input = r#"spec s
uses outer: other
  -> with inner: target.field"#;
        let result = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap()
        .into_flattened_specs();
        let bindings = match &result[0].data[0].value {
            crate::parsing::ast::DataValue::Import { bindings, .. } => bindings,
            other => panic!("expected Import, got: {:?}", other),
        };
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].path.segments, vec![] as Vec<String>);
        assert_eq!(bindings[0].path.name, "inner");
        match &bindings[0].rhs {
            crate::parsing::ast::WithRhs::Reference { target } => {
                assert_eq!(target.segments, vec!["target"]);
                assert_eq!(target.name, "field");
            }
            other => panic!("expected Reference rhs, got: {:?}", other),
        }
    }

    #[test]
    fn parse_deprecated_standalone_with_merges_into_import_bindings() {
        let input = r#"spec inner
data x: number

spec outer
uses i: inner
with i.x: 42
rule r: i.x"#;
        let result = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap()
        .into_flattened_specs();
        let outer = result.iter().find(|s| s.name == "outer").unwrap();
        let bindings = match &outer.data[0].value {
            crate::parsing::ast::DataValue::Import { bindings, .. } => bindings,
            other => panic!("expected Import, got: {:?}", other),
        };
        assert_eq!(bindings.len(), 1);
        assert!(bindings[0].deprecated_standalone_with);
        assert_eq!(bindings[0].path.name, "x");
        match &bindings[0].rhs {
            crate::parsing::ast::WithRhs::Literal(crate::parsing::ast::Value::Number(n)) => {
                assert_eq!(n, &rust_decimal::Decimal::from(42));
            }
            other => panic!("expected literal 42, got: {:?}", other),
        }
    }

    #[test]
    fn parse_deprecated_standalone_with_before_uses_merges() {
        let input = r#"spec inner
data slot: number

spec outer
with i.slot: 99
uses i: inner
rule r: i.slot"#;
        let result = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap()
        .into_flattened_specs();
        let outer = result.iter().find(|s| s.name == "outer").unwrap();
        let bindings = match &outer.data[0].value {
            crate::parsing::ast::DataValue::Import { bindings, .. } => bindings,
            other => panic!("expected Import, got: {:?}", other),
        };
        assert_eq!(bindings.len(), 1);
        assert!(bindings[0].deprecated_standalone_with);
    }

    #[test]
    fn parse_standalone_with_without_matching_uses_errors() {
        let result = parse(
            r#"spec s
with outer.inner: 1"#,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        );
        match result {
            Ok(_) => panic!("standalone with without uses must not parse"),
            Err(err) => {
                let msg = err.to_string();
                assert!(
                    msg.contains("uses") && msg.contains("outer"),
                    "error should mention required uses; got: {msg}"
                );
            }
        }
    }

    #[test]
    fn parse_data_on_binding_path_is_rejected_with_with_hint() {
        let result = parse(
            r#"spec s
data outer.inner: 1"#,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        );
        match result {
            Ok(_) => panic!("data with binding path must not parse"),
            Err(err) => {
                let msg = err.to_string();
                assert!(
                    msg.contains("with"),
                    "error should steer authors toward with; got: {msg}"
                );
            }
        }
    }

    #[test]
    fn parse_bare_file_yields_single_anonymous_repository_group() {
        let input = "spec a\ndata x: 1\nspec b\ndata y: 2";
        let parsed = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap();
        assert_eq!(parsed.repositories.len(), 1);
        let (repo, specs) = parsed.repositories.iter().next().unwrap();
        assert!(repo.name.is_none());
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].name, "a");
        assert_eq!(specs[1].name, "b");
    }

    #[test]
    fn parse_repo_sections_preserve_order_and_names() {
        let input = r#"repo r1

spec a
data x: 1

repo r2

spec b
data y: 2"#;
        let parsed = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap();
        assert_eq!(parsed.repositories.len(), 2);
        let keys: Vec<_> = parsed.repositories.keys().collect();
        assert_eq!(keys[0].name.as_deref(), Some("r1"));
        assert_eq!(keys[1].name.as_deref(), Some("r2"));
    }

    #[test]
    fn parse_duplicate_repo_name_merges_spec_lists() {
        let input = r#"repo dup

spec a
data x: 1

repo dup

spec b
data y: 2"#;
        let parsed = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap();
        assert_eq!(parsed.repositories.len(), 1);
        assert_eq!(parsed.flatten_specs().len(), 2);
    }

    #[test]
    fn parse_repo_with_no_specs_then_eof_yields_empty_spec_vec_for_that_repo() {
        let input = "repo empty";
        let parsed = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap();
        assert_eq!(parsed.repositories.len(), 1);
        let (_repo, specs) = parsed.repositories.iter().next().unwrap();
        assert_eq!(specs.len(), 0);
    }

    #[test]
    fn parse_repo_followed_by_repo_without_specs_first_repo_empty_second_has_spec() {
        let input = "repo a\n\nrepo b\n\nspec s\ndata x: 1";
        let parsed = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap();
        assert_eq!(parsed.repositories.len(), 2);
        let names: Vec<_> = parsed
            .repositories
            .keys()
            .map(|r| r.name.as_deref())
            .collect();
        assert_eq!(names, vec![Some("a"), Some("b")]);
        assert!(parsed.repositories.values().next().unwrap().is_empty());
        assert_eq!(parsed.repositories.values().nth(1).unwrap().len(), 1);
    }

    #[test]
    fn parse_spec_named_repo_keyword_should_be_rejected() {
        assert!(
            parse(
                "spec repo\ndata x: 1",
                crate::parsing::source::SourceType::Volatile,
                &ResourceLimits::default(),
            )
            .is_err(),
            "spec must not be allowed to use reserved keyword `repo` as its name"
        );
    }

    #[test]
    fn parse_repo_declaration_cannot_use_spec_keyword_as_repository_name() {
        assert!(
            parse(
                "repo spec\n\nspec z\ndata q: 1\nrule r: q",
                crate::parsing::source::SourceType::Volatile,
                &ResourceLimits::default(),
            )
            .is_err(),
            "repository name cannot be the token `spec`"
        );
    }

    #[test]
    fn parse_repo_declaration_cannot_use_data_keyword_as_repository_name() {
        assert!(
            parse(
                "repo data\n\nspec z\ndata q: 1\nrule r: q",
                crate::parsing::source::SourceType::Volatile,
                &ResourceLimits::default(),
            )
            .is_err(),
            "repository name cannot be the token `data`"
        );
    }

    #[test]
    fn parse_repo_declaration_cannot_use_rule_keyword_as_repository_name() {
        assert!(
            parse(
                "repo rule\n\nspec z\ndata q: 1\nrule r: q",
                crate::parsing::source::SourceType::Volatile,
                &ResourceLimits::default(),
            )
            .is_err(),
            "repository name cannot be the token `rule`"
        );
    }

    #[test]
    fn parse_data_named_repo_keyword_is_rejected() {
        let err = parse(
            "spec s\ndata repo: 1",
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("repo"),
            "data named repo should not parse: {}",
            err
        );
    }

    #[test]
    fn parse_rule_named_repo_keyword_is_rejected() {
        let err = parse(
            "spec s\ndata x: 1\nrule repo: x",
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("repo") || msg.contains("reserved"),
            "rule named repo should not parse: {msg}"
        );
    }

    #[test]
    fn parse_repo_declaration_accepts_non_keyword_repository_identifier() {
        let parsed = parse(
            "repo warehouse\n\nspec z\ndata q: 1\nrule r: q",
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap();
        assert_eq!(parsed.repositories.len(), 1);
        assert_eq!(
            parsed.repositories.keys().next().unwrap().name.as_deref(),
            Some("warehouse")
        );
    }

    #[test]
    fn parse_repo_name_case_insensitive_same_repository_merged() {
        let input = "repo Foo\n\nspec a\ndata x: 1\n\nrepo foo\n\nspec b\ndata y: 2";
        let parsed = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap();
        assert_eq!(
            parsed.repositories.len(),
            1,
            "Foo and foo are the same repository after canonicalization"
        );
        let specs: Vec<_> = parsed.repositories.values().next().unwrap().clone();
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].name, "a");
        assert_eq!(specs[1].name, "b");
    }

    #[test]
    fn parse_repo_empty_name_errors() {
        let err = parse(
            "repo \nspec a\ndata x: 1",
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap_err();
        assert!(
            !err.to_string().is_empty(),
            "empty repo name should not parse quietly: {err}"
        );
    }

    #[test]
    fn parse_repo_numeric_name_behavior() {
        let input = "repo 123\n\nspec a\ndata x: 1";
        let result = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        );
        match result {
            Ok(parsed) => {
                assert_eq!(
                    parsed.repositories.keys().next().unwrap().name.as_deref(),
                    Some("123"),
                    "if numeric repo names parse, identity must be stable"
                );
            }
            Err(e) => {
                assert!(
                    !e.to_string().is_empty(),
                    "rejecting numeric repo name is ok if explicit: {e}"
                );
            }
        }
    }

    #[test]
    fn parse_duplicate_repo_three_sections_preserves_spec_order_abc() {
        let input = r#"repo dup

spec a
data x: 1

repo dup

spec b
data y: 2

repo dup

spec c
data z: 3"#;
        let parsed = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap();
        assert_eq!(parsed.repositories.len(), 1);
        let specs = parsed.repositories.values().next().unwrap();
        assert_eq!(
            specs.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            vec!["a", "b", "c"]
        );
    }

    #[test]
    fn parse_repo_single_section_roundtrips_through_formatter() {
        let input = "repo r\n\nspec a\ndata x: 1";
        let parsed = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap();
        let formatted = format_parse_result(&parsed);
        let again = parse(
            &formatted,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap();
        assert_eq!(again.repositories.len(), parsed.repositories.len());
        assert_eq!(again.flatten_specs().len(), parsed.flatten_specs().len());
        assert_eq!(
            again.flatten_specs()[0].name,
            parsed.flatten_specs()[0].name
        );
    }

    #[test]
    fn parse_repo_two_sections_roundtrips_through_formatter() {
        let input = "repo r1\n\nspec a\ndata x: 1\n\nrepo r2\n\nspec b\ndata y: 2";
        let parsed = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap();
        let formatted = format_parse_result(&parsed);
        let again = parse(
            &formatted,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap();
        assert_eq!(again.repositories.len(), 2);
        assert_eq!(again.flatten_specs().len(), 2);
    }

    #[test]
    fn parse_repo_duplicate_merge_formatter_emits_single_repo_block_or_equivalent_parse() {
        let input = r#"repo dup

spec a
data x: 1

repo dup

spec b
data y: 2"#;
        let parsed = parse(
            input,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap();
        let formatted = format_parse_result(&parsed);
        let again = parse(
            &formatted,
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap();
        assert_eq!(
            again.repositories.len(),
            1,
            "formatted duplicate-repo file must still merge to one logical repo"
        );
        assert_eq!(again.flatten_specs().len(), 2);
    }

    #[test]
    fn parse_rejects_data_named_measure() {
        let result = parse(
            "spec s\ndata measure: 1",
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        );
        assert!(
            result.is_err(),
            "data named measure (type keyword) must be rejected"
        );
    }

    #[test]
    fn parse_rejects_data_named_number() {
        let result = parse(
            "spec s\ndata number: 42",
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        );
        assert!(
            result.is_err(),
            "data named number (type keyword) must be rejected"
        );
    }

    #[test]
    fn parse_rejects_data_named_text() {
        let result = parse(
            "spec s\ndata text: \"hello\"",
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        );
        assert!(
            result.is_err(),
            "data named text (type keyword) must be rejected"
        );
    }

    #[test]
    fn parse_rejects_data_named_date() {
        let result = parse(
            "spec s\ndata date: 2024-01-01",
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        );
        assert!(
            result.is_err(),
            "data named date (type keyword) must be rejected"
        );
    }

    #[test]
    fn parse_rejects_data_named_boolean() {
        let result = parse(
            "spec s\ndata boolean: true",
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        );
        assert!(
            result.is_err(),
            "data named boolean (type keyword) must be rejected"
        );
    }

    #[test]
    fn parse_rejects_data_named_ratio() {
        let result = parse(
            "spec s\ndata ratio: 5%",
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        );
        assert!(
            result.is_err(),
            "data named ratio (type keyword) must be rejected"
        );
    }

    #[test]
    fn parse_rejects_rule_named_measure() {
        let result = parse(
            "spec s\ndata x: 1\nrule measure: x",
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        );
        assert!(
            result.is_err(),
            "rule named measure (type keyword) must be rejected"
        );
    }

    #[test]
    fn parse_rejects_rule_named_number() {
        let result = parse(
            "spec s\ndata x: 1\nrule number: x",
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        );
        assert!(
            result.is_err(),
            "rule named number (type keyword) must be rejected"
        );
    }

    #[test]
    fn parse_rejects_rule_named_text() {
        let result = parse(
            "spec s\ndata x: 1\nrule text: x",
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        );
        assert!(
            result.is_err(),
            "rule named text (type keyword) must be rejected"
        );
    }

    #[test]
    fn parse_rejects_rule_named_date() {
        let result = parse(
            "spec s\ndata x: 1\nrule date: x",
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        );
        assert!(
            result.is_err(),
            "rule named date (type keyword) must be rejected"
        );
    }

    #[test]
    fn parse_rejects_rule_named_boolean() {
        let result = parse(
            "spec s\ndata x: 1\nrule boolean: x",
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        );
        assert!(
            result.is_err(),
            "rule named boolean (type keyword) must be rejected"
        );
    }

    #[test]
    fn parse_rejects_rule_named_ratio() {
        let result = parse(
            "spec s\ndata x: 1\nrule ratio: x",
            crate::parsing::source::SourceType::Volatile,
            &ResourceLimits::default(),
        );
        assert!(
            result.is_err(),
            "rule named ratio (type keyword) must be rejected"
        );
    }
}

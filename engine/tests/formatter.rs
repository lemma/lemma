use lemma::format_parse_result;
use lemma::{format_source, parse, ResourceLimits};

fn format_and_extract_rule_expr(source: &str) -> String {
    let formatted = format_source(source, lemma::SourceType::Volatile).unwrap();
    let lines: Vec<&str> = formatted.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("rule x: ") {
            return rest.to_string();
        }
        if trimmed == "rule x:" {
            let next = lines.get(i + 1).map(|s| s.trim()).unwrap_or("");
            if !next.is_empty() {
                return next.to_string();
            }
        }
    }
    panic!(
        "Could not find 'rule x: ...' in formatted output: {}",
        formatted
    );
}

// =============================================================================
// Expression precedence tests
// =============================================================================

#[test]
fn precedence_add_inside_multiply_preserves_parens() {
    let src = "spec test data a: 1 data b: 2 data c: 3 rule x: (a + b) * c";
    assert_eq!(format_and_extract_rule_expr(src), "(a + b) * c");
}

#[test]
fn precedence_multiply_inside_add_omits_parens() {
    let src = "spec test data a: 1 data b: 2 data c: 3 rule x: a + b * c";
    assert_eq!(format_and_extract_rule_expr(src), "a + b * c");
}

#[test]
fn precedence_add_right_of_multiply_preserves_parens() {
    let src = "spec test data a: 1 data b: 2 data c: 3 rule x: a * (b + c)";
    assert_eq!(format_and_extract_rule_expr(src), "a * (b + c)");
}

#[test]
fn precedence_same_level_add_no_extra_parens() {
    let src = "spec test data a: 1 data b: 2 data c: 3 rule x: (a + b) + c";
    assert_eq!(format_and_extract_rule_expr(src), "a + b + c");
}

#[test]
fn precedence_same_level_multiply_no_extra_parens() {
    let src = "spec test data a: 1 data b: 2 data c: 3 rule x: (a * b) * c";
    assert_eq!(format_and_extract_rule_expr(src), "a * b * c");
}

#[test]
fn precedence_not_binds_tighter_than_and() {
    let src = "spec test data a: true data b: true rule x: not a and b";
    assert_eq!(format_and_extract_rule_expr(src), "not a and b");
}

#[test]
fn precedence_not_over_and_preserves_parens() {
    let src = "spec test data a: true data b: true rule x: not (a and b)";
    assert_eq!(format_and_extract_rule_expr(src), "not (a and b)");
}

#[test]
fn precedence_subtract_inside_multiply_preserves_parens() {
    let src = "spec test data a: 1 data b: 2 data c: 3 rule x: (a - b) * c";
    assert_eq!(format_and_extract_rule_expr(src), "(a - b) * c");
}

#[test]
fn precedence_multiply_inside_subtract_omits_parens() {
    let src = "spec test data a: 1 data b: 2 data c: 3 rule x: a - b * c";
    assert_eq!(format_and_extract_rule_expr(src), "a - b * c");
}

#[test]
fn precedence_nested_arithmetic_mixed() {
    let src = "spec test data a: 1 data b: 2 data c: 3 data d: 4 rule x: (a + b) * (c - d)";
    assert_eq!(format_and_extract_rule_expr(src), "(a + b) * (c - d)");
}

#[test]
fn precedence_comparison_lower_than_arithmetic() {
    let src = "spec test data a: 1 data b: 2 data c: 3 rule x: a + b > c";
    assert_eq!(format_and_extract_rule_expr(src), "a + b > c");
}

#[test]
fn precedence_deeply_nested() {
    let src = "spec test data a: 1 data b: 2 data c: 3 data d: 4 rule x: a + b * c + d";
    assert_eq!(format_and_extract_rule_expr(src), "a + b * c + d");
}

// =============================================================================
// Numeric unary minus
// =============================================================================

#[test]
fn unary_minus_number_literal_stays_signed() {
    let src = "spec test rule x: -2";
    assert_eq!(format_and_extract_rule_expr(src), "-2");
}

#[test]
fn unary_minus_unless_then_stays_signed() {
    let src = r#"spec test
data id: text
rule x: 0
unless id is "America/Noronha" then -2
"#;
    let formatted = format_source(src, lemma::SourceType::Volatile).unwrap();
    assert!(
        formatted.contains("then -2"),
        "expected signed literal in unless result, got:\n{formatted}"
    );
    assert!(
        !formatted.contains("0 - 2"),
        "must not desugar unary minus to subtract-from-zero, got:\n{formatted}"
    );
    let twice = format_source(&formatted, lemma::SourceType::Volatile).unwrap();
    assert_eq!(formatted, twice);
}

#[test]
fn explicit_subtract_from_zero_preserved() {
    let src = "spec test rule x: 0 - 2";
    assert_eq!(format_and_extract_rule_expr(src), "0 - 2");
}

#[test]
fn unary_minus_measure_stays_signed() {
    let src = "spec test rule x: -5 kilometer";
    assert_eq!(format_and_extract_rule_expr(src), "-5 kilometer");
}

#[test]
fn unary_minus_over_sum_stays_subtract_from_zero() {
    let src = "spec test data a: 1 data b: 2 rule x: -(a + b)";
    assert_eq!(format_and_extract_rule_expr(src), "0 - (a + b)");
}

#[test]
fn same_prec_right_add_under_subtract_keeps_parens() {
    let src = "spec test data a: 1 data b: 2 data c: 3 rule x: a - (b + c)";
    assert_eq!(format_and_extract_rule_expr(src), "a - (b + c)");
}

#[test]
fn same_prec_right_subtract_under_subtract_keeps_parens() {
    let src = "spec test data a: 1 data b: 2 data c: 3 rule x: a - (b - c)";
    assert_eq!(format_and_extract_rule_expr(src), "a - (b - c)");
}

#[test]
fn same_prec_right_parens_format_idempotent() {
    for expr in ["a - (b + c)", "a - (b - c)", "0 - (a + b)"] {
        let src = format!("spec test data a: 1 data b: 2 data c: 3 rule x: {expr}");
        let once = format_source(&src, lemma::SourceType::Volatile).unwrap();
        let twice = format_source(&once, lemma::SourceType::Volatile).unwrap();
        assert_eq!(once, twice, "idempotent for {expr}");
        assert!(
            once.contains(expr),
            "formatted source must keep parens for {expr}, got:\n{once}"
        );
    }
}

// =============================================================================
// Idempotency (synthetic expressions)
// =============================================================================

#[test]
fn idempotency_precedence_expressions() {
    let expressions = [
        "(a + b) * c",
        "a + b * c",
        "a * (b + c)",
        "(a + b) + c",
        "not a and b",
        "not (a and b)",
        "(a + b) * (c - d)",
    ];
    for expr in expressions {
        let src = format!(
            "spec test data a: 1 data b: 2 data c: 3 data d: 4 rule x: {}",
            expr
        );
        let output1 = format_source(&src, lemma::SourceType::Volatile)
            .unwrap_or_else(|e| panic!("first format failed for '{}': {:?}", expr, e));
        let output2 = format_source(&output1, lemma::SourceType::Volatile).unwrap_or_else(|e| {
            panic!(
                "second format failed for '{}': {:?} First output: {}",
                expr, e, output1
            )
        });
        assert_eq!(
            output1, output2,
            "formatter is not idempotent for expression '{}'. First: {} Second: {}",
            expr, output1, output2
        );
    }
}

// =============================================================================
// `uses` / qualified-parent round-trip tests
// =============================================================================

fn import_with_data_lines(formatted: &str, spec_name: &str) -> Vec<String> {
    let marker = format!("spec {spec_name}\n");
    formatted
        .split("rule ")
        .next()
        .unwrap_or(formatted)
        .split(&marker)
        .nth(1)
        .unwrap_or("")
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

const GROUPED_USES_WITH_LINES: &[&str] = &[
    "uses x",
    "  -> with name: \"Ben\"",
    "uses y",
    "  -> with age: 15",
];

#[test]
fn format_groups_with_under_each_uses() {
    let source = r#"spec test

uses x
  -> with name: "Ben"
uses y
  -> with age: 15

rule r: 1
"#;
    let formatted = format_source(source, lemma::SourceType::Volatile).unwrap();
    let lines = import_with_data_lines(&formatted, "test");
    assert_eq!(
        lines, GROUPED_USES_WITH_LINES,
        "expected with under matching uses, got:\n{formatted}"
    );
    let reformatted = format_source(&formatted, lemma::SourceType::Volatile).unwrap();
    assert_eq!(
        formatted, reformatted,
        "grouped uses/with layout is not idempotent"
    );
}

#[test]
fn format_preserves_uses_block_declaration_order() {
    let source = r#"spec test

uses y
  -> with age: 15
uses x
  -> with name: "Ben"

rule r: 1
"#;
    let expected = [
        "uses y",
        "  -> with age: 15",
        "uses x",
        "  -> with name: \"Ben\"",
    ];
    let formatted = format_source(source, lemma::SourceType::Volatile).unwrap();
    let lines = import_with_data_lines(&formatted, "test");
    assert_eq!(
        lines, expected,
        "uses block order must be preserved, got:\n{formatted}"
    );
}

#[test]
fn round_trip_multiple_bare_uses_one_per_line() {
    let source = "spec consumer\nuses a\nuses b\nuses c";
    let formatted = format_source(source, lemma::SourceType::Volatile).unwrap();
    assert!(
        formatted.contains("uses a\n")
            && formatted.contains("uses b\n")
            && formatted.contains("uses c\n"),
        "expected one uses line per import, got: {}",
        formatted
    );
    assert!(
        !formatted.contains("uses a, b") && !formatted.contains(", b,"),
        "must not comma-join imports, got: {}",
        formatted
    );
    let reformatted = format_source(&formatted, lemma::SourceType::Volatile).unwrap();
    assert_eq!(
        formatted, reformatted,
        "multiple bare uses one per line is not idempotent"
    );
}

#[test]
fn round_trip_qualified_type_import_with_effective_on_uses() {
    let source = "spec consumer uses finance 2026-01-15 data money: finance.money data p: money";
    let formatted = format_source(source, lemma::SourceType::Volatile).unwrap();
    assert!(
        formatted.contains("finance.money") && formatted.contains("uses"),
        "expected uses + qualified parent type in formatted output: {}",
        formatted
    );
    let reformatted = format_source(&formatted, lemma::SourceType::Volatile).unwrap();
    assert_eq!(
        formatted, reformatted,
        "qualified type import with effective on uses is not idempotent"
    );
}

#[test]
fn round_trip_qualified_type_import_registry_with_effective_on_uses() {
    let source =
        "spec consumer uses @iso/countries alpha2 2026-01-15 data country: alpha2.code data c: country";
    let formatted = format_source(source, lemma::SourceType::Volatile).unwrap();
    assert!(
        formatted.contains("alpha2.code") && formatted.contains("@iso/countries"),
        "expected registry uses + qualified type in formatted output: {}",
        formatted
    );
    let reformatted = format_source(&formatted, lemma::SourceType::Volatile).unwrap();
    assert_eq!(
        formatted, reformatted,
        "registry qualified type import with effective is not idempotent"
    );
}

#[test]
fn round_trip_slashed_opaque_spec_name_uses_full_alias() {
    let source = r#"
spec finance/units
data money: measure -> unit eur: 1

spec consumer
uses finance/units
data fee: finance/units.money
"#;
    let formatted = format_source(source, lemma::SourceType::Volatile).unwrap();
    assert!(
        formatted.contains("uses finance/units") && !formatted.contains("uses units:"),
        "implicit alias is the full opaque name, got:\n{formatted}"
    );
    assert!(
        formatted.contains("finance/units.money"),
        "qualified type must keep opaque alias, got:\n{formatted}"
    );
    let reformatted = format_source(&formatted, lemma::SourceType::Volatile).unwrap();
    assert_eq!(
        formatted, reformatted,
        "slashed opaque uses + qualified type is not idempotent"
    );
}

// =============================================================================
// `repo` declaration formatting
// =============================================================================

#[test]
fn format_repo_block_preserves_repository_header_in_output() {
    let src = "repo pack\n\nspec a\ndata x: 1";
    let parsed = parse(src, lemma::SourceType::Volatile, &ResourceLimits::default()).unwrap();
    let out = format_parse_result(&parsed);
    assert!(
        out.contains("repo pack"),
        "formatted output must retain repo header:\n{out}"
    );
}

#[test]
fn format_two_repo_blocks_emit_two_headers() {
    let src = "repo p1\n\nspec a\ndata x: 1\n\nrepo p2\n\nspec b\ndata y: 2";
    let parsed = parse(src, lemma::SourceType::Volatile, &ResourceLimits::default()).unwrap();
    let out = format_parse_result(&parsed);
    assert!(out.contains("repo p1") && out.contains("repo p2"), "{out}");
}

#[test]
fn format_repo_sections_idempotent_under_format_parse_result_roundtrip() {
    let src = "repo q\n\nspec z\ndata n: 7";
    let parsed = parse(src, lemma::SourceType::Volatile, &ResourceLimits::default()).unwrap();
    let once = format_parse_result(&parsed);
    let again = parse(
        &once,
        lemma::SourceType::Volatile,
        &ResourceLimits::default(),
    )
    .unwrap();
    let twice = format_parse_result(&again);
    assert_eq!(
        once, twice,
        "format_parse_result must be stable when reapplied"
    );
}

#[test]
fn format_compound_unit_metre_per_second_idempotent() {
    let source = r#"spec test
data velocity: measure
  -> unit mps: meter/second
"#;
    let st = lemma::SourceType::Volatile;
    let once = format_source(source, st.clone()).expect("format");
    assert!(
        once.contains("meter/second"),
        "formatted unit must use slash notation, got:\n{once}"
    );
    assert!(
        !once.contains("second^-1"),
        "must not format denominator as negative exponent, got:\n{once}"
    );
    let twice = format_source(&once, st).expect("reformat");
    assert_eq!(once, twice, "compound unit format must be idempotent");
}

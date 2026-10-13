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
fn not_call_form_before_and() {
    let src = "spec test data a: true data b: true rule x: not (a) and b";
    assert_eq!(format_and_extract_rule_expr(src), "not(a) and b");
}

#[test]
fn not_call_form_around_and() {
    let src = "spec test data a: true data b: true rule x: not (a and b)";
    assert_eq!(format_and_extract_rule_expr(src), "not(a and b)");
}

#[test]
fn not_bare_operand_followed_by_and_is_parse_error() {
    let src = "spec test data a: true data b: true rule x: not a and b";
    let err = format_source(src, lemma::SourceType::Volatile).unwrap_err();
    assert!(err.to_string().contains("not(a) and b"), "{err}");
}

#[test]
fn prefix_operator_canonical_form() {
    let spec = "spec t data x: 1.5 data y: 2 data ok: true data band: number range";
    for (input, expected) in [
        ("floor x", "floor x"),
        ("floor (x)", "floor x"),
        ("floor(x) + 1", "floor(x) + 1"),
        ("(floor x) + 1", "floor(x) + 1"),
        ("floor (x + 1)", "floor(x + 1)"),
        ("1 + floor x", "1 + floor(x)"),
        ("(floor x) * (floor y)", "floor(x) * floor(y)"),
        ("ok and not ok", "ok and not ok"),
        ("ok and not (y > 1)", "ok and not(y > 1)"),
        ("floor abs x", "floor abs x"),
        ("round (x as number)", "round(x as number)"),
        ("floor(x) > 1 and ok", "floor(x) > 1 and ok"),
        ("y > floor(x) and ok", "y > floor(x) and ok"),
        ("lower band", "lower band"),
        ("lower(band) + 1", "lower(band) + 1"),
        ("not ok", "not ok"),
        ("not (y > 1)", "not(y > 1)"),
        ("abs (-1)", "abs(-1)"),
    ] {
        let src = format!("{spec} rule x: {input}");
        assert_eq!(
            format_and_extract_rule_expr(&src),
            expected,
            "input: {input}"
        );
        let formatted = format_source(&src, lemma::SourceType::Volatile)
            .unwrap_or_else(|e| panic!("format failed for '{input}': {e}"));
        let again = format_source(&formatted, lemma::SourceType::Volatile)
            .unwrap_or_else(|e| panic!("reformat failed for '{formatted}': {e}"));
        assert_eq!(again, formatted, "not idempotent: {input}");
    }
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
        "not(a) and b",
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

#[test]
fn list_key_comes_before_with() {
    let source = r#"
spec menu_item
data drink: text
data price: number

spec coffee_order
uses menu: menu_item list
  -> with drink: "espresso", price: 1
  -> key drink
"#;
    let formatted = format_source(source, lemma::SourceType::Volatile).expect("format");
    let key = formatted.find("-> key drink").expect("key");
    let with = formatted.find("-> with").expect("with");
    assert!(key < with, "key before with, got:\n{formatted}");
}

#[test]
fn inline_rows_align_fields() {
    let source = r#"
spec menu_item
data drink: text
data price: number

spec coffee_order
uses menu: menu_item list
  -> with drink: "espresso", price: 2.50
  -> with drink: "latte", price: 3.50
"#;
    let formatted = format_source(source, lemma::SourceType::Volatile).expect("format");
    assert!(
        formatted.contains("drink:") && formatted.contains("price:"),
        "{formatted}"
    );
    let espresso = formatted.find("espresso").expect("espresso");
    let latte = formatted.find("latte").expect("latte");
    assert!(espresso < latte, "{formatted}");
}

#[test]
fn long_inline_row_falls_back_to_one_field_per_line() {
    let source = r#"
spec menu_item
data drink: text
data description: text
data origin: text
data price: number

spec coffee_order
uses menu: menu_item list
  -> with drink: "single-origin espresso from the highlands", description: "a very long tasting note that will not fit", origin: "ethiopia", price: 4.50
"#;
    let formatted = format_source(source, lemma::SourceType::Volatile).expect("format");
    assert!(
        formatted.contains("drink:")
            && formatted
                .lines()
                .filter(|line| line.contains("drink:"))
                .count()
                >= 1,
        "{formatted}"
    );
    assert!(
        formatted.lines().any(|line| line.trim_end().ends_with(',')),
        "vertical field pairs end in comma, got:\n{formatted}"
    );
}

#[test]
fn comma_list_wraps_with_deeper_indent() {
    let source = r#"
spec fares_card
data fares: number list
  -> with 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20
"#;
    let formatted = format_source(source, lemma::SourceType::Volatile).expect("format");
    assert!(formatted.contains(","), "{formatted}");
    assert!(
        formatted
            .lines()
            .any(|line| line.starts_with("       ") || line.starts_with("    ")),
        "continuation indent, got:\n{formatted}"
    );
}

#[test]
fn list_valued_unless_uses_split_then() {
    let source = r#"
spec rate
data band: text
spec parcel
uses standard_rates: rate list
  -> key band
  -> with band: "light"
uses express_rates: rate list
  -> key band
  -> with band: "light"
data express: boolean
rule rates:
  standard_rates
  unless express then express_rates
"#;
    let formatted = format_source(source, lemma::SourceType::Volatile).expect("format");
    assert!(
        formatted.contains("then") && formatted.contains("unless express"),
        "{formatted}"
    );
}

#[test]
fn command_argument_canonical_form() {
    let spec = "spec t data money: measure -> unit eur: 1.00 data fares: money list -> with 1 eur data take: number";
    for (input, expected) in [
        ("fares -> top (take)", "fares -> top take"),
        ("fares -> top(2)", "fares -> top 2"),
        ("fares -> top(take + 1)", "fares -> top(take + 1)"),
        ("fares -> find (1 + 1)", "fares -> find(1 + 1)"),
        ("fares -> where (>= 1 eur)", "fares -> where >= 1 eur"),
        ("fares -> sum * 2", "fares -> sum * 2"),
        ("(fares -> sum) - 1 eur", "fares -> sum - 1 eur"),
        ("lines -> sum (qty * price)", "lines -> sum(qty * price)"),
        ("lines -> sum(qty) * 2", "lines -> sum(qty) * 2"),
        ("2 * lines -> sum(qty)", "2 * lines -> sum(qty)"),
        ("2 * lines -> sum qty", "2 * lines -> sum(qty)"),
        (
            "lines -> sum(qty) > 1 and ok",
            "lines -> sum(qty) > 1 and ok",
        ),
        ("staff -> where (salary > 1)", "staff -> where salary > 1"),
        (
            "staff -> where (a > 1 and b < 2)",
            "staff -> where(a > 1 and b < 2)",
        ),
        (
            "staff -> sort (salary * 2) desc",
            "staff -> sort(salary * 2) desc",
        ),
        ("fares -> sort", "fares -> sort asc"),
        ("staff -> sort salary", "staff -> sort salary asc"),
    ] {
        let src = format!("{spec} rule x: {input}");
        assert_eq!(format_and_extract_rule_expr(&src), expected, "{input}");
        let once = format_source(&src, lemma::SourceType::Volatile).unwrap();
        let twice = format_source(&once, lemma::SourceType::Volatile).unwrap();
        assert_eq!(once, twice, "{input}");
    }
}

#[test]
fn short_expression_chain_stays_on_one_line() {
    let source = r#"
spec employee
data salary: number
spec payroll
uses staff: employee list
  -> with salary: 1
rule top_salary:
  staff -> sort salary desc -> first
"#;
    let formatted = format_source(source, lemma::SourceType::Volatile).expect("format");
    assert!(
        formatted
            .lines()
            .any(|line| line.contains("staff -> sort salary desc -> first")),
        "{formatted}"
    );
}

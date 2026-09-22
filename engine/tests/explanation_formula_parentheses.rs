//! Flat explain formula parentheses: body / Compose expression must re-parse
//! to the same association as the AST.
//!
//! explanation_display parenthesizes by shared precedence and associativity
//! (same rule as Expression Display).

use lemma::{format_explanation, DateTimeValue, Engine, Explanation};
use std::collections::HashMap;

fn load(code: &str) -> Engine {
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("spec must load");
    engine
}

fn run_out(engine: &Engine, data: HashMap<String, String>) -> lemma::Response {
    let now = DateTimeValue::now();
    engine
        .run(
            None,
            "t",
            Some(&now),
            data,
            Some(&["out".to_string()]),
            true,
        )
        .expect("evaluation must succeed")
}

fn out_explanation(response: &lemma::Response) -> &Explanation {
    response
        .results
        .get("out")
        .expect("out in response")
        .explanation
        .as_ref()
        .expect("out explanation")
}

fn assert_out_body(code: &str, expected_body: &str) -> Explanation {
    let engine = load(code);
    let response = run_out(&engine, HashMap::new());
    let explanation = out_explanation(&response).clone();
    assert_eq!(
        explanation.body, expected_body,
        "explanation.body must match parenthesized flat formula"
    );
    explanation
}

fn numbers_abc() -> &'static str {
    r#"
spec t
data a: 2
data b: 3
data c: 5
"#
}

// ── A. Motivating cost_price case ───────────────────────────────────────────

#[test]
fn product_times_sum_of_rate_keeps_parens_in_body() {
    let engine = load(
        r#"
spec t
data q: 120
data p: 4
data l: 21.58
data tput: 12
rule out: q * (p + l / tput)
"#,
    );
    let response = run_out(&engine, HashMap::new());
    assert_eq!(
        response.results.get("out").expect("out").result(),
        Some("695.8")
    );
    let explanation = out_explanation(&response);
    assert_eq!(explanation.body, "q * (p + l / tput)");
}

// ── B. Mixed precedence ─────────────────────────────────────────────────────

#[test]
fn multiply_wraps_sum_right_operand() {
    assert_out_body(
        &format!(
            r#"{}
rule out: a * (b + c)
"#,
            numbers_abc()
        ),
        "a * (b + c)",
    );
}

#[test]
fn multiply_wraps_sum_left_operand() {
    assert_out_body(
        &format!(
            r#"{}
rule out: (a + b) * c
"#,
            numbers_abc()
        ),
        "(a + b) * c",
    );
}

#[test]
fn divide_wraps_sum_denominator() {
    assert_out_body(
        &format!(
            r#"{}
rule out: a / (b + c)
"#,
            numbers_abc()
        ),
        "a / (b + c)",
    );
}

#[test]
fn add_does_not_wrap_product() {
    assert_out_body(
        &format!(
            r#"{}
rule out: a + b * c
"#,
            numbers_abc()
        ),
        "a + b * c",
    );
}

#[test]
fn subtract_does_not_wrap_product() {
    assert_out_body(
        &format!(
            r#"{}
rule out: a - b * c
"#,
            numbers_abc()
        ),
        "a - b * c",
    );
}

// ── C. Same-precedence left-associativity ───────────────────────────────────

#[test]
fn subtract_wraps_right_subtract() {
    assert_out_body(
        &format!(
            r#"{}
rule out: a - (b - c)
"#,
            numbers_abc()
        ),
        "a - (b - c)",
    );
}

#[test]
fn divide_wraps_right_divide() {
    assert_out_body(
        &format!(
            r#"{}
rule out: a / (b / c)
"#,
            numbers_abc()
        ),
        "a / (b / c)",
    );
}

#[test]
fn modulo_wraps_right_modulo() {
    assert_out_body(
        &format!(
            r#"{}
rule out: a % (b % c)
"#,
            numbers_abc()
        ),
        "a % (b % c)",
    );
}

#[test]
fn left_assoc_chain_needs_no_inner_parens() {
    assert_out_body(
        &format!(
            r#"{}
rule out: (a - b) - c
"#,
            numbers_abc()
        ),
        "a - b - c",
    );
}

// ── D. Power (right-associative) ────────────────────────────────────────────

#[test]
fn power_wraps_left_power_when_explicitly_left_grouped() {
    assert_out_body(
        &format!(
            r#"{}
rule out: (a ^ b) ^ c
"#,
            numbers_abc()
        ),
        "(a ^ b) ^ c",
    );
}

#[test]
fn power_right_assoc_chain_needs_no_inner_parens() {
    assert_out_body(
        &format!(
            r#"{}
rule out: a ^ (b ^ c)
"#,
            numbers_abc()
        ),
        "a ^ b ^ c",
    );
}

#[test]
fn multiply_then_power_needs_no_parens_on_power() {
    assert_out_body(
        &format!(
            r#"{}
rule out: a * b ^ c
"#,
            numbers_abc()
        ),
        "a * b ^ c",
    );
}

#[test]
fn power_wraps_product_left_operand() {
    assert_out_body(
        &format!(
            r#"{}
rule out: (a * b) ^ c
"#,
            numbers_abc()
        ),
        "(a * b) ^ c",
    );
}

// ── E. Unary / conversion / logic ───────────────────────────────────────────

#[test]
fn negate_wraps_sum() {
    // Unary minus lowers to `0 - …`; the sum operand still needs parentheses.
    assert_out_body(
        &format!(
            r#"{}
rule out: -(a + b)
"#,
            numbers_abc()
        ),
        "0 - (a + b)",
    );
}

#[test]
fn reciprocal_or_one_over_wraps_sum() {
    // Reciprocal of a sum: display uses 1/... and must parenthesize the sum.
    assert_out_body(
        &format!(
            r#"{}
rule out: 1 / (a + b)
"#,
            numbers_abc()
        ),
        "1 / (a + b)",
    );
}

#[test]
fn not_wraps_and() {
    let engine = load(
        r#"
spec t
data a: true
data b: false
rule out: not (a and b)
"#,
    );
    let response = run_out(&engine, HashMap::new());
    let explanation = out_explanation(&response);
    assert_eq!(explanation.body, "not (a and b)");
}

#[test]
fn and_does_not_wrap_comparison() {
    let engine = load(
        r#"
spec t
data a: 2
data b: 3
rule out: a > 0 and b > 0
"#,
    );
    let response = run_out(&engine, HashMap::new());
    let explanation = out_explanation(&response);
    assert_eq!(explanation.body, "a > 0 and b > 0");
}

#[test]
fn unit_conversion_binds_tight() {
    let engine = load(
        r#"
spec t
data n: 100
rule out: n / 2 as number
"#,
    );
    let response = run_out(&engine, HashMap::new());
    let explanation = out_explanation(&response);
    // `as` binds tighter than `/`, so this is n / (2 as number) with no extra parens.
    assert_eq!(explanation.body, "n / 2 as number");
}

// ── F. Deeper nesting ───────────────────────────────────────────────────────

#[test]
fn nested_cost_style() {
    assert_out_body(
        r#"
spec t
data q: 2
data p: 3
data l: 4
data tput: 5
data u: 6
rule out: q * (p + l / (tput + u))
"#,
        "q * (p + l / (tput + u))",
    );
}

#[test]
fn sum_of_products_no_extra_parens() {
    assert_out_body(
        &format!(
            r#"{}
rule out: a * b + c * a
"#,
            numbers_abc()
        ),
        "a * b + c * a",
    );
}

#[test]
fn product_of_sums_both_wrapped() {
    assert_out_body(
        &format!(
            r#"{}
rule out: (a + b) * (c + a)
"#,
            numbers_abc()
        ),
        "(a + b) * (c + a)",
    );
}

// ── G. Explain surface parity ───────────────────────────────────────────────

#[test]
fn format_explanation_shows_parenthesized_body_line() {
    let explanation = assert_out_body(
        &format!(
            r#"{}
rule out: a * (b + c)
"#,
            numbers_abc()
        ),
        "a * (b + c)",
    );
    let formatted = format_explanation(&explanation);
    let first_lines: Vec<&str> = formatted.lines().take(3).collect();
    assert!(
        first_lines.iter().any(|line| line.contains("a * (b + c)")),
        "format_explanation must show parenthesized body, got:\n{formatted}"
    );
}

#[test]
fn mathop_style_regression_sqrt_product() {
    assert_out_body(
        r#"
spec t
rule out: (sqrt 4) * (sqrt 9)
"#,
        "sqrt(4) * sqrt(9)",
    );
}

// ── H. No spurious parentheses ──────────────────────────────────────────────

#[test]
fn unnecessary_parens_not_invented_for_flat_sum() {
    assert_out_body(
        &format!(
            r#"{}
rule out: a + b + c
"#,
            numbers_abc()
        ),
        "a + b + c",
    );
}

#[test]
fn unnecessary_parens_not_invented_for_flat_product() {
    assert_out_body(
        &format!(
            r#"{}
rule out: a * b * c
"#,
            numbers_abc()
        ),
        "a * b * c",
    );
}

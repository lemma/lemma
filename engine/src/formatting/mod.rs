//! Lemma source code formatting.
//!
//! Formats parsed specs into canonical Lemma source text. Uses `AsLemmaSource`
//! and `Expression::Display` for syntax; this module handles layout only.
//! Canonical source includes ASCII-lowercase logical identifier names.

use crate::parsing::ast::{
    arithmetic_associativity, expression_precedence, operand_needs_parentheses, AsLemmaSource,
    Associativity, Constraint, DataValue, Expression, ExpressionKind, LemmaData, LemmaRule,
    LemmaSpec, OperandSide,
};
use crate::parsing::{parse, ParseResult};
use crate::{Error, ResourceLimits};

/// Soft line length limit. Longer lines may be wrapped (unless clauses, expressions).
/// Data and other constructs are not broken if they exceed this.
/// 56 has been chosen to fit on an average mobile screen with an 11pt font.
pub const MAX_COLS: usize = 56;

// =============================================================================
// Public entry points
// =============================================================================

/// Format a sequence of parsed specs into canonical Lemma source.
///
/// specs are separated by two blank lines.
/// The result ends with a single newline.
#[must_use]
pub fn format_specs(specs: &[LemmaSpec]) -> String {
    let refs: Vec<&LemmaSpec> = specs.iter().collect();
    format_spec_refs(&refs)
}

/// Like [`format_specs`] for borrowed specs (e.g. from Context storage).
#[must_use]
pub fn format_spec_refs(specs: &[&LemmaSpec]) -> String {
    let mut out = String::new();
    for (index, spec) in specs.iter().enumerate() {
        if index > 0 {
            out.push_str("\n\n");
        }
        out.push_str(&format_spec(spec, MAX_COLS));
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Format a [`ParseResult`] (repository groups + specs) into canonical Lemma source.
#[must_use]
pub fn format_parse_result(result: &ParseResult) -> String {
    let mut blocks: Vec<String> = Vec::new();
    for (repo, specs) in &result.repositories {
        let mut prefix = String::new();
        if let Some(name) = repo.name.as_deref() {
            prefix.push_str("repo ");
            prefix.push_str(name);
            prefix.push_str("\n\n");
        }
        if specs.is_empty() {
            if !prefix.is_empty() {
                blocks.push(prefix);
            }
            continue;
        }
        let body = format_specs(specs.as_slice());
        if prefix.is_empty() {
            blocks.push(body);
        } else {
            prefix.push_str(&body);
            blocks.push(prefix);
        }
    }
    let mut out = blocks.join("\n\n");
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Parse a source string and format it to canonical Lemma source.
///
/// Returns an error if the source does not parse.
pub fn format_source(
    source: &str,
    source_type: crate::parsing::source::SourceType,
) -> Result<String, Error> {
    let limits = ResourceLimits::default();
    let result = parse(source, source_type, &limits)?;
    Ok(format_parse_result(&result))
}

// =============================================================================
// Spec
// =============================================================================

pub(crate) fn format_spec(spec: &LemmaSpec, max_cols: usize) -> String {
    let mut out = String::new();
    out.push_str("spec ");
    out.push_str(&spec.name);
    if let crate::parsing::ast::EffectiveDate::DateTimeValue(ref af) = spec.effective_from {
        out.push(' ');
        out.push_str(&af.to_string());
    }
    out.push('\n');

    if let Some(ref commentary) = spec.commentary {
        out.push_str("\"\"\"\n");
        out.push_str(commentary);
        out.push_str("\n\"\"\"\n");
    }

    for meta in &spec.meta_fields {
        out.push_str(&format!(
            "meta {}: {}\n",
            meta.key,
            AsLemmaSource(&meta.value)
        ));
    }

    if !spec.data.is_empty() {
        format_sorted_data(&spec.data, &mut out, "");
    }

    if !spec.rules.is_empty() {
        if !spec.data.is_empty() {
            out.push_str("\n\n");
        } else {
            out.push('\n');
        }
        for (index, rule) in spec.rules.iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            let rule_text = format_rule(rule, max_cols);
            for line in rule_text.lines() {
                out.push_str(line);
                out.push('\n');
            }
        }
    }

    out
}

// =============================================================================
// Data
// =============================================================================

/// Two spaces after `line_prefix` for each `-> ...` constraint line under `data ...: ...`.
const DATA_CONSTRAINT_INDENT: &str = "  ";

fn data_constraints_nonempty(constraints: &Option<Vec<Constraint>>) -> bool {
    constraints.as_ref().is_some_and(|v| !v.is_empty())
}

fn data_value_has_arrow_constraints(value: &DataValue) -> bool {
    match value {
        DataValue::Definition { constraints, .. } => data_constraints_nonempty(constraints),
        DataValue::Import { .. } => false,
    }
}

fn format_with_rhs(rhs: &crate::parsing::ast::WithRhs) -> String {
    match rhs {
        crate::parsing::ast::WithRhs::Literal(v) => format!("{}", AsLemmaSource(v)),
        crate::parsing::ast::WithRhs::Reference { target } => target.to_string(),
    }
}

fn data_value_rhs_for_spec_body(value: &DataValue, continuation_prefix: &str) -> String {
    match value {
        DataValue::Definition {
            base,
            constraints,
            value,
        } if data_constraints_nonempty(constraints) => {
            let cs = constraints
                .as_ref()
                .expect("BUG: constraints checked above");
            let head: String = if base.is_none() {
                match value {
                    Some(v) => format!("{}", AsLemmaSource(v)),
                    None => String::new(),
                }
            } else {
                match base.as_ref() {
                    Some(b) => format!("{}", b),
                    None => String::new(),
                }
            };
            let mut out = head;
            for row in cs {
                out.push('\n');
                out.push_str(continuation_prefix);
                out.push_str("-> ");
                out.push_str(&crate::parsing::ast::format_constraint_as_source(
                    &row.command,
                    &row.args,
                ));
            }
            out
        }
        DataValue::Definition { .. } => format!("{}", AsLemmaSource(value)),
        DataValue::Import { .. } => unreachable!("BUG: format_data called on Import row"),
    }
}

fn data_declaration_keyword(data: &LemmaData) -> &'static str {
    match &data.value {
        DataValue::Import { .. } => unreachable!("BUG: format_data called on Import row"),
        DataValue::Definition { .. } => "data",
    }
}

fn format_data(data: &LemmaData, line_prefix: &str) -> String {
    let kw = data_declaration_keyword(data);
    let ref_str = format!("{}", data.reference);
    let continuation = format!("{line_prefix}{DATA_CONSTRAINT_INDENT}");
    let rhs = data_value_rhs_for_spec_body(&data.value, &continuation);
    if let Some((first, rest)) = rhs.split_once('\n') {
        format!("{kw} {}: {}\n{}", ref_str, first, rest)
    } else {
        format!("{kw} {}: {}", ref_str, rhs)
    }
}

/// Byte length from start of `data ` or `with ` through the single space after `:` (same layout as [`format_data`]).
fn data_line_prefix_len_before_rhs(keyword: &str, ref_str: &str) -> usize {
    keyword.len() + 1 + ref_str.len() + 2
}

fn data_is_simple_single_line(data: &LemmaData, line_prefix: &str) -> bool {
    if data_value_has_arrow_constraints(&data.value) {
        return false;
    }
    let continuation = format!("{line_prefix}{DATA_CONSTRAINT_INDENT}");
    let rhs = data_value_rhs_for_spec_body(&data.value, &continuation);
    !rhs.contains('\n')
}

fn push_formatted_simple_data_line_padded(
    out: &mut String,
    data: &LemmaData,
    line_prefix: &str,
    target_prefix_len_before_rhs: usize,
) {
    let kw = data_declaration_keyword(data);
    let ref_str = format!("{}", data.reference);
    let continuation = format!("{line_prefix}{DATA_CONSTRAINT_INDENT}");
    let rhs = data_value_rhs_for_spec_body(&data.value, &continuation);
    let base = data_line_prefix_len_before_rhs(kw, &ref_str);
    let gap = 1 + target_prefix_len_before_rhs.saturating_sub(base);
    out.push_str(line_prefix);
    out.push_str(kw);
    out.push(' ');
    out.push_str(&ref_str);
    out.push(':');
    out.push_str(&" ".repeat(gap));
    out.push_str(&rhs);
}

fn emit_data_row_group(rows: &[&LemmaData], line_prefix: &str, out: &mut String) {
    let mut i = 0;
    while i < rows.len() {
        if data_is_simple_single_line(rows[i], line_prefix) {
            let run_start = i;
            i += 1;
            while i < rows.len() && data_is_simple_single_line(rows[i], line_prefix) {
                i += 1;
            }
            let run_end = i;
            let target = (run_start..run_end)
                .map(|k| {
                    let row = rows[k];
                    let kw = data_declaration_keyword(row);
                    let ref_str = format!("{}", row.reference);
                    data_line_prefix_len_before_rhs(kw, &ref_str)
                })
                .max()
                .expect("BUG: non-empty run");
            for row in rows[run_start..run_end].iter().copied() {
                push_formatted_simple_data_line_padded(out, row, line_prefix, target);
                out.push('\n');
            }
        } else {
            let row = rows[i];
            out.push_str(line_prefix);
            out.push_str(&format_data(row, line_prefix));
            out.push('\n');
            if data_value_has_arrow_constraints(&row.value) && i + 1 < rows.len() {
                out.push('\n');
            }
            i += 1;
        }
    }
}

fn format_import_header(data: &LemmaData) -> String {
    let alias = &data.reference.name;
    let DataValue::Import { spec_ref, .. } = &data.value else {
        unreachable!("BUG: format_import_header called on non-Import data");
    };
    let spec_name = &spec_ref.name;
    if alias == spec_name {
        format!("uses {}", spec_ref)
    } else {
        format!("uses {}: {}", alias, spec_ref)
    }
}

fn format_uses_block(data: &LemmaData, line_prefix: &str) -> String {
    let mut out = format_import_header(data);
    let DataValue::Import { bindings, .. } = &data.value else {
        unreachable!("BUG: format_uses_block called on non-Import data");
    };
    for binding in bindings {
        out.push('\n');
        out.push_str(line_prefix);
        out.push_str(DATA_CONSTRAINT_INDENT);
        out.push_str("-> ");
        out.push_str(&crate::parsing::ast::format_assignment_continuation(
            "with",
            &format!("{}", binding.path),
            &format_with_rhs(&binding.rhs),
        ));
    }
    out
}

/// Group data into sections separated by blank lines:
///
/// 1. Imports (`uses`) with `-> with` bindings — declaration order
/// 2. Regular local `data` — declaration order
fn format_sorted_data(data: &[LemmaData], out: &mut String, line_prefix: &str) {
    let mut regular: Vec<&LemmaData> = Vec::new();
    let mut imports: Vec<&LemmaData> = Vec::new();

    for data in data {
        if matches!(&data.value, DataValue::Import { .. }) {
            imports.push(data);
        } else {
            regular.push(data);
        }
    }

    let emit_group =
        |rows: &[&LemmaData], out: &mut String| emit_data_row_group(rows, line_prefix, out);

    if !imports.is_empty() {
        out.push('\n');

        for (i, row) in imports.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(line_prefix);
            out.push_str(&format_uses_block(row, line_prefix));
            out.push('\n');
        }
    }

    if !regular.is_empty() {
        out.push('\n');
        emit_group(&regular, out);
    }
}

// =============================================================================
// Rules
// =============================================================================

const UNLESS_LINE_PREFIX: &str = "  unless ";

/// Rule body always starts on the line after `rule name:`.
///
/// When every `unless` clause fits on one line under `max_cols`, `then` columns align across
/// sisters. If any clause needs split `then` (wrapped condition, wrapped result, or oversize
/// flat line), every clause on the rule uses split `then` at a fixed 4-space indent.
fn format_rule(rule: &LemmaRule, max_cols: usize) -> String {
    let expr_indent = "  ";
    let body = format_expr_wrapped(&rule.expression, max_cols, expr_indent, 10);
    let mut out = String::new();
    out.push_str("rule ");
    out.push_str(&rule.name);
    out.push_str(":\n");
    out.push_str(expr_indent);
    out.push_str(&body);

    let pl = UNLESS_LINE_PREFIX.len();
    let naive_single_len = |cond: &str, res: &str| pl + cond.len() + 6 + res.len();
    let aligned_single_len = |res: &str, max_end: usize| max_end + 6 + res.len();
    let unless_condition_budget = max_cols.saturating_sub(pl);

    let mut clauses: Vec<(String, String)> = Vec::new();
    for unless_clause in &rule.unless_clauses {
        let condition = format_expr_wrapped(
            &unless_clause.condition,
            unless_condition_budget,
            "    ",
            10,
        );
        let result = format_expr_wrapped(&unless_clause.result, max_cols, "    ", 10);
        clauses.push((condition, result));
    }

    let clause_needs_split_then = |condition: &str, result: &str| {
        condition.contains('\n')
            || result.contains('\n')
            || naive_single_len(condition, result) > max_cols
    };

    let any_split = clauses.iter().any(|(c, r)| clause_needs_split_then(c, r));

    const SPLIT_THEN_INDENT_SPACES: usize = 4;

    if any_split {
        for (condition, result) in &clauses {
            out.push_str("\n  unless ");
            out.push_str(condition);
            out.push('\n');
            out.push_str(&" ".repeat(SPLIT_THEN_INDENT_SPACES));
            out.push_str("then ");
            out.push_str(result);
        }
    } else {
        let mut singles: Vec<usize> = clauses
            .iter()
            .enumerate()
            .filter(|(_, (c, r))| naive_single_len(c, r) <= max_cols)
            .map(|(i, _)| i)
            .collect();

        loop {
            if singles.is_empty() {
                break;
            }
            let max_end = singles
                .iter()
                .map(|&i| pl + clauses[i].0.len())
                .max()
                .expect("BUG: singles non-empty");
            let before = singles.len();
            singles.retain(|&i| aligned_single_len(&clauses[i].1, max_end) <= max_cols);
            if singles.len() == before {
                break;
            }
        }

        let align_max_end = singles.iter().map(|&i| pl + clauses[i].0.len()).max();

        for (i, (condition, result)) in clauses.iter().enumerate() {
            if singles.contains(&i) {
                let max_end = align_max_end.expect("BUG: singles.contains but align_max_end empty");
                let gap = 1 + max_end.saturating_sub(pl + condition.len());
                out.push('\n');
                out.push_str(UNLESS_LINE_PREFIX);
                out.push_str(condition);
                out.push_str(&" ".repeat(gap));
                out.push_str("then ");
                out.push_str(result);
            } else {
                out.push_str("\n  unless ");
                out.push_str(condition);
                out.push('\n');
                out.push_str(&" ".repeat(SPLIT_THEN_INDENT_SPACES));
                out.push_str("then ");
                out.push_str(result);
            }
        }
    }
    out.push('\n');
    out
}

// =============================================================================
// Expression wrapping (soft line breaking at max_cols)
// =============================================================================

/// Indent every line after the first by `indent`.
fn indent_after_first_line(s: &str, indent: &str) -> String {
    let mut first = true;
    let mut out = String::new();
    for line in s.lines() {
        if first {
            first = false;
            out.push_str(line);
        } else {
            out.push('\n');
            out.push_str(indent);
            out.push_str(line);
        }
    }
    if s.ends_with('\n') {
        out.push('\n');
    }
    out
}

struct BinaryWrapContext<'a> {
    max_cols: usize,
    indent: &'a str,
    parent_prec: u8,
    my_prec: u8,
    assoc: Option<Associativity>,
}

fn format_binary_expr_wrapped(
    left: &Expression,
    op: &str,
    right: &Expression,
    ctx: BinaryWrapContext<'_>,
) -> String {
    let BinaryWrapContext {
        max_cols,
        indent,
        parent_prec,
        my_prec,
        assoc,
    } = ctx;
    let left_inner = format_expr_wrapped(left, max_cols, indent, 10);
    let right_inner = format_expr_wrapped(right, max_cols, indent, 10);
    let left_str = if operand_needs_parentheses(
        expression_precedence(&left.kind),
        my_prec,
        OperandSide::Left,
        assoc,
    ) {
        format!("({})", left_inner)
    } else {
        left_inner
    };
    let right_str = if operand_needs_parentheses(
        expression_precedence(&right.kind),
        my_prec,
        OperandSide::Right,
        assoc,
    ) {
        format!("({})", right_inner)
    } else {
        right_inner
    };
    let single_line = format!("{} {} {}", left_str, op, right_str);
    let body = if single_line.len() <= max_cols && !single_line.contains('\n') {
        single_line
    } else {
        let continued_right = indent_after_first_line(&right_str, indent);
        let continuation = format!("{}{} {}", indent, op, continued_right);
        format!("{}\n{}", left_str, continuation)
    };
    if parent_prec < 10 && operand_needs_parentheses(my_prec, parent_prec, OperandSide::Left, None)
    {
        format!("({})", body)
    } else {
        body
    }
}

/// Format an expression with optional wrapping at arithmetic and `and` operators when over max_cols.
///
/// Binary children use the same parenthesis policy as [`Expression`] display
/// ([`operand_needs_parentheses`]). Pass `10` for top-level (no outer wrap).
fn format_expr_wrapped(
    expr: &Expression,
    max_cols: usize,
    indent: &str,
    parent_prec: u8,
) -> String {
    let my_prec = expression_precedence(&expr.kind);

    match &expr.kind {
        ExpressionKind::Arithmetic(left, op, right) => format_binary_expr_wrapped(
            left,
            &op.to_string(),
            right,
            BinaryWrapContext {
                max_cols,
                indent,
                parent_prec,
                my_prec,
                assoc: Some(arithmetic_associativity(op)),
            },
        ),
        ExpressionKind::LogicalAnd(left, right) => format_binary_expr_wrapped(
            left,
            "and",
            right,
            BinaryWrapContext {
                max_cols,
                indent,
                parent_prec,
                my_prec,
                assoc: Some(Associativity::Left),
            },
        ),
        _ => {
            let s = expr.to_string();
            if parent_prec < 10
                && operand_needs_parentheses(my_prec, parent_prec, OperandSide::Left, None)
            {
                format!("({})", s)
            } else {
                s
            }
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::literals::DateGranularity;
    use crate::parsing::ast::{
        AsLemmaSource, BooleanValue, DateTimeValue, TimeValue, TimezoneValue, Value,
    };
    use rust_decimal::prelude::FromStr;
    use rust_decimal::Decimal;

    /// Helper: format a Value as canonical Lemma source via AsLemmaSource.
    fn fmt_value(v: &Value) -> String {
        format!("{}", AsLemmaSource(v))
    }

    #[test]
    fn test_format_value_text_is_quoted() {
        let v = Value::Text("light".to_string());
        assert_eq!(fmt_value(&v), "\"light\"");
    }

    #[test]
    fn test_format_value_text_escapes_quotes() {
        let v = Value::Text("say \"hello\"".to_string());
        assert_eq!(fmt_value(&v), "\"say \\\"hello\\\"\"");
    }

    #[test]
    fn test_format_value_number() {
        let v = Value::Number(Decimal::from_str("42.50").unwrap());
        assert_eq!(fmt_value(&v), "42.50");
    }

    #[test]
    fn test_format_value_number_integer() {
        let v = Value::Number(Decimal::from_str("100.00").unwrap());
        assert_eq!(fmt_value(&v), "100");
    }

    #[test]
    fn test_format_value_boolean() {
        assert_eq!(fmt_value(&Value::Boolean(BooleanValue::True)), "true");
        assert_eq!(fmt_value(&Value::Boolean(BooleanValue::Yes)), "yes");
        assert_eq!(fmt_value(&Value::Boolean(BooleanValue::No)), "no");
    }

    #[test]
    fn test_format_value_measure() {
        let v = Value::NumberWithUnit(Decimal::from_str("99.50").unwrap(), "eur".to_string());
        assert_eq!(fmt_value(&v), "99.50 eur");
    }

    #[test]
    fn test_format_value_duration_as_measure() {
        let v = Value::NumberWithUnit(Decimal::from(40), "hour".to_string());
        assert_eq!(fmt_value(&v), "40 hour");
    }

    #[test]
    fn test_format_value_calendar() {
        let v = Value::NumberWithUnit(Decimal::from(6), "month".to_string());
        assert_eq!(fmt_value(&v), "6 month");
    }

    #[test]
    fn test_format_value_ratio_percent() {
        let v = Value::NumberWithUnit(Decimal::from_str("10").unwrap(), "percent".to_string());
        assert_eq!(fmt_value(&v), "10%");
    }

    #[test]
    fn test_format_value_ratio_permille() {
        let v = Value::NumberWithUnit(Decimal::from_str("5").unwrap(), "permille".to_string());
        assert_eq!(fmt_value(&v), "5%%");
    }

    #[test]
    fn test_format_value_number_with_unit_named() {
        let v = Value::NumberWithUnit(
            Decimal::from_str("500").unwrap(),
            "basis_points".to_string(),
        );
        assert_eq!(fmt_value(&v), "500 basis_points");
    }

    #[test]
    fn test_format_value_date_only() {
        let v = Value::Date(DateTimeValue {
            year: 2024,
            month: 1,
            day: 15,
            hour: 0,
            minute: 0,
            second: 0,
            microsecond: 0,
            timezone: None,

            granularity: DateGranularity::Full,
        });
        assert_eq!(fmt_value(&v), "2024-01-15");
    }

    #[test]
    fn test_format_value_datetime_with_tz() {
        let v = Value::Date(DateTimeValue {
            year: 2024,
            month: 1,
            day: 15,
            hour: 14,
            minute: 30,
            second: 0,
            microsecond: 0,
            timezone: Some(TimezoneValue {
                offset_hours: 0,
                offset_minutes: 0,
            }),

            granularity: DateGranularity::DateTime,
        });
        assert_eq!(fmt_value(&v), "2024-01-15T14:30:00Z");
    }

    #[test]
    fn test_format_value_time() {
        let v = Value::Time(TimeValue {
            hour: 14,
            minute: 30,
            second: 45,
            microsecond: 0,
            timezone: None,
        });
        assert_eq!(fmt_value(&v), "14:30:45");
    }

    #[test]
    fn test_format_source_preserves_date_granularity() {
        let formatted = format_source(
            "spec x 2026\n",
            crate::parsing::source::SourceType::Volatile,
        )
        .expect("spec x 2026 should format");
        assert!(
            formatted.contains("spec x 2026\n"),
            "year-only effective date must round-trip, got: {formatted}"
        );
        assert!(
            !formatted.contains("2026-01-01"),
            "year-only effective date must not expand, got: {formatted}"
        );
        let reformatted = format_source(&formatted, crate::parsing::source::SourceType::Volatile)
            .expect("reformat");
        assert_eq!(formatted, reformatted, "spec x 2026 must be idempotent");

        let formatted = format_source(
            "spec x 2026-03\n",
            crate::parsing::source::SourceType::Volatile,
        )
        .expect("spec x 2026-03 should format");
        assert!(
            formatted.contains("spec x 2026-03\n"),
            "year-month effective date must round-trip, got: {formatted}"
        );

        let formatted = format_source(
            "spec x 2026-W34\n",
            crate::parsing::source::SourceType::Volatile,
        )
        .expect("spec x 2026-W34 should format");
        assert!(
            formatted.contains("spec x 2026-W34\n"),
            "iso week effective date must round-trip, got: {formatted}"
        );

        let source = "spec consumer\nuses finance 2026\n";
        let formatted = format_source(source, crate::parsing::source::SourceType::Volatile)
            .expect("uses with year should format");
        assert!(
            formatted.contains("uses finance 2026"),
            "uses effective pin must preserve year-only date, got: {formatted}"
        );
        assert!(
            !formatted.contains("2026-01-01"),
            "uses effective pin must not expand year-only date, got: {formatted}"
        );
    }

    #[test]
    fn test_format_source_lowercases_logical_identifiers() {
        let source = r#"spec Test
data Price: number -> suggest 1
rule Total: price
"#;
        let formatted =
            format_source(source, crate::parsing::source::SourceType::Volatile).unwrap();
        assert!(formatted.contains("spec test"), "got: {formatted}");
        assert!(formatted.contains("data price"), "got: {formatted}");
        assert!(formatted.contains("rule total"), "got: {formatted}");
    }

    #[test]
    fn test_format_source_round_trips_text() {
        let source = r#"spec test

data name: "Alice"

rule greeting: "hello"
"#;
        let formatted =
            format_source(source, crate::parsing::source::SourceType::Volatile).unwrap();
        assert!(formatted.contains("\"Alice\""), "data text must be quoted");
        assert!(formatted.contains("\"hello\""), "rule text must be quoted");
    }

    #[test]
    fn test_format_source_preserves_percent() {
        let source = r#"spec test

data rate: 10 percent

rule tax: rate * 21%
"#;
        let formatted =
            format_source(source, crate::parsing::source::SourceType::Volatile).unwrap();
        assert!(
            formatted.contains("10%"),
            "data percent must use shorthand %, got: {}",
            formatted
        );
    }

    #[test]
    fn test_format_groups_data_preserving_order() {
        // Data are deliberately mixed: the formatter keeps all regular data together
        // in original order, aligned
        let source = r#"spec test

data income: number -> minimum 0
data filing_status: filing_status_type -> suggest "single"
data country: "NL"
data deductions: number -> minimum 0
data name: text

rule total: income
"#;
        let formatted =
            format_source(source, crate::parsing::source::SourceType::Volatile).unwrap();
        let data_section = formatted
            .split("rule total")
            .next()
            .unwrap()
            .split("spec test\n")
            .nth(1)
            .unwrap();
        let lines: Vec<&str> = data_section.lines().filter(|l| !l.is_empty()).collect();
        // Constrained rows: one blank line after each when more `data` follows.
        assert_eq!(lines[0], "data income: number");
        assert_eq!(lines[1], "  -> minimum 0");
        assert_eq!(lines[2], "data filing_status: filing_status_type");
        assert_eq!(lines[3], "  -> suggest \"single\"");
        assert_eq!(lines[4], "data country: \"NL\"");
        assert_eq!(lines[5], "data deductions: number");
        assert_eq!(lines[6], "  -> minimum 0");
        assert_eq!(lines[7], "data name: text");
    }

    #[test]
    fn test_format_groups_spec_refs_with_overrides() {
        let source = r#"spec test

uses order wholesale
  -> with quantity: 100
uses order retail
  -> with quantity: 5
data base_price: 50

rule total: base_price
"#;
        let formatted =
            format_source(source, crate::parsing::source::SourceType::Volatile).unwrap();
        let data_section = formatted
            .split("rule total")
            .next()
            .unwrap()
            .split("spec test\n")
            .nth(1)
            .unwrap();
        let lines: Vec<&str> = data_section.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(lines[0], "uses order wholesale");
        assert_eq!(lines[1], "  -> with quantity: 100");
        assert_eq!(lines[2], "uses order retail");
        assert_eq!(lines[3], "  -> with quantity: 5");
        assert_eq!(lines[4], "data base_price: 50");
    }

    #[test]
    fn test_format_groups_with_literals_under_each_uses() {
        let source = r#"spec test

uses x
  -> with name: "Ben"
uses y
  -> with age: 15

rule r: 1
"#;
        let formatted =
            format_source(source, crate::parsing::source::SourceType::Volatile).unwrap();
        let data_section = formatted
            .split("rule r")
            .next()
            .unwrap()
            .split("spec test\n")
            .nth(1)
            .unwrap();
        let lines: Vec<&str> = data_section.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(lines[0], "uses x");
        assert_eq!(lines[1], "  -> with name: \"Ben\"");
        assert_eq!(lines[2], "uses y");
        assert_eq!(lines[3], "  -> with age: 15");
    }

    #[test]
    fn test_format_source_weather_clothing_text_quoted() {
        let source = r#"spec weather_clothing

data clothing_style: text
  -> option "light"
  -> option "warm"

data temperature: number

rule clothing_layer: "light"
  unless temperature < 5 then "warm"
"#;
        let formatted =
            format_source(source, crate::parsing::source::SourceType::Volatile).unwrap();
        assert!(
            formatted.contains("\"light\""),
            "text in rule must be quoted, got: {}",
            formatted
        );
        assert!(
            formatted.contains("\"warm\""),
            "text in unless must be quoted, got: {}",
            formatted
        );
    }

    // NOTE: Default value type validation (e.g. rejecting "10 $$" as a number
    // default) is tested at the planning level in engine.rs, not here. The
    // formatter only parses — it does not validate types. Planning catches
    // invalid defaults for both primitives and named types.

    #[test]
    fn test_format_text_option_round_trips() {
        let source = r#"spec test

data status: text
  -> option "active"
  -> option "inactive"

data s: status

rule out: s
"#;
        let formatted =
            format_source(source, crate::parsing::source::SourceType::Volatile).unwrap();
        assert!(
            formatted.contains("option \"active\""),
            "text option must be quoted, got: {}",
            formatted
        );
        assert!(
            formatted.contains("option \"inactive\""),
            "text option must be quoted, got: {}",
            formatted
        );
        // Round-trip
        let reparsed = format_source(&formatted, crate::parsing::source::SourceType::Volatile);
        assert!(reparsed.is_ok(), "formatted output should re-parse");
    }

    #[test]
    fn test_format_help_round_trips() {
        let source = r#"spec test
data quantity: number -> help "Number of items to order"
rule total: quantity
"#;
        let formatted =
            format_source(source, crate::parsing::source::SourceType::Volatile).unwrap();
        assert!(
            formatted.contains("help \"Number of items to order\""),
            "help must be quoted, got: {}",
            formatted
        );
        // Round-trip
        let reparsed = format_source(&formatted, crate::parsing::source::SourceType::Volatile);
        assert!(reparsed.is_ok(), "formatted output should re-parse");
    }

    #[test]
    fn test_format_measure_type_def_round_trips() {
        let source = r#"spec test

data money: measure
  -> unit eur: 1.00
  -> unit usd: 0.91
  -> decimals 2
  -> minimum 0

data price: money

rule total: price
"#;
        let formatted =
            format_source(source, crate::parsing::source::SourceType::Volatile).unwrap();
        assert!(
            formatted.contains("unit eur: 1.00"),
            "measure unit should not be quoted, got: {}",
            formatted
        );
        // Round-trip
        let reparsed = format_source(&formatted, crate::parsing::source::SourceType::Volatile);
        assert!(
            reparsed.is_ok(),
            "formatted output should re-parse, got: {:?}",
            reparsed
        );
    }

    #[test]
    fn format_deprecated_unit_space_emits_assignment_colon() {
        let source = r#"spec test
uses lemma units

data money: measure
  -> unit eur: 1.00

data rate: measure
  -> unit eur_per_hour: eur/hour
"#;
        let formatted =
            format_source(source, crate::parsing::source::SourceType::Volatile).unwrap();
        assert!(
            formatted.contains("unit eur: 1.00"),
            "formatter must emit assignment colon for unit, got: {}",
            formatted
        );
        assert!(
            formatted.contains("unit eur_per_hour: eur/hour"),
            "formatter must emit assignment colon for compound unit, got: {}",
            formatted
        );
        assert!(
            !formatted.contains("unit eur 1.00"),
            "formatter must not emit deprecated space unit syntax, got: {}",
            formatted
        );
    }

    #[test]
    fn format_canonical_unit_colon_round_trips() {
        let source = r#"spec test
uses lemma units

data money: measure
  -> unit eur: 1.00

data rate: measure
  -> unit eur_per_hour: eur/hour
"#;
        let formatted =
            format_source(source, crate::parsing::source::SourceType::Volatile).unwrap();
        assert!(
            formatted.contains("unit eur: 1.00"),
            "canonical unit syntax must survive format, got: {}",
            formatted
        );
        let reparsed = format_source(&formatted, crate::parsing::source::SourceType::Volatile);
        assert!(
            reparsed.is_ok(),
            "formatted canonical unit syntax should re-parse, got: {:?}",
            reparsed
        );
    }

    #[test]
    fn test_format_expression_display_stable_round_trip() {
        let source = r#"spec test
data a: 1.00
rule r: a + 2.00 * 3
"#;
        let formatted =
            format_source(source, crate::parsing::source::SourceType::Volatile).unwrap();
        let again =
            format_source(&formatted, crate::parsing::source::SourceType::Volatile).unwrap();
        assert_eq!(
            formatted, again,
            "AST Display-based format must be idempotent under parse/format"
        );
    }

    #[test]
    fn test_format_past_future_range_no_duplicate_in() {
        let source = r#"spec test
data start: date
data length: duration
rule valid: start in past length
rule window: past length
"#;
        let formatted =
            format_source(source, crate::parsing::source::SourceType::Volatile).unwrap();
        assert!(
            formatted.contains("rule valid:\n  start in past length"),
            "RangeContainment+PastFutureRange must not emit duplicate 'in', got:\n{formatted}"
        );
        assert!(
            formatted.contains("rule window:\n  past length"),
            "bare PastFutureRange must print 'past' not 'in past', got:\n{formatted}"
        );
        assert!(
            !formatted.contains("in in past"),
            "must not contain duplicate 'in', got:\n{formatted}"
        );
    }

    #[test]
    fn test_format_rule_body_on_next_line() {
        let source = "spec test\nrule r: 1\n";
        let formatted =
            format_source(source, crate::parsing::source::SourceType::Volatile).unwrap();
        assert!(
            formatted.contains("rule r:\n  1\n"),
            "rule body must start on next line, got:\n{formatted}"
        );
    }

    #[test]
    fn test_format_blank_line_between_data_and_rules() {
        let source = "spec test\ndata x: 1\nrule r: x\n";
        let formatted =
            format_source(source, crate::parsing::source::SourceType::Volatile).unwrap();
        assert!(
            formatted.contains("data x: 1\n\n\nrule r:\n"),
            "two blank lines required between data block and rules block, got:\n{formatted:?}"
        );
    }

    #[test]
    fn test_format_rule_unless_single_line_when_short() {
        let source = r#"spec test
data a: number
data b: boolean

rule r: no
  unless a < 1 then yes
  unless b then yes
"#;
        let formatted =
            format_source(source, crate::parsing::source::SourceType::Volatile).unwrap();
        assert!(
            formatted.contains("unless a < 1 then yes")
                && formatted.contains("unless b     then yes"),
            "unless stays on one line when under MAX_COLS, got:\n{formatted}"
        );
    }

    #[test]
    fn test_format_rule_unless_child_premium_applies_inline_aligned() {
        let source = r#"spec child_premium_applies
data child_age_years: number
data is_male: boolean
data child_has_own_children: boolean
data child_is_oldest_insured: boolean

rule child_premium_applies:
  yes
  unless child_age_years >= 25 and is_male then no
  unless child_has_own_children then no
  unless child_is_oldest_insured then no
"#;
        let formatted =
            format_source(source, crate::parsing::source::SourceType::Volatile).unwrap();
        assert!(
            formatted.contains("unless child_age_years >= 25 and is_male then no")
                && formatted.contains("unless child_has_own_children            then no")
                && formatted.contains("unless child_is_oldest_insured           then no"),
            "all-single unless clauses align then, got:\n{formatted}"
        );
        let twice =
            format_source(&formatted, crate::parsing::source::SourceType::Volatile).unwrap();
        assert_eq!(formatted, twice);
    }

    #[test]
    fn test_format_rule_unless_can_request_reinstatement_and_wrap() {
        let source = r#"spec can_request_reinstatement
data days_since_policy_stopped: number
data arrears_paid: boolean
data surrender_value_repaid: boolean
data all_insured_persons_alive: boolean

rule can_request_reinstatement:
  no
  unless days_since_policy_stopped <= 365 and arrears_paid and surrender_value_repaid and all_insured_persons_alive then yes
"#;
        let formatted =
            format_source(source, crate::parsing::source::SourceType::Volatile).unwrap();
        assert!(
            formatted.contains(
                "unless days_since_policy_stopped <= 365\n    and arrears_paid\n    and surrender_value_repaid\n    and all_insured_persons_alive\n    then yes"
            ),
            "long and chain wraps one operand per line, got:\n{formatted}"
        );
        let twice =
            format_source(&formatted, crate::parsing::source::SourceType::Volatile).unwrap();
        assert_eq!(formatted, twice);
    }

    #[test]
    fn test_format_rule_unless_foreign_transport_uniform_split_then() {
        let source = r#"spec foreign_transport_covered
data death_location: text
data trip_duration_months: number
data negative_travel_advisory_at_departure: boolean
data left_area_asap_after_advisory: boolean

rule foreign_transport_covered:
  yes
  unless death_location is "abroad" then no
  unless death_location is "abroad" and trip_duration_months <= 2 then yes
  unless death_location is "abroad" and trip_duration_months <= 2 and negative_travel_advisory_at_departure then no
  unless death_location is "abroad" and trip_duration_months <= 2 and negative_travel_advisory_at_departure and left_area_asap_after_advisory then yes
"#;
        let formatted =
            format_source(source, crate::parsing::source::SourceType::Volatile).unwrap();
        assert!(
            formatted.contains("unless death_location is \"abroad\"\n    then no"),
            "short sister uses split then when any clause needs it, got:\n{formatted}"
        );
        assert!(
            formatted.contains(
                "unless death_location is \"abroad\"\n    and trip_duration_months <= 2\n    then yes"
            ),
            "wrapped and chain with split then, got:\n{formatted}"
        );
        let twice =
            format_source(&formatted, crate::parsing::source::SourceType::Volatile).unwrap();
        assert_eq!(formatted, twice);
    }

    #[test]
    fn test_format_rule_unless_child_auto_covered_service_only() {
        let source = r#"spec child_auto_covered_service_only
data days_since_birth: number
data birth_reported_within_60_days: boolean

rule child_auto_covered_service_only:
  yes
  unless days_since_birth >= 60 and not birth_reported_within_60_days then no
"#;
        let formatted =
            format_source(source, crate::parsing::source::SourceType::Volatile).unwrap();
        assert!(
            formatted.contains(
                "unless days_since_birth >= 60\n    and not birth_reported_within_60_days\n    then no"
            ),
            "multiline unless condition with split then, got:\n{formatted}"
        );
        let twice =
            format_source(&formatted, crate::parsing::source::SourceType::Volatile).unwrap();
        assert_eq!(formatted, twice);
    }
}

//! Lemma source code formatting.
//!
//! Formats parsed specs into canonical Lemma source text. Uses `AsLemmaSource`
//! and `Expression::Display` for syntax; this module handles layout only.
//! Canonical source includes ASCII-lowercase logical identifier names.

pub(crate) mod expression;

use crate::parsing::ast::{
    AsLemmaSource, Constraint, ContinuationShape, DataValue, Expression, LemmaData, LemmaRule,
    LemmaSpec, TypeConstraintCommand,
};
use crate::parsing::{parse, ParseResult};
use crate::{Error, ResourceLimits};
use expression::{display_broken, Syntax};

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
        out.push_str(&format!("{meta}\n"));
    }

    if !spec.data.is_empty() {
        format_sorted_data(&spec.data, &mut out, "", max_cols);
    }

    if !spec.rules.is_empty() {
        out.push('\n');
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
        DataValue::Import { .. } | DataValue::ImportList { .. } => false,
    }
}

fn format_with_rhs(rhs: &crate::parsing::ast::WithRhs) -> String {
    match rhs {
        crate::parsing::ast::WithRhs::Literal(v) => format!("{}", AsLemmaSource(v)),
        crate::parsing::ast::WithRhs::Reference { target } => target.to_string(),
    }
}

fn data_value_rhs_for_spec_body(
    value: &DataValue,
    continuation_prefix: &str,
    max_cols: usize,
) -> String {
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
                out.push_str(&format_constraint_block(
                    &row.command,
                    &row.args,
                    continuation_prefix,
                    max_cols,
                ));
            }
            out
        }
        DataValue::Definition { .. } => format!("{}", AsLemmaSource(value)),
        DataValue::Import { .. } | DataValue::ImportList { .. } => {
            unreachable!("BUG: data row right-hand side requested for an import")
        }
    }
}

fn format_constraint_block(
    command: &TypeConstraintCommand,
    args: &[crate::parsing::ast::CommandArg],
    continuation_prefix: &str,
    max_cols: usize,
) -> String {
    if command.continuation_shape() != ContinuationShape::CommaSeparated || args.len() < 2 {
        return format!(
            "{}-> {}",
            continuation_prefix,
            crate::parsing::ast::format_constraint_as_source(command, args)
        );
    }
    let items: Vec<String> = args
        .iter()
        .map(|arg| format!("{}", AsLemmaSource(arg)))
        .collect();
    let wrap_indent = format!("{continuation_prefix}    ");
    let mut lines: Vec<String> = Vec::new();
    let mut current = format!("{continuation_prefix}-> {command} {}", items[0]);
    for item in items.iter().skip(1) {
        let candidate = format!("{current}, {item}");
        if candidate.len() <= max_cols {
            current = candidate;
        } else {
            current.push(',');
            lines.push(current);
            current = format!("{wrap_indent}{item}");
        }
    }
    lines.push(current);
    lines.join("\n")
}

/// `data reference:` then `gap` spaces, then `rhs`.
fn push_data_line(out: &mut String, line_prefix: &str, reference: &str, gap: usize, rhs: &str) {
    out.push_str(line_prefix);
    out.push_str("data ");
    out.push_str(reference);
    out.push(':');
    out.push_str(&" ".repeat(gap));
    out.push_str(rhs);
    out.push('\n');
}

/// A run of single-line rows aligns its right-hand sides; a multi-line row stands alone.
fn emit_data_row_group(rows: &[&LemmaData], line_prefix: &str, max_cols: usize, out: &mut String) {
    let continuation = format!("{line_prefix}{DATA_CONSTRAINT_INDENT}");
    let formatted: Vec<(String, String)> = rows
        .iter()
        .map(|row| {
            (
                row.reference.to_string(),
                data_value_rhs_for_spec_body(&row.value, &continuation, max_cols),
            )
        })
        .collect();
    let mut i = 0;
    while i < rows.len() {
        let mut run_end = i;
        while run_end < rows.len() && !formatted[run_end].1.contains('\n') {
            run_end += 1;
        }
        if run_end == i {
            let (reference, rhs) = &formatted[i];
            push_data_line(out, line_prefix, reference, 1, rhs);
            if data_value_has_arrow_constraints(&rows[i].value) && i + 1 < rows.len() {
                out.push('\n');
            }
            i += 1;
            continue;
        }
        let width = formatted[i..run_end]
            .iter()
            .map(|(reference, _)| reference.len())
            .max()
            .expect("BUG: non-empty run");
        for (reference, rhs) in &formatted[i..run_end] {
            push_data_line(
                out,
                line_prefix,
                reference,
                1 + width - reference.len(),
                rhs,
            );
        }
        i = run_end;
    }
}

fn format_import_header(data: &LemmaData) -> String {
    let alias = &data.reference.name;
    let (spec_ref, suffix) = match &data.value {
        DataValue::Import { spec_ref, .. } => (spec_ref, ""),
        DataValue::ImportList { spec_ref, .. } => (spec_ref, " list"),
        DataValue::Definition { .. } => {
            unreachable!("BUG: format_import_header called on non-Import data")
        }
    };
    let spec_name = &spec_ref.name;
    if alias == spec_name {
        format!("uses {spec_ref}{suffix}")
    } else {
        format!("uses {alias}: {spec_ref}{suffix}")
    }
}

/// `name: value` strings of one inline row, before alignment.
fn uses_row_fields(bindings: &[crate::parsing::ast::UsesBinding]) -> Vec<(String, String)> {
    bindings
        .iter()
        .map(|binding| (binding.path.to_string(), format_with_rhs(&binding.rhs)))
        .collect()
}

/// `-> key` first, then rows in source order. Inline rows with the same field order align
/// their columns; a row too long for one line puts one field per line.
fn format_uses_list_block(data: &LemmaData, line_prefix: &str, max_cols: usize) -> String {
    use crate::parsing::ast::UsesListRow;
    let mut out = format_import_header(data);
    let DataValue::ImportList { list, .. } = &data.value else {
        unreachable!("BUG: format_uses_list_block called on non-list import");
    };
    let command_prefix = format!("{line_prefix}{DATA_CONSTRAINT_INDENT}-> ");
    if let Some(key) = &list.key {
        let fields: Vec<String> = key.fields.iter().map(ToString::to_string).collect();
        out.push('\n');
        out.push_str(&command_prefix);
        out.push_str("key ");
        out.push_str(&fields.join(", "));
    }

    let field_rows: Vec<Vec<(String, String)>> = list
        .rows
        .iter()
        .filter_map(|row| match row {
            UsesListRow::Fields { bindings, .. } => Some(uses_row_fields(bindings)),
            UsesListRow::Values { .. } => None,
        })
        .collect();
    let shared_names: Option<Vec<&str>> = field_rows
        .first()
        .map(|first| first.iter().map(|(name, _)| name.as_str()).collect());
    let aligned = shared_names.as_ref().is_some_and(|names| {
        field_rows.iter().all(|row| {
            row.len() == names.len()
                && row
                    .iter()
                    .zip(names.iter())
                    .all(|((name, _), shared)| name == shared)
        })
    });
    let column_widths: Vec<usize> = if aligned {
        let columns = shared_names.as_ref().map_or(0, Vec::len);
        (0..columns)
            .map(|column| {
                field_rows
                    .iter()
                    .map(|row| row[column].0.len() + 2 + row[column].1.len() + 1)
                    .max()
                    .expect("BUG: aligned rows are non-empty")
            })
            .collect()
    } else {
        Vec::new()
    };

    let vertical_indent = format!("{line_prefix}{DATA_CONSTRAINT_INDENT}     ");
    for row in &list.rows {
        out.push('\n');
        match row {
            UsesListRow::Fields { bindings, .. } => {
                let fields = uses_row_fields(bindings);
                let mut inline = String::new();
                for (index, (name, value)) in fields.iter().enumerate() {
                    let last = index + 1 == fields.len();
                    let cell = if last {
                        format!("{name}: {value}")
                    } else {
                        format!("{name}: {value},")
                    };
                    inline.push_str(&cell);
                    if !last {
                        let width = column_widths.get(index).copied().unwrap_or(cell.len());
                        inline.push_str(&" ".repeat(width.saturating_sub(cell.len()) + 1));
                    }
                }
                let line = format!("{command_prefix}with {inline}");
                if line.len() <= max_cols {
                    out.push_str(&line);
                } else {
                    out.push_str(&command_prefix);
                    out.push_str("with");
                    for (index, (name, value)) in fields.iter().enumerate() {
                        out.push('\n');
                        out.push_str(&vertical_indent);
                        out.push_str(&format!("{name}: {value}"));
                        if index + 1 < fields.len() {
                            out.push(',');
                        }
                    }
                }
            }
            UsesListRow::Values { values, .. } => {
                let items: Vec<String> = values.iter().map(format_with_rhs).collect();
                out.push_str(&command_prefix);
                out.push_str("with ");
                out.push_str(&items.join(", "));
            }
        }
    }
    out
}

fn format_uses_block(data: &LemmaData, line_prefix: &str, max_cols: usize) -> String {
    if matches!(&data.value, DataValue::ImportList { .. }) {
        return format_uses_list_block(data, line_prefix, max_cols);
    }
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
fn format_sorted_data(data: &[LemmaData], out: &mut String, line_prefix: &str, max_cols: usize) {
    let mut regular: Vec<&LemmaData> = Vec::new();
    let mut imports: Vec<&LemmaData> = Vec::new();

    for data in data {
        if data.value.uses_spec_ref().is_some() {
            imports.push(data);
        } else {
            regular.push(data);
        }
    }

    let emit_group = |rows: &[&LemmaData], out: &mut String| {
        emit_data_row_group(rows, line_prefix, max_cols, out)
    };

    if !imports.is_empty() {
        out.push('\n');

        for (i, row) in imports.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(line_prefix);
            out.push_str(&format_uses_block(row, line_prefix, max_cols));
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
    let body = format_expr_wrapped(&rule.expression, max_cols, expr_indent);
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
        let condition =
            format_expr_wrapped(&unless_clause.condition, unless_condition_budget, "    ");
        let result = format_expr_wrapped(&unless_clause.result, max_cols, "    ");
        clauses.push((condition, result));
    }

    let clause_needs_split_then = |condition: &str, result: &str| {
        condition.contains('\n')
            || result.contains('\n')
            || naive_single_len(condition, result) > max_cols
    };

    let any_split = clauses.iter().any(|(c, r)| clause_needs_split_then(c, r));

    const SPLIT_THEN_INDENT_SPACES: usize = 4;

    let mut singles: Vec<usize> = if any_split {
        Vec::new()
    } else {
        (0..clauses.len()).collect()
    };

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
        out.push('\n');
        out.push_str(UNLESS_LINE_PREFIX);
        out.push_str(condition);
        if singles.contains(&i) {
            let max_end = align_max_end.expect("BUG: singles.contains but align_max_end empty");
            out.push_str(&" ".repeat(1 + max_end.saturating_sub(pl + condition.len())));
        } else {
            out.push('\n');
            out.push_str(&" ".repeat(SPLIT_THEN_INDENT_SPACES));
        }
        out.push_str("then ");
        out.push_str(result);
    }
    out.push('\n');
    out
}

// =============================================================================
// Expression wrapping (soft line breaking at max_cols)
// =============================================================================

/// The expression on one line when it fits `max_cols`; otherwise every top-level `and` /
/// arithmetic operator starts a new line at `indent`. Operands always print on one line.
fn format_expr_wrapped(expr: &Expression, max_cols: usize, indent: &str) -> String {
    let flat = Syntax(expr.root()).to_string();
    if flat.len() <= max_cols {
        flat
    } else {
        display_broken(expr.root(), indent)
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
            timezone: Some(TimezoneValue { offset_minutes: 0 }),

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
            formatted.contains("data x: 1\n\nrule r:\n")
                && !formatted.contains("data x: 1\n\n\nrule r:\n"),
            "one blank line required between data block and rules block, got:\n{formatted:?}"
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

    #[test]
    fn test_format_long_arithmetic_chain_breaks_before_every_operator() {
        let source = r#"spec test
data first_amount: number
data second_amount: number
data third_amount: number
data fourth_amount: number
data fifth_amount: number
data sixth_amount: number

rule total: first_amount + second_amount + third_amount + fourth_amount + fifth_amount + sixth_amount
"#;
        let formatted =
            format_source(source, crate::parsing::source::SourceType::Volatile).unwrap();
        assert!(
            formatted.contains(
                "rule total:\n  first_amount\n  + second_amount\n  + third_amount\n  + fourth_amount\n  + fifth_amount\n  + sixth_amount\n"
            ),
            "overlong chain breaks before every operator, got:\n{formatted}"
        );
        let twice =
            format_source(&formatted, crate::parsing::source::SourceType::Volatile).unwrap();
        assert_eq!(formatted, twice);
    }

    #[test]
    fn test_format_broken_chain_keeps_nested_chain_operand_flat() {
        let source = r#"spec test
data base_amount_value: number
data first_amount: number
data second_amount: number
data third_amount: number
data other_amount_value: number

rule total: base_amount_value * (first_amount + second_amount + third_amount) + other_amount_value
"#;
        let formatted =
            format_source(source, crate::parsing::source::SourceType::Volatile).unwrap();
        assert!(
            formatted.contains(
                "rule total:\n  base_amount_value * (first_amount + second_amount + third_amount)\n  + other_amount_value\n"
            ),
            "only the root chain breaks, got:\n{formatted}"
        );
        let twice =
            format_source(&formatted, crate::parsing::source::SourceType::Volatile).unwrap();
        assert_eq!(formatted, twice);
    }

    #[test]
    fn test_format_constrained_data_row_stands_alone_between_aligned_runs() {
        let source = r#"spec test
data a: 1
data longer_name: 2
data limit: number -> minimum 0
data b: 3
"#;
        let formatted =
            format_source(source, crate::parsing::source::SourceType::Volatile).unwrap();
        assert!(
            formatted.contains(
                "data a:           1\ndata longer_name: 2\ndata limit: number\n  -> minimum 0\n\ndata b: 3\n"
            ),
            "got:\n{formatted}"
        );
    }
}

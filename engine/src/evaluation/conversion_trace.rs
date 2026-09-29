//! Structured explanation steps for unit conversions (`as`).

use std::cmp::Ordering;
use std::sync::Arc;

use crate::computation::rational::{checked_div, RationalInteger};
use crate::planning::explanation::{ConversionTraceRole, SerializedConversionTraceStep};
use crate::planning::semantics::{
    compare_semantic_dates, LemmaType, LiteralValue, SemanticConversionTarget, TypeSpecification,
    ValueKind,
};

/// Build ordered explanation steps (outcome → rule) after a successful unit conversion.
///
/// When the source and target unit are the same (identity), the Rule step is omitted.
pub(crate) fn build_conversion_steps(
    value: &LiteralValue,
    value_type: &Arc<LemmaType>,
    target: &SemanticConversionTarget,
    result: &LiteralValue,
    result_type: &Arc<LemmaType>,
) -> Vec<SerializedConversionTraceStep> {
    let mut steps = Vec::new();
    steps.push(SerializedConversionTraceStep {
        role: ConversionTraceRole::Outcome,
        text: result.display_value_with_type(result_type.as_ref()),
    });

    if let Some(rule_text) = conversion_rule_step_text(value, value_type, target, result) {
        steps.push(SerializedConversionTraceStep {
            role: ConversionTraceRole::Rule,
            text: rule_text,
        });
    }

    steps
}

fn conversion_rule_step_text(
    value: &LiteralValue,
    value_type: &LemmaType,
    target: &SemanticConversionTarget,
    result: &LiteralValue,
) -> Option<String> {
    match &value.value {
        ValueKind::Range(left, right) => range_span_rule_step_text(left, right, result),
        ValueKind::Measure(_) if !value_type.is_calendar_like() => match target {
            SemanticConversionTarget::Unit {
                unit_name,
                owning_type,
            } => {
                let from_signature = value_type.measure_runtime_signature();
                measure_unit_equivalence_step_text(&from_signature, unit_name, owning_type)
            }
            _ => None,
        },
        ValueKind::Number(_) | ValueKind::Ratio(_) => None,
        ValueKind::Measure(_) if value_type.is_calendar_like() => None,
        _ => None,
    }
}

fn range_span_rule_step_text(
    left: &crate::planning::semantics::TypedLiteral,
    right: &crate::planning::semantics::TypedLiteral,
    result: &LiteralValue,
) -> Option<String> {
    match (&left.value, &right.value) {
        (ValueKind::Date(left_date), ValueKind::Date(right_date)) => {
            let (lower, upper) = ordered_date_pair(left_date, right_date);
            let lower_literal = LiteralValue::date(lower.clone());
            let upper_literal = LiteralValue::date(upper.clone());
            Some(format!(
                "{} − {} = {}",
                upper_literal.display_value(),
                lower_literal.display_value(),
                result.display_value()
            ))
        }
        (ValueKind::Number(_), ValueKind::Number(_)) => {
            let left_lit = left.to_literal();
            let right_lit = right.to_literal();
            let (lower, upper) = ordered_number_pair(&left_lit, &right_lit);
            Some(format!(
                "{} − {} = {}",
                upper.display_value(),
                lower.display_value(),
                result.display_value()
            ))
        }
        (ValueKind::Measure(_), ValueKind::Measure(_)) => {
            let left_lit = left.to_literal();
            let right_lit = right.to_literal();
            let (lower, upper) = ordered_measure_pair(&left_lit, &right_lit);
            Some(format!(
                "{} − {} = {}",
                upper.display_value(),
                lower.display_value(),
                result.display_value()
            ))
        }
        _ => None,
    }
}

fn ordered_date_pair<'a>(
    left: &'a crate::planning::semantics::SemanticDateTime,
    right: &'a crate::planning::semantics::SemanticDateTime,
) -> (
    &'a crate::planning::semantics::SemanticDateTime,
    &'a crate::planning::semantics::SemanticDateTime,
) {
    match compare_semantic_dates(left, right) {
        Ordering::Less | Ordering::Equal => (left, right),
        Ordering::Greater => (right, left),
    }
}

fn ordered_number_pair<'a>(
    left: &'a LiteralValue,
    right: &'a LiteralValue,
) -> (&'a LiteralValue, &'a LiteralValue) {
    let ValueKind::Number(left_number) = &left.value else {
        unreachable!("BUG: ordered_number_pair called with non-number operand");
    };
    let ValueKind::Number(right_number) = &right.value else {
        unreachable!("BUG: ordered_number_pair called with non-number operand");
    };
    match left_number.try_cmp(right_number) {
        Ok(Ordering::Greater) => (right, left),
        Ok(Ordering::Less | Ordering::Equal) => (left, right),
        Err(_) => unreachable!("BUG: range endpoint compare after successful span conversion"),
    }
}

fn ordered_measure_pair<'a>(
    left: &'a LiteralValue,
    right: &'a LiteralValue,
) -> (&'a LiteralValue, &'a LiteralValue) {
    let ValueKind::Measure(left_magnitude) = &left.value else {
        unreachable!("BUG: ordered_measure_pair called with non-measure operand");
    };
    let ValueKind::Measure(right_magnitude) = &right.value else {
        unreachable!("BUG: ordered_measure_pair called with non-measure operand");
    };
    match left_magnitude.try_cmp(right_magnitude) {
        Ok(Ordering::Greater) => (right, left),
        Ok(Ordering::Less | Ordering::Equal) => (left, right),
        Err(_) => unreachable!("BUG: range endpoint compare after successful span conversion"),
    }
}

fn format_explanation_multiplier(rational: &RationalInteger) -> String {
    rational.display_str()
}

/// Produce the "1 {source_unit} is {multiplier} {target_unit}" Rule step text.
///
/// Same-family conversions only: when `from_unit` is absent from `owning_type`
/// (cross-family relabel), there is no factor equivalence to narrate.
fn measure_unit_equivalence_step_text(
    from_signature: &[(String, i32)],
    to_unit: &str,
    owning_type: &LemmaType,
) -> Option<String> {
    let from_unit = from_signature
        .first()
        .map(|(name, _)| name.as_str())
        .unwrap_or("");
    if from_unit.is_empty() || from_signature.len() != 1 {
        return None;
    }

    let units = match &owning_type.specifications {
        TypeSpecification::Measure { units, .. }
        | TypeSpecification::MeasureRange { units, .. } => units,
        _ => return None,
    };
    let from_factor = &units.get(from_unit).ok()?.factor;
    let to_factor = &units.get(to_unit).ok()?.factor;
    let multiplier = checked_div(from_factor, to_factor).ok()?;
    let multiplier_display = format_explanation_multiplier(&multiplier);
    if multiplier_display == "1" {
        return None;
    }
    Some(format!("1 {from_unit} is {multiplier_display} {to_unit}"))
}

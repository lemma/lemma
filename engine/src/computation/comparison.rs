//! Type-aware comparison operations

use std::sync::Arc;

use crate::computation::operation_result::{OperationResult, VetoType};
use crate::computation::rational::RationalInteger;
use crate::planning::semantics::{ComparisonComputation, LemmaType, LiteralValue, ValueKind};

/// Perform type-aware comparison, returning OperationResult (Veto on error).
///
/// Operates on [`ValueKind`] magnitudes; written-unit bindings are irrelevant to ordering.
pub fn comparison_operation(
    left: &ValueKind,
    left_type: &Arc<LemmaType>,
    op: &ComparisonComputation,
    right: &ValueKind,
    right_type: &Arc<LemmaType>,
) -> OperationResult {
    match (left, right) {
        (ValueKind::Range(range_left, range_right), ValueKind::Measure(_))
            if left_type.is_date_range() && right_type.is_calendar_like() =>
        {
            let (ValueKind::Date(left_date), ValueKind::Date(right_date)) =
                (&range_left.value, &range_right.value)
            else {
                unreachable!(
                    "BUG: date range calendar comparison received non-date endpoints; planning should have rejected this"
                );
            };
            let calendar_unit =
                crate::planning::semantics::semantic_calendar_unit_from_measure_type(right_type);
            let measure = super::datetime::compute_date_calendar_difference(
                left_date,
                right_date,
                &calendar_unit,
                Arc::clone(right_type),
            );
            compare_with_operation_result(measure, right_type, op, right, right_type)
        }

        (ValueKind::Measure(_), ValueKind::Range(range_left, range_right))
            if right_type.is_date_range() && left_type.is_calendar_like() =>
        {
            let (ValueKind::Date(left_date), ValueKind::Date(right_date)) =
                (&range_left.value, &range_right.value)
            else {
                unreachable!(
                    "BUG: date range calendar comparison received non-date endpoints; planning should have rejected this"
                );
            };
            let calendar_unit =
                crate::planning::semantics::semantic_calendar_unit_from_measure_type(left_type);
            let measure = super::datetime::compute_date_calendar_difference(
                left_date,
                right_date,
                &calendar_unit,
                Arc::clone(left_type),
            );
            compare_with_right_result(left, left_type, op, measure, left_type)
        }

        (ValueKind::Range(range_left, range_right), _) => {
            let endpoint_type = range_endpoint_type_for_runtime_span(left_type);
            let measure = super::range::compute_span(
                range_left.as_ref(),
                &endpoint_type,
                range_right.as_ref(),
                &endpoint_type,
            );
            // Prefer the same endpoint type used for the span (includes injected
            // decomposition for anonymous measure ranges).
            let span_type = if left_type.is_date_range() || left_type.is_time_range() {
                super::range::span_result_type(left_type)
            } else {
                Arc::clone(&endpoint_type)
            };
            compare_with_operation_result(measure, &span_type, op, right, right_type)
        }

        (ValueKind::Number(l), ValueKind::Number(r)) => compare_stored_rationals(l, op, r),

        (ValueKind::Boolean(l), ValueKind::Boolean(r)) => match op {
            ComparisonComputation::Is => {
                OperationResult::from_literal(LiteralValue::from_bool(l == r))
            }
            ComparisonComputation::IsNot => {
                OperationResult::from_literal(LiteralValue::from_bool(l != r))
            }
            _ => unreachable!(
                "BUG: invalid boolean comparison operator {}; this should be rejected during planning",
                op
            ),
        },

        (ValueKind::Text(l), ValueKind::Text(r)) => match op {
            ComparisonComputation::Is => {
                OperationResult::from_literal(LiteralValue::from_bool(l == r))
            }
            ComparisonComputation::IsNot => {
                OperationResult::from_literal(LiteralValue::from_bool(l != r))
            }
            _ => unreachable!(
                "BUG: invalid text comparison operator {}; this should be rejected during planning",
                op
            ),
        },

        (ValueKind::Ratio(l), ValueKind::Ratio(r)) => compare_stored_rationals(l, op, r),
        (ValueKind::Measure(left_value), ValueKind::Measure(right_value))
            if left_type.is_calendar_like() && right_type.is_calendar_like() =>
        {
            compare_stored_rationals(left_value, op, right_value)
        }

        (ValueKind::Measure(l), ValueKind::Measure(r)) => {
            let identical = left_type.as_ref() == right_type.as_ref()
                || (left_type.specifications == right_type.specifications
                    && left_type.name == right_type.name
                    && left_type.extends == right_type.extends);
            let same_family = left_type.same_measure_family(right_type);
            let anonymous_compatible = left_type.compatible_with_anonymous_measure(right_type);
            let same_decomp = match (
                left_type.measure_type_decomposition(),
                right_type.measure_type_decomposition(),
            ) {
                (Some(a), Some(b)) => a == b,
                _ => false,
            };
            if !identical && !same_family && !anonymous_compatible && !same_decomp {
                unreachable!(
                    "BUG: compared incompatible measure types ({} vs {}); planning must reject this",
                    left_type.name(),
                    right_type.name()
                );
            }
            compare_stored_rationals(l, op, r)
        }

        (ValueKind::Date(_), ValueKind::Date(_)) => {
            let left_lit = LiteralValue {
                value: left.clone(),
            };
            let right_lit = LiteralValue {
                value: right.clone(),
            };
            super::datetime::datetime_comparison(
                &left_lit, left_type, op, &right_lit, right_type,
            )
        }
        (ValueKind::Time(_), ValueKind::Time(_)) => {
            let left_lit = LiteralValue {
                value: left.clone(),
            };
            let right_lit = LiteralValue {
                value: right.clone(),
            };
            super::datetime::time_comparison(&left_lit, left_type, op, &right_lit, right_type)
        }

        (ValueKind::Measure(value), ValueKind::Number(n))
            if left_type.is_duration_like_measure() =>
        {
            compare_stored_rationals(value, op, n)
        }
        (ValueKind::Number(n), ValueKind::Measure(value))
            if right_type.is_duration_like_measure() =>
        {
            compare_stored_rationals(n, op, value)
        }
        (ValueKind::Measure(value), ValueKind::Number(n)) if left_type.is_calendar_like() => {
            compare_stored_rationals(value, op, n)
        }
        (ValueKind::Number(n), ValueKind::Measure(value)) if right_type.is_calendar_like() => {
            compare_stored_rationals(n, op, value)
        }

        _ => unreachable!(
            "BUG: unsupported comparison during evaluation: {} {} {}",
            left_type.name(),
            op,
            right_type.name()
        ),
    }
}

fn compare_stored_rationals(
    left: &RationalInteger,
    op: &ComparisonComputation,
    right: &RationalInteger,
) -> OperationResult {
    let ordering = match left.try_cmp(right) {
        Ok(ordering) => ordering,
        Err(failure) => {
            return OperationResult::Veto(VetoType::computation(failure.to_string()));
        }
    };
    let result = match op {
        ComparisonComputation::GreaterThan => ordering == std::cmp::Ordering::Greater,
        ComparisonComputation::LessThan => ordering == std::cmp::Ordering::Less,
        ComparisonComputation::GreaterThanOrEqual => ordering != std::cmp::Ordering::Less,
        ComparisonComputation::LessThanOrEqual => ordering != std::cmp::Ordering::Greater,
        ComparisonComputation::Is => ordering == std::cmp::Ordering::Equal,
        ComparisonComputation::IsNot => ordering != std::cmp::Ordering::Equal,
    };
    OperationResult::from_literal(LiteralValue::from_bool(result))
}

fn compare_with_operation_result(
    left_result: OperationResult,
    left_type: &Arc<LemmaType>,
    op: &ComparisonComputation,
    right: &ValueKind,
    right_type: &Arc<LemmaType>,
) -> OperationResult {
    let left_value = match left_result {
        OperationResult::Value(value) => value,
        OperationResult::Veto(reason) => return OperationResult::Veto(reason),
    };
    comparison_operation(&left_value.value, left_type, op, right, right_type)
}

/// Endpoint type for runtime range span, with decomposition filled when units exist
/// but planning left decomposition unset (anonymous measure ranges).
fn range_endpoint_type_for_runtime_span(range_type: &LemmaType) -> Arc<LemmaType> {
    let mut element_spec = match range_type.specifications.element_from_range() {
        Some(spec) => spec,
        None => return Arc::new(range_type.clone()),
    };
    if let crate::planning::semantics::TypeSpecification::Measure {
        units,
        decomposition,
        ..
    } = &mut element_spec
    {
        if decomposition.is_none() && !units.0.is_empty() {
            *decomposition = Some([(range_type.name(), 1i32)].into_iter().collect());
        }
    }
    Arc::new(LemmaType::primitive(element_spec))
}

fn compare_with_right_result(
    left: &ValueKind,
    left_type: &Arc<LemmaType>,
    op: &ComparisonComputation,
    right_result: OperationResult,
    right_type: &Arc<LemmaType>,
) -> OperationResult {
    let right_value = match right_result {
        OperationResult::Value(value) => value,
        OperationResult::Veto(reason) => return OperationResult::Veto(reason),
    };
    comparison_operation(left, left_type, op, &right_value.value, right_type)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computation::rational::rational_new;
    use crate::planning::semantics::{primitive_number_arc, ComparisonComputation, LiteralValue};

    fn eval_bool(left: &ValueKind, op: &ComparisonComputation, right: &ValueKind) -> bool {
        let number_ty = primitive_number_arc();
        let OperationResult::Value(lit) =
            comparison_operation(left, number_ty, op, right, number_ty)
        else {
            panic!("expected boolean value");
        };
        match &lit.value {
            ValueKind::Boolean(b) => *b,
            other => panic!("expected boolean, got {other:?}"),
        }
    }

    #[test]
    fn number_greater_than() {
        let left = LiteralValue::number(rational_new(5, 1)).value;
        let right = LiteralValue::number(rational_new(3, 1)).value;
        assert!(eval_bool(
            &left,
            &ComparisonComputation::GreaterThan,
            &right
        ));
        assert!(!eval_bool(
            &right,
            &ComparisonComputation::GreaterThan,
            &left
        ));
    }
}

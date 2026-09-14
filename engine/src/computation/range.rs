use std::sync::Arc;

use crate::computation::arithmetic::SignatureIndex;
use crate::computation::operation_result::{OperationResult, VetoType};
use crate::computation::rational::{rational_abs, rational_zero, RationalInteger};
use crate::planning::semantics::{
    ArithmeticComputation, ComparisonComputation, LemmaType, LiteralValue, ValueKind,
};

pub fn compute_span(
    left: &LiteralValue,
    left_type: &Arc<LemmaType>,
    right: &LiteralValue,
    right_type: &Arc<LemmaType>,
) -> OperationResult {
    let (signed, span_type) = compute_signed_span(left, left_type, right, right_type);
    absolute_span(signed, &span_type)
}

/// Result type of a range span derived from the range's LemmaType.
pub fn span_result_type(range_type: &LemmaType) -> Arc<LemmaType> {
    if range_type.is_date_range() || range_type.is_time_range() {
        return Arc::new(LemmaType::anonymous_for_decomposition(
            crate::planning::semantics::duration_decomposition(),
        ));
    }
    range_type
        .specifications
        .element_from_range()
        .map(|element| Arc::new(LemmaType::primitive(element)))
        .unwrap_or_else(|| Arc::new(range_type.clone()))
}

fn compute_signed_span(
    left: &LiteralValue,
    left_type: &Arc<LemmaType>,
    right: &LiteralValue,
    right_type: &Arc<LemmaType>,
) -> (OperationResult, Arc<LemmaType>) {
    match (&left.value, &right.value) {
        (ValueKind::Date(left_date), ValueKind::Date(right_date)) => {
            let left_chrono = match super::datetime::semantic_datetime_to_chrono(left_date) {
                Ok(d) => d,
                Err(msg) => {
                    return (
                        OperationResult::Veto(VetoType::computation(msg)),
                        Arc::clone(left_type),
                    )
                }
            };
            let right_chrono = match super::datetime::semantic_datetime_to_chrono(right_date) {
                Ok(d) => d,
                Err(msg) => {
                    return (
                        OperationResult::Veto(VetoType::computation(msg)),
                        Arc::clone(left_type),
                    )
                }
            };
            (
                compute_elapsed_duration_span(left_chrono, right_chrono),
                Arc::new(LemmaType::anonymous_for_decomposition(
                    crate::planning::semantics::duration_decomposition(),
                )),
            )
        }
        (ValueKind::Time(left_time), ValueKind::Time(right_time)) => {
            let left_chrono = match super::datetime::semantic_time_to_chrono_datetime(left_time) {
                Ok(d) => d,
                Err(msg) => {
                    return (
                        OperationResult::Veto(VetoType::computation(msg)),
                        Arc::clone(left_type),
                    )
                }
            };
            let right_chrono = match super::datetime::semantic_time_to_chrono_datetime(right_time) {
                Ok(d) => d,
                Err(msg) => {
                    return (
                        OperationResult::Veto(VetoType::computation(msg)),
                        Arc::clone(left_type),
                    )
                }
            };
            (
                compute_elapsed_duration_span(left_chrono, right_chrono),
                Arc::new(LemmaType::anonymous_for_decomposition(
                    crate::planning::semantics::duration_decomposition(),
                )),
            )
        }
        _ => {
            // Span computation only performs Subtract, which never resolves a signature_index
            // entry (the result type matches the operand family).
            let empty_unit_index = crate::planning::unit_index::UnitIndex::new();
            let empty_signature_index = SignatureIndex::new();
            (
                super::arithmetic_operation(
                    right,
                    right_type,
                    &ArithmeticComputation::Subtract,
                    left,
                    left_type,
                    &empty_unit_index,
                    &empty_signature_index,
                ),
                Arc::clone(left_type),
            )
        }
    }
}

fn absolute_span(span: OperationResult, span_type: &Arc<LemmaType>) -> OperationResult {
    let OperationResult::Value(literal) = span else {
        return span;
    };
    let magnitude = stored_magnitude(&literal);
    match magnitude.try_cmp(&rational_zero()) {
        Ok(std::cmp::Ordering::Less) => {}
        Ok(_) => return OperationResult::from_literal(literal),
        Err(e) => return OperationResult::Veto(VetoType::computation(e.to_string())),
    }
    let negated = match negate_stored_magnitude(&literal) {
        Ok(magnitude) => magnitude,
        Err(failure) => return OperationResult::Veto(VetoType::computation(failure.message())),
    };
    OperationResult::from_literal(rebuild_literal_with_magnitude(
        &literal,
        negated,
        span_type.as_ref(),
    ))
}

fn stored_magnitude(literal: &LiteralValue) -> RationalInteger {
    match &literal.value {
        ValueKind::Number(n) | ValueKind::Measure(n) | ValueKind::Ratio(n) => n.clone(),
        other => unreachable!(
            "BUG: range span must be number, measure, ratio, or calendar measure, got {other:?}"
        ),
    }
}

fn negate_stored_magnitude(
    literal: &LiteralValue,
) -> Result<RationalInteger, super::arithmetic::NumberArithmeticFailure> {
    let zero = rational_zero();
    let magnitude = stored_magnitude(literal);
    super::arithmetic::number_arithmetic(&zero, &ArithmeticComputation::Subtract, &magnitude)
}

fn rebuild_literal_with_magnitude(
    literal: &LiteralValue,
    magnitude: RationalInteger,
    lemma_type: &LemmaType,
) -> LiteralValue {
    match &literal.value {
        ValueKind::Number(_) => LiteralValue::number(magnitude),
        ValueKind::Measure(_) if lemma_type.is_calendar_like() => {
            let unit =
                crate::planning::semantics::semantic_calendar_unit_from_measure_type(lemma_type);
            LiteralValue::calendar_with_type(magnitude, unit, Arc::new(lemma_type.clone()))
        }
        ValueKind::Measure(_) => LiteralValue::measure(magnitude),
        ValueKind::Ratio(_) => LiteralValue::ratio(magnitude),
        other => unreachable!(
            "BUG: range span must be number, measure, ratio, or calendar measure, got {other:?}"
        ),
    }
}

fn compute_elapsed_duration_span(
    left_chrono: chrono::DateTime<chrono::FixedOffset>,
    right_chrono: chrono::DateTime<chrono::FixedOffset>,
) -> OperationResult {
    let duration = right_chrono - left_chrono;
    let second = match super::datetime::chrono_duration_to_rational_seconds(duration) {
        Ok(s) => s,
        Err(msg) => return OperationResult::Veto(VetoType::computation(msg)),
    };
    let second = match rational_abs(&second) {
        Ok(s) => s,
        Err(failure) => return OperationResult::Veto(VetoType::computation(failure.to_string())),
    };
    OperationResult::from_literal(LiteralValue::measure(second))
}

fn comparison_boolean_result(result: OperationResult, context: &str) -> Result<bool, VetoType> {
    match result {
        OperationResult::Value(literal) => match &literal.value {
            ValueKind::Boolean(value) => Ok(*value),
            other => {
                unreachable!("BUG: {context} expected boolean comparison result, got {other:?}")
            }
        },
        OperationResult::Veto(v) => Err(v),
    }
}

/// Half-open interval `[lo, hi)` where `lo` and `hi` are the ordered range endpoints.
/// Returns `OperationResult::from_literal(Boolean)` or propagates a Veto from inner comparisons.
pub fn check_containment(
    value: &LiteralValue,
    value_type: &Arc<LemmaType>,
    range_left: &LiteralValue,
    range_right: &LiteralValue,
    endpoint_type: &Arc<LemmaType>,
) -> OperationResult {
    let (lo, hi) = match comparison_boolean_result(
        super::comparison_operation(
            range_left,
            endpoint_type,
            &ComparisonComputation::LessThan,
            range_right,
            endpoint_type,
        ),
        "range endpoint ordering",
    ) {
        Ok(true) => (range_left, range_right),
        Ok(false) => (range_right, range_left),
        Err(v) => return OperationResult::Veto(v),
    };

    let lower_ok = match comparison_boolean_result(
        super::comparison_operation(
            value,
            value_type,
            &ComparisonComputation::GreaterThanOrEqual,
            lo,
            endpoint_type,
        ),
        "range containment lower bound",
    ) {
        Ok(b) => b,
        Err(v) => return OperationResult::Veto(v),
    };
    let upper_ok = match comparison_boolean_result(
        super::comparison_operation(
            value,
            value_type,
            &ComparisonComputation::LessThan,
            hi,
            endpoint_type,
        ),
        "range containment upper bound",
    ) {
        Ok(b) => b,
        Err(v) => return OperationResult::Veto(v),
    };

    OperationResult::from_literal(LiteralValue::from_bool(lower_ok && upper_ok))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computation::rational::rational_new;
    use crate::planning::semantics::{anonymous_measure_type, primitive_number_arc, LiteralValue};

    #[test]
    fn compute_span_is_absolute_for_reversed_number_range() {
        let five = LiteralValue::number(rational_new(5, 1));
        let three = LiteralValue::number(rational_new(3, 1));
        let number_ty = primitive_number_arc();
        let OperationResult::Value(span) = compute_span(&five, number_ty, &three, number_ty) else {
            panic!("expected span value");
        };
        match &span.value {
            ValueKind::Number(n) => assert_eq!(n, &rational_new(2, 1)),
            other => panic!("expected number span, got {other:?}"),
        }
    }

    fn assert_contained(value: &LiteralValue, left: &LiteralValue, right: &LiteralValue) -> bool {
        let number_ty = primitive_number_arc();
        match check_containment(value, number_ty, left, right, number_ty) {
            OperationResult::Value(lit) => match &lit.value {
                ValueKind::Boolean(b) => *b,
                other => panic!("expected Boolean, got {other:?}"),
            },
            OperationResult::Veto(v) => panic!("unexpected veto: {v:?}"),
        }
    }

    #[test]
    fn check_containment_half_open_and_reversed_number_range() {
        let three = LiteralValue::number(rational_new(3, 1));
        let four = LiteralValue::number(rational_new(4, 1));
        let five = LiteralValue::number(rational_new(5, 1));
        let two = LiteralValue::number(rational_new(2, 1));

        assert!(assert_contained(&three, &three, &five));
        assert!(!assert_contained(&five, &three, &five));
        assert!(!assert_contained(&two, &three, &five));
        assert!(!assert_contained(&five, &five, &five));
        assert!(assert_contained(&four, &five, &three));
    }

    #[test]
    fn rebuild_literal_with_magnitude_preserves_measure_kind() {
        let lemma_type = Arc::new(anonymous_measure_type());
        let original = LiteralValue::measure(rational_new(10, 1));
        let rebuilt =
            rebuild_literal_with_magnitude(&original, rational_new(99, 1), lemma_type.as_ref());
        match &rebuilt.value {
            ValueKind::Measure(n) => assert_eq!(n, &rational_new(99, 1)),
            other => panic!("expected Measure, got {:?}", other),
        }
    }
}

//! Shared evaluation helpers used by the tree evaluator.
//!
//! Domain failures (division by zero, etc.) surface as Veto, not Error.

use std::sync::Arc;

use crate::computation::measure_math::{
    apply_rational_magnitude_op, mathematical_computation_preserves_measure_magnitude,
    measure_magnitude_math,
};
use crate::computation::{OperationResult, VetoType};
use crate::planning::semantics::{
    BoundValueKind, LemmaType, LiteralValue, MathematicalComputation, ValueKind,
};

pub(crate) fn evaluate_mathematical_operator(
    op: &MathematicalComputation,
    value: &BoundValueKind,
    lemma_type: &Arc<LemmaType>,
) -> OperationResult {
    use crate::computation::decimal_math::{decimal_acos, decimal_asin, decimal_atan, decimal_tan};
    use rust_decimal::MathematicalOps;

    if matches!(&value.value, ValueKind::Measure(_))
        && mathematical_computation_preserves_measure_magnitude(op)
    {
        return measure_magnitude_math(op, value, lemma_type);
    }

    match &value.value {
        ValueKind::Number(stored_rational) => {
            use crate::computation::rational::decimal_to_rational;
            if mathematical_computation_preserves_measure_magnitude(op) {
                return match apply_rational_magnitude_op(op, stored_rational) {
                    Ok(rounded) => OperationResult::from_literal(LiteralValue::number(rounded)),
                    Err(failure) => OperationResult::Veto(VetoType::computation(failure.to_string())),
                };
            }
            let stored_decimal = match stored_rational.try_to_decimal() {
                Ok(decimal) => decimal,
                Err(crate::computation::rational::NumericFailure::Overflow) => {
                    return OperationResult::Veto(VetoType::computation(
                        "The calculated result exceeds the decimal value limit.",
                    ));
                }
                Err(failure) => {
                    return OperationResult::Veto(VetoType::computation(failure.to_string()));
                }
            };
            let decimal_result: Option<rust_decimal::Decimal> = match op {
                MathematicalComputation::Abs
                | MathematicalComputation::Floor
                | MathematicalComputation::Ceil
                | MathematicalComputation::Round => {
                    unreachable!("BUG: exact rational operators return before the Decimal path")
                }
                MathematicalComputation::Sqrt => stored_decimal.sqrt(),
                MathematicalComputation::Sin => stored_decimal.checked_sin(),
                MathematicalComputation::Cos => stored_decimal.checked_cos(),
                MathematicalComputation::Tan => decimal_tan(stored_decimal),
                MathematicalComputation::Log => stored_decimal.checked_ln(),
                MathematicalComputation::Exp => stored_decimal.checked_exp(),
                MathematicalComputation::Asin => decimal_asin(stored_decimal),
                MathematicalComputation::Acos => decimal_acos(stored_decimal),
                MathematicalComputation::Atan => Some(decimal_atan(stored_decimal)),
            };

            let rounded_decimal = match decimal_result {
                Some(rounded) => rounded,
                None => {
                    return OperationResult::Veto(VetoType::computation(
                        "This mathematical operation is undefined for the given input.",
                    ));
                }
            };

            let result_rational = decimal_to_rational(rounded_decimal)
                .expect("BUG: transcendental result must lift back to stored rational");
            let result_value = LiteralValue::number(result_rational);
            OperationResult::from_literal(result_value)
        }
        _ => unreachable!(
            "BUG: mathematical operator with non-number operand (type {}); planning should have rejected this",
            lemma_type.name()
        ),
    }
}

pub(crate) fn resolve_data_path_value(
    data_path: &crate::planning::semantics::DataPath,
    plan: &crate::planning::execution_plan::ExecutionPlan,
    context: &crate::evaluation::EvaluationContext,
) -> OperationResult {
    if let Some(slot) = context.data_slot(plan, data_path) {
        return slot.clone();
    }
    if let Some(crate::planning::semantics::ReferenceEnd::Rule(rule_path)) =
        plan.reference_ends.get(data_path)
    {
        panic!(
            "BUG: rule-target data path '{}' (→ rule '{}') reached evaluation; planning must inline these",
            data_path, rule_path.rule
        );
    }
    let target = plan.promptable_data_path(data_path).expect(
        "BUG: missing-data veto on non-promptable path; planning must not leave this unbound",
    );
    OperationResult::Veto(VetoType::missing_data(
        target.clone(),
        context.missing_data_suggestion(target),
    ))
}

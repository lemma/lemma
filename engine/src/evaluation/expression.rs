//! Shared evaluation helpers used by the tree evaluator.
//!
//! Domain failures (division by zero, etc.) surface as Veto, not Error.

use std::sync::Arc;

use crate::computation::measure_math::{
    apply_rational_number_op, magnitude_abs, mathematical_computation_applies_to_magnitude,
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

    match &value.value {
        ValueKind::Measure(_) | ValueKind::Ratio(_) => {
            assert!(
                mathematical_computation_applies_to_magnitude(op),
                "BUG: planning rejects '{op}' on {}",
                lemma_type.name()
            );
            magnitude_abs(value)
        }
        ValueKind::Number(stored_rational) => {
            use crate::computation::rational::decimal_to_rational;
            if let Some(exact) = apply_rational_number_op(op, stored_rational) {
                return match exact {
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
                MathematicalComputation::Abs => {
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
        return rule_bound_slot_value(data_path, context.rule_value(plan, rule_path).clone(), plan);
    }
    let target = plan.promptable_data_path(data_path).expect(
        "BUG: missing-data veto on non-promptable path; planning must not leave this unbound",
    );
    OperationResult::Veto(VetoType::missing_data(
        target.clone(),
        context.missing_data_suggestion(target),
    ))
}

/// Value of data slot `path` whose reference ends at a rule: the rule's value when it
/// satisfies the slot's declared type, else a veto naming the slot.
fn rule_bound_slot_value(
    path: &crate::planning::semantics::DataPath,
    value: OperationResult,
    plan: &crate::planning::execution_plan::ExecutionPlan,
) -> OperationResult {
    let OperationResult::Value(bound) = &value else {
        return value;
    };
    let Some(crate::planning::semantics::DataDefinition::Reference { resolved_type, .. }) =
        plan.data.get(path)
    else {
        panic!("BUG: rule-bound data path '{path}' is not a reference");
    };
    match crate::planning::execution_plan::validate_value_against_type(
        resolved_type.as_ref(),
        &bound.to_literal(),
        &plan.resolved_types.unit_index,
        None,
    ) {
        Ok(()) => value,
        Err(message) => OperationResult::Veto(VetoType::computation(format!(
            "Reference '{path}' violates its declared constraint: {message}"
        ))),
    }
}

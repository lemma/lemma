use std::sync::Arc;

use crate::computation::operation_result::{OperationResult, VetoType};
use crate::computation::rational::{
    checked_div, checked_mul, rational_abs, rational_ceil, rational_floor, rational_round,
    NumericFailure, RationalInteger,
};
use crate::parsing::ast::RoundingMode;
use crate::planning::semantics::{BoundValueKind, LemmaType, MathematicalComputation, ValueKind};

/// Whether prefix `op` applies to a measure or ratio. Only `abs` does: rounding a
/// measure or ratio needs a step (`price -> round 0.05 eur`).
pub fn mathematical_computation_applies_to_magnitude(op: &MathematicalComputation) -> bool {
    matches!(op, MathematicalComputation::Abs)
}

/// Exact rational result of `abs` on a number. Other prefix math ops use Decimal.
pub(crate) fn apply_rational_number_op(
    op: &MathematicalComputation,
    number: &RationalInteger,
) -> Option<Result<RationalInteger, NumericFailure>> {
    match op {
        MathematicalComputation::Abs => Some(rational_abs(number)),
        _ => None,
    }
}

/// Integer rounding of a number: `x -> round` / `-> floor` / `-> ceil` without a step.
pub fn integer_rounding(mode: RoundingMode, value: &BoundValueKind) -> OperationResult {
    let ValueKind::Number(magnitude) = &value.value else {
        unreachable!("BUG: planning allows `-> {mode}` without a step only on a number");
    };
    let rounded = match mode {
        RoundingMode::Round => rational_round(magnitude),
        RoundingMode::Floor => rational_floor(magnitude),
        RoundingMode::Ceil => rational_ceil(magnitude),
    };
    match rounded {
        Ok(rounded) => OperationResult::from_bound(BoundValueKind {
            value: ValueKind::Number(rounded),
            unit: None,
        }),
        Err(failure) => OperationResult::Veto(VetoType::computation(failure.to_string())),
    }
}

/// `abs` of a measure or ratio. The written unit binding is kept.
pub fn magnitude_abs(value: &BoundValueKind) -> OperationResult {
    let magnitude = match &value.value {
        ValueKind::Measure(magnitude) | ValueKind::Ratio(magnitude) => magnitude,
        other => unreachable!("BUG: magnitude_abs called with {other:?}"),
    };
    match rational_abs(magnitude) {
        Ok(magnitude) => OperationResult::from_bound(BoundValueKind {
            value: match &value.value {
                ValueKind::Measure(_) => ValueKind::Measure(magnitude),
                _ => ValueKind::Ratio(magnitude),
            },
            unit: value.unit.clone(),
        }),
        Err(failure) => OperationResult::Veto(VetoType::computation(failure.to_string())),
    }
}

/// Multiple of `step` nearest to `value` in direction `mode`. Both are canonical values of
/// one kind (number, ratio or measure), so the quotient is unit-free. The result is written
/// in the unit of `result_type`, the step's type.
pub fn step_rounding(
    mode: RoundingMode,
    value: &BoundValueKind,
    step: &BoundValueKind,
    result_type: &Arc<LemmaType>,
) -> OperationResult {
    let (magnitude, step_magnitude) = match (&value.value, &step.value) {
        (ValueKind::Number(magnitude), ValueKind::Number(step_magnitude))
        | (ValueKind::Ratio(magnitude), ValueKind::Ratio(step_magnitude))
        | (ValueKind::Measure(magnitude), ValueKind::Measure(step_magnitude)) => {
            (magnitude, step_magnitude)
        }
        (value, step) => {
            unreachable!("BUG: planning checks the step kind of `-> {mode}`: {value:?} to {step:?}")
        }
    };
    let rounded = checked_div(magnitude, step_magnitude)
        .and_then(|quotient| match mode {
            RoundingMode::Round => rational_round(&quotient),
            RoundingMode::Floor => rational_floor(&quotient),
            RoundingMode::Ceil => rational_ceil(&quotient),
        })
        .and_then(|multiple| checked_mul(&multiple, step_magnitude));
    let rounded = match rounded {
        Ok(rounded) => rounded,
        Err(failure) => return OperationResult::Veto(VetoType::computation(failure.to_string())),
    };
    OperationResult::from_bound(BoundValueKind {
        value: match &value.value {
            ValueKind::Number(_) => ValueKind::Number(rounded),
            ValueKind::Ratio(_) => ValueKind::Ratio(rounded),
            _ => ValueKind::Measure(rounded),
        },
        unit: result_type.unit().map(Arc::from),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computation::rational::rational_new;
    use crate::literals::{MeasureUnit, MeasureUnits};
    use crate::planning::semantics::{
        primitive_number_arc, primitive_ratio_arc, BaseMeasureVector, TypeExtends,
        TypeSpecification,
    };

    fn mass() -> LemmaType {
        let unit = |name: &str, factor| MeasureUnit {
            name: name.to_string(),
            scale: crate::literals::UnitScale::Literal(factor),
            derived_measure_factors: Vec::new(),
            decomposition: BaseMeasureVector::new(),
            minimum: None,
            maximum: None,
            suggestion_magnitude: None,
        };
        LemmaType::new(
            "mass".to_string(),
            TypeSpecification::Measure {
                minimum: None,
                maximum: None,
                units: MeasureUnits::from(vec![
                    unit("kilogram", rational_new(1, 1)),
                    unit("gram", rational_new(1, 1000)),
                ]),
                traits: Vec::new(),
                decomposition: None,
                unit: None,
                help: String::new(),
            },
            TypeExtends::Primitive,
        )
    }

    fn bound(result: OperationResult) -> BoundValueKind {
        let OperationResult::Value(value) = result else {
            panic!("expected a value, got {result:?}");
        };
        value
    }

    #[test]
    fn abs_on_a_measure_keeps_the_unit_binding() {
        let value = BoundValueKind {
            value: ValueKind::Measure(rational_new(-26, 10)),
            unit: Some(Arc::from("kilogram")),
        };
        let result = bound(magnitude_abs(&value));
        assert_eq!(result.value, ValueKind::Measure(rational_new(26, 10)));
        assert_eq!(result.unit.as_deref(), Some("kilogram"));
    }

    #[test]
    fn abs_on_a_ratio_needs_no_unit_binding() {
        let value = BoundValueKind::unbound(ValueKind::Ratio(rational_new(-1, 8)));
        let result = bound(magnitude_abs(&value));
        assert_eq!(result.value, ValueKind::Ratio(rational_new(1, 8)));
        assert_eq!(result.unit, None);
    }

    #[test]
    fn step_rounding_a_measure_writes_the_step_unit() {
        let value = BoundValueKind::unbound(ValueKind::Measure(rational_new(26_004, 10_000)));
        let step = BoundValueKind::unbound(ValueKind::Measure(rational_new(1, 1000)));
        let in_gram = Arc::new(mass().with_unit("gram"));
        let result = bound(step_rounding(RoundingMode::Round, &value, &step, &in_gram));
        assert_eq!(result.value, ValueKind::Measure(rational_new(2600, 1000)));
        assert_eq!(result.unit.as_deref(), Some("gram"));
    }

    #[test]
    fn step_rounding_a_ratio_rounds_to_multiples_of_the_step() {
        let value = BoundValueKind::unbound(ValueKind::Ratio(rational_new(1255, 10_000)));
        let step = BoundValueKind::unbound(ValueKind::Ratio(rational_new(1, 100)));
        let in_percent = Arc::new(primitive_ratio_arc().as_ref().clone().with_unit("percent"));
        let result = bound(step_rounding(
            RoundingMode::Floor,
            &value,
            &step,
            &in_percent,
        ));
        assert_eq!(result.value, ValueKind::Ratio(rational_new(12, 100)));
        assert_eq!(result.unit.as_deref(), Some("percent"));
    }

    #[test]
    fn step_rounding_a_number_has_no_unit() {
        let value = BoundValueKind::unbound(ValueKind::Number(rational_new(17, 1)));
        let step = BoundValueKind::unbound(ValueKind::Number(rational_new(5, 1)));
        let result = bound(step_rounding(
            RoundingMode::Ceil,
            &value,
            &step,
            primitive_number_arc(),
        ));
        assert_eq!(result.value, ValueKind::Number(rational_new(20, 1)));
        assert_eq!(result.unit, None);
    }

    #[test]
    #[should_panic(expected = "BUG")]
    fn step_rounding_across_kinds_is_a_bug() {
        let value = BoundValueKind::unbound(ValueKind::Number(rational_new(17, 1)));
        let step = BoundValueKind::unbound(ValueKind::Measure(rational_new(5, 1)));
        step_rounding(RoundingMode::Round, &value, &step, primitive_number_arc());
    }
}

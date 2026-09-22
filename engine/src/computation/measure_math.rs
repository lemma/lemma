use std::sync::Arc;

use crate::computation::operation_result::{OperationResult, VetoType};
use crate::planning::semantics::{BoundValueKind, LemmaType, MathematicalComputation, ValueKind};

pub fn mathematical_computation_preserves_measure_magnitude(op: &MathematicalComputation) -> bool {
    matches!(
        op,
        MathematicalComputation::Abs
            | MathematicalComputation::Ceil
            | MathematicalComputation::Floor
            | MathematicalComputation::Round
    )
}

fn apply_decimal_magnitude_op(
    op: &MathematicalComputation,
    magnitude: rust_decimal::Decimal,
) -> rust_decimal::Decimal {
    match op {
        MathematicalComputation::Abs => magnitude.abs(),
        MathematicalComputation::Floor => magnitude.floor(),
        MathematicalComputation::Ceil => magnitude.ceil(),
        MathematicalComputation::Round => magnitude.round(),
        _ => unreachable!("BUG: non-magnitude-preserving op passed to apply_decimal_magnitude_op"),
    }
}

pub fn measure_magnitude_math(
    op: &MathematicalComputation,
    value: &BoundValueKind,
    lemma_type: &Arc<LemmaType>,
) -> OperationResult {
    debug_assert!(mathematical_computation_preserves_measure_magnitude(op));

    let ValueKind::Measure(canonical_magnitude) = &value.value else {
        unreachable!("BUG: measure_magnitude_math called with non-measure value");
    };

    let signature = lemma_type.measure_runtime_signature();
    if signature.len() != 1 || signature[0].1 != 1 {
        return OperationResult::Veto(VetoType::computation(format!(
            "Cannot apply '{op}' to measure with compound unit; convert with `as <unit>` first"
        )));
    }
    let unit_name = signature[0].0.clone();

    let unit_factor = lemma_type.measure_unit_factor(&unit_name);
    let magnitude_in_unit =
        match crate::computation::rational::checked_div(canonical_magnitude, unit_factor) {
            Ok(magnitude) => magnitude,
            Err(failure) => {
                return OperationResult::Veto(VetoType::computation(failure.to_string()));
            }
        };

    let magnitude_decimal = match magnitude_in_unit.try_to_decimal() {
        Ok(decimal) => decimal,
        Err(crate::computation::rational::NumericFailure::Overflow) => {
            return OperationResult::Veto(VetoType::computation(
                "Calculated result exceeds decimal value limit",
            ));
        }
        Err(failure) => {
            return OperationResult::Veto(VetoType::computation(failure.to_string()));
        }
    };

    let rounded_decimal = apply_decimal_magnitude_op(op, magnitude_decimal);

    let rounded_rational = match crate::computation::rational::decimal_to_rational(rounded_decimal)
    {
        Ok(rational) => rational,
        Err(failure) => {
            return OperationResult::Veto(VetoType::computation(failure.to_string()));
        }
    };

    let new_canonical =
        match crate::computation::rational::checked_mul(&rounded_rational, unit_factor) {
            Ok(canonical) => canonical,
            Err(failure) => {
                return OperationResult::Veto(VetoType::computation(failure.to_string()));
            }
        };

    OperationResult::from_bound(BoundValueKind {
        value: ValueKind::Measure(new_canonical),
        measure_binding_unit: value.measure_binding_unit.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computation::rational::rational_new;
    use crate::planning::semantics::{BaseMeasureVector, LemmaType};

    #[test]
    fn compound_signature_vetoes() {
        let mut decomp = BaseMeasureVector::new();
        decomp.insert("meter".to_string(), 1);
        decomp.insert("second".to_string(), -1);
        let lemma_type = Arc::new(LemmaType::anonymous_for_decomposition(decomp));
        let value = BoundValueKind::unbound(ValueKind::Measure(rational_new(100, 1)));
        let result = measure_magnitude_math(&MathematicalComputation::Ceil, &value, &lemma_type);
        assert!(
            result.vetoed(),
            "compound signature must veto, got {:?}",
            result
        );
    }
}

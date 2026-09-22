//! Type casts (`as number`, `as text`, `as eur`, …).

use crate::computation::operation_result::OperationResult;
use crate::computation::rational::{
    checked_div, checked_mul, rational_new, rational_one, RationalInteger,
};
use crate::parsing::ast::PrimitiveKind;
use crate::planning::semantics::{
    calendar_unit_factor, primitive_number_arc, primitive_text_arc, BoundValueKind, LemmaType,
    LiteralValue, SemanticCalendarUnit, SemanticConversionTarget, TypeSpecification, ValueKind,
};
use std::sync::Arc;

/// Apply a type cast (`as <target>`).
pub fn convert_unit(
    value: &BoundValueKind,
    value_type: &LemmaType,
    target: &SemanticConversionTarget,
) -> OperationResult {
    match target {
        SemanticConversionTarget::Type(PrimitiveKind::Number) => {
            cast_to_number(&value.value, value_type)
        }
        SemanticConversionTarget::Type(PrimitiveKind::Text) => {
            OperationResult::from_literal(LiteralValue::text_with_type(
                value.to_literal().display_value_with_type(value_type),
                primitive_text_arc().clone(),
            ))
        }
        SemanticConversionTarget::Type(PrimitiveKind::Boolean) => {
            if value_type.is_boolean() {
                OperationResult::from_bound(value.clone())
            } else {
                unreachable!(
                    "BUG: boolean cast on non-boolean; planning should have rejected {:?}",
                    value_type.name()
                );
            }
        }
        SemanticConversionTarget::Type(target_kind) => {
            if same_primitive_kind(value_type, *target_kind) {
                OperationResult::from_bound(value.clone())
            } else {
                unreachable!(
                    "BUG: invalid identity cast {:?} -> {:?} reached runtime",
                    value_type.name(),
                    target_kind
                );
            }
        }
        SemanticConversionTarget::Unit {
            unit_name,
            owning_type,
        } => cast_to_unit(&value.value, value_type, unit_name, owning_type),
    }
}

/// Apply a type cast when the operand is already held as [`BoundValueKind`].
pub fn convert_unit_operand(
    value: &BoundValueKind,
    value_type: &LemmaType,
    target: &SemanticConversionTarget,
) -> OperationResult {
    match target {
        SemanticConversionTarget::Type(PrimitiveKind::Boolean) => {
            if value_type.is_boolean() {
                OperationResult::from_bound(value.clone())
            } else {
                unreachable!(
                    "BUG: boolean cast on non-boolean; planning should have rejected {:?}",
                    value_type.name()
                );
            }
        }
        SemanticConversionTarget::Type(target_kind) => {
            if same_primitive_kind(value_type, *target_kind) {
                OperationResult::from_bound(value.clone())
            } else {
                convert_unit(value, value_type, target)
            }
        }
        _ => convert_unit(value, value_type, target),
    }
}

fn same_primitive_kind(value_type: &LemmaType, target: PrimitiveKind) -> bool {
    matches!(
        (target, &value_type.specifications),
        (PrimitiveKind::Number, TypeSpecification::Number { .. })
            | (PrimitiveKind::Text, TypeSpecification::Text { .. })
            | (PrimitiveKind::Boolean, TypeSpecification::Boolean { .. })
            | (PrimitiveKind::Date, TypeSpecification::Date { .. })
            | (PrimitiveKind::Time, TypeSpecification::Time { .. })
            | (PrimitiveKind::Ratio, TypeSpecification::Ratio { .. })
            | (PrimitiveKind::Measure, TypeSpecification::Measure { .. })
    )
}

fn cast_to_unit(
    value: &ValueKind,
    value_type: &LemmaType,
    unit_name: &str,
    owning_type: &Arc<crate::planning::semantics::LemmaType>,
) -> OperationResult {
    match value {
        ValueKind::Number(magnitude) => {
            cast_number_to_unit(magnitude.clone(), unit_name, owning_type)
        }
        ValueKind::Measure(magnitude) => cast_measure_to_unit(magnitude.clone(), unit_name),
        ValueKind::Range(left, right) => {
            cast_range_span_to_unit(left, right, value_type, unit_name, owning_type)
        }
        ValueKind::Ratio(magnitude) => cast_ratio_to_unit(magnitude.clone(), unit_name),
        other => unreachable!(
            "BUG: unit cast from {:?} should be rejected at planning",
            other
        ),
    }
}

fn cast_number_to_unit(
    magnitude: RationalInteger,
    unit_name: &str,
    owning_type: &Arc<crate::planning::semantics::LemmaType>,
) -> OperationResult {
    if owning_type.is_ratio() {
        return OperationResult::from_bound(BoundValueKind {
            value: ValueKind::Ratio(magnitude),
            measure_binding_unit: Some(Arc::from(unit_name)),
        });
    }
    let factor = owning_type.measure_unit_factor(unit_name).clone();
    let canonical = match checked_mul(&magnitude, &factor) {
        Ok(v) => v,
        Err(failure) => {
            return OperationResult::Veto(
                crate::computation::operation_result::VetoType::computation(failure.to_string()),
            )
        }
    };
    OperationResult::from_bound(BoundValueKind {
        value: ValueKind::Measure(canonical),
        measure_binding_unit: Some(Arc::from(unit_name)),
    })
}

fn cast_measure_to_unit(magnitude: RationalInteger, unit_name: &str) -> OperationResult {
    OperationResult::from_bound(BoundValueKind {
        value: ValueKind::Measure(magnitude),
        measure_binding_unit: Some(Arc::from(unit_name)),
    })
}

fn cast_ratio_to_unit(magnitude: RationalInteger, unit_name: &str) -> OperationResult {
    OperationResult::from_bound(BoundValueKind {
        value: ValueKind::Ratio(magnitude),
        measure_binding_unit: Some(Arc::from(unit_name)),
    })
}

fn cast_range_span_to_unit(
    left: &LiteralValue,
    right: &LiteralValue,
    value_type: &LemmaType,
    unit_name: &str,
    owning_type: &Arc<crate::planning::semantics::LemmaType>,
) -> OperationResult {
    let endpoint_type = value_type
        .specifications
        .element_from_range()
        .map(|element| Arc::new(LemmaType::primitive(element)))
        .unwrap_or_else(|| Arc::new(value_type.clone()));
    if let (ValueKind::Date(left_date), ValueKind::Date(right_date)) = (&left.value, &right.value) {
        if calendar_unit_factor(unit_name).is_some() {
            return super::datetime::compute_date_calendar_difference(
                left_date,
                right_date,
                &semantic_calendar_unit(unit_name),
                Arc::clone(owning_type),
            );
        }
        let span = super::range::compute_span(left, &endpoint_type, right, &endpoint_type);
        let OperationResult::Value(span) = span else {
            return span;
        };
        return convert_span_measure_to_unit(&span, unit_name);
    }

    let span = super::range::compute_span(left, &endpoint_type, right, &endpoint_type);
    let OperationResult::Value(span) = span else {
        return span;
    };
    match &span.value {
        ValueKind::Measure(_) => convert_span_measure_to_unit(&span, unit_name),
        ValueKind::Ratio(magnitude) => cast_ratio_to_unit(magnitude.clone(), unit_name),
        ValueKind::Number(magnitude) => {
            cast_number_to_unit(magnitude.clone(), unit_name, owning_type)
        }
        other => unreachable!("BUG: unexpected range span value kind for unit cast: {other:?}"),
    }
}

fn convert_span_measure_to_unit(span: &BoundValueKind, unit_name: &str) -> OperationResult {
    let ValueKind::Measure(magnitude) = &span.value else {
        unreachable!("BUG: span measure expected");
    };
    cast_measure_to_unit(magnitude.clone(), unit_name)
}

fn semantic_calendar_unit(unit_name: &str) -> SemanticCalendarUnit {
    match unit_name {
        "month" => SemanticCalendarUnit::Month,
        "year" => SemanticCalendarUnit::Year,
        other => unreachable!("BUG: unknown calendar unit '{other}' after planning"),
    }
}

fn cast_to_number(value: &ValueKind, value_type: &LemmaType) -> OperationResult {
    match value {
        ValueKind::Range(left, right) => {
            let endpoint_type = value_type
                .specifications
                .element_from_range()
                .map(|element| Arc::new(LemmaType::primitive(element)))
                .unwrap_or_else(|| Arc::new(value_type.clone()));
            let span = super::range::compute_span(left, &endpoint_type, right, &endpoint_type);
            let OperationResult::Value(span_value) = span else {
                return span;
            };
            // Span type is derived from endpoint types; use number for magnitude cast.
            cast_to_number(&span_value.value, primitive_number_arc().as_ref())
        }
        ValueKind::Measure(magnitude) if value_type.is_calendar_like() => {
            let signature = value_type.measure_runtime_signature();
            let unit_name = signature
                .first()
                .map(|(name, _)| name.as_str())
                .expect("BUG: calendar measure must carry a unit signature");
            let factor = value_type.measure_unit_factor(unit_name).clone();
            let in_unit = checked_div(magnitude, &factor)
                .expect("BUG: calendar de-canonicalization by unit factor must not fail");
            OperationResult::from_literal(LiteralValue::number_with_type(
                in_unit,
                primitive_number_arc().clone(),
            ))
        }
        ValueKind::Number(number) => OperationResult::from_literal(LiteralValue::number_with_type(
            number.clone(),
            primitive_number_arc().clone(),
        )),
        ValueKind::Boolean(b) => {
            let n = if *b {
                rational_new(1, 1)
            } else {
                rational_new(0, 1)
            };
            OperationResult::from_literal(LiteralValue::number_with_type(
                n,
                primitive_number_arc().clone(),
            ))
        }
        ValueKind::Ratio(rational_value) => OperationResult::from_literal(
            LiteralValue::number_with_type(rational_value.clone(), primitive_number_arc().clone()),
        ),
        ValueKind::Measure(magnitude) => {
            let signature = value_type.measure_runtime_signature();
            let factor = if signature.is_empty() {
                rational_one()
            } else if signature.len() == 1 && signature[0].1 == 1 {
                crate::planning::semantics::signature_factor(
                    &signature,
                    &crate::planning::unit_index::UnitIndex::new(),
                    Some(value_type),
                )
                .expect("BUG: de-canonicalization by unit factor must not fail")
            } else {
                panic!("BUG: cast_to_number with compound signature must be rejected at planning")
            };
            let in_unit = checked_div(magnitude, &factor)
                .expect("BUG: de-canonicalization by unit factor must not fail");
            OperationResult::from_literal(LiteralValue::number_with_type(
                in_unit,
                primitive_number_arc().clone(),
            ))
        }
        ValueKind::Text(_) | ValueKind::Date(_) | ValueKind::Time(_) => unreachable!(
            "BUG: cast to number from {:?} should be rejected at planning",
            value_type.name()
        ),
    }
}

pub(crate) fn conversion_target_declares_unit(
    target: &SemanticConversionTarget,
) -> Option<(&str, &Arc<crate::planning::semantics::LemmaType>)> {
    match target {
        SemanticConversionTarget::Unit {
            unit_name,
            owning_type,
        } => Some((unit_name.as_str(), owning_type)),
        SemanticConversionTarget::Type(_) => None,
    }
}

pub(crate) fn owning_type_declares_unit_name(
    owning_type: &crate::planning::semantics::LemmaType,
    unit_name: &str,
) -> bool {
    match &owning_type.specifications {
        TypeSpecification::Measure { units, .. } => units.get(unit_name).is_ok(),
        TypeSpecification::Ratio { units, .. } => units.get(unit_name).is_ok(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cast_ratio_to_unit_relabels_without_converting() {
        let magnitude = rational_new(42, 1);
        let result = cast_ratio_to_unit(magnitude.clone(), "percent");
        match result {
            OperationResult::Value(lit) => {
                let ValueKind::Ratio(m) = &lit.value else {
                    panic!("expected Ratio, got {:?}", lit.value);
                };
                assert_eq!(*m, magnitude, "magnitude must be preserved as-is");
            }
            OperationResult::Veto(v) => panic!("unexpected veto: {:?}", v),
        }
    }
}

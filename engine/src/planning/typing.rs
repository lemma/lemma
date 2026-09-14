//! Single typing kernel: graph validation and NormalForm cell stamping share these rules.

use crate::computation::arithmetic::{expand_signature_to_base_units, SignatureIndex};
use crate::parsing::ast::PrimitiveKind;
use crate::planning::semantics::{
    calendar_decomposition, combine_decompositions, duration_decomposition, primitive_boolean_arc,
    primitive_date_range_arc, primitive_number_arc, primitive_text_arc,
    range_type_specification_from_endpoints, ArithmeticComputation, BaseMeasureVector, LemmaType,
    MathematicalComputation, SemanticConversionTarget, TypeSpecification,
};
use crate::planning::unit_index::UnitIndex;
use std::collections::HashMap;
use std::sync::Arc;

/// Measure scope for arithmetic and anonymous-measure promotion: the plan's
/// unit index plus its reverse signature index.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MeasureScope<'a> {
    pub(crate) unit_index: &'a UnitIndex,
    pub(crate) signature_index: &'a SignatureIndex,
}

/// Result of a decomposition-based type lookup in scope.
///
/// Used by both `infer_expression_type` (to promote anonymous results to named types) and the
/// rule-boundary check (to produce precise error messages naming candidate types).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecompositionMatch {
    /// No declared measure type in scope has this decomposition.
    None,
    /// Exactly one declared measure type in scope has this decomposition.
    Unique(Arc<LemmaType>),
    /// Multiple measure families in scope share this decomposition; family names are
    /// sorted for stable diagnostic ordering.
    Multiple(Vec<String>),
}

/// Promote an anonymous measure to a named type when the scope resolves it.
///
/// Order: exact signature in `signature_index`, then expanded signature via
/// [`expand_signature_to_base_units`], then unique family-keyed decomposition in
/// `unit_index`. Non-anonymous types pass through unchanged.
pub(crate) fn resolve_anonymous_measure(
    ty: &Arc<LemmaType>,
    scope: &MeasureScope<'_>,
) -> Arc<LemmaType> {
    if !ty.is_anonymous_measure() {
        return Arc::clone(ty);
    }
    let signature = ty.measure_runtime_signature();
    if let Some((unit_name, named)) = scope.signature_index.get(&signature) {
        return Arc::new(
            named
                .as_ref()
                .clone()
                .with_measure_binding_unit(unit_name.clone()),
        );
    }
    let owners = [ty.as_ref()];
    let expanded = expand_signature_to_base_units(&signature, scope.unit_index, &owners);
    if let Some((unit_name, named)) = scope.signature_index.get(&expanded) {
        return Arc::new(
            named
                .as_ref()
                .clone()
                .with_measure_binding_unit(unit_name.clone()),
        );
    }
    if let Some(decomposition) = ty.measure_type_decomposition() {
        if !decomposition.is_empty() {
            if let DecompositionMatch::Unique(lemma_type) =
                find_unique_measure_type_in_unit_index(scope.unit_index, decomposition)
            {
                return lemma_type;
            }
        }
    }
    Arc::clone(ty)
}

/// Find the measure family (ies) in `unit_index` whose decomposition matches
/// `decomposition` exactly. Units belong to families; binding aliases in
/// `resolved` play no part. Imports are already merged into `unit_index`
/// during resolution.
pub(crate) fn find_unique_measure_type_in_unit_index(
    unit_index: &UnitIndex,
    decomposition: &BaseMeasureVector,
) -> DecompositionMatch {
    let mut seen: HashMap<String, Arc<LemmaType>> = HashMap::new();

    for arc in unit_index.values() {
        let lemma_type = arc.as_ref();
        if !matches!(lemma_type.specifications, TypeSpecification::Measure { .. }) {
            continue;
        }
        if lemma_type
            .measure_type_decomposition()
            .is_none_or(|decomposition_vector| decomposition_vector != decomposition)
        {
            continue;
        }
        let measure_family = lemma_type
            .measure_family_name()
            .expect("BUG: unit_index measure type must carry a family name");
        seen.entry(measure_family.to_string())
            .or_insert_with(|| Arc::clone(arc));
    }

    match seen.len() {
        0 => DecompositionMatch::None,
        1 => DecompositionMatch::Unique(
            seen.into_values()
                .next()
                .expect("BUG: seen has exactly one element, len checked"),
        ),
        _ => {
            let mut family_names: Vec<String> = seen.into_keys().collect();
            family_names.sort();
            DecompositionMatch::Multiple(family_names)
        }
    }
}

/// Result type of `left op right`. The one arithmetic typing rule: validation
/// (`infer_expression_type`), planning (NormalForm cell stamping) and the n-ary
/// fold in evaluation all call it, so a rule's lowered root has its inferred
/// type by construction (asserted when the plan is built).
///
/// `scope` is the plan's one measure scope: an anonymous measure result whose
/// dimensions match a signature or exactly one measure family in scope becomes
/// that named type. A number divided by a measure with no such family is a
/// plain number.
pub(crate) fn compute_arithmetic_result_type(
    left_type: Arc<LemmaType>,
    op: &ArithmeticComputation,
    right_type: Arc<LemmaType>,
    scope: &MeasureScope<'_>,
) -> Arc<LemmaType> {
    let mut result = compute_arithmetic_result_type_recursive(
        Arc::clone(&left_type),
        op,
        Arc::clone(&right_type),
        false,
    );
    if result.is_anonymous_measure() {
        result = resolve_anonymous_measure(&result, scope);
    }
    if matches!(op, ArithmeticComputation::Divide)
        && left_type.is_number()
        && right_type.is_measure()
        && result.is_anonymous_measure()
    {
        result = primitive_number_arc().clone();
    }
    result
}

fn compute_arithmetic_result_type_recursive(
    left_type: Arc<LemmaType>,
    op: &ArithmeticComputation,
    right_type: Arc<LemmaType>,
    swapped: bool,
) -> Arc<LemmaType> {
    match (&left_type.specifications, &right_type.specifications) {
        (TypeSpecification::Veto { .. }, _) | (_, TypeSpecification::Veto { .. }) => {
            Arc::new(LemmaType::veto_type())
        }
        (TypeSpecification::Undetermined, _) => Arc::new(LemmaType::undetermined_type()),

        (TypeSpecification::Date { .. }, TypeSpecification::Time { .. }) => Arc::new(
            LemmaType::anonymous_for_decomposition(duration_decomposition()),
        ),

        // Measure pairs must fall through to operator-specific arms below.
        // The general equal-type guard must not short-circuit those.
        _ if *left_type == *right_type
            && !matches!(
                &left_type.specifications,
                TypeSpecification::Measure { .. }
                    | TypeSpecification::MeasureRange { .. }
                    | TypeSpecification::NumberRange { .. }
                    | TypeSpecification::DateRange { .. }
                    | TypeSpecification::TimeRange { .. }
                    | TypeSpecification::RatioRange { .. }
            ) =>
        {
            Arc::clone(&left_type)
        }

        (TypeSpecification::Date { .. }, TypeSpecification::Measure { .. })
            if right_type.is_duration_like_measure() =>
        {
            Arc::clone(&left_type)
        }
        (TypeSpecification::Date { .. }, TypeSpecification::Measure { .. })
            if right_type.is_calendar_like_measure() =>
        {
            Arc::clone(&left_type)
        }
        (TypeSpecification::Measure { .. }, TypeSpecification::Date { .. })
            if left_type.is_calendar_like_measure() =>
        {
            Arc::clone(&right_type)
        }
        (TypeSpecification::Time { .. }, TypeSpecification::Measure { .. })
            if right_type.is_duration_like_measure() =>
        {
            Arc::clone(&left_type)
        }

        (TypeSpecification::Measure { .. }, TypeSpecification::Ratio { .. }) => {
            Arc::clone(&left_type)
        }
        (TypeSpecification::Measure { .. }, TypeSpecification::Number { .. }) => match op {
            ArithmeticComputation::Multiply
            | ArithmeticComputation::Divide
            | ArithmeticComputation::Modulo
            | ArithmeticComputation::Power => Arc::clone(&left_type),
            _ => Arc::new(LemmaType::undetermined_type()),
        },
        (
            TypeSpecification::Measure {
                decomposition: l_decomp_opt,
                ..
            },
            TypeSpecification::Measure {
                decomposition: r_decomp_opt,
                ..
            },
        ) => match op {
            ArithmeticComputation::Add | ArithmeticComputation::Subtract => {
                if left_type.compatible_with_anonymous_measure(&right_type)
                    || right_type.compatible_with_anonymous_measure(&left_type)
                {
                    let left_decomp = left_type.measure_type_decomposition();
                    let right_decomp = right_type.measure_type_decomposition();
                    if let (Some(ld), Some(rd)) = (left_decomp, right_decomp) {
                        if ld == rd {
                            if *ld == duration_decomposition() {
                                Arc::new(LemmaType::anonymous_for_decomposition(
                                    duration_decomposition(),
                                ))
                            } else {
                                Arc::new(LemmaType::anonymous_for_decomposition(ld.clone()))
                            }
                        } else if left_type.is_duration_like_measure()
                            && right_type.is_duration_like_measure()
                        {
                            Arc::new(LemmaType::anonymous_for_decomposition(
                                duration_decomposition(),
                            ))
                        } else if left_type.is_calendar_like() && right_type.is_calendar_like() {
                            Arc::new(LemmaType::anonymous_for_decomposition(
                                calendar_decomposition(),
                            ))
                        } else {
                            Arc::clone(&left_type)
                        }
                    } else if left_type.is_duration_like_measure()
                        && right_type.is_duration_like_measure()
                    {
                        Arc::new(LemmaType::anonymous_for_decomposition(
                            duration_decomposition(),
                        ))
                    } else if left_type.is_calendar_like() && right_type.is_calendar_like() {
                        Arc::new(LemmaType::anonymous_for_decomposition(
                            calendar_decomposition(),
                        ))
                    } else {
                        Arc::clone(&left_type)
                    }
                } else {
                    Arc::clone(&left_type)
                }
            }
            ArithmeticComputation::Multiply | ArithmeticComputation::Divide => {
                match (l_decomp_opt, r_decomp_opt) {
                    (Some(l_decomp), Some(r_decomp)) => {
                        let combined = combine_decompositions(
                            l_decomp,
                            r_decomp,
                            matches!(op, ArithmeticComputation::Multiply),
                        );
                        if combined.is_empty() {
                            primitive_number_arc().clone()
                        } else {
                            Arc::new(LemmaType::anonymous_for_decomposition(combined))
                        }
                    }
                    _ => Arc::clone(&left_type),
                }
            }
            _ => primitive_number_arc().clone(),
        },

        (
            TypeSpecification::Number { .. },
            TypeSpecification::Measure {
                decomposition: r_decomp_opt,
                ..
            },
        ) => match op {
            ArithmeticComputation::Multiply => Arc::clone(&right_type),
            ArithmeticComputation::Divide => match r_decomp_opt {
                Some(r_decomp) if !r_decomp.is_empty() => {
                    let negated: BaseMeasureVector =
                        r_decomp.iter().map(|(k, &e)| (k.clone(), -e)).collect();
                    Arc::new(LemmaType::anonymous_for_decomposition(negated))
                }
                _ => primitive_number_arc().clone(),
            },
            _ => Arc::new(LemmaType::undetermined_type()),
        },

        (TypeSpecification::Number { .. }, TypeSpecification::Ratio { .. }) => {
            primitive_number_arc().clone()
        }
        (TypeSpecification::Ratio { .. }, TypeSpecification::Number { .. }) => match op {
            ArithmeticComputation::Multiply => primitive_number_arc().clone(),
            _ => Arc::clone(&left_type),
        },
        (TypeSpecification::Number { .. }, TypeSpecification::Number { .. }) => {
            primitive_number_arc().clone()
        }

        (TypeSpecification::Ratio { .. }, TypeSpecification::Ratio { .. }) => {
            Arc::clone(&left_type)
        }
        (TypeSpecification::DateRange { .. }, TypeSpecification::DateRange { .. }) => match op {
            ArithmeticComputation::Add | ArithmeticComputation::Subtract => {
                range_span_type(&left_type)
            }
            _ => Arc::new(LemmaType::undetermined_type()),
        },
        (TypeSpecification::NumberRange { .. }, TypeSpecification::NumberRange { .. }) => {
            match op {
                ArithmeticComputation::Add | ArithmeticComputation::Subtract => {
                    range_span_type(&left_type)
                }
                _ => Arc::new(LemmaType::undetermined_type()),
            }
        }
        (TypeSpecification::MeasureRange { .. }, TypeSpecification::MeasureRange { .. }) => {
            match op {
                ArithmeticComputation::Add | ArithmeticComputation::Subtract
                    if range_matches_range_measure(&left_type, &right_type) =>
                {
                    range_span_type(&left_type)
                }
                _ => Arc::new(LemmaType::undetermined_type()),
            }
        }
        (TypeSpecification::RatioRange { .. }, TypeSpecification::RatioRange { .. }) => match op {
            ArithmeticComputation::Add | ArithmeticComputation::Subtract => {
                range_span_type(&left_type)
            }
            _ => Arc::new(LemmaType::undetermined_type()),
        },
        (TypeSpecification::NumberRange { .. }, TypeSpecification::Number { .. })
        | (TypeSpecification::RatioRange { .. }, TypeSpecification::Ratio { .. }) => match op {
            ArithmeticComputation::Add | ArithmeticComputation::Subtract => {
                range_span_type(&left_type)
            }
            _ => Arc::new(LemmaType::undetermined_type()),
        },
        (TypeSpecification::MeasureRange { .. }, TypeSpecification::Measure { .. }) => match op {
            ArithmeticComputation::Add | ArithmeticComputation::Subtract
                if range_matches_measure_type(&left_type, &right_type) =>
            {
                range_span_type(&left_type)
            }
            _ => Arc::new(LemmaType::undetermined_type()),
        },
        (TypeSpecification::Number { .. }, TypeSpecification::NumberRange { .. }) => match op {
            ArithmeticComputation::Add | ArithmeticComputation::Subtract => {
                range_span_type(&right_type)
            }
            _ => Arc::new(LemmaType::undetermined_type()),
        },
        (TypeSpecification::Measure { .. }, TypeSpecification::MeasureRange { .. }) => match op {
            ArithmeticComputation::Add | ArithmeticComputation::Subtract
                if range_matches_measure_type(&right_type, &left_type) =>
            {
                range_span_type(&right_type)
            }
            _ => Arc::new(LemmaType::undetermined_type()),
        },
        (TypeSpecification::Ratio { .. }, TypeSpecification::RatioRange { .. }) => match op {
            ArithmeticComputation::Add | ArithmeticComputation::Subtract => {
                range_span_type(&right_type)
            }
            _ => Arc::new(LemmaType::undetermined_type()),
        },
        (TypeSpecification::DateRange { .. }, TypeSpecification::Measure { .. })
            if right_type.is_duration_like_measure() =>
        {
            match op {
                ArithmeticComputation::Add | ArithmeticComputation::Subtract => {
                    range_span_type(&left_type)
                }
                _ => Arc::new(LemmaType::undetermined_type()),
            }
        }
        (TypeSpecification::DateRange { .. }, TypeSpecification::Measure { .. })
            if right_type.is_calendar_like_measure() =>
        {
            match op {
                ArithmeticComputation::Add | ArithmeticComputation::Subtract => {
                    Arc::clone(&left_type)
                }
                _ => Arc::new(LemmaType::undetermined_type()),
            }
        }
        (TypeSpecification::Measure { .. }, TypeSpecification::DateRange { .. })
            if left_type.is_duration_like_measure() =>
        {
            match op {
                ArithmeticComputation::Add | ArithmeticComputation::Subtract => {
                    range_span_type(&right_type)
                }
                _ => Arc::new(LemmaType::undetermined_type()),
            }
        }
        (TypeSpecification::Measure { .. }, TypeSpecification::DateRange { .. })
            if left_type.is_calendar_like_measure() =>
        {
            match op {
                ArithmeticComputation::Add | ArithmeticComputation::Subtract => {
                    Arc::clone(&right_type)
                }
                _ => Arc::new(LemmaType::undetermined_type()),
            }
        }
        _ => {
            if swapped {
                Arc::new(LemmaType::undetermined_type())
            } else {
                compute_arithmetic_result_type_recursive(right_type, op, left_type, true)
            }
        }
    }
}

pub(crate) fn infer_range_type_from_endpoint_types(
    left_type: &LemmaType,
    right_type: &LemmaType,
) -> Arc<LemmaType> {
    range_type_specification_from_endpoints(left_type, right_type)
        .map(|spec| Arc::new(LemmaType::primitive(spec)))
        .unwrap_or_else(|| Arc::new(LemmaType::undetermined_type()))
}

pub(crate) fn range_span_type(range_type: &LemmaType) -> Arc<LemmaType> {
    match &range_type.specifications {
        TypeSpecification::DateRange { .. } => Arc::new(LemmaType::anonymous_for_decomposition(
            duration_decomposition(),
        )),
        TypeSpecification::TimeRange { .. } => Arc::new(LemmaType::anonymous_for_decomposition(
            duration_decomposition(),
        )),
        TypeSpecification::NumberRange { .. } => primitive_number_arc().clone(),
        TypeSpecification::MeasureRange { .. } | TypeSpecification::RatioRange { .. } => {
            let element_spec = range_type
                .specifications
                .element_from_range()
                .expect("BUG: MeasureRange and RatioRange always define element_from_range");
            Arc::new(LemmaType {
                name: range_type.name.clone(),
                specifications: element_spec,
                extends: range_type.extends.clone(),
                measure_binding_unit: None,
            })
        }
        _ => Arc::new(LemmaType::undetermined_type()),
    }
}

pub(crate) fn range_matches_measure_type(range_type: &LemmaType, measure_type: &LemmaType) -> bool {
    match &range_type.specifications {
        TypeSpecification::DateRange { .. } => {
            measure_type.is_duration_like() || measure_type.is_calendar_like()
        }
        TypeSpecification::TimeRange { .. } => measure_type.is_duration_like(),
        TypeSpecification::NumberRange { .. } => measure_type.is_number(),
        TypeSpecification::MeasureRange { .. } => {
            measure_type.is_measure() && measure_range_matches_measure(range_type, measure_type)
        }
        TypeSpecification::RatioRange { .. } => measure_type.is_ratio(),
        _ => false,
    }
}

pub(crate) fn range_matches_range_measure(left_range: &LemmaType, right_range: &LemmaType) -> bool {
    let right_measure_type = range_span_type(right_range);
    !right_measure_type.is_undetermined()
        && range_matches_measure_type(left_range, &right_measure_type)
}

pub(crate) fn measure_range_matches_measure(
    range_type: &LemmaType,
    measure_type: &LemmaType,
) -> bool {
    if !measure_type.is_measure() {
        return false;
    }
    if let Some(element_spec) = range_type.specifications.element_from_range() {
        let endpoint_type = LemmaType::primitive(element_spec);
        if endpoint_type.same_measure_family(measure_type)
            || endpoint_type.compatible_with_anonymous_measure(measure_type)
            || measure_type.compatible_with_anonymous_measure(&endpoint_type)
        {
            return true;
        }
    }
    match (&range_type.specifications, &measure_type.specifications) {
        (
            TypeSpecification::MeasureRange {
                units: range_units,
                decomposition: range_decomposition,
                ..
            },
            TypeSpecification::Measure {
                units: measure_units,
                decomposition: measure_decomposition,
                ..
            },
        ) => {
            if range_units.0.is_empty() && range_decomposition.is_none() {
                true
            } else if measure_decomposition.is_none() {
                range_units == measure_units
            } else {
                range_units == measure_units && range_decomposition == measure_decomposition
            }
        }
        _ => false,
    }
}

pub(crate) fn logical_and_type(left: &LemmaType, right: &LemmaType) -> Arc<LemmaType> {
    if left.vetoed() || right.vetoed() {
        return Arc::new(LemmaType::veto_type());
    }
    if left.is_undetermined()
        || right.is_undetermined()
        || !left.is_boolean()
        || !right.is_boolean()
    {
        return Arc::new(LemmaType::undetermined_type());
    }
    primitive_boolean_arc().clone()
}

pub(crate) fn logical_not_type(operand: &LemmaType) -> Arc<LemmaType> {
    if operand.vetoed() {
        return Arc::new(LemmaType::veto_type());
    }
    if operand.is_undetermined() {
        return Arc::new(LemmaType::undetermined_type());
    }
    primitive_boolean_arc().clone()
}

pub(crate) fn comparison_type(left: &LemmaType, right: &LemmaType) -> Arc<LemmaType> {
    if left.vetoed() || right.vetoed() {
        return Arc::new(LemmaType::veto_type());
    }
    if left.is_undetermined() || right.is_undetermined() {
        return Arc::new(LemmaType::undetermined_type());
    }
    primitive_boolean_arc().clone()
}

pub(crate) fn date_predicate_type(operand: &LemmaType) -> Arc<LemmaType> {
    if operand.vetoed() {
        return Arc::new(LemmaType::veto_type());
    }
    primitive_boolean_arc().clone()
}

pub(crate) fn math_op_type(
    op: &MathematicalComputation,
    operand: &Arc<LemmaType>,
    scope: &MeasureScope<'_>,
) -> Arc<LemmaType> {
    if operand.vetoed() {
        return Arc::new(LemmaType::veto_type());
    }
    if operand.is_undetermined() {
        return Arc::new(LemmaType::undetermined_type());
    }
    if crate::computation::mathematical_computation_preserves_measure_magnitude(op)
        && operand.is_measure()
    {
        return resolve_anonymous_measure(operand, scope);
    }
    primitive_number_arc().clone()
}

pub(crate) fn unit_conversion_type(
    source: &Arc<LemmaType>,
    target: &SemanticConversionTarget,
) -> Arc<LemmaType> {
    match target {
        SemanticConversionTarget::Type(PrimitiveKind::Number) => primitive_number_arc().clone(),
        SemanticConversionTarget::Type(PrimitiveKind::Text) => primitive_text_arc().clone(),
        SemanticConversionTarget::Type(PrimitiveKind::Boolean) => primitive_boolean_arc().clone(),
        SemanticConversionTarget::Type(kind) if source.matches_primitive_kind(*kind) => {
            Arc::clone(source)
        }
        SemanticConversionTarget::Unit {
            unit_name,
            owning_type,
        } => Arc::new(
            owning_type
                .as_ref()
                .clone()
                .with_measure_binding_unit(unit_name.clone()),
        ),
        SemanticConversionTarget::Type(_) => Arc::new(LemmaType::undetermined_type()),
    }
}

pub(crate) fn past_future_range_type(offset: &LemmaType) -> Arc<LemmaType> {
    if offset.vetoed() {
        return Arc::new(LemmaType::veto_type());
    }
    primitive_date_range_arc().clone()
}

pub(crate) fn result_is_veto_type() -> Arc<LemmaType> {
    primitive_boolean_arc().clone()
}

/// First non-veto non-undetermined body type; else veto.
pub(crate) fn piecewise_type(bodies: impl IntoIterator<Item = Arc<LemmaType>>) -> Arc<LemmaType> {
    let mut saw_any = false;
    for ty in bodies {
        saw_any = true;
        if !ty.vetoed() && !ty.is_undetermined() {
            return ty;
        }
    }
    assert!(saw_any, "BUG: piecewise_type with zero bodies");
    Arc::new(LemmaType::veto_type())
}

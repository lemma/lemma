//! Single typing kernel: graph validation and NormalForm cell stamping share these rules.

use crate::computation::arithmetic::{expand_signature_to_base_units, SignatureIndex};
use crate::parsing::ast::ConversionTypeKind;
use crate::planning::semantics::{
    combine_decompositions, duration_decomposition, primitive_boolean_arc, primitive_date_arc,
    primitive_date_range_arc, primitive_number_arc, primitive_ratio_arc, primitive_text_arc,
    primitive_time_arc, range_type_specification_from_endpoints, ArithmeticComputation,
    BaseMeasureVector, ComparisonComputation, DataPath, KeyField, LemmaType, ListKey,
    MathematicalComputation, SemanticConversionTarget, TypeExtends, TypeFamily, TypeOptions,
    TypeSpecification,
};
use crate::planning::unit_index::UnitIndex;
use indexmap::IndexMap;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Measure scope for arithmetic and anonymous-measure promotion: the plan's
/// unit index, reverse signature index, and named types for family-root lookup.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MeasureScope<'a> {
    pub(crate) unit_index: &'a UnitIndex,
    pub(crate) signature_index: &'a SignatureIndex,
    pub(crate) named_types: &'a IndexMap<String, Arc<LemmaType>>,
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
    if let Some((unit_name, named)) =
        crate::computation::arithmetic::find_by_names(scope.signature_index, &signature)
    {
        return Arc::new(named.as_ref().clone().with_unit(unit_name.clone()));
    }
    let owners = [ty.as_ref()];
    let expanded = expand_signature_to_base_units(&signature, scope.unit_index, &owners);
    if let Some((unit_name, named)) =
        crate::computation::arithmetic::find_by_names(scope.signature_index, &expanded)
    {
        return Arc::new(named.as_ref().clone().with_unit(unit_name.clone()));
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
    let mut seen: BTreeMap<TypeFamily, Arc<LemmaType>> = BTreeMap::new();

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
            .measure_family()
            .expect("BUG: unit_index measure type must carry a family");
        seen.entry(measure_family.clone())
            .or_insert_with(|| Arc::clone(arc));
    }

    match seen.len() {
        0 => DecompositionMatch::None,
        1 => DecompositionMatch::Unique(
            seen.into_values()
                .next()
                .expect("BUG: seen has exactly one element, len checked"),
        ),
        _ => DecompositionMatch::Multiple(seen.into_keys().map(|family| family.name).collect()),
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
    let mut result =
        arithmetic_result_either_order(Arc::clone(&left_type), op, Arc::clone(&right_type));
    if result.is_anonymous_measure() {
        result = resolve_anonymous_measure(&result, scope);
    }
    generalized_value_type(&result, scope)
}

/// A measure raised to a known integer. The composition is `scale_decomposition`
/// of the left measure. An empty result is a number. [`resolve_anonymous_measure`]
/// names the result when one declared unit matches, and keeps that unit. No match,
/// or several families, stays anonymous so the rule boundary can reject it.
///
/// The exponent has already scaled. A cap failure is a planning error before this
/// is called.
pub(crate) fn quantity_integer_power_type(
    left: &LemmaType,
    exponent: i32,
    scope: &MeasureScope<'_>,
) -> Arc<LemmaType> {
    let decomposition = left
        .measure_type_decomposition()
        .unwrap_or_else(|| panic!("BUG: measure power requires a resolved decomposition"));
    let scaled = crate::planning::semantics::scale_decomposition(decomposition, exponent)
        .expect("BUG: measure power exponent exceeds the cap after validation");
    if scaled.is_empty() {
        return primitive_number_arc().clone();
    }
    let anonymous = Arc::new(LemmaType::anonymous_for_decomposition(scaled));
    resolve_anonymous_measure(&anonymous, scope)
}

/// Primitive (non-measure) or measure-family root, with slot bounds stripped.
/// Anonymous measures pass through unchanged so the rule boundary can reject them.
pub(crate) fn generalized_value_type(ty: &LemmaType, scope: &MeasureScope<'_>) -> Arc<LemmaType> {
    if ty.vetoed() {
        return Arc::new(LemmaType::veto_type());
    }
    if ty.is_undetermined() {
        return Arc::new(LemmaType::undetermined_type());
    }
    if ty.is_anonymous_measure() {
        return Arc::new(ty.clone());
    }
    if ty.is_measure() {
        return Arc::new(measure_family_root(ty, scope).without_slot_bounds());
    }
    if ty.is_measure_range() {
        let root = measure_family_root(ty, scope);
        let mut range = root.without_slot_bounds();
        range.specifications = root
            .specifications
            .range_from_element()
            .expect("BUG: measure family root must define range_from_element");
        range = range.without_slot_bounds();
        return Arc::new(range);
    }
    if ty.is_number() || ty.is_number_range() {
        return if ty.is_number_range() {
            Arc::new(LemmaType::primitive(TypeSpecification::number_range()))
        } else {
            primitive_number_arc().clone()
        };
    }
    if ty.is_ratio() || ty.is_ratio_range() {
        return if ty.is_ratio_range() {
            Arc::new(LemmaType::primitive(TypeSpecification::ratio_range()))
        } else {
            primitive_ratio_arc().clone()
        };
    }
    if ty.is_boolean() {
        return primitive_boolean_arc().clone();
    }
    if ty.is_text() {
        return primitive_text_arc().clone();
    }
    if ty.is_date() {
        return primitive_date_arc().clone();
    }
    if ty.is_time() {
        return primitive_time_arc().clone();
    }
    if ty.is_date_range() {
        return primitive_date_range_arc().clone();
    }
    if ty.is_time_range() {
        return Arc::new(LemmaType::primitive(TypeSpecification::time_range()));
    }
    Arc::new(ty.clone())
}

fn measure_family_root(ty: &LemmaType, scope: &MeasureScope<'_>) -> LemmaType {
    let Some(family) = ty.family() else {
        return ty.clone();
    };
    if let Some(root) = scope.named_types.get(&family.name) {
        if root.family() == Some(family) {
            return root.as_ref().clone();
        }
    }
    if let Some(root) = scope.unit_index.family_root(family) {
        return root.as_ref().clone();
    }
    if ty.name.as_deref() == Some(family.name.as_str()) && ty.family() == Some(family) {
        return ty.clone();
    }
    let mut root = ty.clone();
    root.name = Some(family.name.clone());
    root.extends = TypeExtends::Custom {
        parent: "measure".to_string(),
        family: family.clone(),
        defining_spec: match &ty.extends {
            TypeExtends::Custom { defining_spec, .. } => defining_spec.clone(),
            TypeExtends::Primitive => crate::planning::semantics::TypeDefiningSpec::Local,
        },
    };
    root
}

/// Result of `left op right`. Add/Multiply also try swapped operands (commutative);
/// Subtract/Divide/Modulo/Power keep written order only.
fn arithmetic_result_either_order(
    left_type: Arc<LemmaType>,
    op: &ArithmeticComputation,
    right_type: Arc<LemmaType>,
) -> Arc<LemmaType> {
    let as_written = arithmetic_result_ordered(Arc::clone(&left_type), op, Arc::clone(&right_type));
    if !matches!(
        op,
        ArithmeticComputation::Add | ArithmeticComputation::Multiply
    ) {
        return as_written;
    }
    if !as_written.is_undetermined() {
        return as_written;
    }
    arithmetic_result_ordered(right_type, op, left_type)
}

fn arithmetic_result_ordered(
    left_type: Arc<LemmaType>,
    op: &ArithmeticComputation,
    right_type: Arc<LemmaType>,
) -> Arc<LemmaType> {
    match (&left_type.specifications, &right_type.specifications) {
        (TypeSpecification::Veto { .. }, _) | (_, TypeSpecification::Veto { .. }) => {
            Arc::new(LemmaType::veto_type())
        }
        (TypeSpecification::Undetermined, _) | (_, TypeSpecification::Undetermined) => {
            Arc::new(LemmaType::undetermined_type())
        }

        (TypeSpecification::Date { .. }, TypeSpecification::Time { .. })
            if matches!(op, ArithmeticComputation::Subtract) =>
        {
            Arc::new(LemmaType::anonymous_for_decomposition(
                duration_decomposition(),
            ))
        }

        // `mod` is numbers only.
        _ if matches!(op, ArithmeticComputation::Modulo) => {
            if left_type.is_number() && right_type.is_number() {
                primitive_number_arc().clone()
            } else {
                Arc::new(LemmaType::undetermined_type())
            }
        }

        // Measure pairs and date/time arithmetic must fall through to
        // operator-specific arms below. Date−date / time−time are not values
        // of date/time; the equal-type guard must not invent them.
        _ if *left_type == *right_type
            && !matches!(
                &left_type.specifications,
                TypeSpecification::Measure { .. }
                    | TypeSpecification::MeasureRange { .. }
                    | TypeSpecification::NumberRange { .. }
                    | TypeSpecification::DateRange { .. }
                    | TypeSpecification::TimeRange { .. }
                    | TypeSpecification::RatioRange { .. }
                    | TypeSpecification::Date { .. }
                    | TypeSpecification::Time { .. }
            ) =>
        {
            Arc::clone(&left_type)
        }

        (TypeSpecification::Date { .. }, TypeSpecification::Measure { .. })
            if right_type.is_duration_like_measure()
                && matches!(
                    op,
                    ArithmeticComputation::Add | ArithmeticComputation::Subtract
                ) =>
        {
            Arc::clone(&left_type)
        }
        (TypeSpecification::Date { .. }, TypeSpecification::Measure { .. })
            if right_type.is_calendar_like_measure()
                && matches!(
                    op,
                    ArithmeticComputation::Add | ArithmeticComputation::Subtract
                ) =>
        {
            Arc::clone(&left_type)
        }
        (TypeSpecification::Measure { .. }, TypeSpecification::Date { .. })
            if left_type.is_calendar_like_measure() && matches!(op, ArithmeticComputation::Add) =>
        {
            Arc::clone(&right_type)
        }
        (TypeSpecification::Time { .. }, TypeSpecification::Measure { .. })
            if right_type.is_duration_like_measure()
                && matches!(
                    op,
                    ArithmeticComputation::Add | ArithmeticComputation::Subtract
                ) =>
        {
            Arc::clone(&left_type)
        }

        (TypeSpecification::Measure { .. }, TypeSpecification::Ratio { .. })
            if matches!(
                op,
                ArithmeticComputation::Multiply | ArithmeticComputation::Divide
            ) =>
        {
            Arc::clone(&left_type)
        }
        (TypeSpecification::Measure { .. }, TypeSpecification::Number { .. }) => match op {
            ArithmeticComputation::Multiply | ArithmeticComputation::Divide => {
                Arc::clone(&left_type)
            }
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
                if left_type.is_anonymous_measure() || right_type.is_anonymous_measure() {
                    Arc::new(LemmaType::undetermined_type())
                } else if left_type.same_measure_family(&right_type)
                    || left_type.compatible_with_anonymous_measure(&right_type)
                    || right_type.compatible_with_anonymous_measure(&left_type)
                {
                    Arc::clone(&left_type)
                } else {
                    Arc::new(LemmaType::undetermined_type())
                }
            }
            ArithmeticComputation::Multiply | ArithmeticComputation::Divide => {
                match (l_decomp_opt, r_decomp_opt) {
                    (Some(l_decomp), Some(r_decomp)) => {
                        let Some(combined) = combine_decompositions(
                            l_decomp,
                            r_decomp,
                            matches!(op, ArithmeticComputation::Multiply),
                        ) else {
                            return Arc::new(LemmaType::undetermined_type());
                        };
                        if combined.is_empty() {
                            primitive_number_arc().clone()
                        } else {
                            Arc::new(LemmaType::anonymous_for_decomposition(combined))
                        }
                    }
                    _ => Arc::clone(&left_type),
                }
            }
            _ => Arc::new(LemmaType::undetermined_type()),
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

        (TypeSpecification::Number { .. }, TypeSpecification::Ratio { .. }) => match op {
            ArithmeticComputation::Multiply
            | ArithmeticComputation::Divide
            | ArithmeticComputation::Power => primitive_number_arc().clone(),
            _ => Arc::new(LemmaType::undetermined_type()),
        },
        (TypeSpecification::Ratio { .. }, TypeSpecification::Number { .. }) => match op {
            ArithmeticComputation::Multiply => primitive_number_arc().clone(),
            ArithmeticComputation::Divide | ArithmeticComputation::Power => Arc::clone(&left_type),
            _ => Arc::new(LemmaType::undetermined_type()),
        },
        (TypeSpecification::Number { .. }, TypeSpecification::Number { .. }) => {
            primitive_number_arc().clone()
        }

        (TypeSpecification::Ratio { .. }, TypeSpecification::Ratio { .. }) => match op {
            ArithmeticComputation::Add
            | ArithmeticComputation::Subtract
            | ArithmeticComputation::Multiply
            | ArithmeticComputation::Divide
            | ArithmeticComputation::Power => Arc::clone(&left_type),
            _ => Arc::new(LemmaType::undetermined_type()),
        },
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
        _ => Arc::new(LemmaType::undetermined_type()),
    }
}

pub(crate) fn infer_range_type_from_endpoint_types(
    left_type: &LemmaType,
    right_type: &LemmaType,
) -> Arc<LemmaType> {
    let Some(spec) = range_type_specification_from_endpoints(left_type, right_type) else {
        return Arc::new(LemmaType::undetermined_type());
    };
    let extends = match (&left_type.extends, left_type.family(), right_type.family()) {
        (TypeExtends::Custom { .. }, Some(left_family), Some(right_family))
            if left_family == right_family =>
        {
            left_type.extends.clone()
        }
        _ => TypeExtends::Primitive,
    };
    Arc::new(LemmaType {
        name: None,
        specifications: spec,
        extends,
        options: TypeOptions::default(),
    })
}

pub(crate) fn range_span_type(range_type: &LemmaType) -> Arc<LemmaType> {
    match &range_type.specifications {
        TypeSpecification::DateRange { .. } | TypeSpecification::TimeRange { .. } => Arc::new(
            LemmaType::anonymous_for_decomposition(duration_decomposition()),
        ),
        TypeSpecification::NumberRange { .. } => primitive_number_arc().clone(),
        TypeSpecification::MeasureRange { .. } | TypeSpecification::RatioRange { .. } => {
            range_type.range_element_type()
        }
        _ => Arc::new(LemmaType::undetermined_type()),
    }
}

/// Result type of `lower` / `upper`, including anonymous-measure promotion.
///
/// Promotion may pin a canonical unit (`year` → `month`). That pin is cleared so the
/// endpoint's written unit, carried on the value, is what display uses.
pub(crate) fn range_bound_result_type(
    range_type: &LemmaType,
    scope: &MeasureScope<'_>,
) -> Arc<LemmaType> {
    let endpoint = range_endpoint_type(range_type);
    let anonymous = endpoint.is_anonymous_measure();
    let resolved = resolve_anonymous_measure(&endpoint, scope);
    if anonymous && resolved.unit().is_some() {
        let mut cleared = resolved.as_ref().clone();
        if let TypeSpecification::Measure { unit, .. } = &mut cleared.specifications {
            *unit = None;
        }
        return Arc::new(cleared);
    }
    resolved
}

/// Endpoint type of `lower` / `upper`.
///
/// Measure and ratio ranges keep the element spec and `extends` (including the
/// measure family's identity). A slot such as `data band: money range` is named
/// after the family's root (`money`), not the slot. Date, time, and number ranges
/// use the element primitive. Anonymous measures are promoted by the caller via
/// [`resolve_anonymous_measure`].
pub(crate) fn range_endpoint_type(range_type: &LemmaType) -> Arc<LemmaType> {
    if range_type.vetoed() {
        return Arc::new(LemmaType::veto_type());
    }
    if range_type.is_undetermined() {
        return Arc::new(LemmaType::undetermined_type());
    }
    match &range_type.specifications {
        TypeSpecification::MeasureRange { .. } | TypeSpecification::RatioRange { .. } => {
            range_type.range_element_type()
        }
        _ => range_type
            .specifications
            .element_from_range()
            .map(|spec| Arc::new(LemmaType::primitive(spec)))
            .unwrap_or_else(|| Arc::new(LemmaType::undetermined_type())),
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
    if matches!(
        &range_type.specifications,
        TypeSpecification::MeasureRange { .. }
    ) {
        let endpoint_type = range_type.range_element_type();
        if endpoint_type.same_measure_family(measure_type)
            || endpoint_type.compatible_with_anonymous_measure(measure_type)
            || measure_type.compatible_with_anonymous_measure(endpoint_type.as_ref())
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
            if range_type.specifications.is_unresolved_measure_range() {
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

pub(crate) fn logical_connective_type(left: &LemmaType, right: &LemmaType) -> Arc<LemmaType> {
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
    if operand.is_undetermined() || !operand.is_boolean() {
        return Arc::new(LemmaType::undetermined_type());
    }
    primitive_boolean_arc().clone()
}

pub(crate) fn comparison_type(
    left: &LemmaType,
    op: &ComparisonComputation,
    right: &LemmaType,
) -> Arc<LemmaType> {
    if left.vetoed() || right.vetoed() {
        return Arc::new(LemmaType::veto_type());
    }
    if left.is_undetermined() || right.is_undetermined() {
        return Arc::new(LemmaType::undetermined_type());
    }
    let equality_only = matches!(op, ComparisonComputation::Is | ComparisonComputation::IsNot);
    if (left.is_boolean() && right.is_boolean()) || (left.is_text() && right.is_text()) {
        return if equality_only {
            primitive_boolean_arc().clone()
        } else {
            Arc::new(LemmaType::undetermined_type())
        };
    }
    let comparable = (left.is_number() && right.is_number())
        || (left.is_ratio() && right.is_ratio())
        || (left.is_date() && right.is_date())
        || (left.is_time() && right.is_time())
        || (left.is_measure()
            && right.is_measure()
            && !left.is_anonymous_measure()
            && !right.is_anonymous_measure()
            && left.same_measure_family(right))
        || (left.is_duration_like() && right.is_duration_like())
        || (left.is_calendar_like() && right.is_calendar_like())
        || (left.is_calendar_like() && right.is_number())
        || (left.is_number() && right.is_calendar_like())
        || (left.is_range() && range_matches_measure_type(left, right))
        || (right.is_range() && range_matches_measure_type(right, left));
    if !comparable {
        return Arc::new(LemmaType::undetermined_type());
    }
    primitive_boolean_arc().clone()
}

/// Result type of `value in range`. Distinct from comparisons: the right operand is a range.
pub(crate) fn range_containment_type(value: &LemmaType, range: &LemmaType) -> Arc<LemmaType> {
    if value.vetoed() || range.vetoed() {
        return Arc::new(LemmaType::veto_type());
    }
    if value.is_undetermined() || range.is_undetermined() {
        return Arc::new(LemmaType::undetermined_type());
    }
    if !range.is_range() {
        return Arc::new(LemmaType::undetermined_type());
    }
    let compatible = (range.is_date_range() && value.is_date())
        || (range.is_time_range() && (value.is_time() || value.is_date()))
        || (range.is_number_range() && value.is_number())
        || (range.is_measure_range()
            && value.is_measure()
            && !value.is_anonymous_measure()
            && range_matches_measure_type(range, value))
        || (range.is_ratio_range() && value.is_ratio())
        || (range.is_calendar_like_range() && value.is_calendar_like());
    if !compatible {
        return Arc::new(LemmaType::undetermined_type());
    }
    primitive_boolean_arc().clone()
}

pub(crate) fn date_predicate_type(operand: &LemmaType) -> Arc<LemmaType> {
    if operand.vetoed() {
        return Arc::new(LemmaType::veto_type());
    }
    if operand.is_undetermined() || !operand.is_date() {
        return Arc::new(LemmaType::undetermined_type());
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
    if operand.is_anonymous_measure() {
        return Arc::new(LemmaType::undetermined_type());
    }
    if crate::computation::mathematical_computation_applies_to_magnitude(op)
        && (operand.is_measure() || operand.is_ratio())
    {
        if operand.is_measure() {
            return resolve_anonymous_measure(operand, scope);
        }
        return Arc::clone(operand);
    }
    if operand.is_number() {
        return primitive_number_arc().clone();
    }
    Arc::new(LemmaType::undetermined_type())
}

/// Whether `operand -> round step` is defined: a number to a number step, a ratio to a
/// ratio step, a measure to a step of its family (or of its dimensions when anonymous).
pub(crate) fn step_rounding_compatible(operand: &LemmaType, step: &LemmaType) -> bool {
    (operand.is_number() && step.is_number())
        || (operand.is_ratio() && step.is_ratio())
        || operand.same_measure_family(step)
        || operand.compatible_with_anonymous_measure(step)
}

/// Type of `operand -> round` or `operand -> round step`.
/// Without a step the result is the operand (a number). With a step it is the step's type.
pub(crate) fn step_rounding_type(
    operand: &Arc<LemmaType>,
    step: Option<&Arc<LemmaType>>,
) -> Arc<LemmaType> {
    let Some(step) = step else {
        if operand.vetoed() {
            return Arc::new(LemmaType::veto_type());
        }
        if operand.is_undetermined() || !operand.is_number() {
            return Arc::new(LemmaType::undetermined_type());
        }
        return Arc::clone(operand);
    };
    if operand.vetoed() || step.vetoed() {
        return Arc::new(LemmaType::veto_type());
    }
    if operand.is_undetermined()
        || step.is_undetermined()
        || !step_rounding_compatible(operand, step)
    {
        return Arc::new(LemmaType::undetermined_type());
    }
    Arc::clone(step)
}

pub(crate) fn unit_conversion_type(
    source: &Arc<LemmaType>,
    target: &SemanticConversionTarget,
) -> Arc<LemmaType> {
    if source.vetoed() {
        return Arc::new(LemmaType::veto_type());
    }
    if source.is_undetermined() || source.is_spec() {
        return Arc::new(LemmaType::undetermined_type());
    }
    match target {
        SemanticConversionTarget::Type(ConversionTypeKind::Number) => {
            if source.is_date_range()
                || source.is_time_range()
                || source.is_measure_range()
                || source.is_ratio_range()
                || source.is_calendar_like_range()
            {
                return Arc::new(LemmaType::undetermined_type());
            }
            if source.is_anonymous_measure()
                && source
                    .measure_type_decomposition()
                    .is_some_and(|decomp| !decomp.is_empty())
            {
                return Arc::new(LemmaType::undetermined_type());
            }
            // Measure/calendar/duration strip requires an expression-named unit
            // (`5 eur as number`, not a bare data path).
            if source.is_measure() && source.unit().is_none() {
                return Arc::new(LemmaType::undetermined_type());
            }
            if source.is_number_range()
                || source.is_number()
                || source.is_measure()
                || source.is_ratio()
            {
                return primitive_number_arc().clone();
            }
            Arc::new(LemmaType::undetermined_type())
        }
        SemanticConversionTarget::Type(ConversionTypeKind::Boolean) if source.is_boolean() => {
            primitive_boolean_arc().clone()
        }
        SemanticConversionTarget::Type(ConversionTypeKind::Time)
            if source.is_date() || source.is_time() =>
        {
            primitive_time_arc().clone()
        }
        SemanticConversionTarget::Type(kind)
            if source.matches_primitive_kind(kind.to_primitive_kind()) =>
        {
            Arc::clone(source)
        }
        SemanticConversionTarget::Unit {
            unit_name,
            owning_type,
        } => {
            let target_type = owning_type.as_ref();
            // Span-to-unit validity needs the unit index; planning check owns that.
            let compatible = source.is_number()
                || (source.is_measure() && measure_unit_conversion_compatible(source, target_type))
                || source.is_date_range()
                || source.is_time_range()
                || source.is_measure_range()
                || source.is_ratio_range()
                || source.is_ratio();
            if compatible {
                Arc::new(owning_type.as_ref().clone().with_unit(unit_name.clone()))
            } else {
                Arc::new(LemmaType::undetermined_type())
            }
        }
        SemanticConversionTarget::Type(_) => Arc::new(LemmaType::undetermined_type()),
    }
}

/// Whether a measure expression may convert or relabel onto `target_type`'s unit.
fn measure_unit_conversion_compatible(source: &LemmaType, target_type: &LemmaType) -> bool {
    if source.is_calendar_like() {
        return target_type.is_calendar_like();
    }
    if source.same_measure_family(target_type)
        || source.compatible_with_anonymous_measure(target_type)
        || target_type.compatible_with_anonymous_measure(source)
    {
        return true;
    }
    // Cross-family relabel: expression named a unit, both sides simple base,
    // neither duration-trait nor calendar target/source (calendar handled above).
    source.unit().is_some()
        && !source.is_duration_like()
        && !target_type.is_calendar_like()
        && !target_type.is_duration_like()
        && source.is_simple_base_measure()
        && target_type.is_simple_base_measure()
}

pub(crate) fn past_future_range_type(offset: &LemmaType) -> Arc<LemmaType> {
    if offset.vetoed() {
        return Arc::new(LemmaType::veto_type());
    }
    if offset.is_undetermined() || (!offset.is_duration_like() && !offset.is_calendar_like()) {
        return Arc::new(LemmaType::undetermined_type());
    }
    primitive_date_range_arc().clone()
}

pub(crate) fn result_is_veto_type() -> Arc<LemmaType> {
    primitive_boolean_arc().clone()
}

/// How a list command's result type is determined from the list type.
#[derive(Clone, Copy)]
pub(crate) enum ListCommandResultKind {
    Count,
    BooleanPredicate,
    KeepList,
    Element,
    InstanceReduce,
}

pub(crate) fn list_has_instance_rows(list_type: &LemmaType) -> bool {
    matches!(
        &list_type.specifications,
        TypeSpecification::List { element, .. }
            if matches!(&element.specifications, TypeSpecification::Spec { .. })
    )
}

pub(crate) fn list_command_result_type(
    list_type: &Arc<LemmaType>,
    kind: ListCommandResultKind,
    first_argument: Option<&Arc<LemmaType>>,
    measure_scope: &MeasureScope<'_>,
) -> Arc<LemmaType> {
    match kind {
        ListCommandResultKind::Count => primitive_number_arc().clone(),
        ListCommandResultKind::BooleanPredicate => primitive_boolean_arc().clone(),
        ListCommandResultKind::KeepList => Arc::clone(list_type),
        ListCommandResultKind::InstanceReduce => Arc::clone(
            first_argument.expect("BUG: reduce over instance rows lowered without argument"),
        ),
        ListCommandResultKind::Element => {
            resolve_anonymous_measure(list_type.list_element_lemma_type(), measure_scope)
        }
    }
}

/// After `-> key`, the list type's key becomes those fields (planning and normalize share this).
pub(crate) fn refine_list_key_type(
    list_type: &Arc<LemmaType>,
    fields: &[DataPath],
) -> Arc<LemmaType> {
    let TypeSpecification::List { element, help, .. } = &list_type.specifications else {
        return Arc::clone(list_type);
    };
    Arc::new(LemmaType {
        name: list_type.name.clone(),
        specifications: TypeSpecification::List {
            element: Arc::clone(element),
            key: ListKey::Fields(
                fields
                    .iter()
                    .map(|path| KeyField {
                        name: path.input_key(),
                        path: path.clone(),
                    })
                    .collect(),
            ),
            help: help.clone(),
        },
        extends: list_type.extends.clone(),
        options: list_type.options.clone(),
    })
}

/// First non-veto non-undetermined body type when all such bodies are identical;
/// otherwise the generalized value type of the first. All-veto → veto.
pub(crate) fn piecewise_type(
    bodies: impl IntoIterator<Item = Arc<LemmaType>>,
    scope: &MeasureScope<'_>,
) -> Arc<LemmaType> {
    let mut kept = Vec::new();
    let mut saw_any = false;
    for ty in bodies {
        saw_any = true;
        if !ty.vetoed() && !ty.is_undetermined() {
            kept.push(ty);
        }
    }
    assert!(saw_any, "BUG: piecewise_type with zero bodies");
    let Some(first) = kept.first() else {
        return Arc::new(LemmaType::veto_type());
    };
    if kept.iter().all(|ty| ty.as_ref() == first.as_ref()) {
        return Arc::clone(first);
    }
    generalized_value_type(first, scope)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computation::rational::rational_new;
    use crate::planning::semantics::tests::test_type_family;
    use crate::planning::unit_index::UnitIndex;

    struct MoneyFamilyFixture {
        money: Arc<LemmaType>,
        deposit: Arc<LemmaType>,
        fee: Arc<LemmaType>,
        named: IndexMap<String, Arc<LemmaType>>,
    }

    fn money_family_types() -> MoneyFamilyFixture {
        let family = test_type_family("money");
        let money = Arc::new(LemmaType::new(
            "money".to_string(),
            TypeSpecification::measure(),
            TypeExtends::custom_local("measure".to_string(), family.clone()),
        ));
        let mut deposit_spec = TypeSpecification::measure();
        if let TypeSpecification::Measure { maximum, help, .. } = &mut deposit_spec {
            *maximum = Some((rational_new(1000, 1), "eur".to_string()));
            *help = "deposit cap".to_string();
        }
        let deposit = Arc::new(LemmaType::new(
            "deposit".to_string(),
            deposit_spec,
            TypeExtends::custom_local("money".to_string(), family.clone()),
        ));
        let mut fee_spec = TypeSpecification::measure();
        if let TypeSpecification::Measure { minimum, help, .. } = &mut fee_spec {
            *minimum = Some((rational_new(1, 1), "eur".to_string()));
            *help = "fee floor".to_string();
        }
        let fee = Arc::new(LemmaType::new(
            "fee".to_string(),
            fee_spec,
            TypeExtends::custom_local("money".to_string(), family),
        ));
        let mut named = IndexMap::new();
        named.insert("money".to_string(), Arc::clone(&money));
        named.insert("deposit".to_string(), Arc::clone(&deposit));
        named.insert("fee".to_string(), Arc::clone(&fee));
        MoneyFamilyFixture {
            money,
            deposit,
            fee,
            named,
        }
    }

    #[test]
    fn arithmetic_measure_siblings_generalize_to_family_root() {
        let fixture = money_family_types();
        let unit_index = UnitIndex::new();
        let signature_index = SignatureIndex::new();
        let scope = MeasureScope {
            unit_index: &unit_index,
            signature_index: &signature_index,
            named_types: &fixture.named,
        };
        let result = compute_arithmetic_result_type(
            Arc::clone(&fixture.deposit),
            &ArithmeticComputation::Add,
            Arc::clone(&fixture.fee),
            &scope,
        );
        assert_eq!(result.name.as_deref(), Some("money"));
        assert!(result.same_measure_family(&fixture.money));
        match &result.specifications {
            TypeSpecification::Measure {
                minimum,
                maximum,
                unit,
                help,
                ..
            } => {
                assert!(minimum.is_none());
                assert!(maximum.is_none());
                assert!(unit.is_none());
                assert_eq!(help, "");
            }
            other => panic!("expected Measure, got {other:?}"),
        }
    }

    #[test]
    fn arithmetic_generalizes_via_unit_index_family_root_when_named_types_miss() {
        let fixture = money_family_types();
        let mut unit_index = UnitIndex::new();
        unit_index.insert_owner(
            "eur".into(),
            crate::planning::unit_index::UnitOwner::Named {
                import_path: Vec::new(),
                type_name: "money".into(),
                lemma_type: Arc::clone(&fixture.money),
            },
        );
        let signature_index = SignatureIndex::new();
        let named = IndexMap::new();
        let scope = MeasureScope {
            unit_index: &unit_index,
            signature_index: &signature_index,
            named_types: &named,
        };
        let result = compute_arithmetic_result_type(
            Arc::clone(&fixture.deposit),
            &ArithmeticComputation::Add,
            Arc::clone(&fixture.fee),
            &scope,
        );
        assert_eq!(result.name.as_deref(), Some("money"));
        assert!(Arc::ptr_eq(
            unit_index
                .family_root(fixture.money.family().expect("money family"))
                .expect("indexed root"),
            &fixture.money
        ));
        assert!(result.same_measure_family(&fixture.money));
    }

    #[test]
    fn arithmetic_imported_measure_does_not_generalize_to_local_same_named_family() {
        let local_family = TypeFamily {
            repository: None,
            spec: "shop".into(),
            effective: crate::parsing::ast::EffectiveDate::Origin,
            name: "money".into(),
        };
        let imported_family = TypeFamily {
            repository: None,
            spec: "pricing".into(),
            effective: crate::parsing::ast::EffectiveDate::Origin,
            name: "money".into(),
        };
        let local_money = Arc::new(LemmaType::new(
            "money".to_string(),
            TypeSpecification::measure(),
            TypeExtends::custom_local("measure".to_string(), local_family.clone()),
        ));
        let imported_money = Arc::new(LemmaType::new(
            "money".to_string(),
            TypeSpecification::measure(),
            TypeExtends::custom_local("measure".to_string(), imported_family.clone()),
        ));
        let imported_fee = Arc::new(LemmaType::new(
            "fee".to_string(),
            TypeSpecification::measure(),
            TypeExtends::custom_local("money".to_string(), imported_family.clone()),
        ));
        let imported_deposit = Arc::new(LemmaType::new(
            "deposit".to_string(),
            TypeSpecification::measure(),
            TypeExtends::custom_local("money".to_string(), imported_family.clone()),
        ));
        let mut unit_index = UnitIndex::new();
        unit_index.insert_owner(
            "eur".into(),
            crate::planning::unit_index::UnitOwner::Named {
                import_path: Vec::new(),
                type_name: "money".into(),
                lemma_type: Arc::clone(&local_money),
            },
        );
        unit_index.insert_owner(
            "eur".into(),
            crate::planning::unit_index::UnitOwner::Named {
                import_path: vec!["pricing".into()],
                type_name: "money".into(),
                lemma_type: Arc::clone(&imported_money),
            },
        );
        let mut named = IndexMap::new();
        named.insert("money".to_string(), Arc::clone(&local_money));
        let signature_index = SignatureIndex::new();
        let scope = MeasureScope {
            unit_index: &unit_index,
            signature_index: &signature_index,
            named_types: &named,
        };
        let result = compute_arithmetic_result_type(
            imported_deposit,
            &ArithmeticComputation::Add,
            imported_fee,
            &scope,
        );
        assert_eq!(result.name.as_deref(), Some("money"));
        assert!(result.same_measure_family(&imported_money));
        assert!(!result.same_measure_family(&local_money));
        assert_eq!(result.family(), Some(&imported_family));
        assert_ne!(result.family(), Some(&local_family));
    }

    #[test]
    fn arithmetic_number_extensions_generalize_to_primitive() {
        let mut age_spec = TypeSpecification::number();
        if let TypeSpecification::Number { minimum, .. } = &mut age_spec {
            *minimum = Some(rational_new(0, 1));
        }
        let age = Arc::new(LemmaType::new(
            "age".to_string(),
            age_spec,
            TypeExtends::custom_local("number".to_string(), test_type_family("age")),
        ));
        let mut bonus_spec = TypeSpecification::number();
        if let TypeSpecification::Number { minimum, .. } = &mut bonus_spec {
            *minimum = Some(rational_new(0, 1));
        }
        let bonus = Arc::new(LemmaType::new(
            "bonus_years".to_string(),
            bonus_spec,
            TypeExtends::custom_local("number".to_string(), test_type_family("bonus_years")),
        ));
        let unit_index = UnitIndex::new();
        let signature_index = SignatureIndex::new();
        let named = IndexMap::new();
        let scope = MeasureScope {
            unit_index: &unit_index,
            signature_index: &signature_index,
            named_types: &named,
        };
        let result =
            compute_arithmetic_result_type(age, &ArithmeticComputation::Add, bonus, &scope);
        assert!(result.is_number());
        assert!(result.name.is_none());
        assert!(matches!(result.extends, TypeExtends::Primitive));
    }

    #[test]
    fn piecewise_mixed_measure_branches_generalize_to_family_root() {
        let fixture = money_family_types();
        let unit_index = UnitIndex::new();
        let signature_index = SignatureIndex::new();
        let scope = MeasureScope {
            unit_index: &unit_index,
            signature_index: &signature_index,
            named_types: &fixture.named,
        };
        let picked = piecewise_type(
            [Arc::clone(&fixture.deposit), Arc::clone(&fixture.fee)],
            &scope,
        );
        assert_eq!(picked.name.as_deref(), Some("money"));
        assert!(picked.same_measure_family(&fixture.money));
        let reverse = piecewise_type([fixture.fee, Arc::clone(&fixture.deposit)], &scope);
        assert_eq!(reverse.name.as_deref(), Some("money"));
    }

    #[test]
    fn piecewise_identical_measure_branches_keep_named_type() {
        let fixture = money_family_types();
        let unit_index = UnitIndex::new();
        let signature_index = SignatureIndex::new();
        let scope = MeasureScope {
            unit_index: &unit_index,
            signature_index: &signature_index,
            named_types: &fixture.named,
        };
        let picked = piecewise_type(
            [Arc::clone(&fixture.deposit), Arc::clone(&fixture.deposit)],
            &scope,
        );
        assert_eq!(picked.name.as_deref(), Some("deposit"));
        assert_eq!(picked.as_ref(), fixture.deposit.as_ref());
    }

    fn empty_scope<'a>(
        unit_index: &'a UnitIndex,
        signature_index: &'a SignatureIndex,
        named: &'a IndexMap<String, Arc<LemmaType>>,
    ) -> MeasureScope<'a> {
        MeasureScope {
            unit_index,
            signature_index,
            named_types: named,
        }
    }

    #[test]
    fn arithmetic_rejects_non_commutative_invalid_orders() {
        let unit_index = UnitIndex::new();
        let signature_index = SignatureIndex::new();
        let named = IndexMap::new();
        let scope = empty_scope(&unit_index, &signature_index, &named);

        let date = Arc::new(LemmaType::primitive(TypeSpecification::date()));
        let time = Arc::new(LemmaType::primitive(TypeSpecification::time()));
        let duration = Arc::new(LemmaType::anonymous_for_decomposition(
            duration_decomposition(),
        ));
        let ratio = Arc::new(LemmaType::primitive(TypeSpecification::ratio()));
        let measure = Arc::new(LemmaType::new(
            "money".to_string(),
            TypeSpecification::measure(),
            TypeExtends::custom_local("measure".to_string(), test_type_family("money")),
        ));

        let duration_minus_date = compute_arithmetic_result_type(
            Arc::clone(&duration),
            &ArithmeticComputation::Subtract,
            Arc::clone(&date),
            &scope,
        );
        assert!(
            duration_minus_date.is_undetermined(),
            "duration - date must be Undetermined, got {}",
            duration_minus_date.name()
        );

        let time_minus_date = compute_arithmetic_result_type(
            Arc::clone(&time),
            &ArithmeticComputation::Subtract,
            Arc::clone(&date),
            &scope,
        );
        assert!(
            time_minus_date.is_undetermined(),
            "time - date must be Undetermined, got {}",
            time_minus_date.name()
        );

        let ratio_div_measure = compute_arithmetic_result_type(
            Arc::clone(&ratio),
            &ArithmeticComputation::Divide,
            Arc::clone(&measure),
            &scope,
        );
        assert!(
            ratio_div_measure.is_undetermined(),
            "ratio / measure must be Undetermined, got {}",
            ratio_div_measure.name()
        );

        let date_plus_time =
            compute_arithmetic_result_type(date, &ArithmeticComputation::Add, time, &scope);
        assert!(
            date_plus_time.is_undetermined(),
            "date + time must be Undetermined, got {}",
            date_plus_time.name()
        );
    }

    #[test]
    fn infer_helpers_return_undetermined_for_invalid_operands() {
        let number = Arc::new(LemmaType::primitive(TypeSpecification::number()));
        let text = Arc::new(LemmaType::primitive(TypeSpecification::text()));
        let measure = Arc::new(LemmaType::new(
            "money".to_string(),
            TypeSpecification::measure(),
            TypeExtends::custom_local("measure".to_string(), test_type_family("money")),
        ));
        let unit_index = UnitIndex::new();
        let signature_index = SignatureIndex::new();
        let named = IndexMap::new();
        let scope = empty_scope(&unit_index, &signature_index, &named);

        assert!(logical_not_type(&number).is_undetermined());
        assert!(comparison_type(
            &text,
            &crate::planning::semantics::ComparisonComputation::Is,
            &number
        )
        .is_undetermined());
        assert!(math_op_type(&MathematicalComputation::Sqrt, &measure, &scope).is_undetermined());
        assert!(date_predicate_type(&number).is_undetermined());
        assert!(past_future_range_type(&number).is_undetermined());
    }

    fn simple_base_measure(name: &str, family_name: &str) -> Arc<LemmaType> {
        let family = test_type_family(family_name);
        let mut spec = TypeSpecification::measure();
        if let TypeSpecification::Measure { decomposition, .. } = &mut spec {
            *decomposition = Some(
                [(
                    crate::planning::semantics::Dimension::Family(family.clone()),
                    1i32,
                )]
                .into_iter()
                .collect(),
            );
        }
        Arc::new(LemmaType::new(
            name.to_string(),
            spec,
            TypeExtends::custom_local(family_name.to_string(), family),
        ))
    }

    fn compound_measure(name: &str, family_name: &str) -> Arc<LemmaType> {
        let family = test_type_family(family_name);
        let other = test_type_family("other_axis");
        let mut spec = TypeSpecification::measure();
        if let TypeSpecification::Measure { decomposition, .. } = &mut spec {
            *decomposition = Some(
                [
                    (
                        crate::planning::semantics::Dimension::Family(family.clone()),
                        1i32,
                    ),
                    (crate::planning::semantics::Dimension::Family(other), -1i32),
                ]
                .into_iter()
                .collect(),
            );
        }
        Arc::new(LemmaType::new(
            name.to_string(),
            spec,
            TypeExtends::custom_local(family_name.to_string(), family),
        ))
    }

    #[test]
    fn unit_conversion_cross_family_relabel_needs_expression_unit() {
        let money = simple_base_measure("money", "money");
        let mass = simple_base_measure("mass", "mass");
        let target = SemanticConversionTarget::Unit {
            unit_name: "kg".to_string(),
            owning_type: Arc::clone(&mass),
        };
        let bare = unit_conversion_type(&money, &target);
        assert!(
            bare.is_undetermined(),
            "path-like measure without written unit must not relabel"
        );
        let stamped = Arc::new(money.as_ref().clone().with_unit("eur"));
        let relabelled = unit_conversion_type(&stamped, &target);
        assert_eq!(relabelled.name.as_deref(), Some("mass"));
        assert_eq!(relabelled.unit(), Some("kg"));
    }

    #[test]
    fn unit_conversion_compound_cannot_cross_family_relabel() {
        let rate = compound_measure("rate", "rate");
        let calendar = {
            let mut spec = TypeSpecification::measure();
            if let TypeSpecification::Measure {
                decomposition,
                traits,
                ..
            } = &mut spec
            {
                *decomposition = Some(crate::planning::semantics::calendar_decomposition());
                *traits = vec![crate::planning::semantics::MeasureTrait::Calendar];
            }
            Arc::new(LemmaType::new(
                "calendar".to_string(),
                spec,
                TypeExtends::Primitive,
            ))
        };
        let stamped = Arc::new(rate.as_ref().clone().with_unit("eur_per_month"));
        let target = SemanticConversionTarget::Unit {
            unit_name: "month".to_string(),
            owning_type: calendar,
        };
        assert!(unit_conversion_type(&stamped, &target).is_undetermined());
    }

    #[test]
    fn unit_conversion_calendar_cannot_become_duration_unit() {
        let mut calendar_spec = TypeSpecification::measure();
        if let TypeSpecification::Measure {
            decomposition,
            traits,
            ..
        } = &mut calendar_spec
        {
            *decomposition = Some(crate::planning::semantics::calendar_decomposition());
            *traits = vec![crate::planning::semantics::MeasureTrait::Calendar];
        }
        let calendar = Arc::new(
            LemmaType::new(
                "calendar".to_string(),
                calendar_spec,
                TypeExtends::Primitive,
            )
            .with_unit("month"),
        );
        let mut duration_spec = TypeSpecification::measure();
        if let TypeSpecification::Measure {
            decomposition,
            traits,
            ..
        } = &mut duration_spec
        {
            *decomposition = Some(crate::planning::semantics::duration_decomposition());
            *traits = vec![crate::planning::semantics::MeasureTrait::Duration];
        }
        let duration = Arc::new(LemmaType::new(
            "duration".to_string(),
            duration_spec,
            TypeExtends::Primitive,
        ));
        let target = SemanticConversionTarget::Unit {
            unit_name: "second".to_string(),
            owning_type: duration,
        };
        assert!(unit_conversion_type(&calendar, &target).is_undetermined());
    }

    #[test]
    fn unit_conversion_duration_trait_cannot_relabel_to_money() {
        let mut duration_spec = TypeSpecification::measure();
        if let TypeSpecification::Measure {
            decomposition,
            traits,
            ..
        } = &mut duration_spec
        {
            *decomposition = Some(crate::planning::semantics::duration_decomposition());
            *traits = vec![crate::planning::semantics::MeasureTrait::Duration];
        }
        let duration = Arc::new(
            LemmaType::new(
                "duration".to_string(),
                duration_spec,
                TypeExtends::Primitive,
            )
            .with_unit("hour"),
        );
        let money = simple_base_measure("money", "money");
        let target = SemanticConversionTarget::Unit {
            unit_name: "eur".to_string(),
            owning_type: money,
        };
        assert!(unit_conversion_type(&duration, &target).is_undetermined());
    }

    #[test]
    fn unit_conversion_measure_as_number_needs_expression_unit() {
        let money = simple_base_measure("money", "money");
        let bare = unit_conversion_type(
            &money,
            &SemanticConversionTarget::Type(ConversionTypeKind::Number),
        );
        assert!(bare.is_undetermined());
        let stamped = Arc::new(money.as_ref().clone().with_unit("eur"));
        let stripped = unit_conversion_type(
            &stamped,
            &SemanticConversionTarget::Type(ConversionTypeKind::Number),
        );
        assert!(stripped.is_number());
    }
}

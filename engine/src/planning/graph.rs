use crate::engine::Context;
use crate::literals::{MeasureUnit, MeasureUnits};
use crate::parsing::ast::{
    self as ast, CommandArg, Constraint, EffectiveDate, LemmaData, LemmaRepository, LemmaRule,
    LemmaSpec, ParentType, PrimitiveKind, TypeConstraintCommand, Value, WithRhs,
};
use crate::parsing::source::Source;
use crate::planning::discovery;
use crate::planning::semantics::{
    self, calendar_decomposition, canonicalize_signature, conversion_target_to_semantic,
    duration_decomposition, number_with_unit_to_value_kind, parser_value_to_value_kind,
    primitive_boolean_arc, primitive_date_arc, primitive_number_arc, primitive_ratio_arc,
    primitive_text_arc, primitive_time_arc, range_type_specification_from_endpoints,
    value_kind_from_raw_suggestion, value_kind_matches_spec, value_to_semantic,
    ArithmeticComputation, BaseMeasureVector, ComparisonComputation, DataDefinition, DataPath,
    Expression, ExpressionKind, LemmaType, LiteralValue, PathSegment, RawSuggestion, ReferenceEnd,
    ReferenceTarget, RulePath, SemanticConversionTarget, TypeDefiningSpec, TypeExtends,
    TypeSpecification, TypedLiteral, ValueKind,
};
use crate::planning::typing::{
    comparison_type, date_predicate_type, logical_and_type, logical_not_type, math_op_type,
    measure_range_matches_measure, past_future_range_type, piecewise_type,
    range_matches_measure_type, range_matches_range_measure, range_span_type, result_is_veto_type,
    unit_conversion_type, MeasureScope,
};
use crate::planning::unit_index::{UnitIndex, UnitMergeConflict, UnitOwner};
use crate::Error;

pub use crate::planning::typing::DecompositionMatch;
pub(crate) use crate::planning::typing::{
    compute_arithmetic_result_type, find_unique_measure_type_in_unit_index,
    infer_range_type_from_endpoint_types,
};
use ast::DataValue as ParsedDataValue;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fmt;
use std::sync::Arc;

// Filled for the duration of one graph type-inference + type-check pass so
// `infer_expression_type` does not re-walk a subtree that was already typed.
// Cleared when the pass ends. Keyed by expression pointer identity (stable for
// the life of the [`Graph`] being built).
thread_local! {
    static INFER_EXPRESSION_TYPE_CACHE: RefCell<Option<HashMap<usize, Arc<LemmaType>>>> =
        const { RefCell::new(None) };
}

fn with_infer_expression_type_cache<T>(f: impl FnOnce() -> T) -> T {
    INFER_EXPRESSION_TYPE_CACHE.with(|slot| {
        if slot.borrow().is_some() {
            panic!("BUG: infer_expression_type cache already installed (reentrant planning pass)");
        }
        *slot.borrow_mut() = Some(HashMap::new());
        let result = f();
        *slot.borrow_mut() = None;
        result
    })
}

fn clear_infer_expression_type_cache() {
    INFER_EXPRESSION_TYPE_CACHE.with(|slot| {
        let mut borrow = slot.borrow_mut();
        let cache = borrow
            .as_mut()
            .expect("BUG: clear_infer_expression_type_cache without an active cache");
        cache.clear();
    });
}

/// Data bindings map: maps a target data name path to the binding's value and source.
///
/// The key is the full path of **data names** from the root spec to the target data.
/// Spec set names are intentionally excluded from the key because spec ref bindings may change
/// which spec a segment points to — matching by data names only ensures bindings
/// are applied correctly regardless of spec ref bindings.
///
/// Example: `data employee.salary: 7500` in the root spec produces key `["employee", "salary"]`.
type DataBindings = HashMap<Vec<String>, (BindingValue, Source)>;

/// Binding value stored in [`DataBindings`]. Only two forms are valid for a
/// cross-spec binding: a literal value, or a reference to another data or rule.
///
/// References on the binding's right-hand side (e.g. `data license.other: law.other`)
/// are resolved at binding collection time against the spec in which the binding
/// itself was written (not the nested target spec). The resolved [`ReferenceTarget`]
/// is carried through so the nested spec's planning does not need the outer
/// spec's scope to interpret the reference.
#[derive(Debug, Clone)]
pub(crate) enum BindingValue {
    /// Literal RHS (parsed as a `Value`). Applied as a plain value to the bound data.
    Literal(ast::Value),
    /// Reference RHS pre-resolved to a concrete reference target.
    Reference {
        target: ReferenceTarget,
        constraints: Option<Vec<Constraint>>,
    },
}

#[derive(Debug)]
pub(crate) struct Graph<'a> {
    /// Root spec being planned (for error spec_context).
    main_spec: &'a LemmaSpec,
    data: IndexMap<DataPath, DataDefinition>,
    rules: BTreeMap<RulePath, RuleNode<'a>>,
    /// Rules in dependency order (topo).
    rule_order: Vec<RulePath>,
    /// Data→data references in dependency order (topo). Targets before dependents
    /// so chained references resolve. Refs to non-reference data are unordered
    /// among themselves.
    data_reference_order: Vec<DataPath>,
    /// Every [`DataDefinition::Reference`] path → where its chain ends.
    /// Filled in [`Self::validate`] after data-reference cycles are rejected.
    reference_ends: IndexMap<DataPath, ReferenceEnd>,
}

impl<'a> Graph<'a> {
    pub(crate) fn data(&self) -> &IndexMap<DataPath, DataDefinition> {
        &self.data
    }

    pub(crate) fn rules(&self) -> &BTreeMap<RulePath, RuleNode<'a>> {
        &self.rules
    }

    pub(crate) fn rules_mut(&mut self) -> &mut BTreeMap<RulePath, RuleNode<'a>> {
        &mut self.rules
    }

    pub(crate) fn rule_order(&self) -> &[RulePath] {
        &self.rule_order
    }

    pub(crate) fn data_reference_order(&self) -> &[DataPath] {
        &self.data_reference_order
    }

    pub(crate) fn reference_ends(&self) -> &IndexMap<DataPath, ReferenceEnd> {
        &self.reference_ends
    }

    pub(crate) fn main_spec(&self) -> &LemmaSpec {
        self.main_spec
    }

    /// Build the data map: one entry per data (Value or Import), with defaults and coercion applied.
    /// Preserves definition order from the source spec.
    pub(crate) fn build_data(
        &self,
        resolved_by_type_name: &IndexMap<String, Arc<LemmaType>>,
    ) -> Result<IndexMap<DataPath, DataDefinition>, Vec<Error>> {
        struct PendingReference {
            target: ReferenceTarget,
            resolved_type: Arc<LemmaType>,
            local_constraints: Option<Vec<Constraint>>,
            local_suggestion: Option<ValueKind>,
            local_fill: Option<ValueKind>,
        }

        let mut schema: HashMap<DataPath, Arc<LemmaType>> = HashMap::new();
        let mut declared_suggestions: HashMap<DataPath, ValueKind> = HashMap::new();
        let mut declared_fills: HashMap<DataPath, ValueKind> = HashMap::new();
        let mut values: HashMap<DataPath, LiteralValue> = HashMap::new();
        let mut value_sources: HashMap<DataPath, Source> = HashMap::new();
        let mut import_targets: HashMap<DataPath, String> = HashMap::new();
        let mut references: HashMap<DataPath, PendingReference> = HashMap::new();

        for (path, rfv) in self.data.iter() {
            match rfv {
                DataDefinition::Value {
                    value,
                    resolved_type,
                    source,
                } => {
                    values.insert(path.clone(), value.clone());
                    value_sources.insert(path.clone(), source.clone());
                    schema.insert(path.clone(), Arc::clone(resolved_type));
                }
                DataDefinition::TypeDeclaration {
                    resolved_type,
                    declared_suggestion,
                    declared_fill,
                    ..
                } => {
                    schema.insert(path.clone(), Arc::clone(resolved_type));
                    if let Some(dv) = declared_suggestion {
                        declared_suggestions.insert(path.clone(), dv.clone());
                    }
                    if let Some(dv) = declared_fill {
                        declared_fills.insert(path.clone(), dv.clone());
                    }
                }
                DataDefinition::Import { target_name, .. } => {
                    import_targets.insert(path.clone(), target_name.clone());
                }
                DataDefinition::Reference {
                    target,
                    resolved_type,
                    local_constraints,
                    local_suggestion,
                    local_fill,
                    ..
                } => {
                    schema.insert(path.clone(), Arc::clone(resolved_type));
                    references.insert(
                        path.clone(),
                        PendingReference {
                            target: target.clone(),
                            resolved_type: Arc::clone(resolved_type),
                            local_constraints: local_constraints.clone(),
                            local_suggestion: local_suggestion.clone(),
                            local_fill: local_fill.clone(),
                        },
                    );
                }
            }
        }

        let mut coercion_errors: Vec<Error> = Vec::new();
        for (path, value) in values.iter_mut() {
            let Some(schema_type) = schema.get(path).cloned() else {
                continue;
            };
            if let Some(type_name) = schema_type.name.as_deref() {
                if let Some(resolved) = resolved_by_type_name.get(type_name) {
                    if let Err(failure) = semantics::refresh_measure_literal_canonical_magnitude(
                        value,
                        schema_type.as_ref(),
                        resolved,
                    ) {
                        coercion_errors.push(Error::validation(
                            format!("Data '{path}' measure recanonicalization failed: {failure}"),
                            value_sources.get(path).cloned(),
                            None::<String>,
                        ));
                        continue;
                    }
                }
            }
            let typed = TypedLiteral {
                value: value.value.clone(),
                lemma_type: Arc::clone(&schema_type),
            };
            match Self::coerce_literal_to_schema_type(&typed, &schema_type) {
                Ok(coerced) => {
                    *value = coerced.to_literal();
                    schema.insert(path.clone(), Arc::clone(&coerced.lemma_type));
                }
                Err(msg) => {
                    coercion_errors.push(Error::validation(
                        format!("Data '{path}' incompatible with declared type: {msg}"),
                        value_sources.get(path).cloned(),
                        None::<String>,
                    ));
                }
            }
        }
        if !coercion_errors.is_empty() {
            return Err(coercion_errors);
        }

        let mut data = IndexMap::new();
        for (path, rfv) in &self.data {
            let source = rfv.source().clone();
            if let Some(target_name) = import_targets.remove(path) {
                data.insert(
                    path.clone(),
                    DataDefinition::Import {
                        target_name,
                        source,
                    },
                );
            } else if let Some(pending) = references.remove(path) {
                data.insert(
                    path.clone(),
                    DataDefinition::Reference {
                        target: pending.target,
                        resolved_type: pending.resolved_type,
                        local_constraints: pending.local_constraints,
                        local_suggestion: pending.local_suggestion,
                        local_fill: pending.local_fill,
                        source,
                    },
                );
            } else if let Some(value) = values.remove(path) {
                let resolved_type = schema
                    .remove(path)
                    .expect("BUG: value data must have schema type after coercion");
                data.insert(
                    path.clone(),
                    DataDefinition::Value {
                        value,
                        resolved_type,
                        source,
                    },
                );
            } else {
                let resolved_type = schema
                    .get(path)
                    .cloned()
                    .expect("non-spec-ref data has schema (value, reference, or type-only)");
                let declared_suggestion = declared_suggestions.remove(path);
                let declared_fill = declared_fills.remove(path);
                data.insert(
                    path.clone(),
                    DataDefinition::TypeDeclaration {
                        resolved_type,
                        declared_suggestion,
                        declared_fill,
                        source,
                    },
                );
            }
        }
        Ok(data)
    }

    /// Keep a measure literal's binding unit when replacing `lemma_type` with a schema type
    /// that has the same specifications (or compatible family).
    fn preserve_measure_binding(
        schema_type: &Arc<LemmaType>,
        previous: &LemmaType,
    ) -> Arc<LemmaType> {
        match &previous.measure_binding_unit {
            Some(unit) => Arc::new(
                schema_type
                    .as_ref()
                    .clone()
                    .with_measure_binding_unit(unit.clone()),
            ),
            None => Arc::clone(schema_type),
        }
    }

    pub(crate) fn coerce_literal_to_schema_type(
        lit: &TypedLiteral,
        schema_type: &Arc<LemmaType>,
    ) -> Result<TypedLiteral, String> {
        fn range_endpoint_schema_type(schema_type: &LemmaType) -> Option<Arc<LemmaType>> {
            schema_type
                .specifications
                .element_from_range()
                .map(|element_spec| Arc::new(LemmaType::primitive(element_spec)))
        }

        let schema_ref = schema_type.as_ref();
        if lit.lemma_type.specifications == schema_ref.specifications {
            if !value_kind_matches_spec(&lit.value, &schema_ref.specifications) {
                panic!(
                    "BUG: LiteralValue value kind {:?} inconsistent with lemma_type {:?}",
                    lit.value, lit.lemma_type.specifications
                );
            }
            if let ValueKind::Measure(_) = &lit.value {
                // Unit identity lives on lemma_type.measure_binding_unit when bound.
            }
            let mut out = lit.clone();
            out.lemma_type = Self::preserve_measure_binding(schema_type, &lit.lemma_type);
            return Ok(out);
        }
        match (&schema_ref.specifications, &lit.value) {
            (TypeSpecification::Number { .. }, ValueKind::Number(_))
            | (TypeSpecification::Text { .. }, ValueKind::Text(_))
            | (TypeSpecification::Boolean { .. }, ValueKind::Boolean(_))
            | (TypeSpecification::Date { .. }, ValueKind::Date(_))
            | (TypeSpecification::Time { .. }, ValueKind::Time(_)) => {
                let mut out = lit.clone();
                out.lemma_type = Arc::clone(schema_type);
                Ok(out)
            }
            (TypeSpecification::Measure { .. }, ValueKind::Measure(_)) => {
                if !lit.lemma_type.same_measure_family(schema_ref)
                    && !lit.lemma_type.compatible_with_anonymous_measure(schema_ref)
                    && lit.lemma_type.specifications != schema_ref.specifications
                {
                    return Err(format!(
                        "value {} cannot be used as type {}: incompatible measure families",
                        lit,
                        schema_ref.name()
                    ));
                }
                let mut out = lit.clone();
                out.lemma_type = Self::preserve_measure_binding(schema_type, &lit.lemma_type);
                Ok(out)
            }
            (TypeSpecification::Ratio { .. }, ValueKind::Ratio(_)) => {
                let mut out = lit.clone();
                out.lemma_type = Self::preserve_measure_binding(schema_type, &lit.lemma_type);
                Ok(out)
            }
            (
                TypeSpecification::NumberRange { .. }
                | TypeSpecification::DateRange { .. }
                | TypeSpecification::TimeRange { .. }
                | TypeSpecification::RatioRange { .. }
                | TypeSpecification::MeasureRange { .. },
                ValueKind::Range(left, right),
            ) => {
                let endpoint_schema_type =
                    range_endpoint_schema_type(schema_ref).unwrap_or_else(|| {
                        unreachable!("BUG: range_endpoint_schema_type missing range schema arm")
                    });
                let left_typed = TypedLiteral {
                    value: left.value.clone(),
                    lemma_type: Arc::clone(&endpoint_schema_type),
                };
                let right_typed = TypedLiteral {
                    value: right.value.clone(),
                    lemma_type: Arc::clone(&endpoint_schema_type),
                };
                let coerced_left =
                    Self::coerce_literal_to_schema_type(&left_typed, &endpoint_schema_type)?;
                let coerced_right =
                    Self::coerce_literal_to_schema_type(&right_typed, &endpoint_schema_type)?;
                Ok(TypedLiteral {
                    value: ValueKind::Range(
                        Box::new(coerced_left.to_literal()),
                        Box::new(coerced_right.to_literal()),
                    ),
                    lemma_type: Arc::clone(schema_type),
                })
            }
            (TypeSpecification::Ratio { .. }, ValueKind::Number(n)) => Ok(
                TypedLiteral::ratio_with_type(n.clone(), Arc::clone(schema_type)),
            ),
            _ => Err(format!(
                "value {} cannot be used as type {}",
                lit,
                schema_ref.name()
            )),
        }
    }

    /// Resolve each data-target [`DataDefinition::Reference`]'s provisional
    /// `resolved_type` into its final merged form by combining:
    ///   1. the target data's declared schema type,
    ///   2. any local `-> ...` constraints attached to the reference itself,
    ///   3. the LHS-declared type of the referencing data (when present; only
    ///      possible in a binding whose bound data has its own type
    ///      declaration in the nested spec).
    ///
    /// `ordered_reference_paths` is the dependency order from
    /// [`Self::compute_data_reference_order`] (targets before
    /// dependents). Each resolution is applied immediately so a reference
    /// whose target is itself a reference reads the target's final resolved
    /// type — batched application would leave chains `Undetermined`.
    ///
    /// Rule-target references are not in the order — they are resolved later
    /// in [`Self::resolve_rule_reference_types`] using the inferred rule
    /// type, which is only available after [`infer_rule_types`] has run.
    fn resolve_data_reference_types(
        &mut self,
        ordered_reference_paths: &[DataPath],
    ) -> Result<(), Vec<Error>> {
        let mut errors: Vec<Error> = Vec::new();

        for reference_path in ordered_reference_paths {
            let (target_data_path, provisional, local_constraints, source) =
                match self.data.get(reference_path) {
                    Some(DataDefinition::Reference {
                        target: ReferenceTarget::Data(path),
                        resolved_type,
                        local_constraints,
                        source,
                        ..
                    }) => (
                        path.clone(),
                        Arc::clone(resolved_type),
                        local_constraints.clone(),
                        source.clone(),
                    ),
                    _ => unreachable!(
                        "BUG: reference evaluation order must contain only data-target references"
                    ),
                };

            let Some(target_entry) = self.data.get(&target_data_path) else {
                errors.push(reference_error(
                    self.main_spec,
                    &source,
                    format!(
                        "Data reference '{}' target '{}' does not exist",
                        reference_path, target_data_path
                    ),
                ));
                continue;
            };

            let target_type_arc = match target_entry {
                DataDefinition::TypeDeclaration { resolved_type, .. }
                | DataDefinition::Reference { resolved_type, .. }
                | DataDefinition::Value { resolved_type, .. } => Arc::clone(resolved_type),
                DataDefinition::Import { .. } => {
                    errors.push(reference_error(
                        self.main_spec,
                        &source,
                        format!(
                            "Data reference '{}' target '{}' is a spec reference and cannot carry a value",
                            reference_path, target_data_path
                        ),
                    ));
                    continue;
                }
            };

            let lhs_declared_type: Option<&LemmaType> = if provisional.is_undetermined() {
                None
            } else {
                Some(provisional.as_ref())
            };

            if let Some(lhs) = lhs_declared_type {
                if let Some(msg) = reference_kind_mismatch_message(
                    lhs,
                    target_type_arc.as_ref(),
                    reference_path,
                    &target_data_path,
                    "target",
                ) {
                    errors.push(reference_error(self.main_spec, &source, msg));
                    continue;
                }
            }

            // Merge: prefer LHS-declared spec when present so child-declared
            // constraints (e.g. `maximum 5` from a binding's parent type
            // chain) are enforced on the copied value at run time. Without
            // a LHS-declared type, fall back to the target's spec.
            let mut merged = match lhs_declared_type {
                Some(lhs) => lhs.clone(),
                None => target_type_arc.as_ref().clone(),
            };
            let mut raw_suggestion: Option<RawSuggestion> = None;
            let mut raw_fill: Option<RawSuggestion> = None;
            if let Some(constraints) = &local_constraints {
                let constraint_type_name = merged.name();
                match apply_constraints_to_spec(
                    self.main_spec,
                    &constraint_type_name,
                    merged.specifications.clone(),
                    constraints,
                    &source,
                    &mut raw_suggestion,
                    &mut raw_fill,
                ) {
                    Ok(specs) => merged.specifications = specs,
                    Err(errs) => {
                        errors.extend(errs);
                        continue;
                    }
                }
            }
            let captured_suggestion = match raw_suggestion {
                None => None,
                Some(raw) => {
                    match value_kind_from_raw_suggestion(
                        raw,
                        &merged.specifications,
                        &merged.name(),
                    ) {
                        Ok(vk) => Some(vk),
                        Err(message) => {
                            errors.push(reference_error(self.main_spec, &source, message));
                            continue;
                        }
                    }
                }
            };
            let captured_fill = match raw_fill {
                None => None,
                Some(raw) => {
                    match value_kind_from_raw_suggestion(
                        raw,
                        &merged.specifications,
                        &merged.name(),
                    ) {
                        Ok(vk) => Some(vk),
                        Err(message) => {
                            errors.push(reference_error(self.main_spec, &source, message));
                            continue;
                        }
                    }
                }
            };

            // Apply immediately: later references in the order may target
            // this one and must read the final merged type.
            if let Some(DataDefinition::Reference {
                resolved_type,
                local_suggestion,
                local_fill,
                ..
            }) = self.data.get_mut(reference_path)
            {
                *resolved_type = Arc::new(merged);
                if captured_suggestion.is_some() {
                    *local_suggestion = captured_suggestion;
                }
                if captured_fill.is_some() {
                    *local_fill = captured_fill;
                }
            } else {
                unreachable!("BUG: reference path disappeared during type resolution");
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Resolve each rule-target [`DataDefinition::Reference`]'s `resolved_type`
    /// from the inferred type of the target rule. Applies the same LHS-vs-target
    /// kind compatibility check and local `-> ...` constraint merge that
    /// [`Self::resolve_data_reference_types`] applies to data-target references.
    ///
    /// Must run AFTER [`infer_rule_types`] so each target rule's inferred type
    /// is available, and BEFORE [`check_rule_types`] so consumers see the
    /// merged reference type during validation.
    fn resolve_rule_reference_types(
        &mut self,
        computed_rule_types: &HashMap<RulePath, Arc<LemmaType>>,
    ) -> Result<(), Vec<Error>> {
        let mut errors: Vec<Error> = Vec::new();
        let mut updates: Vec<RuleReferenceUpdate> = Vec::new();

        for (reference_path, entry) in &self.data {
            let DataDefinition::Reference {
                target,
                resolved_type: provisional,
                local_constraints,
                source,
                ..
            } = entry
            else {
                continue;
            };

            let target_rule_path = match target {
                ReferenceTarget::Rule(path) => path,
                ReferenceTarget::Data(_) => continue,
            };

            let Some(target_type) = computed_rule_types.get(target_rule_path) else {
                errors.push(reference_error(
                    self.main_spec,
                    source,
                    format!(
                        "Data reference '{}' target rule '{}' does not exist",
                        reference_path, target_rule_path
                    ),
                ));
                continue;
            };

            // A target rule whose inferred type is `veto` carries no concrete
            // schema kind, so a LHS declared type cannot be checked against
            // it at planning time. The runtime veto propagation in the
            // evaluator will surface the rule's veto reason directly.
            if target_type.vetoed() || target_type.is_undetermined() {
                let mut merged = target_type.as_ref().clone();
                let mut raw_suggestion: Option<RawSuggestion> = None;
                let mut raw_fill: Option<RawSuggestion> = None;
                if let Some(constraints) = local_constraints {
                    let constraint_type_name = merged.name();
                    match apply_constraints_to_spec(
                        self.main_spec,
                        &constraint_type_name,
                        merged.specifications.clone(),
                        constraints,
                        source,
                        &mut raw_suggestion,
                        &mut raw_fill,
                    ) {
                        Ok(specs) => merged.specifications = specs,
                        Err(errs) => {
                            errors.extend(errs);
                            continue;
                        }
                    }
                }
                let captured_suggestion = match raw_suggestion {
                    None => None,
                    Some(raw) => {
                        match value_kind_from_raw_suggestion(
                            raw,
                            &merged.specifications,
                            &merged.name(),
                        ) {
                            Ok(vk) => Some(vk),
                            Err(message) => {
                                errors.push(reference_error(self.main_spec, source, message));
                                continue;
                            }
                        }
                    }
                };
                let captured_fill = match raw_fill {
                    None => None,
                    Some(raw) => {
                        match value_kind_from_raw_suggestion(
                            raw,
                            &merged.specifications,
                            &merged.name(),
                        ) {
                            Ok(vk) => Some(vk),
                            Err(message) => {
                                errors.push(reference_error(self.main_spec, source, message));
                                continue;
                            }
                        }
                    }
                };
                updates.push((
                    reference_path.clone(),
                    Arc::new(merged),
                    captured_suggestion,
                    captured_fill,
                ));
                continue;
            }

            let lhs_declared_type: Option<&LemmaType> = if provisional.is_undetermined() {
                None
            } else {
                Some(provisional.as_ref())
            };

            if let Some(lhs) = lhs_declared_type {
                if let Some(msg) = reference_kind_mismatch_message(
                    lhs,
                    target_type,
                    reference_path,
                    target_rule_path,
                    "target rule",
                ) {
                    errors.push(reference_error(self.main_spec, source, msg));
                    continue;
                }
            }

            // Prefer LHS-declared spec when present (see data-target merge
            // for rationale).
            let mut merged = match lhs_declared_type {
                Some(lhs) => lhs.clone(),
                None => target_type.as_ref().clone(),
            };
            let mut raw_suggestion: Option<RawSuggestion> = None;
            let mut raw_fill: Option<RawSuggestion> = None;
            if let Some(constraints) = local_constraints {
                let constraint_type_name = merged.name();
                match apply_constraints_to_spec(
                    self.main_spec,
                    &constraint_type_name,
                    merged.specifications.clone(),
                    constraints,
                    source,
                    &mut raw_suggestion,
                    &mut raw_fill,
                ) {
                    Ok(specs) => merged.specifications = specs,
                    Err(errs) => {
                        errors.extend(errs);
                        continue;
                    }
                }
            }
            let captured_suggestion = match raw_suggestion {
                None => None,
                Some(raw) => {
                    match value_kind_from_raw_suggestion(
                        raw,
                        &merged.specifications,
                        &merged.name(),
                    ) {
                        Ok(vk) => Some(vk),
                        Err(message) => {
                            errors.push(reference_error(self.main_spec, source, message));
                            continue;
                        }
                    }
                }
            };
            let captured_fill = match raw_fill {
                None => None,
                Some(raw) => {
                    match value_kind_from_raw_suggestion(
                        raw,
                        &merged.specifications,
                        &merged.name(),
                    ) {
                        Ok(vk) => Some(vk),
                        Err(message) => {
                            errors.push(reference_error(self.main_spec, source, message));
                            continue;
                        }
                    }
                }
            };

            updates.push((
                reference_path.clone(),
                Arc::new(merged),
                captured_suggestion,
                captured_fill,
            ));
        }

        for (path, new_type, new_suggestion, new_fill) in updates {
            if let Some(DataDefinition::Reference {
                resolved_type,
                local_suggestion,
                local_fill,
                ..
            }) = self.data.get_mut(&path)
            {
                *resolved_type = new_type;
                if new_suggestion.is_some() {
                    *local_suggestion = new_suggestion;
                }
                if new_fill.is_some() {
                    *local_fill = new_fill;
                }
            } else {
                unreachable!(
                    "BUG: rule-target reference path disappeared between collect and update phases"
                );
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Add a `depends_on_rules` edge from every rule that reads a rule-target
    /// reference data path to the reference's target rule. Planning needs the
    /// target in `completed_rules` (NormalFormId) before the consumer lowers,
    /// and topological sort must detect cycles that flow through reference paths.
    ///
    /// Walks data-target reference chains so that a path `y: m.x` where
    /// `m.x: r` is a rule-target reference, still adds a dep edge from any
    /// consumer of `y` to `r`.
    fn add_rule_reference_dependency_edges(&mut self) {
        if self.reference_ends.is_empty() {
            return;
        }

        let mut updates: Vec<(RulePath, RulePath)> = Vec::new();
        for (rule_path, rule_node) in &self.rules {
            let mut found: BTreeSet<RulePath> = BTreeSet::new();
            for (cond, result) in &rule_node.branches {
                if let Some(c) = cond {
                    collect_rule_reference_dependencies(c, &self.reference_ends, &mut found);
                }
                collect_rule_reference_dependencies(result, &self.reference_ends, &mut found);
            }
            for target in found {
                updates.push((rule_path.clone(), target));
            }
        }

        for (rule_path, target) in updates {
            if let Some(node) = self.rules.get_mut(&rule_path) {
                node.depends_on_rules.insert(target);
            }
        }
    }

    /// For each [`DataDefinition::Reference`] in `self.data`, follow the chain to
    /// its end (promptable slot, rule, or import). Cycles among data-target
    /// references are impossible here because `compute_data_reference_order`
    /// already rejected them.
    fn compute_reference_ends(&self) -> IndexMap<DataPath, ReferenceEnd> {
        let mut out: IndexMap<DataPath, ReferenceEnd> = IndexMap::new();
        for (path, def) in &self.data {
            let DataDefinition::Reference { target, .. } = def else {
                continue;
            };
            match target {
                ReferenceTarget::Rule(rule_path) => {
                    out.insert(path.clone(), ReferenceEnd::Rule(rule_path.clone()));
                }
                ReferenceTarget::Data(start) => {
                    let mut cursor = start.clone();
                    loop {
                        match self.data.get(&cursor) {
                            Some(
                                DataDefinition::Value { .. }
                                | DataDefinition::TypeDeclaration { .. },
                            ) => {
                                out.insert(path.clone(), ReferenceEnd::Promptable(cursor));
                                break;
                            }
                            Some(DataDefinition::Import { .. }) => {
                                out.insert(path.clone(), ReferenceEnd::Import);
                                break;
                            }
                            Some(DataDefinition::Reference {
                                target: ReferenceTarget::Rule(rule_path),
                                ..
                            }) => {
                                out.insert(path.clone(), ReferenceEnd::Rule(rule_path.clone()));
                                break;
                            }
                            Some(DataDefinition::Reference {
                                target: ReferenceTarget::Data(next),
                                ..
                            }) => {
                                cursor = next.clone();
                            }
                            None => {
                                panic!(
                                    "BUG: data-target reference chain from '{path}' ends at missing data '{cursor}'"
                                );
                            }
                        }
                    }
                }
            }
        }
        out
    }

    /// Compute an order in which data-target references can be evaluated at
    /// runtime so each reference's target (when itself a reference) has been
    /// evaluated first. Rule-target references are intentionally excluded —
    /// they are resolved lazily on first read in the evaluator from the
    /// already-evaluated target rule's result. Cycles among data-target
    /// references are reported as planning errors.
    fn compute_data_reference_order(&self) -> Result<Vec<DataPath>, Vec<Error>> {
        let reference_paths: Vec<DataPath> = self
            .data
            .iter()
            .filter_map(|(p, d)| match d {
                DataDefinition::Reference {
                    target: ReferenceTarget::Data(_),
                    ..
                } => Some(p.clone()),
                _ => None,
            })
            .collect();

        if reference_paths.is_empty() {
            return Ok(Vec::new());
        }

        let reference_set: BTreeSet<DataPath> = reference_paths.iter().cloned().collect();
        let mut in_degree: BTreeMap<DataPath, usize> = BTreeMap::new();
        let mut dependents: BTreeMap<DataPath, Vec<DataPath>> = BTreeMap::new();
        for p in &reference_paths {
            in_degree.insert(p.clone(), 0);
            dependents.insert(p.clone(), Vec::new());
        }

        for p in &reference_paths {
            let Some(DataDefinition::Reference { target, .. }) = self.data.get(p) else {
                unreachable!("BUG: reference entry lost between collect and walk");
            };
            if let ReferenceTarget::Data(target_path) = target {
                if reference_set.contains(target_path) {
                    *in_degree
                        .get_mut(p)
                        .expect("BUG: reference missing in_degree") += 1;
                    dependents
                        .get_mut(target_path)
                        .expect("BUG: reference missing dependents list")
                        .push(p.clone());
                }
            }
        }

        let mut queue: VecDeque<DataPath> = in_degree
            .iter()
            .filter(|(_, d)| **d == 0)
            .map(|(p, _)| p.clone())
            .collect();

        let mut result: Vec<DataPath> = Vec::new();
        while let Some(path) = queue.pop_front() {
            result.push(path.clone());
            if let Some(deps) = dependents.get(&path) {
                for dependent in deps.clone() {
                    let degree = in_degree
                        .get_mut(&dependent)
                        .expect("BUG: reference dependent missing in_degree");
                    *degree -= 1;
                    if *degree == 0 {
                        queue.push_back(dependent);
                    }
                }
            }
        }

        if result.len() != reference_paths.len() {
            let cycle_members: Vec<DataPath> = reference_paths
                .iter()
                .filter(|p| !result.contains(p))
                .cloned()
                .collect();
            let cycle_display: String = cycle_members
                .iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            let errors: Vec<Error> = cycle_members
                .iter()
                .filter_map(|p| {
                    self.data.get(p).map(|entry| {
                        reference_error(
                            self.main_spec,
                            entry.source(),
                            format!("Circular data reference ({})", cycle_display),
                        )
                    })
                })
                .collect();
            return Err(errors);
        }

        Ok(result)
    }

    fn topological_sort(&self) -> Result<Vec<RulePath>, Vec<Error>> {
        let mut in_degree: BTreeMap<RulePath, usize> = BTreeMap::new();
        let mut dependents: BTreeMap<RulePath, Vec<RulePath>> = BTreeMap::new();
        let mut queue = VecDeque::new();
        let mut result = Vec::new();

        for rule_path in self.rules.keys() {
            in_degree.insert(rule_path.clone(), 0);
            dependents.insert(rule_path.clone(), Vec::new());
        }

        for (rule_path, rule_node) in &self.rules {
            for dependency in &rule_node.depends_on_rules {
                if self.rules.contains_key(dependency) {
                    if let Some(degree) = in_degree.get_mut(rule_path) {
                        *degree += 1;
                    }
                    if let Some(deps) = dependents.get_mut(dependency) {
                        deps.push(rule_path.clone());
                    }
                }
            }
        }

        for (rule_path, degree) in &in_degree {
            if *degree == 0 {
                queue.push_back(rule_path.clone());
            }
        }

        while let Some(rule_path) = queue.pop_front() {
            result.push(rule_path.clone());

            if let Some(dependent_rules) = dependents.get(&rule_path) {
                for dependent in dependent_rules {
                    if let Some(degree) = in_degree.get_mut(dependent) {
                        *degree -= 1;
                        if *degree == 0 {
                            queue.push_back(dependent.clone());
                        }
                    }
                }
            }
        }

        if result.len() != self.rules.len() {
            let missing: Vec<RulePath> = self
                .rules
                .keys()
                .filter(|rule| !result.contains(rule))
                .cloned()
                .collect();
            let cycle: Vec<Source> = missing
                .iter()
                .filter_map(|rule| self.rules.get(rule).map(|n| n.source.clone()))
                .collect();

            if cycle.is_empty() {
                unreachable!(
                    "BUG: circular dependency detected but no sources could be collected ({} missing rules)",
                    missing.len()
                );
            }
            let rules_involved: String = missing
                .iter()
                .map(|rp| rp.rule.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let message = format!("Circular dependency (rules: {})", rules_involved);
            let errors: Vec<Error> = cycle
                .into_iter()
                .map(|source| {
                    Error::validation_with_context(
                        message.clone(),
                        Some(source),
                        None::<String>,
                        Some(self.main_spec),
                        None,
                    )
                })
                .collect();
            return Err(errors);
        }

        Ok(result)
    }
}

#[derive(Debug)]
pub(crate) struct RuleNode<'a> {
    /// First branch has condition=None (default expression), subsequent branches are unless clauses.
    /// Resolved expressions (Reference -> DataPath or RulePath).
    pub branches: Vec<(Option<Expression>, Expression)>,
    pub source: Source,

    pub depends_on_rules: BTreeSet<RulePath>,

    /// Computed type of this rule's result (populated during validation)
    /// Every rule MUST have a type (Lemma is strictly typed)
    pub rule_type: Arc<LemmaType>,

    /// Spec this rule belongs to (for type resolution during validation)
    pub spec: &'a LemmaSpec,
}

type ResolvedTypesMap<'a> = Vec<(Arc<LemmaRepository>, &'a LemmaSpec, ResolvedSpecTypes)>;

/// Parent lookup result: specs, inherited suggest, inherited fill, parent type.
type ParentLookupOk = (
    TypeSpecification,
    Option<RawSuggestion>,
    Option<RawSuggestion>,
    Arc<LemmaType>,
);

/// Local resolved type entry during topo-sort: type, suggest, fill.
type ResolvedTypeEntry = (Arc<LemmaType>, Option<RawSuggestion>, Option<RawSuggestion>);

/// Unqualified import type match: alias, type, suggest, fill.
type ImportedTypeMatch<'a> = (
    &'a str,
    Arc<LemmaType>,
    Option<RawSuggestion>,
    Option<RawSuggestion>,
);

/// Pending rule-reference type merge: path, type, local suggest, local fill.
type RuleReferenceUpdate = (
    DataPath,
    Arc<LemmaType>,
    Option<ValueKind>,
    Option<ValueKind>,
);

/// Ok payload of [`GraphBuilder::build`]: data, rules, soft errors, resolved types.
type GraphBuildOk<'a> = (
    IndexMap<DataPath, DataDefinition>,
    BTreeMap<RulePath, RuleNode<'a>>,
    Vec<Error>,
    ResolvedTypesMap<'a>,
);

struct GraphBuilder<'a> {
    data: IndexMap<DataPath, DataDefinition>,
    rules: BTreeMap<RulePath, RuleNode<'a>>,
    context: &'a Context,
    local_types: ResolvedTypesMap<'a>,
    errors: Vec<Error>,
    main_spec: &'a LemmaSpec,
    main_repository: Arc<ast::LemmaRepository>,
    limits: &'a crate::limits::ResourceLimits,
}

struct RuleExpressionConversion<'a, 'b> {
    spec: &'a LemmaSpec,
    data_map: &'b HashMap<String, &'b LemmaData>,
    segments: &'b [PathSegment],
    rule_names: &'b HashSet<&'b str>,
    effective: &'b EffectiveDate,
    depends_on_rules: &'b mut BTreeSet<RulePath>,
}

fn reference_error(main_spec: &LemmaSpec, source: &Source, message: String) -> Error {
    Error::validation_with_context(
        message,
        Some(source.clone()),
        None::<String>,
        Some(main_spec),
        None,
    )
}

/// Decide whether an LHS-declared reference type and the resolved target type
/// share a compatible kind. Returns `None` when they do; returns `Some(msg)`
/// describing the mismatch otherwise.
///
/// "Same kind" requires:
/// 1. matching base type spec (number / measure / text / ratio / …) — see
///    [`LemmaType::has_same_base_type`]; and
/// 2. for measure and measure-range types, matching measure family — see
///    [`LemmaType::same_measure_family`] (ranges via [`range_span_type`]
///    element). Two quantities in different families (e.g. `eur` vs
///    `celsius`) share the `Measure` discriminant but are not interchangeable
///    values; copying one into the other would silently propagate a
///    wrong-domain measure. Ratio / ratio-range refs stay base-kind only.
///
/// `target_kind_label` distinguishes the two callers ("target" for data
/// references, "target rule" for rule references) so the message reads
/// naturally.
fn reference_kind_mismatch_message<P: fmt::Display>(
    lhs: &LemmaType,
    target_type: &LemmaType,
    reference_path: &DataPath,
    target_path: &P,
    target_kind_label: &str,
) -> Option<String> {
    if !lhs.has_same_base_type(target_type) {
        return Some(format!(
            "Data reference '{}' type mismatch: declared as '{}' but {} '{}' is '{}'",
            reference_path,
            lhs.name(),
            target_kind_label,
            target_path,
            target_type.name(),
        ));
    }

    let lhs_span;
    let target_span;
    let (lhs_measure, target_measure): (&LemmaType, &LemmaType) = if lhs.is_measure() {
        (lhs, target_type)
    } else if lhs.is_measure_range() {
        lhs_span = range_span_type(lhs);
        target_span = range_span_type(target_type);
        if lhs_span.is_undetermined() || target_span.is_undetermined() {
            unreachable!(
                "BUG: MeasureRange reference kinds must yield measure elements via range_span_type"
            );
        }
        (lhs_span.as_ref(), target_span.as_ref())
    } else {
        return None;
    };

    if lhs_measure.same_measure_family(target_measure) {
        return None;
    }
    let lhs_family = lhs_measure.measure_family_name().expect(
        "BUG: declared measure data must carry a family name; \
         anonymous measure types only arise from runtime synthesis \
         and never appear as a reference's LHS-declared type",
    );
    let target_family = target_measure.measure_family_name().expect(
        "BUG: declared measure data must carry a family name; \
         anonymous measure types only arise from runtime synthesis \
         and never appear as a reference target's schema type",
    );
    Some(format!(
        "Data reference '{}' measure family mismatch: declared as '{}' (family '{}') but {} '{}' is '{}' (family '{}')",
        reference_path,
        lhs.name(),
        lhs_family,
        target_kind_label,
        target_path,
        target_type.name(),
        target_family,
    ))
}

/// Type name shown in `-> suggest` constraint errors (the declared type, not the data slot).
fn constraint_application_type_name(parent: &ParentType, data_name: &str) -> String {
    match parent {
        ParentType::Custom { name } => name.clone(),
        ParentType::Qualified { inner, .. } => constraint_application_type_name(inner, data_name),
        ParentType::Ranged { inner, .. } => constraint_application_type_name(inner, data_name),
        ParentType::Primitive { .. } => data_name.to_string(),
    }
}

/// Named ranged types (`cargo_mass range`, `score range`) refresh from the element after
/// decomposition. Apply their constraints only after that refresh so units/decomp exist and
/// range-local bounds overwrite inherited endpoints as last writer.
fn should_defer_ranged_constraints(parent: &ParentType) -> bool {
    matches!(parent, ParentType::Ranged { .. })
        && matches!(
            element_parent_type(parent),
            ParentType::Custom { .. } | ParentType::Qualified { .. }
        )
}

/// Fold a list of definition-style constraints into a [`TypeSpecification`].
/// Used for both the GraphBuilder's regular TypeDeclaration path and the
/// post-build reference type-merging pass, so the underlying constraint
/// application logic stays in one place.
fn apply_constraints_to_spec(
    spec: &LemmaSpec,
    type_name: &str,
    mut specs: TypeSpecification,
    constraints: &[Constraint],
    source: &crate::parsing::source::Source,
    declared_suggestion: &mut Option<RawSuggestion>,
    declared_fill: &mut Option<RawSuggestion>,
) -> Result<TypeSpecification, Vec<Error>> {
    let mut errors = Vec::new();

    let mut seen_unit_names: std::collections::HashSet<String> = Default::default();
    let mut seen_singleton_commands: std::collections::HashSet<TypeConstraintCommand> =
        Default::default();
    let mut has_suggest = false;
    let mut has_fill = false;
    for row in constraints.iter() {
        let command = row.command;
        let args = &row.args;
        if command == TypeConstraintCommand::Suggest {
            has_suggest = true;
        }
        if command == TypeConstraintCommand::Fill {
            has_fill = true;
        }
        if command == TypeConstraintCommand::Unit {
            if let Some(CommandArg::Label(name)) = args.first() {
                if !seen_unit_names.insert(name.clone()) {
                    errors.push(Error::validation_with_context(
                        format!(
                            "Duplicate unit '{}': each unit name may appear at most once per type definition",
                            name
                        ),
                        Some(source.clone()),
                        None::<String>,
                        Some(spec),
                        None,
                    ));
                }
            }
        }
        if matches!(
            command,
            TypeConstraintCommand::Minimum
                | TypeConstraintCommand::Maximum
                | TypeConstraintCommand::Decimals
                | TypeConstraintCommand::Suggest
                | TypeConstraintCommand::Fill
        ) && !seen_singleton_commands.insert(command)
        {
            errors.push(Error::validation_with_context(
                format!(
                    "Duplicate '{command}' constraint: each may appear at most once per type declaration"
                ),
                Some(source.clone()),
                None::<String>,
                Some(spec),
                None,
            ));
        }
    }
    if has_suggest && has_fill {
        errors.push(Error::validation_with_context(
            "Cannot use both 'fill' and 'suggest' on the same data declaration".to_string(),
            Some(source.clone()),
            None::<String>,
            Some(spec),
            None,
        ));
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let mut apply_one = |specs: &mut TypeSpecification,
                         command: TypeConstraintCommand,
                         args: &[CommandArg],
                         declared_suggestion: &mut Option<RawSuggestion>,
                         declared_fill: &mut Option<RawSuggestion>| {
        let mut suggestion_before = declared_suggestion.clone();
        let mut fill_before = declared_fill.clone();
        match specs.apply_constraint(
            type_name,
            command,
            args,
            &mut suggestion_before,
            &mut fill_before,
        ) {
            Ok(()) => {
                *declared_suggestion = suggestion_before;
                *declared_fill = fill_before;
            }
            Err(e) => {
                // Commands fail before writing; specs is unchanged. No success-path clone.
                errors.push(Error::validation_with_context(
                    format!("Failed to apply constraint '{}': {}", command, e),
                    Some(source.clone()),
                    None::<String>,
                    Some(spec),
                    None,
                ));
            }
        }
    };

    for row in constraints {
        let command = row.command;
        let args = &row.args;
        if matches!(
            command,
            TypeConstraintCommand::Unit | TypeConstraintCommand::Trait
        ) {
            apply_one(
                &mut specs,
                command,
                args,
                declared_suggestion,
                declared_fill,
            );
        }
    }
    for row in constraints {
        let command = row.command;
        let args = &row.args;
        if !matches!(
            command,
            TypeConstraintCommand::Unit | TypeConstraintCommand::Trait
        ) {
            apply_one(
                &mut specs,
                command,
                args,
                declared_suggestion,
                declared_fill,
            );
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(specs)
}

/// Whether `expr` and every nested sub-expression carry a `source_location`.
///
/// ASTs produced by the parser always carry locations; this guards the
/// programmatic-AST boundary (external consumers constructing ASTs directly).
fn expression_has_source_locations(expr: &ast::Expression) -> bool {
    if expr.source_location.is_none() {
        return false;
    }
    match &expr.kind {
        ast::ExpressionKind::Literal(_)
        | ast::ExpressionKind::Reference(_)
        | ast::ExpressionKind::Now
        | ast::ExpressionKind::Veto(_) => true,
        ast::ExpressionKind::DateRelative(_, e)
        | ast::ExpressionKind::DateCalendar(_, _, e)
        | ast::ExpressionKind::PastFutureRange(_, e)
        | ast::ExpressionKind::UnitConversion(e, _)
        | ast::ExpressionKind::LogicalNegation(e, _)
        | ast::ExpressionKind::MathematicalComputation(_, e)
        | ast::ExpressionKind::ResultIsVeto(e) => expression_has_source_locations(e),
        ast::ExpressionKind::RangeLiteral(l, r)
        | ast::ExpressionKind::RangeContainment(l, r)
        | ast::ExpressionKind::LogicalAnd(l, r)
        | ast::ExpressionKind::Arithmetic(l, _, r)
        | ast::ExpressionKind::Comparison(l, _, r) => {
            expression_has_source_locations(l) && expression_has_source_locations(r)
        }
    }
}

/// Boundary validation for programmatically constructed ASTs: every expression
/// in every spec of the ordered dependency list must carry a `source_location`.
/// Parser output always does; a miss means an external consumer built an invalid
/// AST, which is an `Err(Error)` at this system boundary (not a panic).
fn validate_ast_source_locations(
    ordered_dependencies: &[discovery::DependencySpec<'_>],
) -> Vec<Error> {
    let mut errors = Vec::new();
    for node in ordered_dependencies {
        let spec = node.spec;
        for rule in &spec.rules {
            if !expression_has_source_locations(&rule.expression) {
                errors.push(Error::validation(
                    format!(
                        "In spec '{}': rule '{}' contains an expression without a source location; \
                         programmatically constructed ASTs must set source_location on every expression",
                        spec.name, rule.name
                    ),
                    Some(rule.source_location.clone()),
                    None::<String>,
                ));
            }
            for clause in &rule.unless_clauses {
                if !expression_has_source_locations(&clause.condition)
                    || !expression_has_source_locations(&clause.result)
                {
                    errors.push(Error::validation(
                        format!(
                            "In spec '{}': rule '{}' has an unless clause with an expression without \
                             a source location; programmatically constructed ASTs must set \
                             source_location on every expression",
                            spec.name, rule.name
                        ),
                        Some(clause.source_location.clone()),
                        None::<String>,
                    ));
                }
            }
        }
    }
    errors
}

impl<'a> Graph<'a> {
    /// Build the typed rule/data graph for main_spec using its ordered dependencies.
    pub(crate) fn build(
        context: &'a Context,
        repository: &Arc<LemmaRepository>,
        main_spec: &'a LemmaSpec,
        ordered_dependencies: &[discovery::DependencySpec<'a>],
        effective: &EffectiveDate,
        limits: &'a crate::limits::ResourceLimits,
    ) -> Result<(Graph<'a>, ResolvedSpecTypes), Vec<Error>> {
        let boundary_errors = validate_ast_source_locations(ordered_dependencies);
        if !boundary_errors.is_empty() {
            return Err(boundary_errors);
        }

        let mut errors: Vec<Error> = Vec::new();

        let mut type_resolver = TypeResolver::new(context);
        for node in ordered_dependencies {
            errors.extend(type_resolver.register_all(&node.repository, node.spec));
        }

        let (data, rules, graph_errors, local_types) = GraphBuilder::build(
            context,
            repository,
            main_spec,
            effective,
            &type_resolver,
            limits,
        )?;

        let mut graph = Graph {
            data,
            rules,
            rule_order: Vec::new(),
            data_reference_order: Vec::new(),
            reference_ends: IndexMap::new(),
            main_spec,
        };

        let validation_errors = match graph.validate(&local_types) {
            Ok(()) => Vec::new(),
            Err(errors) => errors,
        };

        errors.extend(graph_errors);
        errors.extend(validation_errors);

        if errors.is_empty() {
            let main_resolved_types = local_types
                .into_iter()
                .find_map(|(_, spec, types)| {
                    discovery::same_loaded_spec(spec, main_spec).then_some(types)
                })
                .expect("BUG: main spec missing from local_types");
            Ok((graph, main_resolved_types))
        } else {
            Err(errors)
        }
    }

    fn validate(&mut self, resolved_types: &ResolvedTypesMap) -> Result<(), Vec<Error>> {
        let mut errors = Vec::new();

        // Structural checks (no type info needed)
        if let Err(structural_errors) = check_all_rule_references_exist(self) {
            errors.extend(structural_errors);
        }
        if let Err(collision_errors) = check_data_and_rule_name_collisions(self) {
            errors.extend(collision_errors);
        }

        // Compute the data-target reference evaluation (copy) order first: it
        // is both the type-resolution order for Phase 1 (targets before
        // dependents, so reference→reference chains resolve) and the runtime
        // prepop copy order. Rule-target references are resolved lazily at
        // evaluation time — they do not participate in either.
        let reference_order = match self.compute_data_reference_order() {
            Ok(order) => order,
            Err(circular_errors) => {
                errors.extend(circular_errors);
                return Err(errors);
            }
        };

        // Phase 1: Resolve data-target reference types now that all data
        // definitions (across all specs) are populated. Rule-target references
        // are resolved in Phase 4 once the target rule's type is inferred.
        if let Err(reference_errors) = self.resolve_data_reference_types(&reference_order) {
            errors.extend(reference_errors);
        }

        // Resolve every Reference chain end once. Cycles were just rejected.
        // Used for rule-dep edges, normalize embeds, show promptability, eval guard.
        self.reference_ends = self.compute_reference_ends();

        // Phase 2: Inject rule-rule dependency edges for rule-target references.
        // A rule R that reads a data path D where D is `Reference(target: rule T)`
        // must be evaluated AFTER T so the lazy resolver can read T's result.
        // This must happen before topological_sort so cycles through reference
        // paths are detected.
        self.add_rule_reference_dependency_edges();

        let rule_order = match self.topological_sort() {
            Ok(order) => order,
            Err(circular_errors) => {
                errors.extend(circular_errors);
                return Err(errors);
            }
        };

        // Continue to type inference and type checking even when structural
        // checks found errors.  This lets us report structural errors (e.g.
        // missing rule reference) alongside type errors (e.g. branch type
        // mismatch) in a single pass.

        // Phase 3–5 share one infer cache for walk reuse within a phase. Phase 4
        // mutates rule-target `resolved_type` (constraint merge), so the cache is
        // cleared before phase 5 re-infers against the merged types.
        let (inferred_types, rule_reference_errors, type_errors) =
            with_infer_expression_type_cache(|| {
                let inferred_types = infer_rule_types(self, &rule_order, resolved_types);
                let rule_reference_errors = self.resolve_rule_reference_types(&inferred_types);
                clear_infer_expression_type_cache();
                let type_errors =
                    check_rule_types(self, &rule_order, &inferred_types, resolved_types);
                (inferred_types, rule_reference_errors, type_errors)
            });

        if let Err(errs) = rule_reference_errors {
            errors.extend(errs);
        }
        if let Err(errs) = type_errors {
            errors.extend(errs);
        }

        if !errors.is_empty() {
            return Err(errors);
        }

        // Phase 6: Apply (only on full success)
        apply_inferred_types(self, inferred_types);
        self.rule_order = rule_order;
        self.data_reference_order = reference_order;
        Ok(())
    }
}

fn local_data_field_names(spec: &LemmaSpec) -> Vec<String> {
    spec.data
        .iter()
        .filter(|d| d.reference.is_local())
        .map(|d| d.reference.name.clone())
        .collect()
}

fn is_uses_vs_data_clash(existing: &DataDefinition, incoming: &ParsedDataValue) -> bool {
    matches!(
        (existing, incoming),
        (
            DataDefinition::Import { .. },
            ParsedDataValue::Definition { .. }
        )
    ) || matches!(
        (existing, incoming),
        (
            DataDefinition::TypeDeclaration { .. } | DataDefinition::Value { .. },
            ParsedDataValue::Import { .. }
        )
    )
}

fn qualified_type_name_from_definition(incoming: &ParsedDataValue) -> Option<&str> {
    let ParsedDataValue::Definition {
        base: Some(ParentType::Qualified { inner, .. }),
        ..
    } = incoming
    else {
        return None;
    };
    match inner.as_ref() {
        ParentType::Custom { name } => Some(name.as_str()),
        _ => None,
    }
}

fn import_spec_ref_for_clash<'a>(
    consumer_spec: &'a LemmaSpec,
    incoming: &'a LemmaData,
    existing: &DataDefinition,
) -> std::borrow::Cow<'a, ast::SpecRef> {
    if let ParsedDataValue::Import { spec_ref, .. } = &incoming.value {
        return std::borrow::Cow::Borrowed(spec_ref);
    }
    let alias = &incoming.reference.name;
    for row in &consumer_spec.data {
        if !row.reference.is_local() || row.reference.name != *alias {
            continue;
        }
        let ParsedDataValue::Import { spec_ref, .. } = &row.value else {
            continue;
        };
        return std::borrow::Cow::Borrowed(spec_ref);
    }
    let DataDefinition::Import { target_name, .. } = existing else {
        unreachable!("BUG: uses vs data clash but no Import side with recoverable spec_ref");
    };
    std::borrow::Cow::Owned(ast::SpecRef::same_repository(target_name.clone()))
}

fn uses_vs_data_duplicate_message(
    consumer_spec: &LemmaSpec,
    incoming: &LemmaData,
    existing: &DataDefinition,
) -> (String, Option<String>) {
    match (existing, &incoming.value) {
        (DataDefinition::Import { .. }, ParsedDataValue::Definition { .. })
        | (
            DataDefinition::TypeDeclaration { .. } | DataDefinition::Value { .. },
            ParsedDataValue::Import { .. },
        ) => {}
        _ => unreachable!("uses_vs_data_duplicate_message requires a uses vs data clash"),
    };
    let alias = &incoming.reference.name;
    let spec_ref = import_spec_ref_for_clash(consumer_spec, incoming, existing);
    let target_label = spec_ref.to_string();
    let import_alias = format!("{alias}_spec");
    let message = format!(
        "You used the name `{alias}` for both a `uses` import (target {target_label}) and a `data` definition.",
    );
    let suggestion = match qualified_type_name_from_definition(&incoming.value) {
        Some(type_name) => format!(
            "Rename the `uses` import alias to `{import_alias}`. The qualified type path must use `{import_alias}.{type_name}`."
        ),
        _ => format!("Rename the `uses` import alias to `{import_alias}`."),
    };
    (message, Some(suggestion))
}

impl<'a> GraphBuilder<'a> {
    fn build(
        context: &'a Context,
        repository: &Arc<LemmaRepository>,
        main_spec: &'a LemmaSpec,
        effective: &EffectiveDate,
        type_resolver: &TypeResolver<'a>,
        limits: &'a crate::limits::ResourceLimits,
    ) -> Result<GraphBuildOk<'a>, Vec<Error>> {
        let mut builder = GraphBuilder {
            data: IndexMap::new(),
            rules: BTreeMap::new(),
            context,
            local_types: Vec::new(),
            errors: Vec::new(),
            main_spec,
            main_repository: Arc::clone(repository),
            limits,
        };

        builder.build_spec(
            main_spec,
            repository,
            Vec::new(),
            HashMap::new(),
            effective,
            type_resolver,
        )?;

        Ok((
            builder.data,
            builder.rules,
            builder.errors,
            builder.local_types,
        ))
    }

    fn engine_error(&self, message: impl Into<String>, source: &Source) -> Error {
        Error::validation_with_context(
            message.into(),
            Some(source.clone()),
            None::<String>,
            Some(self.main_spec),
            None,
        )
    }

    fn resolve_unit_ref(
        &self,
        current_spec: &LemmaSpec,
        unit_ref: &str,
    ) -> Result<(String, Arc<LemmaType>), String> {
        let resolved = find_types_by_spec(&self.local_types, current_spec)
            .ok_or_else(|| {
                format!(
                    "Unknown unit '{unit_ref}'. Declare it on a measure or ratio type, or import it with uses."
                )
            })?;
        resolved
            .unit_index
            .resolve_with_named_types(unit_ref, &resolved.resolved)
    }

    fn process_meta_fields(&mut self, spec: &LemmaSpec) {
        let mut seen = HashSet::new();
        for field in &spec.meta_fields {
            if !seen.insert(field.key.clone()) {
                self.errors.push(self.engine_error(
                    format!("Duplicate meta key '{}'", field.key),
                    &field.source_location,
                ));
            }
        }
    }

    fn resolve_spec_ref(
        &self,
        spec_ref: &ast::SpecRef,
        effective: &EffectiveDate,
        consumer_spec: &LemmaSpec,
        consumer_repository: &Arc<LemmaRepository>,
    ) -> Result<(Arc<LemmaRepository>, &'a LemmaSpec), Error> {
        discovery::resolve_spec_ref(
            self.context,
            spec_ref,
            consumer_repository,
            consumer_spec,
            effective,
            None,
        )
    }

    fn push_missing_with_data_field_error(
        &mut self,
        import_row: &LemmaData,
        binding: &ast::UsesBinding,
        walk_spec: &LemmaSpec,
        field_name: &str,
    ) {
        let ParsedDataValue::Import { spec_ref, .. } = &import_row.value else {
            unreachable!("BUG: push_missing_with_data_field_error called on non-Import LemmaData");
        };
        let import_alias = &import_row.reference.name;
        let target_label = spec_ref.to_string();
        let did_you_mean =
            crate::string_distance::closest_name(field_name, &local_data_field_names(walk_spec));
        let message = format!("`{}` has no data field `{}`", walk_spec.name, field_name);
        let mut suggestion = format!(
            "`-> with` on import `{import_alias}` (target {target_label}) must name a data field on `{spec}`",
            spec = walk_spec.name,
        );
        if let Some(name) = did_you_mean {
            suggestion = format!("{suggestion}. Did you mean `{name}`?");
        }
        self.errors.push(Error::validation_with_context(
            message,
            Some(binding.source_location.clone()),
            Some(suggestion),
            Some(self.main_spec),
            None,
        ));
    }

    /// Resolve a parsed [`ast::Reference`] appearing on the RHS of a `data x: ref`
    /// assignment against the scope of `containing_spec`. Returns an
    /// [`ReferenceTarget`] pointing at a data path or rule path. Errors push into
    /// `self.errors`; this function returns `None` on failure (and does not
    /// return a proper `Result` because it mirrors `resolve_path_segments`'s
    /// side-effecting convention so the two can compose cleanly).
    fn resolve_reference_target_in_spec(
        &mut self,
        reference: &ast::Reference,
        reference_source: &Source,
        containing_spec: &'a LemmaSpec,
        containing_segments: &[PathSegment],
        effective: &EffectiveDate,
    ) -> Option<ReferenceTarget> {
        let containing_data_map: HashMap<String, LemmaData> = containing_spec
            .data
            .iter()
            .filter(|d| d.reference.is_local())
            .map(|d| (d.reference.name.clone(), d.clone()))
            .collect();

        let containing_rule_names: HashSet<&str> = containing_spec
            .rules
            .iter()
            .map(|r| r.name.as_str())
            .collect();

        let containing_segments = containing_segments.to_vec();

        if reference.segments.is_empty() {
            let is_data = containing_data_map.contains_key(&reference.name);
            let is_rule = containing_rule_names.contains(reference.name.as_str());
            if is_data && is_rule {
                self.errors.push(self.engine_error(
                    format!(
                        "Reference target '{}' is ambiguous: both a data and a rule in spec '{}'",
                        reference.name, containing_spec.name
                    ),
                    reference_source,
                ));
                return None;
            }
            if is_data {
                return Some(ReferenceTarget::Data(DataPath {
                    segments: containing_segments,
                    data: reference.name.clone(),
                }));
            }
            if is_rule {
                return Some(ReferenceTarget::Rule(RulePath {
                    segments: containing_segments,
                    rule: reference.name.clone(),
                }));
            }
            self.errors.push(self.engine_error(
                format!(
                    "Reference target '{}' not found in spec '{}'",
                    reference.name, containing_spec.name
                ),
                reference_source,
            ));
            return None;
        }

        let (resolved_segments, target_spec) = self.resolve_path_segments(
            &reference.segments,
            reference_source,
            containing_data_map,
            containing_segments,
            containing_spec,
            effective,
        )?;

        let target_data_names: HashSet<&str> = target_spec
            .data
            .iter()
            .filter(|d| d.reference.is_local())
            .map(|d| d.reference.name.as_str())
            .collect();
        let target_rule_names: HashSet<&str> =
            target_spec.rules.iter().map(|r| r.name.as_str()).collect();
        let is_data = target_data_names.contains(reference.name.as_str());
        let is_rule = target_rule_names.contains(reference.name.as_str());

        if is_data && is_rule {
            self.errors.push(self.engine_error(
                format!(
                    "Reference target '{}' is ambiguous: both a data and a rule in spec '{}'",
                    reference.name, target_spec.name
                ),
                reference_source,
            ));
            return None;
        }
        if is_data {
            return Some(ReferenceTarget::Data(DataPath {
                segments: resolved_segments,
                data: reference.name.clone(),
            }));
        }
        if is_rule {
            return Some(ReferenceTarget::Rule(RulePath {
                segments: resolved_segments,
                rule: reference.name.clone(),
            }));
        }

        self.errors.push(self.engine_error(
            format!(
                "Reference target '{}' not found in spec '{}'",
                reference.name, target_spec.name
            ),
            reference_source,
        ));
        None
    }

    /// Collect `uses` block bindings (`-> with`) declared on import rows in `spec`.
    fn build_data_bindings(
        &mut self,
        spec: &'a LemmaSpec,
        current_segments: &[PathSegment],
        effective: &EffectiveDate,
    ) -> Result<DataBindings, Vec<Error>> {
        let mut bindings: DataBindings = HashMap::new();
        let mut errors: Vec<Error> = Vec::new();

        for import_row in &spec.data {
            if !import_row.reference.is_local() {
                continue;
            }
            let ParsedDataValue::Import {
                bindings: uses_bindings,
                ..
            } = &import_row.value
            else {
                continue;
            };
            for binding in uses_bindings {
                let binding_reference =
                    ast::prefix_reference(&import_row.reference.name, &binding.path);
                let mut walk_spec = spec;
                let mut binding_resolved = true;

                for segment in &binding_reference.segments {
                    let Some(seg_data) = walk_spec.data.iter().find(|field| {
                        field.reference.segments.is_empty() && field.reference.name == *segment
                    }) else {
                        self.push_missing_with_data_field_error(
                            import_row, binding, walk_spec, segment,
                        );
                        binding_resolved = false;
                        break;
                    };

                    let spec_ref = match &seg_data.value {
                        ParsedDataValue::Import { spec_ref, .. } => spec_ref,
                        _ => {
                            self.errors.push(Error::validation_with_context(
                                format!(
                                    "`{segment}` in `{spec}` is not an imported spec — `-> with` paths step through data fields that hold a `uses` import.",
                                    spec = walk_spec.name
                                ),
                                Some(binding.source_location.clone()),
                                Some(format!(
                                    "`-> with` on import `{alias}` must step through import data fields",
                                    alias = import_row.reference.name,
                                )),
                                Some(self.main_spec),
                                None,
                            ));
                            binding_resolved = false;
                            break;
                        }
                    };

                    let walk_repository =
                        discovery::lookup_owning_repository(self.context, walk_spec)
                            .unwrap_or_else(|| Arc::clone(&self.main_repository));
                    walk_spec = match self.resolve_spec_ref(
                        spec_ref,
                        effective,
                        walk_spec,
                        &walk_repository,
                    ) {
                        Ok((_, arc)) => arc,
                        Err(error) => {
                            self.errors.push(error);
                            binding_resolved = false;
                            break;
                        }
                    };
                }

                if !binding_resolved {
                    continue;
                }

                if !walk_spec.data.iter().any(|field| {
                    field.reference.segments.is_empty()
                        && field.reference.name == binding_reference.name
                }) {
                    self.push_missing_with_data_field_error(
                        import_row,
                        binding,
                        walk_spec,
                        &binding_reference.name,
                    );
                    continue;
                }

                let mut binding_key: Vec<String> = current_segments
                    .iter()
                    .map(|segment| segment.data.clone())
                    .collect();
                binding_key.extend(binding_reference.segments.iter().cloned());
                binding_key.push(binding_reference.name.clone());

                let binding_value = match &binding.rhs {
                    WithRhs::Literal(value) => BindingValue::Literal(value.clone()),
                    WithRhs::Reference { target } => {
                        let Some(resolved_target) = self.resolve_reference_target_in_spec(
                            target,
                            &binding.source_location,
                            spec,
                            current_segments,
                            effective,
                        ) else {
                            continue;
                        };
                        BindingValue::Reference {
                            target: resolved_target,
                            constraints: None,
                        }
                    }
                };

                let source = binding.source_location.clone();
                if let Some((_, existing_source)) = bindings.get(&binding_key) {
                    errors.push(self.engine_error(
                        format!(
                            "Duplicate data binding for '{}' (previously bound at {}:{})",
                            binding_key.join("."),
                            existing_source.source_type,
                            existing_source.span.line
                        ),
                        &binding.source_location,
                    ));
                } else {
                    bindings.insert(binding_key, (binding_value, source));
                }
            }
        }

        if !errors.is_empty() {
            return Err(errors);
        }

        Ok(bindings)
    }

    /// Add a single local data to the graph.
    ///
    /// Determines the effective value by checking `data_bindings` for an entry at
    /// the data's path. If a binding exists, uses the bound value; otherwise uses
    /// the data's own value. Reports an error on duplicate data.
    fn add_data(
        &mut self,
        data: &LemmaData,
        current_segments: &[PathSegment],
        data_bindings: &DataBindings,
        current_spec: &LemmaSpec,
        used_binding_keys: &mut HashSet<Vec<String>>,
        effective: &EffectiveDate,
    ) {
        let data_path = DataPath {
            segments: current_segments.to_vec(),
            data: data.reference.name.clone(),
        };

        // Check for duplicates
        if let Some(existing) = self.data.get(&data_path) {
            let (message, suggestion) = if is_uses_vs_data_clash(existing, &data.value) {
                uses_vs_data_duplicate_message(current_spec, data, existing)
            } else {
                (
                    format!(
                        "The name '{}' is already used for data in this spec.",
                        data_path.data
                    ),
                    None,
                )
            };
            self.errors.push(Error::validation_with_context(
                message,
                Some(data.source_location.clone()),
                suggestion,
                Some(self.main_spec),
                None,
            ));
            return;
        }

        // Build the binding key for this data: segment data names + data name
        let binding_key: Vec<String> = current_segments
            .iter()
            .map(|s| s.data.clone())
            .chain(std::iter::once(data.reference.name.clone()))
            .collect();

        // A binding (if any) overrides the data's own RHS. We track the binding
        // separately from the data's own value because `BindingValue` (resolved)
        // and `ParsedDataValue` (raw AST) are different types.
        let binding_override: Option<(BindingValue, Source)> =
            data_bindings.get(&binding_key).map(|(v, s)| {
                used_binding_keys.insert(binding_key.clone());
                (v.clone(), s.clone())
            });

        let (original_schema_type, original_declared_suggestion, original_declared_fill) = if matches!(
            &data.value,
            ParsedDataValue::Definition { .. }
        )
            && data.value.definition_needs_type_resolution()
        {
            let resolved = self
                .local_types
                .iter()
                .find(|(_, s, _)| discovery::same_loaded_spec(s, current_spec))
                .map(|(_, _, t)| t)
                .expect("BUG: no resolved types for spec during add_local_data");
            let lemma_type = Arc::clone(
                resolved
                    .resolved
                    .get(&data.reference.name)
                    .expect(
                        "BUG: type not in ResolvedSpecTypes.resolved. TypeResolver should have registered it",
                    ),
            );
            let declared = resolved
                .declared_suggestions
                .get(&data.reference.name)
                .cloned();
            let declared_fill = resolved.declared_fills.get(&data.reference.name).cloned();
            (Some(lemma_type), declared, declared_fill)
        } else {
            (None, None, None)
        };

        if let Some((binding_value, binding_source)) = binding_override {
            self.add_data_from_binding(
                data_path,
                binding_value,
                binding_source,
                original_schema_type,
                current_spec,
            );
            return;
        }

        let effective_source = data.source_location.clone();

        match &data.value {
            ParsedDataValue::Definition { .. } if data.value.is_definition_literal_only() => {
                let ParsedDataValue::Definition {
                    value: Some(value), ..
                } = &data.value
                else {
                    unreachable!("BUG: literal-only Definition must carry value");
                };
                self.insert_literal_data(
                    data_path,
                    value,
                    original_schema_type,
                    effective_source,
                    current_spec,
                );
            }
            ParsedDataValue::Definition { .. } => {
                let resolved_type = original_schema_type.unwrap_or_else(|| {
                    unreachable!(
                        "BUG: Definition without schema — TypeResolver should have registered it"
                    )
                });
                let declared_suggestion = original_declared_suggestion;
                let declared_fill = original_declared_fill;

                let is_generic_measure_range = matches!(
                    &resolved_type.specifications,
                    TypeSpecification::MeasureRange {
                        units,
                        decomposition,
                        ..
                    } if units.0.is_empty() && decomposition.is_none()
                );

                if is_generic_measure_range {
                    let measure_endpoint_suggest = matches!(
                        &declared_suggestion,
                        Some(ValueKind::Range(left, right))
                            if matches!(
                                (&left.value, &right.value),
                                (ValueKind::Measure(_), ValueKind::Measure(_))
                            )
                    );
                    let measure_endpoint_fill = matches!(
                        &declared_fill,
                        Some(ValueKind::Range(left, right))
                            if matches!(
                                (&left.value, &right.value),
                                (ValueKind::Measure(_), ValueKind::Measure(_))
                            )
                    );
                    if measure_endpoint_suggest || measure_endpoint_fill {
                        self.errors.push(self.engine_error(
                            "measure range requires unit declarations before a measure-endpoint suggest or fill".to_string(),
                            &effective_source,
                        ));
                        return;
                    }
                }

                self.data.insert(
                    data_path,
                    DataDefinition::TypeDeclaration {
                        resolved_type,
                        declared_suggestion,
                        declared_fill,
                        source: effective_source,
                    },
                );
            }
            ParsedDataValue::Import { spec_ref, .. } => {
                let consumer_repository =
                    discovery::lookup_owning_repository(self.context, current_spec)
                        .unwrap_or_else(|| Arc::clone(&self.main_repository));
                let effective_spec = match self.resolve_spec_ref(
                    spec_ref,
                    effective,
                    current_spec,
                    &consumer_repository,
                ) {
                    Ok((_, arc)) => arc,
                    Err(e) => {
                        self.errors.push(e);
                        return;
                    }
                };

                self.data.insert(
                    data_path,
                    DataDefinition::Import {
                        target_name: effective_spec.name.clone(),
                        source: effective_source,
                    },
                );
            }
        }
    }

    /// Inserts a literal-value data definition using the given literal.
    /// Shared between the literal path of `add_data` and the literal path of
    /// a binding-provided value (bindings can only be literals or references).
    fn insert_literal_data(
        &mut self,
        data_path: DataPath,
        value: &ast::Value,
        declared_schema_type: Option<Arc<LemmaType>>,
        effective_source: Source,
        current_spec: &LemmaSpec,
    ) {
        let semantic_value = if let Some(ref schema) = declared_schema_type {
            match parser_value_to_value_kind(value, &schema.specifications) {
                Ok(s) => s,
                Err(e) => {
                    self.errors.push(self.engine_error(e, &effective_source));
                    return;
                }
            }
        } else {
            match value {
                Value::NumberWithUnit(magnitude, unit) => {
                    let (bare, lt) = match self.resolve_unit_ref(current_spec, unit) {
                        Ok(resolved) => resolved,
                        Err(message) => {
                            self.errors
                                .push(self.engine_error(message, &effective_source));
                            return;
                        }
                    };
                    match number_with_unit_to_value_kind(*magnitude, &bare, lt.as_ref()) {
                        Ok(s) => s,
                        Err(e) => {
                            self.errors.push(self.engine_error(e, &effective_source));
                            return;
                        }
                    }
                }
                Value::Range(left, right) => match (left.as_ref(), right.as_ref()) {
                    (
                        Value::NumberWithUnit(left_mag, unit),
                        Value::NumberWithUnit(right_mag, right_unit),
                    ) if unit == right_unit => {
                        let (bare, lt) = match self.resolve_unit_ref(current_spec, unit) {
                            Ok(resolved) => resolved,
                            Err(message) => {
                                self.errors
                                    .push(self.engine_error(message, &effective_source));
                                return;
                            }
                        };
                        let left_kind =
                            match number_with_unit_to_value_kind(*left_mag, &bare, lt.as_ref()) {
                                Ok(v) => v,
                                Err(e) => {
                                    self.errors.push(self.engine_error(e, &effective_source));
                                    return;
                                }
                            };
                        let right_kind =
                            match number_with_unit_to_value_kind(*right_mag, &bare, lt.as_ref()) {
                                Ok(v) => v,
                                Err(e) => {
                                    self.errors.push(self.engine_error(e, &effective_source));
                                    return;
                                }
                            };
                        ValueKind::Range(
                            Box::new(LiteralValue { value: left_kind }),
                            Box::new(LiteralValue { value: right_kind }),
                        )
                    }
                    _ => match value_to_semantic(value) {
                        Ok(s) => s,
                        Err(e) => {
                            self.errors.push(self.engine_error(e, &effective_source));
                            return;
                        }
                    },
                },
                _ => match value_to_semantic(value) {
                    Ok(s) => s,
                    Err(e) => {
                        self.errors.push(self.engine_error(e, &effective_source));
                        return;
                    }
                },
            }
        };
        let inferred_type: Arc<LemmaType> = match value {
            Value::Text(_) => primitive_text_arc().clone(),
            Value::Number(_) => primitive_number_arc().clone(),
            Value::NumberWithUnit(_, unit) => match self.resolve_unit_ref(current_spec, unit) {
                Ok((_, lt)) => lt,
                Err(message) => {
                    self.errors
                        .push(self.engine_error(message, &effective_source));
                    return;
                }
            },
            Value::Boolean(_) => primitive_boolean_arc().clone(),
            Value::Date(_) => primitive_date_arc().clone(),
            Value::Time(_) => primitive_time_arc().clone(),
            Value::Range(left_v, right_v) => match (left_v.as_ref(), right_v.as_ref()) {
                (Value::NumberWithUnit(_, unit), Value::NumberWithUnit(_, right_unit))
                    if unit == right_unit =>
                {
                    match self.resolve_unit_ref(current_spec, unit) {
                        Ok((_, lt)) => {
                            let endpoint = Arc::new(
                                lt.as_ref().clone().with_measure_binding_unit(unit.clone()),
                            );
                            let specs = range_type_specification_from_endpoints(
                                endpoint.as_ref(),
                                endpoint.as_ref(),
                            )
                            .unwrap_or_else(|| {
                                unreachable!(
                                "BUG: measure range endpoints of same unit must form a range type"
                            )
                            });
                            Arc::new(LemmaType::primitive(specs))
                        }
                        Err(message) => {
                            self.errors
                                .push(self.engine_error(message, &effective_source));
                            return;
                        }
                    }
                }
                _ => match &semantic_value {
                    ValueKind::Range(left, right) => {
                        let endpoint_type = |endpoint: &LiteralValue| -> Arc<LemmaType> {
                            match &endpoint.value {
                                ValueKind::Number(_) => primitive_number_arc().clone(),
                                ValueKind::Date(_) => primitive_date_arc().clone(),
                                ValueKind::Time(_) => primitive_time_arc().clone(),
                                ValueKind::Ratio(_) => primitive_ratio_arc().clone(),
                                other => unreachable!(
                                    "BUG: untyped range endpoint kind for insert_literal_data: {other:?}"
                                ),
                            }
                        };
                        let specs = range_type_specification_from_endpoints(
                            endpoint_type(left).as_ref(),
                            endpoint_type(right).as_ref(),
                        )
                        .unwrap_or_else(|| {
                            unreachable!(
                                "BUG: attempted to construct a range literal from incompatible endpoint types"
                            )
                        });
                        Arc::new(LemmaType::primitive(specs))
                    }
                    _ => unreachable!(
                        "BUG: semantic range literal conversion returned non-range value kind"
                    ),
                },
            },
        };
        let schema_type = declared_schema_type.unwrap_or_else(|| inferred_type.clone());
        let lemma_type = match (value, &semantic_value) {
            (Value::NumberWithUnit(_, unit), ValueKind::Measure(_) | ValueKind::Ratio(_)) => {
                Arc::new(
                    schema_type
                        .as_ref()
                        .clone()
                        .with_measure_binding_unit(unit.clone()),
                )
            }
            _ => schema_type,
        };
        let literal_value = LiteralValue {
            value: semantic_value,
        };
        self.data.insert(
            data_path,
            DataDefinition::Value {
                value: literal_value,
                resolved_type: lemma_type,
                source: effective_source,
            },
        );
    }

    /// Apply a binding override to insert the bound data's definition.
    /// Bindings are pre-resolved — literal values or reference targets.
    fn add_data_from_binding(
        &mut self,
        data_path: DataPath,
        binding_value: BindingValue,
        binding_source: Source,
        declared_schema_type: Option<Arc<LemmaType>>,
        current_spec: &LemmaSpec,
    ) {
        match binding_value {
            BindingValue::Literal(value) => {
                self.insert_literal_data(
                    data_path,
                    &value,
                    declared_schema_type,
                    binding_source,
                    current_spec,
                );
            }
            BindingValue::Reference {
                target,
                constraints,
            } => {
                let provisional_type = declared_schema_type
                    .unwrap_or_else(|| Arc::new(LemmaType::undetermined_type()));
                self.data.insert(
                    data_path,
                    DataDefinition::Reference {
                        target,
                        resolved_type: provisional_type,
                        local_constraints: constraints,
                        local_suggestion: None,
                        local_fill: None,
                        source: binding_source,
                    },
                );
            }
        }
    }

    /// Returns (path_segments, last_resolved_spec) on success.
    fn resolve_path_segments(
        &mut self,
        segments: &[String],
        reference_source: &Source,
        mut current_data_map: HashMap<String, LemmaData>,
        mut path_segments: Vec<PathSegment>,
        mut spec_context: &'a LemmaSpec,
        effective: &EffectiveDate,
    ) -> Option<(Vec<PathSegment>, &'a LemmaSpec)> {
        let mut last_arc: Option<&LemmaSpec> = None;

        for segment in segments.iter() {
            let data_ref =
                match current_data_map.get(segment) {
                    Some(f) => f,
                    None => {
                        self.errors.push(self.engine_error(
                            format!("Data '{}' not found", segment),
                            reference_source,
                        ));
                        return None;
                    }
                };

            if let ParsedDataValue::Import { spec_ref, .. } = &data_ref.value {
                let context_repository =
                    discovery::lookup_owning_repository(self.context, spec_context)
                        .unwrap_or_else(|| Arc::clone(&self.main_repository));
                let arc = match self.resolve_spec_ref(
                    spec_ref,
                    effective,
                    spec_context,
                    &context_repository,
                ) {
                    Ok((_, a)) => a,
                    Err(e) => {
                        self.errors.push(e);
                        return None;
                    }
                };
                spec_context = arc;

                path_segments.push(PathSegment {
                    data: segment.clone(),
                    spec: arc.name.clone(),
                });
                current_data_map = arc
                    .data
                    .iter()
                    .map(|f| (f.reference.name.clone(), f.clone()))
                    .collect();
                last_arc = Some(arc);
            } else {
                self.errors.push(self.engine_error(
                    format!("Data '{}' is not a spec reference", segment),
                    reference_source,
                ));
                return None;
            }
        }

        let final_arc = last_arc.unwrap_or_else(|| {
            unreachable!(
                "BUG: resolve_path_segments called with empty segments should not reach here"
            )
        });
        Some((path_segments, final_arc))
    }

    /// Ensure `spec` and every qualified-import target spec (transitively) are resolved into
    /// [`Self::local_types`] before the consumer reads cross-spec parent types.
    fn ensure_spec_types_resolved(
        &mut self,
        spec: &'a LemmaSpec,
        spec_repository: &Arc<LemmaRepository>,
        effective: &EffectiveDate,
        type_resolver: &TypeResolver<'a>,
    ) -> bool {
        if self
            .local_types
            .iter()
            .any(|(_, s, _)| discovery::same_loaded_spec(s, spec))
        {
            return true;
        }
        if !type_resolver.is_registered(spec) {
            panic!(
                "BUG: spec '{}' reachable from spec '{}' was not registered during dependency discovery",
                spec.name, self.main_spec.name
            );
        }
        if type_resolver.registration_failed(spec) {
            // The registration errors were already collected by
            // `register_all`; the spec cannot be built without its types.
            return false;
        }
        for data in &spec.data {
            match &data.value {
                ParsedDataValue::Import { spec_ref, .. } => {
                    let import_effective = spec_ref.at(effective);
                    match self.resolve_spec_ref(spec_ref, &import_effective, spec, spec_repository)
                    {
                        Ok((source_repo, source_arc)) => {
                            self.ensure_spec_types_resolved(
                                source_arc,
                                &source_repo,
                                &import_effective,
                                type_resolver,
                            );
                        }
                        Err(error) => self.errors.push(error),
                    }
                }
                ParsedDataValue::Definition {
                    base: Some(ParentType::Qualified { spec_alias, .. }),
                    ..
                } => {
                    match self.resolve_spec_ref(
                        &ast::SpecRef::same_repository(spec_alias.clone()),
                        effective,
                        spec,
                        spec_repository,
                    ) {
                        Ok((source_repo, source_arc)) => {
                            self.ensure_spec_types_resolved(
                                source_arc,
                                &source_repo,
                                effective,
                                type_resolver,
                            );
                        }
                        Err(error) => self.errors.push(error),
                    }
                }
                _ => {}
            }
        }
        match type_resolver.resolve_and_validate(spec, effective, &self.local_types) {
            Ok(resolved_types) => {
                self.local_types
                    .push((Arc::clone(spec_repository), spec, resolved_types));
                true
            }
            Err(errors) => {
                self.errors.extend(errors);
                false
            }
        }
    }

    fn build_spec(
        &mut self,
        spec: &'a LemmaSpec,
        spec_repository: &Arc<LemmaRepository>,
        current_segments: Vec<PathSegment>,
        data_bindings: DataBindings,
        effective: &EffectiveDate,
        type_resolver: &TypeResolver<'a>,
    ) -> Result<(), Vec<Error>> {
        if current_segments.len() > self.limits.max_spec_dependency_depth {
            return Err(vec![Error::resource_limit_exceeded(
                "max_spec_dependency_depth",
                self.limits.max_spec_dependency_depth.to_string(),
                current_segments.len().to_string(),
                format!(
                    "Spec '{}' exceeds the maximum import nesting depth; flatten the import chain",
                    spec.name
                ),
                None,
                Some(spec),
                None,
            )]);
        }

        if current_segments.is_empty() {
            self.process_meta_fields(spec);
        }

        let current_segment_names: Vec<String> =
            current_segments.iter().map(|s| s.data.clone()).collect();

        // Step 2: Build data bindings declared in this spec (for passing to referenced specs)
        let this_spec_bindings = match self.build_data_bindings(spec, &current_segments, effective)
        {
            Ok(bindings) => bindings,
            Err(errors) => {
                self.errors.extend(errors);
                HashMap::new()
            }
        };

        // Build data_map for rule resolution and other lookups
        let data_map: HashMap<String, &LemmaData> = spec
            .data
            .iter()
            .map(|data| (data.reference.name.clone(), data))
            .collect();

        if !self
            .local_types
            .iter()
            .any(|(_, s, _)| discovery::same_loaded_spec(s, spec))
            && !self.ensure_spec_types_resolved(spec, spec_repository, effective, type_resolver)
        {
            return Ok(());
        }

        let mut effective_bindings = data_bindings.clone();
        effective_bindings.extend(this_spec_bindings.clone());

        // Step 4: Add local data using effective bindings (caller + this spec)
        let mut used_binding_keys: HashSet<Vec<String>> = HashSet::new();
        for data in &spec.data {
            if !data.reference.is_local() {
                continue;
            }
            if matches!(&data.value, ParsedDataValue::Import { .. }) {
                continue;
            }
            self.add_data(
                data,
                &current_segments,
                &effective_bindings,
                spec,
                &mut used_binding_keys,
                effective,
            );
        }

        for data in &spec.data {
            if !data.reference.segments.is_empty() {
                continue;
            }
            if let ParsedDataValue::Import { spec_ref, .. } = &data.value {
                let nested_effective = spec_ref.at(effective);
                let (nested_repo, nested_arc) =
                    match self.resolve_spec_ref(spec_ref, effective, spec, spec_repository) {
                        Ok(pair) => pair,
                        Err(e) => {
                            self.errors.push(e);
                            continue;
                        }
                    };
                self.add_data(
                    data,
                    &current_segments,
                    &effective_bindings,
                    spec,
                    &mut used_binding_keys,
                    effective,
                );
                let mut nested_segments = current_segments.clone();
                nested_segments.push(PathSegment {
                    data: data.reference.name.clone(),
                    spec: nested_arc.name.clone(),
                });

                let nested_segment_names: Vec<String> =
                    nested_segments.iter().map(|s| s.data.clone()).collect();
                let mut combined_bindings = effective_bindings.clone();
                for (key, value_and_source) in &data_bindings {
                    if key.len() > nested_segment_names.len()
                        && key[..nested_segment_names.len()] == nested_segment_names[..]
                        && !combined_bindings.contains_key(key)
                    {
                        combined_bindings.insert(key.clone(), value_and_source.clone());
                    }
                }

                if let Err(errs) = self.build_spec(
                    nested_arc,
                    &nested_repo,
                    nested_segments,
                    combined_bindings,
                    &nested_effective,
                    type_resolver,
                ) {
                    self.errors.extend(errs);
                }
            }
        }

        // Path fills (LHS with segments) must match a declared slot in a nested spec.
        let expected_key_len = current_segments.len() + 1;
        for data in &spec.data {
            if data.reference.segments.is_empty() {
                continue;
            }
            let mut binding_key: Vec<String> = current_segment_names.clone();
            binding_key.extend(data.reference.segments.iter().cloned());
            binding_key.push(data.reference.name.clone());
            if binding_key.len() != expected_key_len {
                continue;
            }
            if used_binding_keys.contains(&binding_key) {
                continue;
            }
            let Some((_, binding_source)) = effective_bindings.get(&binding_key) else {
                continue;
            };
            self.errors.push(self.engine_error(
                format!(
                    "No declared data matches with or binding for '{}'",
                    binding_key.join(".")
                ),
                binding_source,
            ));
        }

        let rule_names: HashSet<&str> = spec.rules.iter().map(|r| r.name.as_str()).collect();
        for rule in &spec.rules {
            self.add_rule(
                rule,
                spec,
                &data_map,
                &current_segments,
                &rule_names,
                effective,
            );
        }

        Ok(())
    }

    fn add_rule(
        &mut self,
        rule: &LemmaRule,
        current_spec: &'a LemmaSpec,
        data_map: &HashMap<String, &LemmaData>,
        current_segments: &[PathSegment],
        rule_names: &HashSet<&str>,
        effective: &EffectiveDate,
    ) {
        let rule_path = RulePath {
            segments: current_segments.to_vec(),
            rule: rule.name.clone(),
        };

        if self.rules.contains_key(&rule_path) {
            let rule_source = &rule.source_location;
            self.errors.push(
                self.engine_error(format!("Duplicate rule '{}'", rule_path.rule), rule_source),
            );
            return;
        }

        let mut branches = Vec::new();
        let mut depends_on_rules = BTreeSet::new();
        let mut convert_ctx = RuleExpressionConversion {
            spec: current_spec,
            data_map,
            segments: current_segments,
            rule_names,
            effective,
            depends_on_rules: &mut depends_on_rules,
        };

        let converted_expression = match self
            .convert_expression_and_extract_dependencies(&rule.expression, &mut convert_ctx)
        {
            Some(expr) => expr,
            None => return,
        };
        branches.push((None, converted_expression));

        for unless_clause in &rule.unless_clauses {
            let converted_condition = match self.convert_expression_and_extract_dependencies(
                &unless_clause.condition,
                &mut convert_ctx,
            ) {
                Some(expr) => expr,
                None => return,
            };
            let converted_result = match self.convert_expression_and_extract_dependencies(
                &unless_clause.result,
                &mut convert_ctx,
            ) {
                Some(expr) => expr,
                None => return,
            };
            branches.push((Some(converted_condition), converted_result));
        }

        let rule_node = RuleNode {
            branches,
            source: rule.source_location.clone(),
            depends_on_rules,
            rule_type: Arc::new(LemmaType::veto_type()),
            spec: current_spec,
        };

        self.rules.insert(rule_path, rule_node);
    }

    /// Converts left and right expressions and accumulates rule dependencies.
    fn convert_binary_operands(
        &mut self,
        left: &ast::Expression,
        right: &ast::Expression,
        ctx: &mut RuleExpressionConversion<'a, '_>,
    ) -> Option<(Expression, Expression)> {
        let converted_left = self.convert_expression_and_extract_dependencies(left, ctx);
        let converted_right = self.convert_expression_and_extract_dependencies(right, ctx);
        match (converted_left, converted_right) {
            (Some(l), Some(r)) => Some((l, r)),
            _ => None,
        }
    }

    /// Converts an AST expression into a resolved expression and records any rule references.
    fn convert_expression_and_extract_dependencies(
        &mut self,
        expr: &ast::Expression,
        ctx: &mut RuleExpressionConversion<'a, '_>,
    ) -> Option<Expression> {
        let expr_src = expr
            .source_location
            .as_ref()
            .expect("BUG: AST expression missing source location");
        match &expr.kind {
            ast::ExpressionKind::Reference(r) => {
                let expr_source = expr_src;
                let (segments, target_arc_opt) = if r.segments.is_empty() {
                    (ctx.segments.to_vec(), None)
                } else {
                    let data_map_owned: HashMap<String, LemmaData> = ctx
                        .data_map
                        .iter()
                        .map(|(k, v)| (k.clone(), (*v).clone()))
                        .collect();
                    let (segs, arc) = self.resolve_path_segments(
                        &r.segments,
                        expr_source,
                        data_map_owned,
                        ctx.segments.to_vec(),
                        ctx.spec,
                        ctx.effective,
                    )?;
                    (segs, Some(arc))
                };

                let (is_data, is_rule, target_spec_name_opt) = match &target_arc_opt {
                    None => {
                        let is_data = ctx.data_map.contains_key(&r.name);
                        let is_rule = ctx.rule_names.contains(r.name.as_str());
                        (is_data, is_rule, None)
                    }
                    Some(target_arc) => {
                        let target_spec = target_arc;
                        let target_data_names: HashSet<&str> = target_spec
                            .data
                            .iter()
                            .filter(|f| f.reference.is_local())
                            .map(|f| f.reference.name.as_str())
                            .collect();
                        let target_rule_names: HashSet<&str> =
                            target_spec.rules.iter().map(|r| r.name.as_str()).collect();
                        let is_data = target_data_names.contains(r.name.as_str());
                        let is_rule = target_rule_names.contains(r.name.as_str());
                        (is_data, is_rule, Some(target_spec.name.as_str()))
                    }
                };

                if is_data && is_rule {
                    self.errors.push(self.engine_error(
                        format!("'{}' is both a data and a rule", r.name),
                        expr_source,
                    ));
                    return None;
                }
                if is_data {
                    let data_path = DataPath {
                        segments,
                        data: r.name.clone(),
                    };
                    return Some(Expression::with_source(
                        ExpressionKind::DataPath(data_path),
                        expr.source_location.clone(),
                    ));
                }
                if is_rule {
                    let rule_path = RulePath {
                        segments,
                        rule: r.name.clone(),
                    };
                    ctx.depends_on_rules.insert(rule_path.clone());
                    return Some(Expression::with_source(
                        ExpressionKind::RulePath(rule_path),
                        expr.source_location.clone(),
                    ));
                }
                let msg = match target_spec_name_opt {
                    Some(s) => format!("Reference '{}' not found in spec '{}'", r.name, s),
                    None => format!("Reference '{}' not found", r.name),
                };
                self.errors.push(self.engine_error(msg, expr_source));
                None
            }

            ast::ExpressionKind::LogicalAnd(left, right) => {
                let (l, r) = self.convert_binary_operands(left, right, ctx)?;
                Some(Expression::with_source(
                    ExpressionKind::LogicalAnd(Arc::new(l), Arc::new(r)),
                    expr.source_location.clone(),
                ))
            }

            ast::ExpressionKind::Arithmetic(left, op, right) => {
                let (l, r) = self.convert_binary_operands(left, right, ctx)?;
                Some(Expression::with_source(
                    ExpressionKind::Arithmetic(Arc::new(l), op.clone(), Arc::new(r)),
                    expr.source_location.clone(),
                ))
            }

            ast::ExpressionKind::Comparison(left, op, right) => {
                let (l, r) = self.convert_binary_operands(left, right, ctx)?;
                Some(Expression::with_source(
                    ExpressionKind::Comparison(Arc::new(l), op.clone(), Arc::new(r)),
                    expr.source_location.clone(),
                ))
            }

            ast::ExpressionKind::UnitConversion(value, target) => {
                let converted_value =
                    self.convert_expression_and_extract_dependencies(value, ctx)?;

                let resolved_spec_types = self
                    .local_types
                    .iter()
                    .find(|(_, s, _)| discovery::same_loaded_spec(s, ctx.spec))
                    .map(|(_, _, t)| t);
                let unit_index = resolved_spec_types.map(|dt| &dt.unit_index);
                let resolved_map = resolved_spec_types.map(|dt| &dt.resolved);
                let semantic_target =
                    match conversion_target_to_semantic(target, unit_index, resolved_map) {
                        Ok(t) => t,
                        Err(msg) => {
                            let full_msg = unit_index
                                .map(|idx| {
                                    let mut valid: Vec<&str> =
                                        idx.keys().map(String::as_str).collect();
                                    valid.sort_unstable();
                                    format!("{} Valid units: {}", msg, valid.join(", "))
                                })
                                .unwrap_or(msg);
                            self.errors.push(Error::validation_with_context(
                                full_msg,
                                expr.source_location.clone(),
                                None::<String>,
                                Some(self.main_spec),
                                None,
                            ));
                            return None;
                        }
                    };

                Some(Expression::with_source(
                    ExpressionKind::UnitConversion(Arc::new(converted_value), semantic_target),
                    expr.source_location.clone(),
                ))
            }

            ast::ExpressionKind::LogicalNegation(operand, neg_type) => {
                let converted_operand =
                    self.convert_expression_and_extract_dependencies(operand, ctx)?;
                Some(Expression::with_source(
                    ExpressionKind::LogicalNegation(Arc::new(converted_operand), neg_type.clone()),
                    expr.source_location.clone(),
                ))
            }

            ast::ExpressionKind::MathematicalComputation(op, operand) => {
                let converted_operand =
                    self.convert_expression_and_extract_dependencies(operand, ctx)?;
                Some(Expression::with_source(
                    ExpressionKind::MathematicalComputation(
                        op.clone(),
                        Arc::new(converted_operand),
                    ),
                    expr.source_location.clone(),
                ))
            }

            ast::ExpressionKind::Literal(value) => {
                let semantic_value = match value {
                    Value::NumberWithUnit(magnitude, unit) => {
                        let (bare, lt) = match self.resolve_unit_ref(ctx.spec, unit) {
                            Ok(resolved) => resolved,
                            Err(message) => {
                                self.errors.push(self.engine_error(message, expr_src));
                                return None;
                            }
                        };
                        match number_with_unit_to_value_kind(*magnitude, &bare, lt.as_ref()) {
                            Ok(v) => v,
                            Err(e) => {
                                self.errors.push(self.engine_error(e, expr_src));
                                return None;
                            }
                        }
                    }
                    Value::Range(left, right) => match (left.as_ref(), right.as_ref()) {
                        (
                            Value::NumberWithUnit(left_mag, unit),
                            Value::NumberWithUnit(right_mag, right_unit),
                        ) if unit == right_unit => {
                            let (bare, lt) = match self.resolve_unit_ref(ctx.spec, unit) {
                                Ok(resolved) => resolved,
                                Err(message) => {
                                    self.errors.push(self.engine_error(message, expr_src));
                                    return None;
                                }
                            };
                            let left_kind =
                                match number_with_unit_to_value_kind(*left_mag, &bare, lt.as_ref())
                                {
                                    Ok(v) => v,
                                    Err(e) => {
                                        self.errors.push(self.engine_error(e, expr_src));
                                        return None;
                                    }
                                };
                            let right_kind = match number_with_unit_to_value_kind(
                                *right_mag,
                                &bare,
                                lt.as_ref(),
                            ) {
                                Ok(v) => v,
                                Err(e) => {
                                    self.errors.push(self.engine_error(e, expr_src));
                                    return None;
                                }
                            };
                            ValueKind::Range(
                                Box::new(LiteralValue { value: left_kind }),
                                Box::new(LiteralValue { value: right_kind }),
                            )
                        }
                        _ => match value_to_semantic(value) {
                            Ok(v) => v,
                            Err(e) => {
                                self.errors.push(self.engine_error(e, expr_src));
                                return None;
                            }
                        },
                    },
                    _ => match value_to_semantic(value) {
                        Ok(v) => v,
                        Err(e) => {
                            self.errors.push(self.engine_error(e, expr_src));
                            return None;
                        }
                    },
                };
                let lemma_type: Arc<LemmaType> = match value {
                    Value::Text(_) => primitive_text_arc().clone(),
                    Value::Number(_) => primitive_number_arc().clone(),
                    Value::NumberWithUnit(_, unit) => match self.resolve_unit_ref(ctx.spec, unit) {
                        Ok((_, lt)) => match &semantic_value {
                            ValueKind::Measure(_) | ValueKind::Ratio(_) => {
                                Arc::new(lt.as_ref().clone().with_measure_binding_unit(unit.clone()))
                            }
                            _ => lt,
                        },
                        Err(message) => {
                            self.errors.push(self.engine_error(message, expr_src));
                            return None;
                        }
                    },
                    Value::Boolean(_) => primitive_boolean_arc().clone(),
                    Value::Date(_) => primitive_date_arc().clone(),
                    Value::Time(_) => primitive_time_arc().clone(),
                    Value::Range(left_v, right_v) => match (left_v.as_ref(), right_v.as_ref()) {
                        (
                            Value::NumberWithUnit(_, unit),
                            Value::NumberWithUnit(_, right_unit),
                        ) if unit == right_unit => match self.resolve_unit_ref(ctx.spec, unit) {
                            Ok((_, lt)) => {
                                let endpoint = Arc::new(
                                    lt.as_ref().clone().with_measure_binding_unit(unit.clone()),
                                );
                                let specs = range_type_specification_from_endpoints(
                                    endpoint.as_ref(),
                                    endpoint.as_ref(),
                                )
                                .unwrap_or_else(|| {
                                    unreachable!(
                                        "BUG: measure range endpoints of same unit must form a range type"
                                    )
                                });
                                Arc::new(LemmaType::primitive(specs))
                            }
                            Err(message) => {
                                self.errors.push(self.engine_error(message, expr_src));
                                return None;
                            }
                        },
                        _ => match &semantic_value {
                            ValueKind::Range(left, right) => {
                                let endpoint_type = |endpoint: &LiteralValue| -> Arc<LemmaType> {
                                    match &endpoint.value {
                                        ValueKind::Number(_) => primitive_number_arc().clone(),
                                        ValueKind::Date(_) => primitive_date_arc().clone(),
                                        ValueKind::Time(_) => primitive_time_arc().clone(),
                                        ValueKind::Ratio(_) => {
                                            semantics::primitive_ratio_arc().clone()
                                        }
                                        other => unreachable!(
                                            "BUG: untyped range endpoint kind in expression conversion: {other:?}"
                                        ),
                                    }
                                };
                                let specs = range_type_specification_from_endpoints(
                                    endpoint_type(left).as_ref(),
                                    endpoint_type(right).as_ref(),
                                )
                                .unwrap_or_else(|| {
                                    unreachable!(
                                        "BUG: attempted to construct a range literal from incompatible endpoint types"
                                    )
                                });
                                Arc::new(LemmaType::primitive(specs))
                            }
                            _ => unreachable!(
                                "BUG: semantic range literal conversion returned non-range value kind"
                            ),
                        },
                    },
                };
                let literal_value = TypedLiteral {
                    value: semantic_value,
                    lemma_type,
                };
                Some(Expression::with_source(
                    ExpressionKind::Literal(Box::new(literal_value)),
                    expr.source_location.clone(),
                ))
            }

            ast::ExpressionKind::Veto(veto_expression) => Some(Expression::with_source(
                ExpressionKind::Veto(veto_expression.clone()),
                expr.source_location.clone(),
            )),

            ast::ExpressionKind::ResultIsVeto(operand) => {
                let converted = self.convert_expression_and_extract_dependencies(operand, ctx)?;
                Some(Expression::with_source(
                    ExpressionKind::ResultIsVeto(Arc::new(converted)),
                    expr.source_location.clone(),
                ))
            }

            ast::ExpressionKind::Now => Some(Expression::with_source(
                ExpressionKind::Now,
                expr.source_location.clone(),
            )),

            ast::ExpressionKind::DateRelative(kind, date_expr) => {
                let converted_date =
                    self.convert_expression_and_extract_dependencies(date_expr, ctx)?;
                Some(Expression::with_source(
                    ExpressionKind::DateRelative(*kind, Arc::new(converted_date)),
                    expr.source_location.clone(),
                ))
            }

            ast::ExpressionKind::DateCalendar(kind, unit, date_expr) => {
                let converted_date =
                    self.convert_expression_and_extract_dependencies(date_expr, ctx)?;
                Some(Expression::with_source(
                    ExpressionKind::DateCalendar(*kind, *unit, Arc::new(converted_date)),
                    expr.source_location.clone(),
                ))
            }

            ast::ExpressionKind::RangeLiteral(left, right) => {
                let (l, r) = self.convert_binary_operands(left, right, ctx)?;
                Some(Expression::with_source(
                    ExpressionKind::RangeLiteral(Arc::new(l), Arc::new(r)),
                    expr.source_location.clone(),
                ))
            }

            ast::ExpressionKind::PastFutureRange(kind, offset_expr) => {
                let converted_offset =
                    self.convert_expression_and_extract_dependencies(offset_expr, ctx)?;
                Some(Expression::with_source(
                    ExpressionKind::PastFutureRange(*kind, Arc::new(converted_offset)),
                    expr.source_location.clone(),
                ))
            }

            ast::ExpressionKind::RangeContainment(value, range) => {
                let (converted_value, converted_range) =
                    self.convert_binary_operands(value, range, ctx)?;
                Some(Expression::with_source(
                    ExpressionKind::RangeContainment(
                        Arc::new(converted_value),
                        Arc::new(converted_range),
                    ),
                    expr.source_location.clone(),
                ))
            }
        }
    }
}

/// Find resolved types for a spec by name. Since per-slice resolution registers
/// at most one version per spec name, this is a simple name match.
fn find_types_by_spec<'b>(
    types: &'b ResolvedTypesMap,
    spec: &LemmaSpec,
) -> Option<&'b ResolvedSpecTypes> {
    types
        .iter()
        .find(|(_, s, _)| discovery::same_loaded_spec(s, spec))
        .map(|(_, _, t)| t)
}

/// The one measure scope of a plan: the main spec's resolved unit index and
/// signature index, which already merge every `uses` import. Graph inference,
/// NormalForm cell stamping and evaluation all resolve arithmetic in it, for
/// imported rules too, so a rule has exactly one type inside a plan.
fn arithmetic_scope<'b>(resolved_types: &'b ResolvedTypesMap, graph: &Graph) -> MeasureScope<'b> {
    let main_spec = graph.main_spec();
    let types = find_types_by_spec(resolved_types, main_spec).unwrap_or_else(|| {
        panic!(
            "BUG: main spec '{}' typed before its types were resolved",
            main_spec.name
        )
    });
    MeasureScope {
        unit_index: &types.unit_index,
        signature_index: &types.signature_index,
    }
}

/// True iff an anonymous measure at the rule boundary must be rejected.
///
/// Planning promotes anonymous intermediates to named types when a unique in-scope type
/// shares the decomposition. Any anonymous measure that still reaches a rule boundary
/// cannot be serialized on the API (no declared unit map to emit).
fn anonymous_rule_boundary_requires_rejection() -> bool {
    true
}

/// Build a precise error message for an anonymous measure that survives to a rule boundary.
///
/// The boundary check rejects such intermediates: a rule must produce a named type so its
/// result is unambiguous. If multiple in-scope types share the decomposition, the message
/// names them as a hint to `as <type>` cast the result.
fn anonymous_rule_boundary_error(
    rule_path: &RulePath,
    spec: &LemmaSpec,
    graph: &Graph,
    resolved_types: &ResolvedTypesMap,
    decomposition: &BaseMeasureVector,
    branch_index: Option<usize>,
) -> String {
    let candidates_hint = match find_unique_measure_type_in_unit_index(
        arithmetic_scope(resolved_types, graph).unit_index,
        decomposition,
    ) {
        DecompositionMatch::Multiple(family_names) => format!(
            " Multiple measure families in scope share these dimensions: {}. Give the rule an explicit named type.",
            family_names.join(", ")
        ),
        _ => String::new(),
    };
    match branch_index {
        Some(index) => format!(
            "Unless clause {} in rule '{}' (spec '{}') returns an anonymous intermediate with              unresolved dimensions {:?}. Give the rule a named measure or ratio type with              declared units, or rewrite the expression so dimensions resolve to a named type in scope.{}",
            index, rule_path.rule, spec.name, decomposition, candidates_hint
        ),
        None => format!(
            "Rule '{}' in spec '{}' returns an anonymous intermediate with unresolved              dimensions {:?}. Give the rule a named measure or ratio type with declared units,              or rewrite the expression so dimensions resolve to a named type in scope.{}",
            rule_path.rule, spec.name, decomposition, candidates_hint
        ),
    }
}

// =============================================================================
// Phase 1: Pure type inference (no validation, no error collection)
// =============================================================================

/// Infer the type of an expression without performing any validation.
/// Returns `LemmaType::undetermined_type()` when a type cannot be determined (e.g. unknown data).
fn infer_expression_type(
    expression: &Expression,
    graph: &Graph,
    computed_rule_types: &HashMap<RulePath, Arc<LemmaType>>,
    resolved_types: &ResolvedTypesMap,
    spec: &LemmaSpec,
) -> Arc<LemmaType> {
    let key = expression as *const Expression as usize;
    let cached = INFER_EXPRESSION_TYPE_CACHE.with(|slot| {
        slot.borrow()
            .as_ref()
            .and_then(|cache| cache.get(&key).cloned())
    });
    if let Some(lemma_type) = cached {
        return lemma_type;
    }
    let lemma_type = infer_expression_type_uncached(
        expression,
        graph,
        computed_rule_types,
        resolved_types,
        spec,
    );
    INFER_EXPRESSION_TYPE_CACHE.with(|slot| {
        if let Some(cache) = slot.borrow_mut().as_mut() {
            cache.insert(key, Arc::clone(&lemma_type));
        }
    });
    lemma_type
}

fn infer_expression_type_uncached(
    expression: &Expression,
    graph: &Graph,
    computed_rule_types: &HashMap<RulePath, Arc<LemmaType>>,
    resolved_types: &ResolvedTypesMap,
    spec: &LemmaSpec,
) -> Arc<LemmaType> {
    match &expression.kind {
        ExpressionKind::Literal(literal_value) => Arc::clone(&literal_value.lemma_type),

        ExpressionKind::DataPath(data_path) => {
            infer_data_type(data_path, graph, computed_rule_types)
        }

        ExpressionKind::RulePath(rule_path) => computed_rule_types
            .get(rule_path)
            .cloned()
            .unwrap_or_else(|| Arc::new(LemmaType::undetermined_type())),

        ExpressionKind::LogicalAnd(left, right) => {
            let left_type =
                infer_expression_type(left, graph, computed_rule_types, resolved_types, spec);
            let right_type =
                infer_expression_type(right, graph, computed_rule_types, resolved_types, spec);
            logical_and_type(&left_type, &right_type)
        }

        ExpressionKind::LogicalNegation(operand, _) => {
            let operand_type =
                infer_expression_type(operand, graph, computed_rule_types, resolved_types, spec);
            logical_not_type(&operand_type)
        }

        ExpressionKind::Comparison(left, _op, right) => {
            let left_type =
                infer_expression_type(left, graph, computed_rule_types, resolved_types, spec);
            let right_type =
                infer_expression_type(right, graph, computed_rule_types, resolved_types, spec);
            comparison_type(&left_type, &right_type)
        }

        ExpressionKind::Arithmetic(left, operator, right) => {
            let left_type =
                infer_expression_type(left, graph, computed_rule_types, resolved_types, spec);
            let right_type =
                infer_expression_type(right, graph, computed_rule_types, resolved_types, spec);
            compute_arithmetic_result_type(
                left_type,
                operator,
                right_type,
                &arithmetic_scope(resolved_types, graph),
            )
        }

        ExpressionKind::UnitConversion(source_expression, target) => {
            let source_type = infer_expression_type(
                source_expression,
                graph,
                computed_rule_types,
                resolved_types,
                spec,
            );
            unit_conversion_type(&source_type, target)
        }

        ExpressionKind::MathematicalComputation(op, operand) => {
            let operand_type =
                infer_expression_type(operand, graph, computed_rule_types, resolved_types, spec);
            math_op_type(op, &operand_type, &arithmetic_scope(resolved_types, graph))
        }

        ExpressionKind::Veto(_) => Arc::new(LemmaType::veto_type()),

        ExpressionKind::ResultIsVeto(operand) => {
            let _ =
                infer_expression_type(operand, graph, computed_rule_types, resolved_types, spec);
            result_is_veto_type()
        }

        ExpressionKind::Now => primitive_date_arc().clone(),

        ExpressionKind::DateRelative(_, date_expr)
        | ExpressionKind::DateCalendar(_, _, date_expr) => {
            let date_type =
                infer_expression_type(date_expr, graph, computed_rule_types, resolved_types, spec);
            date_predicate_type(&date_type)
        }

        ExpressionKind::RangeContainment(value, range) => {
            let value_type =
                infer_expression_type(value, graph, computed_rule_types, resolved_types, spec);
            let range_type =
                infer_expression_type(range, graph, computed_rule_types, resolved_types, spec);
            if value_type.vetoed() || range_type.vetoed() {
                return Arc::new(LemmaType::veto_type());
            }
            primitive_boolean_arc().clone()
        }

        ExpressionKind::RangeLiteral(left, right) => {
            let left_type =
                infer_expression_type(left, graph, computed_rule_types, resolved_types, spec);
            let right_type =
                infer_expression_type(right, graph, computed_rule_types, resolved_types, spec);
            if left_type.vetoed() || right_type.vetoed() {
                return Arc::new(LemmaType::veto_type());
            }
            if left_type.is_undetermined() || right_type.is_undetermined() {
                return Arc::new(LemmaType::undetermined_type());
            }
            infer_range_type_from_endpoint_types(left_type.as_ref(), right_type.as_ref())
        }

        ExpressionKind::PastFutureRange(_, offset_expr) => {
            let offset_type = infer_expression_type(
                offset_expr,
                graph,
                computed_rule_types,
                resolved_types,
                spec,
            );
            past_future_range_type(&offset_type)
        }

        ExpressionKind::Piecewise(arms) => {
            let mut result_type: Option<Arc<LemmaType>> = None;
            for (condition, result) in arms {
                let condition_type = infer_expression_type(
                    condition,
                    graph,
                    computed_rule_types,
                    resolved_types,
                    spec,
                );
                if !condition_type.is_boolean() && !condition_type.is_undetermined() {
                    return Arc::new(LemmaType::undetermined_type());
                }
                let arm_result_type =
                    infer_expression_type(result, graph, computed_rule_types, resolved_types, spec);
                match &result_type {
                    None => result_type = Some(arm_result_type),
                    Some(existing) if *existing == arm_result_type => {}
                    Some(_)
                        if arm_result_type.is_undetermined()
                            || result_type.as_ref().is_some_and(|t| t.is_undetermined()) =>
                    {
                        result_type = Some(Arc::new(LemmaType::undetermined_type()));
                    }
                    Some(_) => return Arc::new(LemmaType::undetermined_type()),
                }
            }
            result_type.unwrap_or_else(|| Arc::new(LemmaType::undetermined_type()))
        }
    }
}

/// Infer the type of a data reference without producing errors.
/// Returns `LemmaType::undetermined_type()` when the data cannot be found or is a spec reference.
///
/// For rule-target references the reference's stored `resolved_type` is still
/// the LHS-only placeholder (or fully `undetermined`) at the time
/// [`infer_rule_types`] runs — that field is filled by
/// [`Graph::resolve_rule_reference_types`] AFTER this pass. We therefore
/// look the target rule's inferred type up in `computed_rule_types`.
fn infer_data_type(
    data_path: &DataPath,
    graph: &Graph,
    computed_rule_types: &HashMap<RulePath, Arc<LemmaType>>,
) -> Arc<LemmaType> {
    let entry = match graph.data().get(data_path) {
        Some(e) => e,
        None => return Arc::new(LemmaType::undetermined_type()),
    };
    match entry {
        DataDefinition::Value { resolved_type, .. } => Arc::clone(resolved_type),
        DataDefinition::TypeDeclaration { resolved_type, .. } => Arc::clone(resolved_type),
        DataDefinition::Reference {
            target: ReferenceTarget::Rule(target_rule),
            resolved_type,
            ..
        } => {
            if !resolved_type.is_undetermined() {
                Arc::clone(resolved_type)
            } else {
                computed_rule_types
                    .get(target_rule)
                    .cloned()
                    .unwrap_or_else(|| Arc::new(LemmaType::undetermined_type()))
            }
        }
        DataDefinition::Reference { resolved_type, .. } => Arc::clone(resolved_type),
        DataDefinition::Import { .. } => Arc::new(LemmaType::undetermined_type()),
    }
}

/// Walk an expression tree, find every `DataPath` that ends at a rule in
/// `reference_ends`, and accumulate that rule into `out`. Used by
/// [`Graph::add_rule_reference_dependency_edges`] to inject rule-rule
/// dependency edges so `topological_sort` orders the target rule before any
/// consumer of the reference data path.
fn collect_rule_reference_dependencies(
    expression: &Expression,
    reference_ends: &IndexMap<DataPath, ReferenceEnd>,
    out: &mut BTreeSet<RulePath>,
) {
    let mut paths: HashSet<DataPath> = HashSet::new();
    expression.kind.collect_data_paths(&mut paths);
    for path in paths {
        if let Some(ReferenceEnd::Rule(target_rule)) = reference_ends.get(&path) {
            out.insert(target_rule.clone());
        }
    }
}

// =============================================================================
// Phase 2: Pure type checking (validation only, no mutation, returns Result)
// =============================================================================

fn engine_error_at_graph(graph: &Graph, source: &Source, message: impl Into<String>) -> Error {
    Error::validation_with_context(
        message.into(),
        Some(source.clone()),
        None::<String>,
        Some(graph.main_spec),
        None,
    )
}

fn check_logical_and_operands(
    graph: &Graph,
    left_type: &LemmaType,
    right_type: &LemmaType,
    source: &Source,
) -> Result<(), Vec<Error>> {
    if left_type.vetoed() || right_type.vetoed() {
        return Ok(());
    }
    if !left_type.is_boolean() {
        return Err(vec![engine_error_at_graph(
            graph,
            source,
            format!(
                "Logical AND requires boolean left operand, got {}",
                left_type
            ),
        )]);
    }
    if !right_type.is_boolean() {
        return Err(vec![engine_error_at_graph(
            graph,
            source,
            format!(
                "Logical AND requires boolean right operand, got {}",
                right_type
            ),
        )]);
    }
    Ok(())
}

fn check_logical_operand(
    graph: &Graph,
    operand_type: &LemmaType,
    source: &Source,
) -> Result<(), Vec<Error>> {
    if operand_type.vetoed() {
        return Ok(());
    }
    if !operand_type.is_boolean() {
        Err(vec![engine_error_at_graph(
            graph,
            source,
            format!(
                "Logical negation requires boolean operand, got {}",
                operand_type
            ),
        )])
    } else {
        Ok(())
    }
}

fn check_comparison_types(
    graph: &Graph,
    left_type: &LemmaType,
    op: &ComparisonComputation,
    right_type: &LemmaType,
    source: &Source,
) -> Result<(), Vec<Error>> {
    if left_type.vetoed() || right_type.vetoed() {
        return Ok(());
    }
    let is_equality_only = matches!(op, ComparisonComputation::Is | ComparisonComputation::IsNot);

    if left_type.is_range() {
        if range_matches_measure_type(left_type, right_type) {
            return Ok(());
        }
        return Err(vec![engine_error_at_graph(
            graph,
            source,
            format!("Cannot compare {} with {}", left_type, right_type),
        )]);
    }

    if left_type.is_boolean() && right_type.is_boolean() {
        if !is_equality_only {
            return Err(vec![engine_error_at_graph(
                graph,
                source,
                format!("Can only use 'is' and 'is not' with booleans (got {})", op),
            )]);
        }
        return Ok(());
    }

    if left_type.is_text() && right_type.is_text() {
        if !is_equality_only {
            return Err(vec![engine_error_at_graph(
                graph,
                source,
                format!("Can only use 'is' and 'is not' with text (got {})", op),
            )]);
        }
        return Ok(());
    }

    if left_type.is_number() && right_type.is_number() {
        return Ok(());
    }

    if left_type.is_ratio() && right_type.is_ratio() {
        return Ok(());
    }

    if left_type.is_date() && right_type.is_date() {
        return Ok(());
    }

    if left_type.is_time() && right_type.is_time() {
        return Ok(());
    }

    if left_type.is_measure() && right_type.is_measure() {
        let same_decomposition = match (
            left_type.measure_type_decomposition(),
            right_type.measure_type_decomposition(),
        ) {
            (Some(ld), Some(rd)) => ld == rd,
            _ => false,
        };
        if !left_type.same_measure_family(right_type)
            && !left_type.compatible_with_anonymous_measure(right_type)
            && !same_decomposition
        {
            return Err(vec![engine_error_at_graph(
                graph,
                source,
                format!(
                    "Cannot compare unrelated measure types: {} and {}",
                    left_type.name(),
                    right_type.name()
                ),
            )]);
        }
        return Ok(());
    }

    if left_type.is_duration_like() && right_type.is_duration_like() {
        return Ok(());
    }
    if left_type.is_calendar_like() && right_type.is_calendar_like() {
        return Ok(());
    }
    if left_type.is_calendar_like() && right_type.is_number() {
        return Ok(());
    }
    if left_type.is_number() && right_type.is_calendar_like() {
        return Ok(());
    }

    Err(vec![engine_error_at_graph(
        graph,
        source,
        format!("Cannot compare {} with {}", left_type, right_type),
    )])
}

/// Literal zero on the right of `/` or `%` is rejected at planning time (runtime data divisors may Veto).
fn arithmetic_literal_zero_divisor_planning_errors(
    graph: &Graph,
    right: &Expression,
    operator: &ArithmeticComputation,
    source: &Source,
) -> Result<(), Vec<Error>> {
    if !matches!(
        operator,
        ArithmeticComputation::Divide | ArithmeticComputation::Modulo
    ) {
        return Ok(());
    }

    if let ExpressionKind::Literal(literal) = &right.kind {
        if let ValueKind::Number(number) = &literal.value {
            if crate::computation::rational::rational_is_zero(number) {
                return Err(vec![engine_error_at_graph(
                    graph,
                    source,
                    format!("Cannot apply '{}' with a zero divisor literal.", operator),
                )]);
            }
        }
    }

    Ok(())
}

fn arithmetic_power_exponent_planning_errors(
    graph: &Graph,
    _left: &Expression,
    right: &Expression,
    left_type: &LemmaType,
    _right_type: &LemmaType,
    operator: &ArithmeticComputation,
    source: &Source,
) -> Result<(), Vec<Error>> {
    if *operator != ArithmeticComputation::Power {
        return Ok(());
    }
    // Measure ^ non-integer-literal is rejected: fractional dimensions are undefined,
    // and variable exponents cannot be statically verified to be integers at plan time.
    if left_type.is_measure() || left_type.is_duration_like() {
        let is_integer_literal = if let ExpressionKind::Literal(lit) = &right.kind {
            if let crate::planning::semantics::ValueKind::Number(n) = &lit.value {
                n.is_integer()
            } else {
                false
            }
        } else {
            false
        };
        if !is_integer_literal {
            return Err(vec![engine_error_at_graph(
                graph,
                source,
                "Cannot raise a measure value to a fractional or variable exponent. Use a positive integer literal.".to_string(),
            )]);
        }
    }
    Ok(())
}

/// Discharges planning obligations for numeric arithmetic beyond type compatibility:
/// literal zero divisors and integer power exponents where required.
fn arithmetic_plan_time_exactness_planning_errors(
    graph: &Graph,
    left: &Expression,
    right: &Expression,
    left_type: &LemmaType,
    right_type: &LemmaType,
    operator: &ArithmeticComputation,
    source: &Source,
) -> Result<(), Vec<Error>> {
    if left_type.vetoed() || right_type.vetoed() {
        return Ok(());
    }
    if left_type.is_undetermined() || right_type.is_undetermined() {
        return Ok(());
    }

    let mut errors = Vec::new();
    let collect = |result: Result<(), Vec<Error>>, errors: &mut Vec<Error>| {
        if let Err(mut errs) = result {
            errors.append(&mut errs);
        }
    };

    collect(
        arithmetic_literal_zero_divisor_planning_errors(graph, right, operator, source),
        &mut errors,
    );
    collect(
        arithmetic_power_exponent_planning_errors(
            graph, left, right, left_type, right_type, operator, source,
        ),
        &mut errors,
    );

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn check_arithmetic_types(
    graph: &Graph,
    left_type: &LemmaType,
    right_type: &LemmaType,
    operator: &ArithmeticComputation,
    source: &Source,
) -> Result<(), Vec<Error>> {
    if left_type.vetoed() || right_type.vetoed() {
        return Ok(());
    }

    if left_type.is_range() || right_type.is_range() {
        let range_measure_allowed = matches!(
            operator,
            ArithmeticComputation::Add | ArithmeticComputation::Subtract
        ) && ((left_type.is_range()
            && right_type.is_range()
            && range_matches_range_measure(left_type, right_type))
            || (left_type.is_range()
                && !right_type.is_range()
                && range_matches_measure_type(left_type, right_type))
            || (right_type.is_range()
                && !left_type.is_range()
                && range_matches_measure_type(right_type, left_type)));
        if range_measure_allowed {
            return Ok(());
        }

        return Err(vec![engine_error_at_graph(
            graph,
            source,
            format!(
                "Cannot apply '{}' to {} and {}.",
                operator,
                left_type.name(),
                right_type.name()
            ),
        )]);
    }

    // Date/Time arithmetic is limited to supported temporal combinations.
    if left_type.is_date() || left_type.is_time() || right_type.is_date() || right_type.is_time() {
        if matches!(operator, ArithmeticComputation::Subtract) {
            if left_type.is_date() && right_type.is_date() {
                return Err(vec![engine_error_at_graph(
                    graph,
                    source,
                    "Cannot subtract dates. Use a date range instead: `start...end as day` or `start...end as year`.".to_string(),
                )]);
            }
            if left_type.is_time() && right_type.is_time() {
                return Err(vec![engine_error_at_graph(
                    graph,
                    source,
                    "Cannot subtract times. Use a datetime range instead: `start...end as hour` or `start...end as second`.".to_string(),
                )]);
            }
        }

        let left_is_duration_like = left_type.is_duration_like();
        let right_is_duration_like = right_type.is_duration_like();
        let valid = matches!(
            (
                left_type.is_date(),
                left_type.is_time(),
                right_type.is_date(),
                right_type.is_time(),
                left_is_duration_like,
                right_is_duration_like,
                left_type.is_calendar_like(),
                right_type.is_calendar_like(),
                operator
            ),
            (
                true,
                _,
                _,
                true,
                _,
                _,
                _,
                _,
                ArithmeticComputation::Subtract
            ) | (
                true,
                _,
                _,
                _,
                _,
                true,
                _,
                _,
                ArithmeticComputation::Add | ArithmeticComputation::Subtract
            ) | (
                true,
                _,
                _,
                _,
                _,
                _,
                _,
                true,
                ArithmeticComputation::Add | ArithmeticComputation::Subtract
            ) | (_, _, true, _, true, _, _, _, ArithmeticComputation::Add)
                | (_, _, true, _, _, _, true, _, ArithmeticComputation::Add)
                | (
                    _,
                    true,
                    _,
                    _,
                    _,
                    true,
                    _,
                    _,
                    ArithmeticComputation::Add | ArithmeticComputation::Subtract
                )
                | (_, _, _, true, true, _, _, _, ArithmeticComputation::Add)
        );
        if !valid {
            return Err(vec![engine_error_at_graph(
                graph,
                source,
                format!(
                    "Cannot apply '{}' to {} and {}.",
                    operator,
                    left_type.name(),
                    right_type.name()
                ),
            )]);
        }
        return Ok(());
    }

    // Measure/Measure rules:
    //   +/- requires same measure family (dimensionless addition is not meaningful otherwise).
    //   *   requires different measure families (same-family measure*measure is rejected; use `as number`).
    //   /   is allowed for all families (same-family → scalar Number; cross-family → anonymous measure).
    //   %   and ^ on two Quantities are always rejected.
    if left_type.is_measure() && right_type.is_measure() {
        return match operator {
            ArithmeticComputation::Add | ArithmeticComputation::Subtract => {
                if left_type.same_measure_family(right_type)
                    || left_type.compatible_with_anonymous_measure(right_type)
                {
                    Ok(())
                } else {
                    Err(vec![engine_error_at_graph(
                        graph,
                        source,
                        format!(
                            "Cannot {} unrelated measure types: {} and {}.",
                            if matches!(operator, ArithmeticComputation::Add) {
                                "add"
                            } else {
                                "subtract"
                            },
                            left_type.name(),
                            right_type.name()
                        ),
                    )])
                }
            }
            ArithmeticComputation::Multiply => {
                // Measure * Measure (same or cross family) → anonymous intermediate when
                // dimensions combine; promotion resolves a unique named type when possible.
                Ok(())
            }
            ArithmeticComputation::Divide => {
                // Measure / Measure (any family) → scalar Number or anonymous intermediate. Allowed.
                Ok(())
            }
            ArithmeticComputation::Modulo | ArithmeticComputation::Power => {
                Err(vec![engine_error_at_graph(
                    graph,
                    source,
                    format!(
                        "Cannot apply '{}' to two measure values ({} and {}).",
                        operator,
                        left_type.name(),
                        right_type.name()
                    ),
                )])
            }
        };
    }

    // Duration * Duration (and power/modulo) rejected for same reason.
    if left_type.is_duration_like() && right_type.is_duration_like() {
        return match operator {
            ArithmeticComputation::Add | ArithmeticComputation::Subtract => Ok(()),
            ArithmeticComputation::Divide => Ok(()),
            _ => Err(vec![engine_error_at_graph(
                graph,
                source,
                "Cannot multiply two duration values. Convert operands first: 'value as number'."
                    .to_string(),
            )]),
        };
    }

    if left_type.is_calendar_like() && right_type.is_calendar_like() {
        return match operator {
            ArithmeticComputation::Add | ArithmeticComputation::Subtract => Ok(()),
            ArithmeticComputation::Divide => Ok(()),
            _ => Err(vec![engine_error_at_graph(
                graph,
                source,
                "Cannot multiply two calendar values. Convert operands first: 'value as number'."
                    .to_string(),
            )]),
        };
    }

    if (left_type.is_duration_like() && right_type.is_calendar_like())
        || (left_type.is_calendar_like() && right_type.is_duration_like())
    {
        return Err(vec![engine_error_at_graph(
            graph,
            source,
            format!(
                "Cannot apply '{}' to {} and {}. Duration and calendar are unrelated types.",
                operator,
                left_type.name(),
                right_type.name()
            ),
        )]);
    }

    // Only Measure, Number, Ratio, Duration, and Calendar can participate in arithmetic
    let left_valid = left_type.is_measure()
        || left_type.is_number()
        || left_type.is_duration_like()
        || left_type.is_calendar_like()
        || left_type.is_ratio();
    let right_valid = right_type.is_measure()
        || right_type.is_number()
        || right_type.is_duration_like()
        || right_type.is_calendar_like()
        || right_type.is_ratio();

    if !left_valid || !right_valid {
        return Err(vec![engine_error_at_graph(
            graph,
            source,
            format!(
                "Cannot apply '{}' to {} and {}.",
                operator,
                left_type.name(),
                right_type.name()
            ),
        )]);
    }

    // Operator-specific constraints (same base type is always allowed)
    if left_type.has_same_base_type(right_type) {
        return Ok(());
    }

    let pair = |a: fn(&LemmaType) -> bool, b: fn(&LemmaType) -> bool| {
        (a(left_type) && b(right_type)) || (b(left_type) && a(right_type))
    };

    let allowed = match operator {
        ArithmeticComputation::Multiply => {
            pair(LemmaType::is_measure, LemmaType::is_number)
                || pair(LemmaType::is_measure, LemmaType::is_ratio)
                || pair(LemmaType::is_measure, LemmaType::is_duration_like_measure)
                || pair(LemmaType::is_measure, LemmaType::is_calendar_like)
                || pair(LemmaType::is_duration_like_measure, LemmaType::is_number)
                || pair(LemmaType::is_duration_like_measure, LemmaType::is_ratio)
                || pair(LemmaType::is_calendar_like, LemmaType::is_number)
                || pair(LemmaType::is_calendar_like, LemmaType::is_ratio)
                || pair(LemmaType::is_number, LemmaType::is_ratio)
        }
        ArithmeticComputation::Divide => {
            pair(LemmaType::is_measure, LemmaType::is_number)
                || (left_type.is_measure() && right_type.is_ratio())
                || pair(LemmaType::is_measure, LemmaType::is_duration_like_measure)
                || pair(LemmaType::is_measure, LemmaType::is_calendar_like)
                || (left_type.is_duration_like() && right_type.is_number())
                || (left_type.is_duration_like() && right_type.is_ratio())
                || (left_type.is_calendar_like() && right_type.is_number())
                || (left_type.is_calendar_like() && right_type.is_ratio())
                || (left_type.is_number() && right_type.is_duration_like())
                || (left_type.is_number() && right_type.is_calendar_like())
                || pair(LemmaType::is_number, LemmaType::is_ratio)
        }
        ArithmeticComputation::Add | ArithmeticComputation::Subtract => {
            if pair(LemmaType::is_ratio, |t: &LemmaType| {
                t.is_number() || t.is_measure() || t.is_duration_like() || t.is_calendar_like()
            }) {
                return Err(vec![engine_error_at_graph(
                    graph,
                    source,
                    format!(
                        "Cannot apply '{}' to {} and {}. \
                         Adding or subtracting a ratio is ambiguous; \
                         scale explicitly, e.g. 'price - discount * price' \
                         or 'price * (100% - discount)'.",
                        operator,
                        left_type.name(),
                        right_type.name(),
                    ),
                )]);
            }
            false
        }
        ArithmeticComputation::Power => {
            // Exponent must be a dimensionless integer for measure left (enforced separately
            // in arithmetic_power_exponent_planning_errors). Measure ^ Ratio is rejected here.
            // number ^ (number | ratio) is allowed; exact rational result or runtime Veto.
            let left_ok = left_type.is_number()
                || left_type.is_measure()
                || left_type.is_ratio()
                || left_type.is_duration_like();
            let right_ok = if left_type.is_measure() || left_type.is_duration_like() {
                right_type.is_number()
            } else {
                right_type.is_number() || right_type.is_ratio()
            };
            left_ok && right_ok
        }
        ArithmeticComputation::Modulo => {
            // Measure % Ratio is rejected: ratio is dimensionless-fractional, not a meaningful
            // modulus for a dimensioned value.
            if left_type.is_measure() && right_type.is_ratio() {
                return Err(vec![engine_error_at_graph(
                    graph,
                    source,
                    format!(
                        "Cannot apply modulo to {} with a ratio. Use a number divisor.",
                        left_type.name()
                    ),
                )]);
            }
            right_type.is_number() || right_type.is_ratio()
        }
    };

    if !allowed {
        return Err(vec![engine_error_at_graph(
            graph,
            source,
            format!(
                "Cannot apply '{}' to {} and {}.",
                operator,
                left_type.name(),
                right_type.name(),
            ),
        )]);
    }

    Ok(())
}

fn has_explicit_unit(source: &Expression) -> bool {
    match &source.kind {
        ExpressionKind::Literal(lit) => match &lit.value {
            ValueKind::Measure(_) => {
                let sig = lit.lemma_type.measure_runtime_signature();
                sig.len() == 1 && sig[0].1 == 1 && !sig[0].0.is_empty()
            }
            ValueKind::Ratio(_) => lit.lemma_type.ratio_primary_unit().is_some(),
            _ => false,
        },
        ExpressionKind::UnitConversion(_, SemanticConversionTarget::Unit { .. }) => true,
        _ => false,
    }
}

fn expression_suggestion_label(expression: &Expression) -> String {
    match &expression.kind {
        ExpressionKind::DataPath(p) => p.data.clone(),
        ExpressionKind::RulePath(p) => p.rule.clone(),
        ExpressionKind::Arithmetic(left, op, right) => {
            format!(
                "{} {} {}",
                expression_suggestion_label(left),
                op,
                expression_suggestion_label(right),
            )
        }
        ExpressionKind::UnitConversion(inner, target) => {
            format!("{} as {}", expression_suggestion_label(inner), target)
        }
        ExpressionKind::Literal(lit) => match &lit.value {
            ValueKind::Number(n) => n.display_str(),
            _ => "<value>".to_string(),
        },
        _ => "<expr>".to_string(),
    }
}

fn first_unit_suggestion(source_type: &LemmaType) -> String {
    source_type
        .measure_unit_names()
        .and_then(|names| names.first().map(|u| (*u).to_string()))
        .unwrap_or_else(|| "<unit>".to_string())
}

fn lookup_unit_type(
    resolved_types: &ResolvedTypesMap,
    spec: &LemmaSpec,
    unit_name: &str,
) -> Option<Arc<LemmaType>> {
    find_types_by_spec(resolved_types, spec)
        .and_then(|dt| dt.unit_index.resolve(unit_name).ok())
        .map(|(_, owner)| owner)
}

fn is_valid_range_span_unit(
    source_type: &LemmaType,
    unit_name: &str,
    unit_index: &UnitIndex,
) -> bool {
    let Ok((bare, target_type)) = unit_index.resolve(unit_name) else {
        return false;
    };
    let unit_name = bare.as_str();
    if source_type.is_date_range() {
        return target_type.is_duration_like() || target_type.is_calendar_like();
    }
    if source_type.is_time_range() {
        return target_type.is_duration_like();
    }
    if source_type.is_measure_range() {
        if source_type
            .measure_unit_names()
            .is_some_and(|names| names.contains(&unit_name))
        {
            return target_type.is_measure();
        }
        let TypeSpecification::MeasureRange { decomposition, .. } = &source_type.specifications
        else {
            unreachable!("BUG: is_measure_range without MeasureRange spec");
        };
        let Some(source_decomp) = decomposition else {
            return false;
        };
        return target_type.is_measure()
            && target_type
                .measure_type_decomposition()
                .is_some_and(|td| td == source_decomp);
    }
    if source_type.is_ratio_range() {
        return target_type.is_ratio();
    }
    false
}

fn check_unit_conversion_types(
    graph: &Graph,
    source: &Expression,
    source_type: &LemmaType,
    target: &SemanticConversionTarget,
    resolved_types: &ResolvedTypesMap,
    source_loc: &Source,
    spec: &LemmaSpec,
) -> Result<(), Vec<Error>> {
    if source_type.vetoed() {
        return Ok(());
    }
    let unit_index = find_types_by_spec(resolved_types, spec)
        .map(|dt| &dt.unit_index)
        .expect("BUG: spec types missing during unit conversion check");

    match target {
        SemanticConversionTarget::Type(PrimitiveKind::Text) => Ok(()),
        SemanticConversionTarget::Type(PrimitiveKind::Boolean) => {
            if source_type.is_boolean() {
                Ok(())
            } else {
                Err(vec![engine_error_at_graph(
                    graph,
                    source_loc,
                    format!("Cannot convert {} to boolean.", source_type.name()),
                )])
            }
        }
        SemanticConversionTarget::Unit {
            unit_name,
            owning_type,
        } => {
            if source_type.is_number() {
                if crate::computation::units::owning_type_declares_unit_name(owning_type, unit_name)
                {
                    return Ok(());
                }
                return Err(vec![engine_error_at_graph(
                    graph,
                    source_loc,
                    format!("Unknown unit '{unit_name}' in spec '{}'.", spec.name),
                )]);
            }
            if source_type.is_measure() {
                let target_type = owning_type.as_ref();
                if source_type.same_measure_family(target_type)
                    || source_type.compatible_with_anonymous_measure(target_type)
                    || target_type.compatible_with_anonymous_measure(source_type)
                {
                    return Ok(());
                }
                if !has_explicit_unit(source) {
                    let expr_label = expression_suggestion_label(source);
                    let first_unit = first_unit_suggestion(source_type);
                    return Err(vec![engine_error_at_graph(
                        graph,
                        source_loc,
                        format!(
                            "Cannot convert '{}' to '{unit_name}' (different measure families). \
                             Express the source unit first, for example '{expr_label} as {first_unit} as {unit_name}'.",
                            source_type.name()
                        ),
                    )]);
                }
                return Ok(());
            }
            if source_type.is_date_range()
                || source_type.is_time_range()
                || source_type.is_measure_range()
                || source_type.is_ratio_range()
            {
                if is_valid_range_span_unit(source_type, unit_name, unit_index) {
                    return Ok(());
                }
                return Err(vec![engine_error_at_graph(
                    graph,
                    source_loc,
                    format!(
                        "Cannot convert {} span to unit '{unit_name}'.",
                        source_type.name()
                    ),
                )]);
            }
            if source_type.is_calendar_like() {
                if crate::computation::units::owning_type_declares_unit_name(owning_type, unit_name)
                {
                    return Ok(());
                }
                return Err(vec![engine_error_at_graph(
                    graph,
                    source_loc,
                    format!("Cannot convert calendar to unit '{unit_name}'."),
                )]);
            }
            if source_type.is_ratio() {
                if let TypeSpecification::Ratio { units, .. } = &source_type.specifications {
                    if units.get(unit_name).is_ok() {
                        return Ok(());
                    }
                    let valid: Vec<&str> = units.0.iter().map(|u| u.name.as_str()).collect();
                    return Err(vec![engine_error_at_graph(
                        graph,
                        source_loc,
                        format!(
                            "Unknown unit '{unit_name}' for ratio type '{}'. Valid units: {}",
                            source_type.name(),
                            valid.join(", ")
                        ),
                    )]);
                }
                return Err(vec![engine_error_at_graph(
                    graph,
                    source_loc,
                    format!("Cannot convert ratio to unit '{unit_name}'."),
                )]);
            }
            Err(vec![engine_error_at_graph(
                graph,
                source_loc,
                format!(
                    "Cannot convert {} to unit '{unit_name}'.",
                    source_type.name()
                ),
            )])
        }
        SemanticConversionTarget::Type(PrimitiveKind::Number) => {
            if source_type.is_date_range()
                || source_type.is_time_range()
                || source_type.is_measure_range()
            {
                return Err(vec![engine_error_at_graph(
                    graph,
                    source_loc,
                    format!(
                        "Cannot use 'as number' on a {}. \
                         Express the span in a unit first, for example '<range> as <unit> as number'.",
                        source_type.name()
                    ),
                )]);
            }
            if source_type.is_number_range() {
                return Ok(());
            }
            if source_type.is_ratio_range() || source_type.is_calendar_like_range() {
                return Err(vec![engine_error_at_graph(
                    graph,
                    source_loc,
                    format!("Cannot convert {} to number.", source_type.name()),
                )]);
            }
            if source_type.is_anonymous_measure() {
                if let Some(decomp) = source_type.measure_type_decomposition() {
                    if !decomp.is_empty() {
                        return Err(vec![engine_error_at_graph(
                            graph,
                            source_loc,
                            format!(
                                "Cannot use 'as number' to strip an anonymous intermediate with unresolved \
                                 dimensions {:?}. Ensure all dimensions cancel before converting to number.",
                                decomp
                            ),
                        )]);
                    }
                }
            }
            if source_type.is_measure() && !source_type.is_anonymous_measure() {
                if !has_explicit_unit(source) {
                    let expr_label = expression_suggestion_label(source);
                    let first_unit = first_unit_suggestion(source_type);
                    return Err(vec![engine_error_at_graph(
                        graph,
                        source_loc,
                        format!(
                            "Cannot use 'as number' on measure '{}' without a unit. \
                             Express it in a unit first, for example '{expr_label} as {first_unit} as number'.",
                            source_type.name()
                        ),
                    )]);
                }
                return Ok(());
            }
            if source_type.is_measure()
                || source_type.is_number()
                || source_type.is_duration_like()
                || source_type.is_calendar_like()
                || source_type.is_ratio()
            {
                Ok(())
            } else {
                Err(vec![engine_error_at_graph(
                    graph,
                    source_loc,
                    format!("Cannot convert {} to number.", source_type.name()),
                )])
            }
        }
        SemanticConversionTarget::Type(target_kind)
            if source_type.matches_primitive_kind(*target_kind) =>
        {
            Ok(())
        }
        SemanticConversionTarget::Type(target_kind) => Err(vec![engine_error_at_graph(
            graph,
            source_loc,
            format!(
                "Cannot convert {} to {:?}.",
                source_type.name(),
                target_kind
            ),
        )]),
    }
}
fn check_mathematical_operand(
    graph: &Graph,
    op: &semantics::MathematicalComputation,
    operand_type: &LemmaType,
    source: &Source,
) -> Result<(), Vec<Error>> {
    if operand_type.vetoed() {
        return Ok(());
    }
    if operand_type.is_number() {
        return Ok(());
    }
    if operand_type.is_measure()
        && crate::computation::mathematical_computation_preserves_measure_magnitude(op)
    {
        return Ok(());
    }
    let message = if operand_type.is_measure() {
        format!(
            "Mathematical function '{op}' cannot be applied to {operand_type}; use 'as number' first"
        )
    } else {
        format!("Mathematical function '{op}' requires number operand, got {operand_type}")
    };
    Err(vec![engine_error_at_graph(graph, source, message)])
}

/// Check that all rule references in the graph point to existing rules.
fn check_all_rule_references_exist(graph: &Graph) -> Result<(), Vec<Error>> {
    let mut errors = Vec::new();
    let existing_rules: HashSet<&RulePath> = graph.rules().keys().collect();
    for (rule_path, rule_node) in graph.rules() {
        for dependency in &rule_node.depends_on_rules {
            if !existing_rules.contains(dependency) {
                errors.push(engine_error_at_graph(
                    graph,
                    &rule_node.source,
                    format!(
                        "Rule '{}' references non-existent rule '{}'",
                        rule_path.rule, dependency.rule
                    ),
                ));
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Check that no data and rule share the same name in the same spec.
fn check_data_and_rule_name_collisions(graph: &Graph) -> Result<(), Vec<Error>> {
    let mut errors = Vec::new();
    for rule_path in graph.rules().keys() {
        let data_path = DataPath::new(rule_path.segments.clone(), rule_path.rule.clone());
        if graph.data().contains_key(&data_path) {
            let rule_node = graph.rules().get(rule_path).unwrap_or_else(|| {
                unreachable!(
                    "BUG: rule '{}' missing from graph while validating name collisions",
                    rule_path.rule
                )
            });
            errors.push(engine_error_at_graph(
                graph,
                &rule_node.source,
                format!(
                    "Name collision: '{}' is defined as both a data and a rule",
                    data_path
                ),
            ));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Check that a data reference is valid (exists and is not a bare spec reference).
fn check_data_reference(
    data_path: &DataPath,
    graph: &Graph,
    data_source: &Source,
) -> Result<(), Vec<Error>> {
    let entry = match graph.data().get(data_path) {
        Some(e) => e,
        None => {
            return Err(vec![engine_error_at_graph(
                graph,
                data_source,
                format!("Unknown data reference '{}'", data_path),
            )]);
        }
    };
    match entry {
        DataDefinition::Value { .. }
        | DataDefinition::TypeDeclaration { .. }
        | DataDefinition::Reference { .. } => Ok(()),
        DataDefinition::Import { .. } => Err(vec![engine_error_at_graph(
            graph,
            entry.source(),
            format!(
                "Cannot compute type for spec reference data '{}'",
                data_path
            ),
        )]),
    }
}

/// Check a single expression for type errors, given precomputed inferred types.
fn check_expression(
    expression: &Expression,
    graph: &Graph,
    inferred_types: &HashMap<RulePath, Arc<LemmaType>>,
    resolved_types: &ResolvedTypesMap,
    spec: &LemmaSpec,
) -> Result<(), Vec<Error>> {
    let mut errors = Vec::new();

    let collect = |result: Result<(), Vec<Error>>, errors: &mut Vec<Error>| {
        if let Err(errs) = result {
            errors.extend(errs);
        }
    };

    match &expression.kind {
        ExpressionKind::Literal(_) => {}

        ExpressionKind::DataPath(data_path) => {
            let data_source = expression
                .source_location
                .as_ref()
                .expect("BUG: expression missing source in check_expression");
            collect(
                check_data_reference(data_path, graph, data_source),
                &mut errors,
            );
        }

        ExpressionKind::RulePath(_) => {}

        ExpressionKind::LogicalAnd(left, right) => {
            collect(
                check_expression(left, graph, inferred_types, resolved_types, spec),
                &mut errors,
            );
            collect(
                check_expression(right, graph, inferred_types, resolved_types, spec),
                &mut errors,
            );

            let left_type =
                infer_expression_type(left, graph, inferred_types, resolved_types, spec);
            let right_type =
                infer_expression_type(right, graph, inferred_types, resolved_types, spec);
            let expr_source = expression
                .source_location
                .as_ref()
                .expect("BUG: expression missing source in check_expression");
            collect(
                check_logical_and_operands(graph, &left_type, &right_type, expr_source),
                &mut errors,
            );
        }

        ExpressionKind::LogicalNegation(operand, _) => {
            collect(
                check_expression(operand, graph, inferred_types, resolved_types, spec),
                &mut errors,
            );

            let operand_type =
                infer_expression_type(operand, graph, inferred_types, resolved_types, spec);
            let expr_source = expression
                .source_location
                .as_ref()
                .expect("BUG: expression missing source in check_expression");
            collect(
                check_logical_operand(graph, &operand_type, expr_source),
                &mut errors,
            );
        }

        ExpressionKind::Comparison(left, op, right) => {
            collect(
                check_expression(left, graph, inferred_types, resolved_types, spec),
                &mut errors,
            );
            collect(
                check_expression(right, graph, inferred_types, resolved_types, spec),
                &mut errors,
            );

            let left_type =
                infer_expression_type(left, graph, inferred_types, resolved_types, spec);
            let right_type =
                infer_expression_type(right, graph, inferred_types, resolved_types, spec);
            let expr_source = expression
                .source_location
                .as_ref()
                .expect("BUG: expression missing source in check_expression");
            collect(
                check_comparison_types(graph, &left_type, op, &right_type, expr_source),
                &mut errors,
            );
        }

        ExpressionKind::Arithmetic(left, operator, right) => {
            collect(
                check_expression(left, graph, inferred_types, resolved_types, spec),
                &mut errors,
            );

            // Detect `left OP (inner as Unit{..})` where OP is *, /, or % and check whether
            // the `as Unit` was meant to convert the operand or the result of the arithmetic.
            // When the operand conversion is invalid but the result conversion would be valid,
            // emit a targeted error with a parenthesized suggestion instead of the generic
            // "different measure families" message that `check_unit_conversion_types` would emit.
            //
            // Three outcomes:
            //   CheckedInlineConversionValid   — conversion on operand is valid; right was
            //                                    checked inline; arithmetic checks must run.
            //   CheckedConversionErrorEmitted  — conversion error (targeted or standard)
            //                                    already emitted; arithmetic checks must be
            //                                    skipped to avoid confusing secondary errors.
            //   NotHandledInline               — pattern did not match; fall through to the
            //                                    normal `check_expression(right)` call.
            enum InlineConversionOutcome {
                CheckedInlineConversionValid,
                CheckedConversionErrorEmitted,
                NotHandledInline,
            }

            let is_multiplicative = matches!(
                operator,
                ArithmeticComputation::Multiply
                    | ArithmeticComputation::Divide
                    | ArithmeticComputation::Modulo
            );

            let right_outcome = if is_multiplicative {
                if let ExpressionKind::UnitConversion(
                    inner_source,
                    conversion_target @ SemanticConversionTarget::Unit { unit_name, .. },
                ) = &right.kind
                {
                    // Always recurse into the inner source to catch errors in sub-expressions.
                    collect(
                        check_expression(inner_source, graph, inferred_types, resolved_types, spec),
                        &mut errors,
                    );

                    let inner_type = infer_expression_type(
                        inner_source,
                        graph,
                        inferred_types,
                        resolved_types,
                        spec,
                    );
                    let expr_source = expression
                        .source_location
                        .as_ref()
                        .expect("BUG: expression missing source in check_expression");

                    let target_type_opt = lookup_unit_type(resolved_types, spec, unit_name);

                    if let Some(target_type) = &target_type_opt {
                        let inner_is_valid_conversion_source = inner_type.is_measure()
                            && (inner_type.same_measure_family(target_type)
                                || inner_type.compatible_with_anonymous_measure(target_type)
                                || target_type.compatible_with_anonymous_measure(&inner_type));

                        if inner_is_valid_conversion_source {
                            // The parse tree is correct: `left OP (inner as unit)`.
                            // Run check_unit_conversion_types to catch explicit-unit requirements,
                            // then let arithmetic checks run on the full `left OP right` below.
                            collect(
                                check_unit_conversion_types(
                                    graph,
                                    inner_source,
                                    &inner_type,
                                    conversion_target,
                                    resolved_types,
                                    expr_source,
                                    spec,
                                ),
                                &mut errors,
                            );
                            InlineConversionOutcome::CheckedInlineConversionValid
                        } else if inner_type.is_measure() || inner_type.is_number() {
                            // Conversion on the operand fails. Check whether converting the
                            // result of the whole arithmetic expression would be valid instead.
                            let left_type = infer_expression_type(
                                left,
                                graph,
                                inferred_types,
                                resolved_types,
                                spec,
                            );
                            let combined_type = compute_arithmetic_result_type(
                                Arc::clone(&left_type),
                                operator,
                                Arc::clone(&inner_type),
                                &arithmetic_scope(resolved_types, graph),
                            );
                            let combined_is_valid_conversion_source = combined_type.is_measure()
                                && (combined_type.same_measure_family(target_type)
                                    || combined_type
                                        .compatible_with_anonymous_measure(target_type)
                                    || target_type
                                        .compatible_with_anonymous_measure(&combined_type));

                            if combined_is_valid_conversion_source {
                                let inner_label = expression_suggestion_label(inner_source);
                                let left_label = expression_suggestion_label(left);
                                errors.push(engine_error_at_graph(
                                    graph,
                                    expr_source,
                                    format!(
                                        "'as {unit_name}' converts '{inner_label}' here, not \
                                         the result of the expression. Write \
                                         '({left_label} {operator} {inner_label}) as {unit_name}' \
                                         to convert the result.",
                                    ),
                                ));
                            } else {
                                // Neither interpretation is valid. Emit the standard conversion
                                // error so the user knows the unit is incompatible.
                                collect(
                                    check_unit_conversion_types(
                                        graph,
                                        inner_source,
                                        &inner_type,
                                        conversion_target,
                                        resolved_types,
                                        expr_source,
                                        spec,
                                    ),
                                    &mut errors,
                                );
                            }
                            InlineConversionOutcome::CheckedConversionErrorEmitted
                        } else {
                            // Inner is not measure/number (e.g. text, boolean, date). Fall through
                            // to standard path so check_expression(right) runs normally.
                            InlineConversionOutcome::NotHandledInline
                        }
                    } else {
                        // Unknown unit — fall through so check_expression(right) emits the standard
                        // "Unknown unit" error from check_unit_conversion_types.
                        InlineConversionOutcome::NotHandledInline
                    }
                } else {
                    InlineConversionOutcome::NotHandledInline
                }
            } else {
                InlineConversionOutcome::NotHandledInline
            };

            match right_outcome {
                InlineConversionOutcome::NotHandledInline => {
                    collect(
                        check_expression(right, graph, inferred_types, resolved_types, spec),
                        &mut errors,
                    );
                }
                InlineConversionOutcome::CheckedInlineConversionValid
                | InlineConversionOutcome::CheckedConversionErrorEmitted => {
                    // Right child was already checked inline; do not recurse again.
                }
            }

            let run_arithmetic_checks = matches!(
                right_outcome,
                InlineConversionOutcome::NotHandledInline
                    | InlineConversionOutcome::CheckedInlineConversionValid
            );

            if run_arithmetic_checks {
                let left_type =
                    infer_expression_type(left, graph, inferred_types, resolved_types, spec);
                let right_type =
                    infer_expression_type(right, graph, inferred_types, resolved_types, spec);
                let expr_source = expression
                    .source_location
                    .as_ref()
                    .expect("BUG: expression missing source in check_expression");
                collect(
                    check_arithmetic_types(graph, &left_type, &right_type, operator, expr_source),
                    &mut errors,
                );
                collect(
                    arithmetic_plan_time_exactness_planning_errors(
                        graph,
                        left,
                        right,
                        &left_type,
                        &right_type,
                        operator,
                        expr_source,
                    ),
                    &mut errors,
                );
            }
        }

        ExpressionKind::UnitConversion(source_expression, target) => {
            collect(
                check_expression(
                    source_expression,
                    graph,
                    inferred_types,
                    resolved_types,
                    spec,
                ),
                &mut errors,
            );

            let source_type = infer_expression_type(
                source_expression,
                graph,
                inferred_types,
                resolved_types,
                spec,
            );
            let expr_source = expression
                .source_location
                .as_ref()
                .expect("BUG: expression missing source in check_expression");
            collect(
                check_unit_conversion_types(
                    graph,
                    source_expression,
                    &source_type,
                    target,
                    resolved_types,
                    expr_source,
                    spec,
                ),
                &mut errors,
            );
        }

        ExpressionKind::MathematicalComputation(op, operand) => {
            collect(
                check_expression(operand, graph, inferred_types, resolved_types, spec),
                &mut errors,
            );

            let operand_type =
                infer_expression_type(operand, graph, inferred_types, resolved_types, spec);
            let expr_source = expression
                .source_location
                .as_ref()
                .expect("BUG: expression missing source in check_expression");
            collect(
                check_mathematical_operand(graph, op, &operand_type, expr_source),
                &mut errors,
            );
        }

        ExpressionKind::Veto(_) => {}

        ExpressionKind::ResultIsVeto(operand) => {
            collect(
                check_expression(operand, graph, inferred_types, resolved_types, spec),
                &mut errors,
            );
        }

        ExpressionKind::Now => {}

        ExpressionKind::DateRelative(_, date_expr) => {
            collect(
                check_expression(date_expr, graph, inferred_types, resolved_types, spec),
                &mut errors,
            );

            let date_type =
                infer_expression_type(date_expr, graph, inferred_types, resolved_types, spec);
            if !date_type.vetoed() && !date_type.is_date() {
                let expr_source = expression
                    .source_location
                    .as_ref()
                    .expect("BUG: expression missing source in check_expression");
                errors.push(engine_error_at_graph(
                    graph,
                    expr_source,
                    format!(
                        "Date sugar 'in past/future' requires a date expression, got type '{}'",
                        date_type
                    ),
                ));
            }
        }

        ExpressionKind::DateCalendar(_, _, date_expr) => {
            collect(
                check_expression(date_expr, graph, inferred_types, resolved_types, spec),
                &mut errors,
            );

            let date_type =
                infer_expression_type(date_expr, graph, inferred_types, resolved_types, spec);
            if !date_type.vetoed() && !date_type.is_date() {
                let expr_source = expression
                    .source_location
                    .as_ref()
                    .expect("BUG: expression missing source in check_expression");
                errors.push(engine_error_at_graph(
                    graph,
                    expr_source,
                    format!(
                        "Calendar sugar requires a date expression, got type '{}'",
                        date_type
                    ),
                ));
            }
        }

        ExpressionKind::RangeLiteral(left, right) => {
            collect(
                check_expression(left, graph, inferred_types, resolved_types, spec),
                &mut errors,
            );
            collect(
                check_expression(right, graph, inferred_types, resolved_types, spec),
                &mut errors,
            );

            let left_type =
                infer_expression_type(left, graph, inferred_types, resolved_types, spec);
            let right_type =
                infer_expression_type(right, graph, inferred_types, resolved_types, spec);
            let expr_source = expression
                .source_location
                .as_ref()
                .expect("BUG: expression missing source in check_expression");

            if !(left_type.vetoed() || right_type.vetoed()) {
                let inferred_range_type =
                    infer_range_type_from_endpoint_types(&left_type, &right_type);
                if inferred_range_type.is_undetermined() {
                    errors.push(engine_error_at_graph(
                        graph,
                        expr_source,
                        format!(
                            "Cannot create a range from {} and {}.",
                            left_type.name(),
                            right_type.name()
                        ),
                    ));
                } else if inferred_range_type.is_time_range() {
                    if let (ExpressionKind::Literal(left_lit), ExpressionKind::Literal(right_lit)) =
                        (&left.kind, &right.kind)
                    {
                        if let (ValueKind::Time(left_time), ValueKind::Time(right_time)) =
                            (&left_lit.value, &right_lit.value)
                        {
                            if !time_range_endpoints_share_timezone(left_time, right_time) {
                                errors.push(engine_error_at_graph(
                                    graph,
                                    expr_source,
                                    "Time range endpoints must use the same timezone".to_string(),
                                ));
                            }
                        }
                    }
                }
            }
        }

        ExpressionKind::PastFutureRange(_, offset_expr) => {
            collect(
                check_expression(offset_expr, graph, inferred_types, resolved_types, spec),
                &mut errors,
            );

            let offset_type =
                infer_expression_type(offset_expr, graph, inferred_types, resolved_types, spec);
            if !offset_type.vetoed()
                && !offset_type.is_duration_like()
                && !offset_type.is_calendar_like()
            {
                let expr_source = expression
                    .source_location
                    .as_ref()
                    .expect("BUG: expression missing source in check_expression");
                errors.push(engine_error_at_graph(
                    graph,
                    expr_source,
                    format!(
                        "Past/future range requires a duration or calendar expression, got type '{}'",
                        offset_type.name()
                    ),
                ));
            }
        }

        ExpressionKind::RangeContainment(value, range) => {
            collect(
                check_expression(value, graph, inferred_types, resolved_types, spec),
                &mut errors,
            );
            collect(
                check_expression(range, graph, inferred_types, resolved_types, spec),
                &mut errors,
            );

            let value_type =
                infer_expression_type(value, graph, inferred_types, resolved_types, spec);
            let range_type =
                infer_expression_type(range, graph, inferred_types, resolved_types, spec);
            let expr_source = expression
                .source_location
                .as_ref()
                .expect("BUG: expression missing source in check_expression");

            if !(value_type.vetoed() || range_type.vetoed()) {
                if !range_type.is_range() {
                    errors.push(engine_error_at_graph(
                        graph,
                        expr_source,
                        format!(
                            "Right side of 'in' must be a range, got type '{}'",
                            range_type.name()
                        ),
                    ));
                } else {
                    let compatible = (range_type.is_date_range() && value_type.is_date())
                        || (range_type.is_time_range() && value_type.is_time())
                        || (range_type.is_number_range() && value_type.is_number())
                        || (range_type.is_measure_range()
                            && value_type.is_measure()
                            && measure_range_matches_measure(&range_type, &value_type))
                        || (range_type.is_ratio_range() && value_type.is_ratio())
                        || (range_type.is_calendar_like_range() && value_type.is_calendar_like());
                    if !compatible {
                        errors.push(engine_error_at_graph(
                            graph,
                            expr_source,
                            format!(
                                "Cannot test whether {} is in {}.",
                                value_type.name(),
                                range_type.name()
                            ),
                        ));
                    }
                }
            }
        }

        ExpressionKind::Piecewise(arms) => {
            for (condition, result) in arms {
                collect(
                    check_expression(condition, graph, inferred_types, resolved_types, spec),
                    &mut errors,
                );
                collect(
                    check_expression(result, graph, inferred_types, resolved_types, spec),
                    &mut errors,
                );
                let condition_type =
                    infer_expression_type(condition, graph, inferred_types, resolved_types, spec);
                if !condition_type.vetoed() && !condition_type.is_boolean() {
                    let expr_source = condition
                        .source_location
                        .as_ref()
                        .expect("BUG: expression missing source in check_expression");
                    errors.push(engine_error_at_graph(
                        graph,
                        expr_source,
                        format!(
                            "Piecewise condition must be boolean, got type '{}'",
                            condition_type.name()
                        ),
                    ));
                }
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Check all rule types in topological order, given precomputed inferred types.
/// Validates:
/// - Branch type consistency (all non-Veto branches must return the same value type)
/// - Condition types (unless clause conditions must be boolean)
/// - All sub-expressions via `check_expression`
fn check_rule_types(
    graph: &Graph,
    rule_order: &[RulePath],
    inferred_types: &HashMap<RulePath, Arc<LemmaType>>,
    resolved_types: &ResolvedTypesMap,
) -> Result<(), Vec<Error>> {
    let mut errors = Vec::new();

    let collect = |result: Result<(), Vec<Error>>, errors: &mut Vec<Error>| {
        if let Err(errs) = result {
            errors.extend(errs);
        }
    };

    for rule_path in rule_order {
        let rule_node = graph
            .rules()
            .get(rule_path)
            .expect("BUG: rule from topological sort not in graph");
        let branches = &rule_node.branches;
        let spec = &rule_node.spec;

        if branches.is_empty() {
            continue;
        }

        let (_, default_result) = &branches[0];
        collect(
            check_expression(default_result, graph, inferred_types, resolved_types, spec),
            &mut errors,
        );
        let default_type =
            infer_expression_type(default_result, graph, inferred_types, resolved_types, spec);

        if default_type.is_anonymous_measure() {
            if let Some(decomp) = default_type.measure_type_decomposition() {
                if !decomp.is_empty() && anonymous_rule_boundary_requires_rejection() {
                    let default_source = default_result
                        .source_location
                        .as_ref()
                        .expect("BUG: default branch result expression has no source location");
                    errors.push(engine_error_at_graph(
                        graph,
                        default_source,
                        anonymous_rule_boundary_error(
                            rule_path,
                            spec,
                            graph,
                            resolved_types,
                            decomp,
                            None,
                        ),
                    ));
                }
            }
        }

        let mut non_veto_type: Option<LemmaType> = None;
        if !default_type.vetoed() && !default_type.is_undetermined() {
            non_veto_type = Some(default_type.as_ref().clone());
        }

        for (branch_index, (condition, result)) in branches.iter().enumerate().skip(1) {
            if let Some(condition_expression) = condition {
                collect(
                    check_expression(
                        condition_expression,
                        graph,
                        inferred_types,
                        resolved_types,
                        spec,
                    ),
                    &mut errors,
                );
                let condition_type = infer_expression_type(
                    condition_expression,
                    graph,
                    inferred_types,
                    resolved_types,
                    spec,
                );
                if !condition_type.vetoed()
                    && !condition_type.is_boolean()
                    && !condition_type.is_undetermined()
                {
                    let condition_source = condition_expression
                        .source_location
                        .as_ref()
                        .expect("BUG: condition expression missing source in check_rule_types");
                    errors.push(engine_error_at_graph(
                        graph,
                        condition_source,
                        format!(
                            "Unless clause condition in rule '{}' must be boolean, got {}",
                            rule_path.rule, condition_type
                        ),
                    ));
                }
            }

            collect(
                check_expression(result, graph, inferred_types, resolved_types, spec),
                &mut errors,
            );
            let result_type =
                infer_expression_type(result, graph, inferred_types, resolved_types, spec);

            if result_type.is_anonymous_measure() {
                if let Some(decomp) = result_type.measure_type_decomposition() {
                    if !decomp.is_empty() && anonymous_rule_boundary_requires_rejection() {
                        let branch_source = result
                            .source_location
                            .as_ref()
                            .expect("BUG: unless branch result expression has no source location");
                        errors.push(engine_error_at_graph(
                            graph,
                            branch_source,
                            anonymous_rule_boundary_error(
                                rule_path,
                                spec,
                                graph,
                                resolved_types,
                                decomp,
                                Some(branch_index),
                            ),
                        ));
                    }
                }
            }

            if !result_type.vetoed() && !result_type.is_undetermined() {
                if non_veto_type.is_none() {
                    non_veto_type = Some(result_type.as_ref().clone());
                } else if let Some(ref existing_type) = non_veto_type {
                    if !existing_type.same_value_type(result_type.as_ref()) {
                        let Some(rule_node) = graph.rules().get(rule_path) else {
                            unreachable!(
                                "BUG: rule type validation referenced missing rule '{}'",
                                rule_path.rule
                            );
                        };
                        let rule_source = &rule_node.source;
                        let default_expr = &branches[0].1;

                        let mut location_parts = vec![format!(
                            "{}:{}:{}",
                            rule_source.source_type, rule_source.span.line, rule_source.span.col
                        )];

                        if let Some(loc) = &default_expr.source_location {
                            location_parts.push(format!(
                                "default branch at {}:{}:{}",
                                loc.source_type, loc.span.line, loc.span.col
                            ));
                        }
                        if let Some(loc) = &result.source_location {
                            location_parts.push(format!(
                                "unless clause {} at {}:{}:{}",
                                branch_index, loc.source_type, loc.span.line, loc.span.col
                            ));
                        }

                        errors.push(Error::validation_with_context(
                            format!("Type mismatch in rule '{}' in spec '{}' ({}): default branch returns {}, but unless clause {} returns {}. All branches must return the same value type (matching base kind and, for measures, dimensions).",
                            rule_path.rule,
                            spec.name,
                            location_parts.join(", "),
                            existing_type.name(),
                            branch_index,
                            result_type.name()),
                            Some(rule_source.clone()),
                            None::<String>,
                            Some(graph.main_spec),
                            None,
                        ));
                    }
                }
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

// =============================================================================
// Phase 3: Apply inferred types to the graph (the only mutation point)
// =============================================================================

/// Write inferred types into the graph's rule nodes.
/// This is the only function that mutates the graph during the validation pipeline.
/// It must only be called after all checks pass (no errors).
fn apply_inferred_types(graph: &mut Graph, inferred_types: HashMap<RulePath, Arc<LemmaType>>) {
    for (rule_path, rule_type) in inferred_types {
        if let Some(rule_node) = graph.rules_mut().get_mut(&rule_path) {
            rule_node.rule_type = rule_type;
        }
    }
}

/// Infer the types of all rules in topological order without performing any validation.
/// Returns a map from rule path to its inferred type.
/// This function is pure: it takes `&Graph` and returns data with no side effects.
fn infer_rule_types(
    graph: &Graph,
    rule_order: &[RulePath],
    resolved_types: &ResolvedTypesMap,
) -> HashMap<RulePath, Arc<LemmaType>> {
    let mut computed_types: HashMap<RulePath, Arc<LemmaType>> = HashMap::new();

    for rule_path in rule_order {
        let rule_node = graph
            .rules()
            .get(rule_path)
            .expect("BUG: rule from topological sort not in graph");
        let branches = &rule_node.branches;
        let spec = &rule_node.spec;

        if branches.is_empty() {
            continue;
        }

        let body_types = branches.iter().map(|(_condition, result)| {
            infer_expression_type(result, graph, &computed_types, resolved_types, spec)
        });
        let rule_type = piecewise_type(body_types);
        computed_types.insert(rule_path.clone(), rule_type);
    }

    computed_types
}

type UnitDecompLookup = HashMap<
    String,
    (
        String,
        BaseMeasureVector,
        crate::computation::rational::RationalInteger,
    ),
>;

fn declared_measure_decomposition(type_name: &str, lemma_type: &LemmaType) -> BaseMeasureVector {
    match &lemma_type.specifications {
        TypeSpecification::Measure { traits, .. }
            if traits.contains(&semantics::MeasureTrait::Duration) =>
        {
            duration_decomposition()
        }
        TypeSpecification::Measure { traits, .. }
            if traits.contains(&semantics::MeasureTrait::Calendar) =>
        {
            calendar_decomposition()
        }
        _ => {
            let dimension_key = lemma_type
                .measure_family_name()
                .unwrap_or(type_name)
                .to_string();
            [(dimension_key, 1i32)].into_iter().collect()
        }
    }
}

/// Extract owned `LemmaType` from an `Arc` that the build pipeline holds exclusively.
/// During build, every `Arc<LemmaType>` in `ResolvedSpecTypes.{resolved,unit_index}` has
/// refcount 1 until the value is moved into [`ExecutionPlan::resolved_types`]; `try_unwrap`
/// always succeeds without cloning. The fallback clone is defensive and never executes
/// in normal flow.
fn arc_unwrap(arc: Arc<LemmaType>) -> LemmaType {
    Arc::try_unwrap(arc).unwrap_or_else(|shared| (*shared).clone())
}

fn unit_index_arc_declares_unit(lemma_type: &LemmaType, unit_name: &str) -> bool {
    match &lemma_type.specifications {
        TypeSpecification::Measure { units, .. } => units.get(unit_name).is_ok(),
        TypeSpecification::Ratio { units, .. } => units.get(unit_name).is_ok(),
        _ => false,
    }
}

fn sync_unit_index_from_resolved(
    resolved: &IndexMap<String, Arc<LemmaType>>,
    unit_index: UnitIndex,
) -> UnitIndex {
    let mut synced = UnitIndex::new();
    for (unit_name, owner) in unit_index.into_iter_owners() {
        let pre_decomp_type = owner.owning_type;
        let lookup_name = pre_decomp_type
            .name
            .as_deref()
            .or_else(|| pre_decomp_type.measure_family_name());
        let post = if pre_decomp_type.is_measure() {
            let type_name = lookup_name
                .expect("BUG: measure arc in unit_index must carry name or family for sync");
            if let Some(synced_type) = resolved.get(type_name).or_else(|| {
                pre_decomp_type
                    .measure_family_name()
                    .and_then(|family| resolved.get(family))
            }) {
                Arc::clone(synced_type)
            } else if pre_decomp_type.measure_type_decomposition().is_some() {
                pre_decomp_type
            } else {
                panic!(
                    "BUG: measure unit_index unit '{}' type '{}' must exist in resolved or carry decomposition after import merge",
                    unit_name, type_name
                )
            }
        } else {
            lookup_name
                .and_then(|type_name| {
                    resolved.get(type_name).or_else(|| {
                        pre_decomp_type
                            .measure_family_name()
                            .and_then(|family| resolved.get(family))
                    })
                })
                .map(Arc::clone)
                .unwrap_or(pre_decomp_type)
        };
        if unit_index_arc_declares_unit(post.as_ref(), &unit_name) {
            synced.insert_owner(
                unit_name,
                UnitOwner {
                    owning_type: post,
                    type_name: owner.type_name,
                    import_alias: owner.import_alias,
                },
            );
        }
    }
    merge_family_root_measure_units_into_index(resolved, synced)
}

/// Ensure every unit declared on a family-root measure type in `resolved` has an index entry.
fn merge_family_root_measure_units_into_index(
    resolved: &IndexMap<String, Arc<LemmaType>>,
    mut unit_index: UnitIndex,
) -> UnitIndex {
    for (type_name, lemma_type) in resolved {
        let TypeSpecification::Measure { units, .. } = &lemma_type.specifications else {
            continue;
        };
        let is_family_root = lemma_type
            .measure_family_name()
            .is_some_and(|family| family == type_name.as_str());
        if !is_family_root {
            continue;
        }
        for unit in units.iter() {
            let already = unit_index.owners_for(&unit.name).iter().any(|owner| {
                owner.type_name == *type_name
                    || owner.owning_type.same_measure_family(lemma_type.as_ref())
            });
            if !already {
                unit_index.insert_owner(
                    unit.name.clone(),
                    UnitOwner {
                        owning_type: Arc::clone(lemma_type),
                        type_name: type_name.clone(),
                        import_alias: None,
                    },
                );
            }
        }
    }
    unit_index
}

/// `uses`-merged measure rows in `unit_index` can still have empty `decomposition` until synced from
/// `resolved`. Compound unit resolution consults `unit_index` first; fill simple base measures here
/// before building [`UnitDecompLookup`].
fn repair_empty_simple_measure_decomposition_in_unit_index(unit_index: UnitIndex) -> UnitIndex {
    let mut repaired = UnitIndex::new();
    for (unit_key, owner) in unit_index.into_iter_owners() {
        repaired.insert_owner(
            unit_key,
            UnitOwner {
                owning_type: Arc::new(repair_simple_measure_decomposition(arc_unwrap(
                    owner.owning_type,
                ))),
                type_name: owner.type_name,
                import_alias: owner.import_alias,
            },
        );
    }
    repaired
}

fn repair_simple_measure_decomposition(lemma_type: LemmaType) -> LemmaType {
    let Some(base_decomp) = simple_measure_repair_decomposition(&lemma_type) else {
        return lemma_type;
    };
    lemma_type.map_measure(|units, _decomposition| {
        let units = units.map(|u| u.with_decomposition(base_decomp.clone()));
        (units, Some(base_decomp))
    })
}

fn simple_measure_repair_decomposition(lemma_type: &LemmaType) -> Option<BaseMeasureVector> {
    let TypeSpecification::Measure {
        units,
        decomposition,
        ..
    } = &lemma_type.specifications
    else {
        return None;
    };
    if decomposition.is_some() {
        return None;
    }
    if units.is_empty() || units.iter().any(|u| !measure_unit_is_simple(u)) {
        return None;
    }
    let type_name = lemma_type.name.as_deref()?;
    if type_name.is_empty() {
        return None;
    }
    let candidate = declared_measure_decomposition(type_name, lemma_type);
    (!candidate.is_empty()).then_some(candidate)
}

fn owning_measure_type_name_for_unit(
    unit_name: &str,
    lookup: &UnitDecompLookup,
    unit_index: &UnitIndex,
) -> Option<String> {
    if let Some((owning_measure_name, _, _)) = lookup.get(unit_name) {
        return Some(owning_measure_name.clone());
    }
    unit_index
        .resolve(unit_name)
        .ok()
        .and_then(|(_, lemma_type)| {
            lemma_type
                .name
                .clone()
                .or_else(|| lemma_type.measure_family_name().map(str::to_string))
        })
}

/// Order compound measure types so every referenced unit from another compound type is resolved first.
fn sort_derived_measure_types_for_resolution(
    spec_name: &str,
    derived_measure_type_names: Vec<String>,
    resolved: &IndexMap<String, Arc<LemmaType>>,
    lookup: &UnitDecompLookup,
    unit_index: &UnitIndex,
    source_for: &dyn Fn(&str) -> Option<Source>,
) -> Result<Vec<String>, Error> {
    let derived_measure_type_count = derived_measure_type_names.len();
    if derived_measure_type_count == 0 {
        return Ok(derived_measure_type_names);
    }

    let type_index: HashMap<&str, usize> = derived_measure_type_names
        .iter()
        .enumerate()
        .map(|(index, name)| (name.as_str(), index))
        .collect();

    let mut dependency_sets: Vec<BTreeSet<usize>> =
        vec![BTreeSet::new(); derived_measure_type_count];

    for (dependent_index, type_name) in derived_measure_type_names.iter().enumerate() {
        let TypeSpecification::Measure { units, .. } = &resolved[type_name].specifications else {
            continue;
        };
        for unit in units.iter() {
            for (factor_unit_name, _) in &unit.derived_measure_factors {
                let Some(owning_measure_name) =
                    owning_measure_type_name_for_unit(factor_unit_name, lookup, unit_index)
                else {
                    continue;
                };
                let Some(dependency_index) = type_index.get(owning_measure_name.as_str()).copied()
                else {
                    continue;
                };
                if dependency_index == dependent_index {
                    continue;
                }
                dependency_sets[dependent_index].insert(dependency_index);
            }
        }
    }

    let mut in_degree = vec![0usize; derived_measure_type_count];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); derived_measure_type_count];

    for (dependent_index, dependencies) in dependency_sets.iter().enumerate() {
        for &dependency_index in dependencies {
            in_degree[dependent_index] += 1;
            dependents[dependency_index].push(dependent_index);
        }
    }

    let mut queue: VecDeque<usize> = (0..derived_measure_type_count)
        .filter(|&index| in_degree[index] == 0)
        .collect();
    let mut sorted_indices: Vec<usize> = Vec::with_capacity(derived_measure_type_count);

    while let Some(index) = queue.pop_front() {
        sorted_indices.push(index);
        for &dependent_index in &dependents[index] {
            in_degree[dependent_index] -= 1;
            if in_degree[dependent_index] == 0 {
                queue.push_back(dependent_index);
            }
        }
    }

    if sorted_indices.len() != derived_measure_type_count {
        let mut cycle_type_names: Vec<String> = (0..derived_measure_type_count)
            .filter(|&index| in_degree[index] > 0)
            .map(|index| derived_measure_type_names[index].clone())
            .collect();
        cycle_type_names.sort();
        return Err(Error::validation(
            format!(
                "In spec '{}': circular compound measure type dependency among: {}",
                spec_name,
                cycle_type_names.join(", ")
            ),
            source_for(&cycle_type_names[0]),
            None::<String>,
        ));
    }

    Ok(sorted_indices
        .into_iter()
        .map(|index| derived_measure_type_names[index].clone())
        .collect())
}

/// Build the reverse signature index over every named-measure-type unit in the spec.
/// Each unit's canonical-form `derived_measure_factors` becomes a key; the value is the
/// owning unit name and type. An ambiguity (the same canonical key matched by two units in
/// distinct types) is a planning error: the spec must rename one of the units or change
/// its factor decomposition so it is unique.
pub(crate) fn build_signature_index(
    spec_name: &str,
    unit_index: &UnitIndex,
) -> Result<crate::computation::arithmetic::SignatureIndex, Error> {
    use crate::computation::arithmetic::SignatureIndex;
    let mut signature_index: SignatureIndex = SignatureIndex::new();
    let mut sorted_units: Vec<_> = unit_index.iter_entries().collect();
    sorted_units.sort_by_key(|(name, _)| *name);
    for (unit_name, lemma_type) in sorted_units {
        let unit_name = unit_name.to_string();
        let TypeSpecification::Measure { units, .. } = &lemma_type.specifications else {
            continue;
        };
        let unit = units.get(unit_name.as_str()).map_err(|_| {
            Error::validation(
                format!(
                    "In spec '{}': unit_index entry '{}' is not declared on measure type '{}'",
                    spec_name,
                    unit_name,
                    lemma_type.name()
                ),
                None::<Source>,
                None::<String>,
            )
        })?;
        // Identity signatures `[(name, 1)]` collide when the same bare name has
        // multiple owners. Skip those; keep unique identities so expand can
        // promote compounds that reduce to a single unique unit (e.g. eur).
        if measure_unit_is_simple(unit) && unit_index.owners_for(&unit_name).len() != 1 {
            continue;
        }
        let owning_type_name = lemma_type.name.clone().unwrap_or_default();
        let signature = unit.derived_measure_factors.clone();
        match signature_index.get(&signature) {
            Some((existing_unit_name, existing_owning_type))
                if existing_owning_type.name() != lemma_type.name() =>
            {
                let existing_type_name = existing_owning_type.name.clone().unwrap_or_default();
                return Err(Error::validation(
                    format!(
                        "In spec '{}': ambiguous unit signature {:?}: matched by '{}' in type '{}' and '{}' in type '{}'. \
                         Rename one or differentiate factors.",
                        spec_name,
                        signature,
                        existing_unit_name,
                        existing_type_name,
                        unit_name,
                        owning_type_name,
                    ),
                    None::<Source>,
                    None::<String>,
                ));
            }
            _ => {
                signature_index.insert(signature, (unit_name.clone(), Arc::clone(lemma_type)));
            }
        }
    }
    Ok(signature_index)
}

/// Rebuild [`MeasureRange`] (and other range) specs for `ParentType::Ranged` declarations
/// after the decomposition pass has populated parent measure types.
fn refresh_named_range_specs(
    resolver: &TypeResolver<'_>,
    spec: &LemmaSpec,
    data_defs: &HashMap<String, DataTypeDef>,
    resolved: &mut TypeMap,
    declared_suggestions: &mut IndexMap<String, ValueKind>,
    already_resolved: &ResolvedTypesMap<'_>,
    at: &EffectiveDate,
) -> Vec<Error> {
    let mut errors = Vec::new();
    for (type_name, def) in data_defs {
        let ParentType::Ranged { .. } = &def.parent else {
            continue;
        };
        let element = element_parent_type(&def.parent);
        let element_type = match element {
            ParentType::Custom { name } => resolved.get(name.as_str()).cloned().or_else(|| {
                spec.data
                    .iter()
                    .filter_map(|row| match &row.value {
                        ParsedDataValue::Import { spec_ref, .. } => Some(spec_ref),
                        _ => None,
                    })
                    .find_map(|spec_ref| {
                        let (_, imported_spec) = resolver
                            .resolve_spec_for_import(spec, spec_ref, &def.source, at)
                            .ok()?;
                        already_resolved
                            .iter()
                            .find(|(_, s, _)| discovery::same_loaded_spec(s, imported_spec))
                            .and_then(|(_, _, rts)| rts.resolved.get(name.as_str()).cloned())
                    })
            }),
            ParentType::Qualified {
                spec_alias,
                inner: qualified_inner,
            } => {
                let ParentType::Custom { name } = qualified_inner.as_ref() else {
                    continue;
                };
                let import_name = format!("{spec_alias}.{name}");
                if let Some(local) = resolved
                    .get(name.as_str())
                    .or_else(|| resolved.get(import_name.as_str()))
                    .cloned()
                {
                    Some(local)
                } else {
                    let spec_ref = ast::SpecRef::same_repository(spec_alias.clone());
                    match resolver.resolve_spec_for_import(spec, &spec_ref, &def.source, at) {
                        Ok((_, target_spec)) => already_resolved
                            .iter()
                            .find(|(_, imported, _)| {
                                discovery::same_loaded_spec(imported, target_spec)
                            })
                            .and_then(|(_, _, rts)| rts.resolved.get(name.as_str()).cloned()),
                        Err(_) => None,
                    }
                }
            }
            ParentType::Primitive { .. } => continue,
            ParentType::Ranged { .. } => {
                unreachable!("BUG: element_parent_type must unwrap Ranged")
            }
        };
        let Some(endpoint_type) = element_type else {
            let missing_name = match element {
                ParentType::Custom { name } => name.clone(),
                ParentType::Qualified {
                    spec_alias,
                    inner: qualified_inner,
                } => {
                    let ParentType::Custom { name } = qualified_inner.as_ref() else {
                        continue;
                    };
                    format!("{spec_alias}.{name}")
                }
                _ => continue,
            };
            errors.push(Error::validation_with_context(
                format!(
                    "In spec '{}': ranged type '{}' references missing element type '{}'",
                    spec.name, type_name, missing_name
                ),
                Some(def.source.clone()),
                None::<String>,
                Some(spec),
                None,
            ));
            continue;
        };
        let Some(range_spec) = endpoint_type.specifications.range_from_element() else {
            continue;
        };
        let Some(lemma_type) = resolved.get_mut(type_name.as_str()) else {
            continue;
        };
        let mut updated = lemma_type.as_ref().clone();
        updated.specifications = range_spec;
        *lemma_type = Arc::new(updated);

        if let Some(ValueKind::Range(left, right)) =
            declared_suggestions.get_mut(type_name.as_str())
        {
            let stamp_for_value = |value: &ValueKind| -> Arc<LemmaType> {
                match value {
                    ValueKind::Number(_) => Arc::clone(semantics::primitive_number_arc()),
                    ValueKind::Text(_) => Arc::clone(semantics::primitive_text_arc()),
                    ValueKind::Boolean(_) => Arc::clone(semantics::primitive_boolean_arc()),
                    ValueKind::Date(_) => Arc::clone(semantics::primitive_date_arc()),
                    ValueKind::Time(_) => Arc::clone(semantics::primitive_time_arc()),
                    ValueKind::Ratio(_) => Arc::clone(semantics::primitive_ratio_arc()),
                    ValueKind::Measure(_) => {
                        Arc::new(LemmaType::primitive(TypeSpecification::measure()))
                    }
                    other => panic!(
                        "BUG: named range suggestion endpoint has non-endpoint kind {other:?}"
                    ),
                }
            };
            let left_value = left.value.clone();
            let right_value = right.value.clone();
            let coerced_left = match Graph::coerce_literal_to_schema_type(
                &TypedLiteral {
                    value: left_value.clone(),
                    lemma_type: stamp_for_value(&left_value),
                },
                &endpoint_type,
            ) {
                Ok(coerced) => coerced,
                Err(message) => {
                    errors.push(Error::validation_with_context(
                        format!(
                            "In spec '{}': named range default left endpoint for '{}': {message}",
                            spec.name, type_name
                        ),
                        Some(def.source.clone()),
                        None::<String>,
                        Some(spec),
                        None,
                    ));
                    continue;
                }
            };
            let coerced_right = match Graph::coerce_literal_to_schema_type(
                &TypedLiteral {
                    value: right_value.clone(),
                    lemma_type: stamp_for_value(&right_value),
                },
                &endpoint_type,
            ) {
                Ok(coerced) => coerced,
                Err(message) => {
                    errors.push(Error::validation_with_context(
                        format!(
                            "In spec '{}': named range default right endpoint for '{}': {message}",
                            spec.name, type_name
                        ),
                        Some(def.source.clone()),
                        None::<String>,
                        Some(spec),
                        None,
                    ));
                    continue;
                }
            };
            *declared_suggestions
                .get_mut(type_name.as_str())
                .expect("BUG: named range default removed while refreshing endpoints") =
                ValueKind::Range(
                    Box::new(coerced_left.to_literal()),
                    Box::new(coerced_right.to_literal()),
                );
        }
    }
    errors
}

/// Apply range-local constraints after [`refresh_named_range_specs`] for named ranged types.
fn apply_deferred_named_range_constraints(
    spec: &LemmaSpec,
    data_defs: &HashMap<String, DataTypeDef>,
    resolved: &mut TypeMap,
    declared_suggestions: &mut IndexMap<String, ValueKind>,
    declared_fills: &mut IndexMap<String, ValueKind>,
    type_sources: &HashMap<String, Source>,
) -> Vec<Error> {
    let mut errors = Vec::new();
    for (type_name, def) in data_defs {
        if !should_defer_ranged_constraints(&def.parent) {
            continue;
        }
        let Some(constraints) = &def.constraints else {
            continue;
        };
        let Some(lemma_type) = resolved.get(type_name.as_str()).cloned() else {
            continue;
        };
        let mut declared_suggestion: Option<RawSuggestion> = None;
        let mut declared_fill: Option<RawSuggestion> = None;
        match apply_constraints_to_spec(
            spec,
            &constraint_application_type_name(&def.parent, type_name),
            lemma_type.specifications.clone(),
            constraints,
            &def.source,
            &mut declared_suggestion,
            &mut declared_fill,
        ) {
            Ok(updated_specs) => {
                let mut updated = lemma_type.as_ref().clone();
                updated.specifications = updated_specs;
                let updated_arc = Arc::new(updated);
                if let Some(raw) = declared_suggestion {
                    match value_kind_from_raw_suggestion(
                        raw,
                        &updated_arc.specifications,
                        type_name.as_str(),
                    ) {
                        Ok(value_kind) => {
                            declared_suggestions.insert(type_name.clone(), value_kind);
                        }
                        Err(message) => {
                            let source = type_sources
                                .get(type_name.as_str())
                                .cloned()
                                .unwrap_or_else(|| def.source.clone());
                            errors.push(Error::validation_with_context(
                                message,
                                Some(source),
                                None::<String>,
                                Some(spec),
                                None,
                            ));
                        }
                    }
                }
                if let Some(raw) = declared_fill {
                    match value_kind_from_raw_suggestion(
                        raw,
                        &updated_arc.specifications,
                        type_name.as_str(),
                    ) {
                        Ok(value_kind) => {
                            declared_fills.insert(type_name.clone(), value_kind);
                        }
                        Err(message) => {
                            let source = type_sources
                                .get(type_name.as_str())
                                .cloned()
                                .unwrap_or_else(|| def.source.clone());
                            errors.push(Error::validation_with_context(
                                message,
                                Some(source),
                                None::<String>,
                                Some(spec),
                                None,
                            ));
                        }
                    }
                }
                resolved.insert(type_name.clone(), updated_arc);
            }
            Err(constraint_errors) => errors.extend(constraint_errors),
        }
    }
    errors
}

type TypeMap = IndexMap<String, Arc<LemmaType>>;

fn resolve_measure_decompositions(
    spec_name: &str,
    mut resolved: TypeMap,
    mut unit_index: UnitIndex,
    type_sources: &HashMap<String, Source>,
) -> (TypeMap, UnitIndex, Vec<Error>) {
    let mut errors: Vec<Error> = Vec::new();

    let source_for = |type_name: &str| -> Option<Source> { type_sources.get(type_name).cloned() };

    let base_type_names: Vec<String> = resolved
        .iter()
        .filter_map(|(name, lt)| {
            if let TypeSpecification::Measure { units, .. } = &lt.specifications {
                if units.iter().all(measure_unit_is_simple) {
                    return Some(name.clone());
                }
            }
            None
        })
        .collect();

    for type_name in &base_type_names {
        let owned = arc_unwrap(
            resolved
                .shift_remove(type_name)
                .expect("BUG: type_name comes from resolved's own keys"),
        );
        let base_decomp = declared_measure_decomposition(type_name, &owned);

        let reserved_collision = reserved_calendar_units_in_measure(&owned);
        if !reserved_collision.is_empty() && !owned.has_trait_calendar() {
            errors.push(Error::validation(
                format!(
                    "In spec '{}': measure type '{}' declares unit(s) {:?} which are reserved calendar unit names.",
                    spec_name, type_name, reserved_collision
                ),
                source_for(type_name),
                None::<String>,
            ));
            resolved.insert(type_name.clone(), Arc::new(owned));
            continue;
        }

        let updated = owned.map_measure(|units, _decomposition| {
            let units = units.map(|u| u.with_decomposition(base_decomp.clone()));
            (units, Some(base_decomp.clone()))
        });
        resolved.insert(type_name.clone(), Arc::new(updated));
    }

    unit_index = repair_empty_simple_measure_decomposition_in_unit_index(unit_index);

    let mut lookup = UnitDecompLookup::new();

    for (unit_name, lemma_type) in unit_index.iter_entries() {
        let unit_name = unit_name.to_string();
        if let TypeSpecification::Measure {
            decomposition: Some(decomposition),
            units,
            ..
        } = &lemma_type.specifications
        {
            let measure_name = lemma_type.name.clone().unwrap_or_default();
            let factor = units
                .iter()
                .find(|u| u.name == unit_name)
                .unwrap_or_else(|| {
                    panic!(
                        "BUG: unit_name '{}' from unit_index must be in its owning type's units",
                        unit_name
                    )
                })
                .factor
                .clone();
            // Unique bare names only in decomp lookup; ambiguous factors resolve via UnitIndex.
            if unit_index.has_unique_owner(unit_name.as_str()) {
                lookup.insert(
                    unit_name.clone(),
                    (measure_name, decomposition.clone(), factor),
                );
            }
        }
    }

    for (type_name, lemma_type) in resolved.iter() {
        if let TypeSpecification::Measure {
            units,
            decomposition: Some(decomposition),
            ..
        } = &lemma_type.specifications
        {
            let is_defining_type = lemma_type
                .measure_family_name()
                .map(|family| family == type_name.as_str())
                .unwrap_or(false);
            if !is_defining_type {
                continue;
            }
            for unit in units.iter() {
                lookup.insert(
                    unit.name.clone(),
                    (
                        type_name.clone(),
                        decomposition.clone(),
                        unit.factor.clone(),
                    ),
                );
            }
        }
    }

    let derived_measure_type_names_unsorted: Vec<String> = resolved
        .iter()
        .filter_map(|(name, lemma_type)| {
            if let TypeSpecification::Measure { units, .. } = &lemma_type.specifications {
                if units.iter().any(|unit| !measure_unit_is_simple(unit)) {
                    return Some(name.clone());
                }
            }
            None
        })
        .collect();

    let derived_measure_type_names = match sort_derived_measure_types_for_resolution(
        spec_name,
        derived_measure_type_names_unsorted,
        &resolved,
        &lookup,
        &unit_index,
        &|type_name| source_for(type_name),
    ) {
        Ok(sorted) => sorted,
        Err(error) => {
            errors.push(error);
            return (resolved, unit_index, errors);
        }
    };

    for type_name in &derived_measure_type_names {
        let type_source = source_for(type_name);

        let units_snapshot = match &resolved[type_name].specifications {
            TypeSpecification::Measure { units, .. } => units.clone(),
            _ => continue,
        };

        let mut resolved_type_decomp: Option<BaseMeasureVector> = None;
        let mut unit_errors: Vec<Error> = Vec::new();
        let mut resolved_unit_factors: Vec<Option<crate::computation::rational::RationalInteger>> =
            vec![None; units_snapshot.len()];

        for (unit_idx, unit) in units_snapshot.iter().enumerate() {
            if crate::planning::semantics::calendar_unit_factor(&unit.name).is_some()
                && !resolved[type_name].has_trait_calendar()
            {
                unit_errors.push(Error::validation(
                    format!(
                        "In spec '{}': measure type '{}' declares unit '{}' which is a reserved calendar unit name.",
                        spec_name, type_name, unit.name
                    ),
                    type_source.clone(),
                    None::<String>,
                ));
                continue;
            }
            if measure_unit_is_simple(unit) {
                let simple_decomp = declared_measure_decomposition(type_name, &resolved[type_name]);

                if let Some(existing) = &resolved_type_decomp {
                    if existing != &simple_decomp {
                        unit_errors.push(Error::validation(
                            format!(
                                "In spec '{}': measure type '{}' has inconsistent unit decompositions. \
                                 Unit '{}' is a simple unit (decomposition {{{}: 1}}) but other units \
                                 have decomposition {:?}.",
                                spec_name, type_name, unit.name, type_name, existing
                            ),
                            type_source.clone(),
                            None::<String>,
                        ));
                    }
                } else {
                    resolved_type_decomp = Some(simple_decomp);
                }

                resolved_unit_factors[unit_idx] = Some(unit.factor.clone());
                continue;
            }

            match resolve_compound_unit(
                spec_name,
                type_name,
                unit,
                &lookup,
                &unit_index,
                type_source.as_ref(),
            ) {
                Ok((unit_decomp, derived_factor)) => {
                    if let Some(existing) = &resolved_type_decomp {
                        if existing != &unit_decomp {
                            unit_errors.push(Error::validation(
                                format!(
                                    "In spec '{}': measure type '{}' has inconsistent unit \
                                     decompositions. Unit '{}' resolved to {:?} but other units \
                                     resolved to {:?}. All units of a measure must measure the same \
                                     physical measure.",
                                    spec_name, type_name, unit.name, unit_decomp, existing
                                ),
                                type_source.clone(),
                                None::<String>,
                            ));
                        }
                    } else {
                        resolved_type_decomp = Some(unit_decomp);
                    }

                    resolved_unit_factors[unit_idx] = Some(derived_factor);
                }
                Err(err) => unit_errors.push(err),
            }
        }

        if !unit_errors.is_empty() {
            errors.extend(unit_errors);
            continue;
        }

        let type_decomp = match resolved_type_decomp {
            Some(d) => d,
            None => continue,
        };

        let owned = arc_unwrap(
            resolved
                .shift_remove(type_name)
                .expect("BUG: type_name comes from resolved's own keys"),
        );

        let updated = owned.map_measure(|units, _decomposition| {
            let units = MeasureUnits(
                units
                    .0
                    .into_iter()
                    .enumerate()
                    .map(|(idx, u)| {
                        let u = u.with_decomposition(type_decomp.clone());
                        match resolved_unit_factors[idx].clone() {
                            Some(factor) => u.with_factor(factor.clone()),
                            None => u,
                        }
                    })
                    .collect(),
            );
            (units, Some(type_decomp.clone()))
        });

        if let TypeSpecification::Measure { units, .. } = &updated.specifications {
            for unit in units.iter() {
                lookup.insert(
                    unit.name.clone(),
                    (type_name.clone(), type_decomp.clone(), unit.factor.clone()),
                );
            }
        }
        resolved.insert(type_name.clone(), Arc::new(updated));
    }

    let resolved = canonicalize_unit_signatures(resolved);
    let unit_index = sync_unit_index_from_resolved(&resolved, unit_index);
    let unit_index = repair_empty_simple_measure_decomposition_in_unit_index(unit_index);
    let unit_index = canonicalize_unit_index_signatures(unit_index);

    (resolved, unit_index, errors)
}

/// Functional wrapper around the still-`&mut`-based
/// [`semantics::finalize_measure_unit_constraint_magnitudes`]. Consumes the
/// `LemmaType`, performs the validation, and returns either the updated value
/// or an error message (the type is dropped on failure).
fn finalize_lemma_measure_magnitudes(
    lemma_type: LemmaType,
    declared_suggestion: Option<&ValueKind>,
    type_name: &str,
) -> Result<LemmaType, String> {
    let LemmaType {
        name,
        mut specifications,
        extends,
        measure_binding_unit,
    } = lemma_type;
    semantics::finalize_measure_unit_constraint_magnitudes(
        &mut specifications,
        declared_suggestion,
        type_name,
    )?;
    Ok(LemmaType {
        name,
        specifications,
        extends,
        measure_binding_unit,
    })
}

fn finalize_measure_magnitudes_in_resolved(
    resolved: IndexMap<String, Arc<LemmaType>>,
    declared_suggestions: &IndexMap<String, ValueKind>,
    type_sources: &HashMap<String, Source>,
    spec_name: &str,
    spec: &LemmaSpec,
) -> (IndexMap<String, Arc<LemmaType>>, Vec<Error>) {
    resolved.into_iter().fold(
        (IndexMap::new(), Vec::new()),
        |(mut acc, mut errors), (type_name, arc)| {
            let source = type_sources.get(&type_name).cloned().unwrap_or_else(|| {
                unreachable!(
                    "BUG: resolved type '{}' has no corresponding DataTypeDef in spec '{}'",
                    type_name, spec_name
                )
            });
            let fallback = (*arc).clone();
            let lemma_type = match finalize_lemma_measure_magnitudes(
                arc_unwrap(arc),
                declared_suggestions.get(&type_name),
                &type_name,
            ) {
                Ok(lt) => lt,
                Err(message) => {
                    errors.push(Error::validation_with_context(
                        format!(
                            "Type '{}' has invalid measure unit constraints: {}",
                            type_name, message
                        ),
                        Some(source),
                        None::<String>,
                        Some(spec),
                        None,
                    ));
                    fallback
                }
            };
            acc.insert(type_name, Arc::new(lemma_type));
            (acc, errors)
        },
    )
}

fn finalize_measure_magnitudes_in_unit_index(
    unit_index: UnitIndex,
    declared_suggestions: &IndexMap<String, ValueKind>,
    type_sources: &HashMap<String, Source>,
    all_data_types: &[(&LemmaSpec, HashMap<String, DataTypeDef>)],
    spec: &LemmaSpec,
) -> (UnitIndex, Vec<Error>) {
    let mut out = UnitIndex::new();
    let mut errors = Vec::new();
    let mut finalized_types: HashMap<(Option<String>, String), Arc<LemmaType>> = HashMap::new();

    for (unit_name, owner) in unit_index.into_iter_owners() {
        let cache_key = (owner.import_alias.clone(), owner.type_name.clone());
        if let Some(existing) = finalized_types.get(&cache_key) {
            out.insert_owner(
                unit_name,
                UnitOwner {
                    owning_type: Arc::clone(existing),
                    type_name: owner.type_name,
                    import_alias: owner.import_alias,
                },
            );
            continue;
        }

        let arc = owner.owning_type;
        let type_name_opt = arc
            .name
            .as_deref()
            .or_else(|| arc.measure_family_name())
            .map(str::to_string);
        let Some(type_name) = type_name_opt else {
            let arc = Arc::clone(&arc);
            finalized_types.insert(cache_key, Arc::clone(&arc));
            out.insert_owner(
                unit_name,
                UnitOwner {
                    owning_type: arc,
                    type_name: owner.type_name,
                    import_alias: owner.import_alias,
                },
            );
            continue;
        };
        if !arc.is_measure() {
            finalized_types.insert(cache_key, Arc::clone(&arc));
            out.insert_owner(
                unit_name,
                UnitOwner {
                    owning_type: arc,
                    type_name: owner.type_name,
                    import_alias: owner.import_alias,
                },
            );
            continue;
        }
        let fallback = (*arc).clone();
        let lemma_type = match finalize_lemma_measure_magnitudes(
            arc_unwrap(arc),
            declared_suggestions.get(type_name.as_str()),
            type_name.as_str(),
        ) {
            Ok(lt) => lt,
            Err(message) => {
                let source = type_sources
                    .get(type_name.as_str())
                    .cloned()
                    .or_else(|| {
                        all_data_types.iter().find_map(|(_, defs)| {
                            defs.get(type_name.as_str()).map(|def| def.source.clone())
                        })
                    })
                    .unwrap_or_else(|| {
                        unreachable!(
                            "BUG: measure type '{}' in unit_index has no DataTypeDef source",
                            type_name
                        )
                    });
                errors.push(Error::validation_with_context(
                    format!(
                        "Type '{}' has invalid measure unit constraints: {}",
                        type_name, message
                    ),
                    Some(source),
                    None::<String>,
                    Some(spec),
                    None,
                ));
                fallback
            }
        };
        let arc = Arc::new(lemma_type);
        finalized_types.insert(cache_key, Arc::clone(&arc));
        out.insert_owner(
            unit_name,
            UnitOwner {
                owning_type: arc,
                type_name: owner.type_name,
                import_alias: owner.import_alias,
            },
        );
    }
    (out, errors)
}

/// Populate every unit's `derived_measure_factors` to its canonical symbolic signature.
///
/// - Compound declarations keep their parsed factors, canonicalized.
/// - Simple units (empty `derived_measure_factors`) get `[(unit.name, 1)]`.
///
/// After this pass every unit carries a non-empty, canonical-form signature so cross-type
/// arithmetic can combine them deterministically and `signature_index` can be built.
fn canonicalize_unit_signatures(types: TypeMap) -> TypeMap {
    types
        .into_iter()
        .map(|(name, arc)| {
            (
                name,
                Arc::new(canonicalize_lemma_unit_signatures(arc_unwrap(arc))),
            )
        })
        .collect()
}

fn canonicalize_unit_index_signatures(unit_index: UnitIndex) -> UnitIndex {
    let mut out = UnitIndex::new();
    for (bare, owner) in unit_index.into_iter_owners() {
        out.insert_owner(
            bare,
            UnitOwner {
                owning_type: Arc::new(canonicalize_lemma_unit_signatures(arc_unwrap(
                    owner.owning_type,
                ))),
                type_name: owner.type_name,
                import_alias: owner.import_alias,
            },
        );
    }
    out
}

fn canonicalize_lemma_unit_signatures(lemma_type: LemmaType) -> LemmaType {
    lemma_type.map_measure(|units, decomposition| {
        let units = units.map(canonicalize_unit_for_measure);
        (units, decomposition)
    })
}

fn reserved_calendar_units_in_measure(lemma_type: &LemmaType) -> Vec<String> {
    let TypeSpecification::Measure { units, .. } = &lemma_type.specifications else {
        return Vec::new();
    };
    units
        .iter()
        .filter(|u| crate::planning::semantics::calendar_unit_factor(&u.name).is_some())
        .map(|u| u.name.clone())
        .collect()
}

fn measure_unit_is_simple(unit: &MeasureUnit) -> bool {
    unit.derived_measure_factors.is_empty()
        || (unit.derived_measure_factors.len() == 1
            && unit.derived_measure_factors[0].0 == unit.name
            && unit.derived_measure_factors[0].1 == 1)
}

fn canonicalize_unit_for_measure(unit: MeasureUnit) -> MeasureUnit {
    let factors = if measure_unit_is_simple(&unit) {
        vec![(unit.name.clone(), 1)]
    } else {
        canonicalize_signature(&unit.derived_measure_factors)
    };
    unit.with_derived_measure_factors(factors)
}

fn resolve_compound_unit(
    spec_name: &str,
    declaring_type_name: &str,
    unit: &MeasureUnit,
    lookup: &UnitDecompLookup,
    unit_index: &UnitIndex,
    source: Option<&Source>,
) -> Result<
    (
        BaseMeasureVector,
        crate::computation::rational::RationalInteger,
    ),
    Error,
> {
    use crate::computation::rational::{checked_mul, checked_pow_i32};

    let mut result: BaseMeasureVector = BaseMeasureVector::new();
    let mut derived_factor = unit.factor.clone();

    for (measure_ref, exponent) in &unit.derived_measure_factors {
        let (owning_measure_name, owning_decomp, unit_factor) = if let Some(entry) =
            lookup.get(measure_ref.as_str())
        {
            (entry.0.clone(), entry.1.clone(), entry.2.clone())
        } else {
            let (bare, owning_type) = unit_index.resolve(measure_ref).map_err(|err| {
                Error::validation(
                    format!(
                        "In spec '{}': unit '{}' in measure type '{}' references '{}': {}. \
                             Add `uses <spec>` (or declare the owning measure type in this spec) \
                             so its units are in scope.",
                        spec_name, unit.name, declaring_type_name, measure_ref, err
                    ),
                    source.cloned(),
                    None::<String>,
                )
            })?;
            let TypeSpecification::Measure {
                decomposition: Some(decomposition),
                units,
                ..
            } = &owning_type.specifications
            else {
                return Err(Error::validation(
                        format!(
                            "In spec '{}': unit '{}' in measure type '{}' references '{}' which did not resolve to a measure unit",
                            spec_name, unit.name, declaring_type_name, measure_ref
                        ),
                        source.cloned(),
                        None::<String>,
                    ));
            };
            let factor = units
                .iter()
                .find(|u| u.name == bare)
                .unwrap_or_else(|| {
                    panic!(
                        "BUG: resolved unit '{}' must be declared on type '{}'",
                        bare,
                        owning_type.name()
                    )
                })
                .factor
                .clone();
            (
                owning_type.name.clone().unwrap_or_default(),
                decomposition.clone(),
                factor,
            )
        };
        let owning_measure_name = owning_measure_name.as_str();
        let owning_decomp = &owning_decomp;
        let unit_factor = &unit_factor;

        if owning_measure_name == declaring_type_name {
            return Err(Error::validation(
                format!(
                    "In spec '{}': unit '{}' in measure type '{}' references unit '{}' which \
                     belongs to the same measure type. A measure cannot reference its own units \
                     in a compound expression.",
                    spec_name, unit.name, declaring_type_name, measure_ref
                ),
                source.cloned(),
                None::<String>,
            ));
        }

        for (dim, &dim_exp) in owning_decomp {
            accumulate(&mut result, dim, dim_exp * exponent);
        }

        let component_contribution = checked_pow_i32(unit_factor, *exponent).map_err(|error| {
            overflow_to_validation_error(
                spec_name,
                &unit.name,
                declaring_type_name,
                measure_ref,
                error,
                source,
            )
        })?;
        derived_factor =
            checked_mul(&derived_factor, &component_contribution).map_err(|error| {
                overflow_to_validation_error(
                    spec_name,
                    &unit.name,
                    declaring_type_name,
                    measure_ref,
                    error,
                    source,
                )
            })?;
    }

    Ok((result, derived_factor))
}

fn overflow_to_validation_error(
    spec_name: &str,
    unit_name: &str,
    declaring_type_name: &str,
    measure_ref: &str,
    failure: crate::computation::rational::NumericFailure,
    source: Option<&Source>,
) -> Error {
    Error::validation(
        format!(
            "In spec '{}': unit '{}' in measure type '{}' overflowed while combining '{}': {}",
            spec_name, unit_name, declaring_type_name, measure_ref, failure
        ),
        source.cloned(),
        None::<String>,
    )
}

fn accumulate(result: &mut BaseMeasureVector, dim: &str, value: i32) {
    let entry = result.entry(dim.to_string()).or_insert(0);
    *entry += value;
    if *entry == 0 {
        result.remove(dim);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::parsing::ast::{BooleanValue, Reference, Span, Value};

    fn test_source() -> Source {
        Source::new(
            crate::parsing::source::SourceType::Volatile,
            Span {
                start: 0,
                end: 0,
                line: 1,
                col: 0,
            },
        )
    }

    fn build_graph(
        main_spec: &LemmaSpec,
        all_specs: &[LemmaSpec],
    ) -> Result<Graph<'static>, Vec<Error>> {
        use crate::engine::Context;
        use crate::planning::discovery;

        let mut ctx = Context::new();
        let repository = ctx.workspace();
        for s in all_specs {
            ctx.insert_spec(Arc::clone(&repository), s.clone())?;
        }
        let effective = EffectiveDate::from_option(main_spec.effective_from().cloned());
        let ctx = Box::leak(Box::new(ctx));
        let repository = ctx.workspace();
        let main_spec_ref = ctx
            .spec_set(&repository, main_spec.name.as_str())
            .and_then(|ss| ss.get_exact(main_spec.effective_from()))
            .expect("main_spec must be in all_specs");
        let limits = Box::leak(Box::new(crate::ResourceLimits::default()));
        let ordered_dependencies =
            discovery::discover_dependency_order(ctx, main_spec_ref, &effective, limits).map_err(
                |e| match e {
                    discovery::DependencyDiscoveryError::Cycle(es)
                    | discovery::DependencyDiscoveryError::Other(es) => es,
                },
            )?;
        let ordered_dependencies = Box::leak(Box::new(ordered_dependencies));
        match Graph::build(
            ctx,
            &repository,
            main_spec_ref,
            ordered_dependencies,
            &effective,
            limits,
        ) {
            Ok((graph, _types)) => Ok(graph),
            Err(errors) => Err(errors),
        }
    }

    fn create_test_spec(name: &str) -> LemmaSpec {
        LemmaSpec::new(name.to_string())
    }

    fn create_literal_data(name: &str, value: Value) -> LemmaData {
        LemmaData {
            reference: Reference {
                segments: Vec::new(),
                name: name.to_string(),
            },
            value: ParsedDataValue::Definition {
                base: None,
                constraints: None,
                value: Some(value),
            },
            source_location: test_source(),
        }
    }

    fn create_literal_expr(value: Value) -> ast::Expression {
        ast::Expression {
            kind: ast::ExpressionKind::Literal(value),
            source_location: Some(test_source()),
        }
    }

    #[test]
    fn dotted_data_definition_rows_are_not_bindings() {
        // Bindings live on `uses` blocks only; dotted `LemmaData` rows are not binding paths.
        let mut spec = create_test_spec("test");
        spec = spec.add_data(create_literal_data("x", Value::Number(1.into())));

        spec = spec.add_data(LemmaData {
            reference: Reference::from_path(vec!["x".to_string(), "y".to_string()]),
            value: ParsedDataValue::Definition {
                base: None,
                constraints: None,
                value: Some(Value::Number(2.into())),
            },
            source_location: test_source(),
        });

        let result = build_graph(&spec, &[spec.clone()]);
        assert!(
            result.is_ok(),
            "dotted definition rows are ignored by binding collection"
        );
    }

    #[test]
    fn programmatic_ast_without_source_location_is_validation_error() {
        let mut spec = create_test_spec("test");
        spec = spec.add_rule(LemmaRule {
            name: "no_location".to_string(),
            expression: ast::Expression {
                kind: ast::ExpressionKind::Literal(Value::Number(1.into())),
                source_location: None,
            },
            unless_clauses: Vec::new(),
            source_location: test_source(),
        });

        let result = build_graph(&spec, &[spec.clone()]);
        let errors = result.expect_err("missing source_location must be a planning error");
        assert!(
            errors
                .iter()
                .any(|e| e.to_string().contains("source location")),
            "expected source-location boundary error, got: {errors:?}"
        );
    }

    #[test]
    fn programmatic_ast_nested_expression_without_source_location_is_validation_error() {
        let inner = ast::Expression {
            kind: ast::ExpressionKind::Literal(Value::Number(1.into())),
            source_location: None,
        };
        let outer = ast::Expression {
            kind: ast::ExpressionKind::Arithmetic(
                Arc::new(create_literal_expr(Value::Number(2.into()))),
                ast::ArithmeticComputation::Add,
                Arc::new(inner),
            ),
            source_location: Some(test_source()),
        };
        let mut spec = create_test_spec("test");
        spec = spec.add_rule(LemmaRule {
            name: "nested_no_location".to_string(),
            expression: outer,
            unless_clauses: Vec::new(),
            source_location: test_source(),
        });

        let result = build_graph(&spec, &[spec.clone()]);
        let errors = result.expect_err("nested missing source_location must be a planning error");
        assert!(
            errors
                .iter()
                .any(|e| e.to_string().contains("source location")),
            "expected source-location boundary error, got: {errors:?}"
        );
    }

    #[test]
    fn should_reject_data_and_rule_name_collision() {
        // Higher-standard language rule: data and rule names should not collide.
        // It's ambiguous for humans and leads to confusing error messages.
        //
        // This is currently expected to FAIL until the language enforces it.
        let mut spec = create_test_spec("test");
        spec = spec.add_data(create_literal_data("x", Value::Number(1.into())));
        spec = spec.add_rule(LemmaRule {
            name: "x".to_string(),
            expression: create_literal_expr(Value::Number(2.into())),
            unless_clauses: Vec::new(),
            source_location: test_source(),
        });

        let result = build_graph(&spec, &[spec.clone()]);
        assert!(
            result.is_err(),
            "Data and rule name collisions should be rejected"
        );
    }

    #[test]
    fn test_duplicate_data() {
        let mut spec = create_test_spec("test");
        spec = spec.add_data(create_literal_data(
            "age",
            Value::Number(rust_decimal::Decimal::from(25)),
        ));
        spec = spec.add_data(create_literal_data(
            "age",
            Value::Number(rust_decimal::Decimal::from(30)),
        ));

        let result = build_graph(&spec, &[spec.clone()]);
        assert!(result.is_err(), "Should detect duplicate data");

        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| {
            let s = e.to_string();
            s.contains("already used") && s.contains("age")
        }));
    }

    #[test]
    fn test_duplicate_rule() {
        let mut spec = create_test_spec("test");

        let rule1 = LemmaRule {
            name: "test_rule".to_string(),
            expression: create_literal_expr(Value::Boolean(BooleanValue::True)),
            unless_clauses: Vec::new(),
            source_location: test_source(),
        };
        let rule2 = LemmaRule {
            name: "test_rule".to_string(),
            expression: create_literal_expr(Value::Boolean(BooleanValue::False)),
            unless_clauses: Vec::new(),
            source_location: test_source(),
        };

        spec = spec.add_rule(rule1);
        spec = spec.add_rule(rule2);

        let result = build_graph(&spec, &[spec.clone()]);
        assert!(result.is_err(), "Should detect duplicate rule");

        let errors = result.unwrap_err();
        assert!(errors.iter().any(
            |e| e.to_string().contains("Duplicate rule") && e.to_string().contains("test_rule")
        ));
    }

    #[test]
    fn test_missing_data_reference() {
        let mut spec = create_test_spec("test");

        let missing_data_expr = ast::Expression {
            kind: ast::ExpressionKind::Reference(Reference {
                segments: Vec::new(),
                name: "nonexistent".to_string(),
            }),
            source_location: Some(test_source()),
        };

        let rule = LemmaRule {
            name: "test_rule".to_string(),
            expression: missing_data_expr,
            unless_clauses: Vec::new(),
            source_location: test_source(),
        };
        spec = spec.add_rule(rule);

        let result = build_graph(&spec, &[spec.clone()]);
        assert!(result.is_err(), "Should detect missing data");

        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| e.to_string().contains("Reference 'nonexistent' not found")));
    }

    #[test]
    fn chained_add_reports_all_missing_references() {
        let minimal = r#"spec s
data a: 1
rule bad: a + missing_one + missing_two
"#;
        let specs = crate::parse(
            minimal,
            crate::parsing::source::SourceType::Volatile,
            &crate::ResourceLimits::default(),
        )
        .expect("minimal fixture parses")
        .into_flattened_specs();
        let s = specs.iter().find(|d| d.name == "s").expect("spec s");
        let result = build_graph(s, &specs);
        let errors = result.expect_err("chained + with two missing refs must fail planning");
        let not_found: Vec<_> = errors
            .iter()
            .filter(|e| e.to_string().contains("not found"))
            .collect();
        assert_eq!(
            not_found.len(),
            2,
            "expected two missing-reference errors, got: {}",
            errors
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("; ")
        );
        assert!(
            not_found
                .iter()
                .any(|e| e.to_string().contains("missing_one")),
            "expected error for missing_one"
        );
        assert!(
            not_found
                .iter()
                .any(|e| e.to_string().contains("missing_two")),
            "expected error for missing_two"
        );
        for error in &not_found {
            let source = error
                .location()
                .expect("missing-reference error must carry source span");
            assert_ne!(
                source.span.start, source.span.end,
                "error span must not be empty"
            );
        }

        let user_shaped = r#"spec s
data base_cost: 1
data starch_levy: 2
data quality_extra: 3
rule landed_cost_per_kg:
  base_cost + starch_levy + quality_extra + transport_extra_per_kg + amortization_per_kg
"#;
        let specs = crate::parse(
            user_shaped,
            crate::parsing::source::SourceType::Volatile,
            &crate::ResourceLimits::default(),
        )
        .expect("user-shaped fixture parses")
        .into_flattened_specs();
        let s = specs.iter().find(|d| d.name == "s").expect("spec s");
        let result = build_graph(s, &specs);
        let errors = result.expect_err("landed_cost_per_kg with two typos must fail planning");
        let not_found: Vec<_> = errors
            .iter()
            .filter(|e| e.to_string().contains("not found"))
            .collect();
        assert_eq!(
            not_found.len(),
            2,
            "expected two missing-reference errors, got: {}",
            errors
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("; ")
        );
        assert!(
            not_found
                .iter()
                .any(|e| e.to_string().contains("transport_extra_per_kg")),
            "expected error for transport_extra_per_kg"
        );
        assert!(
            not_found
                .iter()
                .any(|e| e.to_string().contains("amortization_per_kg")),
            "expected error for amortization_per_kg"
        );
        for error in &not_found {
            let source = error
                .location()
                .expect("missing-reference error must carry source span");
            assert_ne!(
                source.span.start, source.span.end,
                "error span must not be empty"
            );
        }
    }

    #[test]
    fn test_missing_spec_reference() {
        let mut spec = create_test_spec("test");

        let data = LemmaData {
            reference: Reference {
                segments: Vec::new(),
                name: "contract".to_string(),
            },
            value: ParsedDataValue::import(crate::parsing::ast::SpecRef::same_repository(
                "nonexistent",
            )),
            source_location: test_source(),
        };
        spec = spec.add_data(data);

        let result = build_graph(&spec, &[spec.clone()]);
        assert!(result.is_err(), "Should detect missing spec");

        let errors = result.unwrap_err();
        assert!(
            errors.iter().any(|e| e.to_string().contains("nonexistent")),
            "Error should mention nonexistent spec: {:?}",
            errors.iter().map(|e| e.to_string()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_data_reference_conversion() {
        let mut spec = create_test_spec("test");
        spec = spec.add_data(create_literal_data(
            "age",
            Value::Number(rust_decimal::Decimal::from(25)),
        ));

        let age_expr = ast::Expression {
            kind: ast::ExpressionKind::Reference(Reference {
                segments: Vec::new(),
                name: "age".to_string(),
            }),
            source_location: Some(test_source()),
        };

        let rule = LemmaRule {
            name: "test_rule".to_string(),
            expression: age_expr,
            unless_clauses: Vec::new(),
            source_location: test_source(),
        };
        spec = spec.add_rule(rule);

        let result = build_graph(&spec, &[spec.clone()]);
        assert!(result.is_ok(), "Should build graph successfully");

        let graph = result.unwrap();
        let rule_node = graph.rules().values().next().unwrap();

        assert!(matches!(
            rule_node.branches[0].1.kind,
            ExpressionKind::DataPath(_)
        ));
    }

    #[test]
    fn test_rule_reference_conversion() {
        let mut spec = create_test_spec("test");

        let rule1_expr = ast::Expression {
            kind: ast::ExpressionKind::Reference(Reference {
                segments: Vec::new(),
                name: "age".to_string(),
            }),
            source_location: Some(test_source()),
        };

        let rule1 = LemmaRule {
            name: "rule1".to_string(),
            expression: rule1_expr,
            unless_clauses: Vec::new(),
            source_location: test_source(),
        };
        spec = spec.add_rule(rule1);

        let rule2_expr = ast::Expression {
            kind: ast::ExpressionKind::Reference(Reference {
                segments: Vec::new(),
                name: "rule1".to_string(),
            }),
            source_location: Some(test_source()),
        };

        let rule2 = LemmaRule {
            name: "rule2".to_string(),
            expression: rule2_expr,
            unless_clauses: Vec::new(),
            source_location: test_source(),
        };
        spec = spec.add_rule(rule2);

        spec = spec.add_data(create_literal_data(
            "age",
            Value::Number(rust_decimal::Decimal::from(25)),
        ));

        let result = build_graph(&spec, &[spec.clone()]);
        assert!(result.is_ok(), "Should build graph successfully");

        let graph = result.unwrap();
        let rule2_node = graph
            .rules()
            .get(&RulePath {
                segments: Vec::new(),
                rule: "rule2".to_string(),
            })
            .unwrap();

        assert_eq!(rule2_node.depends_on_rules.len(), 1);
        // Source branches preserve RulePath; normalized_expression must inline deps and must not.
        assert!(matches!(
            rule2_node.branches[0].1.kind,
            ExpressionKind::RulePath(_)
        ));
    }

    #[test]
    fn test_collect_multiple_errors() {
        let mut spec = create_test_spec("test");
        spec = spec.add_data(create_literal_data(
            "age",
            Value::Number(rust_decimal::Decimal::from(25)),
        ));
        spec = spec.add_data(create_literal_data(
            "age",
            Value::Number(rust_decimal::Decimal::from(30)),
        ));

        let missing_data_expr = ast::Expression {
            kind: ast::ExpressionKind::Reference(Reference {
                segments: Vec::new(),
                name: "nonexistent".to_string(),
            }),
            source_location: Some(test_source()),
        };

        let rule = LemmaRule {
            name: "test_rule".to_string(),
            expression: missing_data_expr,
            unless_clauses: Vec::new(),
            source_location: test_source(),
        };
        spec = spec.add_rule(rule);

        let result = build_graph(&spec, &[spec.clone()]);
        assert!(result.is_err(), "Should collect multiple errors");

        let errors = result.unwrap_err();
        assert!(errors.len() >= 2, "Should have at least 2 errors");
        assert!(errors
            .iter()
            .any(|e| e.to_string().contains("already used")));
        assert!(errors
            .iter()
            .any(|e| e.to_string().contains("Reference 'nonexistent' not found")));
    }

    #[test]
    fn test_type_registration_collects_multiple_errors() {
        use crate::parsing::ast::{DataValue, ParentType, PrimitiveKind, SpecRef};

        let type_source = Source::new(
            crate::parsing::source::SourceType::Volatile,
            Span {
                start: 0,
                end: 0,
                line: 1,
                col: 0,
            },
        );
        let spec_a = create_test_spec("spec_a")
            .with_source_type(crate::parsing::source::SourceType::Volatile)
            .add_data(LemmaData {
                reference: Reference::local("dep".to_string()),
                value: DataValue::import(SpecRef::same_repository("spec_b")),
                source_location: type_source.clone(),
            })
            .add_data(LemmaData {
                reference: Reference::local("money".to_string()),
                value: DataValue::Definition {
                    base: Some(ParentType::Primitive {
                        primitive: PrimitiveKind::Number,
                    }),
                    constraints: None,
                    value: None,
                },
                source_location: type_source.clone(),
            })
            .add_data(LemmaData {
                reference: Reference::local("money".to_string()),
                value: DataValue::Definition {
                    base: Some(ParentType::Primitive {
                        primitive: PrimitiveKind::Number,
                    }),
                    constraints: None,
                    value: None,
                },
                source_location: type_source,
            });

        let type_source_b = Source::new(
            crate::parsing::source::SourceType::Volatile,
            Span {
                start: 0,
                end: 0,
                line: 1,
                col: 0,
            },
        );
        let spec_b = create_test_spec("spec_b")
            .with_source_type(crate::parsing::source::SourceType::Volatile)
            .add_data(LemmaData {
                reference: Reference::local("length".to_string()),
                value: DataValue::Definition {
                    base: Some(ParentType::Primitive {
                        primitive: PrimitiveKind::Number,
                    }),
                    constraints: None,
                    value: None,
                },
                source_location: type_source_b.clone(),
            })
            .add_data(LemmaData {
                reference: Reference::local("length".to_string()),
                value: DataValue::Definition {
                    base: Some(ParentType::Primitive {
                        primitive: PrimitiveKind::Number,
                    }),
                    constraints: None,
                    value: None,
                },
                source_location: type_source_b,
            });

        let mut sources = HashMap::new();
        sources.insert(
            crate::parsing::source::SourceType::Volatile.to_string(),
            "spec spec_a\nuses dep: spec_b\ndata money: number\ndata money: number".to_string(),
        );
        sources.insert(
            crate::parsing::source::SourceType::Volatile.to_string(),
            "spec spec_b\ndata length: number\ndata length: number".to_string(),
        );

        let result = build_graph(&spec_a, &[spec_a.clone(), spec_b.clone()]);
        assert!(
            result.is_err(),
            "Should fail with duplicate type/data errors"
        );
    }

    // =================================================================
    // Versioned spec identifiers: latest-resolution (section 6.3)
    // =================================================================

    #[test]
    fn spec_ref_resolves_to_single_spec_by_name() {
        let code = r#"spec myspec
data x: 10

spec consumer
uses m: myspec
rule result: m.x"#;
        let specs = crate::parse(
            code,
            crate::parsing::source::SourceType::Volatile,
            &crate::ResourceLimits::default(),
        )
        .unwrap()
        .into_flattened_specs();
        let consumer = specs.iter().find(|d| d.name == "consumer").unwrap();

        let graph = build_graph(consumer, &specs).unwrap();
        let data_path = DataPath {
            segments: vec![PathSegment {
                data: "m".to_string(),
                spec: "myspec".to_string(),
            }],
            data: "x".to_string(),
        };
        assert!(
            graph.data.contains_key(&data_path),
            "Ref should resolve to myspec. Data: {:?}",
            graph.data.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn spec_ref_to_nonexistent_spec_is_error() {
        let code = r#"spec myspec
data x: 10

spec consumer
uses m: nonexistent
rule result: m.x"#;
        let specs = crate::parse(
            code,
            crate::parsing::source::SourceType::Volatile,
            &crate::ResourceLimits::default(),
        )
        .unwrap()
        .into_flattened_specs();
        let consumer = specs.iter().find(|d| d.name == "consumer").unwrap();
        let result = build_graph(consumer, &specs);
        assert!(result.is_err(), "Should fail for non-existent spec");
    }

    // =================================================================
    // Self-reference: same spec body via uses (planning)
    // =================================================================

    #[test]
    fn import_alias_registered_in_graph() {
        let code = r#"
spec inner
data x: number -> suggest 1

spec outer
uses i: inner
rule r: i.x
"#;
        let specs = crate::parse(
            code,
            crate::parsing::source::SourceType::Volatile,
            &crate::ResourceLimits::default(),
        )
        .unwrap()
        .into_flattened_specs();
        let outer = specs.iter().find(|s| s.name == "outer").unwrap();
        let graph = build_graph(outer, &specs).expect("uses i: inner must plan");

        let alias_path = DataPath {
            segments: Vec::new(),
            data: "i".to_string(),
        };
        match graph.data().get(&alias_path) {
            Some(DataDefinition::Import { target_name, .. }) => {
                assert_eq!(target_name, "inner");
            }
            other => panic!(
                "alias path 'i' must be DataDefinition::Import, got {:?}",
                other
            ),
        }

        let nested_path = DataPath {
            segments: vec![PathSegment {
                data: "i".to_string(),
                spec: "inner".to_string(),
            }],
            data: "x".to_string(),
        };
        assert!(
            graph.data().contains_key(&nested_path),
            "nested data i.x must exist after nested build_spec"
        );
    }

    #[test]
    fn self_reference_is_error() {
        let code = "spec myspec\nuses m: myspec";
        let specs = crate::parse(
            code,
            crate::parsing::source::SourceType::Volatile,
            &crate::ResourceLimits::default(),
        )
        .unwrap()
        .into_flattened_specs();
        let result = build_graph(&specs[0], &specs);
        assert!(result.is_err(), "Self-reference should be an error");
        let errors = result.unwrap_err();
        let joined: String = errors
            .iter()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            joined.contains("cannot reference itself") && joined.contains("myspec"),
            "Error should name self-reference: {:?}",
            errors.iter().map(|e| e.to_string()).collect::<Vec<_>>()
        );
    }

    mod type_resolution {
        use super::super::*;
        use crate::computation::rational::rational_new;
        use crate::parsing::ast::{
            CommandArg, LemmaSpec, ParentType, PrimitiveKind, TypeConstraintCommand,
        };
        use crate::parsing::parse;
        use crate::ResourceLimits;
        use rust_decimal::Decimal;
        use std::sync::Arc;

        fn test_context_and_effective(
            specs: &[&LemmaSpec],
        ) -> (&'static Context, &'static EffectiveDate) {
            use crate::engine::Context;
            let mut ctx = Context::new();
            let repository = ctx.workspace();
            for s in specs {
                ctx.insert_spec(Arc::clone(&repository), (*s).clone())
                    .unwrap();
            }
            let ctx = Box::leak(Box::new(ctx));
            let eff = Box::leak(Box::new(EffectiveDate::Origin));
            (ctx, eff)
        }

        fn ordered_dependencies_and_spec() -> (
            &'static [discovery::DependencySpec<'static>],
            &'static LemmaSpec,
        ) {
            use crate::engine::Context;
            let mut ctx = Context::new();
            let repository = ctx.workspace();
            let spec = LemmaSpec::new("test_spec".to_string());
            ctx.insert_spec(Arc::clone(&repository), spec).unwrap();
            let ctx = Box::leak(Box::new(ctx));
            let repository = ctx.workspace();
            let spec = ctx
                .spec_set(&repository, "test_spec")
                .and_then(|ss| ss.get_exact(None))
                .expect("inserted");
            let ordered_dependencies = Box::leak(Box::new(vec![discovery::DependencySpec {
                repository: Arc::clone(&repository),
                spec,
            }]));
            (ordered_dependencies.as_slice(), spec)
        }

        fn resolver_for_code(code: &str) -> (TypeResolver<'static>, Vec<&'static LemmaSpec>) {
            use crate::engine::Context;
            let owned = parse(
                code,
                crate::parsing::source::SourceType::Volatile,
                &ResourceLimits::default(),
            )
            .unwrap()
            .into_flattened_specs();
            let mut ctx = Context::new();
            let repository = ctx.workspace();
            for s in owned {
                ctx.insert_spec(Arc::clone(&repository), s).unwrap();
            }
            let ctx = Box::leak(Box::new(ctx));
            let repository = ctx.workspace();
            let mut spec_refs: Vec<&'static LemmaSpec> = Vec::new();
            for ss in ctx.spec_sets_for(&repository) {
                for s in ss.iter_specs() {
                    spec_refs.push(s);
                }
            }
            let mut resolver = TypeResolver::new(ctx);
            for spec in &spec_refs {
                resolver.register_all(&repository, spec);
            }
            (resolver, spec_refs)
        }

        fn resolver_single_spec(code: &str) -> (TypeResolver<'static>, &LemmaSpec) {
            let (resolver, spec_arcs) = resolver_for_code(code);
            let spec = spec_arcs.into_iter().next().expect("at least one spec");
            (resolver, spec)
        }

        #[test]
        fn test_type_spec_for_primitive_covers_all_variants() {
            use crate::parsing::ast::PrimitiveKind;
            use crate::planning::semantics::type_spec_for_primitive;

            for kind in [
                PrimitiveKind::Boolean,
                PrimitiveKind::Measure,
                PrimitiveKind::MeasureRange,
                PrimitiveKind::Number,
                PrimitiveKind::NumberRange,
                PrimitiveKind::Ratio,
                PrimitiveKind::RatioRange,
                PrimitiveKind::Text,
                PrimitiveKind::Date,
                PrimitiveKind::DateRange,
                PrimitiveKind::Time,
                PrimitiveKind::TimeRange,
            ] {
                let spec = type_spec_for_primitive(kind);
                assert!(
                    !matches!(
                        spec,
                        crate::planning::semantics::TypeSpecification::Undetermined
                    ),
                    "type_spec_for_primitive({:?}) returned Undetermined",
                    kind
                );
            }
        }

        #[test]
        fn test_register_data_type_def() {
            let (_ordered_dependencies, spec) = ordered_dependencies_and_spec();
            let (ctx, _) = test_context_and_effective(&[spec]);
            let mut resolver = TypeResolver::new(ctx);
            let ftd = DataTypeDef {
                parent: ParentType::Primitive {
                    primitive: PrimitiveKind::Number,
                },
                constraints: Some(vec![
                    Constraint::new(
                        TypeConstraintCommand::Minimum,
                        vec![CommandArg::Literal(crate::literals::Value::Number(
                            Decimal::ZERO,
                        ))],
                        crate::parsing::source::Source::new(
                            crate::parsing::source::SourceType::Volatile,
                            crate::parsing::ast::Span {
                                start: 0,
                                end: 0,
                                line: 1,
                                col: 0,
                            },
                        ),
                    ),
                    Constraint::new(
                        TypeConstraintCommand::Maximum,
                        vec![CommandArg::Literal(crate::literals::Value::Number(
                            Decimal::from(150),
                        ))],
                        crate::parsing::source::Source::new(
                            crate::parsing::source::SourceType::Volatile,
                            crate::parsing::ast::Span {
                                start: 0,
                                end: 0,
                                line: 1,
                                col: 0,
                            },
                        ),
                    ),
                ]),
                source: crate::parsing::source::Source::new(
                    crate::parsing::source::SourceType::Volatile,
                    crate::parsing::ast::Span {
                        start: 0,
                        end: 0,
                        line: 1,
                        col: 0,
                    },
                ),
                name: "age".to_string(),
                bound_literal: None,
            };

            let result = resolver.register_type(spec, ftd);
            assert!(result.is_ok());
            let resolved = resolver
                .resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new())
                .unwrap();
            assert!(resolved.resolved.contains_key("age"));
        }

        #[test]
        fn test_register_duplicate_type_fails() {
            let (_ordered_dependencies, spec) = ordered_dependencies_and_spec();
            let (ctx, _) = test_context_and_effective(&[spec]);
            let mut resolver = TypeResolver::new(ctx);
            let ftd = DataTypeDef {
                parent: ParentType::Primitive {
                    primitive: PrimitiveKind::Number,
                },
                constraints: None,
                source: crate::parsing::source::Source::new(
                    crate::parsing::source::SourceType::Volatile,
                    crate::parsing::ast::Span {
                        start: 0,
                        end: 0,
                        line: 1,
                        col: 0,
                    },
                ),
                name: "money".to_string(),
                bound_literal: None,
            };
            resolver.register_type(spec, ftd.clone()).unwrap();
            let result = resolver.register_type(spec, ftd);
            assert!(result.is_err());
        }

        #[test]
        fn test_resolve_custom_type_from_primitive() {
            let (_ordered_dependencies, spec) = ordered_dependencies_and_spec();
            let (ctx, _) = test_context_and_effective(&[spec]);
            let mut resolver = TypeResolver::new(ctx);
            let ftd = DataTypeDef {
                parent: ParentType::Primitive {
                    primitive: PrimitiveKind::Number,
                },
                constraints: None,
                source: crate::parsing::source::Source::new(
                    crate::parsing::source::SourceType::Volatile,
                    crate::parsing::ast::Span {
                        start: 0,
                        end: 0,
                        line: 1,
                        col: 0,
                    },
                ),
                name: "money".to_string(),
                bound_literal: None,
            };

            resolver.register_type(spec, ftd).unwrap();
            let resolved = resolver
                .resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new())
                .unwrap();

            assert!(resolved.resolved.contains_key("money"));
            let money_type = resolved.resolved.get("money").unwrap();
            assert_eq!(money_type.name, Some("money".to_string()));
        }

        #[test]
        fn test_child_measure_type_keeps_declared_name_and_child_units() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data length: measure
      -> unit meter: 1
    data road_length: length
      -> unit kilometer: 1000"#,
            );

            let resolved_types = resolver
                .resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new())
                .unwrap();

            let road_length_type = resolved_types.resolved.get("road_length").unwrap();
            assert_eq!(road_length_type.name.as_deref(), Some("road_length"));

            match &road_length_type.specifications {
                TypeSpecification::Measure { units, .. } => {
                    assert!(units.iter().any(|unit| unit.name == "kilometer"));
                }
                _ => panic!("Expected Measure type specifications"),
            }

            let kilometer_owner = resolved_types.unit_index.unique_owner("kilometer").unwrap();
            assert_eq!(kilometer_owner.name.as_deref(), Some("road_length"));
        }

        #[test]
        fn test_type_definition_resolution() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data dice: number -> minimum 0 -> maximum 6"#,
            );

            let resolved_types = resolver
                .resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new())
                .unwrap();
            let dice_type = resolved_types.resolved.get("dice").unwrap();

            match &dice_type.specifications {
                TypeSpecification::Number {
                    minimum, maximum, ..
                } => {
                    assert_eq!(minimum, &Some(rational_new(0, 1)));
                    assert_eq!(maximum, &Some(rational_new(6, 1)));
                }
                _ => panic!("Expected Number type specifications"),
            }
        }

        #[test]
        fn test_type_definition_with_multiple_commands() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data money: measure -> decimals 2 -> unit eur: 1.0 -> unit usd: 1.18"#,
            );

            let resolved_types = resolver
                .resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new())
                .unwrap();
            let money_type = resolved_types.resolved.get("money").unwrap();

            match &money_type.specifications {
                TypeSpecification::Measure {
                    decimals, units, ..
                } => {
                    assert_eq!(*decimals, Some(2));
                    assert_eq!(units.len(), 2);
                    assert!(units.iter().any(|u| u.name == "eur"));
                    assert!(units.iter().any(|u| u.name == "usd"));
                }
                _ => panic!("Expected Measure type specifications"),
            }
        }

        #[test]
        fn test_number_type_with_decimals() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data price: number -> decimals 2 -> minimum 0"#,
            );

            let resolved_types = resolver
                .resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new())
                .unwrap();
            let price_type = resolved_types.resolved.get("price").unwrap();

            match &price_type.specifications {
                TypeSpecification::Number {
                    decimals, minimum, ..
                } => {
                    assert_eq!(*decimals, Some(2));
                    assert_eq!(minimum, &Some(rational_new(0, 1)));
                }
                _ => panic!("Expected Number type specifications with decimals"),
            }
        }

        #[test]
        fn test_number_type_decimals_only() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data precise_number: number -> decimals 4"#,
            );

            let resolved_types = resolver
                .resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new())
                .unwrap();
            let precise_type = resolved_types.resolved.get("precise_number").unwrap();

            match &precise_type.specifications {
                TypeSpecification::Number { decimals, .. } => {
                    assert_eq!(*decimals, Some(4));
                }
                _ => panic!("Expected Number type with decimals 4"),
            }
        }

        #[test]
        fn test_measure_type_decimals_only() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data weight: measure -> unit kg: 1 -> decimals 3"#,
            );

            let resolved_types = resolver
                .resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new())
                .unwrap();
            let weight_type = resolved_types.resolved.get("weight").unwrap();

            match &weight_type.specifications {
                TypeSpecification::Measure { decimals, .. } => {
                    assert_eq!(*decimals, Some(3));
                }
                _ => panic!("Expected Measure type with decimals 3"),
            }
        }

        #[test]
        fn test_ratio_type_accepts_optional_decimals_command() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data ratio_type: ratio -> decimals 2"#,
            );

            let resolved_types = resolver
                .resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new())
                .unwrap();
            let ratio_type = resolved_types.resolved.get("ratio_type").unwrap();

            match &ratio_type.specifications {
                TypeSpecification::Ratio { decimals, .. } => {
                    assert_eq!(
                        *decimals,
                        Some(2),
                        "ratio type should accept decimals command"
                    );
                }
                _ => panic!("Expected Ratio type with decimals 2"),
            }
        }

        #[test]
        fn typedef_default_inherits_through_extension_chain() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data money: measure -> unit eur: 1 -> suggest 4 eur
    data price: money
    data final_price: price"#,
            );

            let resolved = resolver
                .resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new())
                .expect("extension chain must resolve");

            assert!(
                resolved.declared_suggestions.contains_key("money"),
                "base typedef default must convert to ValueKind"
            );
            assert!(
                resolved.declared_suggestions.contains_key("final_price"),
                "suggestion must inherit through extension chain (money → price → final_price)"
            );
        }

        #[test]
        fn test_ratio_type_with_default_command() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data percentage: ratio -> minimum 0% -> maximum 100% -> suggest 50%"#,
            );

            let resolved_types = resolver
                .resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new())
                .unwrap();
            let percentage_type = resolved_types.resolved.get("percentage").unwrap();

            match &percentage_type.specifications {
                TypeSpecification::Ratio {
                    minimum, maximum, ..
                } => {
                    assert_eq!(
                        *minimum,
                        Some(rational_new(0, 1)),
                        "ratio type should have minimum 0"
                    );
                    assert_eq!(
                        *maximum,
                        Some(rational_new(1, 1)),
                        "ratio type should have maximum 1"
                    );
                }
                _ => panic!("Expected Ratio type with minimum and maximum"),
            }

            let declared = resolved_types
                .declared_suggestions
                .get("percentage")
                .expect("declared default must be tracked for percentage");
            match declared {
                ValueKind::Ratio(v) => {
                    assert_eq!(v, &rational_new(1, 2));
                }
                other => panic!("expected Ratio declared default, got {:?}", other),
            }
        }

        #[test]
        fn test_measure_extension_chain_same_family_units_allowed() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data money: measure -> unit eur: 1
    data money2: money -> unit usd: 1.24"#,
            );

            let result = resolver.resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new());
            assert!(
                result.is_ok(),
                "Measure extension chain should resolve: {:?}",
                result.err()
            );

            let resolved = result.unwrap();
            assert!(
                resolved.unit_index.has_unique_owner("eur"),
                "eur should be in unit_index"
            );
            assert!(
                resolved.unit_index.has_unique_owner("usd"),
                "usd should be in unit_index"
            );
            let eur_type = resolved.unit_index.unique_owner("eur").unwrap();
            let usd_type = resolved.unit_index.unique_owner("usd").unwrap();
            assert_eq!(
                eur_type.name.as_deref(),
                Some("money"),
                "declarer money must own inherited eur, not extension"
            );
            assert_eq!(
                usd_type.name.as_deref(),
                Some("money2"),
                "usd defined on money2 should be owned by money2"
            );
        }

        #[test]
        fn test_invalid_parent_type_in_named_type_should_error() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data invalid: nonexistent_type -> minimum 0"#,
            );

            let result = resolver.resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new());
            assert!(result.is_err(), "Should reject invalid parent type");

            let errs = result.unwrap_err();
            assert!(!errs.is_empty(), "expected at least one error");
            let error_msg = errs[0].to_string();
            assert!(
                error_msg.contains("Unknown parent") && error_msg.contains("nonexistent_type"),
                "Error should mention unknown type. Got: {}",
                error_msg
            );
        }

        #[test]
        fn test_invalid_primitive_type_name_should_error() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data invalid: choice -> option "a""#,
            );

            let result = resolver.resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new());
            assert!(result.is_err(), "Should reject invalid type base 'choice'");

            let errs = result.unwrap_err();
            assert!(!errs.is_empty(), "expected at least one error");
            let error_msg = errs[0].to_string();
            assert!(
                error_msg.contains("Unknown parent") && error_msg.contains("choice"),
                "Error should mention unknown type 'choice'. Got: {}",
                error_msg
            );
        }

        #[test]
        fn extension_unit_factor_override_errors() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data money: measure
      -> unit eur: 1
    data money2: money
      -> unit eur: 1.10"#,
            );

            let result = resolver.resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new());
            assert!(
                result.is_err(),
                "extension child must not override inherited unit factor"
            );
            let error_msg = result
                .unwrap_err()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ");
            assert!(
                error_msg.contains("eur"),
                "error must name unit eur, got: {error_msg}"
            );
            assert!(
                error_msg.contains("inherited") || error_msg.contains("cannot change"),
                "error must reject inherited unit redefinition, got: {error_msg}"
            );
        }

        #[test]
        fn inherited_unit_idempotent_redeclare_ok() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data money: measure
      -> unit eur: 1
    data money2: money
      -> unit eur: 1"#,
            );

            let resolved = resolver
                .resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new())
                .expect("idempotent inherited unit redeclare must resolve");
            let money2 = resolved.resolved.get("money2").expect("money2");
            match &money2.specifications {
                TypeSpecification::Measure { units, .. } => {
                    let eur = units.iter().find(|u| u.name == "eur").expect("eur");
                    assert_eq!(
                        eur.factor.try_to_decimal().unwrap(),
                        Decimal::ONE,
                        "eur factor must remain 1"
                    );
                }
                other => panic!("expected Measure, got {other:?}"),
            }
        }

        #[test]
        fn extension_additive_unit_registers_in_unit_index() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data money: measure
      -> unit eur: 1
    data money2: money
      -> unit usd: 1.24"#,
            );

            let resolved = resolver
                .resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new())
                .expect("additive extension unit must resolve");
            let money = resolved
                .resolved
                .get("money")
                .expect("money resolved")
                .clone();
            let money2 = resolved
                .resolved
                .get("money2")
                .expect("money2 resolved")
                .clone();
            assert!(
                resolved.unit_index.has_unique_owner("eur"),
                "eur must be in unit_index"
            );
            assert!(
                resolved.unit_index.has_unique_owner("usd"),
                "usd must be in unit_index"
            );
            let eur_owner = resolved.unit_index.unique_owner("eur").expect("eur owner");
            let usd_owner = resolved.unit_index.unique_owner("usd").expect("usd owner");
            assert_eq!(
                eur_owner.name.as_deref(),
                Some("money"),
                "declarer money must own eur, not extension money2"
            );
            assert_eq!(
                usd_owner.name.as_deref(),
                Some("money2"),
                "money2 must own new unit usd"
            );
            assert_eq!(eur_owner.as_ref(), money.as_ref());
            assert_eq!(usd_owner.as_ref(), money2.as_ref());
        }

        #[test]
        fn find_unique_reports_multiple_families_with_same_decomposition() {
            let code = r#"spec units
    data money: measure
      -> unit eur: 1
      -> unit usd: 0.86
    data mass: measure
      -> unit kg: 1
    data price_eur_per_kg: measure
      -> unit eur_per_kg: eur/kg
    data price_usd_per_kg: measure
      -> unit usd_per_kg: usd/kg
    "#;
            let (resolver, spec_arcs) = resolver_for_code(code);
            let units_arc = spec_arcs
                .iter()
                .find(|spec| spec.name == "units")
                .cloned()
                .expect("units spec");
            let (context, _) = test_context_and_effective(&spec_arcs);
            let repository = context.workspace();
            let resolved = resolver
                .resolve_and_validate(units_arc, &EffectiveDate::Origin, &ResolvedTypesMap::new())
                .expect("units resolves");
            let eur_decomp = resolved
                .unit_index
                .unique_owner("eur_per_kg")
                .expect("eur_per_kg")
                .measure_type_decomposition()
                .expect("eur_per_kg decomposition")
                .clone();
            let map = vec![(repository, units_arc, resolved)];

            let unique = find_unique_measure_type_in_unit_index(
                &find_types_by_spec(&map, units_arc)
                    .expect("units in map")
                    .unit_index,
                &eur_decomp,
            );
            match unique {
                DecompositionMatch::Multiple(families) => {
                    assert_eq!(families.len(), 2);
                    assert!(families.contains(&"price_eur_per_kg".to_string()));
                    assert!(families.contains(&"price_usd_per_kg".to_string()));
                }
                other => panic!("expected Multiple families, got {other:?}"),
            }
        }

        #[test]
        fn unit_index_maps_family_root_not_binding_alias() {
            let code = r#"spec units
    data money: measure
      -> unit eur: 1
    data mass: measure
      -> unit kg: 1
    data price_per_weight: measure
      -> unit eur_per_kg: eur/kg

    spec consumer
    uses u: units
    data starch_levy_per_kg: u.price_per_weight
      -> suggest 0.1 eur_per_kg
    data amortization_per_kg: u.price_per_weight
      -> suggest 0.2 eur_per_kg
    "#;
            let (resolver, spec_arcs) = resolver_for_code(code);
            let units_arc = spec_arcs
                .iter()
                .find(|spec| spec.name == "units")
                .cloned()
                .expect("units spec");
            let consumer_arc = spec_arcs
                .iter()
                .find(|spec| spec.name == "consumer")
                .cloned()
                .expect("consumer spec");
            let (context, _) = test_context_and_effective(&spec_arcs);
            let repository = context.workspace();

            let mut already_resolved = ResolvedTypesMap::new();
            let units_types = resolver
                .resolve_and_validate(units_arc, &EffectiveDate::Origin, &already_resolved)
                .expect("units spec resolves");
            let units_price_per_weight = units_types
                .resolved
                .get("price_per_weight")
                .expect("units defines price_per_weight")
                .clone();
            already_resolved.push((Arc::clone(&repository), units_arc, units_types));

            let consumer_types = resolver
                .resolve_and_validate(consumer_arc, &EffectiveDate::Origin, &already_resolved)
                .expect("consumer spec resolves");

            let eur_per_kg_owner = consumer_types
                .unit_index
                .unique_owner("eur_per_kg")
                .expect("eur_per_kg in unit_index");
            assert_eq!(
                eur_per_kg_owner.name.as_deref(),
                Some("price_per_weight"),
                "unit must belong to family root, not binding alias row name"
            );
            assert_eq!(
                eur_per_kg_owner.measure_family_name(),
                Some("price_per_weight")
            );
            assert_eq!(eur_per_kg_owner.as_ref(), units_price_per_weight.as_ref());

            let starch_alias = consumer_types
                .resolved
                .get("starch_levy_per_kg")
                .expect("alias row in resolved");
            assert!(
                !std::ptr::eq(eur_per_kg_owner.as_ref(), starch_alias.as_ref()),
                "unit_index must not store binding alias arc"
            );
        }

        #[test]
        fn import_merge_skips_locally_owned_family_root() {
            let code = r#"spec std_units
    data mass: measure
      -> unit kilogram: 1
      -> unit kilograms: 1
      -> unit gram: 0.001

    spec s
    uses u: std_units
    data mass: measure
      -> unit kg: 1
      -> unit tonne: 1000
    rule smoke: true
    "#;
            let (resolver, spec_arcs) = resolver_for_code(code);
            let std_units_arc = spec_arcs
                .iter()
                .find(|spec| spec.name == "std_units")
                .cloned()
                .expect("std_units spec");
            let consumer_arc = spec_arcs
                .iter()
                .find(|spec| spec.name == "s")
                .cloned()
                .expect("consumer spec");
            let (context, _) = test_context_and_effective(&spec_arcs);
            let repository = context.workspace();

            let mut already_resolved = ResolvedTypesMap::new();
            let std_units_types = resolver
                .resolve_and_validate(std_units_arc, &EffectiveDate::Origin, &already_resolved)
                .expect("std_units resolves");
            already_resolved.push((Arc::clone(&repository), std_units_arc, std_units_types));

            let consumer_types = resolver
                .resolve_and_validate(consumer_arc, &EffectiveDate::Origin, &already_resolved)
                .expect("consumer resolves");

            let local_mass = consumer_types
                .resolved
                .get("mass")
                .expect("consumer defines mass")
                .clone();

            assert!(
                consumer_types.unit_index.has_unique_owner("kg"),
                "local kg must be indexed"
            );
            assert!(
                consumer_types.unit_index.has_unique_owner("tonne"),
                "local tonne must be indexed"
            );
            assert!(
                !consumer_types.unit_index.has_unique_owner("kilogram"),
                "import kilogram must not leak after local shadow"
            );
            assert!(
                !consumer_types.unit_index.has_unique_owner("kilograms"),
                "import kilograms must not leak after local shadow"
            );
            assert!(
                !consumer_types.unit_index.has_unique_owner("gram"),
                "import gram must not leak after local shadow"
            );

            let kg_owner = consumer_types
                .unit_index
                .unique_owner("kg")
                .expect("kg owner");
            let tonne_owner = consumer_types
                .unit_index
                .unique_owner("tonne")
                .expect("tonne owner");
            assert_eq!(kg_owner.as_ref(), local_mass.as_ref());
            assert_eq!(tonne_owner.as_ref(), local_mass.as_ref());
        }

        #[test]
        fn same_family_same_unit_same_factor_is_idempotent() {
            let code = r#"spec units_a
    data duration: measure
      -> unit hour: 1

    spec consumer
    uses a: units_a
    uses b: units_a
    rule smoke: true
    "#;
            let (resolver, spec_arcs) = resolver_for_code(code);
            let units_a_arc = spec_arcs
                .iter()
                .find(|spec| spec.name == "units_a")
                .cloned()
                .expect("units_a spec");
            let consumer_arc = spec_arcs
                .iter()
                .find(|spec| spec.name == "consumer")
                .cloned()
                .expect("consumer spec");
            let (context, _) = test_context_and_effective(&spec_arcs);
            let repository = context.workspace();

            let mut already_resolved = ResolvedTypesMap::new();
            let units_a_types = resolver
                .resolve_and_validate(units_a_arc, &EffectiveDate::Origin, &already_resolved)
                .expect("units_a resolves");
            already_resolved.push((Arc::clone(&repository), units_a_arc, units_a_types));

            let consumer_types = resolver
                .resolve_and_validate(consumer_arc, &EffectiveDate::Origin, &already_resolved)
                .expect("double import of same duration must be idempotent");

            assert!(
                consumer_types.unit_index.has_unique_owner("hour"),
                "hour must be indexed once"
            );
        }

        #[test]
        fn same_family_same_unit_different_factor_errors() {
            let code = r#"spec units_a
    data duration: measure
      -> unit hour: 1

    spec units_b
    data duration: measure
      -> unit hour: 60

    spec consumer
    uses a: units_a
    uses b: units_b
    rule smoke: true
    "#;
            let (resolver, spec_arcs) = resolver_for_code(code);
            let units_a_arc = spec_arcs
                .iter()
                .find(|spec| spec.name == "units_a")
                .cloned()
                .expect("units_a spec");
            let units_b_arc = spec_arcs
                .iter()
                .find(|spec| spec.name == "units_b")
                .cloned()
                .expect("units_b spec");
            let consumer_arc = spec_arcs
                .iter()
                .find(|spec| spec.name == "consumer")
                .cloned()
                .expect("consumer spec");
            let (context, _) = test_context_and_effective(&spec_arcs);
            let repository = context.workspace();

            let mut already_resolved = ResolvedTypesMap::new();
            for units_arc in [units_a_arc, units_b_arc] {
                let units_types = resolver
                    .resolve_and_validate(units_arc, &EffectiveDate::Origin, &already_resolved)
                    .expect("units spec resolves");
                already_resolved.push((Arc::clone(&repository), units_arc, units_types));
            }

            let result = resolver.resolve_and_validate(
                consumer_arc,
                &EffectiveDate::Origin,
                &already_resolved,
            );
            assert!(
                result.is_err(),
                "conflicting hour factors in same family must error"
            );
            let error_msg = result
                .unwrap_err()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ");
            assert!(
                error_msg.contains("hour")
                    && (error_msg.contains("different values")
                        || error_msg.contains("conflicting factors")),
                "expected conflicting factor error for hour, got: {error_msg}"
            );
        }

        #[test]
        fn test_spec_level_duplicate_unit_names_rejected() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data money_a: measure
      -> unit eur: 1.00
      -> unit usd: 0.84

    data money_b: measure
      -> unit eur: 1.00
      -> unit usd: 1.20

    data length_a: measure
      -> unit meter: 1.0

    data length_b: measure
      -> unit meter: 1.0"#,
            );

            let result = resolver.resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new());
            assert!(
                result.is_err(),
                "second independent declarer of same unit name must Error"
            );
            let error_msg = result
                .unwrap_err()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ");
            assert!(
                error_msg.contains("eur")
                    || error_msg.contains("meter")
                    || error_msg.contains("usd"),
                "expected duplicate unit name Error, got: {error_msg}"
            );
        }

        #[test]
        fn test_ratio_unit_cross_family_collision_errors() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data q: measure
      -> unit foo: 1

    data r: ratio
      -> unit foo: 100"#,
            );

            let result = resolver.resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new());
            assert!(
                result.is_err(),
                "measure and ratio must not share a unit name"
            );
            let error_msg = result
                .unwrap_err()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ");
            assert!(
                error_msg.contains("foo"),
                "expected cross-family collision on 'foo', got: {}",
                error_msg
            );
        }

        #[test]
        fn test_same_ratio_unit_across_types_rejected() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data spread_a: ratio
      -> unit basis_points: 10000

    data spread_b: ratio
      -> unit basis_points: 10000"#,
            );

            let result = resolver.resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new());
            assert!(
                result.is_err(),
                "second independent declarer of same ratio unit must Error"
            );
            let error_msg = result
                .unwrap_err()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ");
            assert!(
                error_msg.contains("basis_points"),
                "expected duplicate ratio unit Error, got: {error_msg}"
            );
        }

        #[test]
        fn test_different_ratio_unit_factor_across_types_errors() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data spread_a: ratio
      -> unit basis_points: 10000

    data spread_b: ratio
      -> unit basis_points: 5000"#,
            );

            let result = resolver.resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new());
            assert!(
                result.is_err(),
                "unrelated ratio types with different factors for the same unit must error"
            );
            let error_msg = result
                .unwrap_err()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ");
            assert!(
                error_msg.contains("spread_a") && error_msg.contains("spread_b"),
                "expected ambiguous ratio unit between types, got: {}",
                error_msg
            );
        }

        #[test]
        fn test_multiple_builtin_ratio_types_share_percent_in_unit_index() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec targets
    data standard_margin_pct: ratio
      -> minimum 0%
      -> suggest 15%

    data default_credit_insurance_pct: ratio
      -> suggest 1.5%"#,
            );

            resolver
                .resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new())
                .expect("multiple ratio types with built-in percent must not conflict");
        }

        #[test]
        fn test_three_ratio_types_share_builtin_and_custom_unit() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data margin: ratio -> suggest 10%
    data fee: ratio
      -> unit tenths: 10
    data tax: ratio -> suggest 1%"#,
            );

            resolver
                .resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new())
                .expect("builtin percent/permille plus custom tenths on second type must load");
        }

        #[test]
        fn test_ratio_unit_index_allows_builtin_after_two_named_types() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data first_ratio: ratio -> suggest 5%
    data second_ratio: ratio -> suggest 10%"#,
            );

            let resolved = resolver
                .resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new())
                .expect("two ratio types with only builtin units");

            assert!(
                resolved.unit_index.has_unique_owner("percent"),
                "percent must remain in unit index"
            );
            assert!(
                resolved.unit_index.has_unique_owner("permille"),
                "permille must remain in unit index"
            );
        }

        #[test]
        fn test_number_type_cannot_have_units() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data price: number
      -> unit eur: 1.00"#,
            );

            let result = resolver.resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new());
            assert!(result.is_err(), "Number types must reject unit commands");

            let errs = result.unwrap_err();
            assert!(!errs.is_empty(), "expected at least one error");
            let error_msg = errs[0].to_string();
            assert!(
                error_msg.contains("unit") && error_msg.contains("number"),
                "Error should mention units are invalid on number. Got: {}",
                error_msg
            );
        }

        #[test]
        fn test_extending_type_inherits_units() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data money: measure
      -> unit eur: 1.00
      -> unit usd: 0.84

    data my_money: money
      -> unit gbp: 1.30"#,
            );

            let resolved = resolver
                .resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new())
                .unwrap();
            let my_money_type = resolved.resolved.get("my_money").unwrap();

            match &my_money_type.specifications {
                TypeSpecification::Measure { units, .. } => {
                    assert_eq!(units.len(), 3);
                    assert!(units.iter().any(|u| u.name == "eur"));
                    assert!(units.iter().any(|u| u.name == "usd"));
                    assert!(units.iter().any(|u| u.name == "gbp"));
                }
                other => panic!("Expected Measure type specifications, got {:?}", other),
            }
        }

        #[test]
        fn binding_unit_factor_override_errors() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data source_measure: measure
      -> unit usd: 1.00
    data z: source_measure
      -> unit usd: 0.84"#,
            );

            let result = resolver.resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new());
            assert!(
                result.is_err(),
                "binding row must not override inherited unit factor"
            );
            let error_msg = result
                .unwrap_err()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ");
            assert!(
                error_msg.contains("usd"),
                "error must name unit usd, got: {error_msg}"
            );
        }

        #[test]
        fn ratio_extension_unit_factor_override_errors() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data parent: ratio
      -> unit basis: 1
    data child: parent
      -> unit basis: 100"#,
            );

            let result = resolver.resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new());
            assert!(
                result.is_err(),
                "ratio extension must not override inherited unit factor"
            );
            let error_msg = result
                .unwrap_err()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ");
            assert!(
                error_msg.contains("basis"),
                "error must name unit basis, got: {error_msg}"
            );
        }

        #[test]
        fn test_duplicate_unit_name_in_same_type_is_error() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data money: measure
      -> unit eur: 1.00
      -> unit eur: 1.19"#,
            );

            let result = resolver.resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new());
            assert!(result.is_err(), "duplicate unit name must be rejected");
            let errs = result.unwrap_err();
            assert!(!errs.is_empty());
            let msg = errs[0].to_string();
            assert!(
                msg.contains("eur"),
                "error must name the duplicate unit; got: {msg}"
            );
        }

        #[test]
        fn test_duplicate_unit_name_compound_is_error() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data currency: measure
      -> unit usd: 1.00
      -> unit eur: 0.92

    data time_period: measure
      -> unit year: 1

    data run_rate: measure
      -> unit arr: usd/year
      -> unit arr: eur/year"#,
            );

            let result = resolver.resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new());
            assert!(
                result.is_err(),
                "duplicate compound unit name must be rejected"
            );
            let errs = result.unwrap_err();
            assert!(!errs.is_empty());
            let msg = errs[0].to_string();
            assert!(
                msg.contains("arr"),
                "error must name the duplicate unit; got: {msg}"
            );
        }
    }

    mod validation {
        use super::super::*;
        use crate::computation::rational::rational_new;
        use crate::parsing::ast::{CommandArg, TypeConstraintCommand};
        use crate::planning::semantics::TypeSpecification;
        use rust_decimal::Decimal;

        fn test_source() -> Source {
            Source::new(
                crate::parsing::source::SourceType::Volatile,
                crate::parsing::ast::Span {
                    start: 0,
                    end: 0,
                    line: 1,
                    col: 0,
                },
            )
        }

        fn apply(
            mut specs: TypeSpecification,
            command: TypeConstraintCommand,
            args: &[CommandArg],
        ) -> TypeSpecification {
            let mut suggestion: Option<RawSuggestion> = None;
            specs
                .apply_constraint("test", command, args, &mut suggestion, &mut None)
                .unwrap();
            specs
        }

        fn number_arg(n: i64) -> CommandArg {
            CommandArg::Literal(crate::literals::Value::Number(Decimal::from(n)))
        }

        fn date_arg(s: &str) -> CommandArg {
            let dt = s.parse::<crate::literals::DateTimeValue>().expect("date");
            CommandArg::Literal(crate::literals::Value::Date(dt))
        }

        fn time_arg(s: &str) -> CommandArg {
            let t = s.parse::<crate::literals::TimeValue>().expect("time");
            CommandArg::Literal(crate::literals::Value::Time(t))
        }

        #[test]
        fn validate_number_minimum_greater_than_maximum() {
            let mut specs = TypeSpecification::number();
            specs = apply(specs, TypeConstraintCommand::Minimum, &[number_arg(100)]);
            specs = apply(specs, TypeConstraintCommand::Maximum, &[number_arg(50)]);

            let src = test_source();
            let errors = validate_type_specifications(
                &specs,
                None,
                "suggestion",
                "test",
                &src,
                None,
                &UnitIndex::new(),
            );
            assert_eq!(errors.len(), 1);
            assert!(errors[0]
                .to_string()
                .contains("minimum 100 is greater than maximum 50"));
        }

        #[test]
        fn validate_number_default_below_minimum() {
            let specs = TypeSpecification::Number {
                minimum: Some(rational_new(10, 1)),
                maximum: None,
                decimals: None,
                help: String::new(),
            };
            let default = ValueKind::Number(rational_new(5, 1));

            let src = test_source();
            let errors = validate_type_specifications(
                &specs,
                Some(&default),
                "suggestion",
                "test",
                &src,
                None,
                &UnitIndex::new(),
            );
            assert_eq!(errors.len(), 1);
            assert!(errors[0]
                .to_string()
                .contains("suggestion value 5 is less than minimum 10"));
        }

        #[test]
        fn validate_number_default_above_maximum() {
            let specs = TypeSpecification::Number {
                minimum: None,
                maximum: Some(rational_new(100, 1)),
                decimals: None,
                help: String::new(),
            };
            let default = ValueKind::Number(rational_new(150, 1));

            let src = test_source();
            let errors = validate_type_specifications(
                &specs,
                Some(&default),
                "suggestion",
                "test",
                &src,
                None,
                &UnitIndex::new(),
            );
            assert_eq!(errors.len(), 1);
            assert!(errors[0]
                .to_string()
                .contains("suggestion value 150 is greater than maximum 100"));
        }

        #[test]
        fn validate_number_default_valid() {
            let specs = TypeSpecification::Number {
                minimum: Some(rational_new(0, 1)),
                maximum: Some(rational_new(100, 1)),
                decimals: None,
                help: String::new(),
            };
            let default = ValueKind::Number(rational_new(50, 1));

            let src = test_source();
            let errors = validate_type_specifications(
                &specs,
                Some(&default),
                "suggestion",
                "test",
                &src,
                None,
                &UnitIndex::new(),
            );
            assert!(errors.is_empty());
        }

        #[test]
        fn text_minimum_command_is_rejected() {
            let mut specs = TypeSpecification::text();
            let res = specs.apply_constraint(
                "test",
                TypeConstraintCommand::Minimum,
                &[number_arg(5)],
                &mut None,
                &mut None,
            );
            assert!(res.is_err());
            assert!(res
                .unwrap_err()
                .contains("Invalid command 'minimum' for text type"));
        }

        #[test]
        fn text_maximum_command_is_rejected() {
            let mut specs = TypeSpecification::text();
            let res = specs.apply_constraint(
                "test",
                TypeConstraintCommand::Maximum,
                &[number_arg(5)],
                &mut None,
                &mut None,
            );
            assert!(res.is_err());
            assert!(res
                .unwrap_err()
                .contains("Invalid command 'maximum' for text type"));
        }

        #[test]
        fn validate_text_default_not_in_options() {
            let specs = TypeSpecification::Text {
                length: None,
                options: vec!["red".to_string(), "blue".to_string()],
                help: String::new(),
            };
            let default = ValueKind::Text("green".to_string());

            let src = test_source();
            let errors = validate_type_specifications(
                &specs,
                Some(&default),
                "suggestion",
                "test",
                &src,
                None,
                &UnitIndex::new(),
            );
            assert_eq!(errors.len(), 1);
            assert!(errors[0]
                .to_string()
                .contains("suggestion value 'green' is not in allowed options"));
        }

        #[test]
        fn validate_ratio_minimum_greater_than_maximum() {
            let specs = TypeSpecification::Ratio {
                minimum: Some(rational_new(2, 1)),
                maximum: Some(rational_new(1, 1)),
                decimals: None,
                units: crate::planning::semantics::RatioUnits::new(),
                help: String::new(),
            };

            let src = test_source();
            let errors = validate_type_specifications(
                &specs,
                None,
                "suggestion",
                "test",
                &src,
                None,
                &UnitIndex::new(),
            );
            assert_eq!(errors.len(), 1);
            assert!(errors[0]
                .to_string()
                .contains("minimum 2 is greater than maximum 1"));
        }

        #[test]
        fn validate_date_minimum_after_maximum() {
            let mut specs = TypeSpecification::date();
            specs = apply(
                specs,
                TypeConstraintCommand::Minimum,
                &[date_arg("2024-12-31")],
            );
            specs = apply(
                specs,
                TypeConstraintCommand::Maximum,
                &[date_arg("2024-01-01")],
            );

            let src = test_source();
            let errors = validate_type_specifications(
                &specs,
                None,
                "suggestion",
                "test",
                &src,
                None,
                &UnitIndex::new(),
            );
            assert_eq!(errors.len(), 1);
            assert!(
                errors[0].to_string().contains("minimum")
                    && errors[0].to_string().contains("is after maximum")
            );
        }

        #[test]
        fn validate_date_valid_range() {
            let mut specs = TypeSpecification::date();
            specs = apply(
                specs,
                TypeConstraintCommand::Minimum,
                &[date_arg("2024-01-01")],
            );
            specs = apply(
                specs,
                TypeConstraintCommand::Maximum,
                &[date_arg("2024-12-31")],
            );

            let src = test_source();
            let errors = validate_type_specifications(
                &specs,
                None,
                "suggestion",
                "test",
                &src,
                None,
                &UnitIndex::new(),
            );
            assert!(errors.is_empty());
        }

        #[test]
        fn validate_time_minimum_after_maximum() {
            let mut specs = TypeSpecification::time();
            specs = apply(
                specs,
                TypeConstraintCommand::Minimum,
                &[time_arg("23:00:00")],
            );
            specs = apply(
                specs,
                TypeConstraintCommand::Maximum,
                &[time_arg("10:00:00")],
            );

            let src = test_source();
            let errors = validate_type_specifications(
                &specs,
                None,
                "suggestion",
                "test",
                &src,
                None,
                &UnitIndex::new(),
            );
            assert_eq!(errors.len(), 1);
            assert!(
                errors[0].to_string().contains("minimum")
                    && errors[0].to_string().contains("is after maximum")
            );
        }

        #[test]
        fn large_magnitude_minimum_does_not_fail_type_validation() {
            use crate::computation::rational::rational_one;
            use crate::literals::MeasureUnits;
            use crate::planning::semantics::TypeSpecification;

            let too_large = magnitude_beyond_decimal_max();
            assert_eq!(
                too_large.try_to_decimal().unwrap_err(),
                crate::computation::rational::NumericFailure::Overflow,
            );

            let spec = TypeSpecification::Measure {
                minimum: Some((too_large, "eur".to_string())),
                maximum: None,
                decimals: None,
                units: MeasureUnits(vec![crate::literals::MeasureUnit {
                    name: "eur".to_string(),
                    factor: rational_one(),
                    derived_measure_factors: Default::default(),
                    decomposition: Default::default(),
                    minimum: None,
                    maximum: None,
                    suggestion_magnitude: None,
                }]),
                traits: vec![],
                decomposition: None,
                help: String::new(),
            };

            let src = test_source();
            let errors = validate_type_specifications(
                &spec,
                None,
                "suggestion",
                "money",
                &src,
                None,
                &UnitIndex::new(),
            );
            assert!(
                errors.is_empty(),
                "internal Q bounds must not fail type validation; API decimal rounding: {:?}",
                errors.iter().map(|e| e.to_string()).collect::<Vec<_>>()
            );
        }

        #[test]
        fn large_magnitude_maximum_does_not_fail_type_validation() {
            use crate::computation::rational::rational_one;
            use crate::literals::MeasureUnits;
            use crate::planning::semantics::TypeSpecification;

            let too_large = magnitude_beyond_decimal_max();
            assert_eq!(
                too_large.try_to_decimal().unwrap_err(),
                crate::computation::rational::NumericFailure::Overflow,
            );

            let spec = TypeSpecification::Measure {
                minimum: None,
                maximum: Some((too_large, "eur".to_string())),
                decimals: None,
                units: MeasureUnits(vec![crate::literals::MeasureUnit {
                    name: "eur".to_string(),
                    factor: rational_one(),
                    derived_measure_factors: Default::default(),
                    decomposition: Default::default(),
                    minimum: None,
                    maximum: None,
                    suggestion_magnitude: None,
                }]),
                traits: vec![],
                decomposition: None,
                help: String::new(),
            };

            let src = test_source();
            let errors = validate_type_specifications(
                &spec,
                None,
                "suggestion",
                "money",
                &src,
                None,
                &UnitIndex::new(),
            );
            assert!(errors.is_empty(), "got: {:?}", errors);
        }

        #[test]
        fn large_magnitude_default_does_not_fail_type_validation() {
            use crate::computation::rational::rational_one;
            use crate::literals::MeasureUnits;
            use crate::planning::semantics::TypeSpecification;

            let too_large = magnitude_beyond_decimal_max();
            assert_eq!(
                too_large.try_to_decimal().unwrap_err(),
                crate::computation::rational::NumericFailure::Overflow,
            );

            let spec = TypeSpecification::Measure {
                minimum: None,
                maximum: None,
                decimals: None,
                units: MeasureUnits(vec![crate::literals::MeasureUnit {
                    name: "eur".to_string(),
                    factor: rational_one(),
                    derived_measure_factors: Default::default(),
                    decomposition: Default::default(),
                    minimum: None,
                    maximum: None,
                    suggestion_magnitude: Some(too_large),
                }]),
                traits: vec![],
                decomposition: None,
                help: String::new(),
            };

            let src = test_source();
            let errors = validate_type_specifications(
                &spec,
                None,
                "suggestion",
                "money",
                &src,
                None,
                &UnitIndex::new(),
            );
            assert!(errors.is_empty(), "got: {:?}", errors);
        }

        fn magnitude_beyond_decimal_max() -> crate::computation::rational::RationalInteger {
            use crate::computation::rational::{decimal_to_rational, rational_new, try_mul};
            use rust_decimal::Decimal;
            let max = Decimal::MAX.normalize();
            let max_rational = decimal_to_rational(max).expect("BUG: Decimal::MAX must lift to Q");
            try_mul(&max_rational, &rational_new(2, 1))
                .expect("BUG: test rational multiply must succeed")
        }

        #[test]
        fn empty_measure_signature_unit_must_not_coerce_as_unknown_empty_string() {
            use crate::computation::rational::rational_new;
            use crate::literals::MeasureUnits;
            use crate::planning::semantics::{
                LemmaType, TypeSpecification, TypedLiteral, ValueKind,
            };

            let schema_type = Arc::new(LemmaType {
                name: Some("money".to_string()),
                extends: TypeExtends::Primitive,
                specifications: TypeSpecification::Measure {
                    minimum: None,
                    maximum: None,
                    decimals: None,
                    units: MeasureUnits(vec![crate::literals::MeasureUnit {
                        name: "eur".to_string(),
                        factor: rational_new(1, 1),
                        derived_measure_factors: Default::default(),
                        decomposition: Default::default(),
                        minimum: None,
                        maximum: None,
                        suggestion_magnitude: None,
                    }]),
                    traits: vec![],
                    decomposition: Default::default(),
                    help: String::new(),
                },
                measure_binding_unit: None,
            });

            let literal = TypedLiteral {
                value: ValueKind::Measure(rational_new(1, 1)),
                lemma_type: Arc::clone(&schema_type),
            };

            let result = Graph::coerce_literal_to_schema_type(&literal, &schema_type);

            // Binding unit lives on lemma_type; equal Measure specs coerce without a
            // ValueKind signature. Empty binding is valid.
            assert!(
                result.is_ok(),
                "measure with matching schema type must coerce, got: {result:?}"
            );
        }

        #[test]
        fn refresh_named_range_specs_missing_element_must_not_leave_unconverted_measure() {
            use crate::engine::Context;
            use crate::parsing::ast::{LemmaSpec, ParentType, Span};
            use crate::parsing::source::{Source, SourceType};
            use crate::planning::semantics::tests::primitive_measure_arc;

            let ctx = Box::leak(Box::new(Context::new()));
            let resolver = TypeResolver::new(ctx);
            let spec = LemmaSpec::new("t".to_string());
            let source = Source::new(
                SourceType::Volatile,
                Span {
                    start: 0,
                    end: 0,
                    line: 1,
                    col: 0,
                },
            );
            let mut data_defs = HashMap::new();
            data_defs.insert(
                "band".to_string(),
                DataTypeDef {
                    parent: ParentType::Ranged {
                        inner: Box::new(ParentType::Custom {
                            name: "ghost".to_string(),
                        }),
                    },
                    constraints: None,
                    source: source.clone(),
                    name: "band".to_string(),
                    bound_literal: None,
                },
            );
            let mut resolved = IndexMap::new();
            resolved.insert("band".to_string(), primitive_measure_arc().clone());
            let mut defaults = IndexMap::new();
            let already_resolved = ResolvedTypesMap::new();

            let errors = refresh_named_range_specs(
                &resolver,
                &spec,
                &data_defs,
                &mut resolved,
                &mut defaults,
                &already_resolved,
                &EffectiveDate::Origin,
            );

            assert!(
                !errors.is_empty(),
                "planning must error when ranged inner type is missing"
            );
            assert!(
                errors[0]
                    .to_string()
                    .contains("references missing element type 'ghost'"),
                "got: {}",
                errors[0]
            );
        }

        fn apply_n_text_options(option_count: usize) -> TypeSpecification {
            let spec = LemmaSpec::new("wide_options".to_string());
            let source = test_source();
            let constraints: Vec<Constraint> = (0..option_count)
                .map(|i| {
                    Constraint::new(
                        TypeConstraintCommand::Option,
                        vec![CommandArg::Literal(crate::literals::Value::Text(
                            i.to_string(),
                        ))],
                        source.clone(),
                    )
                })
                .collect();
            let mut suggestion = None;
            let mut fill = None;
            apply_constraints_to_spec(
                &spec,
                "code",
                TypeSpecification::text(),
                &constraints,
                &source,
                &mut suggestion,
                &mut fill,
            )
            .expect("options must apply")
        }

        #[test]
        fn apply_constraints_many_text_options_preserve_count() {
            for count in [16_usize, 64] {
                match apply_n_text_options(count) {
                    TypeSpecification::Text { options, .. } => {
                        assert_eq!(options.len(), count);
                    }
                    other => panic!("expected Text, got {other:?}"),
                }
            }
        }
    }

    mod decomposition_promotion {
        use super::super::*;
        use crate::engine::Context;
        use crate::parsing::parse;
        use crate::planning::typing::range_span_type;
        use crate::ResourceLimits;
        use std::sync::Arc;

        fn insert_parsed_repos(ctx: &mut Context, code: &str) {
            let result = parse(
                code,
                crate::parsing::source::SourceType::Volatile,
                &ResourceLimits::default(),
            )
            .expect("parse");
            for (repo_arc, specs) in result.repositories {
                for spec in specs {
                    ctx.insert_spec(Arc::clone(&repo_arc), spec)
                        .expect("insert spec");
                }
            }
        }

        fn repo_by_name(ctx: &Context, name: &str) -> Arc<LemmaRepository> {
            ctx.repositories()
                .keys()
                .find(|r| r.name.as_deref() == Some(name))
                .cloned()
                .expect("repository must exist after insert")
        }

        fn resolve_specs_in_order<'a>(
            resolver: &TypeResolver<'a>,
            repo: &Arc<LemmaRepository>,
            specs: &[&'a LemmaSpec],
            local_types: &mut ResolvedTypesMap<'a>,
        ) {
            for spec in specs {
                let types = resolver
                    .resolve_and_validate(spec, &EffectiveDate::Origin, local_types)
                    .unwrap_or_else(|e| panic!("resolve {} failed: {:?}", spec.name, e));
                local_types.push((Arc::clone(repo), *spec, types));
            }
        }

        fn worker_torque_fixture() -> (
            ResolvedTypesMap<'static>,
            &'static LemmaSpec,
            BaseMeasureVector,
            Arc<LemmaType>,
        ) {
            let mut ctx = Context::new();
            insert_parsed_repos(&mut ctx, crate::stdlib::UNITS_LEMMA);
            let alpha_code = r#"repo alpha
    spec units
    data force: measure
      -> unit newton: 1
    data length: measure
      -> unit meter: 1
    data torque: measure
      -> unit nm: newton*meter

    spec worker
    uses u: alpha units
    data f: 3 newton
    data d: 4 meter
    rule t: f * d
    "#;
            insert_parsed_repos(&mut ctx, alpha_code);
            let ctx = Box::leak(Box::new(ctx));
            let alpha_repo = repo_by_name(ctx, "alpha");
            let worker = ctx
                .spec_set(&alpha_repo, "worker")
                .and_then(|ss| ss.get_exact(None))
                .expect("worker spec");
            let units = ctx
                .spec_set(&alpha_repo, "units")
                .and_then(|ss| ss.get_exact(None))
                .expect("alpha units spec");
            let lemma_repo = repo_by_name(ctx, crate::engine::EMBEDDED_STDLIB_REPOSITORY);
            let lemma_units = ctx
                .spec_set(&lemma_repo, "units")
                .expect("lemma repo")
                .get_exact(None)
                .expect("lemma units spec");

            let mut resolver = TypeResolver::new(ctx);
            for spec in [units, worker] {
                resolver
                    .register_all(&alpha_repo, spec)
                    .into_iter()
                    .for_each(|e| panic!("register alpha: {e:?}"));
            }
            resolver
                .register_all(&lemma_repo, lemma_units)
                .into_iter()
                .for_each(|e| panic!("register lemma units: {e:?}"));

            let mut local_types = ResolvedTypesMap::new();
            resolve_specs_in_order(
                &resolver,
                &lemma_repo,
                std::slice::from_ref(&lemma_units),
                &mut local_types,
            );
            resolve_specs_in_order(
                &resolver,
                &alpha_repo,
                std::slice::from_ref(&units),
                &mut local_types,
            );
            resolve_specs_in_order(
                &resolver,
                &alpha_repo,
                std::slice::from_ref(&worker),
                &mut local_types,
            );

            let alpha_torque = local_types
                .iter()
                .find(|(_, s, _)| discovery::same_loaded_spec(s, units))
                .and_then(|(_, _, t)| t.resolved.get("torque"))
                .expect("alpha torque type")
                .clone();
            let decomp = alpha_torque
                .measure_type_decomposition()
                .expect("torque decomposition")
                .clone();

            (local_types, worker, decomp, alpha_torque)
        }

        #[test]
        fn unique_decomposition_carries_matching_arc() {
            let (map, worker_spec, decomp, alpha_torque) = worker_torque_fixture();
            let unique = find_unique_measure_type_in_unit_index(
                &find_types_by_spec(&map, worker_spec)
                    .expect("worker in map")
                    .unit_index,
                &decomp,
            );
            match unique {
                DecompositionMatch::Unique(arc) => {
                    assert_eq!(*arc, *alpha_torque);
                    assert_eq!(arc.name.as_deref(), Some("torque"));
                }
                other => panic!("expected Unique(Arc) torque match, got {other:?}"),
            }
        }

        #[test]
        fn find_unique_ignores_polluted_resolved_map() {
            let (mut map, worker_spec, decomp, alpha_torque) = worker_torque_fixture();
            map.iter_mut()
                .find(|(_, s, _)| discovery::same_loaded_spec(s, worker_spec))
                .expect("worker must be in resolved map")
                .2
                .resolved = IndexMap::new();

            let unique = find_unique_measure_type_in_unit_index(
                &find_types_by_spec(&map, worker_spec)
                    .expect("worker in map")
                    .unit_index,
                &decomp,
            );
            match unique {
                DecompositionMatch::Unique(arc) => {
                    assert_eq!(*arc, *alpha_torque);
                    assert_eq!(arc.name.as_deref(), Some("torque"));
                }
                other => panic!("expected Unique(alpha torque) via unit_index, got {other:?}"),
            }
        }

        fn weight_measure_type() -> Arc<LemmaType> {
            use crate::computation::rational::{decimal_to_rational, rational_one};
            use rust_decimal::Decimal;
            Arc::new(LemmaType::new(
                "weight".to_string(),
                TypeSpecification::Measure {
                    minimum: None,
                    maximum: None,
                    decimals: None,
                    units: MeasureUnits(vec![
                        MeasureUnit {
                            name: "gram".to_string(),
                            factor: rational_one(),
                            derived_measure_factors: Vec::new(),
                            decomposition: Default::default(),
                            minimum: None,
                            maximum: None,
                            suggestion_magnitude: None,
                        },
                        MeasureUnit {
                            name: "kilogram".to_string(),
                            factor: decimal_to_rational(Decimal::from(1000))
                                .expect("BUG: kilogram factor must be rational"),
                            derived_measure_factors: Vec::new(),
                            decomposition: Default::default(),
                            minimum: None,
                            maximum: None,
                            suggestion_magnitude: None,
                        },
                    ]),
                    traits: Vec::new(),
                    decomposition: None,
                    help: String::new(),
                },
                TypeExtends::Primitive,
            ))
        }

        fn weight_measure_range_type() -> Arc<LemmaType> {
            let weight = weight_measure_type();
            Arc::new(LemmaType {
                name: weight.name.clone(),
                specifications: weight
                    .specifications
                    .range_from_element()
                    .expect("BUG: weight measure defines MeasureRange"),
                extends: weight.extends.clone(),
                measure_binding_unit: None,
            })
        }

        fn duration_like_measure_type() -> Arc<LemmaType> {
            Arc::new(LemmaType::anonymous_for_decomposition(
                duration_decomposition(),
            ))
        }

        fn calendar_like_measure_type() -> Arc<LemmaType> {
            use crate::computation::rational::rational_one;
            use crate::planning::semantics::MeasureTrait;
            Arc::new(LemmaType::new(
                "calendar".to_string(),
                TypeSpecification::Measure {
                    minimum: None,
                    maximum: None,
                    decimals: None,
                    units: MeasureUnits(vec![MeasureUnit {
                        name: "month".to_string(),
                        factor: rational_one(),
                        derived_measure_factors: Vec::new(),
                        decomposition: calendar_decomposition(),
                        minimum: None,
                        maximum: None,
                        suggestion_magnitude: None,
                    }]),
                    traits: vec![MeasureTrait::Calendar],
                    decomposition: Some(calendar_decomposition()),
                    help: String::new(),
                },
                TypeExtends::Primitive,
            ))
        }

        #[test]
        fn range_span_type_date_range_is_duration_span_not_date_range() {
            let date_range = Arc::new(LemmaType::primitive(TypeSpecification::date_range()));
            let span = range_span_type(&date_range);
            assert!(span.is_duration_like_measure());
            assert!(!span.is_date_range());
            assert!(!span.is_date());
        }

        #[test]
        fn range_span_type_time_range_is_duration_span() {
            let time_range = Arc::new(LemmaType::primitive(TypeSpecification::time_range()));
            let span = range_span_type(&time_range);
            assert!(span.is_duration_like_measure());
            assert!(!span.is_time_range());
            assert!(!span.is_time());
        }

        #[test]
        fn range_span_type_number_range_is_number() {
            let number_range = Arc::new(LemmaType::primitive(TypeSpecification::number_range()));
            let span = range_span_type(&number_range);
            assert!(span.is_number());
        }

        #[test]
        fn range_span_type_measure_range_preserves_name_and_extends() {
            let weight_range = weight_measure_range_type();
            let span = range_span_type(&weight_range);
            assert!(span.is_measure());
            assert!(!span.is_measure_range());
            assert_eq!(span.name.as_deref(), Some("weight"));
            assert_eq!(span.extends, weight_range.extends);
            assert!(span.same_measure_family(weight_measure_type().as_ref()));
        }

        #[test]
        fn range_span_type_anonymous_measure_range_preserves_units() {
            use crate::computation::rational::{decimal_to_rational, rational_one};
            use rust_decimal::Decimal;
            let units = MeasureUnits(vec![
                MeasureUnit {
                    name: "gram".to_string(),
                    factor: rational_one(),
                    derived_measure_factors: Vec::new(),
                    decomposition: Default::default(),
                    minimum: None,
                    maximum: None,
                    suggestion_magnitude: None,
                },
                MeasureUnit {
                    name: "kilogram".to_string(),
                    factor: decimal_to_rational(Decimal::from(1000))
                        .expect("BUG: kilogram factor must be rational"),
                    derived_measure_factors: Vec::new(),
                    decomposition: Default::default(),
                    minimum: None,
                    maximum: None,
                    suggestion_magnitude: None,
                },
            ]);
            let measure_range = Arc::new(LemmaType::without_name(
                TypeSpecification::MeasureRange {
                    lower: None,
                    upper: None,
                    minimum: None,
                    maximum: None,
                    units: units.clone(),
                    decomposition: None,
                    help: String::new(),
                },
                TypeExtends::Primitive,
            ));
            let span = range_span_type(&measure_range);
            assert!(span.is_measure());
            assert!(span.is_anonymous_measure());
            match &span.specifications {
                TypeSpecification::Measure {
                    units: span_units, ..
                } => {
                    assert_eq!(span_units, &units);
                }
                other => panic!("expected Measure span, got {other:?}"),
            }
        }

        #[test]
        fn range_span_type_ratio_range_is_ratio() {
            let ratio_range = Arc::new(LemmaType::primitive(TypeSpecification::ratio_range()));
            let span = range_span_type(&ratio_range);
            assert!(span.is_ratio());
        }

        #[test]
        fn range_span_type_non_range_is_undetermined() {
            let boolean = Arc::new(LemmaType::primitive(TypeSpecification::boolean()));
            let span = range_span_type(&boolean);
            assert!(span.is_undetermined());
        }

        #[test]
        fn arithmetic_measure_range_plus_measure_yields_named_measure_span() {
            let weight_range = weight_measure_range_type();
            let gram = weight_measure_type();
            let unit_index = UnitIndex::new();
            let signature_index = crate::computation::arithmetic::SignatureIndex::new();
            let scope = MeasureScope {
                unit_index: &unit_index,
                signature_index: &signature_index,
            };
            let result = compute_arithmetic_result_type(
                weight_range,
                &ArithmeticComputation::Add,
                gram,
                &scope,
            );
            assert!(result.is_measure());
            assert!(!result.is_measure_range());
            assert_eq!(result.name.as_deref(), Some("weight"));
            assert!(result.same_measure_family(weight_measure_type().as_ref()));
        }

        #[test]
        fn arithmetic_measure_range_minus_measure_yields_named_measure_span() {
            let weight_range = weight_measure_range_type();
            let gram = weight_measure_type();
            let unit_index = UnitIndex::new();
            let signature_index = crate::computation::arithmetic::SignatureIndex::new();
            let scope = MeasureScope {
                unit_index: &unit_index,
                signature_index: &signature_index,
            };
            let result = compute_arithmetic_result_type(
                weight_range,
                &ArithmeticComputation::Subtract,
                gram,
                &scope,
            );
            assert!(result.is_measure());
            assert!(!result.is_measure_range());
            assert_eq!(result.name.as_deref(), Some("weight"));
            assert!(result.same_measure_family(weight_measure_type().as_ref()));
        }

        #[test]
        fn arithmetic_date_range_plus_duration_yields_duration_span_not_date_range() {
            let date_range = Arc::new(LemmaType::primitive(TypeSpecification::date_range()));
            let duration = duration_like_measure_type();
            let unit_index = UnitIndex::new();
            let signature_index = crate::computation::arithmetic::SignatureIndex::new();
            let scope = MeasureScope {
                unit_index: &unit_index,
                signature_index: &signature_index,
            };
            let result = compute_arithmetic_result_type(
                date_range,
                &ArithmeticComputation::Add,
                duration,
                &scope,
            );
            assert!(result.is_duration_like_measure());
            assert!(!result.is_date_range());
            assert!(!result.is_date());
        }

        #[test]
        fn arithmetic_date_range_plus_calendar_yields_date_range() {
            let date_range = Arc::new(LemmaType::primitive(TypeSpecification::date_range()));
            let calendar = calendar_like_measure_type();
            let unit_index = UnitIndex::new();
            let signature_index = crate::computation::arithmetic::SignatureIndex::new();
            let scope = MeasureScope {
                unit_index: &unit_index,
                signature_index: &signature_index,
            };
            let result = compute_arithmetic_result_type(
                date_range,
                &ArithmeticComputation::Add,
                calendar,
                &scope,
            );
            assert!(result.is_date_range());
        }
    }
}

// ============================================================================
// Type resolution
// ============================================================================

/// Fully resolved types for a single spec.
/// After resolution, all imports are inlined — specs are independent.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResolvedSpecTypes {
    /// Resolved [`LemmaType`] for each **data type row name** declared in this spec (`data name: …`).
    /// Planning-only: includes measure units and post-`resolve_measure_decompositions` decomposition.
    pub resolved: IndexMap<String, Arc<LemmaType>>,

    /// Declared default per named type (e.g. `type rate: ratio -> suggest 50%`).
    /// Only present for types that declared a `-> suggest ...` constraint anywhere
    /// in their extension chain; the inner-most `-> suggest` wins. Defaults live
    /// outside [`TypeSpecification`] so the type itself stays free of binding data.
    /// Populated after [`value_kind_from_raw_suggestion`] (post-decomposition).
    pub declared_suggestions: IndexMap<String, ValueKind>,

    /// Declared fill per named type (e.g. `type rate: ratio -> fill 50%`).
    /// Only present for types that declared a `-> fill ...` constraint anywhere
    /// in their extension chain; the inner-most `-> fill` wins.
    pub declared_fills: IndexMap<String, ValueKind>,

    /// Defaults captured during type resolution, before measure unit factors are final.
    pub(crate) raw_suggestions: Vec<(String, RawSuggestion)>,

    /// Raw fills captured during type resolution, before measure unit factors are final.
    pub(crate) raw_fills: Vec<(String, RawSuggestion)>,

    /// Raw defaults retained after [`value_kind_from_raw_suggestion`] for cross-spec parent lookup during later specs.
    pub(crate) source_defaults: IndexMap<String, RawSuggestion>,

    /// Raw fills retained for cross-spec parent lookup during later specs.
    pub(crate) source_fill_defaults: IndexMap<String, RawSuggestion>,

    /// Expression-scope units. One declarer per bare unit name; qualify with Type.unit or alias.Type.unit.
    /// Binding aliases never appear as index keys.
    pub unit_index: crate::planning::unit_index::UnitIndex,

    /// Reverse index: canonical-form unit signature → (unit_name, owning type).
    /// Built from [`Self::unit_index`] during resolve; cloned into the execution plan.
    pub signature_index: crate::computation::arithmetic::SignatureIndex,
}

/// Intermediate type definition extracted from [`DataValue::Definition`] data.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DataTypeDef {
    pub parent: ParentType,
    pub constraints: Option<Vec<Constraint>>,
    pub source: crate::parsing::source::Source,
    pub name: String,
    /// When the source row was `data N: <literal>` (no explicit parent type), the AST literal.
    pub bound_literal: Option<ast::Value>,
}

///
/// Named types are extracted from [`DataValue::Definition`] data and stored in
/// DAG-parallel maps keyed by Context-owned `&LemmaSpec` identity.
#[derive(Debug, Clone)]
pub(crate) struct TypeResolver<'a> {
    data_types: Vec<(&'a LemmaSpec, HashMap<String, DataTypeDef>)>,
    context: &'a Context,
    all_registered_specs: Vec<(Arc<LemmaRepository>, &'a LemmaSpec)>,
    /// Specs for which [`TypeResolver::register_all`] could not register every
    /// type-declaring data row. Planning must skip building these specs:
    /// downstream code (`add_data`) relies on every type-resolving data row
    /// being present in the resolved type map.
    specs_with_failed_registration: Vec<&'a LemmaSpec>,
}

/// Parent type used for measure-family lookup, unwrapping [`ParentType::Ranged`].
fn element_parent_type(parent: &ParentType) -> &ParentType {
    match parent {
        ParentType::Ranged { inner } => element_parent_type(inner),
        other => other,
    }
}

fn time_range_endpoints_share_timezone(
    left: &semantics::SemanticTime,
    right: &semantics::SemanticTime,
) -> bool {
    match (&left.timezone, &right.timezone) {
        (None, None) => true,
        (Some(left_tz), Some(right_tz)) => {
            left_tz.offset_hours == right_tz.offset_hours
                && left_tz.offset_minutes == right_tz.offset_minutes
        }
        _ => false,
    }
}

/// Display name for a parser literal's base type, matching the names produced
/// by [`LemmaType::name`] for the corresponding semantic literal. Used in
/// range endpoint validation error messages.
fn parser_value_base_type_display_name(value: &ast::Value) -> &'static str {
    match value {
        ast::Value::Number(_) => "number",
        ast::Value::Text(_) => "text",
        ast::Value::Boolean(_) => "boolean",
        ast::Value::Date(_) => "date",
        ast::Value::Time(_) => "time",
        ast::Value::NumberWithUnit(_, unit) => match unit.as_str() {
            "percent" | "permille" => "ratio",
            _ => "measure",
        },
        ast::Value::Range(_, _) => "range",
    }
}

/// Infer primitive [`ParentType`] from a literal RHS (`data x: 3.14`).
///
/// Returns an error for range literals whose endpoint types are not a
/// supported range element combination (e.g. `1 ... yes`, `"a" ... "b"`).
fn inferred_parent_type_from_literal(value: &ast::Value) -> Result<ParentType, String> {
    let parent_type = match value {
        ast::Value::Number(_) => ParentType::Primitive {
            primitive: PrimitiveKind::Number,
        },
        ast::Value::Text(_) => ParentType::Primitive {
            primitive: PrimitiveKind::Text,
        },
        ast::Value::Boolean(_) => ParentType::Primitive {
            primitive: PrimitiveKind::Boolean,
        },
        ast::Value::Date(_) => ParentType::Primitive {
            primitive: PrimitiveKind::Date,
        },
        ast::Value::Time(_) => ParentType::Primitive {
            primitive: PrimitiveKind::Time,
        },
        ast::Value::NumberWithUnit(_, _) => ParentType::Primitive {
            primitive: PrimitiveKind::Measure,
        },
        ast::Value::Range(left, right) => {
            let primitive = match (left.as_ref(), right.as_ref()) {
                (ast::Value::Number(_), ast::Value::Number(_)) => PrimitiveKind::NumberRange,
                (ast::Value::Date(_), ast::Value::Date(_)) => PrimitiveKind::DateRange,
                (ast::Value::Time(_), ast::Value::Time(_)) => PrimitiveKind::TimeRange,
                (ast::Value::NumberWithUnit(_, u1), ast::Value::NumberWithUnit(_, u2))
                    if u1 == u2 && matches!(u1.as_str(), "percent" | "permille") =>
                {
                    PrimitiveKind::RatioRange
                }
                (ast::Value::NumberWithUnit(_, _), ast::Value::NumberWithUnit(_, _)) => {
                    PrimitiveKind::MeasureRange
                }
                (left_value, right_value) => {
                    return Err(format!(
                        "range endpoints must have the same supported base type, got {} and {}",
                        parser_value_base_type_display_name(left_value),
                        parser_value_base_type_display_name(right_value)
                    ));
                }
            };
            ParentType::Primitive { primitive }
        }
    };
    Ok(parent_type)
}

impl<'a> TypeResolver<'a> {
    pub fn new(context: &'a Context) -> Self {
        TypeResolver {
            data_types: Vec::new(),
            context,
            all_registered_specs: Vec::new(),
            specs_with_failed_registration: Vec::new(),
        }
    }

    pub fn is_registered(&self, spec: &LemmaSpec) -> bool {
        self.all_registered_specs
            .iter()
            .any(|(_, s)| discovery::same_loaded_spec(s, spec))
    }

    /// Record that a type-declaring data row of `spec` could not be registered.
    fn mark_registration_failure(&mut self, spec: &'a LemmaSpec) {
        if !self
            .specs_with_failed_registration
            .iter()
            .any(|s| discovery::same_loaded_spec(s, spec))
        {
            self.specs_with_failed_registration.push(spec);
        }
    }

    /// Whether [`TypeResolver::register_all`] failed to register a
    /// type-declaring data row of `spec`. The corresponding errors were
    /// returned by `register_all`; specs with failed registrations must not
    /// be built.
    pub fn registration_failed(&self, spec: &LemmaSpec) -> bool {
        self.specs_with_failed_registration
            .iter()
            .any(|s| discovery::same_loaded_spec(s, spec))
    }

    /// Register all type-declaring data from a spec.
    pub fn register_all(
        &mut self,
        repository: &Arc<LemmaRepository>,
        spec: &'a LemmaSpec,
    ) -> Vec<Error> {
        if !self
            .all_registered_specs
            .iter()
            .any(|(_, s)| discovery::same_loaded_spec(s, spec))
        {
            self.all_registered_specs
                .push((Arc::clone(repository), spec));
        }

        let mut errors = Vec::new();
        for data in &spec.data {
            match &data.value {
                ParsedDataValue::Definition {
                    base,
                    constraints,
                    value,
                } => {
                    if matches!(
                        (base.as_ref(), constraints.as_ref(), value.as_ref()),
                        (None, None, Some(Value::NumberWithUnit(_, _)),)
                    ) {
                        continue;
                    }
                    let name = &data.reference.name;
                    let parent = match (base.as_ref(), value.as_ref()) {
                        (Some(b), _) => b.clone(),
                        (None, Some(v)) => match inferred_parent_type_from_literal(v) {
                            Ok(parent) => parent,
                            Err(message) => {
                                errors.push(Error::validation_with_context(
                                    message,
                                    Some(data.source_location.clone()),
                                    None::<String>,
                                    Some(spec),
                                    None,
                                ));
                                self.mark_registration_failure(spec);
                                continue;
                            }
                        },
                        (None, None) => {
                            errors.push(Error::validation_with_context(
                                format!(
                                    "Data '{name}' in spec '{}' must declare a type or a literal value",
                                    spec.name
                                ),
                                Some(data.source_location.clone()),
                                None::<String>,
                                Some(spec),
                                None,
                            ));
                            self.mark_registration_failure(spec);
                            continue;
                        }
                    };
                    let ftd = DataTypeDef {
                        parent,
                        constraints: constraints.clone(),
                        source: data.source_location.clone(),
                        name: name.clone(),
                        bound_literal: value.clone(),
                    };
                    if let Err(e) = self.register_type(spec, ftd) {
                        errors.push(e);
                    }
                }
                ParsedDataValue::Import { .. } => {}
            }
        }
        errors
    }

    /// Register a type from a data declaration.
    pub fn register_type(&mut self, spec: &'a LemmaSpec, def: DataTypeDef) -> Result<(), Error> {
        let spec_types = if let Some(pos) = self
            .data_types
            .iter()
            .position(|(s, _)| discovery::same_loaded_spec(s, spec))
        {
            &mut self.data_types[pos].1
        } else {
            self.data_types.push((spec, HashMap::new()));
            let last = self.data_types.len() - 1;
            &mut self.data_types[last].1
        };
        if spec_types.contains_key(&def.name) {
            return Err(Error::validation_with_context(
                format!(
                    "The name '{}' is already used for data in this spec.",
                    def.name
                ),
                Some(def.source.clone()),
                None::<String>,
                Some(spec),
                None,
            ));
        }
        spec_types.insert(def.name.clone(), def);
        Ok(())
    }

    /// Resolve types for a single spec and validate their specifications.
    /// `at` is the planning instant for this spec (nested qualified refs use their pin).
    pub fn resolve_and_validate(
        &self,
        spec: &LemmaSpec,
        at: &EffectiveDate,
        already_resolved: &ResolvedTypesMap,
    ) -> Result<ResolvedSpecTypes, Vec<Error>> {
        let mut resolved_types = self.resolve_types_internal(spec, at, already_resolved)?;
        resolved_types.source_defaults = resolved_types
            .raw_suggestions
            .iter()
            .map(|(name, raw)| (name.clone(), raw.clone()))
            .collect();
        resolved_types.source_fill_defaults = resolved_types
            .raw_fills
            .iter()
            .map(|(name, raw)| (name.clone(), raw.clone()))
            .collect();
        let mut errors = Vec::new();

        // Build the type-name → source map for precise error reporting.
        let type_sources: std::collections::HashMap<String, Source> = resolved_types
            .resolved
            .keys()
            .filter_map(|type_name| {
                self.data_types
                    .iter()
                    .find(|(s, _)| discovery::same_loaded_spec(s, spec))
                    .and_then(|(_, defs)| defs.get(type_name.as_str()))
                    .map(|ftd| (type_name.clone(), ftd.source.clone()))
            })
            .collect();

        // Run the decomposition pass to populate `BaseMeasureVector` on all Measure types.
        // The pass also syncs `unit_index` with the post-decomp types as its final phase.
        let (new_resolved, new_unit_index, decomp_errors) = resolve_measure_decompositions(
            &spec.name,
            std::mem::take(&mut resolved_types.resolved),
            std::mem::take(&mut resolved_types.unit_index),
            &type_sources,
        );
        resolved_types.resolved = new_resolved;
        resolved_types.unit_index = new_unit_index;
        errors.extend(decomp_errors);

        for (type_name, raw) in std::mem::take(&mut resolved_types.raw_suggestions) {
            let lemma_type = resolved_types
                .resolved
                .get(&type_name)
                .expect("BUG: raw default for type not in resolved");
            match value_kind_from_raw_suggestion(raw, &lemma_type.specifications, &type_name) {
                Ok(value_kind) => {
                    resolved_types
                        .declared_suggestions
                        .insert(type_name, value_kind);
                }
                Err(message) => {
                    let source = type_sources
                        .get(&type_name)
                        .cloned()
                        .unwrap_or_else(|| unreachable!("BUG: type '{}' has no source", type_name));
                    errors.push(Error::validation_with_context(
                        message,
                        Some(source),
                        None::<String>,
                        Some(spec),
                        None,
                    ));
                }
            }
        }

        for (type_name, raw) in std::mem::take(&mut resolved_types.raw_fills) {
            let lemma_type = resolved_types
                .resolved
                .get(&type_name)
                .expect("BUG: raw fill for type not in resolved");
            match value_kind_from_raw_suggestion(raw, &lemma_type.specifications, &type_name) {
                Ok(value_kind) => {
                    resolved_types.declared_fills.insert(type_name, value_kind);
                }
                Err(message) => {
                    let source = type_sources
                        .get(&type_name)
                        .cloned()
                        .unwrap_or_else(|| unreachable!("BUG: type '{}' has no source", type_name));
                    errors.push(Error::validation_with_context(
                        message,
                        Some(source),
                        None::<String>,
                        Some(spec),
                        None,
                    ));
                }
            }
        }

        for (type_name, raw) in std::mem::take(&mut resolved_types.raw_fills) {
            let lemma_type = resolved_types
                .resolved
                .get(&type_name)
                .expect("BUG: raw fill for type not in resolved");
            match value_kind_from_raw_suggestion(raw, &lemma_type.specifications, &type_name) {
                Ok(value_kind) => {
                    resolved_types.declared_fills.insert(type_name, value_kind);
                }
                Err(message) => {
                    let source = type_sources
                        .get(&type_name)
                        .cloned()
                        .unwrap_or_else(|| unreachable!("BUG: type '{}' has no source", type_name));
                    errors.push(Error::validation_with_context(
                        message,
                        Some(source),
                        None::<String>,
                        Some(spec),
                        None,
                    ));
                }
            }
        }

        if let Some((_, data_defs)) = self
            .data_types
            .iter()
            .find(|(s, _)| discovery::same_loaded_spec(s, spec))
        {
            errors.extend(refresh_named_range_specs(
                self,
                spec,
                data_defs,
                &mut resolved_types.resolved,
                &mut resolved_types.declared_suggestions,
                already_resolved,
                at,
            ));
            errors.extend(apply_deferred_named_range_constraints(
                spec,
                data_defs,
                &mut resolved_types.resolved,
                &mut resolved_types.declared_suggestions,
                &mut resolved_types.declared_fills,
                &type_sources,
            ));
        }

        let (new_resolved, resolved_errors) = finalize_measure_magnitudes_in_resolved(
            std::mem::take(&mut resolved_types.resolved),
            &resolved_types.declared_suggestions,
            &type_sources,
            &spec.name,
            spec,
        );
        resolved_types.resolved = new_resolved;

        resolved_types.unit_index = sync_unit_index_from_resolved(
            &resolved_types.resolved,
            std::mem::take(&mut resolved_types.unit_index),
        );

        let (new_unit_index, unit_index_errors) = finalize_measure_magnitudes_in_unit_index(
            std::mem::take(&mut resolved_types.unit_index),
            &resolved_types.declared_suggestions,
            &type_sources,
            &self.data_types,
            spec,
        );
        resolved_types.unit_index = new_unit_index;

        match build_signature_index(&spec.name, &resolved_types.unit_index) {
            Ok(index) => resolved_types.signature_index = index,
            Err(error) => errors.push(error),
        }

        if !resolved_errors.is_empty() || !unit_index_errors.is_empty() {
            errors.extend(resolved_errors);
            errors.extend(unit_index_errors);
            return Err(errors);
        }

        let mut validated_in_unit_index: HashSet<String> = HashSet::new();
        for lemma_type in resolved_types.unit_index.values() {
            let Some(measure_family) = lemma_type.measure_family_name() else {
                continue;
            };
            if !lemma_type.is_measure()
                || !validated_in_unit_index.insert(measure_family.to_string())
            {
                continue;
            }
            let type_name = lemma_type
                .name
                .as_deref()
                .filter(|name| *name == measure_family)
                .unwrap_or(measure_family);
            let source = type_sources
                .get(type_name)
                .cloned()
                .or_else(|| {
                    self.data_types
                        .iter()
                        .find_map(|(_, defs)| defs.get(type_name).map(|def| def.source.clone()))
                })
                .unwrap_or_else(|| {
                    unreachable!(
                        "BUG: measure type '{}' in unit_index has no DataTypeDef source",
                        type_name
                    )
                });
            errors.extend(validate_type_specifications(
                &lemma_type.specifications,
                resolved_types.declared_suggestions.get(type_name),
                "suggestion",
                type_name,
                &source,
                Some(spec),
                &resolved_types.unit_index,
            ));
            errors.extend(validate_type_specifications(
                &lemma_type.specifications,
                resolved_types.declared_fills.get(type_name),
                "fill",
                type_name,
                &source,
                Some(spec),
                &resolved_types.unit_index,
            ));
        }

        for (type_name, lemma_type) in &resolved_types.resolved {
            let source = type_sources.get(type_name).cloned().unwrap_or_else(|| {
                unreachable!(
                    "BUG: resolved type '{}' has no corresponding DataTypeDef in spec '{}'",
                    type_name, spec.name
                )
            });
            let mut spec_errors = validate_type_specifications(
                &lemma_type.specifications,
                resolved_types.declared_suggestions.get(type_name),
                "suggestion",
                type_name,
                &source,
                Some(spec),
                &resolved_types.unit_index,
            );
            errors.append(&mut spec_errors);
            let mut fill_errors = validate_type_specifications(
                &lemma_type.specifications,
                resolved_types.declared_fills.get(type_name),
                "fill",
                type_name,
                &source,
                Some(spec),
                &resolved_types.unit_index,
            );
            errors.append(&mut fill_errors);
        }

        for (type_name, lemma_type) in &resolved_types.resolved {
            let is_range = matches!(
                &lemma_type.specifications,
                TypeSpecification::NumberRange { .. }
                    | TypeSpecification::DateRange { .. }
                    | TypeSpecification::TimeRange { .. }
                    | TypeSpecification::MeasureRange { .. }
                    | TypeSpecification::RatioRange { .. }
            );
            if !is_range {
                continue;
            }
            let source = type_sources.get(type_name).cloned().unwrap_or_else(|| {
                unreachable!(
                    "BUG: resolved type '{}' has no corresponding DataTypeDef in spec '{}'",
                    type_name, spec.name
                )
            });
            if let Some(default_kind) = resolved_types.declared_suggestions.get(type_name.as_str())
            {
                let lit = TypedLiteral {
                    value: default_kind.clone(),
                    lemma_type: Arc::clone(lemma_type),
                };
                if let Err(message) = crate::planning::execution_plan::validate_value_against_type(
                    lemma_type.as_ref(),
                    &lit.to_literal(),
                    &resolved_types.unit_index,
                ) {
                    errors.push(Error::validation_with_context(
                        format!("Type '{}' suggestion is invalid: {}", type_name, message),
                        Some(source.clone()),
                        None::<String>,
                        Some(spec),
                        None,
                    ));
                }
            }
            if let Some(default_kind) = resolved_types.declared_fills.get(type_name.as_str()) {
                let lit = TypedLiteral {
                    value: default_kind.clone(),
                    lemma_type: Arc::clone(lemma_type),
                };
                if let Err(message) = crate::planning::execution_plan::validate_value_against_type(
                    lemma_type.as_ref(),
                    &lit.to_literal(),
                    &resolved_types.unit_index,
                ) {
                    errors.push(Error::validation_with_context(
                        format!("Type '{}' fill is invalid: {}", type_name, message),
                        Some(source),
                        None::<String>,
                        Some(spec),
                        None,
                    ));
                }
            }
        }

        if errors.is_empty() {
            Ok(resolved_types)
        } else {
            Err(errors)
        }
    }

    // =========================================================================
    // Private resolution methods
    // =========================================================================

    fn resolve_types_internal(
        &self,
        spec: &LemmaSpec,
        at: &EffectiveDate,
        already_resolved: &ResolvedTypesMap,
    ) -> Result<ResolvedSpecTypes, Vec<Error>> {
        let data_defs = self
            .data_types
            .iter()
            .find(|(s, _)| discovery::same_loaded_spec(s, spec))
            .map(|(_, defs)| defs)
            .cloned()
            .unwrap_or_default();

        let mut type_names: Vec<String> = data_defs.keys().cloned().collect();
        type_names.sort();
        let sorted_type_names = if type_names.is_empty() {
            type_names
        } else {
            let type_index: HashMap<&str, usize> = type_names
                .iter()
                .enumerate()
                .map(|(index, name)| (name.as_str(), index))
                .collect();
            let type_count = type_names.len();
            let mut in_degree = vec![0usize; type_count];
            let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); type_count];
            for (child_index, type_name) in type_names.iter().enumerate() {
                let custom_parent = match &data_defs[type_name].parent {
                    ParentType::Ranged { inner } => match element_parent_type(inner) {
                        ParentType::Custom { name } => Some(name.as_str()),
                        _ => None,
                    },
                    ParentType::Custom { name } => Some(name.as_str()),
                    _ => None,
                };
                let Some(parent_name) = custom_parent else {
                    continue;
                };
                let Some(&parent_index) = type_index.get(parent_name) else {
                    continue;
                };
                if parent_index == child_index {
                    continue;
                }
                in_degree[child_index] += 1;
                dependents[parent_index].push(child_index);
            }
            let mut queue: std::collections::VecDeque<usize> = (0..type_count)
                .filter(|&index| in_degree[index] == 0)
                .collect();
            let mut sorted = Vec::with_capacity(type_count);
            while let Some(index) = queue.pop_front() {
                sorted.push(type_names[index].clone());
                for &dependent_index in &dependents[index] {
                    in_degree[dependent_index] -= 1;
                    if in_degree[dependent_index] == 0 {
                        queue.push_back(dependent_index);
                    }
                }
            }
            if sorted.len() != type_count {
                let cycle_type_name = type_names
                    .iter()
                    .find(|name| in_degree[type_index[name.as_str()]] > 0)
                    .expect("BUG: incomplete topo sort without cycle participant");
                return Err(vec![Error::validation_with_context(
                    format!(
                        "Circular dependency detected in type resolution: {}::{}",
                        spec.name, cycle_type_name
                    ),
                    Some(data_defs[cycle_type_name].source.clone()),
                    None::<String>,
                    Some(spec),
                    None,
                )]);
            }
            sorted
        };

        let mut resolved: HashMap<String, ResolvedTypeEntry> = HashMap::new();

        fn lookup_parent_type(
            resolver: &TypeResolver<'_>,
            spec: &LemmaSpec,
            parent: &ParentType,
            source: &Source,
            at: &EffectiveDate,
            already_resolved: &ResolvedTypesMap,
            resolved: &HashMap<String, ResolvedTypeEntry>,
        ) -> Result<ParentLookupOk, Vec<Error>> {
            match parent {
                ParentType::Ranged { inner } => {
                    let (element_specs, element_default, element_fill, element_type) =
                        lookup_parent_type(
                            resolver,
                            spec,
                            inner,
                            source,
                            at,
                            already_resolved,
                            resolved,
                        )?;
                    let range_spec = element_specs.range_from_element().ok_or_else(|| {
                        vec![Error::validation_with_context(
                            format!(
                                "'{inner}' is not rangeable: only measure, number, ratio, date, and time types support ranges"
                            ),
                            Some(source.clone()),
                            None::<String>,
                            Some(spec),
                            None,
                        )]
                    })?;
                    Ok((range_spec, element_default, element_fill, element_type))
                }
                ParentType::Primitive { primitive: kind } => {
                    let lemma_type = Arc::new(LemmaType::primitive(
                        semantics::type_spec_for_primitive(*kind),
                    ));
                    Ok((
                        lemma_type.as_ref().specifications.clone(),
                        None,
                        None,
                        lemma_type,
                    ))
                }
                ParentType::Custom { name } => {
                    if let Some((parent_type, parent_suggestion, parent_fill)) =
                        resolved.get(name.as_str())
                    {
                        return Ok((
                            parent_type.as_ref().specifications.clone(),
                            parent_suggestion.clone(),
                            parent_fill.clone(),
                            Arc::clone(parent_type),
                        ));
                    }
                    let type_exists = resolver
                        .data_types
                        .iter()
                        .find(|(s, _)| discovery::same_loaded_spec(s, spec))
                        .map(|(_, type_map)| type_map.contains_key(name.as_str()))
                        .unwrap_or(false);
                    if !type_exists {
                        if spec.data.iter().any(|data| {
                            data.reference.is_local()
                                && data.reference.name == name.as_str()
                                && matches!(&data.value, ParsedDataValue::Import { .. })
                        }) {
                            return Err(vec![Error::validation_with_context(
                                format!(
                                    "'{name}' names a spec import alias, not a type. Use `data x: {name}.TypeName` or `data x: TypeName` (if unambiguous) after `uses`"
                                ),
                                Some(source.clone()),
                                None::<String>,
                                Some(spec),
                                None,
                            )]);
                        }
                        if spec.rules.iter().any(|rule| rule.name == name.as_str()) {
                            return Err(vec![Error::validation_with_context(
                                format!(
                                    "Unknown parent '{parent}' for data definition: '{name}' is a local rule, not a type. Use a type name, or a reference expression if you meant the rule's value."
                                ),
                                Some(source.clone()),
                                None::<String>,
                                Some(spec),
                                None,
                            )]);
                        }
                        let mut imported_matches: Vec<ImportedTypeMatch<'_>> = Vec::new();
                        for data_row in &spec.data {
                            let ParsedDataValue::Import { spec_ref, .. } = &data_row.value else {
                                continue;
                            };
                            let import_alias = &data_row.reference.name;
                            let (_, imported_spec) = match resolver
                                .resolve_spec_for_import(spec, spec_ref, source, at)
                            {
                                Ok(pair) => pair,
                                Err(_) => continue,
                            };
                            let Some(imported_resolved) = already_resolved
                                .iter()
                                .find(|(_, s, _)| discovery::same_loaded_spec(s, imported_spec))
                                .map(|(_, _, r)| r)
                            else {
                                continue;
                            };
                            if let Some(type_arc) = imported_resolved.resolved.get(name.as_str()) {
                                let suggestion = imported_resolved
                                    .source_defaults
                                    .get(name.as_str())
                                    .cloned();
                                let fill = imported_resolved
                                    .source_fill_defaults
                                    .get(name.as_str())
                                    .cloned();
                                imported_matches.push((
                                    import_alias.as_str(),
                                    Arc::clone(type_arc),
                                    suggestion,
                                    fill,
                                ));
                            }
                        }
                        match imported_matches.len() {
                            1 => {
                                let (_, parent_type, parent_suggestion, parent_fill) =
                                    imported_matches
                                        .into_iter()
                                        .next()
                                        .expect("BUG: len checked");
                                return Ok((
                                    parent_type.specifications.clone(),
                                    parent_suggestion,
                                    parent_fill,
                                    parent_type,
                                ));
                            }
                            n if n > 1 => {
                                let aliases: Vec<&str> =
                                    imported_matches.iter().map(|(a, _, _, _)| *a).collect();
                                return Err(vec![Error::validation_with_context(
                                    format!(
                                        "Ambiguous type '{name}': exported by imports {}. Qualify as `alias.{name}`",
                                        aliases.join(", "),
                                    ),
                                    Some(source.clone()),
                                    None::<String>,
                                    Some(spec),
                                    None,
                                )]);
                            }
                            _ => {}
                        }
                        return Err(vec![Error::validation_with_context(
                            format!(
                                "Unknown parent '{parent}' for data definition. Parent must be defined before use. Valid primitive types are: boolean, measure, number, ratio, text, date, time"
                            ),
                            Some(source.clone()),
                            None::<String>,
                            Some(spec),
                            None,
                        )]);
                    }
                    Err(vec![Error::validation_with_context(
                        format!(
                            "Circular dependency detected in type resolution: {}::{}",
                            spec.name, name
                        ),
                        Some(source.clone()),
                        None::<String>,
                        Some(spec),
                        None,
                    )])
                }
                ParentType::Qualified { spec_alias, inner } => {
                    let spec_ref = ast::SpecRef::same_repository(spec_alias.clone());
                    let (_, target_spec) =
                        match resolver.resolve_spec_for_import(spec, &spec_ref, source, at) {
                            Ok(import_pair) => import_pair,
                            Err(error) => return Err(vec![error]),
                        };
                    match inner.as_ref() {
                        ParentType::Primitive { primitive: kind } => {
                            let lemma_type = Arc::new(LemmaType::primitive(
                                semantics::type_spec_for_primitive(*kind),
                            ));
                            Ok((
                                lemma_type.as_ref().specifications.clone(),
                                None,
                                None,
                                lemma_type,
                            ))
                        }
                        ParentType::Custom { name } => {
                            let Some(resolved_spec_types) = already_resolved
                                .iter()
                                .find(|(_, imported, _)| {
                                    discovery::same_loaded_spec(imported, target_spec)
                                })
                                .map(|(_, _, resolved_spec_types)| resolved_spec_types)
                            else {
                                // The import target exists but failed its own type
                                // resolution (its errors were already collected), so
                                // the consumer cannot resolve this qualified type.
                                return Err(vec![Error::validation_with_context(
                                    format!(
                                        "Cannot resolve type '{name}' from spec '{}' (via import '{spec_alias}'): spec '{}' failed type resolution",
                                        target_spec.name, target_spec.name
                                    ),
                                    Some(source.clone()),
                                    None::<String>,
                                    Some(spec),
                                    None,
                                )]);
                            };
                            let Some(parent_type) = resolved_spec_types.resolved.get(name.as_str())
                            else {
                                let type_exists = resolver
                                    .data_types
                                    .iter()
                                    .find(|(s, _)| discovery::same_loaded_spec(s, target_spec))
                                    .map(|(_, type_map)| type_map.contains_key(name.as_str()))
                                    .unwrap_or(false);
                                if !type_exists {
                                    return Err(vec![Error::validation_with_context(
                                        format!(
                                            "Type '{name}' is not defined in spec '{}' (via import '{spec_alias}')",
                                            target_spec.name
                                        ),
                                        Some(source.clone()),
                                        None::<String>,
                                        Some(spec),
                                        None,
                                    )]);
                                }
                                return Err(vec![Error::validation_with_context(
                                    format!(
                                        "Circular dependency detected in type resolution: {}::{}",
                                        target_spec.name, name
                                    ),
                                    Some(source.clone()),
                                    None::<String>,
                                    Some(spec),
                                    None,
                                )]);
                            };
                            Ok((
                                parent_type.as_ref().specifications.clone(),
                                resolved_spec_types
                                    .source_defaults
                                    .get(name.as_str())
                                    .cloned(),
                                resolved_spec_types
                                    .source_fill_defaults
                                    .get(name.as_str())
                                    .cloned(),
                                Arc::clone(parent_type),
                            ))
                        }
                        ParentType::Qualified { .. } => Err(vec![Error::validation_with_context(
                            "Nested qualified parent types are invalid",
                            Some(source.clone()),
                            None::<String>,
                            Some(spec),
                            None,
                        )]),
                        ParentType::Ranged { .. } => Err(vec![Error::validation_with_context(
                            "Nested ranged parent types are invalid",
                            Some(source.clone()),
                            None::<String>,
                            Some(spec),
                            None,
                        )]),
                    }
                }
            }
        }

        for type_name in &sorted_type_names {
            let ftd = data_defs
                .get(type_name.as_str())
                .expect("BUG: topo-sorted type missing from registry");
            let parent = ftd.parent.clone();
            let constraints = ftd.constraints.clone();

            let (parent_specs, parent_suggestion, parent_fill, parent_type) = lookup_parent_type(
                self,
                spec,
                &parent,
                &ftd.source,
                at,
                already_resolved,
                &resolved,
            )?;

            let mut declared_suggestion = parent_suggestion;
            let mut declared_fill = parent_fill;
            let final_specs = if should_defer_ranged_constraints(&parent) {
                parent_specs
            } else if let Some(constraints) = &constraints {
                apply_constraints_to_spec(
                    spec,
                    &constraint_application_type_name(&parent, type_name),
                    parent_specs,
                    constraints,
                    &ftd.source,
                    &mut declared_suggestion,
                    &mut declared_fill,
                )?
            } else {
                parent_specs
            };

            let is_import =
                if let ParentType::Qualified { spec_alias, .. } = element_parent_type(&parent) {
                    let spec_ref = ast::SpecRef::same_repository(spec_alias.clone());
                    self.resolve_spec_for_import(spec, &spec_ref, &ftd.source, at)
                        .map_err(|error| vec![error])?;
                    true
                } else {
                    false
                };

            let family = match element_parent_type(&parent) {
                ParentType::Primitive { .. } => type_name.clone(),
                _ => parent_type
                    .measure_family_name()
                    .map(String::from)
                    .unwrap_or_else(|| type_name.clone()),
            };

            let extends = TypeExtends::Custom {
                parent: parent.to_string(),
                family,
                defining_spec: if is_import {
                    TypeDefiningSpec::Import
                } else {
                    TypeDefiningSpec::Local
                },
            };

            let declared_suggestion = match &ftd.bound_literal {
                Some(literal) => match semantics::parser_value_to_value_kind(literal, &final_specs)
                {
                    Ok(value_kind) => Some(RawSuggestion::Value(value_kind)),
                    Err(message) => {
                        return Err(vec![Error::validation_with_context(
                            message,
                            Some(ftd.source.clone()),
                            None::<String>,
                            Some(spec),
                            None,
                        )]);
                    }
                },
                None => declared_suggestion,
            };

            let lemma_type = Arc::new(LemmaType {
                name: Some(type_name.clone()),
                specifications: final_specs,
                extends,
                measure_binding_unit: None,
            });
            resolved.insert(
                type_name.clone(),
                (lemma_type, declared_suggestion, declared_fill),
            );
        }

        let mut unit_index = UnitIndex::new();
        let mut errors = Vec::new();
        let prim_ratio = semantics::primitive_ratio_arc().clone();
        for unit in Self::extract_units_from_type(&prim_ratio.as_ref().specifications) {
            unit_index.insert_owner(
                unit,
                UnitOwner {
                    owning_type: Arc::clone(&prim_ratio),
                    type_name: prim_ratio.name(),
                    import_alias: None,
                },
            );
        }

        for type_name in &sorted_type_names {
            let (type_arc, _, _) = resolved
                .get(type_name)
                .expect("BUG: type was resolved but missing from map");
            let data_type_def = data_defs
                .get(type_name.as_str())
                .expect("BUG: type was resolved but not in registry");
            let merge_result = if type_arc.is_measure() {
                if let ParentType::Qualified { spec_alias, inner } = &data_type_def.parent {
                    match Self::parent_units_for_qualified_extension(
                        self,
                        spec,
                        data_type_def,
                        already_resolved,
                        at,
                        spec_alias,
                        inner,
                    ) {
                        Ok(None) => Ok(()),
                        Ok(Some(parent_units)) => Self::merge_additive_measure_units(
                            spec,
                            &mut unit_index,
                            type_arc,
                            data_type_def,
                            None,
                            &parent_units,
                        ),
                        Err(error) => Err(error),
                    }
                } else {
                    Self::add_measure_units_to_index(
                        spec,
                        &mut unit_index,
                        type_arc,
                        data_type_def,
                        None,
                    )
                }
            } else if type_arc.is_ratio() {
                Self::add_ratio_units_to_index(spec, &mut unit_index, type_arc, data_type_def, None)
            } else {
                Ok(())
            };
            if let Err(error) = merge_result {
                errors.push(error);
            }
        }

        for data_row in &spec.data {
            let ParsedDataValue::Import { spec_ref, .. } = &data_row.value else {
                continue;
            };
            let import_alias = data_row.reference.name.clone();
            let (_, imported_spec) =
                match self.resolve_spec_for_import(spec, spec_ref, &data_row.source_location, at) {
                    Ok(import_pair) => import_pair,
                    Err(error) => {
                        errors.push(error);
                        continue;
                    }
                };
            let Some(imported_resolved) = already_resolved
                .iter()
                .find(|(_, imported, _)| discovery::same_loaded_spec(imported, imported_spec))
                .map(|(_, _, resolved_spec_types)| resolved_spec_types)
            else {
                continue;
            };
            let Some(imported_defs) = self
                .data_types
                .iter()
                .find(|(s, _)| discovery::same_loaded_spec(s, imported_spec))
                .map(|(_, defs)| defs)
            else {
                continue;
            };
            for (imported_type_name, type_arc) in &imported_resolved.resolved {
                let Some(def) = imported_defs.get(imported_type_name.as_str()) else {
                    continue;
                };
                if matches!(def.parent, ParentType::Qualified { .. }) {
                    continue;
                }
                let merge_result = if type_arc.is_measure() {
                    let consumer_owns_type_locally = data_defs
                        .get(imported_type_name.as_str())
                        .is_some_and(|local_def| {
                            !matches!(local_def.parent, ParentType::Qualified { .. })
                        });
                    if consumer_owns_type_locally {
                        Ok(())
                    } else {
                        Self::add_measure_units_to_index(
                            spec,
                            &mut unit_index,
                            type_arc,
                            def,
                            Some(import_alias.clone()),
                        )
                    }
                } else if type_arc.is_ratio() {
                    Self::add_ratio_units_to_index(
                        spec,
                        &mut unit_index,
                        type_arc,
                        def,
                        Some(import_alias.clone()),
                    )
                } else {
                    Ok(())
                };
                if let Err(error) = merge_result {
                    errors.push(error);
                }
            }
        }

        if !errors.is_empty() {
            return Err(errors);
        }

        let mut raw_suggestions = Vec::new();
        let mut raw_fills = Vec::new();
        let mut resolved_types = IndexMap::new();
        for type_name in &sorted_type_names {
            let (lemma_type, default, fill) = resolved
                .remove(type_name)
                .expect("BUG: every sorted type name must exist in resolved");
            if let Some(raw_suggestion) = default {
                raw_suggestions.push((type_name.clone(), raw_suggestion));
            }
            if let Some(raw_fill) = fill {
                raw_fills.push((type_name.clone(), raw_fill));
            }
            resolved_types.insert(type_name.clone(), lemma_type);
        }
        if !resolved.is_empty() {
            panic!(
                "BUG: resolved types remain after draining sorted_type_names: {:?}",
                resolved.keys().collect::<Vec<_>>()
            );
        }

        Ok(ResolvedSpecTypes {
            resolved: resolved_types,
            declared_suggestions: IndexMap::new(),
            declared_fills: IndexMap::new(),
            raw_suggestions,
            raw_fills,
            source_defaults: IndexMap::new(),
            source_fill_defaults: IndexMap::new(),
            unit_index,
            signature_index: crate::computation::arithmetic::SignatureIndex::new(),
        })
    }

    fn resolve_spec_for_import(
        &self,
        spec: &LemmaSpec,
        from: &crate::parsing::ast::SpecRef,
        import_site: &crate::parsing::source::Source,
        at: &EffectiveDate,
    ) -> Result<(Arc<LemmaRepository>, &'a LemmaSpec), Error> {
        let consumer_repository = self
            .all_registered_specs
            .iter()
            .find(|(_, s)| discovery::same_loaded_spec(s, spec))
            .map(|(r, _)| Arc::clone(r))
            .unwrap_or_else(|| self.context.workspace());
        discovery::resolve_spec_ref(
            self.context,
            from,
            &consumer_repository,
            spec,
            at,
            Some(import_site.clone()),
        )
    }

    // =========================================================================
    // Static helpers (no &self)
    // =========================================================================

    fn add_measure_units_to_index(
        spec: &LemmaSpec,
        unit_index: &mut UnitIndex,
        resolved_type: &Arc<LemmaType>,
        defined_by: &DataTypeDef,
        import_alias: Option<String>,
    ) -> Result<(), Error> {
        if matches!(defined_by.parent, ParentType::Qualified { .. }) {
            unreachable!(
                "BUG: Qualified parents must use parent_units_for_qualified_extension + merge_additive_measure_units"
            );
        }
        let measure_family = resolved_type
            .measure_family_name()
            .expect("BUG: add_measure_units_to_index requires measure type with family");
        for unit in Self::extract_units_from_type(&resolved_type.specifications) {
            unit_index
                .merge_measure_unit(
                    unit,
                    resolved_type,
                    &defined_by.name,
                    import_alias.clone(),
                    measure_family,
                )
                .map_err(|conflict| {
                    Self::unit_merge_conflict_to_error(conflict, defined_by, spec)
                })?;
        }
        Ok(())
    }

    /// Parent unit names for a Qualified extension; `None` if parent not resolved yet.
    fn parent_units_for_qualified_extension(
        resolver: &TypeResolver<'_>,
        spec: &LemmaSpec,
        defined_by: &DataTypeDef,
        already_resolved: &ResolvedTypesMap<'_>,
        at: &EffectiveDate,
        spec_alias: &str,
        inner: &ParentType,
    ) -> Result<Option<BTreeSet<String>>, Error> {
        let ParentType::Custom {
            name: parent_type_name,
        } = inner
        else {
            return Ok(None);
        };
        let spec_ref = ast::SpecRef::same_repository(spec_alias.to_string());
        let (_, target_spec) =
            resolver.resolve_spec_for_import(spec, &spec_ref, &defined_by.source, at)?;
        let Some(imported_resolved) = already_resolved
            .iter()
            .find(|(_, imported, _)| discovery::same_loaded_spec(imported, target_spec))
            .map(|(_, _, types)| types)
        else {
            return Ok(None);
        };
        let Some(parent_type) = imported_resolved.resolved.get(parent_type_name.as_str()) else {
            return Ok(None);
        };
        Ok(Some(
            Self::extract_units_from_type(&parent_type.specifications)
                .into_iter()
                .collect(),
        ))
    }

    /// Register only units the Qualified extension newly declares (not inherited from parent).
    fn merge_additive_measure_units(
        spec: &LemmaSpec,
        unit_index: &mut UnitIndex,
        resolved_type: &Arc<LemmaType>,
        defined_by: &DataTypeDef,
        import_alias: Option<String>,
        parent_units: &BTreeSet<String>,
    ) -> Result<(), Error> {
        let measure_family = resolved_type
            .measure_family_name()
            .expect("BUG: additive measure registration requires family");
        for unit in Self::extract_units_from_type(&resolved_type.specifications) {
            if parent_units.contains(&unit) {
                continue;
            }
            unit_index
                .merge_measure_unit(
                    unit,
                    resolved_type,
                    &defined_by.name,
                    import_alias.clone(),
                    measure_family,
                )
                .map_err(|conflict| {
                    Self::unit_merge_conflict_to_error(conflict, defined_by, spec)
                })?;
        }
        Ok(())
    }

    fn add_ratio_units_to_index(
        spec: &LemmaSpec,
        unit_index: &mut UnitIndex,
        resolved_type: &Arc<LemmaType>,
        defined_by: &DataTypeDef,
        import_alias: Option<String>,
    ) -> Result<(), Error> {
        let primitive_ratio = semantics::primitive_ratio_arc();
        for unit in Self::extract_units_from_type(&resolved_type.specifications) {
            unit_index
                .merge_ratio_unit(
                    unit,
                    resolved_type,
                    &defined_by.name,
                    import_alias.clone(),
                    primitive_ratio,
                )
                .map_err(|conflict| {
                    Self::unit_merge_conflict_to_error(conflict, defined_by, spec)
                })?;
        }
        Ok(())
    }

    fn unit_merge_conflict_to_error(
        conflict: UnitMergeConflict,
        defined_by: &DataTypeDef,
        spec: &LemmaSpec,
    ) -> Error {
        let message = match conflict {
            UnitMergeConflict::Ambiguous {
                unit,
                existing_name,
                new_name,
            } => format!(
                "Unit '{unit}' is defined on both '{existing_name}' and '{new_name}'. \
                 Each unit name may be declared only once."
            ),
            UnitMergeConflict::ConflictingFactors { unit, family } => format!(
                "Unit '{unit}' has different values on related measure types in '{family}'. \
                 Use the same unit value on each type, or use different unit names."
            ),
            UnitMergeConflict::AmbiguousRatio {
                unit,
                existing_name,
                new_name,
            } => format!(
                "Unit '{unit}' is defined on both '{existing_name}' and '{new_name}'. \
                 Each unit name may be declared only once."
            ),
        };
        Error::validation_with_context(
            message,
            Some(defined_by.source.clone()),
            None::<String>,
            Some(spec),
            None,
        )
    }

    fn extract_units_from_type(specs: &TypeSpecification) -> Vec<String> {
        match specs {
            TypeSpecification::Measure { units, .. } => {
                units.iter().map(|unit| unit.name.clone()).collect()
            }
            TypeSpecification::Ratio { units, .. } => {
                units.iter().map(|unit| unit.name.clone()).collect()
            }
            _ => Vec::new(),
        }
    }
}

// ============================================================================
// Validation (formerly validation.rs)
// ============================================================================

/// Validate that TypeSpecification constraints are internally consistent.
///
/// Checks range, decimals, length, unit, and option constraints, and
/// validates the declared default (when present) against those constraints.
/// The default lives outside the type specification (on the data binding or
/// typedef entry); callers thread it in explicitly so this function can verify
/// consistency without owning the value. Pass `default_label` as `"suggestion"`
/// or `"fill"` for error messages.
///
/// Returns a vector of errors (empty if valid).
pub fn validate_type_specifications(
    specs: &TypeSpecification,
    declared_default: Option<&ValueKind>,
    default_label: &str,
    type_name: &str,
    source: &Source,
    spec_context: Option<&LemmaSpec>,
    unit_index: &UnitIndex,
) -> Vec<Error> {
    let mut errors = Vec::new();

    match specs {
        TypeSpecification::Measure {
            minimum,
            maximum,
            decimals,
            units,
            ..
        } => {
            // Validate range consistency
            if let (Some(min), Some(max)) = (minimum, maximum) {
                match (
                    semantics::measure_declared_bound_to_canonical(
                        &min.0,
                        &min.1,
                        units,
                        type_name,
                        "minimum",
                    ),
                    semantics::measure_declared_bound_to_canonical(
                        &max.0,
                        &max.1,
                        units,
                        type_name,
                        "maximum",
                    ),
                ) {
                    (Ok(min_canonical), Ok(max_canonical)) => {
                        match min_canonical.try_cmp(&max_canonical) {
                            Ok(std::cmp::Ordering::Greater) => {
                                errors.push(Error::validation_with_context(
                                    format!(
                                        "Type '{}' has invalid range: minimum {} {} is greater than maximum {} {}",
                                        type_name,
                                        min.0,
                                        min.1,
                                        max.0,
                                        max.1
                                    ),
                                    Some(source.clone()),
                                    None::<String>,
                                    spec_context,
                                    None,
                                ));
                            }
                            Ok(std::cmp::Ordering::Less | std::cmp::Ordering::Equal) => {}
                            Err(failure) => {
                                errors.push(Error::validation_with_context(
                                    format!(
                                        "Type '{}' has invalid measure bound: range compare failed: {failure}",
                                        type_name
                                    ),
                                    Some(source.clone()),
                                    None::<String>,
                                    spec_context,
                                    None,
                                ));
                            }
                        }
                    }
                    (Err(message), _) | (_, Err(message)) => {
                        errors.push(Error::validation_with_context(
                            format!(
                                "Type '{}' has invalid measure bound: {}",
                                type_name, message
                            ),
                            Some(source.clone()),
                            None::<String>,
                            spec_context,
                            None,
                        ));
                    }
                }
            }

            // Validate decimals range (0-28 is rust_decimal limit)
            if let Some(d) = decimals {
                if *d > 28 {
                    errors.push(Error::validation_with_context(
                        format!(
                            "Type '{}' has invalid decimals value: {}. Must be between 0 and 28",
                            type_name, d
                        ),
                        Some(source.clone()),
                        None::<String>,
                        spec_context,
                        None,
                    ));
                }
            }

            // Measure defaults are validated during suggestion lift against this type's units.

            // Measure types must have at least one unit (required for parsing and conversion)
            if units.is_empty() {
                errors.push(Error::validation_with_context(
                    format!(
                        "Type '{}' is a measure type but has no units. Measure types must define at least one unit (e.g. -> unit eur: 1).",
                        type_name
                    ),
                    Some(source.clone()),
                    None::<String>,
                    spec_context,
                    None,
                ));
            }

            // Validate units (if present)
            if !units.is_empty() {
                let mut seen_names: Vec<String> = Vec::new();
                for unit in units.iter() {
                    // Validate unit name is not empty
                    if unit.name.trim().is_empty() {
                        errors.push(Error::validation_with_context(
                            format!(
                                "Type '{}' has a unit with empty name. Unit names cannot be empty.",
                                type_name
                            ),
                            Some(source.clone()),
                            None::<String>,
                            spec_context,
                            None,
                        ));
                    }

                    // Validate unit names are unique within the type
                    if seen_names.contains(&unit.name) {
                        errors.push(Error::validation_with_context(
                            format!("Type '{}' has duplicate unit name '{}'. Unit names must be unique within a type.", type_name, unit.name),
                            Some(source.clone()),
                            None::<String>,
                            spec_context,
                            None,
                        ));
                    } else {
                        seen_names.push(unit.name.clone());
                    }

                    if !unit.is_positive_factor() {
                        errors.push(Error::validation_with_context(
                            format!("Type '{}' has unit '{}' with invalid value {}/{}. Unit values must be positive (conversion factor relative to type base).", type_name, unit.name, unit.factor.numer_to_string(), unit.factor.denom_to_string()),
                            Some(source.clone()),
                            None::<String>,
                            spec_context,
                            None,
                        ));
                    }
                }
            }
        }
        TypeSpecification::Number {
            minimum,
            maximum,
            decimals,
            ..
        } => {
            // Validate range consistency
            if let (Some(min), Some(max)) = (minimum, maximum) {
                match min.try_cmp(max) {
                    Ok(std::cmp::Ordering::Greater) => {
                        errors.push(Error::validation_with_context(
                            format!(
                                "Type '{}' has invalid range: minimum {} is greater than maximum {}",
                                type_name, min, max
                            ),
                            Some(source.clone()),
                            None::<String>,
                            spec_context,
                            None,
                        ));
                    }
                    Ok(std::cmp::Ordering::Less | std::cmp::Ordering::Equal) => {}
                    Err(failure) => {
                        errors.push(Error::validation_with_context(
                            format!(
                                "Type '{}' has invalid range: bound compare failed: {failure}",
                                type_name
                            ),
                            Some(source.clone()),
                            None::<String>,
                            spec_context,
                            None,
                        ));
                    }
                }
            }

            // Validate decimals range (0-28 is rust_decimal limit)
            if let Some(d) = decimals {
                if *d > 28 {
                    errors.push(Error::validation_with_context(
                        format!(
                            "Type '{}' has invalid decimals value: {}. Must be between 0 and 28",
                            type_name, d
                        ),
                        Some(source.clone()),
                        None::<String>,
                        spec_context,
                        None,
                    ));
                }
            }

            if let Some(ValueKind::Number(def)) = declared_default {
                if let Some(min) = minimum {
                    match def.try_cmp(min) {
                        Ok(std::cmp::Ordering::Less) => {
                            errors.push(Error::validation_with_context(
                                format!(
                                    "Type '{}' {default_label} value {} is less than minimum {}",
                                    type_name, def, min
                                ),
                                Some(source.clone()),
                                None::<String>,
                                spec_context,
                                None,
                            ));
                        }
                        Ok(std::cmp::Ordering::Equal | std::cmp::Ordering::Greater) => {}
                        Err(failure) => {
                            errors.push(Error::validation_with_context(
                                format!(
                                    "Type '{}' {default_label} bound compare failed: {failure}",
                                    type_name
                                ),
                                Some(source.clone()),
                                None::<String>,
                                spec_context,
                                None,
                            ));
                        }
                    }
                }
                if let Some(max) = maximum {
                    match def.try_cmp(max) {
                        Ok(std::cmp::Ordering::Greater) => {
                            errors.push(Error::validation_with_context(
                                format!(
                                    "Type '{}' {default_label} value {} is greater than maximum {}",
                                    type_name, def, max
                                ),
                                Some(source.clone()),
                                None::<String>,
                                spec_context,
                                None,
                            ));
                        }
                        Ok(std::cmp::Ordering::Less | std::cmp::Ordering::Equal) => {}
                        Err(failure) => {
                            errors.push(Error::validation_with_context(
                                format!(
                                    "Type '{}' {default_label} bound compare failed: {failure}",
                                    type_name
                                ),
                                Some(source.clone()),
                                None::<String>,
                                spec_context,
                                None,
                            ));
                        }
                    }
                }
            }
            // Note: Number types are dimensionless and cannot have units (validated in apply_constraint)
        }

        TypeSpecification::Ratio {
            minimum,
            maximum,
            decimals,
            units,
            ..
        } => {
            // Validate decimals range (0-28 is rust_decimal limit)
            if let Some(d) = decimals {
                if *d > 28 {
                    errors.push(Error::validation_with_context(
                        format!(
                            "Type '{}' has invalid decimals value: {}. Must be between 0 and 28",
                            type_name, d
                        ),
                        Some(source.clone()),
                        None::<String>,
                        spec_context,
                        None,
                    ));
                }
            }

            // Validate range consistency
            if let (Some(min), Some(max)) = (minimum, maximum) {
                match min.try_cmp(max) {
                    Ok(std::cmp::Ordering::Greater) => {
                        errors.push(Error::validation_with_context(
                            format!(
                                "Type '{}' has invalid range: minimum {} is greater than maximum {}",
                                type_name, min, max
                            ),
                            Some(source.clone()),
                            None::<String>,
                            spec_context,
                            None,
                        ));
                    }
                    Ok(std::cmp::Ordering::Less | std::cmp::Ordering::Equal) => {}
                    Err(failure) => {
                        errors.push(Error::validation_with_context(
                            format!(
                                "Type '{}' has invalid range: bound compare failed: {failure}",
                                type_name
                            ),
                            Some(source.clone()),
                            None::<String>,
                            spec_context,
                            None,
                        ));
                    }
                }
            }

            if let Some(ValueKind::Ratio(def)) = declared_default {
                if let Some(min) = minimum {
                    match def.try_cmp(min) {
                        Ok(std::cmp::Ordering::Less) => {
                            errors.push(Error::validation_with_context(
                                format!(
                                    "Type '{}' {default_label} value {} is less than minimum {}",
                                    type_name, def, min
                                ),
                                Some(source.clone()),
                                None::<String>,
                                spec_context,
                                None,
                            ));
                        }
                        Ok(std::cmp::Ordering::Equal | std::cmp::Ordering::Greater) => {}
                        Err(failure) => {
                            errors.push(Error::validation_with_context(
                                format!(
                                    "Type '{}' {default_label} bound compare failed: {failure}",
                                    type_name
                                ),
                                Some(source.clone()),
                                None::<String>,
                                spec_context,
                                None,
                            ));
                        }
                    }
                }
                if let Some(max) = maximum {
                    match def.try_cmp(max) {
                        Ok(std::cmp::Ordering::Greater) => {
                            errors.push(Error::validation_with_context(
                                format!(
                                    "Type '{}' {default_label} value {} is greater than maximum {}",
                                    type_name, def, max
                                ),
                                Some(source.clone()),
                                None::<String>,
                                spec_context,
                                None,
                            ));
                        }
                        Ok(std::cmp::Ordering::Less | std::cmp::Ordering::Equal) => {}
                        Err(failure) => {
                            errors.push(Error::validation_with_context(
                                format!(
                                    "Type '{}' {default_label} bound compare failed: {failure}",
                                    type_name
                                ),
                                Some(source.clone()),
                                None::<String>,
                                spec_context,
                                None,
                            ));
                        }
                    }
                }
            }

            // Validate units (if present)
            // Types can have zero units (e.g., type ratio: number -> ratio) - this is valid
            // Only validate if units are defined
            if !units.is_empty() {
                let mut seen_names: Vec<String> = Vec::new();
                for unit in units.iter() {
                    // Validate unit name is not empty
                    if unit.name.trim().is_empty() {
                        errors.push(Error::validation_with_context(
                            format!(
                                "Type '{}' has a unit with empty name. Unit names cannot be empty.",
                                type_name
                            ),
                            Some(source.clone()),
                            None::<String>,
                            spec_context,
                            None,
                        ));
                    }

                    // Validate unit names are unique within the type
                    if seen_names.contains(&unit.name) {
                        errors.push(Error::validation_with_context(
                            format!("Type '{}' has duplicate unit name '{}'. Unit names must be unique within a type.", type_name, unit.name),
                            Some(source.clone()),
                            None::<String>,
                            spec_context,
                            None,
                        ));
                    } else {
                        seen_names.push(unit.name.clone());
                    }

                    if !unit.value.numer_is_positive() {
                        errors.push(Error::validation_with_context(
                            format!("Type '{}' has unit '{}' with invalid value {}/{}. Unit values must be positive (conversion factor relative to type base).", type_name, unit.name, unit.value.numer_to_string(), unit.value.denom_to_string()),
                            Some(source.clone()),
                            None::<String>,
                            spec_context,
                            None,
                        ));
                    }
                }
            }
        }

        TypeSpecification::Text {
            length, options, ..
        } => {
            if let Some(ValueKind::Text(def)) = declared_default {
                let def_len = def.len();

                if let Some(len) = length {
                    if def_len != *len {
                        errors.push(Error::validation_with_context(
                            format!("Type '{}' {default_label} value length {} does not match required length {}", type_name, def_len, len),
                            Some(source.clone()),
                            None::<String>,
                            spec_context,
                            None,
                        ));
                    }
                }
                if !options.is_empty() && !options.contains(def) {
                    errors.push(Error::validation_with_context(
                        format!(
                            "Type '{}' {default_label} value '{}' is not in allowed options: {:?}",
                            type_name, def, options
                        ),
                        Some(source.clone()),
                        None::<String>,
                        spec_context,
                        None,
                    ));
                }
            }
        }

        TypeSpecification::Date {
            minimum,
            maximum,
            ..
        } => {
            // Validate range consistency
            if let (Some(min), Some(max)) = (minimum, maximum) {
                let min_sem = semantics::date_time_to_semantic(min);
                let max_sem = semantics::date_time_to_semantic(max);
                if semantics::compare_semantic_dates(&min_sem, &max_sem) == Ordering::Greater {
                    errors.push(Error::validation_with_context(
                        format!(
                            "Type '{}' has invalid date range: minimum {} is after maximum {}",
                            type_name, min, max
                        ),
                        Some(source.clone()),
                        None::<String>,
                        spec_context,
                        None,
                    ));
                }
            }

            if let Some(ValueKind::Date(def)) = declared_default {
                if let Some(min) = minimum {
                    let min_sem = semantics::date_time_to_semantic(min);
                    if semantics::compare_semantic_dates(def, &min_sem) == Ordering::Less {
                        errors.push(Error::validation_with_context(
                            format!(
                                "Type '{}' default date {} is before minimum {}",
                                type_name, def, min
                            ),
                            Some(source.clone()),
                            None::<String>,
                            spec_context,
                            None,
                        ));
                    }
                }
                if let Some(max) = maximum {
                    let max_sem = semantics::date_time_to_semantic(max);
                    if semantics::compare_semantic_dates(def, &max_sem) == Ordering::Greater {
                        errors.push(Error::validation_with_context(
                            format!(
                                "Type '{}' default date {} is after maximum {}",
                                type_name, def, max
                            ),
                            Some(source.clone()),
                            None::<String>,
                            spec_context,
                            None,
                        ));
                    }
                }
            }
        }

        TypeSpecification::Time {
            minimum,
            maximum,
            ..
        } => {
            // Validate range consistency
            if let (Some(min), Some(max)) = (minimum, maximum) {
                let min_sem = semantics::time_to_semantic(min);
                let max_sem = semantics::time_to_semantic(max);
                if semantics::compare_semantic_times(&min_sem, &max_sem) == Ordering::Greater {
                    errors.push(Error::validation_with_context(
                        format!(
                            "Type '{}' has invalid time range: minimum {} is after maximum {}",
                            type_name, min, max
                        ),
                        Some(source.clone()),
                        None::<String>,
                        spec_context,
                        None,
                    ));
                }
            }

            if let Some(ValueKind::Time(def)) = declared_default {
                if let Some(min) = minimum {
                    let min_sem = semantics::time_to_semantic(min);
                    if semantics::compare_semantic_times(def, &min_sem) == Ordering::Less {
                        errors.push(Error::validation_with_context(
                            format!(
                                "Type '{}' default time {} is before minimum {}",
                                type_name, def, min
                            ),
                            Some(source.clone()),
                            None::<String>,
                            spec_context,
                            None,
                        ));
                    }
                }
                if let Some(max) = maximum {
                    let max_sem = semantics::time_to_semantic(max);
                    if semantics::compare_semantic_times(def, &max_sem) == Ordering::Greater {
                        errors.push(Error::validation_with_context(
                            format!(
                                "Type '{}' default time {} is after maximum {}",
                                type_name, def, max
                            ),
                            Some(source.clone()),
                            None::<String>,
                            spec_context,
                            None,
                        ));
                    }
                }
            }
        }

        TypeSpecification::NumberRange { .. }
        | TypeSpecification::DateRange { .. }
        | TypeSpecification::TimeRange { .. }
        | TypeSpecification::MeasureRange { .. }
        | TypeSpecification::RatioRange { .. } => {
            if let Err(message) = semantics::check_range_bound_consistency(specs, unit_index) {
                errors.push(Error::validation_with_context(
                    format!("Type '{}' has {message}", type_name),
                    Some(source.clone()),
                    None::<String>,
                    spec_context,
                    None,
                ));
            }
        }
        TypeSpecification::Boolean { .. } => {
            // No constraint validation needed for these types
        }
        TypeSpecification::Veto { .. } => {
            // Veto is not a user-declarable type, so validation should not be called on it
            // But if it is, there's nothing to validate
        }
        TypeSpecification::Undetermined => unreachable!(
            "BUG: validate_type_specification_constraints called with Undetermined sentinel type; this type exists only during type inference"
        ),
    }

    errors
}

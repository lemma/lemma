use crate::computation::rational::{NumericFailure, RationalInteger};
use crate::engine::Context;
use crate::literals::{MeasureUnit, MeasureUnits};
use crate::parsing::arena::{ExprId, ExprKind, ExprRef, ExprTable, ExprTree};
use crate::parsing::ast::{
    self as ast, CommandArg, Constraint, EffectiveDate, LemmaData, LemmaRepository, LemmaRule,
    LemmaSpec, ParentType, PrimitiveKind, RowCondition, TypeConstraintCommand, Value, WithRhs,
};
use crate::parsing::source::Source;
use crate::planning::discovery;
use crate::planning::execution_plan::{self, import_rule_needs, ExecutionPlan, ImportNeed};
use crate::planning::instance_list::{
    self, DataSource, InstanceListSource, InstanceLists, KeyedRow, SourceRow,
};
use crate::planning::semantics::{
    self, bound_value_kind_from_raw_suggestion, calendar_decomposition, canonicalize_signature,
    conversion_target_to_semantic, duration_decomposition, lift_range_endpoint,
    number_with_unit_to_value_kind, parser_value_to_value_kind, primitive_boolean_arc,
    primitive_date_arc, primitive_number_arc, primitive_ratio_arc, primitive_text_arc,
    primitive_time_arc, range_type_specification_from_endpoints, value_kind_matches_spec,
    value_to_semantic, ArithmeticComputation, BaseMeasureVector, BoundValueKind,
    ComparisonComputation, DataDefinition, DataPath, Expression, ExpressionKind, ImportOverlay,
    LemmaType, ListKey, ListOp, ListPredKind, ListReduce, LiteralValue, PathSegment, RawSuggestion,
    ReferenceEnd, ReferenceTarget, RowEnd, RulePath, SemanticConversionTarget, SpecIdentity,
    SpecInstance, SpecMemberEnd, SpecMemberTarget, SpecValue, TypeDefiningSpec, TypeExtends,
    TypeFamily, TypeSpecification, TypedLiteral, UnitTokenResolver, ValueKind,
};
use crate::planning::spec_value::RulePresence;
use crate::planning::typing::{
    comparison_type, date_predicate_type, list_command_result_type, list_has_instance_rows,
    logical_and_type, logical_not_type, math_op_type, measure_range_matches_measure,
    past_future_range_type, piecewise_type, range_bound_result_type, range_matches_measure_type,
    range_matches_range_measure, range_span_type, resolve_anonymous_measure, result_is_veto_type,
    unit_conversion_type, ListCommandResultKind, MeasureScope,
};
use crate::planning::unit_index::{
    PlanningOwner, PlanningUnitIndex, UnitIndex, UnitMergeConflict, UnitOwner,
};
use crate::Error;

pub use crate::planning::typing::DecompositionMatch;
pub(crate) use crate::planning::typing::{
    compute_arithmetic_result_type, find_unique_measure_type_in_unit_index,
    infer_range_type_from_endpoint_types,
};
use ast::DataValue as ParsedDataValue;
use indexmap::IndexMap;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fmt;
use std::sync::Arc;

/// Memo of inferred expression types per expression table (one table per rule).
/// The memo does not include `computed_rule_types` or `resolved_types`, so an entry is valid
/// only while those are unchanged. Every change to either must be followed by `clear()`:
/// after phase 4 merges rule-target constraints into `resolved_types`, and when
/// `recursive_rule_seed` replaces a recursive rule's undetermined placeholder with its seed.
#[derive(Default)]
struct InferExpressionTypeCache {
    /// Types of a table's nodes in table order, keyed by the table's address. A table lists
    /// children before parents, so a typed prefix always covers the operands of its next node.
    tables: HashMap<usize, Vec<Arc<LemmaType>>>,
}

impl InferExpressionTypeCache {
    fn new() -> Self {
        Self::default()
    }

    /// Forgets every type and keeps each table's storage for the next wave.
    fn clear(&mut self) {
        self.tables.values_mut().for_each(Vec::clear);
    }
}

/// Data bindings map: maps a target data name path to the binding's value and source.
///
/// The key is the full path of **data names** from the root spec to the target data.
/// Spec set names are intentionally excluded from the key because spec ref bindings may change
/// which spec a segment points to — matching by data names only ensures bindings
/// are applied correctly regardless of spec ref bindings.
///
/// Example: `data employee.salary: 7500` in the root spec produces key `["employee", "salary"]`.
type DataBindings = IndexMap<Vec<String>, (BindingValue, Source)>;

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

impl execution_plan::ReferenceOwner for Graph<'_> {
    fn owner_data(&self) -> &IndexMap<DataPath, DataDefinition> {
        &self.data
    }

    fn owner_templates(&self) -> &BTreeMap<SpecIdentity, Arc<ExecutionPlan>> {
        &self.instance_lists.templates
    }
}

#[derive(Debug)]
pub(crate) struct Graph<'a> {
    /// Root spec being planned (for error spec_context).
    main_spec: &'a LemmaSpec,
    data: IndexMap<DataPath, DataDefinition>,
    rules: BTreeMap<RulePath, RuleNode<'a>>,
    /// Rules whose expression conversion failed. Each has an error in `graph_errors`.
    dropped_rules: BTreeSet<RulePath>,
    /// Local rules in dependency order (topo).
    rule_order: Vec<RulePath>,
    /// [`Self::rule_order`] with the named-`uses` rules locals reach, each placed
    /// after the owner rules its site overlays bind.
    execution_order: Vec<RulePath>,
    /// Data→data references in dependency order (topo). Targets before dependents
    /// so chained references resolve. Refs to non-reference data are unordered
    /// among themselves.
    data_reference_order: Vec<DataPath>,
    /// Every [`DataDefinition::Reference`] path → where its chain ends.
    /// Filled in [`Self::validate`] after data-reference cycles are rejected.
    reference_ends: IndexMap<DataPath, ReferenceEnd>,
    /// Instances each local rule can yield (empty when not spec-valued), relative to
    /// this spec. Filled by [`Self::link_spec_values`].
    rule_candidates: HashMap<RulePath, Vec<SpecInstance>>,
    instance_lists: InstanceLists,
}

/// Result of [`Graph::lookup_data`]: local graph slot or a peeled import-template field.
pub(crate) enum DataLookup<'p> {
    Local(&'p DataDefinition),
    Import(&'p DataDefinition),
}

impl<'p> DataLookup<'p> {
    pub(crate) fn definition(&self) -> &'p DataDefinition {
        match self {
            DataLookup::Local(definition) | DataLookup::Import(definition) => definition,
        }
    }
}

/// First Import hop on consumer `data` → shared template plan.
pub(crate) fn first_import_template<'p>(
    data: &'p IndexMap<DataPath, DataDefinition>,
    templates: &'p BTreeMap<SpecIdentity, Arc<ExecutionPlan>>,
    first: &PathSegment,
) -> Option<&'p ExecutionPlan> {
    let import_marker = DataPath::local(first.uses.clone());
    let DataDefinition::Import { identity, .. } = data.get(&import_marker)? else {
        return None;
    };
    templates.get(identity).map(Arc::as_ref)
}

/// Local `uses` marker `hop` on `data`, its identity and template plan.
fn root_import_hop<'p>(
    data: &'p IndexMap<DataPath, DataDefinition>,
    templates: &'p BTreeMap<SpecIdentity, Arc<ExecutionPlan>>,
    hop: &str,
) -> Option<(DataPath, &'p SpecIdentity, &'p ExecutionPlan)> {
    let import = DataPath::local(hop.to_string());
    let DataDefinition::Import { identity, .. } = data.get(&import)? else {
        return None;
    };
    let template = templates.get(identity)?;
    Some((import, identity, template))
}

/// Self-list row field `name` (dotted through named `uses`) on the spec being planned,
/// `rules` being its rules so far.
fn root_row_end(
    data: &IndexMap<DataPath, DataDefinition>,
    templates: &BTreeMap<SpecIdentity, Arc<ExecutionPlan>>,
    rules: &[&RulePath],
    name: &str,
) -> Option<RowEnd> {
    let Some((hop, rest)) = name.split_once('.') else {
        return instance_list::row_end_of(instance_list::RowEndView::Root(data, rules), name);
    };
    let (import, identity, template) = root_import_hop(data, templates, hop)?;
    Some(
        match instance_list::row_end_of(instance_list::RowEndView::Plan(template), rest)? {
            RowEnd::Data(path) => {
                RowEnd::Data(absolute_import_member_path(&import, identity, &path))
            }
            RowEnd::Rule(rule) => {
                let absolute = absolute_import_member_path(
                    &import,
                    identity,
                    &DataPath {
                        segments: rule.segments,
                        data: rule.rule,
                    },
                );
                RowEnd::Rule(RulePath {
                    segments: absolute.segments,
                    rule: absolute.data,
                })
            }
        },
    )
}

/// Peel Import hops on a rule path using a data map + template store (no Graph borrow).
pub(crate) fn peel_rule_from<'p>(
    data: &'p IndexMap<DataPath, DataDefinition>,
    templates: &'p BTreeMap<SpecIdentity, Arc<ExecutionPlan>>,
    path: &RulePath,
) -> Option<(&'p ExecutionPlan, &'p RulePath)> {
    let first = path.segments.first()?;
    let template = first_import_template(data, templates, first)?;
    let rest = RulePath {
        segments: path.segments[1..].to_vec(),
        rule: path.rule.clone(),
    };
    template.peel_rule(&rest)
}

/// Consumer-qualified path for a template-relative overlay field under `import_path`.
pub(crate) fn absolute_import_member_path(
    import_path: &DataPath,
    identity: &SpecIdentity,
    relative: &DataPath,
) -> DataPath {
    let mut segments = import_path.segments.clone();
    segments.push(PathSegment {
        uses: import_path.data.clone(),
        repository: identity.repository.clone(),
        spec: identity.spec.clone(),
    });
    segments.extend(relative.segments.iter().cloned());
    DataPath {
        segments,
        data: relative.data.clone(),
    }
}

impl<'a> Graph<'a> {
    pub(crate) fn data(&self) -> &IndexMap<DataPath, DataDefinition> {
        &self.data
    }

    pub(crate) fn rules(&self) -> &BTreeMap<RulePath, RuleNode<'a>> {
        &self.rules
    }

    pub(crate) fn rule_presence(&self, path: &RulePath) -> RulePresence {
        if self.rules.contains_key(path) {
            RulePresence::Present
        } else if self.dropped_rules.contains(path) {
            RulePresence::Dropped
        } else if peel_rule_from(&self.data, &self.instance_lists.templates, path).is_some() {
            // Rule lives on a named `uses` template, not in this graph's rule map.
            RulePresence::Present
        } else if path.segments.first().is_some_and(|hop| {
            matches!(
                self.data.get(&DataPath::local(hop.uses.clone())),
                Some(DataDefinition::Import { .. })
            )
        }) {
            // Import hop exists but template peel failed (nested planning error
            // already recorded). Treat as dropped so topo does not panic.
            RulePresence::Dropped
        } else {
            RulePresence::Absent
        }
    }

    /// Whether `path` is a local rule of this graph (not merely an import-template member).
    pub(crate) fn has_local_rule(&self, path: &RulePath) -> bool {
        self.rules.contains_key(path)
    }

    /// Data at `path` on this graph, or after peeling leading Import hops into templates.
    pub(crate) fn lookup_data(&self, path: &DataPath) -> Option<DataLookup<'_>> {
        if let Some(definition) = self.data.get(path) {
            return Some(DataLookup::Local(definition));
        }
        let (_, _, definition) = self.peel_data(path)?;
        Some(DataLookup::Import(definition))
    }

    /// Peel `alias…` hops through Import markers into nested templates; `path.data` is
    /// the final field on the last template.
    pub(crate) fn peel_data(
        &self,
        path: &DataPath,
    ) -> Option<(&ExecutionPlan, &DataPath, &DataDefinition)> {
        let first = path.segments.first()?;
        let template = first_import_template(&self.data, &self.instance_lists.templates, first)?;
        let rest = DataPath {
            segments: path.segments[1..].to_vec(),
            data: path.data.clone(),
        };
        template.peel_data(&rest)
    }

    pub(crate) fn rules_mut(&mut self) -> &mut BTreeMap<RulePath, RuleNode<'a>> {
        &mut self.rules
    }

    pub(crate) fn rule_order(&self) -> &[RulePath] {
        &self.rule_order
    }

    pub(crate) fn execution_order(&self) -> &[RulePath] {
        &self.execution_order
    }

    pub(crate) fn data_reference_order(&self) -> &[DataPath] {
        &self.data_reference_order
    }

    pub(crate) fn reference_ends(&self) -> &IndexMap<DataPath, ReferenceEnd> {
        &self.reference_ends
    }

    pub(crate) fn rule_candidates(&self) -> &HashMap<RulePath, Vec<SpecInstance>> {
        &self.rule_candidates
    }

    pub(crate) fn main_spec(&self) -> &LemmaSpec {
        self.main_spec
    }

    pub(crate) fn instance_lists(&self) -> &InstanceLists {
        &self.instance_lists
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
            local_suggestion: Option<BoundValueKind>,
            local_value: Option<BoundValueKind>,
        }

        let mut schema: HashMap<DataPath, Arc<LemmaType>> = HashMap::new();
        let mut declared_suggestions: HashMap<DataPath, BoundValueKind> = HashMap::new();
        let mut declared_values: HashMap<DataPath, BoundValueKind> = HashMap::new();
        let mut values: HashMap<DataPath, LiteralValue> = HashMap::new();
        let mut value_sources: HashMap<DataPath, Source> = HashMap::new();
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
                    declared_value,
                    ..
                } => {
                    schema.insert(path.clone(), Arc::clone(resolved_type));
                    if let Some(dv) = declared_suggestion {
                        declared_suggestions.insert(path.clone(), dv.clone());
                    }
                    if let Some(dv) = declared_value {
                        declared_values.insert(path.clone(), dv.clone());
                    }
                }
                DataDefinition::Import { .. } => {}
                DataDefinition::InstanceList { .. } => {}
                DataDefinition::Reference {
                    target,
                    resolved_type,
                    local_constraints,
                    local_suggestion,
                    local_value,
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
                            local_value: local_value.clone(),
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
            if let DataDefinition::InstanceList { .. } | DataDefinition::Import { .. } = rfv {
                data.insert(path.clone(), rfv.clone());
            } else if let Some(pending) = references.remove(path) {
                data.insert(
                    path.clone(),
                    DataDefinition::Reference {
                        target: pending.target,
                        resolved_type: pending.resolved_type,
                        local_constraints: pending.local_constraints,
                        local_suggestion: pending.local_suggestion,
                        local_value: pending.local_value,
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
                let declared_value = declared_values.remove(path);
                data.insert(
                    path.clone(),
                    DataDefinition::TypeDeclaration {
                        resolved_type,
                        declared_suggestion,
                        declared_value,
                        source,
                    },
                );
            }
        }
        Ok(data)
    }

    /// Keep a measure literal's written unit when replacing `lemma_type` with a schema type
    /// that has the same specifications (or compatible family).
    fn preserve_measure_binding(
        schema_type: &Arc<LemmaType>,
        previous: &LemmaType,
    ) -> Arc<LemmaType> {
        match previous.unit() {
            Some(unit)
                if matches!(
                    &schema_type.specifications,
                    TypeSpecification::Measure { .. } | TypeSpecification::Ratio { .. }
                ) =>
            {
                Arc::new(schema_type.as_ref().clone().with_unit(unit))
            }
            Some(_) | None => Arc::clone(schema_type),
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
        match (&schema_ref.specifications, &lit.value) {
            (
                TypeSpecification::NumberRange { .. }
                | TypeSpecification::DateRange { .. }
                | TypeSpecification::TimeRange { .. }
                | TypeSpecification::RatioRange { .. }
                | TypeSpecification::MeasureRange { .. },
                ValueKind::Range(left, right),
            ) if !lit
                .lemma_type
                .specifications
                .equal_ignoring_unit(&schema_ref.specifications) =>
            {
                let endpoint_schema_type =
                    range_endpoint_schema_type(schema_ref).unwrap_or_else(|| {
                        unreachable!("BUG: range_endpoint_schema_type missing range schema arm")
                    });
                let left_schema =
                    Self::preserve_measure_binding(&endpoint_schema_type, left.lemma_type.as_ref());
                let right_schema = Self::preserve_measure_binding(
                    &endpoint_schema_type,
                    right.lemma_type.as_ref(),
                );
                let coerced_left = Self::coerce_scalar_literal(left, &left_schema)?;
                let coerced_right = Self::coerce_scalar_literal(right, &right_schema)?;
                Ok(TypedLiteral {
                    value: ValueKind::Range(Box::new(coerced_left), Box::new(coerced_right)),
                    lemma_type: Arc::clone(schema_type),
                })
            }
            _ => Self::coerce_scalar_literal(lit, schema_type),
        }
    }

    /// [`Self::coerce_literal_to_schema_type`] for a literal that is not a range against a
    /// range schema: a range endpoint is coerced here, never as a range.
    fn coerce_scalar_literal(
        lit: &TypedLiteral,
        schema_type: &Arc<LemmaType>,
    ) -> Result<TypedLiteral, String> {
        let schema_ref = schema_type.as_ref();
        if lit
            .lemma_type
            .specifications
            .equal_ignoring_unit(&schema_ref.specifications)
        {
            if !value_kind_matches_spec(&lit.value, &schema_ref.specifications) {
                panic!(
                    "BUG: LiteralValue value kind {:?} inconsistent with lemma_type {:?}",
                    lit.value, lit.lemma_type.specifications
                );
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
                    && !lit
                        .lemma_type
                        .specifications
                        .equal_ignoring_unit(&schema_ref.specifications)
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
        resolved_types: &ResolvedTypesMap,
    ) -> Result<(), Vec<Error>> {
        let mut errors: Vec<Error> = Vec::new();
        let main_spec = self.main_spec;
        let resolve_unit = unit_tokens_for_spec(resolved_types, main_spec);

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
                | DataDefinition::InstanceList { resolved_type, .. }
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
            let mut raw_value: Option<RawSuggestion> = None;
            if let Some(constraints) = &local_constraints {
                let constraint_type_name = merged.name();
                match apply_constraints_to_spec(
                    self.main_spec,
                    &constraint_type_name,
                    merged.specifications.clone(),
                    constraints,
                    &source,
                    ConstraintSuggestionSink {
                        suggestion: &mut raw_suggestion,
                        value: &mut raw_value,
                        resolve_unit: Some(&resolve_unit),
                    },
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
                    match bound_value_kind_from_raw_suggestion(
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
            let captured_value = match raw_value {
                None => None,
                Some(raw) => {
                    match bound_value_kind_from_raw_suggestion(
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
                local_value,
                ..
            }) = self.data.get_mut(reference_path)
            {
                *resolved_type = Arc::new(merged);
                if captured_suggestion.is_some() {
                    *local_suggestion = captured_suggestion;
                }
                if captured_value.is_some() {
                    *local_value = captured_value;
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

    /// Type-check named-`uses` overlay references against the template field type.
    /// Same kind / measure-family rules as [`Self::resolve_data_reference_types`].
    fn validate_import_overlay_reference_types(&self) -> Result<(), Vec<Error>> {
        let mut errors = Vec::new();
        for (import_path, definition) in &self.data {
            let DataDefinition::Import {
                identity,
                overlay,
                source,
                ..
            } = definition
            else {
                continue;
            };
            if overlay.is_empty() {
                continue;
            }
            let Some(template) = self.instance_lists.templates.get(identity).map(Arc::as_ref)
            else {
                panic!(
                    "BUG: import '{}' has overlay but no template during overlay type check",
                    import_path.input_key()
                );
            };
            for (relative, entry) in overlay {
                // Rule-target overlays: type-checked after rule inference (Phase 4).
                let ImportOverlay::Reference {
                    target: ReferenceTarget::Data(target_path),
                    ..
                } = entry
                else {
                    continue;
                };
                let Some((_, _, field_def)) = template.peel_data(relative) else {
                    panic!(
                        "BUG: overlay path '{}' missing from template during type check",
                        relative.input_key()
                    );
                };
                let Some(lhs) = field_def.resolved_type_arc() else {
                    panic!(
                        "BUG: overlay field '{}' must be typed",
                        relative.input_key()
                    );
                };
                if lhs.is_undetermined() {
                    continue;
                }
                let absolute = absolute_import_member_path(import_path, identity, relative);
                let target_type = self
                    .lookup_data(target_path)
                    .and_then(|lookup| lookup.definition().resolved_type_arc().cloned());
                let Some(target_type) = target_type else {
                    errors.push(reference_error(
                        self.main_spec,
                        source,
                        format!(
                            "Data reference '{}' target '{}' does not exist",
                            absolute, target_path
                        ),
                    ));
                    continue;
                };
                if let Some(msg) = reference_kind_mismatch_message(
                    lhs.as_ref(),
                    target_type.as_ref(),
                    &absolute,
                    target_path,
                    "target",
                ) {
                    errors.push(reference_error(self.main_spec, source, msg));
                }
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
        resolved_types: &ResolvedTypesMap,
    ) -> Result<(), Vec<Error>> {
        let mut errors: Vec<Error> = Vec::new();
        let mut updates: Vec<RuleReferenceUpdate> = Vec::new();
        let main_spec = self.main_spec;
        let resolve_unit = unit_tokens_for_spec(resolved_types, main_spec);

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
                match self.rule_presence(target_rule_path) {
                    RulePresence::Dropped => {}
                    RulePresence::Present => {
                        panic!("BUG: rule type not inferred before dependent in topological order")
                    }
                    RulePresence::Absent => {
                        errors.push(reference_error(
                            self.main_spec,
                            source,
                            format!(
                                "Data reference '{}' target rule '{}' does not exist",
                                reference_path, target_rule_path
                            ),
                        ));
                    }
                }
                continue;
            };

            if target_type.is_spec() {
                errors.push(reference_error(
                    self.main_spec,
                    source,
                    format!(
                        "cannot supply spec instance '{}' as a value",
                        target_rule_path.rule
                    ),
                ));
                continue;
            }

            // A target rule whose inferred type is `veto` carries no concrete
            // schema kind, so a LHS declared type cannot be checked against
            // it at planning time. The runtime veto propagation in the
            // evaluator will surface the rule's veto reason directly.
            if target_type.vetoed() || target_type.is_undetermined() {
                let mut merged = target_type.as_ref().clone();
                let mut raw_suggestion: Option<RawSuggestion> = None;
                let mut raw_value: Option<RawSuggestion> = None;
                if let Some(constraints) = local_constraints {
                    let constraint_type_name = merged.name();
                    match apply_constraints_to_spec(
                        self.main_spec,
                        &constraint_type_name,
                        merged.specifications.clone(),
                        constraints,
                        source,
                        ConstraintSuggestionSink {
                            suggestion: &mut raw_suggestion,
                            value: &mut raw_value,
                            resolve_unit: Some(&resolve_unit),
                        },
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
                        match bound_value_kind_from_raw_suggestion(
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
                let captured_value = match raw_value {
                    None => None,
                    Some(raw) => {
                        match bound_value_kind_from_raw_suggestion(
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
                    captured_value,
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
            let mut raw_value: Option<RawSuggestion> = None;
            if let Some(constraints) = local_constraints {
                let constraint_type_name = merged.name();
                match apply_constraints_to_spec(
                    self.main_spec,
                    &constraint_type_name,
                    merged.specifications.clone(),
                    constraints,
                    source,
                    ConstraintSuggestionSink {
                        suggestion: &mut raw_suggestion,
                        value: &mut raw_value,
                        resolve_unit: Some(&resolve_unit),
                    },
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
                    match bound_value_kind_from_raw_suggestion(
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
            let captured_value = match raw_value {
                None => None,
                Some(raw) => {
                    match bound_value_kind_from_raw_suggestion(
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
                captured_value,
            ));
        }

        for (path, new_type, new_suggestion, new_value) in updates {
            if let Some(DataDefinition::Reference {
                resolved_type,
                local_suggestion,
                local_value,
                ..
            }) = self.data.get_mut(&path)
            {
                *resolved_type = new_type;
                if new_suggestion.is_some() {
                    *local_suggestion = new_suggestion;
                }
                if new_value.is_some() {
                    *local_value = new_value;
                }
            } else {
                unreachable!(
                    "BUG: rule-target reference path disappeared between collect and update phases"
                );
            }
        }

        // Named-`uses` overlay rule targets: same kind check once rule types exist.
        for (import_path, definition) in &self.data {
            let DataDefinition::Import {
                identity,
                overlay,
                source,
                ..
            } = definition
            else {
                continue;
            };
            if overlay.is_empty() {
                continue;
            }
            let Some(template) = self.instance_lists.templates.get(identity).map(Arc::as_ref)
            else {
                continue;
            };
            for (relative, entry) in overlay {
                let ImportOverlay::Reference {
                    target: ReferenceTarget::Rule(target_rule),
                    ..
                } = entry
                else {
                    continue;
                };
                let Some((_, _, field_def)) = template.peel_data(relative) else {
                    continue;
                };
                let Some(lhs) = field_def.resolved_type_arc() else {
                    continue;
                };
                if lhs.is_undetermined() {
                    continue;
                }
                let absolute = absolute_import_member_path(import_path, identity, relative);
                let target_type = computed_rule_types
                    .get(target_rule)
                    .cloned()
                    .or_else(|| {
                        self.rules
                            .get(target_rule)
                            .map(|node| Arc::clone(&node.rule_type))
                    })
                    .or_else(|| {
                        peel_rule_from(&self.data, &self.instance_lists.templates, target_rule)
                            .and_then(|(body, local)| {
                                body.rules
                                    .get(local)
                                    .map(|rule| Arc::clone(&rule.rule_type))
                            })
                    });
                let Some(target_type) = target_type else {
                    errors.push(reference_error(
                        self.main_spec,
                        source,
                        format!(
                            "Data reference '{}' target rule '{}' does not exist",
                            absolute, target_rule
                        ),
                    ));
                    continue;
                };
                if let Some(msg) = reference_kind_mismatch_message(
                    lhs.as_ref(),
                    target_type.as_ref(),
                    &absolute,
                    target_rule,
                    "target rule",
                ) {
                    errors.push(reference_error(self.main_spec, source, msg));
                }
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
            collect_rule_reference_dependencies(
                &rule_node.expression,
                &self.reference_ends,
                &mut found,
            );
            for target in found {
                updates.push((rule_path.clone(), target));
            }
        }

        for (rule_path, target) in updates {
            self.rules
                .get_mut(&rule_path)
                .expect("BUG: rule from iteration missing in graph")
                .depends_on_rules
                .insert(target);
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
                    out.insert(path.clone(), self.expect_reference_end(start));
                }
            }
        }
        for (import_path, definition) in &self.data {
            let DataDefinition::Import {
                identity, overlay, ..
            } = definition
            else {
                continue;
            };
            for (relative, entry) in overlay {
                let absolute = absolute_import_member_path(import_path, identity, relative);
                if out.contains_key(&absolute) {
                    continue;
                }
                match entry {
                    ImportOverlay::Reference {
                        target: ReferenceTarget::Rule(rule_path),
                        ..
                    } => {
                        out.insert(absolute, ReferenceEnd::Rule(rule_path.clone()));
                    }
                    ImportOverlay::Reference {
                        target: ReferenceTarget::Data(start),
                        ..
                    } => {
                        out.insert(absolute, self.expect_reference_end(start));
                    }
                    ImportOverlay::Literal(_) => {}
                }
            }
        }
        out
    }

    /// [`execution_plan::reference_end_on`] for a target planning already validated.
    fn expect_reference_end(&self, start: &DataPath) -> ReferenceEnd {
        execution_plan::reference_end_on(self, start).unwrap_or_else(|| {
            panic!("BUG: data-target reference chain from '{start}' ends at missing data")
        })
    }

    /// Compute an order in which data-target references can be evaluated at
    /// runtime so each reference's target (when itself a reference) has been
    /// evaluated first. Rule-target references are intentionally excluded —
    /// they are resolved lazily on first read in the evaluator from the
    /// already-evaluated target rule's result. Cycles among data-target
    /// references are reported as planning errors.
    fn compute_data_reference_order(&self) -> Result<Vec<DataPath>, Vec<Error>> {
        // Local DataDefinition::Reference plus named-`uses` overlay data→data edges
        // (consumer-qualified paths). Overlay refs are not stored in `self.data`.
        let mut reference_paths: Vec<DataPath> = self
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
        let mut overlay_targets: BTreeMap<DataPath, DataPath> = BTreeMap::new();
        let mut overlay_sources: BTreeMap<DataPath, Source> = BTreeMap::new();
        for (import_path, definition) in &self.data {
            let DataDefinition::Import {
                identity,
                overlay,
                source,
                ..
            } = definition
            else {
                continue;
            };
            for (relative, entry) in overlay {
                let ImportOverlay::Reference {
                    target: ReferenceTarget::Data(target),
                    ..
                } = entry
                else {
                    continue;
                };
                let absolute = absolute_import_member_path(import_path, identity, relative);
                if !reference_paths.contains(&absolute) {
                    reference_paths.push(absolute.clone());
                }
                overlay_targets.insert(absolute.clone(), target.clone());
                overlay_sources.insert(absolute, source.clone());
            }
        }

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
            let target_path =
                if let Some(DataDefinition::Reference { target, .. }) = self.data.get(p) {
                    match target {
                        ReferenceTarget::Data(path) => Some(path.clone()),
                        ReferenceTarget::Rule(_) => None,
                    }
                } else {
                    overlay_targets.get(p).cloned()
                };
            let Some(target_path) = target_path else {
                continue;
            };
            if reference_set.contains(&target_path) {
                *in_degree
                    .get_mut(p)
                    .expect("BUG: reference missing in_degree") += 1;
                dependents
                    .get_mut(&target_path)
                    .expect("BUG: reference missing dependents list")
                    .push(p.clone());
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
                    let source = self
                        .data
                        .get(p)
                        .map(|entry| entry.source())
                        .or_else(|| overlay_sources.get(p));
                    source.map(|source| {
                        reference_error(
                            self.main_spec,
                            source,
                            format!("Circular data reference ({})", cycle_display),
                        )
                    })
                })
                .collect();
            return Err(errors);
        }

        // Runtime copy order only includes local DataDefinition::Reference paths.
        Ok(result
            .into_iter()
            .filter(|path| {
                matches!(
                    self.data.get(path),
                    Some(DataDefinition::Reference {
                        target: ReferenceTarget::Data(_),
                        ..
                    })
                )
            })
            .collect())
    }

    /// Candidate instances of spec-valued rules, field targets of `rule.field`,
    /// and the rule edges those need. Repeats the sort until those edges stop
    /// growing, so a spec-valued rule field is linked after the rule it reads.
    fn link_spec_values(&mut self) -> Result<Vec<Error>, Vec<Error>> {
        use crate::planning::spec_value::{
            assert_expression_candidates_linked, expression_candidates, fill_spec_members,
            rule_under_instance,
        };

        let rule_paths: HashSet<RulePath> = self.rules.keys().cloned().collect();
        let dropped_rules = self.dropped_rules.clone();
        // Presence closure cannot borrow `self` while rules are mutably taken below.
        // Clone data/templates only when named-`uses` peels can matter.
        let import_presence = self
            .data
            .values()
            .any(|definition| matches!(definition, DataDefinition::Import { .. }))
            .then(|| (self.data.clone(), self.instance_lists.templates.clone()));
        let presence_locals = rule_paths.clone();
        let presence = move |rule: &RulePath| {
            if presence_locals.contains(rule) {
                RulePresence::Present
            } else if dropped_rules.contains(rule) {
                RulePresence::Dropped
            } else if let Some((data, templates)) = &import_presence {
                if peel_rule_from(data, templates, rule).is_some() {
                    // Named `uses` template rule — same as [`Graph::rule_presence`].
                    RulePresence::Present
                } else if rule.segments.first().is_some_and(|hop| {
                    matches!(
                        data.get(&DataPath::local(hop.uses.clone())),
                        Some(DataDefinition::Import { .. })
                    )
                }) {
                    RulePresence::Dropped
                } else {
                    RulePresence::Absent
                }
            } else {
                RulePresence::Absent
            }
        };
        loop {
            let order = self.local_rules_in(&self.topological_sort()?);
            let mut rule_candidates: HashMap<RulePath, Vec<SpecInstance>> = HashMap::new();
            let mut member_errors = Vec::new();
            let mut grew = false;

            for path in order {
                let mut candidates = Vec::new();
                let node = self
                    .rules
                    .get(&path)
                    .expect("BUG: rule from topological sort not in graph");
                for (_, result) in &node.branches {
                    for candidate in expression_candidates(
                        node.expression.at(*result),
                        &rule_candidates,
                        &self.data,
                        &self.instance_lists.templates,
                        &presence,
                    ) {
                        if !candidates.contains(&candidate) {
                            candidates.push(candidate);
                        }
                    }
                }
                rule_candidates.insert(path.clone(), candidates.clone());

                let node = self
                    .rules
                    .get_mut(&path)
                    .expect("BUG: rule from topological sort not in graph");
                let (extra_deps, branch_errors) = fill_spec_members(
                    &mut node.expression,
                    &rule_candidates,
                    &self.data,
                    &self.instance_lists.templates,
                    &presence,
                );
                let before = node.depends_on_rules.len();
                node.depends_on_rules.extend(extra_deps);
                if !candidates.is_empty() {
                    let under: Vec<RulePath> = rule_paths
                        .iter()
                        .filter(|rule| {
                            *rule != &path
                                && candidates
                                    .iter()
                                    .any(|instance| rule_under_instance(rule, instance))
                        })
                        .cloned()
                        .collect();
                    node.depends_on_rules.extend(under);
                }
                if node.depends_on_rules.len() != before {
                    grew = true;
                }
                let main_spec = self.main_spec;
                for (source, message) in branch_errors {
                    let source = source
                        .unwrap_or_else(|| panic!("BUG: spec member expression missing source"));
                    member_errors.push(Error::validation_with_context(
                        message,
                        Some(source),
                        None::<String>,
                        Some(main_spec),
                        None,
                    ));
                }
            }
            if !grew {
                for node in self.rules.values() {
                    assert_expression_candidates_linked(
                        node.expression.root(),
                        &rule_candidates,
                        &self.data,
                        &self.instance_lists.templates,
                        &presence,
                    );
                }
                self.rule_candidates = rule_candidates;
                return Ok(member_errors);
            }
        }
    }

    fn local_rules_in(&self, execution_order: &[RulePath]) -> Vec<RulePath> {
        execution_order
            .iter()
            .filter(|path| self.has_local_rule(path))
            .cloned()
            .collect()
    }

    /// Dependency order of local rules and the named-`uses` rules they reach. A
    /// named-`uses` rule depends on the owner rules its site overlays bind
    /// ([`import_rule_needs`]), so overlay cycles are circular dependencies.
    fn topological_sort(&self) -> Result<Vec<RulePath>, Vec<Error>> {
        let mut depends_on: BTreeMap<RulePath, BTreeSet<RulePath>> = BTreeMap::new();
        let mut pending_imports: Vec<RulePath> = Vec::new();
        let present_dependencies = |dependencies: &mut dyn Iterator<Item = RulePath>,
                                    pending_imports: &mut Vec<RulePath>|
         -> BTreeSet<RulePath> {
            let mut present = BTreeSet::new();
            for dependency in dependencies {
                match self.rule_presence(&dependency) {
                    RulePresence::Present => {
                        if !self.has_local_rule(&dependency) {
                            pending_imports.push(dependency.clone());
                        }
                        present.insert(dependency);
                    }
                    RulePresence::Dropped => {}
                    RulePresence::Absent => {
                        panic!("BUG: rule '{dependency}' neither in graph nor dropped")
                    }
                }
            }
            present
        };
        for (rule_path, rule_node) in &self.rules {
            let present = present_dependencies(
                &mut rule_node.depends_on_rules.iter().cloned(),
                &mut pending_imports,
            );
            depends_on.insert(rule_path.clone(), present);
        }
        while let Some(import_path) = pending_imports.pop() {
            if depends_on.contains_key(&import_path) {
                continue;
            }
            let needs = import_rule_needs(self, &import_path);
            let present = present_dependencies(
                &mut needs.into_iter().filter_map(|need| match need {
                    ImportNeed::Rule(rule) => Some(rule),
                    ImportNeed::Data(_) => None,
                }),
                &mut pending_imports,
            );
            depends_on.insert(import_path, present);
        }
        let mut in_degree: BTreeMap<RulePath, usize> = BTreeMap::new();
        let mut dependents: BTreeMap<RulePath, Vec<RulePath>> = BTreeMap::new();
        let mut queue = VecDeque::new();
        let mut result = Vec::new();
        for (rule_path, dependencies) in &depends_on {
            in_degree.insert(rule_path.clone(), dependencies.len());
            for dependency in dependencies {
                dependents
                    .entry(dependency.clone())
                    .or_default()
                    .push(rule_path.clone());
            }
        }

        for (rule_path, degree) in &in_degree {
            if *degree == 0 {
                queue.push_back(rule_path.clone());
            }
        }

        while let Some(rule_path) = queue.pop_front() {
            for dependent in dependents.get(&rule_path).into_iter().flatten() {
                let degree = in_degree
                    .get_mut(dependent)
                    .expect("BUG: rule missing from in_degree");
                *degree -= 1;
                if *degree == 0 {
                    queue.push_back(dependent.clone());
                }
            }
            result.push(rule_path);
        }

        if result.len() != depends_on.len() {
            let missing: Vec<RulePath> = depends_on
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
                .map(RulePath::input_key)
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
    /// The whole rule in one table: the default expression when the rule has no unless
    /// clauses, else a `Piecewise` over every branch in source order whose default arm
    /// has condition literal `true`.
    pub expression: Expression,
    /// Condition and result ids in [`Self::expression`]: the default first (no condition),
    /// then each unless clause in source order.
    pub branches: Vec<(Option<ExprId>, ExprId)>,
    pub source: Source,

    pub depends_on_rules: BTreeSet<RulePath>,

    /// Reads itself on rows of a self-list (`components -> sum components.unit_cost`).
    /// Not an edge in [`Self::depends_on_rules`]: child rows are evaluated first.
    pub recursive: bool,

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

fn local_type_entries(resolved_types: &ResolvedSpecTypes) -> HashMap<String, ResolvedTypeEntry> {
    resolved_types
        .resolved
        .iter()
        .map(|(name, lemma_type)| {
            (
                name.clone(),
                (
                    Arc::clone(lemma_type),
                    resolved_types.source_defaults.get(name).cloned(),
                    resolved_types.source_value_defaults.get(name).cloned(),
                ),
            )
        })
        .collect()
}

/// Pending rule-reference type merge: path, type, local suggest, local fill.
type RuleReferenceUpdate = (
    DataPath,
    Arc<LemmaType>,
    Option<BoundValueKind>,
    Option<BoundValueKind>,
);

/// Ok payload of [`GraphBuilder::build`]: data, rules, dropped rules, soft errors, resolved types.
type GraphBuildOk<'a> = (
    IndexMap<DataPath, DataDefinition>,
    BTreeMap<RulePath, RuleNode<'a>>,
    BTreeSet<RulePath>,
    Vec<Error>,
    ResolvedTypesMap<'a>,
    InstanceLists,
);

struct GraphBuilder<'a, 'i> {
    data: IndexMap<DataPath, DataDefinition>,
    rules: BTreeMap<RulePath, RuleNode<'a>>,
    dropped_rules: BTreeSet<RulePath>,
    context: &'a Context,
    local_types: ResolvedTypesMap<'a>,
    errors: Vec<Error>,
    nested_template_failed: bool,
    main_spec: &'a LemmaSpec,
    limits: &'a crate::limits::ResourceLimits,
    instance_lists: InstanceLists,
    interners: &'i mut instance_list::PassInterners,
}

struct RuleExpressionConversion<'a, 'b> {
    /// Nodes of every expression converted for the rule being added.
    table: ExprTable<ExpressionKind>,
    spec: &'a LemmaSpec,
    data_map: &'b HashMap<String, &'b LemmaData>,
    segments: &'b [PathSegment],
    rule_names: &'b HashSet<&'b str>,
    /// Rule being converted. Never a clash for a row field: it cannot read itself.
    rule: &'b str,
    effective: &'b EffectiveDate,
    depends_on_rules: &'b mut BTreeSet<RulePath>,
    /// Instance-list commands enclosing the expression being converted, outermost first.
    row_scopes: Vec<RowScope>,
    /// The rule reads itself on rows of a self-list.
    reads_itself_on_rows: bool,
}

impl RuleExpressionConversion<'_, '_> {
    fn push(&mut self, kind: ExpressionKind, source: Option<Source>) -> ExprId {
        self.table.push(kind, source)
    }
}

type AstRef<'a> = ExprRef<'a, ast::ExpressionKind>;

/// Work item of a DFS over resolved and source expressions and rule references.
enum Pending<'a> {
    Sem(ExprRef<'a, ExpressionKind>),
    Ast(AstRef<'a>),
    Rule(RulePath),
}

/// Children converted in the enclosing row scope. List command arguments are not among them:
/// they are converted in their own row scope by the list command.
fn for_each_conversion_child(kind: &ast::ExpressionKind, mut f: impl FnMut(ExprId)) {
    match kind {
        ast::ExpressionKind::ListCommand { list, .. } => f(*list),
        other => other.for_each_child(f),
    }
}

fn one(children: &[ExprId]) -> ExprId {
    let &[child] = children else {
        panic!("BUG: AST expression with one operand has one converted child");
    };
    child
}

/// A chain's first operand and the operands of its steps, in source order.
/// Converted `first` and steps of an AST chain whose operands converted to `children`.
fn convert_chain_steps<Op: Clone>(
    steps: &[ast::ChainStep<Op>],
    children: &[ExprId],
) -> (ExprId, Vec<ast::ChainStep<Op>>) {
    let mut children = children.iter().copied();
    let first = children
        .next()
        .expect("BUG: chain expression always has a first operand");
    let steps: Vec<ast::ChainStep<Op>> = steps
        .iter()
        .zip(children.by_ref())
        .map(|(step, operand)| ast::ChainStep {
            op: step.op.clone(),
            operand,
            source_location: step.source_location.clone(),
        })
        .collect();
    assert!(
        children.next().is_none(),
        "BUG: chain converted to more operands than steps"
    );
    (first, steps)
}

fn two(children: &[ExprId]) -> (ExprId, ExprId) {
    let &[left, right] = children else {
        panic!("BUG: AST expression with two operands has two converted children");
    };
    (left, right)
}

/// Work of [`GraphBuilder::convert_ast_expression`]. A node's operands are converted before
/// the node; a list command's arguments are converted between its list operand and its
/// build, each in the row scope the argument names.
enum ConvertFrame<'t> {
    /// Convert the subtree at this node.
    Enter(AstRef<'t>),
    /// Operands of this node are converted; their results are on the value stack.
    Node(AstRef<'t>),
    /// List command whose next argument is converted, or which is built when none is left.
    List(Box<ListCommandBuild<'t>>),
    /// Argument of a list command was converted; its result is on the value stack.
    ListArgument(Box<ListCommandBuild<'t>>, ListArgumentSink),
    /// Ends the row scope pushed for a list command argument.
    PopRowScope,
}

/// Stacks of [`GraphBuilder::convert_ast_expression`], reused across one rule's expressions.
/// Empty between calls.
struct ConvertScratch<'t> {
    frames: Vec<ConvertFrame<'t>>,
    values: Vec<Option<ExprId>>,
    children: Vec<ExprId>,
}

/// Where the converted value of a list command argument goes.
enum ListArgumentSink {
    /// Positional argument: an unconvertible argument makes the command unconvertible.
    Positional,
    /// Value of the `-> with` row field at this data path.
    Field(DataPath),
}

/// Argument of a list command, converted in queue order.
enum ListArgument {
    /// Converted in the row scopes enclosing the command.
    Enclosing(ExprId),
    /// Converted with the rows of the list in view.
    InRows(ExprId),
    /// Value of the `-> with` row field `name`.
    Field { name: String, value: ExprId },
}

/// A list command whose arguments are being converted.
struct ListCommandBuild<'t> {
    node: AstRef<'t>,
    source: Source,
    list: ExprId,
    row_level: Option<u32>,
    row_scope: Option<RowScope>,
    pending: VecDeque<ListArgument>,
    converted: Vec<ExprId>,
    fields: Vec<(DataPath, ExprId)>,
    failed: bool,
}

/// Arguments of `command` in the order they are converted.
fn list_command_arguments(
    command: &ast::ListCommand,
    row_scope: &Option<RowScope>,
) -> VecDeque<ListArgument> {
    match command {
        ast::ListCommand::Find { keys } => keys
            .iter()
            .map(|key| ListArgument::Enclosing(*key))
            .collect(),
        ast::ListCommand::First | ast::ListCommand::Last | ast::ListCommand::Count => {
            VecDeque::new()
        }
        ast::ListCommand::Sum { argument }
        | ast::ListCommand::Avg { argument }
        | ast::ListCommand::Product { argument }
        | ast::ListCommand::Min { argument }
        | ast::ListCommand::Max { argument }
        | ast::ListCommand::Sort { argument, .. } => argument
            .iter()
            .map(|argument| ListArgument::InRows(*argument))
            .collect(),
        ast::ListCommand::Any { form }
        | ast::ListCommand::All { form }
        | ast::ListCommand::Where { form } => match form {
            ast::ListPredicate::Value { right, .. } => {
                VecDeque::from([ListArgument::Enclosing(*right)])
            }
            ast::ListPredicate::Instance { expr } => VecDeque::from([ListArgument::InRows(*expr)]),
        },
        ast::ListCommand::Top { n } => VecDeque::from([ListArgument::Enclosing(*n)]),
        ast::ListCommand::With { rows } => match (rows, row_scope) {
            (ast::ListWithRows::Values(values), None) => values
                .iter()
                .map(|value| ListArgument::Enclosing(*value))
                .collect(),
            (ast::ListWithRows::Values(_), Some(_)) | (ast::ListWithRows::FieldPairs(_), None) => {
                VecDeque::new()
            }
            (ast::ListWithRows::FieldPairs(pairs), Some(_)) => pairs
                .iter()
                .map(|(field, value)| ListArgument::Field {
                    name: field.to_string(),
                    value: *value,
                })
                .collect(),
        },
    }
}

fn optional_argument(
    argument: &Option<ExprId>,
    converted: &mut impl Iterator<Item = ExprId>,
) -> Option<ExprId> {
    argument.as_ref().map(|_| next_argument(converted))
}

fn next_argument(converted: &mut impl Iterator<Item = ExprId>) -> ExprId {
    converted
        .next()
        .expect("BUG: one converted argument per list command argument")
}

fn list_predicate_op(
    form: &ast::ListPredicate,
    kind: ListPredKind,
    converted: &mut impl Iterator<Item = ExprId>,
) -> ListOp {
    match form {
        ast::ListPredicate::Value { op, .. } => ListOp::Predicate {
            kind,
            op: op.clone(),
            right: next_argument(converted),
        },
        ast::ListPredicate::Instance { .. } => ListOp::InstancePredicate {
            kind,
            expr: next_argument(converted),
        },
    }
}

/// Spec being converted and its `uses` path in the graph.
struct SourceScope<'s> {
    spec: &'s LemmaSpec,
    segments: &'s [PathSegment],
}

/// Outcome of reading a name as a row field inside an instance-list command.
enum RowFieldLookup {
    Field(ExpressionKind),
    NotARowField,
    /// An error was pushed.
    Failed,
}

/// Rows of one instance-list command: argument names may read their fields.
#[derive(Clone)]
struct RowScope {
    /// Path of the list operand (`menu`, `card`+`menu`) when it is a plain reference.
    list_path: Option<Vec<String>>,
    identity: SpecIdentity,
    template: RowTemplate,
    level: u32,
}

/// Where rows of an instance list read their fields while the consumer is planned.
#[derive(Clone)]
enum RowTemplate {
    /// Row spec planned on its own.
    Plan(Arc<ExecutionPlan>),
    /// Self-list: rows are the spec being planned.
    Root,
}

/// A spec whose imports are being resolved into `local_types`. It is finished after its
/// last import.
struct SpecTypesFrame<'a> {
    spec: &'a LemmaSpec,
    repository: Arc<LemmaRepository>,
    effective: EffectiveDate,
    next_data: usize,
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
    let mut current = parent;
    loop {
        match current {
            ParentType::Custom { name } => return name.clone(),
            ParentType::Qualified { inner, .. }
            | ParentType::Ranged { inner, .. }
            | ParentType::List { inner, .. } => current = inner.as_ref(),
            ParentType::Primitive { .. } => return data_name.to_string(),
        }
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
struct ConstraintSuggestionSink<'a> {
    suggestion: &'a mut Option<RawSuggestion>,
    value: &'a mut Option<RawSuggestion>,
    resolve_unit: Option<&'a UnitTokenResolver<'a>>,
}

fn apply_constraints_to_spec(
    spec: &LemmaSpec,
    type_name: &str,
    mut specs: TypeSpecification,
    constraints: &[Constraint],
    source: &crate::parsing::source::Source,
    sink: ConstraintSuggestionSink<'_>,
) -> Result<TypeSpecification, Vec<Error>> {
    let mut errors = Vec::new();

    let mut seen_unit_names: std::collections::HashSet<String> = Default::default();
    let mut seen_singleton_commands: std::collections::HashSet<TypeConstraintCommand> =
        Default::default();
    let mut has_suggest = false;
    let mut has_value = false;
    for row in constraints.iter() {
        let command = row.command;
        let args = &row.args;
        if command == TypeConstraintCommand::Suggest {
            has_suggest = true;
        }
        if command == TypeConstraintCommand::With {
            has_value = true;
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
                | TypeConstraintCommand::With
        ) && !(command == TypeConstraintCommand::With
            && matches!(specs, TypeSpecification::List { .. }))
            && !seen_singleton_commands.insert(command)
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
    if has_suggest && has_value {
        errors.push(Error::validation_with_context(
            "Cannot use both 'with' and 'suggest' on the same data declaration".to_string(),
            Some(source.clone()),
            None::<String>,
            Some(spec),
            None,
        ));
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let ConstraintSuggestionSink {
        suggestion: declared_suggestion,
        value: declared_value,
        resolve_unit,
    } = sink;

    let mut apply_one = |specs: &mut TypeSpecification,
                         command: TypeConstraintCommand,
                         args: &[CommandArg],
                         declared_suggestion: &mut Option<RawSuggestion>,
                         declared_value: &mut Option<RawSuggestion>| {
        let mut suggestion_before = declared_suggestion.clone();
        let mut value_before = declared_value.clone();
        match specs.apply_constraint(
            type_name,
            command,
            args,
            &mut suggestion_before,
            &mut value_before,
            resolve_unit,
        ) {
            Ok(()) => {
                *declared_suggestion = suggestion_before;
                *declared_value = value_before;
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
                declared_value,
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
                declared_value,
            );
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(specs)
}

/// Every node reachable from the root carries a `source_location`.
fn expression_has_source_locations(expr: &ast::Expression) -> bool {
    expr.table
        .nodes()
        .all(|node| node.source_location.is_some())
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
        interners: &mut instance_list::PassInterners,
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

        let (data, rules, dropped_rules, graph_errors, local_types, instance_lists) =
            match GraphBuilder::build(
                context,
                repository,
                main_spec,
                effective,
                &type_resolver,
                limits,
                interners,
            ) {
                Ok(built) => built,
                Err(build_errors) => {
                    errors.extend(build_errors);
                    return Err(errors);
                }
            };

        let mut graph = Graph {
            data,
            rules,
            dropped_rules,
            rule_order: Vec::new(),
            execution_order: Vec::new(),
            data_reference_order: Vec::new(),
            reference_ends: IndexMap::new(),
            rule_candidates: HashMap::new(),
            main_spec,
            instance_lists,
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
        if let Err(reference_errors) =
            self.resolve_data_reference_types(&reference_order, resolved_types)
        {
            errors.extend(reference_errors);
        }
        if let Err(overlay_errors) = self.validate_import_overlay_reference_types() {
            errors.extend(overlay_errors);
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

        match self.link_spec_values() {
            Ok(member_errors) => errors.extend(member_errors),
            Err(cycle_errors) => {
                errors.extend(cycle_errors);
                return Err(errors);
            }
        }

        let execution_order = match self.topological_sort() {
            Ok(order) => order,
            Err(circular_errors) => {
                errors.extend(circular_errors);
                return Err(errors);
            }
        };
        let rule_order = self.local_rules_in(&execution_order);

        // Continue to type inference and type checking even when structural
        // checks found errors.  This lets us report structural errors (e.g.
        // missing rule reference) alongside type errors (e.g. branch type
        // mismatch) in a single pass.

        // Phase 3–5 share one infer cache for walk reuse within a phase. Phase 4
        // mutates rule-target `resolved_type` (constraint merge), so the cache is
        // cleared before phase 5 re-infers against the merged types.
        let mut infer_cache = InferExpressionTypeCache::new();
        let (inferred_types, seed_errors) =
            infer_rule_types(self, &rule_order, resolved_types, &mut infer_cache);
        let rule_reference_errors =
            self.resolve_rule_reference_types(&inferred_types, resolved_types);
        infer_cache.clear();
        let type_errors = check_rule_types(
            self,
            &rule_order,
            &inferred_types,
            resolved_types,
            &mut infer_cache,
        );

        errors.extend(seed_errors);
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
        self.execution_order = execution_order;
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
    let DataDefinition::Import { identity, .. } = existing else {
        unreachable!("BUG: uses vs data clash but no Import side with recoverable spec_ref");
    };
    std::borrow::Cow::Owned(ast::SpecRef::same_repository(identity.spec.clone()))
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

impl<'a, 'i> GraphBuilder<'a, 'i> {
    fn build(
        context: &'a Context,
        repository: &Arc<LemmaRepository>,
        main_spec: &'a LemmaSpec,
        effective: &EffectiveDate,
        type_resolver: &TypeResolver<'a>,
        limits: &'a crate::limits::ResourceLimits,
        interners: &'i mut instance_list::PassInterners,
    ) -> Result<GraphBuildOk<'a>, Vec<Error>> {
        let mut builder = GraphBuilder {
            data: IndexMap::new(),
            rules: BTreeMap::new(),
            dropped_rules: BTreeSet::new(),
            context,
            local_types: Vec::new(),
            errors: Vec::new(),
            nested_template_failed: false,
            main_spec,
            limits,
            instance_lists: InstanceLists::new(SpecIdentity::of(repository, main_spec)),
            interners,
        };

        builder.build_spec(
            main_spec,
            repository,
            Vec::new(),
            IndexMap::new(),
            effective,
            type_resolver,
        )?;

        if builder.nested_template_failed {
            return Err(builder.errors);
        }

        Ok((
            builder.data,
            builder.rules,
            builder.dropped_rules,
            builder.errors,
            builder.local_types,
            builder.instance_lists,
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

    /// Build a measure/ratio [`ValueKind::Range`] with each endpoint typed and stamped with its
    /// bare resolved unit. Same-unit and mixed-unit ranges both go through this path.
    fn measure_ratio_range_from_number_with_unit_endpoints(
        &self,
        current_spec: &LemmaSpec,
        left_mag: rust_decimal::Decimal,
        left_unit: &str,
        right_mag: rust_decimal::Decimal,
        right_unit: &str,
    ) -> Result<ValueKind, String> {
        let (left_bare, left_lt) = self.resolve_unit_ref(current_spec, left_unit)?;
        let (right_bare, right_lt) = self.resolve_unit_ref(current_spec, right_unit)?;
        let left_kind = number_with_unit_to_value_kind(left_mag, &left_bare, left_lt.as_ref())?;
        let right_kind = number_with_unit_to_value_kind(right_mag, &right_bare, right_lt.as_ref())?;
        let left_ty = Arc::new(left_lt.as_ref().clone().with_unit(left_bare.as_str()));
        let right_ty = Arc::new(right_lt.as_ref().clone().with_unit(right_bare.as_str()));
        range_type_specification_from_endpoints(left_ty.as_ref(), right_ty.as_ref()).ok_or_else(
            || {
                format!(
                    "Range endpoints must share a measure or ratio family (got '{}' and '{}')",
                    left_lt.name(),
                    right_lt.name()
                )
            },
        )?;
        Ok(ValueKind::Range(
            Box::new(TypedLiteral {
                value: left_kind,
                lemma_type: left_ty,
            }),
            Box::new(TypedLiteral {
                value: right_kind,
                lemma_type: right_ty,
            }),
        ))
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
                if matches!(
                    containing_data_map
                        .get(&reference.name)
                        .map(|data| &data.value),
                    Some(ParsedDataValue::Import { .. })
                ) {
                    self.errors.push(self.engine_error(
                        format!(
                            "cannot supply spec instance '{}' as a value",
                            reference.name
                        ),
                        reference_source,
                    ));
                    return None;
                }
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
            if target_spec.data.iter().any(|field| {
                field.reference.name == reference.name
                    && field.reference.segments.is_empty()
                    && matches!(field.value, ParsedDataValue::Import { .. })
            }) {
                self.errors.push(self.engine_error(
                    format!(
                        "cannot supply spec instance '{}' as a value",
                        reference.name
                    ),
                    reference_source,
                ));
                return None;
            }
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
        let mut bindings: DataBindings = IndexMap::new();
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
                        ParsedDataValue::ImportList { .. } => {
                            self.errors.push(Error::validation_with_context(
                                format!(
                                    "binding `{path}` replaces instance list `{segment}`; supply the whole list through evaluation data",
                                    path = binding_reference,
                                ),
                                Some(binding.source_location.clone()),
                                None::<String>,
                                Some(self.main_spec),
                                None,
                            ));
                            binding_resolved = false;
                            break;
                        }
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

                    let walk_repository = discovery::owning_repository(self.context, walk_spec);
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

                if walk_spec.data.iter().any(|field| {
                    field.reference.segments.is_empty()
                        && field.reference.name == binding_reference.name
                        && matches!(&field.value, ParsedDataValue::ImportList { .. })
                }) {
                    self.errors.push(Error::validation_with_context(
                        format!(
                            "binding `{path}` replaces instance list `{name}`; supply the whole list through evaluation data",
                            path = binding_reference,
                            name = binding_reference.name,
                        ),
                        Some(binding.source_location.clone()),
                        None::<String>,
                        Some(self.main_spec),
                        None,
                    ));
                    continue;
                }

                let mut binding_key: Vec<String> = current_segments
                    .iter()
                    .map(|segment| segment.uses.clone())
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

    /// Apply `-> with` bindings whose key is a slot of this spec, in source order.
    fn apply_bindings_in_source_order(
        &mut self,
        current_spec: &LemmaSpec,
        current_segments: &[PathSegment],
        bindings: &DataBindings,
        used_binding_keys: &mut HashSet<Vec<String>>,
    ) {
        let prefix: Vec<&str> = current_segments
            .iter()
            .map(|segment| segment.uses.as_str())
            .collect();
        for (key, (value, source)) in bindings {
            let Some((name, key_prefix)) = key.split_last() else {
                continue;
            };
            if key_prefix.len() != prefix.len()
                || key_prefix
                    .iter()
                    .zip(&prefix)
                    .any(|(key_segment, prefix_segment)| key_segment != prefix_segment)
            {
                continue;
            }
            let path = DataPath {
                segments: current_segments.to_vec(),
                data: name.clone(),
            };
            let Some(existing) = self.data.get(&path).cloned() else {
                continue;
            };
            used_binding_keys.insert(key.clone());
            let schema = match &existing {
                DataDefinition::TypeDeclaration { resolved_type, .. }
                | DataDefinition::Value { resolved_type, .. }
                | DataDefinition::Reference { resolved_type, .. } => {
                    Some(Arc::clone(resolved_type))
                }
                DataDefinition::Import { .. } => None,
                DataDefinition::InstanceList { .. } => unreachable!(
                    "BUG: binding `{}` onto instance list `{path}` passed build_data_bindings",
                    key.join(".")
                ),
            };
            self.add_data_from_binding(path, value.clone(), source.clone(), schema, current_spec);
        }
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
        current_spec: &'a LemmaSpec,
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

        let (original_schema_type, original_declared_suggestion, original_declared_value) = if matches!(
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
            let declared_value = resolved.declared_values.get(&data.reference.name).cloned();
            (Some(lemma_type), declared, declared_value)
        } else {
            (None, None, None)
        };

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
                let declared_value = original_declared_value;

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
                        Some(BoundValueKind {
                            value: ValueKind::Range(left, right),
                            ..
                        }) if matches!(
                            (&left.value, &right.value),
                            (ValueKind::Measure(_), ValueKind::Measure(_))
                        )
                    );
                    let measure_endpoint_fill = matches!(
                        &declared_value,
                        Some(BoundValueKind {
                            value: ValueKind::Range(left, right),
                            ..
                        }) if matches!(
                            (&left.value, &right.value),
                            (ValueKind::Measure(_), ValueKind::Measure(_))
                        )
                    );
                    if measure_endpoint_suggest || measure_endpoint_fill {
                        self.errors.push(self.engine_error(
                            "measure range requires unit declarations before a measure-endpoint suggest or with".to_string(),
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
                        declared_value,
                        source: effective_source,
                    },
                );
            }
            ParsedDataValue::Import { spec_ref, .. } => {
                let consumer_repository = discovery::owning_repository(self.context, current_spec);
                let (resolved_repository, effective_spec) = match self.resolve_spec_ref(
                    spec_ref,
                    effective,
                    current_spec,
                    &consumer_repository,
                ) {
                    Ok(pair) => pair,
                    Err(e) => {
                        self.errors.push(e);
                        return;
                    }
                };

                self.data.insert(
                    data_path,
                    DataDefinition::Import {
                        identity: SpecIdentity::of(&resolved_repository, effective_spec),
                        overlay: IndexMap::new(),
                        source: effective_source,
                    },
                );
            }
            ParsedDataValue::ImportList { .. } => {
                self.add_instance_list(data, data_path, current_segments, current_spec, effective);
            }
        }
    }

    fn add_instance_list(
        &mut self,
        data: &LemmaData,
        data_path: DataPath,
        current_segments: &[PathSegment],
        current_spec: &'a LemmaSpec,
        effective: &EffectiveDate,
    ) {
        let ParsedDataValue::ImportList { spec_ref, list } = &data.value else {
            panic!("BUG: add_instance_list on non-list data '{data_path}'");
        };
        let source = data.source_location.clone();
        let consumer_repository = discovery::owning_repository(self.context, current_spec);
        let declaring_owner = SpecIdentity::of(&consumer_repository, current_spec);
        let (row_repository, row_spec) = match discovery::resolve_spec_ref_or_self(
            self.context,
            spec_ref,
            &consumer_repository,
            current_spec,
            effective,
            None,
        ) {
            Ok(pair) => pair,
            Err(e) => {
                self.errors.push(e);
                return;
            }
        };
        let identity = SpecIdentity::of(&row_repository, row_spec);
        let intern_key = instance_list::ListSourceKey {
            owner: declaring_owner,
            local: DataPath::local(data_path.data.clone()),
            element: identity.clone(),
            at: effective.clone(),
        };

        let (key, rows) = if discovery::same_loaded_spec(row_spec, self.main_spec) {
            assert_eq!(
                identity, self.instance_lists.self_identity,
                "BUG: self-list row identity differs from the planned spec's identity"
            );
            if !list.rows.is_empty() {
                self.errors.push(self.engine_error(
                    format!(
                        "`{}` is a list of `{}` inside `{}` itself: it cannot have source rows, \
                         since every row would hold the same rows forever; rows come from run data",
                        data_path.data, identity.spec, identity.spec
                    ),
                    &source,
                ));
                return;
            }
            match self.instance_list_key(list, &self.data, &identity) {
                Ok((key, _)) => (key, Arc::new(Vec::new())),
                Err(errors) => {
                    self.errors.extend(errors);
                    return;
                }
            }
        } else if let Some((interned_key, interned_rows)) = self
            .interners
            .list_sources
            .get(&intern_key)
            .map(|interned| {
                assert_eq!(
                    interned.identity, identity,
                    "BUG: interned list element identity mismatch for owner {} list {}",
                    intern_key.owner, intern_key.local
                );
                (interned.key.clone(), Arc::clone(&interned.rows))
            })
        {
            let Some(template) = self.row_template(
                &identity,
                &row_repository,
                row_spec,
                &spec_ref.at(effective),
            ) else {
                return;
            };
            let (expected_key, _) = match self.instance_list_key(list, &template.data, &identity) {
                Ok(key) => key,
                Err(errors) => {
                    self.errors.extend(errors);
                    return;
                }
            };
            assert_eq!(
                interned_key, expected_key,
                "BUG: interned list key mismatch for owner {} list {}",
                intern_key.owner, intern_key.local
            );
            (interned_key, interned_rows)
        } else {
            let Some(template) = self.row_template(
                &identity,
                &row_repository,
                row_spec,
                &spec_ref.at(effective),
            ) else {
                return;
            };
            let (key, key_paths) = match self.instance_list_key(list, &template.data, &identity) {
                Ok(key) => key,
                Err(errors) => {
                    self.errors.extend(errors);
                    return;
                }
            };
            let row_spec = RowSpec {
                identity: &identity,
                template: &template,
            };
            let rows = self.instance_list_rows(
                &data_path,
                list,
                &row_spec,
                current_segments,
                current_spec,
                effective,
            );
            self.check_source_row_keys(&key, &key_paths, &rows, &row_spec, &source);
            let rows = Arc::new(rows);
            self.interners.list_sources.insert(
                intern_key,
                instance_list::InternedList {
                    identity: identity.clone(),
                    key: key.clone(),
                    rows: Arc::clone(&rows),
                },
            );
            (key, rows)
        };

        let element = Arc::new(LemmaType::primitive(TypeSpecification::Spec {
            spec: identity.clone(),
        }));
        let resolved_type = Arc::new(LemmaType {
            name: Some(format!("{} list", identity.spec)),
            specifications: TypeSpecification::List {
                element,
                key: key.clone(),
            },
            extends: TypeExtends::Primitive,
        });
        self.data.insert(
            data_path.clone(),
            DataDefinition::InstanceList {
                resolved_type,
                source,
            },
        );
        self.instance_lists.sources.insert(
            data_path,
            InstanceListSource {
                identity,
                key,
                rows,
            },
        );
    }

    /// Rows of `identity` as instance-list commands read them.
    fn row_template_of(&self, identity: &SpecIdentity) -> RowTemplate {
        if *identity == self.instance_lists.self_identity {
            return RowTemplate::Root;
        }
        RowTemplate::Plan(Arc::clone(
            self.instance_lists
                .templates
                .get(identity)
                .expect("BUG: instance list identity without a template plan"),
        ))
    }

    /// Top-level main-spec rules (forward refs while converting) then present
    /// graph rules, each [`RulePath`] once.
    fn conversion_root_rules(&self) -> Vec<RulePath> {
        let mut rules = Vec::new();
        let mut seen = HashSet::new();
        for rule in &self.main_spec.rules {
            let path = RulePath {
                segments: Vec::new(),
                rule: rule.name.clone(),
            };
            if seen.insert(path.clone()) {
                rules.push(path);
            }
        }
        for path in self.rules.keys() {
            if seen.insert(path.clone()) {
                rules.push(path.clone());
            }
        }
        rules
    }

    /// Field `name` on rows read through `template`.
    fn template_row_end(&self, template: &RowTemplate, name: &str) -> Option<RowEnd> {
        match template {
            RowTemplate::Plan(plan) => {
                instance_list::row_end_of(instance_list::RowEndView::Plan(plan), name)
            }
            RowTemplate::Root => {
                let owned = self.conversion_root_rules();
                let rules: Vec<&RulePath> = owned.iter().collect();
                root_row_end(&self.data, &self.instance_lists.templates, &rules, name)
            }
        }
    }

    /// Data path of row data `name` read through `template`.
    fn template_data_path(&self, template: &RowTemplate, name: &str) -> Option<DataPath> {
        match template {
            RowTemplate::Plan(plan) => {
                let parts: Vec<&str> = name.split('.').collect();
                plan.qualify_data_names(&parts)
            }
            RowTemplate::Root => match name.split_once('.') {
                None => self
                    .data
                    .keys()
                    .find(|path| path.input_key() == name)
                    .cloned(),
                Some((hop, rest)) => {
                    let (import, identity, template) =
                        root_import_hop(&self.data, &self.instance_lists.templates, hop)?;
                    let parts: Vec<&str> = rest.split('.').collect();
                    let path = template.qualify_data_names(&parts)?;
                    Some(absolute_import_member_path(&import, identity, &path))
                }
            },
        }
    }

    /// Template plan of `identity`, shared pass-wide and recorded on this consumer.
    ///
    /// `at` is the planning instant for this edge (`spec_ref.at(consumer_effective)`),
    /// so unpinned transitive imports inside the body resolve at that instant.
    /// `identity.effective` is the resolved target version's `effective_from`.
    fn row_template(
        &mut self,
        identity: &SpecIdentity,
        row_repository: &Arc<LemmaRepository>,
        row_spec: &'a LemmaSpec,
        at: &EffectiveDate,
    ) -> Option<Arc<ExecutionPlan>> {
        if let Some(template) = self.instance_lists.templates.get(identity) {
            return Some(Arc::clone(template));
        }
        if let Some(template) = self.interners.spec_templates.get(identity, at) {
            self.record_template(identity.clone(), Arc::clone(&template));
            return Some(template);
        }
        let planning_input = match super::planning_input(self.context, row_repository, row_spec) {
            Ok(planning_input) => planning_input,
            Err(errors) => {
                self.errors.extend(errors);
                self.nested_template_failed = true;
                return None;
            }
        };
        match super::get_or_build_template(
            self.context,
            row_repository,
            row_spec,
            at,
            self.limits,
            self.interners,
            &planning_input,
        ) {
            Ok(template) => {
                self.record_template(identity.clone(), Arc::clone(&template));
                Some(template)
            }
            Err(errors) => {
                self.errors.extend(errors);
                self.nested_template_failed = true;
                None
            }
        }
    }

    /// Record `template` under `identity` and absorb every nested row-element plan it carries.
    fn record_template(&mut self, identity: SpecIdentity, template: Arc<ExecutionPlan>) {
        for (row_identity, row_plan) in &template.row_templates {
            self.instance_lists
                .templates
                .entry(row_identity.clone())
                .or_insert_with(|| Arc::clone(row_plan));
        }
        self.instance_lists
            .templates
            .entry(identity)
            .or_insert(template);
    }

    /// `-> key a, b`: names and template data paths of the key fields.
    fn instance_list_key(
        &self,
        list: &ast::UsesList,
        row_data: &IndexMap<DataPath, DataDefinition>,
        identity: &SpecIdentity,
    ) -> Result<(ListKey, Vec<DataPath>), Vec<Error>> {
        let Some(declared) = &list.key else {
            return Ok((ListKey::Numbered, Vec::new()));
        };
        let mut names: Vec<String> = Vec::with_capacity(declared.fields.len());
        let mut paths: Vec<DataPath> = Vec::with_capacity(declared.fields.len());
        let mut errors = Vec::new();
        for field in &declared.fields {
            if !field.segments.is_empty() {
                errors.push(self.engine_error(
                    format!(
                        "key field `{field}` is a dotted path; a key names data declared on `{}` itself",
                        identity.spec
                    ),
                    &declared.source_location,
                ));
                continue;
            }
            if names.contains(&field.name) {
                errors.push(self.engine_error(
                    format!("`{}` appears twice in `-> key`", field.name),
                    &declared.source_location,
                ));
                continue;
            }
            let local = DataPath::local(field.name.clone());
            let Some(definition) = row_data.get(&local) else {
                errors.push(self.engine_error(
                    format!(
                        "key field `{}` is not data on `{}`",
                        field.name, identity.spec
                    ),
                    &declared.source_location,
                ));
                continue;
            };
            match definition {
                DataDefinition::Value { .. }
                | DataDefinition::TypeDeclaration { .. }
                | DataDefinition::Reference { .. } => {}
                DataDefinition::Import { .. } | DataDefinition::InstanceList { .. } => {
                    errors.push(self.engine_error(
                        format!(
                            "key field `{}` on `{}` is not a value",
                            field.name, identity.spec
                        ),
                        &declared.source_location,
                    ));
                    continue;
                }
            }
            names.push(field.name.clone());
            paths.push(local);
        }
        if errors.is_empty() {
            Ok((ListKey::Fields(names), paths))
        } else {
            Err(errors)
        }
    }

    fn instance_list_rows(
        &mut self,
        list_path: &DataPath,
        list: &ast::UsesList,
        row_spec: &RowSpec<'_>,
        current_segments: &[PathSegment],
        current_spec: &'a LemmaSpec,
        effective: &EffectiveDate,
    ) -> Vec<SourceRow> {
        let RowSpec { identity, template } = *row_spec;
        let mut rows = Vec::new();
        for row in &list.rows {
            match row {
                ast::UsesListRow::Fields { bindings, .. } => {
                    let mut fields: Vec<(DataPath, DataSource)> =
                        Vec::with_capacity(bindings.len());
                    for binding in bindings {
                        let name = binding.path.to_string();
                        let parts: Vec<&str> = binding
                            .path
                            .segments
                            .iter()
                            .map(String::as_str)
                            .chain(std::iter::once(binding.path.name.as_str()))
                            .collect();
                        let Some(path) = template.qualify_data_names(&parts) else {
                            self.errors.push(self.engine_error(
                                format!("row field `{name}` is not data on `{}`", identity.spec),
                                &binding.source_location,
                            ));
                            continue;
                        };
                        let (body, _, definition) = template
                            .peel_data(&path)
                            .expect("BUG: qualified row field path must peel on template");
                        if fields.iter().any(|(existing, _)| existing == &path) {
                            self.errors.push(self.engine_error(
                                format!("row field `{name}` appears twice in one row"),
                                &binding.source_location,
                            ));
                            continue;
                        }
                        let Some(field_type) = definition.resolved_type_arc() else {
                            self.errors.push(self.engine_error(
                                format!(
                                    "row field `{name}` is a `uses` instance of `{}`; bind its fields with `{name}.<field>`",
                                    identity.spec
                                ),
                                &binding.source_location,
                            ));
                            continue;
                        };
                        let source = match &binding.rhs {
                            WithRhs::Literal(value) => {
                                match source_row_literal(value, field_type, body) {
                                    Ok(bound) => DataSource::Literal(bound),
                                    Err(message) => {
                                        self.errors.push(self.engine_error(
                                            format!("row field `{name}`: {message}"),
                                            &binding.source_location,
                                        ));
                                        continue;
                                    }
                                }
                            }
                            WithRhs::Reference { target } => {
                                match self.resolve_reference_target_in_spec(
                                    target,
                                    &binding.source_location,
                                    current_spec,
                                    current_segments,
                                    effective,
                                ) {
                                    Some(ReferenceTarget::Data(consumer)) => {
                                        let relative = instance_list::owner_relative_reference(
                                            &consumer,
                                            current_segments,
                                        );
                                        let target_type = self
                                            .data
                                            .get(&consumer)
                                            .and_then(DataDefinition::resolved_type_arc);
                                        let Some(target_type) = target_type else {
                                            self.errors.push(self.engine_error(
                                                format!(
                                                    "Data reference for row field `{name}` target `{consumer}` does not exist"
                                                ),
                                                &binding.source_location,
                                            ));
                                            continue;
                                        };
                                        if let Some(msg) = reference_kind_mismatch_message(
                                            field_type.as_ref(),
                                            target_type.as_ref(),
                                            &path,
                                            &relative,
                                            "target",
                                        ) {
                                            self.errors.push(
                                                self.engine_error(msg, &binding.source_location),
                                            );
                                            continue;
                                        }
                                        DataSource::Reference(relative)
                                    }
                                    Some(ReferenceTarget::Rule(rule)) => {
                                        self.errors.push(self.engine_error(
                                            format!(
                                                "row field `{name}` takes a value or data, not rule `{rule}`"
                                            ),
                                            &binding.source_location,
                                        ));
                                        continue;
                                    }
                                    None => continue,
                                }
                            }
                        };
                        fields.push((path, source));
                    }
                    if fields.len() != bindings.len() {
                        continue;
                    }
                    rows.push(SourceRow { fields });
                }
                ast::UsesListRow::Values {
                    values,
                    source_location,
                } => {
                    for value in values {
                        match self.named_instance_row(
                            list_path,
                            value,
                            row_spec,
                            current_segments,
                            current_spec,
                            source_location,
                        ) {
                            Some(row) => rows.push(row),
                            None => continue,
                        }
                    }
                }
            }
        }
        rows
    }

    /// `-> with ada`: a row whose every field follows the `uses ada` instance.
    fn named_instance_row(
        &mut self,
        list_path: &DataPath,
        value: &WithRhs,
        row_spec: &RowSpec<'_>,
        current_segments: &[PathSegment],
        current_spec: &LemmaSpec,
        source: &Source,
    ) -> Option<SourceRow> {
        let RowSpec { identity, template } = *row_spec;
        let target = match value {
            WithRhs::Reference { target } if target.segments.is_empty() => target,
            WithRhs::Reference { target } => {
                self.errors.push(self.engine_error(
                    format!(
                        "row `{target}` of `{}` must name a `uses` instance of `{}` declared in this spec",
                        list_path.data, identity.spec
                    ),
                    source,
                ));
                return None;
            }
            WithRhs::Literal(literal) => {
                self.errors.push(self.engine_error(
                    format!(
                        "row `{literal}` of `{}` is not a `{}` instance; write `-> with <field>: <value>` for an inline row",
                        list_path.data, identity.spec
                    ),
                    source,
                ));
                return None;
            }
        };
        let declared_import = current_spec.data.iter().any(|data| {
            data.reference.is_local()
                && data.reference.name == target.name
                && matches!(data.value, ParsedDataValue::Import { .. })
        });
        if !declared_import {
            self.errors.push(self.engine_error(
                format!(
                    "row `{target}` of `{}` is not a `uses` instance of `{}`",
                    list_path.data, identity.spec
                ),
                source,
            ));
            return None;
        }
        let alias_path = DataPath {
            segments: current_segments.to_vec(),
            data: target.name.clone(),
        };
        let Some(DataDefinition::Import {
            identity: instance_identity,
            ..
        }) = self.data.get(&alias_path)
        else {
            // The import itself failed to resolve and already reported an error.
            return None;
        };
        let instance_identity = instance_identity.clone();
        if instance_identity != *identity {
            self.errors.push(self.engine_error(
                format!(
                    "`{}` is a `{}` instance; rows of `{}` are `{}`",
                    target.name, instance_identity, list_path.data, identity
                ),
                source,
            ));
            return None;
        }
        let mut prefix = current_segments.to_vec();
        prefix.push(PathSegment {
            uses: target.name.clone(),
            repository: instance_identity.repository.clone(),
            spec: instance_identity.spec.clone(),
        });
        let fields = ExecutionPlan::nested_input_keys(template)
            .into_iter()
            .map(|(path, _)| {
                let mut segments = prefix.clone();
                segments.extend(path.segments.iter().cloned());
                let consumer = DataPath {
                    segments,
                    data: path.data.clone(),
                };
                {
                    let first = consumer.segments.first().unwrap_or_else(|| {
                        panic!("BUG: named-instance consumer path '{consumer}' has no uses hop")
                    });
                    let body =
                        first_import_template(&self.data, &self.instance_lists.templates, first)
                            .unwrap_or_else(|| {
                                panic!(
                                    "BUG: instance '{}' of '{identity}' lacks import template",
                                    target.name
                                )
                            });
                    let rest = DataPath {
                        segments: consumer.segments[1..].to_vec(),
                        data: consumer.data.clone(),
                    };
                    body.peel_data(&rest).unwrap_or_else(|| {
                        panic!(
                            "BUG: instance '{}' of '{identity}' lacks template data '{path}'",
                            target.name
                        )
                    });
                }
                // Owner-relative: keep the named-instance hop (`ada.name`), strip only
                // outer nested `uses` segments so materialize can re-prefix them.
                (
                    path,
                    DataSource::Reference(instance_list::owner_relative_reference(
                        &consumer,
                        current_segments,
                    )),
                )
            })
            .collect();
        Some(SourceRow { fields })
    }

    /// Source rows of a keyed list: key values are literal and unique.
    fn check_source_row_keys(
        &mut self,
        key: &ListKey,
        key_paths: &[DataPath],
        rows: &[SourceRow],
        row_spec: &RowSpec<'_>,
        source: &Source,
    ) {
        let ListKey::Fields(fields) = key else {
            return;
        };
        let template = row_spec.template;
        let field_types: Vec<Arc<LemmaType>> = key_paths
            .iter()
            .map(|path| {
                let (_, _, definition) = template
                    .peel_data(path)
                    .expect("BUG: key path must peel on template");
                Arc::clone(
                    definition
                        .resolved_type_arc()
                        .expect("BUG: key path must be typed template data"),
                )
            })
            .collect();
        let mut keyed: Vec<KeyedRow> = Vec::with_capacity(rows.len());
        for (index, row) in rows.iter().enumerate() {
            let mut values = Vec::with_capacity(key_paths.len());
            for ((path, field), field_type) in key_paths.iter().zip(fields).zip(&field_types) {
                let bound = row
                    .fields
                    .iter()
                    .find(|(row_path, _)| row_path == path)
                    .map(|(_, row_source)| row_source);
                match bound {
                    Some(DataSource::Literal(bound)) => {
                        values.push(instance_list::typed_from_bound(bound, field_type))
                    }
                    Some(DataSource::Reference(consumer)) => {
                        self.errors.push(self.engine_error(
                            format!(
                                "key field `{field}` of row {} is `{consumer}`; key values must be written as values",
                                index + 1
                            ),
                            source,
                        ));
                        return;
                    }
                    None => match instance_list::template_default(template, path) {
                        Some(default) => values.push(default),
                        None => {
                            self.errors.push(self.engine_error(
                                format!("row {} has no value for key field `{field}`", index + 1),
                                source,
                            ));
                            return;
                        }
                    },
                }
            }
            keyed.push(KeyedRow {
                label: format!("row {}", index + 1),
                values,
            });
        }
        if let Some(message) = instance_list::key_conflict(fields, &keyed) {
            self.errors.push(self.engine_error(message, source));
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
            match value {
                Value::Range(left, right)
                    if matches!(
                        &schema.specifications,
                        TypeSpecification::MeasureRange { .. }
                            | TypeSpecification::RatioRange { .. }
                    ) =>
                {
                    let element = schema
                        .specifications
                        .element_from_range()
                        .expect("BUG: measure/ratio range must define element_from_range");
                    let resolve = unit_tokens_for_spec(&self.local_types, current_spec);
                    let left_lit = match lift_range_endpoint(left, &element, Some(&resolve)) {
                        Ok(lit) => lit,
                        Err(e) => {
                            self.errors.push(self.engine_error(e, &effective_source));
                            return;
                        }
                    };
                    let right_lit = match lift_range_endpoint(right, &element, Some(&resolve)) {
                        Ok(lit) => lit,
                        Err(e) => {
                            self.errors.push(self.engine_error(e, &effective_source));
                            return;
                        }
                    };
                    ValueKind::Range(Box::new(left_lit), Box::new(right_lit))
                }
                _ => {
                    let resolve = unit_tokens_for_spec(&self.local_types, current_spec);
                    match parser_value_to_value_kind(value, &schema.specifications, Some(&resolve))
                    {
                        Ok(s) => s,
                        Err(e) => {
                            self.errors.push(self.engine_error(e, &effective_source));
                            return;
                        }
                    }
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
                        Value::NumberWithUnit(left_mag, left_unit),
                        Value::NumberWithUnit(right_mag, right_unit),
                    ) => {
                        match self.measure_ratio_range_from_number_with_unit_endpoints(
                            current_spec,
                            *left_mag,
                            left_unit,
                            *right_mag,
                            right_unit,
                        ) {
                            Ok(s) => s,
                            Err(e) => {
                                self.errors.push(self.engine_error(e, &effective_source));
                                return;
                            }
                        }
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
                (Value::NumberWithUnit(_, _), Value::NumberWithUnit(_, _)) => match &semantic_value
                {
                    ValueKind::Range(left, right) => {
                        let specs = range_type_specification_from_endpoints(
                                left.lemma_type.as_ref(),
                                right.lemma_type.as_ref(),
                            )
                            .unwrap_or_else(|| {
                                unreachable!(
                                    "BUG: measure/ratio range endpoints must form a range type after resolve"
                                )
                            });
                        Arc::new(LemmaType::primitive(specs))
                    }
                    _ => unreachable!(
                        "BUG: semantic range literal conversion returned non-range value kind"
                    ),
                },
                _ => match &semantic_value {
                    ValueKind::Range(left, right) => {
                        let endpoint_type = |endpoint: &TypedLiteral| -> Arc<LemmaType> {
                            match &endpoint.value {
                                ValueKind::Number(_) => primitive_number_arc().clone(),
                                ValueKind::Date(_) => primitive_date_arc().clone(),
                                ValueKind::Time(_) => primitive_time_arc().clone(),
                                ValueKind::Ratio(_) => primitive_ratio_arc().clone(),
                                ValueKind::Measure(_) => Arc::clone(&endpoint.lemma_type),
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
                let bare = match self.resolve_unit_ref(current_spec, unit) {
                    Ok((bare, _)) => bare,
                    Err(message) => {
                        self.errors
                            .push(self.engine_error(message, &effective_source));
                        return;
                    }
                };
                Arc::new(schema_type.as_ref().clone().with_unit(bare))
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

    /// Ensure the import at `import_path` has a shared template and site overlay.
    ///
    /// `at` is the planning instant for this import edge (unpinned transitive resolution).
    /// Binding keys under the import alias become overlay entries (local to the template);
    /// missing template slots are planning Errors.
    fn ensure_import_template_and_overlay(
        &mut self,
        import_path: &DataPath,
        current_segment_names: &[String],
        bindings: &DataBindings,
        used_binding_keys: &mut HashSet<Vec<String>>,
        at: &EffectiveDate,
    ) {
        if current_segment_names.len() + 1 > self.limits.max_spec_dependency_depth {
            self.errors.push(Error::resource_limit_exceeded(
                "max_spec_dependency_depth",
                self.limits.max_spec_dependency_depth.to_string(),
                (current_segment_names.len() + 1).to_string(),
                format!(
                    "Import '{}' exceeds the maximum import nesting depth; flatten the import chain",
                    import_path.input_key()
                ),
                None,
                Some(self.main_spec),
                None,
            ));
            return;
        }

        let Some(DataDefinition::Import {
            identity, source, ..
        }) = self.data.get(import_path)
        else {
            return;
        };
        let identity = identity.clone();
        let import_source = source.clone();

        let repo = match &identity.repository {
            None => self.context.workspace(),
            Some(name) => match self.context.find_repository(name) {
                Some(repo) => repo,
                None => {
                    self.errors.push(self.engine_error(
                        format!(
                            "repository '{name}' not loaded for import '{}'",
                            identity.spec
                        ),
                        &import_source,
                    ));
                    return;
                }
            },
        };
        let version_from = match &identity.effective {
            EffectiveDate::Origin => None,
            EffectiveDate::DateTimeValue(datetime) => Some(datetime),
        };
        let Some(nested_spec) = self
            .context
            .spec_set(&repo, &identity.spec)
            .and_then(|set| set.get_exact(version_from))
        else {
            self.errors.push(self.engine_error(
                format!(
                    "spec '{}' at {} not loaded for import",
                    identity.spec, identity.effective
                ),
                &import_source,
            ));
            return;
        };

        let Some(template) = self.row_template(&identity, &repo, nested_spec, at) else {
            return;
        };

        self.project_instance_lists_from_template(import_path, &identity, Arc::clone(&template));

        let mut alias_key = current_segment_names.to_vec();
        alias_key.push(import_path.data.clone());

        let mut overlay = IndexMap::new();
        for (key, (value, binding_source)) in bindings {
            if key.len() <= alias_key.len() || key[..alias_key.len()] != alias_key[..] {
                continue;
            }
            let relative: Vec<&str> = key[alias_key.len()..].iter().map(String::as_str).collect();
            let Some(overlay_path) = template.qualify_data_names(&relative) else {
                self.errors.push(self.engine_error(
                    format!(
                        "No declared data matches with or binding for '{}'",
                        key.join(".")
                    ),
                    binding_source,
                ));
                used_binding_keys.insert(key.clone());
                continue;
            };
            let (body, _, definition) = template
                .peel_data(&overlay_path)
                .expect("BUG: qualified overlay path must peel on template");
            if matches!(definition, DataDefinition::Import { .. }) {
                self.errors.push(self.engine_error(
                    format!(
                        "cannot bind import alias '{}' as a value",
                        overlay_path.input_key()
                    ),
                    binding_source,
                ));
                used_binding_keys.insert(key.clone());
                continue;
            }
            if matches!(definition, DataDefinition::InstanceList { .. }) {
                self.errors.push(self.engine_error(
                    format!(
                        "binding `{}` replaces instance list `{}`; supply the whole list through evaluation data",
                        key.join("."),
                        overlay_path.input_key()
                    ),
                    binding_source,
                ));
                used_binding_keys.insert(key.clone());
                continue;
            }
            let overlay_entry = match value {
                BindingValue::Literal(raw) => {
                    let Some(field_type) = definition.resolved_type_arc() else {
                        panic!(
                            "BUG: template data '{}' must be typed for literal overlay",
                            overlay_path.input_key()
                        );
                    };
                    match source_row_literal(raw, field_type, body) {
                        Ok(bound) => ImportOverlay::Literal(bound),
                        Err(message) => {
                            self.errors.push(self.engine_error(message, binding_source));
                            used_binding_keys.insert(key.clone());
                            continue;
                        }
                    }
                }
                BindingValue::Reference {
                    target,
                    constraints,
                } => ImportOverlay::Reference {
                    target: target.clone(),
                    constraints: constraints.clone(),
                },
            };
            overlay.insert(overlay_path, overlay_entry);
            used_binding_keys.insert(key.clone());
        }

        let Some(DataDefinition::Import { overlay: slot, .. }) = self.data.get_mut(import_path)
        else {
            panic!(
                "BUG: import '{}' missing after add_data",
                import_path.input_key()
            );
        };
        *slot = overlay;
    }

    /// Copy template instance lists onto this consumer under the import alias prefix.
    /// Rows stay `Arc`-shared; run data never mutates them. Element row plans are
    /// absorbed through [`Self::record_template`] so `resolve_instance_list` can
    /// address dotted keys on this consumer.
    fn project_instance_lists_from_template(
        &mut self,
        import_path: &DataPath,
        identity: &SpecIdentity,
        template: Arc<ExecutionPlan>,
    ) {
        self.record_template(identity.clone(), Arc::clone(&template));
        let mut prefix = import_path.segments.clone();
        prefix.push(PathSegment {
            uses: import_path.data.clone(),
            repository: identity.repository.clone(),
            spec: identity.spec.clone(),
        });
        for (relative, source) in &template.instance_lists {
            let absolute = DataPath {
                segments: {
                    let mut segments = prefix.clone();
                    segments.extend(relative.segments.iter().cloned());
                    segments
                },
                data: relative.data.clone(),
            };
            if let Some(existing) = self.instance_lists.sources.get(&absolute) {
                assert!(
                    Arc::ptr_eq(&existing.rows, &source.rows),
                    "BUG: projected list '{}' must share the template row Arc",
                    absolute.input_key()
                );
                continue;
            }
            let definition = template.data.get(relative).unwrap_or_else(|| {
                panic!(
                    "BUG: template instance list '{}' missing from template data",
                    relative.input_key()
                )
            });
            let DataDefinition::InstanceList {
                resolved_type,
                source: def_source,
            } = definition
            else {
                panic!(
                    "BUG: template instance list '{}' is not DataDefinition::InstanceList",
                    relative.input_key()
                );
            };
            if !self.instance_lists.templates.contains_key(&source.identity) {
                if source.identity == template.identity {
                    self.instance_lists
                        .templates
                        .insert(source.identity.clone(), Arc::clone(&template));
                } else {
                    let row_plan = template.row_templates.get(&source.identity).unwrap_or_else(|| {
                        panic!(
                            "BUG: projected list '{}' element '{}' missing from template row_templates",
                            relative.input_key(),
                            source.identity
                        )
                    });
                    self.record_template(source.identity.clone(), Arc::clone(row_plan));
                }
            }
            self.data.insert(
                absolute.clone(),
                DataDefinition::InstanceList {
                    resolved_type: Arc::clone(resolved_type),
                    source: def_source.clone(),
                },
            );
            self.instance_lists.sources.insert(
                absolute,
                InstanceListSource {
                    identity: source.identity.clone(),
                    key: source.key.clone(),
                    rows: Arc::clone(&source.rows),
                },
            );
        }
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
                        local_value: None,
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
                let context_repository = discovery::owning_repository(self.context, spec_context);
                let (repo, arc) = match self.resolve_spec_ref(
                    spec_ref,
                    effective,
                    spec_context,
                    &context_repository,
                ) {
                    Ok(pair) => pair,
                    Err(e) => {
                        self.errors.push(e);
                        return None;
                    }
                };
                path_segments.push(PathSegment {
                    uses: segment.clone(),
                    repository: repo.name.clone(),
                    spec: arc.name.clone(),
                });
                current_data_map = arc
                    .data
                    .iter()
                    .map(|f| (f.reference.name.clone(), f.clone()))
                    .collect();
                last_arc = Some(arc);
                spec_context = arc;
            } else if let ParsedDataValue::ImportList { .. } = &data_ref.value {
                self.errors.push(self.engine_error(
                    format!(
                        "`{segment}` is a list: there is no `{segment}.<field>` outside a command argument; \
                         pick a row first, for example `({segment} -> first).<field>`"
                    ),
                    reference_source,
                ));
                return None;
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
        let root = match self.enter_spec_types(spec, spec_repository, effective, type_resolver) {
            Ok(frame) => frame,
            Err(resolved) => return resolved,
        };
        let mut stack = vec![root];
        loop {
            let mut frame = stack
                .pop()
                .expect("BUG: spec types walk ends when its root frame finishes");
            if let Some((child, child_repository, child_effective)) =
                self.next_spec_import(&mut frame)
            {
                stack.push(frame);
                if let Ok(child_frame) =
                    self.enter_spec_types(child, &child_repository, &child_effective, type_resolver)
                {
                    stack.push(child_frame);
                }
                continue;
            }
            let resolved = self.finish_spec_types(&frame, type_resolver);
            if stack.is_empty() {
                return resolved;
            }
        }
    }

    /// Entry of [`Self::ensure_spec_types_resolved`]: `Err` carries the result when there
    /// is nothing to walk.
    fn enter_spec_types(
        &self,
        spec: &'a LemmaSpec,
        spec_repository: &Arc<LemmaRepository>,
        effective: &EffectiveDate,
        type_resolver: &TypeResolver<'a>,
    ) -> Result<SpecTypesFrame<'a>, bool> {
        if self
            .local_types
            .iter()
            .any(|(_, s, _)| discovery::same_loaded_spec(s, spec))
        {
            return Err(true);
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
            return Err(false);
        }
        Ok(SpecTypesFrame {
            spec,
            repository: Arc::clone(spec_repository),
            effective: effective.clone(),
            next_data: 0,
        })
    }

    /// Next import of `frame`'s spec that must be entered. Import errors are recorded and
    /// skipped.
    fn next_spec_import(
        &mut self,
        frame: &mut SpecTypesFrame<'a>,
    ) -> Option<(&'a LemmaSpec, Arc<LemmaRepository>, EffectiveDate)> {
        let spec = frame.spec;
        while let Some(data) = spec.data.get(frame.next_data) {
            frame.next_data += 1;
            if discovery::is_self_list(
                self.context,
                data,
                spec,
                &frame.repository,
                &frame.effective,
            ) {
                continue;
            }
            match &data.value {
                ParsedDataValue::Import { spec_ref, .. }
                | ParsedDataValue::ImportList { spec_ref, .. } => {
                    let import_effective = spec_ref.at(&frame.effective);
                    match self.resolve_spec_ref(
                        spec_ref,
                        &import_effective,
                        spec,
                        &frame.repository,
                    ) {
                        Ok((source_repo, source_arc)) => {
                            return Some((source_arc, source_repo, import_effective));
                        }
                        Err(error) => self.errors.push(error),
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// Resolves and records the types of `frame`'s spec once all its imports are recorded.
    fn finish_spec_types(
        &mut self,
        frame: &SpecTypesFrame<'a>,
        type_resolver: &TypeResolver<'a>,
    ) -> bool {
        match type_resolver.resolve_and_validate(frame.spec, &frame.effective, &self.local_types) {
            Ok(resolved_types) => {
                self.local_types
                    .push((Arc::clone(&frame.repository), frame.spec, resolved_types));
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
            current_segments.iter().map(|s| s.uses.clone()).collect();

        // Step 2: Build data bindings declared in this spec (for passing to referenced specs)
        let this_spec_bindings = match self.build_data_bindings(spec, &current_segments, effective)
        {
            Ok(bindings) => bindings,
            Err(errors) => {
                self.errors.extend(errors);
                IndexMap::new()
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
            if data.value.uses_spec_ref().is_some() {
                continue;
            }
            self.add_data(data, &current_segments, spec, effective);
        }
        self.apply_bindings_in_source_order(
            spec,
            &current_segments,
            &effective_bindings,
            &mut used_binding_keys,
        );

        for data in &spec.data {
            if !data.reference.segments.is_empty() {
                continue;
            }
            if let ParsedDataValue::Import { spec_ref, .. } = &data.value {
                let nested_effective = spec_ref.at(effective);
                self.add_data(data, &current_segments, spec, effective);
                let import_path = DataPath {
                    segments: current_segments.to_vec(),
                    data: data.reference.name.clone(),
                };
                // Merge parent bindings that target this nest, then apply as overlay on the Import.
                let mut nest_key = current_segment_names.clone();
                nest_key.push(data.reference.name.clone());
                let mut combined_bindings = effective_bindings.clone();
                for (key, value_and_source) in &data_bindings {
                    if key.len() > nest_key.len()
                        && key[..nest_key.len()] == nest_key[..]
                        && !combined_bindings.contains_key(key)
                    {
                        combined_bindings.insert(key.clone(), value_and_source.clone());
                    }
                }
                self.ensure_import_template_and_overlay(
                    &import_path,
                    &current_segment_names,
                    &combined_bindings,
                    &mut used_binding_keys,
                    &nested_effective,
                );
            }
        }

        for data in &spec.data {
            if data.reference.is_local()
                && matches!(&data.value, ParsedDataValue::ImportList { .. })
            {
                self.add_data(data, &current_segments, spec, effective);
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

        let piecewise_nodes = if rule.unless_clauses.is_empty() { 0 } else { 2 };
        let ast_nodes = rule.expression.table.len()
            + rule
                .unless_clauses
                .iter()
                .map(|clause| clause.condition.table.len() + clause.result.table.len())
                .sum::<usize>();
        let mut depends_on_rules = BTreeSet::new();
        let mut convert_ctx = RuleExpressionConversion {
            table: ExprTable::with_capacity(ast_nodes + piecewise_nodes),
            spec: current_spec,
            data_map,
            segments: current_segments,
            rule_names,
            rule: &rule.name,
            effective,
            depends_on_rules: &mut depends_on_rules,
            row_scopes: Vec::new(),
            reads_itself_on_rows: false,
        };

        let mut scratch = ConvertScratch {
            frames: Vec::with_capacity(ast_nodes),
            values: Vec::with_capacity(ast_nodes),
            children: Vec::new(),
        };
        let errors_before = self.errors.len();
        let Some(default_result) =
            self.convert_ast_expression(rule.expression.root(), &mut convert_ctx, &mut scratch)
        else {
            self.drop_unconverted_rule(rule_path, errors_before);
            return;
        };
        let mut branches = vec![(None, default_result)];
        let default_source = convert_ctx.table.source(default_result).cloned();
        let mut arms = Vec::with_capacity(rule.unless_clauses.len() + 1);
        if !rule.unless_clauses.is_empty() {
            let default_condition = convert_ctx.push(
                ExpressionKind::Literal(Box::new(TypedLiteral::from_bool(true))),
                default_source.clone(),
            );
            arms.push((default_condition, default_result));
        }
        for unless_clause in &rule.unless_clauses {
            let Some(condition) = self.convert_ast_expression(
                unless_clause.condition.root(),
                &mut convert_ctx,
                &mut scratch,
            ) else {
                self.drop_unconverted_rule(rule_path, errors_before);
                return;
            };
            let Some(result) = self.convert_ast_expression(
                unless_clause.result.root(),
                &mut convert_ctx,
                &mut scratch,
            ) else {
                self.drop_unconverted_rule(rule_path, errors_before);
                return;
            };
            branches.push((Some(condition), result));
            arms.push((condition, result));
        }
        let root = if arms.is_empty() {
            default_result
        } else {
            convert_ctx.push(ExpressionKind::Piecewise(arms), default_source)
        };

        let recursive = convert_ctx.reads_itself_on_rows;
        let expression = ExprTree::new(std::mem::take(&mut convert_ctx.table), root);
        let rule_node = RuleNode {
            expression,
            branches,
            source: rule.source_location.clone(),
            depends_on_rules,
            recursive,
            rule_type: Arc::new(LemmaType::veto_type()),
            spec: current_spec,
        };

        self.rules.insert(rule_path, rule_node);
    }

    fn drop_unconverted_rule(&mut self, rule_path: RulePath, errors_before: usize) {
        if self.errors.len() <= errors_before {
            panic!("BUG: add_rule conversion failed without an error");
        }
        self.dropped_rules.insert(rule_path);
    }

    /// Converts the subtree at `expr` into `ctx.table`, children before parents, with an
    /// explicit stack. List command arguments are converted in their row scope before the
    /// command is built, see [`ConvertFrame`].
    fn convert_ast_expression<'t>(
        &mut self,
        expr: AstRef<'t>,
        ctx: &mut RuleExpressionConversion<'a, '_>,
        scratch: &mut ConvertScratch<'t>,
    ) -> Option<ExprId> {
        let ConvertScratch {
            frames,
            values,
            children,
        } = scratch;
        frames.push(ConvertFrame::Enter(expr));
        while let Some(frame) = frames.pop() {
            match frame {
                ConvertFrame::Enter(node) => {
                    frames.push(ConvertFrame::Node(node));
                    let start = frames.len();
                    for_each_conversion_child(node.kind(), |child| {
                        frames.push(ConvertFrame::Enter(node.child(child)));
                    });
                    frames[start..].reverse();
                }
                ConvertFrame::Node(node) => {
                    let mut arity = 0;
                    for_each_conversion_child(node.kind(), |_| arity += 1);
                    children.clear();
                    let mut converted = true;
                    for value in values.drain(values.len() - arity..) {
                        match value {
                            Some(id) => children.push(id),
                            None => converted = false,
                        }
                    }
                    if !converted {
                        values.push(None);
                    } else if let ast::ExpressionKind::ListCommand { .. } = node.kind() {
                        self.start_list_command(node, one(children), ctx, frames);
                    } else {
                        values.push(self.convert_ast_node(node, children, ctx));
                    }
                }
                ConvertFrame::List(build) => {
                    self.advance_list_command(build, frames, values, ctx);
                }
                ConvertFrame::ListArgument(mut build, sink) => {
                    let converted = values
                        .pop()
                        .expect("BUG: list argument is converted before its frame resumes");
                    match (sink, converted) {
                        (ListArgumentSink::Positional, Some(id)) => build.converted.push(id),
                        (ListArgumentSink::Positional, None) => {
                            values.push(None);
                            continue;
                        }
                        (ListArgumentSink::Field(path), Some(id)) => build.fields.push((path, id)),
                        (ListArgumentSink::Field(_), None) => build.failed = true,
                    }
                    frames.push(ConvertFrame::List(build));
                }
                ConvertFrame::PopRowScope => {
                    ctx.row_scopes.pop();
                }
            }
        }
        let converted = values
            .pop()
            .expect("BUG: conversion leaves one result for its root");
        assert!(
            values.is_empty(),
            "BUG: conversion left operands without a parent"
        );
        converted
    }

    /// Pushes the frame that converts the arguments of the list command at `node`, whose list
    /// operand converted to `list`.
    fn start_list_command<'t>(
        &self,
        node: AstRef<'t>,
        list: ExprId,
        ctx: &RuleExpressionConversion<'a, '_>,
        frames: &mut Vec<ConvertFrame<'t>>,
    ) {
        let ast::ExpressionKind::ListCommand {
            list: list_source,
            command,
        } = node.kind()
        else {
            unreachable!("BUG: list command frame started for a non-list-command node")
        };
        let source = node
            .source()
            .expect("BUG: AST expression missing source location")
            .clone();
        let row_scope = self
            .instance_list_identity(
                ExprRef {
                    table: &ctx.table,
                    id: list,
                },
                ctx.spec,
                ctx.segments,
            )
            .map(|identity| {
                let template = self.row_template_of(&identity);
                let list_path = match node.child(*list_source).kind() {
                    ast::ExpressionKind::Reference(reference) => Some(reference.full_path()),
                    _ => None,
                };
                RowScope {
                    list_path,
                    identity,
                    template,
                    level: u32::try_from(ctx.row_scopes.len())
                        .expect("BUG: row scope depth exceeds u32"),
                }
            });
        let row_level = row_scope.as_ref().map(|scope| scope.level);
        let pending = list_command_arguments(command, &row_scope);
        frames.push(ConvertFrame::List(Box::new(ListCommandBuild {
            node,
            source,
            list,
            row_level,
            row_scope,
            pending,
            converted: Vec::new(),
            fields: Vec::new(),
            failed: false,
        })));
    }

    /// Converts the next argument of a list command, or builds the command once all are done.
    fn advance_list_command<'t>(
        &mut self,
        mut build: Box<ListCommandBuild<'t>>,
        frames: &mut Vec<ConvertFrame<'t>>,
        values: &mut Vec<Option<ExprId>>,
        ctx: &mut RuleExpressionConversion<'a, '_>,
    ) {
        let node = build.node;
        let Some(argument) = build.pending.pop_front() else {
            let ast::ExpressionKind::ListCommand { command, .. } = node.kind() else {
                unreachable!("BUG: list command frame holds a list command node")
            };
            let op = self.finish_list_command(
                command,
                &build.row_scope,
                std::mem::take(&mut build.converted),
                std::mem::take(&mut build.fields),
                build.failed,
                &build.source,
            );
            let source = Some(build.source.clone());
            let converted = op.map(|op| {
                ctx.push(
                    ExpressionKind::ListCommand {
                        list: build.list,
                        op,
                        row_level: build.row_level,
                    },
                    source,
                )
            });
            values.push(converted);
            return;
        };
        match argument {
            ListArgument::Enclosing(child) => {
                frames.push(ConvertFrame::ListArgument(
                    build,
                    ListArgumentSink::Positional,
                ));
                frames.push(ConvertFrame::Enter(node.child(child)));
            }
            ListArgument::InRows(child) => {
                let scoped = build.row_scope.clone();
                frames.push(ConvertFrame::ListArgument(
                    build,
                    ListArgumentSink::Positional,
                ));
                if let Some(scope) = scoped {
                    ctx.row_scopes.push(scope);
                    frames.push(ConvertFrame::PopRowScope);
                }
                frames.push(ConvertFrame::Enter(node.child(child)));
            }
            ListArgument::Field { name, value } => match self.row_field_path(&build, &name) {
                Some(path) => {
                    frames.push(ConvertFrame::ListArgument(
                        build,
                        ListArgumentSink::Field(path),
                    ));
                    frames.push(ConvertFrame::Enter(node.child(value)));
                }
                None => {
                    build.failed = true;
                    frames.push(ConvertFrame::List(build));
                }
            },
        }
    }

    fn convert_ast_node(
        &mut self,
        expr: AstRef<'_>,
        children: &[ExprId],
        ctx: &mut RuleExpressionConversion<'a, '_>,
    ) -> Option<ExprId> {
        let expr_src = expr
            .source()
            .expect("BUG: AST expression missing source location");
        let source = expr.source().cloned();
        match expr.kind() {
            ast::ExpressionKind::Reference(r) => {
                let expr_source = expr_src;
                if !ctx.row_scopes.is_empty() {
                    match self.resolve_row_field(r, expr_source, ctx) {
                        RowFieldLookup::Field(kind) => {
                            if let ExpressionKind::RowField { identity, end, .. } = &kind {
                                self.note_row_end_read(identity, end, ctx);
                            }
                            return Some(ctx.push(kind, source.clone()));
                        }
                        RowFieldLookup::Failed => return None,
                        RowFieldLookup::NotARowField => {}
                    }
                }
                if let Some((head, rest)) = r.segments.split_first() {
                    if ctx.rule_names.contains(head.as_str()) && !ctx.data_map.contains_key(head) {
                        let base = RulePath {
                            segments: ctx.segments.to_vec(),
                            rule: head.clone(),
                        };
                        ctx.depends_on_rules.insert(base.clone());
                        let base = ctx.push(ExpressionKind::RulePath(base), source.clone());
                        self.note_row_member_read(base, rest, &r.name, ctx);
                        return Some(ctx.push(
                            ExpressionKind::SpecMember {
                                base,
                                path: rest.to_vec(),
                                name: r.name.clone(),
                                targets: Vec::new(),
                            },
                            source.clone(),
                        ));
                    }
                }
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
                    if let Some(DataDefinition::Import { identity, .. }) = self.data.get(&data_path)
                    {
                        let mut prefix = data_path.segments.clone();
                        prefix.push(PathSegment {
                            uses: data_path.data.clone(),
                            repository: identity.repository.clone(),
                            spec: identity.spec.clone(),
                        });
                        let typed = TypedLiteral {
                            value: ValueKind::Spec(SpecValue::Fixed(SpecInstance { prefix })),
                            lemma_type: Arc::new(LemmaType::primitive(TypeSpecification::Spec {
                                spec: identity.clone(),
                            })),
                        };
                        return Some(
                            ctx.push(ExpressionKind::Literal(Box::new(typed)), source.clone()),
                        );
                    }
                    return Some(ctx.push(ExpressionKind::DataPath(data_path), source.clone()));
                }
                if is_rule {
                    let rule_path = RulePath {
                        segments,
                        rule: r.name.clone(),
                    };
                    ctx.depends_on_rules.insert(rule_path.clone());
                    return Some(ctx.push(ExpressionKind::RulePath(rule_path), source.clone()));
                }
                let msg = match target_spec_name_opt {
                    Some(s) => format!("Reference '{}' not found in spec '{}'", r.name, s),
                    None => format!("Reference '{}' not found", r.name),
                };
                self.errors.push(self.engine_error(msg, expr_source));
                None
            }

            ast::ExpressionKind::LogicalAnd { steps, .. } => {
                let (first, steps) = convert_chain_steps(steps, children);
                Some(ctx.push(ExpressionKind::LogicalAnd { first, steps }, source))
            }

            ast::ExpressionKind::Arithmetic { steps, .. } => {
                let (first, steps) = convert_chain_steps(steps, children);
                Some(ctx.push(ExpressionKind::Arithmetic { first, steps }, source))
            }

            ast::ExpressionKind::Comparison(_, op, _) => {
                let (left, right) = two(children);
                Some(ctx.push(ExpressionKind::Comparison(left, op.clone(), right), source))
            }

            ast::ExpressionKind::UnitConversion(_, target) => {
                let converted_value = one(children);

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
                                expr.source().cloned(),
                                None::<String>,
                                Some(self.main_spec),
                                None,
                            ));
                            return None;
                        }
                    };

                Some(ctx.push(
                    ExpressionKind::UnitConversion(converted_value, semantic_target),
                    source,
                ))
            }

            ast::ExpressionKind::LogicalNegation(_, neg_type) => {
                let converted_operand = one(children);
                Some(ctx.push(
                    ExpressionKind::LogicalNegation(converted_operand, neg_type.clone()),
                    source,
                ))
            }

            ast::ExpressionKind::MathematicalComputation(op, _) => {
                let converted_operand = one(children);
                Some(ctx.push(
                    ExpressionKind::MathematicalComputation(op.clone(), converted_operand),
                    source,
                ))
            }

            ast::ExpressionKind::RangeBound(bound, _) => {
                let converted_operand = one(children);
                Some(ctx.push(
                    ExpressionKind::RangeBound(*bound, converted_operand),
                    source,
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
                            Value::NumberWithUnit(left_mag, left_unit),
                            Value::NumberWithUnit(right_mag, right_unit),
                        ) => {
                            match self.measure_ratio_range_from_number_with_unit_endpoints(
                                ctx.spec, *left_mag, left_unit, *right_mag, right_unit,
                            ) {
                                Ok(v) => v,
                                Err(e) => {
                                    self.errors.push(self.engine_error(e, expr_src));
                                    return None;
                                }
                            }
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
                        Ok((bare, lt)) => match &semantic_value {
                            ValueKind::Measure(_) | ValueKind::Ratio(_) => {
                                Arc::new(lt.as_ref().clone().with_unit(bare))
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
                        (Value::NumberWithUnit(_, _), Value::NumberWithUnit(_, _)) => {
                            match &semantic_value {
                                ValueKind::Range(left, right) => {
                                    let specs = range_type_specification_from_endpoints(
                                    left.lemma_type.as_ref(),
                                    right.lemma_type.as_ref(),
                                )
                                .unwrap_or_else(|| {
                                    unreachable!(
                                        "BUG: measure/ratio range endpoints must form a range type after resolve"
                                    )
                                });
                                    Arc::new(LemmaType::primitive(specs))
                                }
                                _ => unreachable!(
                                    "BUG: semantic range literal conversion returned non-range value kind"
                                ),
                            }
                        }
                        _ => match &semantic_value {
                            ValueKind::Range(left, right) => {
                                let endpoint_type = |endpoint: &TypedLiteral| -> Arc<LemmaType> {
                                    match &endpoint.value {
                                        ValueKind::Number(_) => primitive_number_arc().clone(),
                                        ValueKind::Date(_) => primitive_date_arc().clone(),
                                        ValueKind::Time(_) => primitive_time_arc().clone(),
                                        ValueKind::Ratio(_) => {
                                            semantics::primitive_ratio_arc().clone()
                                        }
                                        ValueKind::Measure(_) => Arc::clone(&endpoint.lemma_type),
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
                Some(ctx.push(
                    ExpressionKind::Literal(Box::new(literal_value)),
                    source.clone(),
                ))
            }

            ast::ExpressionKind::Veto(veto_expression) => {
                Some(ctx.push(ExpressionKind::Veto(veto_expression.clone()), source))
            }

            ast::ExpressionKind::ResultIsVeto(_) => {
                let converted = one(children);
                Some(ctx.push(ExpressionKind::ResultIsVeto(converted), source))
            }

            ast::ExpressionKind::Now => Some(ctx.push(ExpressionKind::Now, source)),

            ast::ExpressionKind::DateRelative(kind, _) => {
                let converted_date = one(children);
                Some(ctx.push(ExpressionKind::DateRelative(*kind, converted_date), source))
            }

            ast::ExpressionKind::DateCalendar(kind, unit, _) => {
                let converted_date = one(children);
                Some(ctx.push(
                    ExpressionKind::DateCalendar(*kind, *unit, converted_date),
                    source,
                ))
            }

            ast::ExpressionKind::RangeLiteral(..) => {
                let (left, right) = two(children);
                Some(ctx.push(ExpressionKind::RangeLiteral(left, right), source))
            }

            ast::ExpressionKind::PastFutureRange(kind, _) => {
                let converted_offset = one(children);
                Some(ctx.push(
                    ExpressionKind::PastFutureRange(*kind, converted_offset),
                    source,
                ))
            }

            ast::ExpressionKind::RangeContainment(..) => {
                let (converted_value, converted_range) = two(children);
                Some(ctx.push(
                    ExpressionKind::RangeContainment(converted_value, converted_range),
                    source,
                ))
            }

            ast::ExpressionKind::ListType { element } => {
                self.convert_list_type_expression(element, expr, ctx)
            }

            ast::ExpressionKind::ListCommand { .. } => {
                unreachable!("BUG: list commands are converted by convert_ast_expression")
            }

            ast::ExpressionKind::Member { path, .. } => {
                let converted_base = one(children);
                self.note_row_member_read(converted_base, &path.segments, &path.name, ctx);
                Some(ctx.push(
                    ExpressionKind::SpecMember {
                        base: converted_base,
                        path: path.segments.clone(),
                        name: path.name.clone(),
                        targets: Vec::new(),
                    },
                    source,
                ))
            }
        }
    }

    /// Row spec of the rows a converted list expression yields, when it is an instance list.
    ///
    /// `spec` at `segments` is the spec being converted: its rules that are not converted
    /// yet are read from source.
    fn instance_list_identity(
        &self,
        list: ExprRef<'_, ExpressionKind>,
        spec: &LemmaSpec,
        segments: &[PathSegment],
    ) -> Option<SpecIdentity> {
        let scope = SourceScope { spec, segments };
        self.instance_list_identity_guarded(list, &scope, &mut BTreeSet::new())
    }

    /// Depth-first search over pipeline commands, rule bodies and conditional arms, with an
    /// explicit work stack. The first arm that yields an identity wins, in source order.
    fn instance_list_identity_guarded(
        &self,
        list: ExprRef<'_, ExpressionKind>,
        scope: &SourceScope<'_>,
        visiting: &mut BTreeSet<RulePath>,
    ) -> Option<SpecIdentity> {
        self.instance_list_identity_walk(vec![Pending::Sem(list)], scope, visiting)
    }

    fn instance_list_identity_walk<'e>(
        &'e self,
        mut pending: Vec<Pending<'e>>,
        scope: &SourceScope<'e>,
        visiting: &mut BTreeSet<RulePath>,
    ) -> Option<SpecIdentity> {
        while let Some(item) = pending.pop() {
            let identity = match item {
                Pending::Sem(list) => match list.kind() {
                    ExpressionKind::DataPath(path) => self.instance_list_data_identity(path),
                    ExpressionKind::ListCommand {
                        list: inner, op, ..
                    } => match op {
                        ListOp::Predicate {
                            kind: ListPredKind::Where,
                            ..
                        }
                        | ListOp::InstancePredicate {
                            kind: ListPredKind::Where,
                            ..
                        }
                        | ListOp::Sort { .. }
                        | ListOp::Top { .. }
                        | ListOp::With { .. }
                        | ListOp::WithRow { .. } => {
                            pending.push(Pending::Sem(list.child(*inner)));
                            None
                        }
                        ListOp::Predicate { .. }
                        | ListOp::InstancePredicate { .. }
                        | ListOp::First
                        | ListOp::Last
                        | ListOp::Count
                        | ListOp::Find { .. }
                        | ListOp::Reduce { .. } => None,
                    },
                    ExpressionKind::RulePath(rule) => {
                        pending.push(Pending::Rule(rule.clone()));
                        None
                    }
                    ExpressionKind::Piecewise(arms) => {
                        for (_, result) in arms.iter().rev() {
                            pending.push(Pending::Sem(list.child(*result)));
                        }
                        None
                    }
                    _ => None,
                },
                Pending::Ast(expr) => match expr.kind() {
                    ast::ExpressionKind::Reference(reference) if reference.segments.is_empty() => {
                        let is_data = scope.spec.data.iter().any(|data| {
                            data.reference.is_local() && data.reference.name == reference.name
                        });
                        if is_data {
                            self.instance_list_data_identity(&DataPath {
                                segments: scope.segments.to_vec(),
                                data: reference.name.clone(),
                            })
                        } else {
                            pending.push(Pending::Rule(RulePath {
                                segments: scope.segments.to_vec(),
                                rule: reference.name.clone(),
                            }));
                            None
                        }
                    }
                    ast::ExpressionKind::ListCommand {
                        list,
                        command:
                            ast::ListCommand::Where { .. }
                            | ast::ListCommand::Sort { .. }
                            | ast::ListCommand::Top { .. }
                            | ast::ListCommand::With { .. },
                    } => {
                        pending.push(Pending::Ast(expr.child(*list)));
                        None
                    }
                    _ => None,
                },
                Pending::Rule(rule) => {
                    if !visiting.insert(rule.clone()) {
                        None
                    } else if let Some(node) = self.rules.get(&rule) {
                        if let Some((_, body)) = node.branches.first() {
                            pending.push(Pending::Sem(node.expression.at(*body)));
                        }
                        None
                    } else if rule.segments.as_slice() != scope.segments {
                        None
                    } else {
                        if let Some(source_rule) =
                            scope.spec.rules.iter().find(|r| r.name == rule.rule)
                        {
                            pending.push(Pending::Ast(source_rule.expression.root()));
                        }
                        None
                    }
                }
            };
            if identity.is_some() {
                return identity;
            }
        }
        None
    }

    fn instance_list_data_identity(&self, path: &DataPath) -> Option<SpecIdentity> {
        let definition = self.data.get(path)?;
        match &definition.resolved_type_arc()?.specifications {
            TypeSpecification::List { element, .. } => match &element.specifications {
                TypeSpecification::Spec { spec } => Some(spec.clone()),
                _ => None,
            },
            _ => None,
        }
    }

    /// `base.path.name` where `base` may be a row of a self-list: a rule read there is
    /// recursion or an ordering edge, as for [`ExpressionKind::RowField`].
    fn note_row_member_read(
        &self,
        base: ExprId,
        path: &[String],
        name: &str,
        ctx: &mut RuleExpressionConversion<'a, '_>,
    ) {
        let scope = SourceScope {
            spec: ctx.spec,
            segments: ctx.segments,
        };
        let base = ExprRef {
            table: &ctx.table,
            id: base,
        };
        let Some(identity) = self.row_value_identity(base, &scope, &mut BTreeSet::new()) else {
            return;
        };
        if identity != self.instance_lists.self_identity {
            return;
        }
        let mut key = path.join(".");
        if !key.is_empty() {
            key.push('.');
        }
        key.push_str(name);
        if let Some(end) = self.template_row_end(&RowTemplate::Root, &key) {
            self.note_row_end_read(&identity, &end, ctx);
        }
    }

    /// A row field read of a self-list: Rule ends and data that reference a rule
    /// both order typing and mark recursion.
    fn note_row_end_read(
        &self,
        identity: &SpecIdentity,
        end: &RowEnd,
        ctx: &mut RuleExpressionConversion<'a, '_>,
    ) {
        match end {
            RowEnd::Rule(rule) => self.note_row_rule_read(identity, rule, ctx),
            RowEnd::Data(path) => {
                if let Some(DataDefinition::Reference {
                    target: ReferenceTarget::Rule(rule),
                    ..
                }) = self.data.get(path)
                {
                    self.note_row_rule_read(identity, rule, ctx);
                }
            }
        }
    }

    /// Row spec of the single row a converted expression yields (`list -> first`), when
    /// it picks a row of an instance list. Depth-first over rule bodies and conditional arms
    /// with an explicit work stack.
    fn row_value_identity<'e>(
        &'e self,
        expression: ExprRef<'e, ExpressionKind>,
        scope: &SourceScope<'e>,
        visiting: &mut BTreeSet<RulePath>,
    ) -> Option<SpecIdentity> {
        let mut pending = vec![Pending::Sem(expression)];
        while let Some(item) = pending.pop() {
            let identity = match item {
                Pending::Sem(expression) => match expression.kind() {
                    ExpressionKind::ListCommand {
                        list,
                        op: ListOp::First | ListOp::Last | ListOp::Find { .. },
                        ..
                    } => self.instance_list_identity_guarded(
                        expression.child(*list),
                        scope,
                        &mut BTreeSet::new(),
                    ),
                    ExpressionKind::RulePath(rule) => {
                        pending.push(Pending::Rule(rule.clone()));
                        None
                    }
                    ExpressionKind::Piecewise(arms) => {
                        for (_, result) in arms.iter().rev() {
                            pending.push(Pending::Sem(expression.child(*result)));
                        }
                        None
                    }
                    _ => None,
                },
                Pending::Ast(expr) => match expr.kind() {
                    ast::ExpressionKind::Reference(reference) if reference.segments.is_empty() => {
                        pending.push(Pending::Rule(RulePath {
                            segments: scope.segments.to_vec(),
                            rule: reference.name.clone(),
                        }));
                        None
                    }
                    ast::ExpressionKind::ListCommand {
                        list,
                        command:
                            ast::ListCommand::First
                            | ast::ListCommand::Last
                            | ast::ListCommand::Find { .. },
                    } => self.instance_list_identity_from_source(
                        expr.child(*list),
                        scope,
                        &mut BTreeSet::new(),
                    ),
                    _ => None,
                },
                Pending::Rule(rule) => {
                    if !visiting.insert(rule.clone()) {
                        None
                    } else if let Some(node) = self.rules.get(&rule) {
                        for (_, body) in node.branches.iter().rev() {
                            pending.push(Pending::Sem(node.expression.at(*body)));
                        }
                        None
                    } else if rule.segments.as_slice() != scope.segments {
                        None
                    } else {
                        if let Some(source_rule) =
                            scope.spec.rules.iter().find(|r| r.name == rule.rule)
                        {
                            for unless in source_rule.unless_clauses.iter().rev() {
                                pending.push(Pending::Ast(unless.result.root()));
                            }
                            pending.push(Pending::Ast(source_rule.expression.root()));
                        }
                        None
                    }
                }
            };
            if identity.is_some() {
                return identity;
            }
        }
        None
    }

    /// [`Self::instance_list_identity_guarded`] on a rule body that is not converted yet.
    fn instance_list_identity_from_source(
        &self,
        expression: AstRef<'_>,
        scope: &SourceScope<'_>,
        visiting: &mut BTreeSet<RulePath>,
    ) -> Option<SpecIdentity> {
        self.instance_list_identity_walk(vec![Pending::Ast(expression)], scope, visiting)
    }

    /// Rule `rule` read on a row of `identity`. On a self-list row it is the rule being
    /// converted (recursion through the list) or an ordering edge: its type must be
    /// known before this rule is typed.
    fn note_row_rule_read(
        &self,
        identity: &SpecIdentity,
        rule: &RulePath,
        ctx: &mut RuleExpressionConversion<'a, '_>,
    ) {
        if *identity != self.instance_lists.self_identity {
            return;
        }
        if rule.segments.is_empty() && ctx.segments.is_empty() && rule.rule == ctx.rule {
            ctx.reads_itself_on_rows = true;
        } else {
            ctx.depends_on_rules.insert(rule.clone());
        }
    }

    /// Resolve `r` as a field of a row of an enclosing instance-list command.
    fn resolve_row_field(
        &mut self,
        r: &ast::Reference,
        source: &Source,
        ctx: &RuleExpressionConversion<'a, '_>,
    ) -> RowFieldLookup {
        let written = r.full_path();
        for scope in ctx.row_scopes.iter().rev() {
            let Some(list_path) = &scope.list_path else {
                continue;
            };
            let Some(rest_parts) = written.strip_prefix(list_path.as_slice()) else {
                continue;
            };
            if rest_parts.is_empty() {
                continue;
            }
            let rest = rest_parts.join(".");
            let list_name = list_path.join(".");
            return match self.template_row_end(&scope.template, &rest) {
                Some(end) => RowFieldLookup::Field(ExpressionKind::RowField {
                    level: scope.level,
                    identity: scope.identity.clone(),
                    end,
                }),
                None => {
                    let message = if match &scope.template {
                        RowTemplate::Root => instance_list::is_row_import(&self.data, &rest),
                        RowTemplate::Plan(plan) => instance_list::is_row_import(&plan.data, &rest),
                    } {
                        format!(
                            "`{rest}` is a `uses` instance on rows of `{list_name}`; read a field of it (`{list_name}.{rest}.<field>`), not the instance"
                        )
                    } else {
                        format!(
                            "no data or rule `{rest}` on rows of `{list_name}` (`{}`)",
                            scope.identity.spec
                        )
                    };
                    self.errors.push(self.engine_error(message, source));
                    RowFieldLookup::Failed
                }
            };
        }
        let field = r.to_string();
        let row_matches: Vec<(&RowScope, RowEnd)> = ctx
            .row_scopes
            .iter()
            .rev()
            .filter_map(|scope| {
                self.template_row_end(&scope.template, &field)
                    .map(|end| (scope, end))
            })
            .collect();
        let Some((scope, end)) = row_matches.first() else {
            return RowFieldLookup::NotARowField;
        };
        let head = r.segments.first().unwrap_or(&r.name);
        let enclosing = ctx.data_map.contains_key(head)
            || (ctx.rule_names.contains(head.as_str()) && head.as_str() != ctx.rule);
        if enclosing || row_matches.len() > 1 {
            let mut owners: Vec<String> = Vec::new();
            if enclosing {
                owners.push(format!("on `{}`", ctx.spec.name));
            }
            for (other, _) in &row_matches {
                owners.push(match &other.list_path {
                    Some(path) => format!("on rows of `{}`", path.join(".")),
                    None => format!("on `{}` rows", other.identity.spec),
                });
            }
            let qualifier = scope
                .list_path
                .as_ref()
                .map(|path| path.join("."))
                .unwrap_or_else(|| scope.identity.spec.clone());
            self.errors.push(self.engine_error(
                format!(
                    "`{field}` exists {}; write `{qualifier}.{field}`",
                    owners.join(" and ")
                ),
                source,
            ));
            return RowFieldLookup::Failed;
        }
        RowFieldLookup::Field(ExpressionKind::RowField {
            level: scope.level,
            identity: scope.identity.clone(),
            end: end.clone(),
        })
    }

    fn convert_list_type_expression(
        &mut self,
        element: &ParentType,
        expr: AstRef<'_>,
        ctx: &mut RuleExpressionConversion<'a, '_>,
    ) -> Option<ExprId> {
        let (_, _, types) = self
            .local_types
            .iter()
            .find(|(_, spec, _)| discovery::same_loaded_spec(spec, ctx.spec))
            .expect("BUG: rule spec types resolved before rule conversion");
        let (_, list_type) = types
            .rule_list_types
            .iter()
            .find(|(written, _)| written == element)
            .expect("BUG: rule list types resolved with the spec's types");
        Some(ctx.push(
            ExpressionKind::Literal(Box::new(TypedLiteral {
                value: ValueKind::List(Vec::new()),
                lemma_type: Arc::clone(list_type),
            })),
            expr.source().cloned(),
        ))
    }

    /// Path of the data field that `-> with` row field `name` names. Records the error and
    /// returns `None` when it is not data of the row spec or repeats a field of the row.
    fn row_field_path(&mut self, build: &ListCommandBuild<'_>, name: &str) -> Option<DataPath> {
        let scope = build
            .row_scope
            .as_ref()
            .expect("BUG: field arguments are queued only with a row scope");
        let Some(path) = self.template_data_path(&scope.template, name) else {
            self.errors.push(self.engine_error(
                format!(
                    "row field `{name}` is not data on `{}`",
                    scope.identity.spec
                ),
                &build.source,
            ));
            return None;
        };
        if build.fields.iter().any(|(existing, _)| *existing == path) {
            self.errors.push(self.engine_error(
                format!("row field `{name}` appears twice in one row"),
                &build.source,
            ));
            return None;
        }
        Some(path)
    }

    /// Planned list command from its converted arguments, which are in the order of
    /// [`list_command_arguments`]. `None` when a rule-level error was recorded.
    fn finish_list_command(
        &mut self,
        command: &ast::ListCommand,
        row_scope: &Option<RowScope>,
        converted: Vec<ExprId>,
        fields: Vec<(DataPath, ExprId)>,
        failed: bool,
        source: &Source,
    ) -> Option<ListOp> {
        let mut converted = converted.into_iter();
        match command {
            ast::ListCommand::Find { .. } => Some(ListOp::Find {
                keys: converted.collect(),
            }),
            ast::ListCommand::First => Some(ListOp::First),
            ast::ListCommand::Last => Some(ListOp::Last),
            ast::ListCommand::Count => Some(ListOp::Count),
            ast::ListCommand::Sum { argument } => Some(ListOp::Reduce {
                kind: ListReduce::Sum,
                argument: optional_argument(argument, &mut converted),
            }),
            ast::ListCommand::Avg { argument } => Some(ListOp::Reduce {
                kind: ListReduce::Avg,
                argument: optional_argument(argument, &mut converted),
            }),
            ast::ListCommand::Product { argument } => Some(ListOp::Reduce {
                kind: ListReduce::Product,
                argument: optional_argument(argument, &mut converted),
            }),
            ast::ListCommand::Min { argument } => Some(ListOp::Reduce {
                kind: ListReduce::Min,
                argument: optional_argument(argument, &mut converted),
            }),
            ast::ListCommand::Max { argument } => Some(ListOp::Reduce {
                kind: ListReduce::Max,
                argument: optional_argument(argument, &mut converted),
            }),
            ast::ListCommand::Any { form } => {
                Some(list_predicate_op(form, ListPredKind::Any, &mut converted))
            }
            ast::ListCommand::All { form } => {
                Some(list_predicate_op(form, ListPredKind::All, &mut converted))
            }
            ast::ListCommand::Where { form } => {
                Some(list_predicate_op(form, ListPredKind::Where, &mut converted))
            }
            ast::ListCommand::Sort {
                descending,
                argument,
            } => Some(ListOp::Sort {
                descending: *descending,
                argument: optional_argument(argument, &mut converted),
            }),
            ast::ListCommand::Top { .. } => Some(ListOp::Top {
                n: next_argument(&mut converted),
            }),
            ast::ListCommand::With { rows } => match (rows, row_scope) {
                (ast::ListWithRows::Values(_), None) => Some(ListOp::With {
                    rows: converted.collect(),
                }),
                (ast::ListWithRows::Values(_), Some(scope)) => {
                    self.errors.push(self.engine_error(
                        format!(
                            "`-> with <instance>` appends named instances on a `uses` list declaration; on a `{}` list in a rule, write `-> with <field>: <value>` for an inline row",
                            scope.identity.spec
                        ),
                        source,
                    ));
                    None
                }
                (ast::ListWithRows::FieldPairs(_), None) => {
                    self.errors.push(self.engine_error(
                        "`-> with field: value` appends a row to an instance list; this is a value list, write `-> with <value>`",
                        source,
                    ));
                    None
                }
                (ast::ListWithRows::FieldPairs(_), Some(_)) => {
                    (!failed).then_some(ListOp::WithRow { fields })
                }
            },
        }
    }
}

/// Row spec of an instance list: its identity and template plan.
#[derive(Clone, Copy)]
struct RowSpec<'t> {
    identity: &'t SpecIdentity,
    template: &'t ExecutionPlan,
}

/// Literal source-row value read against the template field type.
fn source_row_literal(
    value: &Value,
    field_type: &Arc<LemmaType>,
    template: &ExecutionPlan,
) -> Result<BoundValueKind, String> {
    let typed = crate::evaluation::evaluate_data::typed_literal_from_parsed(
        value,
        field_type,
        template.expression_unit_index(),
        &template.resolved_types.resolved,
    )?;
    execution_plan::validate_value_against_type(
        typed.lemma_type.as_ref(),
        &LiteralValue {
            value: typed.value.clone(),
        },
        template.expression_unit_index(),
    )?;
    Ok(BoundValueKind {
        value: typed.value,
        unit: typed.lemma_type.unit().map(Arc::from),
    })
}

fn unit_tokens_for_spec<'b>(
    types: &'b ResolvedTypesMap,
    spec: &LemmaSpec,
) -> UnitTokenResolver<'b> {
    match find_types_by_spec(types, spec) {
        Some(resolved) => UnitTokenResolver::Declared {
            unit_index: &resolved.unit_index,
            named_types: &resolved.resolved,
        },
        None => UnitTokenResolver::Undeclared,
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

/// `RulePresence::Dropped` here means a nested template already recorded a planning error
/// for the rule (see `Graph::rule_presence`). The dependent gets an undetermined type; the
/// error is reported once, from the template.
fn inferred_or_dropped(
    graph: &Graph,
    computed_rule_types: &HashMap<RulePath, Arc<LemmaType>>,
    rule_path: &RulePath,
) -> Arc<LemmaType> {
    if let Some(lemma_type) = computed_rule_types.get(rule_path) {
        return Arc::clone(lemma_type);
    }
    match graph.rule_presence(rule_path) {
        RulePresence::Dropped => Arc::new(LemmaType::undetermined_type()),
        RulePresence::Present if graph.has_local_rule(rule_path) => {
            panic!("BUG: rule type not inferred before dependent in topological order")
        }
        RulePresence::Present => {
            let (plan, local) =
                peel_rule_from(&graph.data, &graph.instance_lists.templates, rule_path)
                    .expect("BUG: Present import rule must peel");
            let executable = plan
                .rules
                .get(local)
                .expect("BUG: peeled import rule missing from template plan");
            Arc::clone(&executable.rule_type)
        }
        RulePresence::Absent => {
            panic!("BUG: rule '{rule_path}' neither in graph nor dropped")
        }
    }
}

/// Type of `expression`, without validation; `LemmaType::undetermined_type()` when it cannot
/// be determined (e.g. unknown data). Every node of the table up to `expression` is typed in
/// table order, each reading its operands' types from the typed prefix, so nothing recurses.
fn infer_expression_type(
    expression: ExprRef<'_, ExpressionKind>,
    graph: &Graph,
    computed_rule_types: &HashMap<RulePath, Arc<LemmaType>>,
    resolved_types: &ResolvedTypesMap,
    cache: &mut InferExpressionTypeCache,
) -> Arc<LemmaType> {
    let key = expression.table as *const ExprTable<ExpressionKind> as usize;
    let types = cache
        .tables
        .entry(key)
        .or_insert_with(|| Vec::with_capacity(expression.table.len()));
    for index in types.len()..=expression.id.index() {
        let node = expression.child(ExprId::from_index(index));
        let lemma_type =
            infer_expression_type_uncached(node, graph, computed_rule_types, resolved_types, types);
        types.push(lemma_type);
    }
    Arc::clone(&types[expression.id.index()])
}

/// Type of an operand of the node being typed. Operands precede their parent in the table,
/// so they are always in the typed prefix.
fn cached_expression_type(
    cache: &[Arc<LemmaType>],
    expression: ExprRef<'_, ExpressionKind>,
) -> Arc<LemmaType> {
    Arc::clone(
        cache
            .get(expression.id.index())
            .expect("BUG: operand typed before its parent"),
    )
}

fn infer_expression_type_uncached(
    expression: ExprRef<'_, ExpressionKind>,
    graph: &Graph,
    computed_rule_types: &HashMap<RulePath, Arc<LemmaType>>,
    resolved_types: &ResolvedTypesMap,
    cache: &[Arc<LemmaType>],
) -> Arc<LemmaType> {
    match expression.kind() {
        ExpressionKind::Literal(literal_value) => Arc::clone(&literal_value.lemma_type),

        ExpressionKind::DataPath(data_path) => {
            infer_data_type(data_path, graph, computed_rule_types)
        }

        ExpressionKind::RulePath(rule_path) => {
            inferred_or_dropped(graph, computed_rule_types, rule_path)
        }

        ExpressionKind::LogicalAnd { first, steps } => steps.iter().fold(
            cached_expression_type(cache, expression.child(*first)),
            |left_type, step| {
                let right_type = cached_expression_type(cache, expression.child(step.operand));
                logical_and_type(&left_type, &right_type)
            },
        ),

        ExpressionKind::LogicalNegation(operand, _) => {
            let operand_type = cached_expression_type(cache, expression.child(*operand));
            logical_not_type(&operand_type)
        }

        ExpressionKind::Comparison(left, _, right) => {
            let left_type = cached_expression_type(cache, expression.child(*left));
            let right_type = cached_expression_type(cache, expression.child(*right));
            comparison_type(&left_type, &right_type)
        }

        ExpressionKind::Arithmetic { first, steps } => {
            let scope = arithmetic_scope(resolved_types, graph);
            steps.iter().fold(
                cached_expression_type(cache, expression.child(*first)),
                |left_type, step| {
                    let right_type = cached_expression_type(cache, expression.child(step.operand));
                    compute_arithmetic_result_type(left_type, &step.op, right_type, &scope)
                },
            )
        }

        ExpressionKind::UnitConversion(source_expression, target) => {
            let source_type = cached_expression_type(cache, expression.child(*source_expression));
            unit_conversion_type(&source_type, target)
        }

        ExpressionKind::MathematicalComputation(op, operand) => {
            let operand_type = cached_expression_type(cache, expression.child(*operand));
            math_op_type(op, &operand_type, &arithmetic_scope(resolved_types, graph))
        }

        ExpressionKind::RangeBound(_, operand) => {
            let operand_type = cached_expression_type(cache, expression.child(*operand));
            if operand_type.vetoed() {
                return Arc::new(LemmaType::veto_type());
            }
            if operand_type.is_undetermined() {
                return Arc::new(LemmaType::undetermined_type());
            }
            range_bound_result_type(
                operand_type.as_ref(),
                &arithmetic_scope(resolved_types, graph),
            )
        }

        ExpressionKind::Veto(_) => Arc::new(LemmaType::veto_type()),

        ExpressionKind::ResultIsVeto(operand) => {
            cached_expression_type(cache, expression.child(*operand));
            result_is_veto_type()
        }

        ExpressionKind::Now => primitive_date_arc().clone(),

        ExpressionKind::DateRelative(_, date_expr)
        | ExpressionKind::DateCalendar(_, _, date_expr) => {
            let date_type = cached_expression_type(cache, expression.child(*date_expr));
            date_predicate_type(&date_type)
        }

        ExpressionKind::RangeContainment(value, range) => {
            let value_type = cached_expression_type(cache, expression.child(*value));
            let range_type = cached_expression_type(cache, expression.child(*range));
            if value_type.vetoed() || range_type.vetoed() {
                return Arc::new(LemmaType::veto_type());
            }
            primitive_boolean_arc().clone()
        }

        ExpressionKind::RangeLiteral(left, right) => {
            let left_type = cached_expression_type(cache, expression.child(*left));
            let right_type = cached_expression_type(cache, expression.child(*right));
            if left_type.vetoed() || right_type.vetoed() {
                return Arc::new(LemmaType::veto_type());
            }
            if left_type.is_undetermined() || right_type.is_undetermined() {
                return Arc::new(LemmaType::undetermined_type());
            }
            infer_range_type_from_endpoint_types(left_type.as_ref(), right_type.as_ref())
        }

        ExpressionKind::PastFutureRange(_, offset_expr) => {
            let offset_type = cached_expression_type(cache, expression.child(*offset_expr));
            past_future_range_type(&offset_type)
        }

        ExpressionKind::SpecMember {
            base,
            path,
            name,
            targets,
        } => {
            let base_type = cached_expression_type(cache, expression.child(*base));
            if base_type.vetoed() {
                return Arc::new(LemmaType::veto_type());
            }
            if !base_type.is_spec() {
                return Arc::new(LemmaType::undetermined_type());
            }
            if !targets.is_empty() {
                return spec_member_type(targets, graph, computed_rule_types);
            }
            match row_member_end(graph, &base_type, path, name) {
                Some((fields, end)) => fields.end_type(&end, computed_rule_types),
                None => Arc::new(LemmaType::undetermined_type()),
            }
        }

        ExpressionKind::RowField { identity, end, .. } => {
            row_fields(graph, identity).end_type(end, computed_rule_types)
        }

        ExpressionKind::Piecewise(arms) => {
            let mut result_type: Option<Arc<LemmaType>> = None;
            for (condition, result) in arms {
                let condition_type = cached_expression_type(cache, expression.child(*condition));
                if !condition_type.is_boolean() && !condition_type.is_undetermined() {
                    return Arc::new(LemmaType::undetermined_type());
                }
                let arm_result_type = cached_expression_type(cache, expression.child(*result));
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

        ExpressionKind::ListCommand { list, op, .. } => infer_list_command_type(
            expression,
            expression.child(*list),
            op,
            graph,
            resolved_types,
            cache,
        ),
    }
}

/// Fields of rows of one row spec while the consumer is typed.
#[derive(Clone, Copy)]
enum RowFields<'g> {
    /// Row spec with its own template plan.
    Plan(&'g ExecutionPlan),
    /// Self-list: rows are the spec being typed.
    Root(&'g Graph<'g>),
}

impl<'g> RowFields<'g> {
    fn end(&self, name: &str) -> Option<RowEnd> {
        match self {
            RowFields::Plan(plan) => {
                instance_list::row_end_of(instance_list::RowEndView::Plan(plan), name)
            }
            RowFields::Root(graph) => {
                // Typing runs after conversion; present rules only (not main_spec
                // forward refs). Membership matches the completed plan.
                let rules: Vec<&RulePath> = graph.rules().keys().collect();
                root_row_end(
                    graph.data(),
                    &graph.instance_lists().templates,
                    &rules,
                    name,
                )
            }
        }
    }

    /// Type of `end`. A rule of the spec being typed reads `computed_rule_types`:
    /// typing order puts it first, or it is the recursive rule's seed.
    fn end_type(
        &self,
        end: &RowEnd,
        computed_rule_types: &HashMap<RulePath, Arc<LemmaType>>,
    ) -> Arc<LemmaType> {
        match (self, end) {
            (RowFields::Plan(plan), _) => instance_list::row_end_type(plan, end),
            (RowFields::Root(graph), RowEnd::Data(path)) => {
                if path.segments.is_empty() {
                    if let Some(DataDefinition::Reference {
                        target: ReferenceTarget::Rule(rule),
                        ..
                    }) = graph.data().get(path)
                    {
                        return inferred_or_dropped(graph, computed_rule_types, rule);
                    }
                    return instance_list::row_data_type(
                        graph.data(),
                        &graph.instance_lists().templates,
                        path,
                    );
                }
                let (_, _, definition) = graph
                    .peel_data(path)
                    .expect("BUG: root row field path must peel");
                if let DataDefinition::Reference {
                    target: ReferenceTarget::Rule(rule),
                    ..
                } = definition
                {
                    return inferred_or_dropped(graph, computed_rule_types, rule);
                }
                Arc::clone(
                    definition
                        .resolved_type_arc()
                        .expect("BUG: peeled root row data must be typed"),
                )
            }
            (RowFields::Root(graph), RowEnd::Rule(rule)) => {
                inferred_or_dropped(graph, computed_rule_types, rule)
            }
        }
    }

    fn default(&self, path: &DataPath) -> Option<TypedLiteral> {
        match self {
            RowFields::Plan(plan) => instance_list::template_default(plan, path),
            RowFields::Root(graph) => {
                if path.segments.is_empty() {
                    instance_list::template_default_local(graph.data(), path)
                } else {
                    let (_, _, definition) = graph
                        .peel_data(path)
                        .expect("BUG: root row default path must peel");
                    let lemma_type = definition
                        .resolved_type_arc()
                        .expect("BUG: key field must be typed data");
                    if let Some(bound) = definition.bound_value() {
                        return Some(instance_list::typed_from_bound(bound, lemma_type));
                    }
                    definition
                        .value()
                        .map(|literal| crate::planning::semantics::TypedLiteral {
                            value: literal.value,
                            lemma_type: Arc::clone(lemma_type),
                        })
                }
            }
        }
    }
}

/// Fields of rows of row spec `identity`. Every instance-list identity has a template
/// plan, except a self-list's, whose rows are the spec being typed.
fn row_fields<'g>(graph: &'g Graph<'g>, identity: &SpecIdentity) -> RowFields<'g> {
    if *identity == graph.instance_lists().self_identity {
        return RowFields::Root(graph);
    }
    RowFields::Plan(
        graph
            .instance_lists()
            .templates
            .get(identity)
            .unwrap_or_else(|| panic!("BUG: no template plan for row spec '{identity}'")),
    )
}

/// Row field `path.name` read on a base of spec type that yields instance-list rows.
fn row_member_end<'g>(
    graph: &'g Graph<'g>,
    base_type: &LemmaType,
    path: &[String],
    name: &str,
) -> Option<(RowFields<'g>, RowEnd)> {
    let TypeSpecification::Spec { spec: identity } = &base_type.specifications else {
        return None;
    };
    let is_row = *identity == graph.instance_lists().self_identity
        || graph.instance_lists().templates.contains_key(identity);
    if !is_row {
        return None;
    }
    let fields = row_fields(graph, identity);
    let mut key = path.join(".");
    if !key.is_empty() {
        key.push('.');
    }
    key.push_str(name);
    fields.end(&key).map(|end| (fields, end))
}

/// Whether `path.name` on a row of `base_type` names a `uses` instance of the row spec.
fn row_member_import(
    graph: &Graph<'_>,
    base_type: &LemmaType,
    path: &[String],
    name: &str,
) -> bool {
    let TypeSpecification::Spec { spec: identity } = &base_type.specifications else {
        return false;
    };
    let is_row = *identity == graph.instance_lists().self_identity
        || graph.instance_lists().templates.contains_key(identity);
    if !is_row {
        return false;
    }
    let mut key = path.join(".");
    if !key.is_empty() {
        key.push('.');
    }
    key.push_str(name);
    match row_fields(graph, identity) {
        RowFields::Plan(plan) => instance_list::is_row_import(&plan.data, &key),
        RowFields::Root(graph) => instance_list::is_row_import(graph.data(), &key),
    }
}

fn infer_list_command_type(
    node: ExprRef<'_, ExpressionKind>,
    list: ExprRef<'_, ExpressionKind>,
    op: &ListOp,
    graph: &Graph,
    resolved_types: &ResolvedTypesMap,
    cache: &[Arc<LemmaType>],
) -> Arc<LemmaType> {
    let list_type = cached_expression_type(cache, list);
    if list_type.vetoed() {
        return Arc::new(LemmaType::veto_type());
    }
    if list_type.is_undetermined() {
        return Arc::new(LemmaType::undetermined_type());
    }
    if !list_type.is_list() {
        return Arc::new(LemmaType::undetermined_type());
    }
    let instance_rows = list_has_instance_rows(&list_type);
    let kind = match op {
        ListOp::Count => ListCommandResultKind::Count,
        ListOp::First | ListOp::Last | ListOp::Find { .. } => ListCommandResultKind::Element,
        ListOp::Reduce {
            argument: Some(_), ..
        } if instance_rows => ListCommandResultKind::InstanceReduce,
        ListOp::Reduce { argument: None, .. } if !instance_rows => ListCommandResultKind::Element,
        ListOp::Reduce { .. } => return Arc::new(LemmaType::undetermined_type()),
        ListOp::Sort {
            argument: Some(_), ..
        } if !instance_rows => return Arc::new(LemmaType::undetermined_type()),
        ListOp::Predicate { kind, .. } | ListOp::InstancePredicate { kind, .. } => match kind {
            ListPredKind::Any | ListPredKind::All => ListCommandResultKind::BooleanPredicate,
            ListPredKind::Where => ListCommandResultKind::KeepList,
        },
        ListOp::Sort { .. } | ListOp::Top { .. } | ListOp::With { .. } | ListOp::WithRow { .. } => {
            ListCommandResultKind::KeepList
        }
    };
    let first_argument = match op {
        ListOp::Reduce {
            argument: Some(argument),
            ..
        } => Some(cached_expression_type(cache, node.child(*argument))),
        _ => None,
    };
    list_command_result_type(
        &list_type,
        kind,
        first_argument.as_ref(),
        &arithmetic_scope(resolved_types, graph),
    )
}

fn list_element_type(
    list_type: &LemmaType,
    graph: &Graph,
    resolved_types: &ResolvedTypesMap,
) -> Arc<LemmaType> {
    resolve_anonymous_measure(
        list_type.list_element_lemma_type(),
        &arithmetic_scope(resolved_types, graph),
    )
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
    let Some(lookup) = graph.lookup_data(data_path) else {
        return Arc::new(LemmaType::undetermined_type());
    };
    match lookup.definition() {
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
                inferred_or_dropped(graph, computed_rule_types, target_rule)
            }
        }
        DataDefinition::Reference { resolved_type, .. }
        | DataDefinition::InstanceList { resolved_type, .. } => Arc::clone(resolved_type),
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
    for node in expression.table.nodes() {
        if let ExpressionKind::DataPath(path) = &node.kind {
            if let Some(ReferenceEnd::Rule(target_rule)) = reference_ends.get(path) {
                out.insert(target_rule.clone());
            }
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

fn spec_member_end_type(
    end: &SpecMemberEnd,
    graph: &Graph,
    computed_rule_types: &HashMap<RulePath, Arc<LemmaType>>,
) -> Arc<LemmaType> {
    match end {
        SpecMemberEnd::Rule(path) => inferred_or_dropped(graph, computed_rule_types, path),
        SpecMemberEnd::Data(path) => infer_data_type(path, graph, computed_rule_types),
        SpecMemberEnd::Instance { identity, .. } => {
            Arc::new(LemmaType::primitive(TypeSpecification::Spec {
                spec: identity.clone(),
            }))
        }
    }
}

fn spec_member_type(
    targets: &[SpecMemberTarget],
    graph: &Graph,
    computed_rule_types: &HashMap<RulePath, Arc<LemmaType>>,
) -> Arc<LemmaType> {
    let mut chosen: Option<Arc<LemmaType>> = None;
    for target in targets {
        let member_type = spec_member_end_type(&target.end, graph, computed_rule_types);
        if member_type.vetoed() || member_type.is_undetermined() {
            continue;
        }
        match &chosen {
            None => chosen = Some(member_type),
            Some(existing) if existing.same_value_type(member_type.as_ref()) => {}
            Some(_) => return Arc::new(LemmaType::undetermined_type()),
        }
    }
    chosen.unwrap_or_else(|| Arc::new(LemmaType::undetermined_type()))
}

fn spec_value_not_operable(
    graph: &Graph,
    left_type: &LemmaType,
    right_type: &LemmaType,
    source: &Source,
) -> Option<Error> {
    let spec_type = if left_type.is_spec() {
        Some(left_type)
    } else if right_type.is_spec() {
        Some(right_type)
    } else {
        None
    };
    spec_type.map(|spec_type| {
        engine_error_at_graph(
            graph,
            source,
            format!(
                "spec instance is not a value here (got {}); read a field or use `is veto`",
                spec_type.name()
            ),
        )
    })
}

fn check_range_containment_types(
    graph: &Graph,
    value_type: &LemmaType,
    range_type: &LemmaType,
    source: &Source,
) -> Result<(), Vec<Error>> {
    if value_type.vetoed() || range_type.vetoed() {
        return Ok(());
    }
    if !range_type.is_range() {
        return Err(vec![engine_error_at_graph(
            graph,
            source,
            format!(
                "Right side of 'in' must be a range, got type '{}'",
                range_type.name()
            ),
        )]);
    }
    let compatible = (range_type.is_date_range() && value_type.is_date())
        || (range_type.is_time_range() && (value_type.is_time() || value_type.is_date()))
        || (range_type.is_number_range() && value_type.is_number())
        || (range_type.is_measure_range()
            && value_type.is_measure()
            && measure_range_matches_measure(range_type, value_type))
        || (range_type.is_ratio_range() && value_type.is_ratio())
        || (range_type.is_calendar_like_range() && value_type.is_calendar_like());
    if !compatible {
        return Err(vec![engine_error_at_graph(
            graph,
            source,
            format!(
                "Cannot test whether {} is in {}.",
                value_type.name(),
                range_type.name()
            ),
        )]);
    }
    Ok(())
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
    if let Some(error) = spec_value_not_operable(graph, left_type, right_type, source) {
        return Err(vec![error]);
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
    right: ExprRef<'_, ExpressionKind>,
    operator: &ArithmeticComputation,
    source: &Source,
) -> Result<(), Vec<Error>> {
    if !matches!(
        operator,
        ArithmeticComputation::Divide | ArithmeticComputation::Modulo
    ) {
        return Ok(());
    }

    if let ExpressionKind::Literal(literal) = right.kind() {
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
    right: ExprRef<'_, ExpressionKind>,
    left_type: &LemmaType,
    operator: &ArithmeticComputation,
    source: &Source,
) -> Result<(), Vec<Error>> {
    if *operator != ArithmeticComputation::Power {
        return Ok(());
    }
    // Measure ^ non-integer-literal is rejected: fractional dimensions are undefined,
    // and variable exponents cannot be statically verified to be integers at plan time.
    if left_type.is_measure() || left_type.is_duration_like() {
        let is_integer_literal = if let ExpressionKind::Literal(lit) = right.kind() {
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
    right: ExprRef<'_, ExpressionKind>,
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
        arithmetic_power_exponent_planning_errors(graph, right, left_type, operator, source),
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
    if let Some(error) = spec_value_not_operable(graph, left_type, right_type, source) {
        return Err(vec![error]);
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
        if let (
            ArithmeticComputation::Multiply | ArithmeticComputation::Divide,
            Some(left_decomposition),
            Some(right_decomposition),
        ) = (
            operator,
            left_type.measure_type_decomposition(),
            right_type.measure_type_decomposition(),
        ) {
            if semantics::combine_decompositions(
                left_decomposition,
                right_decomposition,
                matches!(operator, ArithmeticComputation::Multiply),
            )
            .is_none()
            {
                return Err(vec![engine_error_at_graph(
                    graph,
                    source,
                    format!(
                        "Cannot apply '{}' to {} and {}: a resulting unit exponent exceeds the maximum of {}.",
                        operator,
                        left_type.name(),
                        right_type.name(),
                        crate::limits::MAX_UNIT_EXPONENT
                    ),
                )]);
            }
        }
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

    struct OperandPair<'t> {
        left_type: &'t LemmaType,
        right_type: &'t LemmaType,
    }
    impl OperandPair<'_> {
        fn check(&self, a: impl Fn(&LemmaType) -> bool, b: impl Fn(&LemmaType) -> bool) -> bool {
            (a(self.left_type) && b(self.right_type)) || (b(self.left_type) && a(self.right_type))
        }
    }
    let pair = OperandPair {
        left_type,
        right_type,
    };

    let allowed = match operator {
        ArithmeticComputation::Multiply => {
            pair.check(LemmaType::is_measure, LemmaType::is_number)
                || pair.check(LemmaType::is_measure, LemmaType::is_ratio)
                || pair.check(LemmaType::is_measure, LemmaType::is_duration_like_measure)
                || pair.check(LemmaType::is_measure, LemmaType::is_calendar_like)
                || pair.check(LemmaType::is_duration_like_measure, LemmaType::is_number)
                || pair.check(LemmaType::is_duration_like_measure, LemmaType::is_ratio)
                || pair.check(LemmaType::is_calendar_like, LemmaType::is_number)
                || pair.check(LemmaType::is_calendar_like, LemmaType::is_ratio)
                || pair.check(LemmaType::is_number, LemmaType::is_ratio)
        }
        ArithmeticComputation::Divide => {
            pair.check(LemmaType::is_measure, LemmaType::is_number)
                || (left_type.is_measure() && right_type.is_ratio())
                || pair.check(LemmaType::is_measure, LemmaType::is_duration_like_measure)
                || pair.check(LemmaType::is_measure, LemmaType::is_calendar_like)
                || (left_type.is_duration_like() && right_type.is_number())
                || (left_type.is_duration_like() && right_type.is_ratio())
                || (left_type.is_calendar_like() && right_type.is_number())
                || (left_type.is_calendar_like() && right_type.is_ratio())
                || (left_type.is_number() && right_type.is_duration_like())
                || (left_type.is_number() && right_type.is_calendar_like())
                || pair.check(LemmaType::is_number, LemmaType::is_ratio)
        }
        ArithmeticComputation::Add | ArithmeticComputation::Subtract => {
            if pair.check(LemmaType::is_ratio, |t: &LemmaType| {
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

fn has_explicit_unit(source: ExprRef<'_, ExpressionKind>) -> bool {
    match source.kind() {
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

enum SuggestionPiece<'a> {
    Node(ExprRef<'a, ExpressionKind>),
    Text(String),
}

/// Pieces of `first op operand ...` over `steps`, in pop order.
fn arithmetic_chain_pieces<'a>(
    node: ExprRef<'a, ExpressionKind>,
    first: ExprId,
    steps: &[ast::ChainStep<ArithmeticComputation>],
    pending: &mut Vec<SuggestionPiece<'a>>,
) {
    for step in steps.iter().rev() {
        pending.push(SuggestionPiece::Node(node.child(step.operand)));
        pending.push(SuggestionPiece::Text(format!(" {} ", step.op)));
    }
    pending.push(SuggestionPiece::Node(node.child(first)));
}

fn expression_suggestion_label(expression: ExprRef<'_, ExpressionKind>) -> String {
    suggestion_label(vec![SuggestionPiece::Node(expression)])
}

fn chain_prefix_suggestion_label(
    expression: ExprRef<'_, ExpressionKind>,
    first: ExprId,
    prefix: &[ast::ChainStep<ArithmeticComputation>],
) -> String {
    let mut pending = Vec::new();
    arithmetic_chain_pieces(expression, first, prefix, &mut pending);
    suggestion_label(pending)
}

fn suggestion_label(mut pending: Vec<SuggestionPiece<'_>>) -> String {
    use SuggestionPiece as Piece;
    let mut out = String::new();
    while let Some(piece) = pending.pop() {
        match piece {
            Piece::Text(text) => out.push_str(&text),
            Piece::Node(node) => match node.kind() {
                ExpressionKind::DataPath(p) => out.push_str(&p.data),
                ExpressionKind::RulePath(p) => out.push_str(&p.rule),
                ExpressionKind::Arithmetic { first, steps } => {
                    arithmetic_chain_pieces(node, *first, steps, &mut pending);
                }
                ExpressionKind::UnitConversion(inner, target) => {
                    pending.push(Piece::Text(format!(" as {target}")));
                    pending.push(Piece::Node(node.child(*inner)));
                }
                ExpressionKind::Literal(lit) => match &lit.value {
                    ValueKind::Number(n) => out.push_str(&n.display_str()),
                    _ => out.push_str("<value>"),
                },
                _ => out.push_str("<expr>"),
            },
        }
    }
    out
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
    source: ExprRef<'_, ExpressionKind>,
    source_type: &LemmaType,
    target: &SemanticConversionTarget,
    resolved_types: &ResolvedTypesMap,
    source_loc: &Source,
    spec: &LemmaSpec,
) -> Result<(), Vec<Error>> {
    if source_type.vetoed() {
        return Ok(());
    }
    if source_type.is_spec() {
        return Err(vec![engine_error_at_graph(
            graph,
            source_loc,
            format!(
                "spec instance is not a value here (got {}); read a field or use `is veto`",
                source_type.name()
            ),
        )]);
    }
    let unit_index = find_types_by_spec(resolved_types, spec)
        .map(|dt| &dt.unit_index)
        .expect("BUG: spec types missing during unit conversion check");

    match target {
        SemanticConversionTarget::Type(PrimitiveKind::Text) => Ok(()),
        SemanticConversionTarget::Type(PrimitiveKind::Time) => {
            if source_type.is_date() || source_type.is_time() {
                Ok(())
            } else {
                Err(vec![engine_error_at_graph(
                    graph,
                    source_loc,
                    format!("Cannot convert {} to time.", source_type.name()),
                )])
            }
        }
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
                // Target unit comes from the resolved owner (builtin percent/permille or
                // an in-scope custom). Source need not declare that unit; builtins bridge.
                if crate::computation::units::owning_type_declares_unit_name(owning_type, unit_name)
                {
                    return Ok(());
                }
                let valid = match &owning_type.specifications {
                    TypeSpecification::Ratio { units, .. } => units
                        .0
                        .iter()
                        .map(|u| u.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    _ => String::new(),
                };
                return Err(vec![engine_error_at_graph(
                    graph,
                    source_loc,
                    format!(
                        "Unknown unit '{unit_name}' for ratio type '{}'. Valid units: {}",
                        owning_type.name(),
                        valid
                    ),
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
    if operand_type.is_spec() {
        return Err(vec![engine_error_at_graph(
            graph,
            source,
            format!(
                "spec instance is not a value here (got {}); read a field or use `is veto`",
                operand_type.name()
            ),
        )]);
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
    for (rule_path, rule_node) in graph.rules() {
        for dependency in &rule_node.depends_on_rules {
            match graph.rule_presence(dependency) {
                RulePresence::Present | RulePresence::Dropped => {}
                RulePresence::Absent => {
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
    let Some(lookup) = graph.lookup_data(data_path) else {
        return Err(vec![engine_error_at_graph(
            graph,
            data_source,
            format!("Unknown data reference '{}'", data_path),
        )]);
    };
    match lookup.definition() {
        DataDefinition::Value { .. }
        | DataDefinition::TypeDeclaration { .. }
        | DataDefinition::InstanceList { .. }
        | DataDefinition::Reference { .. } => Ok(()),
        DataDefinition::Import { .. } => Err(vec![engine_error_at_graph(
            graph,
            lookup.definition().source(),
            format!(
                "Cannot compute type for spec reference data '{}'",
                data_path
            ),
        )]),
    }
}

/// One step of a type check over an expression tree.
enum CheckStep {
    /// Errors found at the node, reported in order.
    Report(Vec<Error>),
    /// Check the child at this id, with its steps in order.
    Visit(ExprId),
}

/// Traversal stacks of [`check_expression`], reused across calls. Empty between calls.
#[derive(Default)]
struct CheckScratch {
    pending: Vec<CheckStep>,
    steps: Vec<CheckStep>,
}

/// Ordered steps of one node's check: local errors interleaved with child visits.
struct CheckSteps<'s> {
    steps: &'s mut Vec<CheckStep>,
    errors: Vec<Error>,
}

impl<'s> CheckSteps<'s> {
    /// Steps are appended to `steps`.
    fn new(steps: &'s mut Vec<CheckStep>) -> Self {
        Self {
            steps,
            errors: Vec::new(),
        }
    }

    fn push(&mut self, error: Error) {
        self.errors.push(error);
    }

    fn collect(&mut self, result: Result<(), Vec<Error>>) {
        if let Err(errs) = result {
            self.errors.extend(errs);
        }
    }

    fn visit(&mut self, child: &ExprId) {
        self.flush();
        self.steps.push(CheckStep::Visit(*child));
    }

    fn splice(&mut self, steps: Vec<CheckStep>) {
        self.flush();
        self.steps.extend(steps);
    }

    fn flush(&mut self) {
        if !self.errors.is_empty() {
            self.steps
                .push(CheckStep::Report(std::mem::take(&mut self.errors)));
        }
    }

    fn finish(mut self) {
        self.flush();
    }
}

/// Type errors of the expression at `expression` and its subtree.
///
/// Children are visited depth-first in source order with an explicit stack; each node's
/// steps come from [`check_steps`].
fn check_expression(
    expression: ExprRef<'_, ExpressionKind>,
    graph: &Graph,
    inferred_types: &HashMap<RulePath, Arc<LemmaType>>,
    resolved_types: &ResolvedTypesMap,
    spec: &LemmaSpec,
    cache: &mut InferExpressionTypeCache,
    scratch: &mut CheckScratch,
) -> Result<(), Vec<Error>> {
    let mut errors = Vec::new();
    let CheckScratch { pending, steps } = scratch;
    pending.push(CheckStep::Visit(expression.id));
    while let Some(step) = pending.pop() {
        match step {
            CheckStep::Report(found) => errors.extend(found),
            CheckStep::Visit(id) => {
                check_steps(
                    expression.child(id),
                    graph,
                    inferred_types,
                    resolved_types,
                    spec,
                    cache,
                    steps,
                );
                pending.extend(steps.drain(..).rev());
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Steps of the check of one node, not including its children's subtrees.
fn check_steps(
    expression: ExprRef<'_, ExpressionKind>,
    graph: &Graph,
    inferred_types: &HashMap<RulePath, Arc<LemmaType>>,
    resolved_types: &ResolvedTypesMap,
    spec: &LemmaSpec,
    cache: &mut InferExpressionTypeCache,
    steps: &mut Vec<CheckStep>,
) {
    let mut checks = CheckSteps::new(steps);
    match expression.kind() {
        ExpressionKind::Literal(_) => {}

        ExpressionKind::DataPath(data_path) => {
            let data_source = expression
                .source()
                .expect("BUG: expression missing source in check_expression");
            checks.collect(check_data_reference(data_path, graph, data_source));
        }

        ExpressionKind::RulePath(_) => {}

        ExpressionKind::SpecMember {
            base,
            targets,
            name,
            path,
        } => {
            let expr_source = expression
                .source()
                .expect("BUG: expression missing source in check_expression");
            checks.visit(base);
            let base_type = infer_expression_type(
                expression.child(*base),
                graph,
                inferred_types,
                resolved_types,
                cache,
            );
            let row_end = row_member_end(graph, &base_type, path, name);
            if !base_type.is_spec() && !base_type.is_undetermined() && !base_type.vetoed() {
                let message = match expression.child(*base).kind() {
                    ExpressionKind::RulePath(rule) => {
                        format!("rule '{rule}' does not return a spec")
                    }
                    _ => format!(
                        "`.{name}` reads a field of a spec value, but the operand is {}",
                        base_type.name()
                    ),
                };
                checks.push(engine_error_at_graph(graph, expr_source, message));
            } else if targets.is_empty() && base_type.is_spec() && row_end.is_none() {
                let message = if row_member_import(graph, &base_type, path, name) {
                    format!(
                        "`{name}` is a `uses` instance of {}; read a field of it (`{name}.<field>`), not the instance",
                        base_type.name()
                    )
                } else {
                    format!("no data or rule '{name}' on spec {}", base_type.name())
                };
                checks.push(engine_error_at_graph(graph, expr_source, message));
            } else if targets.len() > 1 {
                let first = spec_member_end_type(&targets[0].end, graph, inferred_types);
                for target in targets.iter().skip(1) {
                    let other = spec_member_end_type(&target.end, graph, inferred_types);
                    if !first.vetoed()
                        && !other.vetoed()
                        && !first.is_undetermined()
                        && !other.is_undetermined()
                        && !first.same_value_type(other.as_ref())
                    {
                        checks.push(engine_error_at_graph(
                            graph,
                            expr_source,
                            format!(
                                "field '{name}' has type {} on one instance and {} on another",
                                first.name(),
                                other.name()
                            ),
                        ));
                        break;
                    }
                }
            }
            if let (Some((fields, end)), Some(target)) = (&row_end, targets.first()) {
                let row_type = fields.end_type(end, inferred_types);
                let fixed_type = spec_member_end_type(&target.end, graph, inferred_types);
                if !fixed_type.vetoed()
                    && !fixed_type.is_undetermined()
                    && !row_type.same_value_type(fixed_type.as_ref())
                {
                    checks.push(engine_error_at_graph(
                        graph,
                        expr_source,
                        format!(
                            "field '{name}' has type {} on a row and {} on an instance",
                            row_type.name(),
                            fixed_type.name()
                        ),
                    ));
                }
            }
        }

        ExpressionKind::RowField { .. } => {}

        ExpressionKind::LogicalAnd { first, steps } => {
            checks.visit(first);
            let mut left_type = infer_expression_type(
                expression.child(*first),
                graph,
                inferred_types,
                resolved_types,
                cache,
            );
            for step in steps {
                checks.visit(&step.operand);
                let right_type = infer_expression_type(
                    expression.child(step.operand),
                    graph,
                    inferred_types,
                    resolved_types,
                    cache,
                );
                checks.collect(check_logical_and_operands(
                    graph,
                    &left_type,
                    &right_type,
                    &step.source_location,
                ));
                left_type = logical_and_type(&left_type, &right_type);
            }
        }

        ExpressionKind::LogicalNegation(operand, _) => {
            checks.visit(operand);

            let operand_type = infer_expression_type(
                expression.child(*operand),
                graph,
                inferred_types,
                resolved_types,
                cache,
            );
            let expr_source = expression
                .source()
                .expect("BUG: expression missing source in check_expression");
            checks.collect(check_logical_operand(graph, &operand_type, expr_source));
        }

        ExpressionKind::Comparison(left, op, right) => {
            checks.visit(left);
            checks.visit(right);

            let left_type = infer_expression_type(
                expression.child(*left),
                graph,
                inferred_types,
                resolved_types,
                cache,
            );
            let right_type = infer_expression_type(
                expression.child(*right),
                graph,
                inferred_types,
                resolved_types,
                cache,
            );
            let expr_source = expression
                .source()
                .expect("BUG: expression missing source in check_expression");
            checks.collect(check_comparison_types(
                graph,
                &left_type,
                op,
                &right_type,
                expr_source,
            ));
        }

        ExpressionKind::Arithmetic { first, steps } => {
            checks.visit(first);
            let scope = arithmetic_scope(resolved_types, graph);
            let mut left_type = infer_expression_type(
                expression.child(*first),
                graph,
                inferred_types,
                resolved_types,
                cache,
            );
            for (index, step) in steps.iter().enumerate() {
                let operator = &step.op;
                let right = &step.operand;

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
                    ) = expression.child(*right).kind()
                    {
                        // Always descend into the inner source to catch errors in sub-expressions.
                        checks.visit(inner_source);

                        let inner_type = infer_expression_type(
                            expression.child(*inner_source),
                            graph,
                            inferred_types,
                            resolved_types,
                            cache,
                        );
                        let expr_source = &step.source_location;

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
                                checks.collect(check_unit_conversion_types(
                                    graph,
                                    expression.child(*inner_source),
                                    &inner_type,
                                    conversion_target,
                                    resolved_types,
                                    expr_source,
                                    spec,
                                ));
                                InlineConversionOutcome::CheckedInlineConversionValid
                            } else if inner_type.is_measure() || inner_type.is_number() {
                                // Conversion on the operand fails. Check whether converting the
                                // result of the whole arithmetic expression would be valid instead.
                                let combined_type = compute_arithmetic_result_type(
                                    Arc::clone(&left_type),
                                    operator,
                                    Arc::clone(&inner_type),
                                    &arithmetic_scope(resolved_types, graph),
                                );
                                let combined_is_valid_conversion_source = combined_type
                                    .is_measure()
                                    && (combined_type.same_measure_family(target_type)
                                        || combined_type
                                            .compatible_with_anonymous_measure(target_type)
                                        || target_type
                                            .compatible_with_anonymous_measure(&combined_type));

                                if combined_is_valid_conversion_source {
                                    let inner_label = expression_suggestion_label(
                                        expression.child(*inner_source),
                                    );
                                    let left_label = chain_prefix_suggestion_label(
                                        expression,
                                        *first,
                                        &steps[..index],
                                    );
                                    checks.push(engine_error_at_graph(
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
                                    checks.collect(check_unit_conversion_types(
                                        graph,
                                        expression.child(*inner_source),
                                        &inner_type,
                                        conversion_target,
                                        resolved_types,
                                        expr_source,
                                        spec,
                                    ));
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
                        checks.visit(right);
                    }
                    InlineConversionOutcome::CheckedInlineConversionValid
                    | InlineConversionOutcome::CheckedConversionErrorEmitted => {
                        // Right child was already checked inline; do not descend again.
                    }
                }

                let run_arithmetic_checks = matches!(
                    right_outcome,
                    InlineConversionOutcome::NotHandledInline
                        | InlineConversionOutcome::CheckedInlineConversionValid
                );

                if run_arithmetic_checks {
                    let right_type = infer_expression_type(
                        expression.child(*right),
                        graph,
                        inferred_types,
                        resolved_types,
                        cache,
                    );
                    let expr_source = &step.source_location;
                    checks.collect(check_arithmetic_types(
                        graph,
                        &left_type,
                        &right_type,
                        operator,
                        expr_source,
                    ));
                    checks.collect(arithmetic_plan_time_exactness_planning_errors(
                        graph,
                        expression.child(*right),
                        &left_type,
                        &right_type,
                        operator,
                        expr_source,
                    ));
                }
                let right_type = infer_expression_type(
                    expression.child(step.operand),
                    graph,
                    inferred_types,
                    resolved_types,
                    cache,
                );
                left_type = compute_arithmetic_result_type(left_type, &step.op, right_type, &scope);
            }
        }

        ExpressionKind::UnitConversion(source_expression, target) => {
            checks.visit(source_expression);

            let source_type = infer_expression_type(
                expression.child(*source_expression),
                graph,
                inferred_types,
                resolved_types,
                cache,
            );
            let expr_source = expression
                .source()
                .expect("BUG: expression missing source in check_expression");
            checks.collect(check_unit_conversion_types(
                graph,
                expression.child(*source_expression),
                &source_type,
                target,
                resolved_types,
                expr_source,
                spec,
            ));
        }

        ExpressionKind::MathematicalComputation(op, operand) => {
            checks.visit(operand);

            let operand_type = infer_expression_type(
                expression.child(*operand),
                graph,
                inferred_types,
                resolved_types,
                cache,
            );
            let expr_source = expression
                .source()
                .expect("BUG: expression missing source in check_expression");
            checks.collect(check_mathematical_operand(
                graph,
                op,
                &operand_type,
                expr_source,
            ));
        }

        ExpressionKind::RangeBound(bound, operand) => {
            checks.visit(operand);

            let operand_type = infer_expression_type(
                expression.child(*operand),
                graph,
                inferred_types,
                resolved_types,
                cache,
            );
            if !operand_type.vetoed() && !operand_type.is_undetermined() && !operand_type.is_range()
            {
                let expr_source = expression
                    .source()
                    .expect("BUG: expression missing source in check_expression");
                checks.push(engine_error_at_graph(
                    graph,
                    expr_source,
                    format!(
                        "'{bound}' requires a range, got type '{}'",
                        operand_type.name()
                    ),
                ));
            }
        }

        ExpressionKind::Veto(_) => {}

        ExpressionKind::ResultIsVeto(operand) => {
            checks.visit(operand);
        }

        ExpressionKind::Now => {}

        ExpressionKind::DateRelative(_, date_expr) => {
            checks.visit(date_expr);

            let date_type = infer_expression_type(
                expression.child(*date_expr),
                graph,
                inferred_types,
                resolved_types,
                cache,
            );
            if !date_type.vetoed() && !date_type.is_date() {
                let expr_source = expression
                    .source()
                    .expect("BUG: expression missing source in check_expression");
                checks.push(engine_error_at_graph(
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
            checks.visit(date_expr);

            let date_type = infer_expression_type(
                expression.child(*date_expr),
                graph,
                inferred_types,
                resolved_types,
                cache,
            );
            if !date_type.vetoed() && !date_type.is_date() {
                let expr_source = expression
                    .source()
                    .expect("BUG: expression missing source in check_expression");
                checks.push(engine_error_at_graph(
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
            checks.visit(left);
            checks.visit(right);

            let left_type = infer_expression_type(
                expression.child(*left),
                graph,
                inferred_types,
                resolved_types,
                cache,
            );
            let right_type = infer_expression_type(
                expression.child(*right),
                graph,
                inferred_types,
                resolved_types,
                cache,
            );
            let expr_source = expression
                .source()
                .expect("BUG: expression missing source in check_expression");

            if !(left_type.vetoed() || right_type.vetoed()) {
                let inferred_range_type =
                    infer_range_type_from_endpoint_types(&left_type, &right_type);
                if inferred_range_type.is_undetermined() {
                    checks.push(engine_error_at_graph(
                        graph,
                        expr_source,
                        format!(
                            "Cannot create a range from {} and {}.",
                            left_type.name(),
                            right_type.name()
                        ),
                    ));
                } else if inferred_range_type.is_time_range() {
                    if let (ExpressionKind::Literal(left_lit), ExpressionKind::Literal(right_lit)) = (
                        expression.child(*left).kind(),
                        expression.child(*right).kind(),
                    ) {
                        if let (ValueKind::Time(left_time), ValueKind::Time(right_time)) =
                            (&left_lit.value, &right_lit.value)
                        {
                            if !time_range_endpoints_share_timezone(left_time, right_time) {
                                checks.push(engine_error_at_graph(
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
            checks.visit(offset_expr);

            let offset_type = infer_expression_type(
                expression.child(*offset_expr),
                graph,
                inferred_types,
                resolved_types,
                cache,
            );
            if !offset_type.vetoed()
                && !offset_type.is_duration_like()
                && !offset_type.is_calendar_like()
            {
                let expr_source = expression
                    .source()
                    .expect("BUG: expression missing source in check_expression");
                checks.push(engine_error_at_graph(
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
            checks.visit(value);
            checks.visit(range);

            let value_type = infer_expression_type(
                expression.child(*value),
                graph,
                inferred_types,
                resolved_types,
                cache,
            );
            let range_type = infer_expression_type(
                expression.child(*range),
                graph,
                inferred_types,
                resolved_types,
                cache,
            );
            let expr_source = expression
                .source()
                .expect("BUG: expression missing source in check_expression");

            checks.collect(check_range_containment_types(
                graph,
                &value_type,
                &range_type,
                expr_source,
            ));
        }

        ExpressionKind::Piecewise(arms) => {
            for (condition, result) in arms {
                checks.visit(condition);
                checks.visit(result);
                let condition_type = infer_expression_type(
                    expression.child(*condition),
                    graph,
                    inferred_types,
                    resolved_types,
                    cache,
                );
                if !condition_type.vetoed() && !condition_type.is_boolean() {
                    let expr_source = expression
                        .child(*condition)
                        .source()
                        .expect("BUG: expression missing source in check_expression");
                    checks.push(engine_error_at_graph(
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

        ExpressionKind::ListCommand { list, op, .. } => {
            checks.visit(list);
            let list_type = infer_expression_type(
                expression.child(*list),
                graph,
                inferred_types,
                resolved_types,
                cache,
            );
            let expr_source = expression
                .source()
                .expect("BUG: expression missing source in check_expression");
            if !list_type.is_list() && !list_type.is_undetermined() && !list_type.vetoed() {
                checks.push(engine_error_at_graph(
                    graph,
                    expr_source,
                    format!("`->` commands require a list, got {}", list_type.name()),
                ));
            }
            let mut typing = TypingScope {
                graph,
                inferred_types,
                resolved_types,
                cache,
            };
            checks.splice(check_list_op(
                op,
                expression,
                expression.child(*list),
                &list_type,
                &mut typing,
                expr_source,
            ));
        }
    }

    checks.finish();
}

fn check_list_op(
    op: &ListOp,
    node: ExprRef<'_, ExpressionKind>,
    list: ExprRef<'_, ExpressionKind>,
    list_type: &LemmaType,
    typing: &mut TypingScope<'_>,
    source: &Source,
) -> Vec<CheckStep> {
    let TypingScope {
        graph,
        inferred_types,
        resolved_types,
        ..
    } = *typing;
    if let TypeSpecification::List { element, key } = &list_type.specifications {
        if let TypeSpecification::Spec { spec: identity } = &element.specifications {
            return check_instance_list_op(op, node, list, identity, key, typing, source);
        }
    }
    let mut steps = Vec::new();
    let mut checks = CheckSteps::new(&mut steps);
    match op {
        ListOp::First | ListOp::Last | ListOp::Count => {}
        ListOp::Find { keys } => {
            for key in keys {
                checks.visit(key);
            }
            if keys.len() != 1 {
                checks.push(engine_error_at_graph(
                    graph,
                    source,
                    "numbered `-> find` takes one row number".to_string(),
                ));
            } else {
                checks.collect(check_numbered_find_key(node.child(keys[0]), graph, source));
            }
        }
        ListOp::Reduce { argument, .. } => {
            if let Some(argument) = argument {
                checks.visit(argument);
                checks.push(engine_error_at_graph(
                    graph,
                    source,
                    "value lists do not take a field argument; write `fares -> sum`".to_string(),
                ));
            }
        }
        ListOp::Predicate { right, .. } => checks.visit(right),
        ListOp::InstancePredicate { expr, .. } => {
            checks.visit(expr);
            checks.push(engine_error_at_graph(
                graph,
                source,
                "value-list `any`/`all`/`where` take a comparison (`>= 100 eur`), not a field expression"
                    .to_string(),
            ));
        }
        ListOp::Sort { argument, .. } => {
            if let Some(argument) = argument {
                checks.visit(argument);
                checks.push(engine_error_at_graph(
                    graph,
                    source,
                    "value-list `sort` takes `asc` or `desc`, not a field".to_string(),
                ));
            }
        }
        ListOp::Top { n } => {
            checks.visit(n);
            checks.collect(check_top_count(
                node.child(*n),
                graph,
                inferred_types,
                resolved_types,
                source,
                typing.cache,
            ));
        }
        ListOp::With { rows } => {
            for row in rows {
                checks.visit(row);
            }
        }
        ListOp::WithRow { .. } => {
            unreachable!("BUG: `-> with field: value` converted on a value list")
        }
    }
    if list_type.is_list() {
        let element_type = list_element_type(list_type, graph, resolved_types);
        checks.collect(check_list_element_op(
            op,
            &element_type,
            node,
            typing,
            source,
        ));
    }
    checks.finish();
    steps
}

fn check_top_count(
    n: ExprRef<'_, ExpressionKind>,
    graph: &Graph,
    inferred_types: &HashMap<RulePath, Arc<LemmaType>>,
    resolved_types: &ResolvedTypesMap,
    source: &Source,
    cache: &mut InferExpressionTypeCache,
) -> Result<(), Vec<Error>> {
    let error = |message: String| Err(vec![engine_error_at_graph(graph, source, message)]);
    match n.kind() {
        ExpressionKind::Literal(literal) => match &literal.value {
            ValueKind::Number(count) => {
                let reduced = RationalInteger::try_reduce_ref(count)
                    .map_err(|failure: NumericFailure| {
                        vec![engine_error_at_graph(graph, source, failure.to_string())]
                    })?;
                if reduced.is_integer() && !reduced.numer_is_negative() {
                    Ok(())
                } else {
                    error(format!(
                        "`top` takes a whole number >= 0, got {}",
                        count.display_str()
                    ))
                }
            }
            _ => error(format!("`top` takes a number, got {literal}")),
        },
        ExpressionKind::DataPath(_) | ExpressionKind::RulePath(_) => {
            let n_type = infer_expression_type(n, graph, inferred_types, resolved_types, cache);
            if n_type.is_number() || n_type.is_undetermined() || n_type.vetoed() {
                Ok(())
            } else {
                error(format!("`top` takes a number, got {}", n_type.name()))
            }
        }
        _ => error(
            "`top` takes a number literal or a data or rule reference; compute other counts in a rule"
                .to_string(),
        ),
    }
}

/// Row-type restrictions of value-list commands.
fn check_list_element_op(
    op: &ListOp,
    element_type: &LemmaType,
    node: ExprRef<'_, ExpressionKind>,
    typing: &mut TypingScope<'_>,
    source: &Source,
) -> Result<(), Vec<Error>> {
    let graph = typing.graph;
    if element_type.is_undetermined() || element_type.vetoed() || element_type.is_spec() {
        return Ok(());
    }
    let error = |message: String| Err(vec![engine_error_at_graph(graph, source, message)]);
    let summable = element_type.is_numeric() || element_type.is_ratio();
    let ordered = summable || element_type.is_date() || element_type.is_time();
    match op {
        ListOp::Reduce {
            kind,
            argument: None,
        } => match kind {
            ListReduce::Sum | ListReduce::Avg if !summable => error(format!(
                "`{}` takes number, measure or ratio rows, got {}",
                kind.keyword(),
                element_type.name()
            )),
            ListReduce::Product if element_type.is_measure() => error(format!(
                "`product` of {} rows would have a unit that depends on the row count; multiply in a rule instead",
                element_type.name()
            )),
            ListReduce::Product if !(element_type.is_number() || element_type.is_ratio()) => {
                error(format!(
                    "`product` takes number or ratio rows, got {}",
                    element_type.name()
                ))
            }
            ListReduce::Min | ListReduce::Max if !ordered => error(format!(
                "`{}` needs ordered rows (number, measure, ratio, date, time), got {}",
                kind.keyword(),
                element_type.name()
            )),
            _ => Ok(()),
        },
        ListOp::Sort { argument: None, .. } if !ordered => error(format!(
            "`sort` needs ordered rows (number, measure, ratio, date, time), got {}",
            element_type.name()
        )),
        ListOp::Predicate { op, right, .. } => {
            let right_type = typing.infer(node.child(*right));
            match op {
                RowCondition::Compare(comparison) => {
                    check_comparison_types(graph, element_type, comparison, &right_type, source)
                }
                RowCondition::In => {
                    check_range_containment_types(graph, element_type, &right_type, source)
                }
            }
        }
        ListOp::Find { keys } if keys.len() == 1 => {
            let key_type = typing.infer(node.child(keys[0]));
            if key_type.is_number() || key_type.is_undetermined() || key_type.vetoed() {
                Ok(())
            } else {
                error(format!(
                    "numbered `find` takes a row number, got {}",
                    key_type.name()
                ))
            }
        }
        _ => Ok(()),
    }
}
/// Inputs every typing check reads, plus the inference memo they share.
struct TypingScope<'t> {
    graph: &'t Graph<'t>,
    inferred_types: &'t HashMap<RulePath, Arc<LemmaType>>,
    resolved_types: &'t ResolvedTypesMap<'t>,
    cache: &'t mut InferExpressionTypeCache,
}

impl TypingScope<'_> {
    fn infer(&mut self, expression: ExprRef<'_, ExpressionKind>) -> Arc<LemmaType> {
        infer_expression_type(
            expression,
            self.graph,
            self.inferred_types,
            self.resolved_types,
            self.cache,
        )
    }

    fn error(&self, source: &Source, message: impl Into<String>) -> Error {
        engine_error_at_graph(self.graph, source, message)
    }
}

/// Commands on a list whose rows are instances of `identity`.
fn check_instance_list_op(
    op: &ListOp,
    node: ExprRef<'_, ExpressionKind>,
    list: ExprRef<'_, ExpressionKind>,
    identity: &SpecIdentity,
    key: &ListKey,
    typing: &mut TypingScope<'_>,
    source: &Source,
) -> Vec<CheckStep> {
    let mut steps = Vec::new();
    let mut checks = CheckSteps::new(&mut steps);
    let template = row_fields(typing.graph, identity);
    let row_spec = &identity.spec;
    match op {
        ListOp::First | ListOp::Last | ListOp::Count => {}
        ListOp::Find { keys } => {
            for key_expression in keys {
                checks.visit(key_expression);
            }
            checks.collect(check_instance_find(
                keys, node, key, template, typing, source,
            ));
        }
        ListOp::Reduce { kind, argument } => match argument {
            None => checks.collect(Err(vec![typing.error(
                source,
                format!(
                    "`{}` on `{row_spec}` rows needs a field, for example `-> {} <field>`",
                    kind.keyword(),
                    kind.keyword()
                ),
            )])),
            Some(argument) => {
                checks.visit(argument);
                let argument_type = typing.infer(node.child(*argument));
                checks.collect(check_list_element_op(
                    &ListOp::Reduce {
                        kind: *kind,
                        argument: None,
                    },
                    &argument_type,
                    node,
                    typing,
                    source,
                ));
            }
        },
        ListOp::Sort {
            descending,
            argument,
        } => match argument {
            None => checks.collect(Err(vec![typing.error(
                source,
                format!("`sort` on `{row_spec}` rows needs a field, for example `-> sort <field>`"),
            )])),
            Some(argument) => {
                checks.visit(argument);
                let argument_type = typing.infer(node.child(*argument));
                checks.collect(check_list_element_op(
                    &ListOp::Sort {
                        descending: *descending,
                        argument: None,
                    },
                    &argument_type,
                    node,
                    typing,
                    source,
                ));
            }
        },
        ListOp::Predicate { kind, right, .. } => {
            checks.visit(right);
            checks.collect(Err(vec![typing.error(
                source,
                format!(
                    "`{}` on `{row_spec}` rows takes a row expression such as `price > 3 eur`, not a bare comparison",
                    kind.keyword()
                ),
            )]));
        }
        ListOp::InstancePredicate { kind, expr } => {
            checks.visit(expr);
            let condition_type = typing.infer(node.child(*expr));
            if !condition_type.is_boolean()
                && !condition_type.is_undetermined()
                && !condition_type.vetoed()
            {
                checks.collect(Err(vec![typing.error(
                    source,
                    format!(
                        "`{}` on `{row_spec}` rows needs a boolean row expression, got {}",
                        kind.keyword(),
                        condition_type.name()
                    ),
                )]));
            }
        }
        ListOp::Top { n } => {
            checks.visit(n);
            checks.collect(check_top_count(
                node.child(*n),
                typing.graph,
                typing.inferred_types,
                typing.resolved_types,
                source,
                typing.cache,
            ));
        }
        ListOp::With { rows } => {
            for row in rows {
                checks.visit(row);
            }
            checks.collect(Err(vec![typing.error(
                source,
                format!("`-> with` on `{row_spec}` rows takes `field: value` pairs"),
            )]));
        }
        ListOp::WithRow { fields } => {
            for (path, value) in fields {
                checks.visit(value);
                let field_type =
                    template.end_type(&RowEnd::Data(path.clone()), typing.inferred_types);
                let value_type = typing.infer(node.child(*value));
                if !value_type.is_undetermined()
                    && !value_type.vetoed()
                    && !field_type.same_value_type(value_type.as_ref())
                {
                    checks.collect(Err(vec![typing.error(
                        source,
                        format!(
                            "row field `{}` of `{row_spec}` is {}, got {}",
                            path.input_key(),
                            field_type.name(),
                            value_type.name()
                        ),
                    )]));
                }
            }
            if let ListKey::Fields(key_fields) = key {
                for field in key_fields {
                    let path = match template.end(field) {
                        Some(RowEnd::Data(path)) => path,
                        other => {
                            panic!("BUG: key field '{field}' must be template data, got {other:?}")
                        }
                    };
                    let written = fields.iter().any(|(written, _)| *written == path);
                    if !written && template.default(&path).is_none() {
                        checks.collect(Err(vec![typing.error(
                            source,
                            format!("`-> with` row must set key field `{field}`"),
                        )]));
                    }
                }
            }
            checks.collect(check_appended_row_key(
                list, node, fields, key, typing, source,
            ));
        }
    }
    checks.finish();
    steps
}

/// `-> find` on instance rows: one row number on a numbered list, one value per key field otherwise.
fn check_instance_find(
    keys: &[ExprId],
    node: ExprRef<'_, ExpressionKind>,
    key: &ListKey,
    template: RowFields<'_>,
    typing: &mut TypingScope<'_>,
    source: &Source,
) -> Result<(), Vec<Error>> {
    let fields = match key {
        ListKey::Numbered => {
            if keys.len() != 1 {
                return Err(vec![
                    typing.error(source, "numbered `-> find` takes one row number")
                ]);
            }
            check_numbered_find_key(node.child(keys[0]), typing.graph, source)?;
            let key_type = typing.infer(node.child(keys[0]));
            if key_type.is_number() || key_type.is_undetermined() || key_type.vetoed() {
                return Ok(());
            }
            return Err(vec![typing.error(
                source,
                format!(
                    "numbered `find` takes a row number, got {}",
                    key_type.name()
                ),
            )]);
        }
        ListKey::Fields(fields) => fields,
    };
    if keys.len() != fields.len() {
        return Err(vec![typing.error(
            source,
            format!(
                "`-> find` takes {} key fields ({}), got {}",
                fields.len(),
                fields.join(", "),
                keys.len()
            ),
        )]);
    }
    let mut errors = Vec::new();
    for (key_expression, field) in keys.iter().zip(fields) {
        let key_expression = node.child(*key_expression);
        let end = template
            .end(field)
            .expect("BUG: key field must be data of the template");
        let field_type = template.end_type(&end, typing.inferred_types);
        let key_type = typing.infer(key_expression);
        let result = if field_type.is_range() {
            check_range_containment_types(typing.graph, &key_type, &field_type, source)
        } else {
            check_comparison_types(
                typing.graph,
                &field_type,
                &ComparisonComputation::Is,
                &key_type,
                source,
            )
        };
        if let Err(found) = result {
            errors.push(typing.error(
                source,
                format!(
                    "key field `{field}` is {}, got {}: {}",
                    field_type.name(),
                    key_type.name(),
                    found
                        .iter()
                        .map(|error| error.message().to_string())
                        .collect::<Vec<_>>()
                        .join("; ")
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

/// An appended row with literal key values must not collide with the declared source rows.
/// Type name in a branch mismatch; lists also name their key, which must match too.
fn branch_type_label(lemma_type: &LemmaType) -> String {
    match &lemma_type.specifications {
        TypeSpecification::List {
            key: ListKey::Fields(fields),
            ..
        } => format!("{} (key {})", lemma_type.name(), fields.join(", ")),
        TypeSpecification::List {
            key: ListKey::Numbered,
            ..
        } => format!("{} (numbered rows)", lemma_type.name()),
        _ => lemma_type.name().to_string(),
    }
}

fn check_appended_row_key(
    list: ExprRef<'_, ExpressionKind>,
    node: ExprRef<'_, ExpressionKind>,
    fields: &[(DataPath, ExprId)],
    key: &ListKey,
    typing: &TypingScope<'_>,
    source: &Source,
) -> Result<(), Vec<Error>> {
    let ListKey::Fields(key_fields) = key else {
        return Ok(());
    };
    let ExpressionKind::DataPath(list_path) = list.kind() else {
        return Ok(());
    };
    let Some(list_source) = typing.graph.instance_lists().sources.get(list_path) else {
        return Ok(());
    };
    let template = row_fields(typing.graph, &list_source.identity);
    let key_paths: Vec<DataPath> = key_fields
        .iter()
        .map(|field| match template.end(field) {
            Some(RowEnd::Data(path)) => path,
            other => panic!("BUG: key field '{field}' must be template data, got {other:?}"),
        })
        .collect();
    let mut appended = Vec::with_capacity(key_paths.len());
    for path in &key_paths {
        let written = fields
            .iter()
            .find(|(field, _)| field == path)
            .map(|(_, value)| value);
        let value = match written.map(|value| node.child(*value).kind()) {
            Some(ExpressionKind::Literal(literal)) => literal.as_ref().clone(),
            Some(_) => return Ok(()),
            None => match template.default(path) {
                Some(default) => default,
                None => return Ok(()),
            },
        };
        appended.push(value);
    }
    let mut keyed: Vec<KeyedRow> = Vec::with_capacity(list_source.rows.len() + 1);
    for (index, row) in list_source.rows.iter().enumerate() {
        let mut values = Vec::with_capacity(key_paths.len());
        for path in &key_paths {
            let field_type = template.end_type(&RowEnd::Data(path.clone()), typing.inferred_types);
            let bound = row.fields.iter().find(|(field, _)| field == path);
            let value = match bound {
                Some((_, DataSource::Literal(bound))) => {
                    instance_list::typed_from_bound(bound, &field_type)
                }
                Some((_, DataSource::Reference(_))) => return Ok(()),
                None => match template.default(path) {
                    Some(default) => default,
                    None => return Ok(()),
                },
            };
            values.push(value);
        }
        keyed.push(KeyedRow {
            label: format!("row {}", index + 1),
            values,
        });
    }
    keyed.push(KeyedRow {
        label: "the appended row".to_string(),
        values: appended,
    });
    match instance_list::key_conflict(key_fields, &keyed) {
        Some(message) => Err(vec![typing.error(source, message)]),
        None => Ok(()),
    }
}

fn check_numbered_find_key(
    key: ExprRef<'_, ExpressionKind>,
    graph: &Graph,
    source: &Source,
) -> Result<(), Vec<Error>> {
    let ExpressionKind::Literal(literal) = key.kind() else {
        return Ok(());
    };
    let ValueKind::Number(n) = &literal.value else {
        return Ok(());
    };
    let decimal = n
        .try_to_decimal()
        .map_err(|failure| vec![engine_error_at_graph(graph, source, failure.to_string())])?;
    if decimal.fract() != Decimal::ZERO {
        return Err(vec![engine_error_at_graph(
            graph,
            source,
            format!("row number must be a whole number, got {decimal}"),
        )]);
    }
    if decimal <= Decimal::ZERO {
        return Err(vec![engine_error_at_graph(
            graph,
            source,
            format!("row number must be >= 1, got {decimal}"),
        )]);
    }
    Ok(())
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
    cache: &mut InferExpressionTypeCache,
) -> Result<(), Vec<Error>> {
    let mut errors = Vec::new();
    let mut scratch = CheckScratch::default();

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

        let default_result = rule_node.expression.at(branches[0].1);
        collect(
            check_expression(
                default_result,
                graph,
                inferred_types,
                resolved_types,
                spec,
                cache,
                &mut scratch,
            ),
            &mut errors,
        );
        let default_type =
            infer_expression_type(default_result, graph, inferred_types, resolved_types, cache);

        if default_type.is_anonymous_measure() {
            if let Some(decomp) = default_type.measure_type_decomposition() {
                if !decomp.is_empty() && anonymous_rule_boundary_requires_rejection() {
                    let default_source = default_result
                        .source()
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
            let result = rule_node.expression.at(*result);
            if let Some(condition) = condition {
                let condition_expression = rule_node.expression.at(*condition);
                collect(
                    check_expression(
                        condition_expression,
                        graph,
                        inferred_types,
                        resolved_types,
                        spec,
                        cache,
                        &mut scratch,
                    ),
                    &mut errors,
                );
                let condition_type = infer_expression_type(
                    condition_expression,
                    graph,
                    inferred_types,
                    resolved_types,
                    cache,
                );
                if !condition_type.vetoed()
                    && !condition_type.is_boolean()
                    && !condition_type.is_undetermined()
                {
                    let condition_source = condition_expression
                        .source()
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
                check_expression(
                    result,
                    graph,
                    inferred_types,
                    resolved_types,
                    spec,
                    cache,
                    &mut scratch,
                ),
                &mut errors,
            );
            let result_type =
                infer_expression_type(result, graph, inferred_types, resolved_types, cache);

            if result_type.is_anonymous_measure() {
                if let Some(decomp) = result_type.measure_type_decomposition() {
                    if !decomp.is_empty() && anonymous_rule_boundary_requires_rejection() {
                        let branch_source = result
                            .source()
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

                        let mut location_parts = vec![format!(
                            "{}:{}:{}",
                            rule_source.source_type, rule_source.span.line, rule_source.span.col
                        )];

                        if let Some(loc) = default_result.source() {
                            location_parts.push(format!(
                                "default branch at {}:{}:{}",
                                loc.source_type, loc.span.line, loc.span.col
                            ));
                        }
                        if let Some(loc) = result.source() {
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
                            branch_type_label(existing_type),
                            branch_index,
                            branch_type_label(&result_type)),
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

/// Infer the types of all rules in topological order without validating expressions.
/// Returns a map from rule path to its inferred type, and the Errors of recursive
/// rules whose type cannot be seeded or does not match its seed.
fn infer_rule_types(
    graph: &Graph,
    rule_order: &[RulePath],
    resolved_types: &ResolvedTypesMap,
    cache: &mut InferExpressionTypeCache,
) -> (HashMap<RulePath, Arc<LemmaType>>, Vec<Error>) {
    let mut computed_types: HashMap<RulePath, Arc<LemmaType>> = HashMap::new();
    let mut errors = Vec::new();

    for rule_path in rule_order {
        let rule_node = graph
            .rules()
            .get(rule_path)
            .expect("BUG: rule from topological sort not in graph");
        let branches = &rule_node.branches;

        if branches.is_empty() {
            continue;
        }

        let seed = if rule_node.recursive {
            match recursive_rule_seed(rule_path, graph, &mut computed_types, resolved_types, cache)
            {
                Ok(seed) => Some(seed),
                Err(message) => {
                    errors.push(reference_error(graph.main_spec, &rule_node.source, message));
                    computed_types
                        .insert(rule_path.clone(), Arc::new(LemmaType::undetermined_type()));
                    continue;
                }
            }
        } else {
            None
        };

        let body_types = branches.iter().map(|(_, result)| {
            infer_expression_type(
                rule_node.expression.at(*result),
                graph,
                &computed_types,
                resolved_types,
                cache,
            )
        });
        let rule_type = piecewise_type(body_types);
        if let Some(seed) = seed {
            let determined = !rule_type.vetoed() && !rule_type.is_undetermined();
            if determined && !rule_type.same_value_type(&seed) {
                errors.push(reference_error(
                    graph.main_spec,
                    &rule_node.source,
                    format!(
                        "recursive rule `{}` is {} from its non-recursive part, but {} as a whole",
                        rule_path.rule,
                        seed.name(),
                        rule_type.name()
                    ),
                ));
            }
        }
        computed_types.insert(rule_path.clone(), rule_type);
    }

    (computed_types, errors)
}

/// Type of recursive rule `rule_path` from the parts of its body that do not read it
/// on rows: `own + components -> sum(total)` is the type of `own`. Leaves the seed in
/// `computed_types` and clears the inference cache, so the full body is inferred
/// against it.
fn recursive_rule_seed(
    rule_path: &RulePath,
    graph: &Graph,
    computed_types: &mut HashMap<RulePath, Arc<LemmaType>>,
    resolved_types: &ResolvedTypesMap,
    cache: &mut InferExpressionTypeCache,
) -> Result<Arc<LemmaType>, String> {
    let rule_node = graph
        .rules()
        .get(rule_path)
        .expect("BUG: recursive rule not in graph");
    computed_types.insert(rule_path.clone(), Arc::new(LemmaType::undetermined_type()));
    let seed = rule_node.branches.iter().find_map(|(_, result)| {
        seed_type(
            rule_node.expression.at(*result),
            graph,
            computed_types,
            resolved_types,
            cache,
        )
    });
    cache.clear();
    let seed = seed.ok_or_else(|| {
        format!(
            "recursive rule `{}` needs a non-recursive part to determine its type",
            rule_path.rule
        )
    })?;
    computed_types.insert(rule_path.clone(), Arc::clone(&seed));
    Ok(seed)
}

/// Type `expression` has whatever the recursive rule being seeded (typed undetermined
/// in `computed_types`) turns out to be, when its non-recursive parts fix it.
fn seed_type(
    expression: ExprRef<'_, ExpressionKind>,
    graph: &Graph,
    computed_types: &HashMap<RulePath, Arc<LemmaType>>,
    resolved_types: &ResolvedTypesMap,
    cache: &mut InferExpressionTypeCache,
) -> Option<Arc<LemmaType>> {
    let mut pending = vec![expression];
    while let Some(node) = pending.pop() {
        let inferred = infer_expression_type(node, graph, computed_types, resolved_types, cache);
        if !inferred.vetoed() && !inferred.is_undetermined() {
            return Some(inferred);
        }
        let (fixed, children) = seed_candidates(node);
        if fixed.is_some() {
            return fixed;
        }
        pending.extend(children.into_iter().rev().map(|id| node.child(id)));
    }
    None
}

/// The type a node's kind fixes without looking inside, and otherwise the children whose
/// seeds are tried in order.
fn seed_candidates(node: ExprRef<'_, ExpressionKind>) -> (Option<Arc<LemmaType>>, Vec<ExprId>) {
    match node.kind() {
        ExpressionKind::Arithmetic { first, steps }
            if steps.iter().all(|step| {
                matches!(
                    step.op,
                    ArithmeticComputation::Add | ArithmeticComputation::Subtract
                )
            }) =>
        {
            (
                None,
                std::iter::once(*first)
                    .chain(steps.iter().map(|step| step.operand))
                    .collect(),
            )
        }
        ExpressionKind::Comparison(..)
        | ExpressionKind::LogicalAnd { .. }
        | ExpressionKind::LogicalNegation(..) => {
            (Some(primitive_boolean_arc().clone()), Vec::new())
        }
        ExpressionKind::ListCommand { op, .. } => match op {
            ListOp::Count => (Some(primitive_number_arc().clone()), Vec::new()),
            ListOp::Predicate { kind, .. } | ListOp::InstancePredicate { kind, .. } => match kind {
                ListPredKind::Any | ListPredKind::All => {
                    (Some(primitive_boolean_arc().clone()), Vec::new())
                }
                ListPredKind::Where => (None, Vec::new()),
            },
            ListOp::Reduce {
                kind: ListReduce::Sum | ListReduce::Min | ListReduce::Max | ListReduce::Avg,
                argument: Some(argument),
            } => (None, vec![*argument]),
            _ => (None, Vec::new()),
        },
        ExpressionKind::Piecewise(arms) => (None, arms.iter().map(|(_, result)| *result).collect()),
        _ => (None, Vec::new()),
    }
}

/// Unit name -> (owning type name when local to the spec, decomposition, factor).
type UnitDecompLookup = HashMap<
    String,
    (
        Option<String>,
        BaseMeasureVector,
        crate::computation::rational::RationalInteger,
    ),
>;

fn local_owner_name(owner: &PlanningOwner) -> Option<String> {
    match owner {
        PlanningOwner::Local { type_name } => Some(type_name.clone()),
        PlanningOwner::PrimitiveRatio | PlanningOwner::Imported { .. } => None,
    }
}

fn unwrap_unique_resolved_type(arc: Arc<LemmaType>) -> LemmaType {
    Arc::try_unwrap(arc).expect("BUG: resolved LemmaType Arc unique during type resolution")
}

fn declared_measure_decomposition(lemma_type: &LemmaType) -> BaseMeasureVector {
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
            let family = lemma_type
                .measure_family()
                .expect("BUG: named measure type must carry a family");
            [(semantics::Dimension::Family(family.clone()), 1i32)]
                .into_iter()
                .collect()
        }
    }
}

/// Local type owning `unit_name`; `None` when unknown or imported.
fn local_owning_measure_type_for_unit(
    unit_name: &str,
    lookup: &UnitDecompLookup,
    unit_index: &PlanningUnitIndex,
    resolved: &IndexMap<String, Arc<LemmaType>>,
) -> Option<String> {
    if let Some((owning_measure_name, _, _)) = lookup.get(unit_name) {
        return owning_measure_name.clone();
    }
    unit_index
        .resolve_owner(unit_name, resolved)
        .ok()
        .and_then(|(_, owner)| local_owner_name(owner))
}

/// Order compound measure types so every referenced unit from another compound type is resolved first.
fn sort_derived_measure_types_for_resolution(
    spec_name: &str,
    derived_measure_type_names: Vec<String>,
    resolved: &IndexMap<String, Arc<LemmaType>>,
    lookup: &UnitDecompLookup,
    unit_index: &PlanningUnitIndex,
    type_sources: &HashMap<String, Source>,
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
                let Some(owning_measure_name) = local_owning_measure_type_for_unit(
                    factor_unit_name,
                    lookup,
                    unit_index,
                    resolved,
                ) else {
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
            type_sources.get(&cycle_type_names[0]).cloned(),
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
    resolved_types: &mut ResolvedSpecTypes,
    already_resolved: &ResolvedTypesMap<'_>,
    at: &EffectiveDate,
) -> Vec<Error> {
    let local = local_type_entries(resolved_types);
    let resolved = &mut resolved_types.resolved;
    let declared_suggestions = &mut resolved_types.declared_suggestions;
    let mut errors = Vec::new();
    for (type_name, def) in data_defs {
        let ParentType::Ranged { .. } = &def.parent else {
            continue;
        };
        let element = element_parent_type(&def.parent);
        if matches!(element, ParentType::Primitive { .. }) {
            continue;
        }
        let scope = ParentLookup {
            resolver,
            spec,
            source: &def.source,
            at,
            already_resolved,
            resolved: &local,
            site: TypeUseSite::DataDefinition,
        };
        let endpoint_type = match lookup_parent_type(&scope, element) {
            Ok((.., endpoint_type)) => endpoint_type,
            Err(lookup_errors) => {
                errors.extend(lookup_errors);
                continue;
            }
        };
        let range_spec = endpoint_type
            .specifications
            .range_from_element()
            .expect("BUG: ranged element was checked rangeable when the type was resolved");
        let lemma_type = resolved
            .get_mut(type_name.as_str())
            .expect("BUG: ranged type resolved before its range spec is refreshed");
        let mut updated = lemma_type.as_ref().clone();
        updated.specifications = range_spec;
        *lemma_type = Arc::new(updated);

        if let Some(BoundValueKind {
            value: ValueKind::Range(left, right),
            ..
        }) = declared_suggestions.get_mut(type_name.as_str())
        {
            let left_typed = left.as_ref().clone();
            let right_typed = right.as_ref().clone();
            let coerced_left =
                match Graph::coerce_literal_to_schema_type(&left_typed, &endpoint_type) {
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
            let coerced_right =
                match Graph::coerce_literal_to_schema_type(&right_typed, &endpoint_type) {
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
                BoundValueKind {
                    value: ValueKind::Range(Box::new(coerced_left), Box::new(coerced_right)),
                    unit: None,
                };
        }
    }
    errors
}

/// Apply range-local constraints after [`refresh_named_range_specs`] for named ranged types.
fn apply_deferred_named_range_constraints(
    spec: &LemmaSpec,
    data_defs: &HashMap<String, DataTypeDef>,
    resolved: &mut TypeMap,
    declared_suggestions: &mut IndexMap<String, BoundValueKind>,
    declared_values: &mut IndexMap<String, BoundValueKind>,
    type_sources: &HashMap<String, Source>,
    unit_index: &PlanningUnitIndex,
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
        let mut declared_value: Option<RawSuggestion> = None;
        let resolve = UnitTokenResolver::Planning {
            unit_index,
            named_types: resolved,
        };
        match apply_constraints_to_spec(
            spec,
            &constraint_application_type_name(&def.parent, type_name),
            lemma_type.specifications.clone(),
            constraints,
            &def.source,
            ConstraintSuggestionSink {
                suggestion: &mut declared_suggestion,
                value: &mut declared_value,
                resolve_unit: Some(&resolve),
            },
        ) {
            Ok(updated_specs) => {
                let mut updated = lemma_type.as_ref().clone();
                updated.specifications = updated_specs;
                let updated_arc = Arc::new(updated);
                if let Some(raw) = declared_suggestion {
                    match bound_value_kind_from_raw_suggestion(
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
                if let Some(raw) = declared_value {
                    match bound_value_kind_from_raw_suggestion(
                        raw,
                        &updated_arc.specifications,
                        type_name.as_str(),
                    ) {
                        Ok(value_kind) => {
                            declared_values.insert(type_name.clone(), value_kind);
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

/// Where a type is written, for planning error wording.
#[derive(Clone, Copy)]
enum TypeUseSite {
    DataDefinition,
    RuleListType,
}

impl fmt::Display for TypeUseSite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TypeUseSite::DataDefinition => write!(f, "data definition"),
            TypeUseSite::RuleListType => write!(f, "list type in a rule"),
        }
    }
}

/// Scope a parent type name resolves in: the spec's local types so far, the specs it
/// imports, and where the type is written.
#[derive(Clone, Copy)]
struct ParentLookup<'s, 'a> {
    resolver: &'s TypeResolver<'a>,
    spec: &'s LemmaSpec,
    source: &'s Source,
    at: &'s EffectiveDate,
    already_resolved: &'s ResolvedTypesMap<'a>,
    resolved: &'s HashMap<String, ResolvedTypeEntry>,
    site: TypeUseSite,
}

fn lookup_parent_type(
    scope: &ParentLookup<'_, '_>,
    parent: &ParentType,
) -> Result<ParentLookupOk, Vec<Error>> {
    let mut wrappers = Vec::new();
    let mut leaf = parent;
    while let ParentType::List { inner } | ParentType::Ranged { inner } = leaf {
        wrappers.push(leaf);
        leaf = inner.as_ref();
    }
    let mut lookup = lookup_leaf_parent_type(scope, leaf)?;
    while let Some(wrapper) = wrappers.pop() {
        lookup = lookup_wrapper_parent_type(scope, wrapper, lookup)?;
    }
    Ok(lookup)
}

/// Lookup of a `list` or `range` wrapper, given the lookup of its `inner`.
fn lookup_wrapper_parent_type(
    scope: &ParentLookup<'_, '_>,
    wrapper: &ParentType,
    inner_lookup: ParentLookupOk,
) -> Result<ParentLookupOk, Vec<Error>> {
    let ParentLookup { spec, source, .. } = *scope;
    let (element_specs, .., element_type) = inner_lookup;
    match wrapper {
        ParentType::List { inner } => {
            let element = if matches!(inner.as_ref(), ParentType::Ranged { .. }) {
                Arc::new(LemmaType::primitive(element_specs))
            } else {
                element_type
            };
            let lemma_type = Arc::new(LemmaType::list_of(element));
            Ok((lemma_type.specifications.clone(), None, None, lemma_type))
        }
        ParentType::Ranged { inner } => {
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
            Ok((range_spec, None, None, element_type))
        }
        ParentType::Primitive { .. } | ParentType::Custom { .. } | ParentType::Qualified { .. } => {
            unreachable!("BUG: lookup_wrapper_parent_type called on a leaf parent")
        }
    }
}

/// Lookup of a parent that is not a `list` or `range` wrapper.
fn lookup_leaf_parent_type(
    scope: &ParentLookup<'_, '_>,
    parent: &ParentType,
) -> Result<ParentLookupOk, Vec<Error>> {
    let ParentLookup {
        resolver,
        spec,
        source,
        at,
        already_resolved,
        resolved,
        site,
    } = *scope;
    match parent {
        ParentType::List { .. } | ParentType::Ranged { .. } => {
            unreachable!("BUG: list and range wrappers are unwound before the leaf lookup")
        }
        ParentType::Primitive { primitive: kind } => {
            let lemma_type = Arc::clone(semantics::lemma_type_for_primitive(*kind));
            Ok((
                lemma_type.as_ref().specifications.clone(),
                None,
                None,
                lemma_type,
            ))
        }
        ParentType::Custom { name } => {
            if let Some((parent_type, parent_suggestion, parent_value)) =
                resolved.get(name.as_str())
            {
                return Ok((
                    parent_type.as_ref().specifications.clone(),
                    parent_suggestion.clone(),
                    parent_value.clone(),
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
                        && data.value.uses_spec_ref().is_some()
                }) {
                    return Err(vec![Error::validation_with_context(
                        format!(
                            "'{name}' names a spec import alias, not a type. Use `data x: {name}.TypeName` after `uses`"
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
                            "Unknown parent '{parent}' for {site}: '{name}' is a local rule, not a type. Use a type name, or a reference expression if you meant the rule's value."
                        ),
                        Some(source.clone()),
                        None::<String>,
                        Some(spec),
                        None,
                    )]);
                }
                let mut imported_aliases: Vec<&str> = Vec::new();
                for data_row in &spec.data {
                    let Some(spec_ref) = data_row.value.uses_spec_ref() else {
                        continue;
                    };
                    let import_alias = &data_row.reference.name;
                    let (_, imported_spec) =
                        match resolver.resolve_spec_for_import(spec, spec_ref, source, at) {
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
                    if imported_resolved.resolved.contains_key(name.as_str()) {
                        imported_aliases.push(import_alias.as_str());
                    }
                }
                if !imported_aliases.is_empty() {
                    let forms = imported_aliases
                        .iter()
                        .map(|alias| format!("{alias}.{name}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    return Err(vec![Error::validation_with_context(
                        format!(
                            "Unknown parent '{parent}' for {site}: '{name}' is declared by import{}; write {forms}",
                            if imported_aliases.len() == 1 {
                                format!(" '{}'", imported_aliases[0])
                            } else {
                                format!("s {}", imported_aliases.join(", "))
                            }
                        ),
                        Some(source.clone()),
                        None::<String>,
                        Some(spec),
                        None,
                    )]);
                }
                return Err(vec![Error::validation_with_context(
                    format!(
                        "Unknown parent '{parent}' for {site}. Parent must be defined before use. Valid primitive types are: boolean, measure, number, ratio, text, date, time"
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
            let Some(spec_ref) = spec.data.iter().find_map(|row| {
                if row.reference.segments.is_empty() && row.reference.name == *spec_alias {
                    row.value.uses_spec_ref()
                } else {
                    None
                }
            }) else {
                return Err(vec![Error::validation_with_context(
                    format!("'{spec_alias}' is not a `uses` import alias in this spec"),
                    Some(source.clone()),
                    Some(format!(
                        "Add `uses {spec_alias}: <target_spec>`, then write `{spec_alias}.<type>`."
                    )),
                    Some(spec),
                    None,
                )]);
            };
            let (_, target_spec) =
                match resolver.resolve_spec_for_import(spec, spec_ref, source, at) {
                    Ok(import_pair) => import_pair,
                    Err(error) => return Err(vec![error]),
                };
            match inner.as_ref() {
                ParentType::Primitive { primitive: kind } => {
                    let lemma_type = Arc::clone(semantics::lemma_type_for_primitive(*kind));
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
                        .find(|(_, imported, _)| discovery::same_loaded_spec(imported, target_spec))
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
                    let Some(parent_type) = resolved_spec_types.resolved.get(name.as_str()) else {
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
                            .source_value_defaults
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
                ParentType::List { .. } => Err(vec![Error::validation_with_context(
                    "Nested list parent types are invalid",
                    Some(source.clone()),
                    None::<String>,
                    Some(spec),
                    None,
                )]),
            }
        }
    }
}

/// Order `type_names` so each type follows its local element parent. A cycle is a planning Error.
fn topo_sort_type_names(
    spec: &LemmaSpec,
    data_defs: &HashMap<String, DataTypeDef>,
    mut type_names: Vec<String>,
) -> Result<Vec<String>, Vec<Error>> {
    type_names.sort();
    if type_names.is_empty() {
        return Ok(type_names);
    }
    let type_index: HashMap<&str, usize> = type_names
        .iter()
        .enumerate()
        .map(|(index, name)| (name.as_str(), index))
        .collect();
    let type_count = type_names.len();
    let mut in_degree = vec![0usize; type_count];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); type_count];
    for (child_index, type_name) in type_names.iter().enumerate() {
        let custom_parent = match element_parent_type(&data_defs[type_name].parent) {
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
    Ok(sorted)
}

/// Split local type declarations into scalar types and list types.
///
/// A list type (`money list`, or a type extending one) is resolved after every scalar
/// pass has finished, so its element is the final element type.
fn partition_list_type_names(
    data_defs: &HashMap<String, DataTypeDef>,
) -> (Vec<String>, Vec<String>) {
    fn has_list(parent: &ParentType) -> bool {
        let mut current = parent;
        loop {
            match current {
                ParentType::List { .. } => return true,
                ParentType::Ranged { inner } | ParentType::Qualified { inner, .. } => {
                    current = inner.as_ref();
                }
                ParentType::Primitive { .. } | ParentType::Custom { .. } => return false,
            }
        }
    }
    let mut list_names: HashSet<String> = data_defs
        .iter()
        .filter(|(_, def)| has_list(&def.parent))
        .map(|(name, _)| name.clone())
        .collect();
    loop {
        let before = list_names.len();
        for (name, def) in data_defs {
            if let ParentType::Custom { name: parent } = element_parent_type(&def.parent) {
                if list_names.contains(parent) {
                    list_names.insert(name.clone());
                }
            }
        }
        if list_names.len() == before {
            break;
        }
    }
    let scalar_names = data_defs
        .keys()
        .filter(|name| !list_names.contains(*name))
        .cloned()
        .collect();
    (scalar_names, list_names.into_iter().collect())
}

type TypeMap = IndexMap<String, Arc<LemmaType>>;

fn resolve_measure_decompositions(
    spec_name: &str,
    this_spec: &SpecIdentity,
    mut resolved: TypeMap,
    unit_index: &PlanningUnitIndex,
    type_sources: &HashMap<String, Source>,
) -> (TypeMap, Vec<Error>) {
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
        let owned = unwrap_unique_resolved_type(
            resolved
                .shift_remove(type_name)
                .expect("BUG: type_name comes from resolved's own keys"),
        );
        let base_decomp = declared_measure_decomposition(&owned);

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

        let updated = owned.map_measure(|units, _| {
            let units = units.map(|u| u.with_decomposition(base_decomp.clone()));
            (units, Some(base_decomp.clone()))
        });
        resolved.insert(type_name.clone(), Arc::new(updated));
    }

    for lemma_type in resolved.values_mut() {
        let TypeSpecification::MeasureRange { units, .. } = &lemma_type.specifications else {
            continue;
        };
        if units.is_empty() {
            continue;
        }
        let family = lemma_type
            .family()
            .expect("BUG: measure range declaring units must be a named custom type");
        let range_decomp: BaseMeasureVector =
            [(semantics::Dimension::Family(family.clone()), 1i32)]
                .into_iter()
                .collect();
        let TypeSpecification::MeasureRange {
            units,
            decomposition,
            ..
        } = &mut Arc::get_mut(lemma_type)
            .expect("BUG: resolved LemmaType Arc unique during type resolution")
            .specifications
        else {
            unreachable!("BUG: matched MeasureRange above");
        };
        *units = std::mem::take(units).map(|u| u.with_decomposition(range_decomp.clone()));
        *decomposition = Some(range_decomp);
    }

    let mut lookup = UnitDecompLookup::new();

    for (unit_name, owner) in unit_index.iter_owners() {
        let unit_name = unit_name.to_string();
        let lemma_type = owner.lemma_type(&resolved);
        if let TypeSpecification::Measure {
            decomposition: Some(decomposition),
            units,
            ..
        } = &lemma_type.specifications
        {
            let measure_name = local_owner_name(owner);
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
            let is_defining_type = lemma_type.measure_family().is_some_and(|family| {
                family.is_declared_in(this_spec) && family.name == *type_name
            });
            if !is_defining_type {
                continue;
            }
            for unit in units.iter() {
                let declared_by_other_family =
                    unit_index.owners_for(&unit.name).iter().any(|owner| {
                        !owner
                            .lemma_type(&resolved)
                            .same_measure_family(lemma_type.as_ref())
                    });
                if declared_by_other_family {
                    continue;
                }
                lookup.insert(
                    unit.name.clone(),
                    (
                        Some(type_name.clone()),
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
        unit_index,
        type_sources,
    ) {
        Ok(sorted) => sorted,
        Err(error) => {
            errors.push(error);
            return (resolved, errors);
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
                let simple_decomp = declared_measure_decomposition(&resolved[type_name]);

                if let Some(existing) = &resolved_type_decomp {
                    if existing != &simple_decomp {
                        unit_errors.push(Error::validation(
                            format!(
                                "In spec '{}': measure type '{}' has inconsistent unit decompositions. \
                                 Unit '{}' is a simple unit (decomposition {}) but other units \
                                 have decomposition {}.",
                                spec_name,
                                type_name,
                                unit.name,
                                semantics::format_signature_operator_style(
                                    &semantics::base_measure_vector_as_signature(&simple_decomp)
                                ),
                                semantics::format_signature_operator_style(
                                    &semantics::base_measure_vector_as_signature(existing)
                                )
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
                unit_index,
                &resolved,
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

        let owned = unwrap_unique_resolved_type(
            resolved
                .shift_remove(type_name)
                .expect("BUG: type_name comes from resolved's own keys"),
        );

        let updated = owned.map_measure(|units, _| {
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
                    (
                        Some(type_name.clone()),
                        type_decomp.clone(),
                        unit.factor.clone(),
                    ),
                );
            }
        }
        resolved.insert(type_name.clone(), Arc::new(updated));
    }

    let resolved = canonicalize_unit_signatures(resolved);

    (resolved, errors)
}

/// Functional wrapper around the still-`&mut`-based
/// [`semantics::finalize_measure_unit_constraint_magnitudes`]. Consumes the
/// `LemmaType`, performs the validation, and returns either the updated value
/// or an error message (the type is dropped on failure).
fn finalize_lemma_measure_magnitudes(
    lemma_type: LemmaType,
    declared_suggestion: Option<&BoundValueKind>,
    type_name: &str,
) -> Result<LemmaType, String> {
    let LemmaType {
        name,
        mut specifications,
        extends,
    } = lemma_type;
    semantics::finalize_measure_unit_constraint_magnitudes(
        &mut specifications,
        declared_suggestion.map(|bound| &bound.value),
        type_name,
    )?;
    Ok(LemmaType {
        name,
        specifications,
        extends,
    })
}

fn finalize_measure_magnitudes_in_resolved(
    resolved: IndexMap<String, Arc<LemmaType>>,
    declared_suggestions: &IndexMap<String, BoundValueKind>,
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
            let owned = unwrap_unique_resolved_type(arc);
            let lemma_type = match finalize_lemma_measure_magnitudes(
                owned.clone(),
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
                    owned
                }
            };
            acc.insert(type_name, Arc::new(lemma_type));
            (acc, errors)
        },
    )
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
                Arc::new(canonicalize_lemma_unit_signatures(
                    unwrap_unique_resolved_type(arc),
                )),
            )
        })
        .collect()
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
    unit_index: &PlanningUnitIndex,
    resolved: &IndexMap<String, Arc<LemmaType>>,
    source: Option<&Source>,
) -> Result<
    (
        BaseMeasureVector,
        crate::computation::rational::RationalInteger,
    ),
    Error,
> {
    use crate::computation::bigint::BigInt;
    use crate::computation::rational::{checked_mul, checked_pow_integer};

    let mut result: BaseMeasureVector = BaseMeasureVector::new();
    let mut derived_factor = unit.factor.clone();

    for (measure_ref, exponent) in &unit.derived_measure_factors {
        let (owning_measure_name, owning_decomp, unit_factor) = if let Some(entry) =
            lookup.get(measure_ref.as_str())
        {
            (entry.0.clone(), entry.1.clone(), entry.2.clone())
        } else {
            let (bare, owner) = unit_index
                .resolve_owner(measure_ref, resolved)
                .map_err(|err| {
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
            let owning_type = owner.lemma_type(resolved);
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
            (local_owner_name(owner), decomposition.clone(), factor)
        };
        let owning_decomp = &owning_decomp;
        let unit_factor = &unit_factor;

        if owning_measure_name.as_deref() == Some(declaring_type_name) {
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
            let combined = dim_exp
                .checked_mul(*exponent)
                .and_then(|value| accumulate(&mut result, dim, value));
            if combined.is_none() {
                return Err(unit_exponent_limit_error(
                    spec_name,
                    &unit.name,
                    declaring_type_name,
                    source,
                ));
            }
        }

        let component_contribution =
            checked_pow_integer(unit_factor, &BigInt::from_i64(i64::from(*exponent))).map_err(
                |error| {
                    overflow_to_validation_error(
                        spec_name,
                        &unit.name,
                        declaring_type_name,
                        measure_ref,
                        error,
                        source,
                    )
                },
            )?;
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

fn unit_exponent_limit_error(
    spec_name: &str,
    unit_name: &str,
    declaring_type_name: &str,
    source: Option<&Source>,
) -> Error {
    Error::validation(
        format!(
            "In spec '{}': unit '{}' in measure type '{}' has a unit exponent exceeding the maximum of {}",
            spec_name,
            unit_name,
            declaring_type_name,
            crate::limits::MAX_UNIT_EXPONENT
        ),
        source.cloned(),
        None::<String>,
    )
}

/// `None` when the accumulated exponent leaves [`crate::limits::MAX_UNIT_EXPONENT`].
fn accumulate(
    result: &mut BaseMeasureVector,
    dim: &semantics::Dimension,
    value: i32,
) -> Option<()> {
    let entry = result.entry(dim.clone()).or_insert(0);
    *entry = entry.checked_add(value)?;
    if entry.unsigned_abs() > crate::limits::MAX_UNIT_EXPONENT.unsigned_abs() {
        return None;
    }
    if *entry == 0 {
        result.remove(dim);
    }
    Some(())
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
        let mut interners = instance_list::PassInterners::default();
        match Graph::build(
            ctx,
            &repository,
            main_spec_ref,
            ordered_dependencies,
            &effective,
            limits,
            &mut interners,
        ) {
            Ok((graph, _)) => Ok(graph),
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
        ast::Expression::leaf(ast::ExpressionKind::Literal(value), Some(test_source()))
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
            expression: ast::Expression::leaf(
                ast::ExpressionKind::Literal(Value::Number(1.into())),
                None,
            ),
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
        let mut table = ast::ExprTable::default();
        let first = table.push(
            ast::ExpressionKind::Literal(Value::Number(2.into())),
            Some(test_source()),
        );
        let operand = table.push(ast::ExpressionKind::Literal(Value::Number(1.into())), None);
        let root = table.push(
            ast::ExpressionKind::Arithmetic {
                first,
                steps: vec![ast::ChainStep {
                    op: ast::ArithmeticComputation::Add,
                    operand,
                    source_location: test_source(),
                }],
            },
            Some(test_source()),
        );
        let outer = ast::Expression::new(table, root);
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

        let missing_data_expr = ast::Expression::leaf(
            ast::ExpressionKind::Reference(Reference {
                segments: Vec::new(),
                name: "nonexistent".to_string(),
            }),
            Some(test_source()),
        );

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

    fn rule_node_of<'g>(graph: &'g Graph<'static>, rule: &str) -> &'g RuleNode<'static> {
        graph
            .rules()
            .iter()
            .find(|(path, _)| path.rule == rule)
            .map(|(_, node)| node)
            .expect("rule in graph")
    }

    fn graph_of(source: &str) -> Graph<'static> {
        let specs = crate::parse(
            source,
            crate::parsing::source::SourceType::Volatile,
            &crate::ResourceLimits::default(),
        )
        .expect("fixture parses")
        .into_flattened_specs();
        let s = specs.iter().find(|d| d.name == "s").expect("spec s");
        build_graph(s, &specs).expect("fixture plans")
    }

    #[test]
    fn rule_without_unless_is_its_default_expression() {
        let graph = graph_of("spec s\ndata x: number\nrule r: x + 9\n");
        let node = rule_node_of(&graph, "r");
        node.expression.assert_is_tree();
        assert_eq!(node.branches, vec![(None, node.expression.root)]);
        assert!(matches!(
            node.expression.root_kind(),
            ExpressionKind::Arithmetic { .. }
        ));
    }

    #[test]
    fn unless_rule_is_one_piecewise_table_in_source_order() {
        let graph = graph_of(
            "spec s\ndata x: number\nrule r: 0\n unless x > 10 then 100\n unless x > 5 then 50\n",
        );
        let node = rule_node_of(&graph, "r");
        node.expression.assert_is_tree();
        let ExpressionKind::Piecewise(arms) = node.expression.root_kind() else {
            panic!(
                "expected Piecewise root, got {:?}",
                node.expression.root_kind()
            );
        };
        let number = |id: ExprId| match node.expression.at(id).kind() {
            ExpressionKind::Literal(literal) => match &literal.value {
                ValueKind::Number(n) => n.clone(),
                other => panic!("expected number, got {other:?}"),
            },
            other => panic!("expected literal, got {other:?}"),
        };
        assert_eq!(arms.len(), 3);
        assert!(matches!(
            node.expression.at(arms[0].0).kind(),
            ExpressionKind::Literal(literal) if matches!(literal.value, ValueKind::Boolean(true))
        ));
        assert_eq!(number(arms[0].1), number(node.branches[0].1));
        for (index, expected) in [(1, 100), (2, 50)] {
            assert!(matches!(
                node.expression.at(arms[index].0).kind(),
                ExpressionKind::Comparison(..)
            ));
            assert_eq!(
                (Some(arms[index].0), arms[index].1),
                node.branches[index],
                "branch ids name the Piecewise arm"
            );
            assert_eq!(
                number(arms[index].1),
                crate::computation::rational::rational_new(expected, 1)
            );
        }
        assert_eq!(node.branches[0].0, None);
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

        let age_expr = ast::Expression::leaf(
            ast::ExpressionKind::Reference(Reference {
                segments: Vec::new(),
                name: "age".to_string(),
            }),
            Some(test_source()),
        );

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
            rule_node.expression.at(rule_node.branches[0].1).kind(),
            ExpressionKind::DataPath(_)
        ));
    }

    #[test]
    fn test_rule_reference_conversion() {
        let mut spec = create_test_spec("test");

        let rule1_expr = ast::Expression::leaf(
            ast::ExpressionKind::Reference(Reference {
                segments: Vec::new(),
                name: "age".to_string(),
            }),
            Some(test_source()),
        );

        let rule1 = LemmaRule {
            name: "rule1".to_string(),
            expression: rule1_expr,
            unless_clauses: Vec::new(),
            source_location: test_source(),
        };
        spec = spec.add_rule(rule1);

        let rule2_expr = ast::Expression::leaf(
            ast::ExpressionKind::Reference(Reference {
                segments: Vec::new(),
                name: "rule1".to_string(),
            }),
            Some(test_source()),
        );

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
            rule2_node.expression.at(rule2_node.branches[0].1).kind(),
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

        let missing_data_expr = ast::Expression::leaf(
            ast::ExpressionKind::Reference(Reference {
                segments: Vec::new(),
                name: "nonexistent".to_string(),
            }),
            Some(test_source()),
        );

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
                uses: "m".to_string(),
                repository: None,
                spec: "myspec".to_string(),
            }],
            data: "x".to_string(),
        };
        assert!(
            !graph.data.contains_key(&data_path),
            "scalar nested data stays on the template; consumer holds the Import marker only"
        );
        assert!(
            matches!(graph.lookup_data(&data_path), Some(DataLookup::Import(_))),
            "Ref should peel to myspec.x"
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
            Some(DataDefinition::Import { identity, .. }) => {
                assert_eq!(identity.spec, "inner");
            }
            other => panic!(
                "alias path 'i' must be DataDefinition::Import, got {:?}",
                other
            ),
        }

        let nested_path = DataPath {
            segments: vec![PathSegment {
                uses: "i".to_string(),
                repository: None,
                spec: "inner".to_string(),
            }],
            data: "x".to_string(),
        };
        assert!(
            !graph.data().contains_key(&nested_path),
            "scalar nested data stays on the template; consumer holds the Import marker only"
        );
        let peeled = graph
            .lookup_data(&nested_path)
            .expect("nested data i.x must peel to the inner template field");
        assert!(
            matches!(peeled, DataLookup::Import(_)),
            "i.x must resolve through the import template, got {:?}",
            peeled.definition()
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

        fn context_with_test_spec() -> (&'static Context, &'static LemmaSpec) {
            use crate::engine::Context;
            let mut ctx = Context::new();
            let repository = ctx.workspace();
            let spec = LemmaSpec::new("test_spec".to_string());
            ctx.insert_spec(Arc::clone(&repository), spec).unwrap();
            let ctx: &'static Context = Box::leak(Box::new(ctx));
            let repository = ctx.workspace();
            let spec = ctx
                .spec_set(&repository, "test_spec")
                .and_then(|ss| ss.get_exact(None))
                .expect("inserted");
            (ctx, spec)
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
            let (ctx, spec) = context_with_test_spec();
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
            let (ctx, spec) = context_with_test_spec();
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
            let (ctx, spec) = context_with_test_spec();
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
            assert!(
                Arc::ptr_eq(
                    kilometer_owner,
                    resolved_types.resolved.get("road_length").unwrap()
                ),
                "bound kilometer owner must be the resolved road_length Arc"
            );
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
            match &declared.value {
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
                resolved.unit_index.unique_owner("eur").is_some(),
                "eur should be in unit_index"
            );
            assert!(
                resolved.unit_index.unique_owner("usd").is_some(),
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
            assert!(
                Arc::ptr_eq(eur_type, resolved.resolved.get("money").unwrap()),
                "bound eur owner must be the resolved money Arc"
            );
            assert!(
                Arc::ptr_eq(usd_type, resolved.resolved.get("money2").unwrap()),
                "bound usd owner must be the resolved money2 Arc"
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
                resolved.unit_index.unique_owner("eur").is_some(),
                "eur must be in unit_index"
            );
            assert!(
                resolved.unit_index.unique_owner("usd").is_some(),
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
        fn same_named_local_root_does_not_hide_imported_units() {
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
                consumer_types.unit_index.unique_owner("kg").is_some(),
                "local kg must be indexed"
            );
            assert!(
                consumer_types.unit_index.unique_owner("tonne").is_some(),
                "local tonne must be indexed"
            );
            for imported_unit in ["kilogram", "kilograms", "gram"] {
                let owner = consumer_types
                    .unit_index
                    .unique_owner(imported_unit)
                    .unwrap_or_else(|| panic!("imported {imported_unit} must stay in scope"));
                assert_eq!(
                    owner.measure_family().map(|family| family.spec.as_str()),
                    Some("std_units"),
                    "{imported_unit} must belong to std_units.mass, not the local mass"
                );
            }

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

            assert_eq!(
                consumer_types.unit_index.owners_for("hour").len(),
                2,
                "each import path keeps its hour owner"
            );
            consumer_types
                .unit_index
                .resolve("hour")
                .expect("bare hour collapses across identical import paths");
        }

        #[test]
        fn different_version_families_keep_separate_hour_owners() {
            // Two versions are different families; merge keeps both owners.
            // Bare `hour` is ambiguous at use (see type_identity_versions).
            let code = r#"spec units 2024-01-01
    data duration: measure
      -> unit hour: 1

    spec units 2025-01-01
    data duration: measure
      -> unit hour: 60

    spec consumer
    uses a: units 2024-01-01
    uses b: units 2025-01-01
    rule smoke: true
    "#;
            let (resolver, spec_arcs) = resolver_for_code(code);
            let consumer_arc = spec_arcs
                .iter()
                .find(|spec| spec.name == "consumer")
                .cloned()
                .expect("consumer spec");
            let (context, _) = test_context_and_effective(&spec_arcs);
            let repository = context.workspace();

            let mut already_resolved = ResolvedTypesMap::new();
            for units_arc in spec_arcs.iter().filter(|spec| spec.name == "units") {
                let units_types = resolver
                    .resolve_and_validate(units_arc, &units_arc.effective_from, &already_resolved)
                    .expect("units version resolves");
                already_resolved.push((Arc::clone(&repository), units_arc, units_types));
            }

            let consumer_types = resolver
                .resolve_and_validate(consumer_arc, &EffectiveDate::Origin, &already_resolved)
                .expect("different version families resolve without merge conflict");
            assert_eq!(
                consumer_types.unit_index.owners_for("hour").len(),
                2,
                "each version keeps its own hour owner"
            );
        }

        #[test]
        fn test_spec_level_same_unit_names_keep_every_declarer() {
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

            let resolved = resolver
                .resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new())
                .expect("independent declarers of one unit name must load");
            assert_eq!(
                resolved.unit_index.owners_for("eur").len(),
                2,
                "every declarer of 'eur' stays in scope"
            );
            assert!(
                resolved.unit_index.resolve("eur").is_err(),
                "bare 'eur' with two declarers must be qualified"
            );
            assert_eq!(
                resolved.unit_index.owners_for("usd").len(),
                2,
                "every declarer of 'usd' stays in scope"
            );
            assert!(
                resolved.unit_index.resolve("usd").is_err(),
                "bare 'usd' with two declarers must be qualified"
            );
            assert_eq!(
                resolved.unit_index.owners_for("meter").len(),
                2,
                "every declarer of 'meter' stays in scope"
            );
            assert!(
                resolved.unit_index.resolve("meter").is_err(),
                "bare 'meter' with two declarers must be qualified"
            );
            let (_, owner) = resolved
                .unit_index
                .resolve("money_b.usd")
                .expect("qualified unit must resolve");
            assert_eq!(owner.name(), "money_b");
        }

        #[test]
        fn test_measure_and_ratio_sharing_a_unit_name_keep_both_declarers() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data q: measure
      -> unit foo: 1

    data r: ratio
      -> unit foo: 100"#,
            );

            let resolved = resolver
                .resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new())
                .expect("independent declarers of one unit name must load");
            assert_eq!(
                resolved.unit_index.owners_for("foo").len(),
                2,
                "every declarer of 'foo' stays in scope"
            );
            assert!(
                resolved.unit_index.resolve("foo").is_err(),
                "bare 'foo' with two declarers must be qualified"
            );
            let (_, owner) = resolved
                .unit_index
                .resolve("r.foo")
                .expect("qualified unit must resolve");
            assert_eq!(owner.name(), "r");
        }

        #[test]
        fn test_same_ratio_unit_across_types_keeps_both_declarers() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data spread_a: ratio
      -> unit basis_points: 10000

    data spread_b: ratio
      -> unit basis_points: 10000"#,
            );

            let resolved = resolver
                .resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new())
                .expect("independent declarers of one unit name must load");
            assert_eq!(
                resolved.unit_index.owners_for("basis_points").len(),
                2,
                "every declarer of 'basis_points' stays in scope"
            );
            assert!(
                resolved.unit_index.resolve("basis_points").is_err(),
                "bare 'basis_points' with two declarers must be qualified"
            );
            let (_, owner) = resolved
                .unit_index
                .resolve("spread_a.basis_points")
                .expect("qualified unit must resolve");
            assert_eq!(owner.name(), "spread_a");
        }

        #[test]
        fn test_different_ratio_unit_factor_across_types_keeps_both_declarers() {
            let (resolver, spec) = resolver_single_spec(
                r#"spec test
    data spread_a: ratio
      -> unit basis_points: 10000

    data spread_b: ratio
      -> unit basis_points: 5000"#,
            );

            let resolved = resolver
                .resolve_and_validate(spec, &EffectiveDate::Origin, &Vec::new())
                .expect("independent declarers of one unit name must load");
            assert_eq!(
                resolved.unit_index.owners_for("basis_points").len(),
                2,
                "every declarer of 'basis_points' stays in scope"
            );
            assert!(
                resolved.unit_index.resolve("basis_points").is_err(),
                "bare 'basis_points' with two declarers must be qualified"
            );
            let (_, owner) = resolved
                .unit_index
                .resolve("spread_b.basis_points")
                .expect("qualified unit must resolve");
            assert_eq!(owner.name(), "spread_b");
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

            let (_, percent) = resolved
                .unit_index
                .resolve("percent")
                .expect("bare percent binds PrimitiveRatio");
            assert_eq!(percent.name(), "ratio");
            let (_, permille) = resolved
                .unit_index
                .resolve("permille")
                .expect("bare permille binds PrimitiveRatio");
            assert_eq!(permille.name(), "ratio");
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
                .apply_constraint("test", command, args, &mut suggestion, &mut None, None)
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
            let default = BoundValueKind::unbound(ValueKind::Number(rational_new(5, 1)));

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
            let default = BoundValueKind::unbound(ValueKind::Number(rational_new(150, 1)));

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
            let default = BoundValueKind::unbound(ValueKind::Number(rational_new(50, 1)));

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
                None,
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
                None,
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
            let default = BoundValueKind::unbound(ValueKind::Text("green".to_string()));

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
                unit: None,
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
                unit: None,
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
                unit: None,
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
                unit: None,
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
                    unit: None,
                    help: String::new(),
                },
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
            let mut resolved_types = ResolvedSpecTypes::default();
            resolved_types
                .resolved
                .insert("band".to_string(), primitive_measure_arc().clone());
            let already_resolved = ResolvedTypesMap::new();

            let errors = refresh_named_range_specs(
                &resolver,
                &spec,
                &data_defs,
                &mut resolved_types,
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
                    .contains("Unknown parent 'ghost' for data definition"),
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
            let mut value = None;
            apply_constraints_to_spec(
                &spec,
                "code",
                TypeSpecification::text(),
                &constraints,
                &source,
                ConstraintSuggestionSink {
                    suggestion: &mut suggestion,
                    value: &mut value,
                    resolve_unit: None,
                },
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
                    unit: None,
                    help: String::new(),
                },
                TypeExtends::custom_local(
                    "measure".to_string(),
                    crate::planning::semantics::tests::test_type_family("weight"),
                ),
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
                    unit: None,
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
    #[serde(with = "crate::snapshot::shared::arc_index_map")]
    pub resolved: IndexMap<String, Arc<LemmaType>>,

    /// Declared default per named type (e.g. `type rate: ratio -> suggest 50%`).
    /// Only present for types that declared a `-> suggest ...` constraint anywhere
    /// in their extension chain; the inner-most `-> suggest` wins. Defaults live
    /// outside [`TypeSpecification`] so the type itself stays free of binding data.
    /// Populated after [`bound_value_kind_from_raw_suggestion`] (post-decomposition).
    pub declared_suggestions: IndexMap<String, BoundValueKind>,

    /// Declared value per named type (e.g. `type rate: ratio -> with 50%`).
    /// Only present for types that declared a `-> with ...` constraint anywhere
    /// in their extension chain; the inner-most `-> with` wins.
    pub declared_values: IndexMap<String, BoundValueKind>,

    /// Defaults captured during type resolution, before measure unit factors are final.
    pub(crate) raw_suggestions: Vec<(String, RawSuggestion)>,

    /// Raw fills captured during type resolution, before measure unit factors are final.
    pub(crate) raw_values: Vec<(String, RawSuggestion)>,

    /// Raw defaults retained after [`bound_value_kind_from_raw_suggestion`] for cross-spec parent lookup during later specs.
    pub(crate) source_defaults: IndexMap<String, RawSuggestion>,

    /// Raw fills retained for cross-spec parent lookup during later specs.
    pub(crate) source_value_defaults: IndexMap<String, RawSuggestion>,

    /// List types written in this spec's rules (`money list -> with ...`), keyed by the
    /// written type, in order of first appearance.
    #[serde(with = "crate::snapshot::shared::arc_pair_vec")]
    pub(crate) rule_list_types: Vec<(ParentType, Arc<LemmaType>)>,

    /// Expression-scope units. One declarer per bare unit name; qualify with Type.unit or alias.Type.unit.
    /// Binding aliases never appear as index keys.
    pub unit_index: crate::planning::unit_index::UnitIndex,

    /// Names of unit owners until [`PlanningUnitIndex::bind`] after magnitudes.
    #[serde(skip)]
    planning_unit_index: PlanningUnitIndex,

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
    let mut current = parent;
    while let ParentType::Ranged { inner } | ParentType::List { inner } = current {
        current = inner.as_ref();
    }
    current
}

fn time_range_endpoints_share_timezone(
    left: &semantics::SemanticTime,
    right: &semantics::SemanticTime,
) -> bool {
    match (&left.timezone, &right.timezone) {
        (None, None) => true,
        (Some(left_tz), Some(right_tz)) => left_tz.offset_minutes == right_tz.offset_minutes,
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

    /// `data x: some_spec list` names a spec where a type belongs; suggest `uses`.
    fn spec_list_error(
        &self,
        spec: &LemmaSpec,
        declared_name: &str,
        parent: &ParentType,
        source: &Source,
    ) -> Option<Error> {
        let name = self.list_of_spec_name(spec, parent)?;
        Some(Error::validation_with_context(
            format!(
                "'{name}' names a spec. Declare an instance list with `uses {declared_name}: {name} list`"
            ),
            Some(source.clone()),
            None::<String>,
            Some(spec),
            None,
        ))
    }

    /// [`Self::spec_list_error`] for a list type written in a rule.
    fn rule_spec_list_error(
        &self,
        spec: &LemmaSpec,
        parent: &ParentType,
        source: &Source,
    ) -> Option<Error> {
        let name = self.list_of_spec_name(spec, parent)?;
        Some(Error::validation_with_context(
            format!(
                "'{name}' names a spec. Instance lists are declared as data with `uses`, not built in a rule"
            ),
            Some(source.clone()),
            None::<String>,
            Some(spec),
            None,
        ))
    }

    /// `name` when `parent` is `name list` and `name` is a spec in `spec`'s repository.
    fn list_of_spec_name<'p>(&self, spec: &LemmaSpec, parent: &'p ParentType) -> Option<&'p str> {
        let ParentType::List { inner } = parent else {
            return None;
        };
        let ParentType::Custom { name } = inner.as_ref() else {
            return None;
        };
        self.names_spec_in_repository_of(spec, name)
            .then_some(name.as_str())
    }

    /// Whether `name` is a spec in the repository that holds `spec`.
    fn names_spec_in_repository_of(&self, spec: &LemmaSpec, name: &str) -> bool {
        let repository = discovery::owning_repository(self.context, spec);
        self.context.spec_set(&repository, name).is_some()
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
                ParsedDataValue::Import { .. } | ParsedDataValue::ImportList { .. } => {}
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
        let (mut resolved_types, list_type_names) =
            self.resolve_types_internal(spec, at, already_resolved)?;
        resolved_types.source_defaults = resolved_types
            .raw_suggestions
            .iter()
            .map(|(name, raw)| (name.clone(), raw.clone()))
            .collect();
        resolved_types.source_value_defaults = resolved_types
            .raw_values
            .iter()
            .map(|(name, raw)| (name.clone(), raw.clone()))
            .collect();
        let mut errors = Vec::new();

        // Build the type-name → source map for precise error reporting.
        let type_sources: std::collections::HashMap<String, Source> = resolved_types
            .resolved
            .keys()
            .chain(list_type_names.iter())
            .filter_map(|type_name| {
                self.data_types
                    .iter()
                    .find(|(s, _)| discovery::same_loaded_spec(s, spec))
                    .and_then(|(_, defs)| defs.get(type_name.as_str()))
                    .map(|ftd| (type_name.clone(), ftd.source.clone()))
            })
            .collect();

        // Run the decomposition pass to populate `BaseMeasureVector` on all Measure types.
        let this_spec = SpecIdentity::of(&discovery::owning_repository(self.context, spec), spec);
        let (new_resolved, decomp_errors) = resolve_measure_decompositions(
            &spec.name,
            &this_spec,
            std::mem::take(&mut resolved_types.resolved),
            &resolved_types.planning_unit_index,
            &type_sources,
        );
        resolved_types.resolved = new_resolved;
        errors.extend(decomp_errors);

        for (type_name, raw) in std::mem::take(&mut resolved_types.raw_suggestions) {
            let lemma_type = resolved_types
                .resolved
                .get(&type_name)
                .expect("BUG: raw default for type not in resolved");
            match bound_value_kind_from_raw_suggestion(raw, &lemma_type.specifications, &type_name)
            {
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

        for (type_name, raw) in std::mem::take(&mut resolved_types.raw_values) {
            let lemma_type = resolved_types
                .resolved
                .get(&type_name)
                .expect("BUG: raw fill for type not in resolved");
            match bound_value_kind_from_raw_suggestion(raw, &lemma_type.specifications, &type_name)
            {
                Ok(value_kind) => {
                    resolved_types.declared_values.insert(type_name, value_kind);
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
                &mut resolved_types,
                already_resolved,
                at,
            ));
            errors.extend(apply_deferred_named_range_constraints(
                spec,
                data_defs,
                &mut resolved_types.resolved,
                &mut resolved_types.declared_suggestions,
                &mut resolved_types.declared_values,
                &type_sources,
                &resolved_types.planning_unit_index,
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

        resolved_types.unit_index = std::mem::take(&mut resolved_types.planning_unit_index)
            .bind(&resolved_types.resolved, &this_spec);

        match build_signature_index(&spec.name, &resolved_types.unit_index) {
            Ok(index) => resolved_types.signature_index = index,
            Err(error) => errors.push(error),
        }

        if !resolved_errors.is_empty() {
            errors.extend(resolved_errors);
            return Err(errors);
        }

        if let Err(list_errors) = self.resolve_list_types(
            spec,
            at,
            already_resolved,
            &list_type_names,
            &mut resolved_types,
        ) {
            errors.extend(list_errors);
            return Err(errors);
        }

        if let Err(rule_list_errors) =
            self.resolve_rule_list_types(spec, at, already_resolved, &mut resolved_types)
        {
            errors.extend(rule_list_errors);
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
                resolved_types.declared_values.get(type_name),
                "with",
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
                resolved_types.declared_values.get(type_name),
                "with",
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
                    value: default_kind.value.clone(),
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
            if let Some(default_kind) = resolved_types.declared_values.get(type_name.as_str()) {
                let lit = TypedLiteral {
                    value: default_kind.value.clone(),
                    lemma_type: Arc::clone(lemma_type),
                };
                if let Err(message) = crate::planning::execution_plan::validate_value_against_type(
                    lemma_type.as_ref(),
                    &lit.to_literal(),
                    &resolved_types.unit_index,
                ) {
                    errors.push(Error::validation_with_context(
                        format!("Type '{}' with is invalid: {}", type_name, message),
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

    /// Resolve list types once every scalar type is final, and convert their declared rows.
    fn resolve_list_types(
        &self,
        spec: &LemmaSpec,
        at: &EffectiveDate,
        already_resolved: &ResolvedTypesMap,
        list_type_names: &[String],
        resolved_types: &mut ResolvedSpecTypes,
    ) -> Result<(), Vec<Error>> {
        if list_type_names.is_empty() {
            return Ok(());
        }
        let data_defs = self
            .data_types
            .iter()
            .find(|(s, _)| discovery::same_loaded_spec(s, spec))
            .map(|(_, defs)| defs)
            .expect("BUG: list type names come from this spec's type definitions");
        let mut local = local_type_entries(resolved_types);
        let mut errors = Vec::new();
        for type_name in list_type_names {
            let ftd = data_defs
                .get(type_name.as_str())
                .expect("BUG: topo-sorted list type missing from registry");
            let (lemma_type, suggestion, value) =
                self.resolve_type_def(spec, type_name, ftd, at, already_resolved, &local)?;
            let to_error = |message: String| {
                Error::validation_with_context(
                    message,
                    Some(ftd.source.clone()),
                    None::<String>,
                    Some(spec),
                    None,
                )
            };
            if let Some(raw) = &suggestion {
                resolved_types
                    .source_defaults
                    .insert(type_name.clone(), raw.clone());
                match bound_value_kind_from_raw_suggestion(
                    raw.clone(),
                    &lemma_type.specifications,
                    type_name,
                ) {
                    Ok(bound) => {
                        resolved_types
                            .declared_suggestions
                            .insert(type_name.clone(), bound);
                    }
                    Err(message) => errors.push(to_error(message)),
                }
            }
            if let Some(raw) = &value {
                resolved_types
                    .source_value_defaults
                    .insert(type_name.clone(), raw.clone());
                match bound_value_kind_from_raw_suggestion(
                    raw.clone(),
                    &lemma_type.specifications,
                    type_name,
                ) {
                    Ok(bound) => {
                        resolved_types
                            .declared_values
                            .insert(type_name.clone(), bound);
                    }
                    Err(message) => errors.push(to_error(message)),
                }
            }
            resolved_types
                .resolved
                .insert(type_name.clone(), Arc::clone(&lemma_type));
            local.insert(type_name.clone(), (lemma_type, suggestion, value));
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Resolve the list types written in `spec`'s rules against the same scope as its data
    /// types, once every local type is final.
    fn resolve_rule_list_types<'s>(
        &self,
        spec: &'s LemmaSpec,
        at: &EffectiveDate,
        already_resolved: &ResolvedTypesMap,
        resolved_types: &mut ResolvedSpecTypes,
    ) -> Result<(), Vec<Error>> {
        let mut written: Vec<(&'s ParentType, &'s Source)> = Vec::new();
        for rule in &spec.rules {
            let trees = std::iter::once(&rule.expression).chain(
                rule.unless_clauses
                    .iter()
                    .flat_map(|clause| [&clause.condition, &clause.result]),
            );
            for tree in trees {
                let mut pending = vec![tree.root()];
                while let Some(node) = pending.pop() {
                    if let ast::ExpressionKind::ListType { element } = node.kind() {
                        if written.iter().all(|(seen, _)| *seen != element) {
                            let source = node
                                .source()
                                .expect("BUG: AST expression missing source location");
                            written.push((element, source));
                        }
                    }
                    node.push_children(&mut pending);
                }
            }
        }
        if written.is_empty() {
            return Ok(());
        }
        let local = local_type_entries(resolved_types);
        let mut errors = Vec::new();
        for (element, source) in written {
            let scope = ParentLookup {
                resolver: self,
                spec,
                source,
                at,
                already_resolved,
                resolved: &local,
                site: TypeUseSite::RuleListType,
            };
            match lookup_parent_type(&scope, element) {
                Ok((.., lemma_type)) => {
                    resolved_types
                        .rule_list_types
                        .push((element.clone(), lemma_type));
                }
                Err(lookup_errors) => match self.rule_spec_list_error(spec, element, source) {
                    Some(error) => errors.push(error),
                    None => errors.extend(lookup_errors),
                },
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Resolve one data type declaration against `resolved` (already resolved local types).
    fn resolve_type_def(
        &self,
        spec: &LemmaSpec,
        type_name: &str,
        ftd: &DataTypeDef,
        at: &EffectiveDate,
        already_resolved: &ResolvedTypesMap,
        resolved: &HashMap<String, ResolvedTypeEntry>,
    ) -> Result<ResolvedTypeEntry, Vec<Error>> {
        let parent = &ftd.parent;
        let constraints = &ftd.constraints;

        let scope = ParentLookup {
            resolver: self,
            spec,
            source: &ftd.source,
            at,
            already_resolved,
            resolved,
            site: TypeUseSite::DataDefinition,
        };
        let (parent_specs, parent_suggestion, parent_value, parent_type) =
            lookup_parent_type(&scope, parent).map_err(|errors| {
                self.spec_list_error(spec, type_name, parent, &ftd.source)
                    .map_or(errors, |error| vec![error])
            })?;

        let mut declared_suggestion = parent_suggestion;
        let mut declared_value = parent_value;
        let final_specs = if should_defer_ranged_constraints(parent) {
            parent_specs
        } else if let Some(constraints) = &constraints {
            let resolve_unit = UnitTokenResolver::Search(
                already_resolved
                    .iter()
                    .map(|(_, _, rts)| (&rts.unit_index, &rts.resolved))
                    .collect(),
            );
            apply_constraints_to_spec(
                spec,
                &constraint_application_type_name(parent, type_name),
                parent_specs,
                constraints,
                &ftd.source,
                ConstraintSuggestionSink {
                    suggestion: &mut declared_suggestion,
                    value: &mut declared_value,
                    resolve_unit: Some(&resolve_unit),
                },
            )?
        } else {
            parent_specs
        };

        if matches!(&final_specs, TypeSpecification::List { .. }) && declared_value.is_none() {
            declared_value = Some(RawSuggestion::List(Vec::new()));
        }

        let this_spec = SpecIdentity::of(&discovery::owning_repository(self.context, spec), spec);
        let family = match (element_parent_type(parent), parent_type.family()) {
            (ParentType::Primitive { .. }, _) | (_, None) => {
                TypeFamily::declared_in(&this_spec, type_name)
            }
            (_, Some(parent_family)) => parent_family.clone(),
        };
        let defining_spec = match element_parent_type(parent) {
            ParentType::Qualified { .. } => TypeDefiningSpec::Import,
            _ => TypeDefiningSpec::Local,
        };
        let extends = TypeExtends::Custom {
            parent: parent.to_string(),
            family,
            defining_spec,
        };

        let declared_suggestion = match &ftd.bound_literal {
            Some(literal) => {
                let resolve_unit = UnitTokenResolver::Search(
                    already_resolved
                        .iter()
                        .map(|(_, _, rts)| (&rts.unit_index, &rts.resolved))
                        .collect(),
                );
                match semantics::parser_value_to_value_kind(
                    literal,
                    &final_specs,
                    Some(&resolve_unit),
                ) {
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
                }
            }
            None => declared_suggestion,
        };

        let lemma_type = Arc::new(LemmaType {
            name: Some(type_name.to_string()),
            specifications: final_specs,
            extends,
        });
        Ok((lemma_type, declared_suggestion, declared_value))
    }

    /// Resolve the spec's scalar types. Returns them with the topo-sorted list type names,
    /// which [`Self::resolve_list_types`] resolves once the scalar passes are done.
    fn resolve_types_internal(
        &self,
        spec: &LemmaSpec,
        at: &EffectiveDate,
        already_resolved: &ResolvedTypesMap,
    ) -> Result<(ResolvedSpecTypes, Vec<String>), Vec<Error>> {
        let data_defs = self
            .data_types
            .iter()
            .find(|(s, _)| discovery::same_loaded_spec(s, spec))
            .map(|(_, defs)| defs)
            .cloned()
            .unwrap_or_default();

        let (scalar_names, list_names) = partition_list_type_names(&data_defs);
        let sorted_type_names = topo_sort_type_names(spec, &data_defs, scalar_names)?;
        let list_type_names = topo_sort_type_names(spec, &data_defs, list_names)?;

        let mut resolved: HashMap<String, ResolvedTypeEntry> = HashMap::new();

        for type_name in &sorted_type_names {
            let ftd = data_defs
                .get(type_name.as_str())
                .expect("BUG: topo-sorted type missing from registry");
            let entry =
                self.resolve_type_def(spec, type_name, ftd, at, already_resolved, &resolved)?;
            resolved.insert(type_name.clone(), entry);
        }

        let mut unit_index = PlanningUnitIndex::new();
        let mut errors = Vec::new();
        for unit in Self::extract_units_from_type(&semantics::primitive_ratio_arc().specifications)
        {
            unit_index.insert_owner(unit, PlanningOwner::PrimitiveRatio);
        }

        let locals: HashMap<String, Arc<LemmaType>> = resolved
            .iter()
            .map(|(name, (type_arc, _, _))| (name.clone(), Arc::clone(type_arc)))
            .collect();

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
                            &[],
                            &parent_units,
                            &locals,
                        ),
                        Err(error) => Err(error),
                    }
                } else {
                    Self::add_measure_units_to_index(
                        spec,
                        &mut unit_index,
                        type_arc,
                        data_type_def,
                        &[],
                        &locals,
                    )
                }
            } else if type_arc.is_ratio() {
                Self::add_ratio_units_to_index(
                    spec,
                    &mut unit_index,
                    type_arc,
                    data_type_def,
                    &[],
                    &locals,
                )
            } else {
                Ok(())
            };
            if let Err(error) = merge_result {
                errors.push(error);
            }
        }

        for data_row in &spec.data {
            let Some(spec_ref) = data_row.value.uses_spec_ref() else {
                continue;
            };
            let import_alias = data_row.reference.name.clone();
            let import_target = match &data_row.value {
                ParsedDataValue::ImportList { .. } => {
                    self.resolve_list_spec_for_import(spec, spec_ref, &data_row.source_location, at)
                }
                _ => self.resolve_spec_for_import(spec, spec_ref, &data_row.source_location, at),
            };
            let (_, imported_spec) = match import_target {
                Ok(import_pair) => import_pair,
                Err(error) => {
                    errors.push(error);
                    continue;
                }
            };
            if discovery::same_loaded_spec(imported_spec, spec) {
                continue;
            }
            let Some(imported_resolved) = already_resolved
                .iter()
                .find(|(_, imported, _)| discovery::same_loaded_spec(imported, imported_spec))
                .map(|(_, _, resolved_spec_types)| resolved_spec_types)
            else {
                continue;
            };
            for (bare, owner) in imported_resolved.unit_index.iter_owners() {
                let planning_owner = match owner {
                    UnitOwner::PrimitiveRatio => PlanningOwner::PrimitiveRatio,
                    UnitOwner::Named {
                        import_path,
                        type_name,
                        lemma_type,
                    } => {
                        let mut path = vec![import_alias.clone()];
                        path.extend(import_path.iter().cloned());
                        PlanningOwner::Imported {
                            import_path: path,
                            type_name: type_name.clone(),
                            lemma_type: Arc::clone(lemma_type),
                        }
                    }
                };
                unit_index.insert_owner(bare.to_string(), planning_owner);
            }
        }

        if !errors.is_empty() {
            return Err(errors);
        }

        let mut raw_suggestions = Vec::new();
        let mut raw_values = Vec::new();
        let mut resolved_types = IndexMap::new();
        for type_name in &sorted_type_names {
            let (lemma_type, default, fill) = resolved
                .remove(type_name)
                .expect("BUG: every sorted type name must exist in resolved");
            if let Some(raw_suggestion) = default {
                raw_suggestions.push((type_name.clone(), raw_suggestion));
            }
            if let Some(raw_value) = fill {
                raw_values.push((type_name.clone(), raw_value));
            }
            resolved_types.insert(type_name.clone(), lemma_type);
        }
        if !resolved.is_empty() {
            panic!(
                "BUG: resolved types remain after draining sorted_type_names: {:?}",
                resolved.keys().collect::<Vec<_>>()
            );
        }

        Ok((
            ResolvedSpecTypes {
                resolved: resolved_types,
                declared_suggestions: IndexMap::new(),
                declared_values: IndexMap::new(),
                raw_suggestions,
                raw_values,
                source_defaults: IndexMap::new(),
                source_value_defaults: IndexMap::new(),
                rule_list_types: Vec::new(),
                unit_index: UnitIndex::new(),
                planning_unit_index: unit_index,
                signature_index: crate::computation::arithmetic::SignatureIndex::new(),
            },
            list_type_names,
        ))
    }

    fn resolve_spec_for_import(
        &self,
        spec: &LemmaSpec,
        from: &crate::parsing::ast::SpecRef,
        import_site: &crate::parsing::source::Source,
        at: &EffectiveDate,
    ) -> Result<(Arc<LemmaRepository>, &'a LemmaSpec), Error> {
        discovery::resolve_spec_ref(
            self.context,
            from,
            &discovery::owning_repository(self.context, spec),
            spec,
            at,
            Some(import_site.clone()),
        )
    }

    /// [`Self::resolve_spec_for_import`] for `uses x: s list`, where `s` may be `spec` itself.
    fn resolve_list_spec_for_import(
        &self,
        spec: &LemmaSpec,
        from: &crate::parsing::ast::SpecRef,
        import_site: &crate::parsing::source::Source,
        at: &EffectiveDate,
    ) -> Result<(Arc<LemmaRepository>, &'a LemmaSpec), Error> {
        discovery::resolve_spec_ref_or_self(
            self.context,
            from,
            &discovery::owning_repository(self.context, spec),
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
        unit_index: &mut PlanningUnitIndex,
        resolved_type: &Arc<LemmaType>,
        defined_by: &DataTypeDef,
        import_path: &[String],
        locals: &HashMap<String, Arc<LemmaType>>,
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
                    import_path,
                    measure_family,
                    locals,
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
        let Some(spec_ref) = spec.data.iter().find_map(|row| {
            if row.reference.segments.is_empty() && row.reference.name == spec_alias {
                row.value.uses_spec_ref()
            } else {
                None
            }
        }) else {
            return Err(Error::validation_with_context(
                format!("'{spec_alias}' is not a `uses` import alias in this spec"),
                Some(defined_by.source.clone()),
                Some(format!(
                    "Add `uses {spec_alias}: <target_spec>`, then write `{spec_alias}.<type>`."
                )),
                Some(spec),
                None,
            ));
        };
        let (_, target_spec) =
            resolver.resolve_spec_for_import(spec, spec_ref, &defined_by.source, at)?;
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
        unit_index: &mut PlanningUnitIndex,
        resolved_type: &Arc<LemmaType>,
        defined_by: &DataTypeDef,
        import_path: &[String],
        parent_units: &BTreeSet<String>,
        locals: &HashMap<String, Arc<LemmaType>>,
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
                    import_path,
                    measure_family,
                    locals,
                )
                .map_err(|conflict| {
                    Self::unit_merge_conflict_to_error(conflict, defined_by, spec)
                })?;
        }
        Ok(())
    }

    fn add_ratio_units_to_index(
        spec: &LemmaSpec,
        unit_index: &mut PlanningUnitIndex,
        resolved_type: &Arc<LemmaType>,
        defined_by: &DataTypeDef,
        import_path: &[String],
        locals: &HashMap<String, Arc<LemmaType>>,
    ) -> Result<(), Error> {
        for unit in Self::extract_units_from_type(&resolved_type.specifications) {
            unit_index
                .merge_ratio_unit(unit, resolved_type, &defined_by.name, import_path, locals)
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
            UnitMergeConflict::ConflictingFactors { unit, family } => format!(
                "Unit '{unit}' has different values on related measure types in '{family}'. \
                 Use the same unit value on each type, or use different unit names."
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
/// or `"with"` for error messages.
///
/// Returns a vector of errors (empty if valid).
pub fn validate_type_specifications(
    specs: &TypeSpecification,
    declared_default: Option<&BoundValueKind>,
    default_label: &str,
    type_name: &str,
    source: &Source,
    spec_context: Option<&LemmaSpec>,
    unit_index: &UnitIndex,
) -> Vec<Error> {
    let mut errors = Vec::new();
    let declared_default = declared_default.map(|bound| &bound.value);

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
                        &min.0, &min.1, units, type_name, "minimum",
                    ),
                    semantics::measure_declared_bound_to_canonical(
                        &max.0, &max.1, units, type_name, "maximum",
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
            minimum, maximum, ..
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
            minimum, maximum, ..
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
        TypeSpecification::Spec { .. } | TypeSpecification::List { .. } => {}
    }

    errors
}

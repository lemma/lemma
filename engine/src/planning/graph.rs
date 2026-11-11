use crate::computation::rational::{rational_is_zero, NumericFailure, RationalInteger};
use crate::computation::OperationResult;
use crate::engine::Context;
use crate::parsing::arena::{ExprId, ExprKind, ExprRef, ExprTable, ExprTree};
use crate::parsing::ast::{
    self as ast, Constraint, ConversionTypeKind, EffectiveDate, LemmaData, LemmaRepository,
    LemmaRule, LemmaSpec, ParentType, PrimitiveKind, Value, WithRhs,
};
use crate::parsing::source::Source;
use crate::planning::discovery;
use crate::planning::execution_plan::{self, ExecutionPlan};
use crate::planning::instance_list::{
    self, InstanceListSource, InstanceLists, KeyIndex, RowHop, RowStore, RowTemplate, StoreKeyField,
};
use crate::planning::normalize::{LeafKind, NormalFormId, NormalFormKind, NormalFormListOp};
use crate::planning::semantics::{
    self, bound_value_kind_from_raw_suggestion, combine_decompositions,
    conversion_target_to_semantic, lift_range_endpoint, number_with_unit_to_value_kind,
    parser_value_to_value_kind, primitive_boolean_arc, primitive_date_arc, primitive_number_arc,
    primitive_ratio_arc, primitive_text_arc, primitive_time_arc,
    range_type_specification_from_endpoints, scale_decomposition, typed_literal_from_parsed,
    value_to_semantic, ArithmeticComputation, BaseMeasureVector, BoundValueKind,
    ComparisonComputation, DataDefinition, DataOrigin, DataPath, Expression, ExpressionKind,
    ImportEffectivePin, KeyField, LemmaType, ListKey, ListOp, ListPredKind, ListReduce,
    LiteralValue, PathSegment, RawSuggestion, ReferenceEnd, ReferenceTarget, RowBinding, RowEnd,
    RowInstance, RowOrigin, RulePath, SemanticConversionTarget, SpecIdentity, SpecInstance,
    SpecVersionKey, TypeExtends, TypeOptions, TypeSpecification, TypedLiteral, UnitTokenResolver,
    ValueKind, SPEC_LIST_HAS_NO_OPTIONS,
};
use crate::planning::spec_value::{RowFieldRead, RulePresence, SpecCandidates};
pub use crate::planning::types::ResolvedSpecTypes;
use crate::planning::types::{
    apply_constraints_to_spec, deferred_signature_errors, instance_list_option_errors,
    time_range_endpoints_share_timezone, ConstraintSuggestionSink, ResolvedTypesMap, TypeResolver,
};
use crate::planning::typing::{
    comparison_type, date_predicate_type, list_command_result_type, list_has_instance_rows,
    logical_connective_type, logical_not_type, math_op_type, past_future_range_type,
    piecewise_type, range_bound_result_type, range_containment_type, range_span_type,
    refine_list_key_type, resolve_anonymous_measure, result_is_veto_type, step_rounding_type,
    unit_conversion_type, ListCommandResultKind, MeasureScope,
};
use crate::planning::unit_index::UnitIndex;
use crate::Error;

use crate::computation::units::owning_type_declares_unit_name;
use crate::limits::{ResourceLimits, MAX_UNIT_EXPONENT};
pub use crate::planning::typing::DecompositionMatch;
pub(crate) use crate::planning::typing::{
    compute_arithmetic_result_type, find_unique_measure_type_in_unit_index,
    infer_range_type_from_endpoint_types, quantity_integer_power_type,
};
use crate::string_distance::closest_name;
use ast::DataValue as ParsedDataValue;
use indexmap::IndexMap;
use rust_decimal::Decimal;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fmt;
use std::sync::Arc;

/// One `uses` import after its dependency body (or bodies) have been planned.
enum PlannedImport {
    Static {
        identity: Arc<SpecIdentity>,
        template: Arc<ExecutionPlan>,
    },
    Dynamic {
        pin: ImportEffectivePin,
        candidates: Vec<(Arc<SpecIdentity>, Arc<ExecutionPlan>)>,
    },
}

impl PlannedImport {
    fn primary_identity(&self) -> &Arc<SpecIdentity> {
        match self {
            PlannedImport::Static { identity, .. } => identity,
            PlannedImport::Dynamic { candidates, .. } => {
                &candidates
                    .first()
                    .expect("BUG: dynamic import candidates are non-empty")
                    .0
            }
        }
    }

    fn is_dynamic(&self) -> bool {
        matches!(self, PlannedImport::Dynamic { .. })
    }
}

/// Memo of inferred expression types per expression table (one table per rule).
/// The memo does not include `computed_rule_types` or `resolved_types`, so an entry is valid
/// only while those are unchanged. Every change to either must be followed by `clear()`:
/// after phase 4 merges rule-target constraints into `resolved_types`, and when
/// `ensure_seed` replaces a recursive rule's undetermined placeholder with its seed.
#[derive(Default)]
struct InferExpressionTypeCache {
    /// Types of a table's nodes in table order, keyed by the table's address. A table lists
    /// children before parents, so a typed prefix always covers the operands of its next node.
    tables: HashMap<usize, Vec<Arc<LemmaType>>>,
    /// Keys of the tables typed since the last [`Self::clear`].
    typed: Vec<usize>,
}

impl InferExpressionTypeCache {
    fn new() -> Self {
        Self::default()
    }

    /// Forgets every type and keeps each table's storage for the next wave.
    fn clear(&mut self) {
        for key in self.typed.drain(..) {
            self.tables
                .get_mut(&key)
                .expect("BUG: a typed table is in the cache")
                .clear();
        }
    }

    /// Type of `expression` after [`infer_expression_type`] has filled this table.
    fn get(&self, expression: ExprRef<'_, ExpressionKind>) -> Arc<LemmaType> {
        let key = expression.table as *const ExprTable<ExpressionKind> as usize;
        Arc::clone(
            self.tables
                .get(&key)
                .expect("BUG: expression table typed before check")
                .get(expression.id.index())
                .expect("BUG: expression node typed before check"),
        )
    }
}

/// Data bindings map: maps a target data name path to the binding's value and source.
///
/// The key is the path of **data names** from the spec being built to the target data,
/// relative to that spec: `-> with salary: 7500` on `uses employee` in the spec being
/// built is `["employee", "salary"]`; when the import is built, the same binding is
/// passed down as `["salary"]`. Spec names are not part of the key.
type DataBindings = IndexMap<Vec<String>, (BindingValue, Source)>;

/// Binding value stored in [`DataBindings`]: a scalar literal/reference or the rows
/// that replace an instance-list slot.
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
    Reference { target: ReferenceTarget },
    /// `-> with items: ada, kim` on a `uses`: the named `uses` instances of the spec that
    /// wrote the binding, in order. Resolved to [`Self::InstanceRows`] by that spec.
    InstanceAliases(Vec<ast::Reference>),
    /// Rows that replace an instance-list slot of an imported spec: one copy per alias.
    InstanceRows(Vec<RowCopy>),
}

/// A row written in a data list: the bindings its local instance is planned with.
///
/// `-> with ada` copies every binding of `uses ada`; `-> with name: "x"` binds the
/// written fields. Keys are relative to the row spec.
#[derive(Debug, Clone)]
pub(crate) struct RowCopy {
    /// Spec of the copied instance (the alias's spec, or the list's row spec for a field row).
    identity: SpecIdentity,
    /// `ada` for `-> with ada`; `row 2` for a field row.
    name: String,
    bindings: DataBindings,
    source: Source,
}

/// Identity of a row reached from the plan being built, for visited sets.
type RowKey = *const RowInstance;

/// A row reached from the plan being built: `context` holds the rows whose contexts
/// enclose it, outermost first. The last is the row owning it; empty when this plan
/// owns it.
#[derive(Clone)]
struct Located<'s> {
    row: &'s RowInstance,
    context: Vec<&'s RowInstance>,
}

impl<'s> Located<'s> {
    fn root(row: &'s RowInstance) -> Self {
        Self {
            row,
            context: Vec::new(),
        }
    }

    /// Context of the rows this row owns.
    fn inside(&self) -> Vec<&'s RowInstance> {
        let mut context = self.context.clone();
        context.push(self.row);
        context
    }

    /// Context of the rows owned by the scope a binding written `hops` instances
    /// above this row names ([`InstanceKey::scope`]).
    fn context_at(&self, hops: u32) -> Vec<&'s RowInstance> {
        let hops = usize::try_from(hops).expect("BUG: hops fit usize");
        let len = self.context.len();
        assert!(
            hops <= len,
            "BUG: a binding hops above the plan being built"
        );
        self.context[..len - hops].to_vec()
    }

    /// The row a binding written `hops` instances above this row resolves in; `None`
    /// for the plan being built.
    fn scope(&self, hops: u32) -> Option<Located<'s>> {
        let mut context = self.context_at(hops);
        let row = context.pop()?;
        Some(Located { row, context })
    }

    fn key(&self) -> Vec<RowKey> {
        self.context
            .iter()
            .chain(std::iter::once(&self.row))
            .map(|row| *row as RowKey)
            .collect()
    }

    /// Hops naming this row from the plan being built, when it and every row enclosing
    /// it is a `uses` import.
    fn import_segments(&self) -> Option<Vec<PathSegment>> {
        self.context
            .iter()
            .chain(std::iter::once(&self.row))
            .map(|row| match &row.origin {
                RowOrigin::Import { alias } => Some(PathSegment::import(alias, &row.identity)),
                RowOrigin::Numbered { .. }
                | RowOrigin::Written { .. }
                | RowOrigin::Inline { .. } => None,
            })
            .collect()
    }
}

/// A read of `end` landing on row `at`.
struct ForcedRead<'s> {
    at: Located<'s>,
    end: RowEnd,
}

/// How [`Graph::template_rule_reads`] treats rule references.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RuleWalk {
    /// Walk the referenced rule's reads in the same context.
    Follow,
    /// Read the referenced rule, and read data slots too.
    Show,
}

/// Where a reference target lands: on this plan's own `end`, or as a read on a row.
enum Landing<'s> {
    Plan(RowEnd),
    Row(ForcedRead<'s>),
}

/// Rules of `uses` imports that Show reaches from the rules of the plan being built,
/// with every Show edge, keyed by paths composed from the import hops.
#[derive(Debug, Default)]
pub(crate) struct ShowImports {
    /// Edges into imports of each rule of this plan, and every edge of each reached
    /// import rule.
    pub(crate) edges: BTreeMap<RulePath, BTreeSet<RulePath>>,
    /// The edges of each reached import rule its normalized body reads: the rules whose
    /// data needs it shares.
    pub(crate) need_edges: BTreeMap<RulePath, BTreeSet<RulePath>>,
    /// Data each Show rule reads that no binding fills: data of this plan, or data of
    /// an import.
    pub(crate) data_reads: BTreeMap<RulePath, BTreeSet<DataPath>>,
    /// Template identity of every import data path in [`Self::data_reads`].
    pub(crate) data: BTreeMap<DataPath, SpecIdentity>,
    /// Every reached import rule: its template identity and its rule there.
    pub(crate) rules: BTreeMap<RulePath, (SpecIdentity, RulePath)>,
}

type ScopeKey = Vec<Vec<Vec<RowKey>>>;

fn scope_key(scope: &[Vec<Located<'_>>]) -> ScopeKey {
    scope
        .iter()
        .map(|rows| rows.iter().map(Located::key).collect())
        .collect()
}

/// Rows the list command at row level `level` iterates.
fn level_rows<'r, 's>(scope: &'r [Vec<Located<'s>>], level: u32) -> &'r [Located<'s>] {
    scope
        .get(usize::try_from(level).expect("BUG: row level fits usize"))
        .expect("BUG: row read outside its list command's scope")
}

#[derive(Debug)]
pub(crate) struct Graph<'a> {
    /// Root spec being planned (for error spec_context).
    pub(crate) main_spec: &'a LemmaSpec,
    pub(crate) data: IndexMap<DataPath, DataDefinition>,
    pub(crate) rules: BTreeMap<RulePath, RuleNode<'a>>,
    /// Rules whose expression conversion failed. Each has an error in `graph_errors`.
    dropped_rules: BTreeSet<RulePath>,
    /// Every rule in dependency order (topo), imports flattened in.
    pub(crate) rule_order: Vec<RulePath>,
    /// Data→data references in dependency order (topo). Targets before dependents
    /// so chained references resolve. Refs to non-reference data are unordered
    /// among themselves.
    pub(crate) data_reference_order: Vec<DataPath>,
    /// Every [`DataDefinition::Reference`] path → where its chain ends.
    /// Filled in [`Self::validate`] after data-reference cycles are rejected.
    pub(crate) reference_ends: IndexMap<DataPath, ReferenceEnd>,
    /// Instances each rule can yield (empty when not spec-valued or list-valued).
    /// Filled by [`Self::link_spec_values`].
    pub(crate) rule_candidates: HashMap<RulePath, SpecCandidates>,
    pub(crate) instance_lists: InstanceLists,
    /// The rules of this plan and every `uses` import rule they reach, with their reads.
    /// Filled by [`Self::sort_show_rules`].
    pub(crate) show_imports: ShowImports,
    /// Dependencies of every Show rule, keyed like `show_order`.
    pub(crate) show_edges: BTreeMap<RulePath, BTreeSet<RulePath>>,
    /// Show rules in dependency order: ready rules in path order, first ready first.
    pub(crate) show_order: Vec<RulePath>,
}

impl<'a> Graph<'a> {
    pub(crate) fn rule_presence(&self, path: &RulePath) -> RulePresence {
        if self.rules.contains_key(path) {
            RulePresence::Present
        } else if self.dropped_rules.contains(path) {
            RulePresence::Dropped
        } else {
            RulePresence::Absent
        }
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
            origin: DataOrigin,
        }

        let mut schema: HashMap<DataPath, Arc<LemmaType>> = HashMap::new();
        let mut declared_suggestions: HashMap<DataPath, BoundValueKind> = HashMap::new();
        let mut declared_values: HashMap<DataPath, BoundValueKind> = HashMap::new();
        let mut values: HashMap<DataPath, (LiteralValue, DataOrigin)> = HashMap::new();
        let mut value_sources: HashMap<DataPath, Source> = HashMap::new();
        let mut references: HashMap<DataPath, PendingReference> = HashMap::new();

        for (path, rfv) in self.data.iter() {
            match rfv {
                DataDefinition::Value {
                    value,
                    resolved_type,
                    origin,
                    source,
                } => {
                    values.insert(path.clone(), (value.clone(), *origin));
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
                    origin,
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
                            origin: *origin,
                        },
                    );
                }
            }
        }

        let mut coercion_errors: Vec<Error> = Vec::new();
        for (path, (value, _)) in values.iter_mut() {
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
                        origin: pending.origin,
                        source,
                    },
                );
            } else if let Some((value, origin)) = values.remove(path) {
                let resolved_type = schema
                    .remove(path)
                    .expect("BUG: value data must have schema type after coercion");
                data.insert(
                    path.clone(),
                    DataDefinition::Value {
                        value,
                        resolved_type,
                        origin,
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

    pub(crate) fn coerce_literal_to_schema_type(
        lit: &TypedLiteral,
        schema_type: &Arc<LemmaType>,
    ) -> Result<TypedLiteral, String> {
        crate::planning::types::coerce_literal_to_schema_type(lit, schema_type)
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
                    &reference_path.data,
                    &mut merged,
                    constraints,
                    &source,
                    ConstraintSuggestionSink {
                        suggestion: &mut raw_suggestion,
                        value: &mut raw_value,
                        resolve_unit: Some(&resolve_unit),
                    },
                ) {
                    Ok(()) => {}
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
                        panic!(
                            "BUG: rule '{target_rule_path}' type not inferred before dependent '{reference_path}' in topological order"
                        )
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
                        &reference_path.data,
                        &mut merged,
                        constraints,
                        source,
                        ConstraintSuggestionSink {
                            suggestion: &mut raw_suggestion,
                            value: &mut raw_value,
                            resolve_unit: Some(&resolve_unit),
                        },
                    ) {
                        Ok(()) => {}
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
                    &reference_path.data,
                    &mut merged,
                    constraints,
                    source,
                    ConstraintSuggestionSink {
                        suggestion: &mut raw_suggestion,
                        value: &mut raw_value,
                        resolve_unit: Some(&resolve_unit),
                    },
                ) {
                    Ok(()) => {}
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

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Add a `depends_on_rules` edge from every rule that reads a rule-target
    /// reference data path to the reference's target rule. Planning needs a
    /// reference target in `completed_rules` (NormalFormId) before the consumer
    /// lowers, and topological sort must detect cycles that flow through reference
    /// paths.
    /// Kind-check every reference a stored row binds, now that data and rule target
    /// types are final. Stored rows are data of the row store, not data definitions,
    /// so they do not pass through [`Self::resolve_data_reference_types`] /
    /// [`Self::resolve_rule_reference_types`]; the compatibility check mirrors them.
    fn check_row_store_reference_types(
        &mut self,
        computed_rule_types: &HashMap<RulePath, Arc<LemmaType>>,
    ) -> Result<(), Vec<Error>> {
        let mut errors = Vec::new();
        let graph = StoredRowKindCheckGraph {
            main_spec: self.main_spec,
            lists: &self.instance_lists,
            data: &self.data,
            rules: &self.rules,
            dropped_rules: &self.dropped_rules,
            computed_rule_types,
        };
        check_stored_row_reference_types(&graph, &mut errors);
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    fn add_rule_reference_dependency_edges(&mut self) {
        let mut reference_targets: BTreeMap<RulePath, BTreeSet<RulePath>> = BTreeMap::new();
        for (rule_path, rule_node) in &self.rules {
            let mut found = BTreeSet::new();
            for node in rule_node.expression.table.nodes() {
                if let ExpressionKind::DataPath(path) = &node.kind {
                    if let Some(ReferenceEnd::Rule(target)) = self.reference_ends.get(path) {
                        found.insert(target.clone());
                    }
                }
            }
            reference_targets.insert(rule_path.clone(), found);
        }

        for (rule_path, targets) in reference_targets {
            self.rules
                .get_mut(&rule_path)
                .expect("BUG: rule with reference targets missing in graph")
                .depends_on_rules
                .extend(targets);
        }
    }

    /// A rule that reads through a `uses … -> effective <rule>` alias depends on that
    /// pin rule, so a pin that reads the import is a planning cycle.
    fn add_dynamic_import_pin_dependency_edges(&mut self) {
        let mut pin_by_alias: HashMap<String, RulePath> = HashMap::new();
        for (path, definition) in &self.data {
            let DataDefinition::Import {
                pin: Some(ImportEffectivePin::Rule(pin)),
                ..
            } = definition
            else {
                continue;
            };
            pin_by_alias.insert(path.data.clone(), pin.clone());
        }
        if pin_by_alias.is_empty() {
            return;
        }
        let mut updates: Vec<(RulePath, RulePath)> = Vec::new();
        for (rule_path, rule_node) in &self.rules {
            let mut pins = BTreeSet::new();
            for node in rule_node.expression.table.nodes() {
                let ExpressionKind::SpecMember { base, .. } = &node.kind else {
                    continue;
                };
                let ExpressionKind::DataPath(path) = rule_node.expression.table.kind(*base) else {
                    continue;
                };
                if let Some(pin) = pin_by_alias.get(&path.data) {
                    pins.insert(pin.clone());
                }
            }
            for pin in pins {
                updates.push((rule_path.clone(), pin));
            }
        }
        for (rule_path, pin) in updates {
            self.rules
                .get_mut(&rule_path)
                .expect("BUG: rule from dynamic pin edge missing in graph")
                .depends_on_rules
                .insert(pin);
        }
    }

    /// A rule that reads data whose type names an options rule depends on that rule,
    /// so its inputs are needs of the reader and the options rule is ordered first.
    fn add_options_rule_dependency_edges(&mut self) {
        let mut updates: Vec<(RulePath, RulePath)> = Vec::new();
        for (rule_path, rule_node) in &self.rules {
            let mut targets = BTreeSet::new();
            for node in rule_node.expression.table.nodes() {
                let ExpressionKind::DataPath(path) = &node.kind else {
                    continue;
                };
                let Some(target) = self
                    .data
                    .get(path)
                    .and_then(|definition| definition.schema_type())
                    .and_then(|lemma_type| execution_plan::options_rule_path(path, lemma_type))
                else {
                    continue;
                };
                if self.rules.contains_key(&target) {
                    targets.insert(target);
                }
            }
            for target in targets {
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

    /// Plans the dynamic unit scales of this spec, once every node is typed:
    ///
    /// - [`RuleNode::scale_targets`]: the scale targets of every dynamic unit a
    ///   typed node of the rule holds or writes.
    /// - A `depends_on_rules` edge from each rule to every local rule that is one of
    ///   its scale targets, so the scale is evaluated before the rule. Uses-qualified
    ///   targets are member cells, evaluated in the rule's walk, and need no edge.
    fn plan_unit_scales(
        &mut self,
        resolved_types: &ResolvedTypesMap,
        inferred_types: &HashMap<RulePath, Arc<LemmaType>>,
        cache: &mut InferExpressionTypeCache,
    ) -> Vec<Error> {
        find_types_by_spec(resolved_types, self.main_spec)
            .expect("BUG: main spec types present when planning unit scales");

        let mut targets_by_rule: Vec<(RulePath, Vec<ReferenceTarget>)> = Vec::new();
        for (rule_path, rule_node) in &self.rules {
            let expression = rule_node.expression.root();
            infer_expression_type(expression, self, inferred_types, resolved_types, cache);
            let key = expression.table as *const ExprTable<ExpressionKind> as usize;
            let node_types = &cache.tables[&key];
            let mut targets: Vec<ReferenceTarget> = Vec::new();
            let mut seen: BTreeSet<ReferenceTarget> = BTreeSet::new();
            let mut push_target = |target: ReferenceTarget| {
                if seen.insert(target.clone()) {
                    targets.push(target);
                }
            };
            for (index, node_type) in node_types.iter().enumerate() {
                for target in node_type.held_value_type().dynamic_scale_targets() {
                    if scale_target_resolvable_in_graph(self, &target) {
                        push_target(target);
                    }
                }
                let node = expression.child(ExprId::from_index(index));
                match node.kind() {
                    ExpressionKind::Literal(literal) => {
                        for target in
                            scale_targets_of_unscaled(&literal.value, literal.lemma_type.as_ref())
                        {
                            if scale_target_resolvable_in_graph(self, &target) {
                                push_target(target);
                            }
                        }
                    }
                    ExpressionKind::UnitConversion(
                        _,
                        SemanticConversionTarget::Unit {
                            unit_name,
                            owning_type,
                        },
                    ) => {
                        if let Some(target) = owning_type.unit_scale_target(unit_name) {
                            if scale_target_resolvable_in_graph(self, target) {
                                push_target(target.clone());
                            }
                        }
                    }
                    ExpressionKind::UnitConversion(
                        inner,
                        SemanticConversionTarget::Type(ConversionTypeKind::Number),
                    ) if node_types[inner.index()].is_measure() => {
                        let source_type = node_types[inner.index()].as_ref();
                        if let Some((unit, _)) =
                            source_type.measure_runtime_signature().into_iter().next()
                        {
                            if let Some(target) = source_type.unit_scale_target(&unit) {
                                if scale_target_resolvable_in_graph(self, target) {
                                    push_target(target.clone());
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            targets_by_rule.push((rule_path.clone(), targets));
        }

        for (rule_path, targets) in targets_by_rule {
            let rule_node = self
                .rules
                .get_mut(&rule_path)
                .expect("BUG: rule from unit-scale planning missing in graph");
            for target in &targets {
                let local_rule = match target {
                    ReferenceTarget::Rule(path) if path.segments.is_empty() => Some(path.clone()),
                    ReferenceTarget::Data(path) if path.segments.is_empty() => {
                        match self.reference_ends.get(path) {
                            Some(ReferenceEnd::Rule(rule)) if rule.segments.is_empty() => {
                                Some(rule.clone())
                            }
                            Some(ReferenceEnd::Rule(_) | ReferenceEnd::Promptable(_)) | None => {
                                None
                            }
                        }
                    }
                    ReferenceTarget::Rule(_) | ReferenceTarget::Data(_) => None,
                };
                if let Some(rule) = local_rule {
                    rule_node.depends_on_rules.insert(rule);
                }
            }
            rule_node.scale_targets = targets;
        }
        Vec::new()
    }

    /// Lemma type of a resolved unit-scale target. Local targets live on this
    /// graph; uses-qualified targets walk import hops into the template plan
    /// (same hop walk as [`reference_target_type`]).
    fn scale_target_lemma_type(
        &self,
        target: &ReferenceTarget,
        inferred_types: &HashMap<RulePath, Arc<LemmaType>>,
    ) -> Arc<LemmaType> {
        let segments = match target {
            ReferenceTarget::Data(path) => &path.segments,
            ReferenceTarget::Rule(rule) => &rule.segments,
        };
        if !segments.is_empty() {
            let mut identity: Option<&SpecIdentity> = None;
            for segment in segments {
                let slot = DataPath::local(segment.uses.to_string());
                let data = match identity {
                    None => &self.data,
                    Some(identity) => &self.instance_lists.templates[identity].data,
                };
                let Some(DataDefinition::Import { row, .. }) = data.get(&slot) else {
                    panic!("BUG: unit scale target hop '{slot}' of {target:?} is not a uses slot");
                };
                identity = Some(row.identity.as_ref());
            }
            let template = &self.instance_lists.templates
                [identity.expect("BUG: a hop scale target has an import identity")];
            return match target {
                ReferenceTarget::Data(path) => instance_list::row_data_type(
                    &template.data,
                    &DataPath::local(path.data.clone()),
                ),
                ReferenceTarget::Rule(rule) => instance_list::row_end_type(
                    template,
                    &RowEnd::Rule(RulePath::new(Vec::new(), rule.rule.clone())),
                ),
            };
        }
        match target {
            ReferenceTarget::Data(path) => {
                if let Some(definition) = self.data.get(path) {
                    return Arc::clone(definition.resolved_type_arc().unwrap_or_else(|| {
                        panic!("BUG: unit scale data target '{path}' has no schema type")
                    }));
                }
                // Declaring-family coordinates: bare target lives on the family
                // template, not on this consumer graph.
                for template in self.instance_lists.templates.values() {
                    if let Some(definition) = template.data.get(path) {
                        return Arc::clone(definition.resolved_type_arc().unwrap_or_else(|| {
                            panic!(
                                "BUG: unit scale data target '{path}' on template '{}' has no schema type",
                                template.spec_name
                            )
                        }));
                    }
                }
                panic!("BUG: resolved unit scale data target '{path}' missing from graph data")
            }
            ReferenceTarget::Rule(path) => {
                if let Some(arc) = inferred_types.get(path) {
                    return Arc::clone(arc);
                }
                if let Some(rule) = self.rules.get(path) {
                    return Arc::clone(&rule.rule_type);
                }
                for template in self.instance_lists.templates.values() {
                    if let Some(rule) = template.rules.get(path) {
                        return Arc::clone(&rule.rule_type);
                    }
                }
                panic!("BUG: resolved unit scale rule target '{path}' missing from graph rules")
            }
        }
    }

    fn check_unit_scale_target_types(
        &self,
        resolved_types: &ResolvedTypesMap,
        inferred_types: &HashMap<RulePath, Arc<LemmaType>>,
    ) -> Vec<Error> {
        let types = find_types_by_spec(resolved_types, self.main_spec)
            .expect("BUG: main spec types present when checking unit scale target types");
        let mut errors = Vec::new();
        for (type_name, lemma_type) in &types.resolved {
            let TypeSpecification::Measure { units, .. } = &lemma_type.specifications else {
                continue;
            };
            let source = self
                .main_spec
                .data
                .iter()
                .find(|data| data.reference.is_local() && data.reference.name == *type_name)
                .map(|data| &data.source_location)
                .or_else(|| {
                    self.main_spec
                        .data
                        .first()
                        .map(|data| &data.source_location)
                })
                .or_else(|| {
                    self.main_spec
                        .rules
                        .first()
                        .map(|rule| &rule.source_location)
                })
                .expect("BUG: spec with a measure type has a source location");
            for unit in units.iter() {
                let Some(target) = unit.resolved_scale_target() else {
                    continue;
                };
                let target_type = self.scale_target_lemma_type(target, inferred_types);
                if !target_type.is_number() {
                    errors.push(engine_error_at_graph(
                        self,
                        source,
                        format!(
                            "unit '{}' conversion factor must be a number, got {}",
                            unit.name,
                            target_type.name()
                        ),
                    ));
                }
            }
        }
        errors
    }

    /// Rules a rule forces while it reads rows, as dependency edges. A row's
    /// references stay deferred until a read lands on their field: opening the row's
    /// context resolves the deferred reference of every field the read touches, in the
    /// context the binding names. A reference this plan wrote forces its target rule
    /// here; one written inside a row spec runs in that row's context and its reads
    /// land on further rows, so the walk follows them until they reach this plan.
    /// Evaluation spawns each target rule at that point, so a cycle through rows would
    /// re-enter the running rule. Runs after [`Self::link_spec_values`] and before the
    /// final topological sort, which reports the cycle.
    fn add_stored_row_dependency_edges(&mut self) {
        let mut updates: Vec<(RulePath, BTreeSet<RulePath>)> = Vec::new();
        for (rule_path, rule_node) in &self.rules {
            let edges = self.forced_rules(self.rule_row_reads(rule_node));
            if !edges.is_empty() {
                updates.push((rule_path.clone(), edges));
            }
        }
        for (rule_path, edges) in updates {
            self.rules
                .get_mut(&rule_path)
                .expect("BUG: rule with stored-row edges missing in graph")
                .depends_on_rules
                .extend(edges);
        }
    }

    /// Reads rule `rule_node` of this plan makes on rows: row fields of the lists its
    /// commands iterate, and members of the rows its expressions yield.
    fn rule_row_reads<'s>(&'s self, rule_node: &RuleNode<'_>) -> Vec<ForcedRead<'s>> {
        let table = &rule_node.expression.table;
        let mut reads: Vec<ForcedRead<'s>> = Vec::new();
        // `scope` holds the rows each enclosing list command iterates, by row
        // level (outermost first).
        let mut visited: HashSet<(ExprId, ScopeKey)> = HashSet::new();
        let mut stack: Vec<(ExprId, Vec<Vec<Located<'s>>>)> = Vec::new();
        for (condition, result) in &rule_node.branches {
            if let Some(condition) = condition {
                stack.push((*condition, Vec::new()));
            }
            stack.push((*result, Vec::new()));
        }
        while let Some((id, scope)) = stack.pop() {
            if !visited.insert((id, scope_key(&scope))) {
                continue;
            }
            match table.kind(id) {
                ExpressionKind::RowField { level, end, .. } => {
                    for row in level_rows(&scope, *level) {
                        reads.push(ForcedRead {
                            at: row.clone(),
                            end: end.clone(),
                        });
                    }
                }
                ExpressionKind::ListCommand {
                    list,
                    op,
                    row_level,
                } => {
                    stack.push((*list, scope.clone()));
                    let mut inner = scope.clone();
                    if let Some(level) = row_level {
                        let depth =
                            u32::try_from(scope.len()).expect("BUG: row scope depth exceeds u32");
                        assert!(
                                *level == depth,
                                "BUG: list command row level {level} but converted at scope depth {depth}"
                            );
                        inner.push(self.expression_rows(table, *list, &scope));
                    }
                    match op {
                        ListOp::Reduce {
                            argument: Some(argument),
                            ..
                        }
                        | ListOp::Sort {
                            argument: Some(argument),
                            ..
                        } => stack.push((*argument, inner)),
                        ListOp::Predicate { expr, .. } => stack.push((*expr, inner)),
                        ListOp::Find { keys } => {
                            for key in keys {
                                stack.push((*key, scope.clone()));
                            }
                        }
                        ListOp::Top { n } => stack.push((*n, scope.clone())),
                        ListOp::With { rows } => {
                            for row in rows {
                                stack.push((*row, scope.clone()));
                            }
                        }
                        ListOp::WithRow { fields, .. } => {
                            for (_, value) in fields {
                                stack.push((*value, scope.clone()));
                            }
                        }
                        ListOp::First
                        | ListOp::Last
                        | ListOp::Count
                        | ListOp::Key { .. }
                        | ListOp::Reduce { argument: None, .. }
                        | ListOp::Sort { argument: None, .. } => {}
                    }
                }
                ExpressionKind::SpecMember {
                    base, path, name, ..
                } => {
                    for row in self.expression_rows(table, *base, &scope) {
                        if let Some(read) = self.member_read(row, path, name) {
                            reads.push(read);
                        }
                    }
                    stack.push((*base, scope.clone()));
                }
                kind => kind.for_each_child(|child| stack.push((child, scope.clone()))),
            }
        }
        reads
    }

    /// Rules of this plan the reads `reads` force, following every read they open.
    fn forced_rules<'s>(&'s self, mut reads: Vec<ForcedRead<'s>>) -> BTreeSet<RulePath> {
        let mut edges: BTreeSet<RulePath> = BTreeSet::new();
        let mut visited: HashSet<(Vec<RowKey>, RowEnd)> = HashSet::new();
        while let Some(read) = reads.pop() {
            if !visited.insert((read.at.key(), read.end.clone())) {
                continue;
            }
            let row = read.at.row;
            let template = self.template_of_row(row);
            let slots: Vec<DataPath> = match &read.end {
                RowEnd::Data(path) => vec![path.clone()],
                RowEnd::Rule(_) => template
                    .row_end_reads(&read.end)
                    .into_iter()
                    .map(|field_read| field_read.slot)
                    .collect(),
            };
            for slot in &slots {
                if let Some(RowBinding::Deferred { target, hops }) = row.get_binding(template, slot)
                {
                    let owner = read.at.scope(*hops);
                    self.target_read(owner, target, &mut reads, &mut edges);
                }
            }
            if let RowEnd::Rule(rule) = &read.end {
                self.template_rule_reads(template, rule, &read.at, &mut reads, RuleWalk::Follow);
            }
        }
        edges
    }

    /// The Show graph through `uses` imports: every import rule the rules of this plan
    /// reach, read on its instance, with the edges and the unfilled data reads of each.
    ///
    /// Dynamic unit scale targets are Show reads too: `Show.data` is static, so every
    /// unbound scale factor a rule can force must already be listed before a unit is
    /// chosen.
    fn reached_imports(&self) -> ShowImports {
        let mut out = ShowImports::default();
        let mut pending: Vec<(Located<'_>, RulePath)> = Vec::new();
        for (rule_path, rule_node) in &self.rules {
            let mut reads = self.rule_row_reads(rule_node);
            self.scale_target_show_reads(None, &rule_node.scale_targets, &mut reads);
            let (edges, data) = self.show_reads(reads, &mut pending, &mut out);
            out.edges.insert(rule_path.clone(), edges);
            out.data_reads.insert(rule_path.clone(), data);
        }
        while let Some((at, rule)) = pending.pop() {
            let path = RulePath::new(
                at.import_segments()
                    .expect("BUG: a pending Show rule sits on a `uses` import chain"),
                rule.rule.clone(),
            );
            if out.rules.contains_key(&path) {
                continue;
            }
            let template = self.template_of_row(at.row);
            out.rules.insert(
                path.clone(),
                (at.row.identity.as_ref().clone(), rule.clone()),
            );
            let mut reads = Vec::new();
            self.template_rule_reads(template, &rule, &at, &mut reads, RuleWalk::Show);
            let scale_targets = &template
                .rules
                .get(&rule)
                .unwrap_or_else(|| {
                    panic!(
                        "BUG: row rule '{rule}' missing from template plan '{}'",
                        template.spec_name
                    )
                })
                .scale_dependencies;
            self.scale_target_show_reads(Some(at.clone()), scale_targets, &mut reads);
            let (mut edges, data) = self.show_reads(reads, &mut pending, &mut out);
            out.need_edges.insert(path.clone(), edges.clone());
            let show_rule = template.show_rules.get(&rule).unwrap_or_else(|| {
                panic!(
                    "BUG: rule '{rule}' missing from Show of template '{}'",
                    template.spec_name
                )
            });
            let mut reads = Vec::new();
            for dependency in &show_rule.depends_on_rules {
                let at = if dependency.segments.is_empty() {
                    at.clone()
                } else {
                    let aliases: Vec<String> = dependency
                        .segments
                        .iter()
                        .map(|segment| segment.uses.to_string())
                        .collect();
                    self.locate_instance(Some(at.clone()), &aliases)
                };
                reads.push(ForcedRead {
                    at,
                    end: RowEnd::Rule(RulePath::new(Vec::new(), dependency.rule.clone())),
                });
            }
            let (written, written_data) = self.show_reads(reads, &mut pending, &mut out);
            assert!(written_data.is_empty(), "BUG: a rule dependency read data");
            edges.extend(written);
            out.edges.insert(path.clone(), edges);
            out.data_reads.insert(path, data);
        }
        out
    }

    /// Show edges and unfilled data reads of the reads `reads`. A rule read on a `uses`
    /// import chain is an edge to that import rule, queued on `pending`; a rule of this
    /// plan is an edge; a data read follows its binding or reference to where it ends.
    /// Reads on list rows follow every read they open, as [`Self::forced_rules`] does.
    fn show_reads<'s>(
        &'s self,
        mut reads: Vec<ForcedRead<'s>>,
        pending: &mut Vec<(Located<'s>, RulePath)>,
        out: &mut ShowImports,
    ) -> (BTreeSet<RulePath>, BTreeSet<DataPath>) {
        let mut edges: BTreeSet<RulePath> = BTreeSet::new();
        let mut data: BTreeSet<DataPath> = BTreeSet::new();
        let mut visited: HashSet<(Vec<RowKey>, RowEnd)> = HashSet::new();
        let mut landings: Vec<Landing<'s>> = Vec::new();
        loop {
            while let Some(landing) = landings.pop() {
                match landing {
                    Landing::Row(read) => reads.push(read),
                    Landing::Plan(RowEnd::Rule(rule)) => {
                        edges.insert(rule);
                    }
                    Landing::Plan(RowEnd::Data(path)) => match self.data.get(&path) {
                        Some(DataDefinition::Reference { .. }) => {
                            match self.reference_ends.get(&path).unwrap_or_else(|| {
                                panic!("BUG: reference '{path}' missing from reference_ends")
                            }) {
                                ReferenceEnd::Rule(rule) => landings.push(
                                    self.target_landing(None, &ReferenceTarget::Rule(rule.clone())),
                                ),
                                ReferenceEnd::Promptable(target) => {
                                    landings.push(self.target_landing(
                                        None,
                                        &ReferenceTarget::Data(target.clone()),
                                    ))
                                }
                            }
                        }
                        Some(DataDefinition::Import { .. }) => {}
                        Some(definition) => {
                            if !definition.is_bound() {
                                data.insert(path);
                            }
                        }
                        None => panic!("BUG: Show read of data '{path}' missing from the plan"),
                    },
                }
            }
            let Some(read) = reads.pop() else {
                break;
            };
            if !visited.insert((read.at.key(), read.end.clone())) {
                continue;
            }
            let row = read.at.row;
            let template = self.template_of_row(row);
            let segments = read.at.import_segments();
            match (&read.end, segments) {
                (RowEnd::Rule(rule), Some(segments)) => {
                    edges.insert(RulePath::new(segments, rule.rule.clone()));
                    pending.push((read.at, rule.clone()));
                }
                (RowEnd::Rule(rule), None) => {
                    for field_read in template.row_end_reads(&read.end) {
                        if let Some(RowBinding::Deferred { target, hops }) =
                            row.get_binding(template, &field_read.slot)
                        {
                            landings.push(self.target_landing(read.at.scope(*hops), target));
                        }
                    }
                    self.template_rule_reads(
                        template,
                        rule,
                        &read.at,
                        &mut reads,
                        RuleWalk::Follow,
                    );
                }
                (RowEnd::Data(slot), segments) => {
                    match row.get_binding(template, slot) {
                        Some(RowBinding::Deferred { target, hops }) => {
                            landings.push(self.target_landing(read.at.scope(*hops), target));
                        }
                        Some(_) => {}
                        None => {
                            let Some(segments) = segments else {
                                continue;
                            };
                            match template.data.get(slot) {
                                Some(DataDefinition::Reference { .. }) => {
                                    let target = match template.reference_ends.get(slot).unwrap_or_else(
                                    || panic!("BUG: reference '{slot}' missing from reference_ends"),
                                ) {
                                    ReferenceEnd::Rule(rule) => ReferenceTarget::Rule(rule.clone()),
                                    ReferenceEnd::Promptable(target) => {
                                        ReferenceTarget::Data(target.clone())
                                    }
                                };
                                    landings
                                        .push(self.target_landing(Some(read.at.clone()), &target));
                                }
                                Some(DataDefinition::Import { .. }) => {}
                                Some(definition) => {
                                    if !definition.is_bound() {
                                        let path = DataPath {
                                            segments,
                                            data: slot.data.clone(),
                                        };
                                        out.data
                                            .insert(path.clone(), row.identity.as_ref().clone());
                                        data.insert(path);
                                    }
                                }
                                None => panic!(
                                    "BUG: Show read of '{slot}' missing from template '{}'",
                                    template.spec_name
                                ),
                            }
                        }
                    }
                }
            }
        }
        (edges, data)
    }

    /// The read `target` makes in context `owner` (`None` for this plan): a rule of
    /// this plan becomes an edge; any other target is a read on the row it names.
    fn target_read<'s>(
        &'s self,
        owner: Option<Located<'s>>,
        target: &ReferenceTarget,
        reads: &mut Vec<ForcedRead<'s>>,
        edges: &mut BTreeSet<RulePath>,
    ) {
        match self.target_landing(owner, target) {
            Landing::Row(read) => reads.push(read),
            Landing::Plan(RowEnd::Rule(rule)) => {
                edges.insert(rule);
            }
            Landing::Plan(RowEnd::Data(_)) => {}
        }
    }

    /// Show reads of dynamic unit scale targets in context `owner` (`None` for this
    /// plan). Uses-qualified targets become row reads. Bare local targets become
    /// ForcedReads on `owner` when set (import rule context); when `owner` is `None`,
    /// local scales stay on this plan's `needed_by` / Show data from expression walks.
    fn scale_target_show_reads<'s>(
        &'s self,
        owner: Option<Located<'s>>,
        targets: &[ReferenceTarget],
        reads: &mut Vec<ForcedRead<'s>>,
    ) {
        for target in targets {
            match self.target_landing(owner.clone(), target) {
                Landing::Row(read) => reads.push(read),
                Landing::Plan(RowEnd::Rule(rule)) => {
                    if let Some(at) = owner.clone() {
                        reads.push(ForcedRead {
                            at,
                            end: RowEnd::Rule(rule),
                        });
                    }
                }
                Landing::Plan(RowEnd::Data(path)) => {
                    if let Some(at) = owner.clone() {
                        reads.push(ForcedRead {
                            at,
                            end: RowEnd::Data(path),
                        });
                    }
                }
            }
        }
    }

    /// Where `target`, read in context `owner` (`None` for this plan), lands.
    fn target_landing<'s>(
        &'s self,
        owner: Option<Located<'s>>,
        target: &ReferenceTarget,
    ) -> Landing<'s> {
        let (segments, end) = match target {
            ReferenceTarget::Rule(rule) => (
                &rule.segments,
                RowEnd::Rule(RulePath::new(Vec::new(), rule.rule.clone())),
            ),
            ReferenceTarget::Data(path) => (
                &path.segments,
                RowEnd::Data(DataPath::local(path.data.clone())),
            ),
        };
        let aliases: Vec<String> = segments
            .iter()
            .map(|segment| {
                assert!(
                    segment.row.is_none(),
                    "BUG: reference target hops are `uses` slots, got row hop `{}`",
                    segment.input_key()
                );
                segment.uses.to_string()
            })
            .collect();
        let at = match (owner, aliases.is_empty()) {
            (None, true) => return Landing::Plan(end),
            (owner, _) if aliases.is_empty() => owner.expect("BUG: owner checked above"),
            (owner, _) => self.locate_instance(owner, &aliases),
        };
        Landing::Row(ForcedRead { at, end })
    }

    /// Reads landing on rows while `rule` of `template` evaluates in the context of
    /// row `at`: every [`NormalFormKind::RowField`] reads rows of the list command at
    /// its level, and a [`NormalFormKind::SpecMember`] reads the rows its base yields.
    /// Rule-reference cells walk the referenced rule in the same context.
    ///
    /// [`RuleWalk::Show`] stops at rule references instead, reading the rule on `at`, and
    /// also reads every data slot of `template` the rule reads on `at`: the direct reads
    /// of one Show rule.
    fn template_rule_reads<'s>(
        &'s self,
        template: &'s ExecutionPlan,
        rule: &RulePath,
        at: &Located<'s>,
        reads: &mut Vec<ForcedRead<'s>>,
        walk: RuleWalk,
    ) {
        let mut rules: Vec<RulePath> = vec![rule.clone()];
        let mut walked: BTreeSet<RulePath> = BTreeSet::new();
        let mut visited: HashSet<(NormalFormId, ScopeKey)> = HashSet::new();
        while let Some(rule) = rules.pop() {
            if !walked.insert(rule.clone()) {
                continue;
            }
            let root = template
                .rules
                .get(&rule)
                .unwrap_or_else(|| {
                    panic!(
                        "BUG: row rule '{rule}' missing from template plan '{}'",
                        template.spec_name
                    )
                })
                .normal_form;
            let mut stack: Vec<(NormalFormId, Vec<Vec<Located<'s>>>)> = vec![(root, Vec::new())];
            while let Some((id, scope)) = stack.pop() {
                if !visited.insert((id, scope_key(&scope))) {
                    continue;
                }
                let nf = template.normal_form(id);
                if let Some(target) = &nf.rule_ref {
                    match walk {
                        RuleWalk::Follow => rules.push(target.clone()),
                        RuleWalk::Show => reads.push(ForcedRead {
                            at: at.clone(),
                            end: RowEnd::Rule(target.clone()),
                        }),
                    }
                    continue;
                }
                match &nf.kind {
                    NormalFormKind::Leaf(LeafKind::DataPath(path)) if walk == RuleWalk::Show => {
                        if !matches!(template.data.get(path), Some(DataDefinition::Import { .. })) {
                            reads.push(ForcedRead {
                                at: at.clone(),
                                end: RowEnd::Data(path.clone()),
                            });
                        }
                        if let Some(options) = template.options_rule_of(path) {
                            reads.push(ForcedRead {
                                at: at.clone(),
                                end: RowEnd::Rule(options),
                            });
                        }
                    }
                    NormalFormKind::RowField { level, end, .. } => {
                        for row in level_rows(&scope, *level) {
                            reads.push(ForcedRead {
                                at: row.clone(),
                                end: end.clone(),
                            });
                        }
                    }
                    NormalFormKind::SpecMember { base, row, .. } => {
                        let (last, hops) = row
                            .ends
                            .split_last()
                            .expect("BUG: a row read has a last end");
                        for base_row in self.normal_cell_rows(template, *base, at, &scope) {
                            let mut held = base_row;
                            for hop in hops {
                                let RowEnd::Data(slot) = hop else {
                                    panic!("BUG: a row read hop is a `uses` slot, got {hop:?}");
                                };
                                held = self.import_hop(held, slot);
                            }
                            reads.push(ForcedRead {
                                at: held,
                                end: last.clone(),
                            });
                        }
                        stack.push((*base, scope.clone()));
                    }
                    NormalFormKind::ListCommand {
                        list,
                        args,
                        row_level,
                        ..
                    } => {
                        stack.push((*list, scope.clone()));
                        let mut inner = scope.clone();
                        if let Some(level) = row_level {
                            let depth = u32::try_from(scope.len())
                                .expect("BUG: row scope depth exceeds u32");
                            assert!(
                                *level == depth,
                                "BUG: list command row level {level} but walked at scope depth {depth}"
                            );
                            inner.push(self.normal_cell_rows(template, *list, at, &scope));
                        }
                        for arg in args {
                            stack.push((*arg, inner.clone()));
                        }
                    }
                    kind => kind.for_each_child(|child| stack.push((child, scope.clone()))),
                }
            }
        }
    }

    /// Rows the cell `id` of `template` can yield while it evaluates in the context of
    /// row `at`, with `scope` the rows of the enclosing list commands. A single row
    /// picked from a list (`first`, `find`) stands in for all of the list's rows.
    fn normal_cell_rows<'s>(
        &'s self,
        template: &'s ExecutionPlan,
        id: NormalFormId,
        at: &Located<'s>,
        scope: &[Vec<Located<'s>>],
    ) -> Vec<Located<'s>> {
        let mut memo: HashMap<NormalFormId, Vec<Located<'s>>> = HashMap::new();
        let mut stack: Vec<(NormalFormId, bool)> = vec![(id, false)];
        while let Some((cell, ready)) = stack.pop() {
            if memo.contains_key(&cell) {
                continue;
            }
            let nf = template.normal_form(cell);
            let children: Vec<NormalFormId> = if nf.rule_ref.is_some() {
                Vec::new()
            } else {
                match &nf.kind {
                    NormalFormKind::SpecMember { base, .. } => vec![*base],
                    NormalFormKind::ListCommand {
                        list,
                        op: NormalFormListOp::With,
                        args,
                        ..
                    } => std::iter::once(*list).chain(args.iter().copied()).collect(),
                    NormalFormKind::ListCommand { list, .. } => vec![*list],
                    NormalFormKind::Piecewise(arms) => arms.iter().map(|(_, body)| *body).collect(),
                    _ => Vec::new(),
                }
            };
            if !ready {
                stack.push((cell, true));
                stack.extend(children.into_iter().map(|child| (child, false)));
                continue;
            }
            let child_rows = |memo: &HashMap<NormalFormId, Vec<Located<'s>>>| -> Vec<Located<'s>> {
                children
                    .iter()
                    .flat_map(|child| memo[child].iter().cloned())
                    .collect()
            };
            let rows = match (&nf.rule_ref, &nf.kind) {
                (Some(rule), _) => self.end_rows(at.clone(), &RowEnd::Rule(rule.clone())),
                (None, NormalFormKind::Leaf(LeafKind::DataPath(path))) => {
                    self.end_rows(at.clone(), &RowEnd::Data(path.clone()))
                }
                (None, NormalFormKind::Leaf(LeafKind::RowValue { level })) => {
                    level_rows(scope, *level).to_vec()
                }
                (None, NormalFormKind::RowField { level, end, .. }) => level_rows(scope, *level)
                    .iter()
                    .flat_map(|row| self.end_rows(row.clone(), end))
                    .collect(),
                (None, NormalFormKind::SpecMember { row, .. }) => child_rows(&memo)
                    .into_iter()
                    .flat_map(|base| self.ends_rows(base, &row.ends))
                    .collect(),
                (None, NormalFormKind::ListCommand { .. } | NormalFormKind::Piecewise(_)) => {
                    child_rows(&memo)
                }
                (None, _) => Vec::new(),
            };
            memo.insert(cell, rows);
        }
        memo.remove(&id)
            .expect("BUG: the requested cell is resolved last")
    }

    /// Rows the expression `id` of a rule of this plan can yield, with `scope` the
    /// rows of the enclosing list commands. A single row picked from a list
    /// (`first`, `find`) stands in for all of the list's rows.
    fn expression_rows<'s>(
        &'s self,
        table: &ExprTable<ExpressionKind>,
        id: ExprId,
        scope: &[Vec<Located<'s>>],
    ) -> Vec<Located<'s>> {
        let mut memo: HashMap<ExprId, Vec<Located<'s>>> = HashMap::new();
        let mut stack: Vec<(ExprId, bool)> = vec![(id, false)];
        while let Some((at, ready)) = stack.pop() {
            if memo.contains_key(&at) {
                continue;
            }
            let children: Vec<ExprId> = match table.kind(at) {
                ExpressionKind::SpecMember { base, .. } => vec![*base],
                ExpressionKind::ListCommand {
                    list,
                    op: ListOp::With { rows },
                    ..
                } => std::iter::once(*list).chain(rows.iter().copied()).collect(),
                ExpressionKind::ListCommand { list, .. } => vec![*list],
                ExpressionKind::Piecewise(arms) => arms.iter().map(|(_, body)| *body).collect(),
                _ => Vec::new(),
            };
            if !ready {
                stack.push((at, true));
                stack.extend(children.into_iter().map(|child| (child, false)));
                continue;
            }
            let child_rows: Vec<Located<'s>> = children
                .iter()
                .flat_map(|child| memo[child].iter().cloned())
                .collect();
            let rows = match table.kind(at) {
                ExpressionKind::DataPath(path) => self.plan_slot_rows(path),
                ExpressionKind::RulePath(rule) => match self.rule_candidates.get(rule) {
                    Some(candidates) => candidates
                        .instances
                        .iter()
                        .map(|instance| self.locate_instance(None, instance))
                        .collect(),
                    None => {
                        assert!(
                            self.dropped_rules.contains(rule),
                            "BUG: rule '{rule}' is linked but has no candidates"
                        );
                        Vec::new()
                    }
                },
                ExpressionKind::RowValue { level, .. } => level_rows(scope, *level).to_vec(),
                ExpressionKind::RowField { level, end, .. } => level_rows(scope, *level)
                    .iter()
                    .flat_map(|row| self.end_rows(row.clone(), end))
                    .collect(),
                ExpressionKind::SpecMember { path, name, .. } => child_rows
                    .into_iter()
                    .flat_map(|base| {
                        let ends = self.member_ends(base.row, path, name);
                        match ends {
                            Some(ends) => self.ends_rows(base, &ends),
                            None => Vec::new(),
                        }
                    })
                    .collect(),
                ExpressionKind::ListCommand { .. } | ExpressionKind::Piecewise(_) => child_rows,
                _ => Vec::new(),
            };
            memo.insert(at, rows);
        }
        memo.remove(&id)
            .expect("BUG: the requested expression is resolved last")
    }

    /// Rows data `path` of this plan holds: its `uses` instance, or the rows its list
    /// writes.
    fn plan_slot_rows(&self, path: &DataPath) -> Vec<Located<'_>> {
        match self.data.get(path) {
            Some(DataDefinition::Import { row, .. }) => vec![Located::root(row)],
            Some(DataDefinition::InstanceList { .. }) => self
                .instance_lists
                .sources
                .get(path)
                .expect("BUG: instance list data has a source")
                .store
                .shape_rows()
                .map(|row| Located::root(row))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Ends `path.name` reads on a row of `row`'s spec; `None` when its spec has no
    /// such member (type checking reports it).
    fn member_ends(&self, row: &RowInstance, path: &[String], name: &str) -> Option<Vec<RowEnd>> {
        let parts: Vec<&str> = path
            .iter()
            .map(String::as_str)
            .chain(std::iter::once(name))
            .collect();
        let template = self.template_of_row(row);
        instance_list::row_end_chain(
            &row.identity,
            instance_list::RowEndView::Plan(template),
            &self.instance_lists.templates,
            &parts,
        )
        .map(|chain| chain.into_iter().map(|hop| hop.end).collect())
    }

    /// The read `path.name` makes on row `at`.
    fn member_read<'s>(
        &'s self,
        at: Located<'s>,
        path: &[String],
        name: &str,
    ) -> Option<ForcedRead<'s>> {
        let ends = self.member_ends(at.row, path, name)?;
        let (last, hops) = ends
            .split_last()
            .expect("BUG: a member chain has a last end");
        let mut at = at;
        for hop in hops {
            let RowEnd::Data(slot) = hop else {
                panic!("BUG: a member chain hop is a `uses` slot, got {hop:?}");
            };
            at = self.import_hop(at, slot);
        }
        Some(ForcedRead {
            at,
            end: last.clone(),
        })
    }

    /// Rows the chain `ends` yields on row `at`.
    fn ends_rows<'s>(&'s self, at: Located<'s>, ends: &[RowEnd]) -> Vec<Located<'s>> {
        let (last, hops) = ends.split_last().expect("BUG: a row read has a last end");
        let mut at = at;
        for hop in hops {
            let RowEnd::Data(slot) = hop else {
                panic!("BUG: a row read hop is a `uses` slot, got {hop:?}");
            };
            at = self.import_hop(at, slot);
        }
        self.end_rows(at, last)
    }

    /// Rows a read of `end` on row `at` yields: the instance of a `uses` slot, the
    /// rows of a list field, or the instances a spec-valued rule of the row's spec
    /// returns.
    fn end_rows<'s>(&'s self, at: Located<'s>, end: &RowEnd) -> Vec<Located<'s>> {
        let template = self.template_of_row(at.row);
        match end {
            RowEnd::Rule(rule) => template
                .rule_candidates
                .get(rule)
                .unwrap_or_else(|| {
                    panic!(
                        "BUG: rule '{rule}' of '{}' has no candidate list",
                        template.spec_name
                    )
                })
                .instances
                .iter()
                .map(|instance| self.locate_instance(Some(at.clone()), instance))
                .collect(),
            RowEnd::Data(path) => match template.data.get(path) {
                Some(DataDefinition::Import { .. }) => vec![self.import_hop(at, path)],
                Some(DataDefinition::InstanceList { .. }) => {
                    match at.row.get_binding(template, path) {
                        Some(RowBinding::Rows { rows, hops, .. }) => {
                            let owner = at.context_at(*hops);
                            rows.iter()
                                .map(|row| Located {
                                    row,
                                    context: owner.clone(),
                                })
                                .collect()
                        }
                        Some(RowBinding::Bound(_)) | None => {
                            let context = at.inside();
                            template
                                .instance_lists
                                .get(path)
                                .expect("BUG: template instance list has a source")
                                .store
                                .shape_rows()
                                .map(|row| Located {
                                    row,
                                    context: context.clone(),
                                })
                                .collect()
                        }
                        Some(binding) => {
                            panic!("BUG: list field '{path}' holds {binding:?}")
                        }
                    }
                }
                _ => Vec::new(),
            },
        }
    }

    /// The instance in `uses` slot `slot` of row `at`: the nested row a consumer wrote,
    /// or the row the row spec declares.
    fn import_hop<'s>(&'s self, at: Located<'s>, slot: &DataPath) -> Located<'s> {
        let template = self.template_of_row(at.row);
        let row = match at.row.get_binding(template, slot) {
            Some(RowBinding::Instance(row)) => row.as_ref(),
            None => match template.data.get(slot) {
                Some(DataDefinition::Import { row, .. }) => row.as_ref(),
                other => panic!("BUG: `uses` hop '{slot}' is not a `uses` slot: {other:?}"),
            },
            Some(binding) => panic!("BUG: `uses` slot '{slot}' holds {binding:?}"),
        };
        Located {
            row,
            context: at.inside(),
        }
    }

    /// The instance `aliases` names from context `from` (`None` for this plan).
    fn locate_instance<'s>(&'s self, from: Option<Located<'s>>, aliases: &[String]) -> Located<'s> {
        let (first, rest) = aliases
            .split_first()
            .expect("BUG: an instance has an alias");
        let first = DataPath::local(first.clone());
        let mut at = match from {
            Some(from) => self.import_hop(from, &first),
            None => match self.data.get(&first) {
                Some(DataDefinition::Import { row, .. }) => Located::root(row),
                other => panic!("BUG: instance `{first}` is not a `uses` of the plan: {other:?}"),
            },
        };
        for alias in rest {
            at = self.import_hop(at, &DataPath::local(alias.clone()));
        }
        at
    }

    /// Template plan of the spec of `row`.
    fn template_of_row(&self, row: &RowInstance) -> &ExecutionPlan {
        self.instance_lists
            .templates
            .get(&row.identity)
            .expect("BUG: rows are rows of a planned row spec")
    }

    /// For each [`DataDefinition::Reference`] in `self.data`, follow the chain to
    /// its end (promptable slot or rule). Cycles among data-target references are
    /// impossible here because `compute_data_reference_order` already rejected them.
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
        out
    }

    /// Where the data path `start` ends: itself when it holds a slot, else the end
    /// of the reference chain it starts. Every target was validated by planning.
    fn expect_reference_end(&self, start: &DataPath) -> ReferenceEnd {
        let mut path = start;
        for _ in 0..=self.data.len() {
            match self.data.get(path) {
                Some(
                    DataDefinition::Value { .. }
                    | DataDefinition::TypeDeclaration { .. }
                    | DataDefinition::InstanceList { .. },
                ) => return ReferenceEnd::Promptable(path.clone()),
                Some(DataDefinition::Reference {
                    target: ReferenceTarget::Rule(rule),
                    ..
                }) => return ReferenceEnd::Rule(rule.clone()),
                Some(DataDefinition::Reference {
                    target: ReferenceTarget::Data(next),
                    ..
                }) => path = next,
                Some(DataDefinition::Import { .. }) => {
                    panic!("BUG: data reference chain from '{start}' ends at import '{path}'")
                }
                None => {
                    panic!("BUG: data-target reference chain from '{start}' ends at missing data '{path}'")
                }
            }
        }
        panic!("BUG: data reference chain from '{start}' outlasts plan data (cycles are rejected)")
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
            let Some(DataDefinition::Reference {
                target: ReferenceTarget::Data(target_path),
                ..
            }) = self.data.get(p)
            else {
                unreachable!("BUG: reference path collected above is not a data-target reference");
            };
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
                .map(|p| {
                    let source = self
                        .data
                        .get(p)
                        .expect("BUG: cycle member collected from self.data")
                        .source();
                    reference_error(
                        self.main_spec,
                        source,
                        format!("Circular data reference ({})", cycle_display),
                    )
                })
                .collect();
            return Err(errors);
        }

        Ok(result)
    }

    /// Cycles among the data bindings of the `uses` instances this plan writes
    /// (`-> with a: i.b` and `-> with b: i.a`): a chain of data reads through bindings
    /// and references that returns to a read it already made. Each is an error naming
    /// the data on the cycle by its path from this plan.
    fn check_binding_reference_cycles(&self) -> Result<(), Vec<Error>> {
        enum Step<'s> {
            Row(Located<'s>, DataPath),
            Plan(DataPath),
        }
        let mut errors = Vec::new();
        let mut reported: BTreeSet<Vec<String>> = BTreeSet::new();
        let mut rows: Vec<(Located<'_>, &Source)> = Vec::new();
        for definition in self.data.values() {
            if let DataDefinition::Import { row, source, .. } = definition {
                rows.push((Located::root(row), source));
            }
        }
        while let Some((at, source)) = rows.pop() {
            let template = self.template_of_row(at.row);
            for (slot, binding) in at.row.bindings.iter().enumerate() {
                match binding {
                    Some(RowBinding::Instance(nested)) => rows.push((
                        Located {
                            row: nested,
                            context: at.inside(),
                        },
                        source,
                    )),
                    Some(RowBinding::Deferred {
                        target: ReferenceTarget::Data(_),
                        ..
                    }) => {
                        let (path, _) = template
                            .data
                            .get_index(slot)
                            .expect("BUG: row binding slot is data of its template");
                        let mut chain: Vec<(Vec<RowKey>, DataPath)> = Vec::new();
                        let mut names: Vec<String> = Vec::new();
                        let mut step = Step::Row(at.clone(), path.clone());
                        loop {
                            let landing = match step {
                                Step::Plan(path) => match self.data.get(&path) {
                                    Some(DataDefinition::Reference {
                                        target: target @ ReferenceTarget::Data(_),
                                        ..
                                    }) => self.target_landing(None, target),
                                    _ => break,
                                },
                                Step::Row(at, slot) => {
                                    let read = (at.key(), slot.clone());
                                    if let Some(start) = chain.iter().position(|seen| *seen == read)
                                    {
                                        let mut cycle = names[start..].to_vec();
                                        cycle.sort();
                                        if reported.insert(cycle.clone()) {
                                            errors.push(reference_error(
                                                self.main_spec,
                                                source,
                                                format!(
                                                    "Circular data reference ({})",
                                                    cycle.join(", ")
                                                ),
                                            ));
                                        }
                                        break;
                                    }
                                    let segments = at.import_segments().expect(
                                        "BUG: binding cycles walk `uses` rows only: imports, their nested instances, and `uses` hops",
                                    );
                                    names.push(
                                        DataPath::new(segments, slot.data.clone()).input_key(),
                                    );
                                    chain.push(read);
                                    let template = self.template_of_row(at.row);
                                    match at.row.get_binding(template, &slot) {
                                        Some(RowBinding::Deferred {
                                            target: target @ ReferenceTarget::Data(_),
                                            hops,
                                        }) => self.target_landing(at.scope(*hops), target),
                                        Some(_) => break,
                                        None => match template.reference_ends.get(&slot) {
                                            Some(ReferenceEnd::Promptable(end)) => self
                                                .target_landing(
                                                    Some(at.clone()),
                                                    &ReferenceTarget::Data(end.clone()),
                                                ),
                                            Some(ReferenceEnd::Rule(_)) | None => break,
                                        },
                                    }
                                }
                            };
                            step = match landing {
                                Landing::Row(ForcedRead {
                                    at,
                                    end: RowEnd::Data(slot),
                                }) => Step::Row(at, slot),
                                Landing::Plan(RowEnd::Data(path)) => Step::Plan(path),
                                Landing::Row(ForcedRead {
                                    end: RowEnd::Rule(_),
                                    ..
                                })
                                | Landing::Plan(RowEnd::Rule(_)) => break,
                            };
                        }
                    }
                    Some(_) | None => {}
                }
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// What every rule can yield as a spec value, and the errors of member reads on
    /// those values. Repeats until the candidates stop changing: a rule read on a
    /// self-list row has no edge (the row evaluates it), so its candidates reach the
    /// reader through the previous pass.
    fn link_spec_values(&mut self) -> Result<Vec<Error>, Vec<Error>> {
        use crate::planning::spec_value::{
            assert_expression_candidates_linked, spec_member_errors, CandidateScope, CandidateTable,
        };

        let rule_paths: HashSet<RulePath> = self.rules.keys().cloned().collect();
        let dropped_rules = self.dropped_rules.clone();
        let mut all_rules: BTreeSet<RulePath> = rule_paths.iter().cloned().collect();
        all_rules.extend(dropped_rules.iter().cloned());
        let presence = move |rule: &RulePath| {
            if rule_paths.contains(rule) {
                RulePresence::Present
            } else if dropped_rules.contains(rule) {
                RulePresence::Dropped
            } else {
                RulePresence::Absent
            }
        };
        let order = self.topological_sort()?;
        let row_end_reads =
            |identity: &SpecIdentity, end: &RowEnd| self.row_end_reads(identity, end);
        let mut previous: HashMap<RulePath, SpecCandidates> = HashMap::new();
        loop {
            let mut rule_candidates = previous.clone();
            let mut member_errors = Vec::new();
            for path in &order {
                let node = self
                    .rules
                    .get(path)
                    .expect("BUG: rule from topological sort not in graph");
                let scope = CandidateScope {
                    rule_candidates: &rule_candidates,
                    data: &self.data,
                    rules: &all_rules,
                    sources: &self.instance_lists.sources,
                    rule_presence: &presence,
                    row_end_reads: &row_end_reads,
                    self_identity: &self.instance_lists.self_identity,
                    templates: &self.instance_lists.templates,
                };
                let table = CandidateTable::build(&node.expression, &scope);
                let mut candidates = SpecCandidates::default();
                for (_, result) in &node.branches {
                    candidates.extend(table.candidates(*result));
                }
                for (source, message) in spec_member_errors(&node.expression, &table, &scope) {
                    let source = source.expect("BUG: spec member expression carries its source");
                    member_errors.push(Error::validation_with_context(
                        message,
                        Some(source),
                        None::<String>,
                        Some(self.main_spec),
                        None,
                    ));
                }
                rule_candidates.insert(path.clone(), candidates);
            }
            if rule_candidates == previous {
                let scope = CandidateScope {
                    rule_candidates: &rule_candidates,
                    data: &self.data,
                    rules: &all_rules,
                    sources: &self.instance_lists.sources,
                    rule_presence: &presence,
                    row_end_reads: &row_end_reads,
                    self_identity: &self.instance_lists.self_identity,
                    templates: &self.instance_lists.templates,
                };
                for node in self.rules.values() {
                    assert_expression_candidates_linked(&node.expression, &scope);
                }
                self.rule_candidates = rule_candidates;
                return Ok(member_errors);
            }
            previous = rule_candidates;
        }
    }

    /// Row fields a read of `end` on a row of `identity` touches
    /// ([`ExecutionPlan::row_end_reads`]); rows of the spec being planned read this
    /// graph, since their plan is not built yet.
    fn row_end_reads(&self, identity: &SpecIdentity, end: &RowEnd) -> Vec<RowFieldRead> {
        match self.instance_lists.row_template_of(identity) {
            Some(RowTemplate::Plan(template)) => template.row_end_reads(end),
            Some(RowTemplate::Root) => self.root_row_end_reads(end),
            None => Vec::new(),
        }
    }

    fn root_row_end_reads(&self, end: &RowEnd) -> Vec<RowFieldRead> {
        let mut reads: Vec<RowFieldRead> = Vec::new();
        let push = |path: &DataPath, reads: &mut Vec<RowFieldRead>| {
            if let Some(read) = self.root_row_slot(path) {
                if !reads.contains(&read) {
                    reads.push(read);
                }
            }
        };
        let rule_of_data = |path: &DataPath| -> Option<RulePath> {
            let mut current = path;
            for _ in 0..=self.data.len() {
                match self.data.get(current)? {
                    DataDefinition::Reference {
                        target: ReferenceTarget::Data(next),
                        ..
                    } => current = next,
                    DataDefinition::Reference {
                        target: ReferenceTarget::Rule(rule),
                        ..
                    } => return Some(rule.clone()),
                    _ => return None,
                }
            }
            panic!(
                "BUG: data reference chain from '{path}' outlasts graph data (cycles are rejected)"
            )
        };
        let start_rule = match end {
            RowEnd::Data(path) => match rule_of_data(path) {
                Some(rule) => Some(rule),
                None => {
                    push(path, &mut reads);
                    None
                }
            },
            RowEnd::Rule(rule) => Some(rule.clone()),
        };
        match start_rule {
            None => {}
            Some(rule) => {
                let mut pending = vec![rule];
                let mut seen: BTreeSet<RulePath> = BTreeSet::new();
                while let Some(rule) = pending.pop() {
                    if !seen.insert(rule.clone()) {
                        continue;
                    }
                    let Some(node) = self.rules.get(&rule) else {
                        continue;
                    };
                    for id in node.expression.table.ids() {
                        if let ExpressionKind::DataPath(path) = node.expression.table.kind(id) {
                            push(path, &mut reads);
                        }
                    }
                    pending.extend(node.depends_on_rules.iter().cloned());
                }
            }
        }
        reads
    }

    /// The slot a row of this spec supplies when `path` is read on it: the data itself
    /// or the end of its reference chain, unless the spec binds it. `unbound` when the
    /// spec gives it no value.
    fn root_row_slot(&self, path: &DataPath) -> Option<RowFieldRead> {
        let mut current = path;
        for _ in 0..=self.data.len() {
            match self.data.get(current)? {
                DataDefinition::Reference {
                    target: ReferenceTarget::Data(next),
                    local_value: None,
                    ..
                } => current = next,
                DataDefinition::Reference { .. } | DataDefinition::Import { .. } => return None,
                definition @ (DataDefinition::TypeDeclaration { .. }
                | DataDefinition::Value { .. }
                | DataDefinition::InstanceList { .. }) => {
                    return (!definition.is_bound()).then(|| RowFieldRead {
                        slot: current.clone(),
                        unbound: definition.value().is_none(),
                    });
                }
            }
        }
        panic!("BUG: data reference chain from '{path}' outlasts graph data (cycles are rejected)")
    }

    /// Filter `dependencies` to rules that are present.
    fn present_rule_dependencies(&self, dependencies: Vec<RulePath>) -> BTreeSet<RulePath> {
        let mut present = BTreeSet::new();
        for dependency in dependencies {
            match self.rule_presence(&dependency) {
                RulePresence::Present => {
                    present.insert(dependency);
                }
                RulePresence::Dropped => {}
                RulePresence::Absent => {
                    panic!("BUG: rule '{dependency}' neither in graph nor dropped")
                }
            }
        }
        present
    }

    /// The Show graph: every rule of this plan and every `uses` import rule it reaches,
    /// with their edges, sorted. A cycle through import rules is an error naming every
    /// rule on it by its path from this plan.
    fn sort_show_rules(&mut self) -> Result<(), Vec<Error>> {
        let show_imports = self.reached_imports();
        let mut edges: BTreeMap<RulePath, BTreeSet<RulePath>> = BTreeMap::new();
        for (path, node) in &self.rules {
            let mut rule_edges: BTreeSet<RulePath> = node
                .depends_on_rules
                .iter()
                .filter(|dependency| self.rules.contains_key(*dependency))
                .cloned()
                .collect();
            rule_edges.extend(show_imports.edges.get(path).into_iter().flatten().cloned());
            edges.insert(path.clone(), rule_edges);
        }
        for path in show_imports.rules.keys() {
            edges.insert(
                path.clone(),
                show_imports
                    .edges
                    .get(path)
                    .expect("BUG: reached import rule has Show edges")
                    .clone(),
            );
        }
        let mut in_degree: BTreeMap<&RulePath, usize> = BTreeMap::new();
        let mut dependents: BTreeMap<&RulePath, Vec<&RulePath>> = BTreeMap::new();
        for (path, dependencies) in &edges {
            in_degree.insert(path, dependencies.len());
            for dependency in dependencies {
                assert!(
                    edges.contains_key(dependency),
                    "BUG: Show edge '{path}' -> '{dependency}' leaves the Show rules"
                );
                dependents.entry(dependency).or_default().push(path);
            }
        }
        let mut queue: VecDeque<&RulePath> = in_degree
            .iter()
            .filter(|(_, degree)| **degree == 0)
            .map(|(path, _)| *path)
            .collect();
        let mut order: Vec<RulePath> = Vec::with_capacity(edges.len());
        while let Some(path) = queue.pop_front() {
            for dependent in dependents.get(path).into_iter().flatten() {
                let degree = in_degree
                    .get_mut(dependent)
                    .expect("BUG: Show rule missing from in_degree");
                *degree -= 1;
                if *degree == 0 {
                    queue.push_back(dependent);
                }
            }
            order.push(path.clone());
        }
        if order.len() != edges.len() {
            let unsorted: Vec<&RulePath> =
                edges.keys().filter(|path| !order.contains(path)).collect();
            let missing = nodes_on_cycles(&unsorted, |path| {
                edges
                    .get(path)
                    .expect("BUG: unsorted Show rule missing from its edges")
                    .iter()
                    .collect()
            });
            let rules_involved: String = missing
                .iter()
                .map(|path| path.input_key())
                .collect::<Vec<_>>()
                .join(", ");
            let message = format!("Circular dependency (rules: {rules_involved})");
            return Err(missing
                .into_iter()
                .map(|path| {
                    let source = match self.rules.get(path) {
                        Some(node) => node.source.clone(),
                        None => {
                            let (identity, rule) = show_imports.rules.get(path).expect(
                                "BUG: Show rule is neither local nor a reached import rule",
                            );
                            self.instance_lists
                                .templates
                                .get(identity)
                                .unwrap_or_else(|| {
                                    panic!(
                                        "BUG: import rule '{path}' has no template of '{identity}'"
                                    )
                                })
                                .rules
                                .get(rule)
                                .unwrap_or_else(|| {
                                    panic!("BUG: import rule '{path}' missing from its template")
                                })
                                .source
                                .clone()
                        }
                    };
                    Error::validation_with_context(
                        message.clone(),
                        Some(source),
                        None::<String>,
                        Some(self.main_spec),
                        None,
                    )
                })
                .collect());
        }
        self.show_imports = show_imports;
        self.show_edges = edges;
        self.show_order = order;
        Ok(())
    }

    /// Dependency order of every rule in the graph.
    fn topological_sort(&mut self) -> Result<Vec<RulePath>, Vec<Error>> {
        let mut depends_on: BTreeMap<RulePath, BTreeSet<RulePath>> = BTreeMap::new();
        let local_deps: Vec<(RulePath, Vec<RulePath>)> = self
            .rules
            .iter()
            .map(|(rule_path, rule_node)| {
                (
                    rule_path.clone(),
                    rule_node.depends_on_rules.iter().cloned().collect(),
                )
            })
            .collect();
        for (rule_path, dependencies) in local_deps {
            let present = self.present_rule_dependencies(dependencies);
            depends_on.insert(rule_path, present);
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
            let unsorted: Vec<&RulePath> = depends_on
                .keys()
                .filter(|rule| !result.contains(rule))
                .collect();
            let on_cycles = nodes_on_cycles(&unsorted, |rule| {
                depends_on
                    .get(rule)
                    .expect("BUG: unsorted rule missing from depends_on")
                    .iter()
                    .collect()
            });
            let cycle: Vec<Source> = on_cycles
                .iter()
                .filter_map(|rule| self.rules.get(*rule).map(|n| n.source.clone()))
                .collect();

            if cycle.is_empty() {
                unreachable!(
                    "BUG: circular dependency detected but no sources could be collected ({} rules on cycles)",
                    on_cycles.len()
                );
            }
            let rules_involved: String = on_cycles
                .iter()
                .map(|rule| rule.input_key())
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

    /// Named on a self-list row (`components.unit_cost`), by this rule or another.
    /// Planning twin of eval [`crate::planning::execution_plan::ExecutionPlan::self_row_rules`].
    /// Not an edge in [`Self::depends_on_rules`]: child rows are evaluated first.
    pub recursive: bool,

    /// Computed type of this rule's result (populated during validation)
    /// Every rule MUST have a type (Lemma is strictly typed)
    pub rule_type: Arc<LemmaType>,

    /// Scale targets of every dynamic unit a typed node of [`Self::expression`]
    /// holds or writes, in this plan's coordinates. Evaluated before the rule body.
    /// Filled by [`Graph::plan_unit_scales`].
    pub scale_targets: Vec<ReferenceTarget>,

    /// Spec this rule belongs to (for type resolution during validation)
    pub spec: &'a LemmaSpec,
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
    /// Errors of a nested spec whose template could not be built. When any nested
    /// spec fails, the build aborts and reports only these: the consumer's own
    /// follow-on errors are cascading noise (references the failed spec can no
    /// longer resolve), and the nested errors already name the root cause.
    nested_errors: Vec<Error>,
    nested_spec_failed: bool,
    main_spec: &'a LemmaSpec,
    limits: &'a ResourceLimits,
    instance_lists: InstanceLists,
    interners: &'i mut instance_list::PassInterners,
    /// Local rules named on a self-list row while converting; marked [`RuleNode::recursive`]
    /// after every rule node exists.
    self_list_row_rules: BTreeSet<RulePath>,
}

struct RuleExpressionConversion<'a, 'b> {
    /// Nodes of every expression converted for the rule being added.
    table: ExprTable<ExpressionKind>,
    spec: &'a LemmaSpec,
    /// Repository `spec` is loaded under.
    repository: &'b Arc<LemmaRepository>,
    data_map: &'b HashMap<String, &'b LemmaData>,
    rule_names: &'b HashSet<&'b str>,
    /// Rule being converted. Never a clash for a row field: it cannot read itself.
    rule: &'b str,
    effective: &'b EffectiveDate,
    depends_on_rules: &'b mut BTreeSet<RulePath>,
    /// Instance-list commands enclosing the expression being converted, outermost first.
    row_scopes: Vec<RowScope>,
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

/// What an identity search looks for in the expressions it walks.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum SearchKind {
    /// The row spec of the instance list an expression yields.
    List,
    /// The row spec of the single row an expression yields (`list -> first`).
    Row,
}

/// Where the identity a nested search finds goes.
enum Continuation<'a> {
    /// It is the answer of the whole search.
    Answer,
    /// The list searched for a `-> first` / `-> last` / `-> find`: its row spec is the
    /// enclosing row search's answer.
    RowOfList,
    /// The row a member is read on: `path.name` of that row is the list the enclosing
    /// list search is after.
    Member { path: &'a [String], name: &'a str },
}

/// One depth-first search with an explicit work stack. Nested searches (the list behind a
/// `-> first`, the row a member is read on) stack on top of it; the first expression that
/// yields an identity wins, in source order.
struct IdentitySearch<'a> {
    kind: SearchKind,
    pending: Vec<Pending<'a>>,
    /// Rules entered by this search and the searches it nests in, per kind.
    visiting: BTreeSet<(SearchKind, RulePath)>,
    on_found: Continuation<'a>,
}

impl<'a> IdentitySearch<'a> {
    fn new(kind: SearchKind, first: Pending<'a>, on_found: Continuation<'a>) -> Self {
        Self {
            kind,
            pending: vec![first],
            visiting: BTreeSet::new(),
            on_found,
        }
    }

    fn nested(&self, kind: SearchKind, first: Pending<'a>, on_found: Continuation<'a>) -> Self {
        Self {
            kind,
            pending: vec![first],
            visiting: self.visiting.clone(),
            on_found,
        }
    }

    /// `false` when `rule` was already entered by this search or an enclosing one.
    fn enter(&mut self, rule: &RulePath) -> bool {
        self.visiting.insert((self.kind, rule.clone()))
    }
}

/// Outcome of one work item of an [`IdentitySearch`].
enum SearchStep<'a> {
    Next,
    Nested(IdentitySearch<'a>),
    Found(SpecIdentity),
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
        ast::ListCommand::First
        | ast::ListCommand::Last
        | ast::ListCommand::Count
        | ast::ListCommand::Key { .. } => VecDeque::new(),
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
        | ast::ListCommand::Where { form } => {
            let expr = match form {
                ast::ListPredicate::Value { expr } | ast::ListPredicate::Instance { expr } => expr,
            };
            VecDeque::from([ListArgument::InRows(*expr)])
        }
        ast::ListCommand::Top { n } => VecDeque::from([ListArgument::Enclosing(*n)]),
        ast::ListCommand::With { rows } => match (rows, row_scope) {
            (ast::ListWithRows::Values(values), None)
            | (ast::ListWithRows::Values(values), Some(_)) => values
                .iter()
                .map(|value| ListArgument::Enclosing(*value))
                .collect(),
            (ast::ListWithRows::FieldPairs(_), None) => VecDeque::new(),
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

fn list_predicate_op(
    form: &ast::ListPredicate,
    kind: ListPredKind,
    converted: &mut impl Iterator<Item = ExprId>,
) -> ListOp {
    let expr = converted
        .next()
        .expect("BUG: one converted argument per list command argument");
    match form {
        ast::ListPredicate::Value { .. } => ListOp::Predicate {
            kind,
            form: ast::ListPredicateForm::Value,
            expr,
        },
        ast::ListPredicate::Instance { .. } => ListOp::Predicate {
            kind,
            form: ast::ListPredicateForm::Instance,
            expr,
        },
    }
}

/// Spec being converted.
struct SourceScope<'s> {
    spec: &'s LemmaSpec,
}

/// Outcome of reading a name as a row field inside an instance-list command.
enum RowFieldLookup {
    /// The [`ExpressionKind::RowField`] read, and the member read on it when the field
    /// is a `uses` slot read through (`items.sub.doubled`).
    Field {
        field: Box<ExpressionKind>,
        member: Option<(Vec<String>, String)>,
    },
    NotARowField,
    /// An error was pushed.
    Failed,
}

/// The row field the first hop of `chain` reads on rows of `identity` at `level`, and
/// the member read through it for the rest of `parts`.
fn row_field_lookup(
    level: u32,
    identity: &SpecIdentity,
    chain: Vec<RowHop>,
    parts: &[&str],
    qualifier: Option<Vec<String>>,
) -> RowFieldLookup {
    let first = chain
        .into_iter()
        .next()
        .expect("BUG: a row end chain has a first hop");
    assert_eq!(
        first.identity, *identity,
        "BUG: a row field chain starts on the rows it reads"
    );
    let member = match parts {
        [_] => None,
        [_, through @ .., name] => Some((
            through.iter().map(|part| (*part).to_string()).collect(),
            (*name).to_string(),
        )),
        [] => unreachable!("BUG: a row field names at least one part"),
    };
    RowFieldLookup::Field {
        field: Box::new(ExpressionKind::RowField {
            level,
            identity: first.identity,
            end: first.end,
            qualifier,
        }),
        member,
    }
}

/// Rows visible while converting one list command argument.
#[derive(Clone)]
enum RowScope {
    Value {
        level: u32,
        list: ExprId,
    },
    Instance {
        /// Path of the list operand (`menu`, `card`+`menu`) when it is a plain reference.
        list_path: Option<Vec<String>>,
        identity: SpecIdentity,
        template: RowTemplate,
        level: u32,
    },
}

impl RowScope {
    fn level(&self) -> u32 {
        match self {
            Self::Value { level, .. } | Self::Instance { level, .. } => *level,
        }
    }
}

/// A spec whose imports are being resolved into `local_types`. It is finished after its
/// last import.
struct SpecTypesFrame<'a> {
    spec: &'a LemmaSpec,
    repository: Arc<LemmaRepository>,
    effective: EffectiveDate,
    next_data: usize,
    /// Remaining dynamic-import candidates to visit before the next data row.
    pending_targets: VecDeque<discovery::ResolvedDependencyEdge<'a>>,
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

/// Read-only graph state the stored-row reference kind-check resolves field,
/// target, and rule types through.
struct StoredRowKindCheckGraph<'a> {
    main_spec: &'a LemmaSpec,
    lists: &'a InstanceLists,
    data: &'a IndexMap<DataPath, DataDefinition>,
    rules: &'a BTreeMap<RulePath, RuleNode<'a>>,
    dropped_rules: &'a BTreeSet<RulePath>,
    computed_rule_types: &'a HashMap<RulePath, Arc<LemmaType>>,
}

/// A row whose reference bindings resolve in the plan being built: `depth` instances
/// below that plan, named `prefix` for messages, with `source` the place that wrote it.
pub(crate) struct OwnedRow<'r> {
    pub row: &'r RowInstance,
    pub depth: u32,
    pub prefix: Vec<String>,
    pub source: &'r Source,
}

/// Calls `f` with every reference binding the plan holding `data` and `lists` wrote,
/// in plan order: on its `uses` rows and its stored list rows, and on the rows nested
/// in them. Bindings written inside a row spec belong to that spec's plan.
pub(crate) fn for_each_owned_reference<'r>(
    data: &'r IndexMap<DataPath, DataDefinition>,
    lists: &'r InstanceLists,
    mut f: impl FnMut(&OwnedRow<'r>, usize, &'r ReferenceTarget),
) {
    let mut pending: Vec<OwnedRow<'r>> = Vec::new();
    for (path, definition) in data {
        if let DataDefinition::Import { row, source, .. } = definition {
            pending.push(OwnedRow {
                row,
                depth: 0,
                prefix: vec![path.data.clone()],
                source,
            });
        }
    }
    for source in lists.sources.values() {
        for row in &source.store.rows {
            pending.push(written_owned_row(row, 0, &[]));
        }
    }
    pending.reverse();
    while let Some(owned) = pending.pop() {
        let row = owned.row;
        let mut nested_rows: Vec<OwnedRow<'r>> = Vec::new();
        for (slot, binding) in row.bindings.iter().enumerate() {
            match binding {
                None | Some(RowBinding::Bound(_)) => {}
                Some(RowBinding::Rows { rows, hops, .. }) => {
                    let depth = owned
                        .depth
                        .checked_sub(*hops)
                        .expect("BUG: list rows hop above the plan being built");
                    for nested in rows {
                        nested_rows.push(written_owned_row(nested, depth, &owned.prefix));
                    }
                }
                Some(RowBinding::Instance(nested)) => {
                    let RowOrigin::Import { alias } = &nested.origin else {
                        unreachable!("BUG: a nested instance binding holds an import row");
                    };
                    let mut prefix = owned.prefix.clone();
                    prefix.push(alias.to_string());
                    nested_rows.push(OwnedRow {
                        row: nested,
                        depth: owned.depth + 1,
                        prefix,
                        source: owned.source,
                    });
                }
                Some(RowBinding::Deferred { target, hops }) => {
                    if *hops == owned.depth {
                        f(&owned, slot, target);
                    }
                }
            }
        }
        pending.extend(nested_rows.into_iter().rev());
    }
}

/// Kind-check the reference bindings the plan being built wrote against their template
/// field types.
fn check_stored_row_reference_types(graph: &StoredRowKindCheckGraph, errors: &mut Vec<Error>) {
    for_each_owned_reference(graph.data, graph.lists, |owned, slot, target| {
        let template = graph
            .lists
            .templates
            .get(&owned.row.identity)
            .expect("BUG: written rows are rows of a planned row spec");
        let (field_path, definition) = template
            .data
            .get_index(slot)
            .expect("BUG: row slot is template data");
        let display_path = DataPath {
            segments: Vec::new(),
            data: owned
                .prefix
                .iter()
                .map(String::as_str)
                .chain(std::iter::once(field_path.data.as_str()))
                .collect::<Vec<_>>()
                .join("."),
        };
        let lhs = definition
            .resolved_type_arc()
            .expect("BUG: row field is typed template data");
        if lhs.is_undetermined() {
            return;
        }
        let Some((target_type, kind_label)) =
            reference_target_type(graph, target, &display_path, owned.source, errors)
        else {
            return;
        };
        if let Some(msg) = reference_kind_mismatch_message(
            lhs,
            &target_type,
            &display_path,
            &match target {
                ReferenceTarget::Data(path) => path.to_string(),
                ReferenceTarget::Rule(rule) => rule.to_string(),
            },
            kind_label,
        ) {
            errors.push(reference_error(graph.main_spec, owned.source, msg));
        }
    });
}

/// A row written in a list, nested under the row named `holder`.
fn written_owned_row<'r>(row: &'r RowInstance, depth: u32, holder: &[String]) -> OwnedRow<'r> {
    let RowOrigin::Written { instance, source } = &row.origin else {
        unreachable!("BUG: stored rows are written rows");
    };
    let mut prefix = holder.to_vec();
    prefix.extend(instance.prefix.iter().map(PathSegment::input_key));
    OwnedRow {
        row,
        depth,
        prefix,
        source,
    }
}

/// Type of reference `target` of the plan being built, with its label for messages.
/// `None` after pushing an error, or when the target rule was dropped.
fn reference_target_type(
    graph: &StoredRowKindCheckGraph,
    target: &ReferenceTarget,
    display_path: &DataPath,
    source: &Source,
    errors: &mut Vec<Error>,
) -> Option<(Arc<LemmaType>, &'static str)> {
    let segments = match target {
        ReferenceTarget::Data(path) => &path.segments,
        ReferenceTarget::Rule(rule) => &rule.segments,
    };
    if !segments.is_empty() {
        let mut identity: Option<&SpecIdentity> = None;
        for segment in segments {
            let slot = DataPath::local(segment.uses.to_string());
            let data = match identity {
                None => graph.data,
                Some(identity) => &graph.lists.templates[identity].data,
            };
            let Some(DataDefinition::Import { row, .. }) = data.get(&slot) else {
                panic!("BUG: reference target hop '{slot}' of {target:?} is not a `uses` slot");
            };
            identity = Some(&row.identity);
        }
        let template = &graph.lists.templates[identity.expect("BUG: a member target has a hop")];
        return Some(match target {
            ReferenceTarget::Data(path) => (
                instance_list::row_data_type(&template.data, &DataPath::local(path.data.clone())),
                "target",
            ),
            ReferenceTarget::Rule(rule) => (
                instance_list::row_end_type(
                    template,
                    &RowEnd::Rule(RulePath::new(Vec::new(), rule.rule.clone())),
                ),
                "target rule",
            ),
        });
    }
    match target {
        ReferenceTarget::Data(target_path) => {
            let Some(entry) = graph.data.get(target_path) else {
                errors.push(reference_error(
                    graph.main_spec,
                    source,
                    format!(
                        "Data reference '{}' target '{}' does not exist",
                        display_path, target_path
                    ),
                ));
                return None;
            };
            match entry {
                DataDefinition::TypeDeclaration { resolved_type, .. }
                | DataDefinition::Reference { resolved_type, .. }
                | DataDefinition::InstanceList { resolved_type, .. }
                | DataDefinition::Value { resolved_type, .. } => {
                    Some((Arc::clone(resolved_type), "target"))
                }
                DataDefinition::Import { .. } => {
                    errors.push(reference_error(
                        graph.main_spec,
                        source,
                        format!(
                            "Data reference '{}' target '{}' is a spec reference and cannot carry a value",
                            display_path, target_path
                        ),
                    ));
                    None
                }
            }
        }
        ReferenceTarget::Rule(target_rule) => {
            if let Some(target_type) = graph.computed_rule_types.get(target_rule) {
                return Some((Arc::clone(target_type), "target rule"));
            }
            if graph.rules.contains_key(target_rule) {
                panic!("BUG: rule '{target_rule}' type not inferred before row reference check")
            }
            if !graph.dropped_rules.contains(target_rule) {
                errors.push(reference_error(
                    graph.main_spec,
                    source,
                    format!(
                        "Data reference '{}' target rule '{}' does not exist",
                        display_path, target_rule
                    ),
                ));
            }
            None
        }
    }
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
        limits: &'a ResourceLimits,
        interners: &mut instance_list::PassInterners,
    ) -> Result<(Graph<'a>, ResolvedSpecTypes), Vec<Error>> {
        let boundary_errors = validate_ast_source_locations(ordered_dependencies);
        if !boundary_errors.is_empty() {
            return Err(boundary_errors);
        }

        let mut errors: Vec<Error> = Vec::new();

        let mut slices = HashMap::new();
        for node in ordered_dependencies {
            match interners
                .slices
                .of(context, &node.repository, node.spec, limits)
            {
                Ok(version_slices) => {
                    slices.insert(
                        SpecVersionKey::of(&node.repository, node.spec),
                        version_slices,
                    );
                }
                Err(slice_errors) => errors.extend(slice_errors),
            }
        }
        if !errors.is_empty() {
            return Err(errors);
        }

        let mut type_resolver = TypeResolver::new(context, slices);
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
            data_reference_order: Vec::new(),
            reference_ends: IndexMap::new(),
            rule_candidates: HashMap::new(),
            main_spec,
            instance_lists,
            show_imports: ShowImports::default(),
            show_edges: BTreeMap::new(),
            show_order: Vec::new(),
        };

        let validation_errors = match graph.validate(&local_types, limits) {
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

    fn validate(
        &mut self,
        resolved_types: &ResolvedTypesMap,
        limits: &ResourceLimits,
    ) -> Result<(), Vec<Error>> {
        let mut errors = Vec::new();

        // Structural checks (no type info needed)
        if let Err(structural_errors) = check_all_rule_references_exist(self) {
            errors.extend(structural_errors);
        }
        if let Err(depth_errors) = instance_list::check_instance_list_depths(
            &self.instance_lists,
            &self.data,
            limits.max_list_depth,
        ) {
            errors.extend(depth_errors);
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
        if let Err(circular_errors) = self.check_binding_reference_cycles() {
            errors.extend(circular_errors);
            return Err(errors);
        }

        // Phase 1: Resolve data-target reference types now that all data
        // definitions (across all specs) are populated. Rule-target references
        // are resolved in Phase 4 once the target rule's type is inferred.
        if let Err(reference_errors) =
            self.resolve_data_reference_types(&reference_order, resolved_types)
        {
            errors.extend(reference_errors);
        }

        // Resolve every Reference chain end once. Cycles were just rejected.
        // Used for rule-dep edges, normalize embeds, show promptability, eval guard.
        self.reference_ends = self.compute_reference_ends();

        // Phase 2: Inject rule-rule dependency edges for rule-target references.
        // A rule R that reads a data path D where D is `Reference(target: rule T)`
        // must be evaluated AFTER T so the lazy resolver can read T's result.
        // This must happen before topological_sort so cycles through reference
        // paths are detected. List row sources contribute the same edges.
        self.add_rule_reference_dependency_edges();
        self.add_options_rule_dependency_edges();
        self.add_dynamic_import_pin_dependency_edges();

        match self.link_spec_values() {
            Ok(member_errors) => errors.extend(member_errors),
            Err(cycle_errors) => {
                errors.extend(cycle_errors);
                return Err(errors);
            }
        }

        // Phase 2b: Inject rule-rule dependency edges for rules a stored list
        // row forces at evaluation (nested references at materialization,
        // deferred field references at row-read). Needs `link_spec_values`'s
        // resolved SpecMember targets; must precede the final sort so cycles
        // through stored rows are reported here, not hit at evaluation.
        self.add_stored_row_dependency_edges();

        if let Err(circular_errors) = self.sort_show_rules() {
            errors.extend(circular_errors);
            return Err(errors);
        }

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

        let has_rule_references = self.data.values().any(|def| {
            matches!(
                def,
                DataDefinition::Reference {
                    target: ReferenceTarget::Rule(_),
                    ..
                }
            )
        });
        let mut infer_cache = InferExpressionTypeCache::new();
        let (inferred_types, seed_errors) =
            infer_rule_types(self, &rule_order, resolved_types, &mut infer_cache);
        let rule_reference_errors =
            self.resolve_rule_reference_types(&inferred_types, resolved_types);
        let row_store_reference_errors = self.check_row_store_reference_types(&inferred_types);
        if has_rule_references {
            infer_cache.clear();
        }
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
        if let Err(errs) = row_store_reference_errors {
            errors.extend(errs);
        }
        if let Err(errs) = type_errors {
            errors.extend(errs);
        }
        errors.extend(deferred_signature_errors(
            self.main_spec,
            resolved_types,
            &inferred_types,
        ));

        // Type resolve failed for this spec: GraphBuilder already recorded those errors
        // and left it out of `resolved_types`. Scale checks and planning have nothing to attach.
        if find_types_by_spec(resolved_types, self.main_spec).is_none() {
            if errors.is_empty() {
                return Ok(());
            }
            return Err(errors);
        }

        errors.extend(self.check_unit_scale_target_types(resolved_types, &inferred_types));

        if !errors.is_empty() {
            return Err(errors);
        }

        // Phase 5: Scale dependencies need every node's type, so their edges join the
        // graph only now; the second sort orders scale rules first and reports cycles.
        // Show must be rebuilt afterward: `reached_imports` seeds from `scale_targets`,
        // which are empty until `plan_unit_scales` fills them.
        let scale_errors = self.plan_unit_scales(resolved_types, &inferred_types, &mut infer_cache);
        if !scale_errors.is_empty() {
            return Err(scale_errors);
        }
        self.sort_show_rules()?;
        let rule_order = self.topological_sort()?;

        errors.extend(self.validate_dynamic_import_rule_pins(&inferred_types));
        if !errors.is_empty() {
            return Err(errors);
        }

        // Phase 6: Apply (only on full success)
        apply_inferred_types(self, inferred_types);
        self.rule_order = rule_order;
        self.data_reference_order = reference_order;
        Ok(())
    }

    /// After rule types are inferred, `-> effective <rule>` pins must be date/datetime.
    fn validate_dynamic_import_rule_pins(
        &self,
        inferred_types: &HashMap<RulePath, Arc<LemmaType>>,
    ) -> Vec<Error> {
        let mut errors = Vec::new();
        for definition in self.data.values() {
            let DataDefinition::Import {
                pin: Some(ImportEffectivePin::Rule(path)),
                source,
                ..
            } = definition
            else {
                continue;
            };
            let Some(lemma_type) = inferred_types.get(path) else {
                errors.push(Error::validation_with_context(
                    format!(
                        "`-> effective {}` does not name a rule of this spec",
                        path.rule
                    ),
                    Some(source.clone()),
                    None::<String>,
                    Some(self.main_spec),
                    None,
                ));
                continue;
            };
            if !lemma_type.is_date() {
                errors.push(Error::validation_with_context(
                    format!(
                        "`-> effective` requires a date or datetime, got {}",
                        lemma_type.specifications
                    ),
                    Some(source.clone()),
                    None::<String>,
                    Some(self.main_spec),
                    None,
                ));
            }
        }
        errors
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
        ParentType::Custom { name, .. } => Some(name.as_str()),
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
    let DataDefinition::Import { row, .. } = existing else {
        unreachable!("BUG: uses vs data clash but no Import side with recoverable spec_ref");
    };
    std::borrow::Cow::Owned(ast::SpecRef::same_repository(row.identity.spec.clone()))
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
        limits: &'a ResourceLimits,
        interners: &'i mut instance_list::PassInterners,
    ) -> Result<GraphBuildOk<'a>, Vec<Error>> {
        let mut builder = GraphBuilder {
            data: IndexMap::new(),
            rules: BTreeMap::new(),
            dropped_rules: BTreeSet::new(),
            context,
            local_types: Vec::new(),
            errors: Vec::new(),
            nested_errors: Vec::new(),
            nested_spec_failed: false,
            main_spec,
            limits,
            instance_lists: InstanceLists::new(SpecIdentity::of(
                repository,
                main_spec,
                effective.clone(),
            )),
            interners,
            self_list_row_rules: BTreeSet::new(),
        };

        builder.build_spec(main_spec, repository, effective, type_resolver);

        if builder.nested_spec_failed {
            return Err(builder.nested_errors);
        }

        builder.mark_self_list_row_rules_recursive();

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

    /// Unit tokens in a row-level list argument resolve against the row spec's unit
    /// index first (the expression is per-row), then fall back to the enclosing spec.
    fn resolve_unit_ref_in_scopes(
        &self,
        current_spec: &LemmaSpec,
        unit_ref: &str,
        row_scopes: &[RowScope],
    ) -> Result<(String, Arc<LemmaType>), String> {
        for scope in row_scopes.iter().rev() {
            let RowScope::Instance { template, .. } = scope else {
                continue;
            };
            match template {
                RowTemplate::Plan(plan) => {
                    if let Ok(resolved) = plan
                        .resolved_types
                        .unit_index
                        .resolve_with_named_types(unit_ref, &plan.resolved_types.resolved)
                    {
                        return Ok(resolved);
                    }
                }
                RowTemplate::Root => {
                    return self.resolve_unit_ref(current_spec, unit_ref);
                }
            }
        }
        self.resolve_unit_ref(current_spec, unit_ref)
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
        self.measure_ratio_range_from_number_with_unit_endpoints_in_scopes(
            current_spec,
            left_mag,
            left_unit,
            right_mag,
            right_unit,
            &[],
        )
    }

    fn measure_ratio_range_from_number_with_unit_endpoints_in_scopes(
        &self,
        current_spec: &LemmaSpec,
        left_mag: rust_decimal::Decimal,
        left_unit: &str,
        right_mag: rust_decimal::Decimal,
        right_unit: &str,
        row_scopes: &[RowScope],
    ) -> Result<ValueKind, String> {
        let (left_bare, left_lt) =
            self.resolve_unit_ref_in_scopes(current_spec, left_unit, row_scopes)?;
        let (right_bare, right_lt) =
            self.resolve_unit_ref_in_scopes(current_spec, right_unit, row_scopes)?;
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
        let did_you_mean = closest_name(field_name, &local_data_field_names(walk_spec));
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
        containing_repository: &Arc<LemmaRepository>,
        effective: &EffectiveDate,
    ) -> Option<ReferenceTarget> {
        if reference.segments.is_empty() {
            let data = containing_spec
                .data
                .iter()
                .find(|d| d.reference.is_local() && d.reference.name == reference.name);
            let is_rule = containing_spec
                .rules
                .iter()
                .any(|r| r.name == reference.name);
            if data.is_some() && is_rule {
                self.errors.push(self.engine_error(
                    format!(
                        "Reference target '{}' is ambiguous: both a data and a rule in spec '{}'",
                        reference.name, containing_spec.name
                    ),
                    reference_source,
                ));
                return None;
            }
            if let Some(data) = data {
                if matches!(data.value, ParsedDataValue::Import { .. }) {
                    self.errors.push(self.engine_error(
                        format!(
                            "cannot supply spec instance '{}' as a value",
                            reference.name
                        ),
                        reference_source,
                    ));
                    return None;
                }
                return Some(ReferenceTarget::Data(DataPath::local(
                    reference.name.clone(),
                )));
            }
            if is_rule {
                return Some(ReferenceTarget::Rule(RulePath::new(
                    Vec::new(),
                    reference.name.clone(),
                )));
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
            Vec::new(),
            containing_spec,
            containing_repository,
            effective,
        )?;

        let is_data = target_spec
            .data
            .iter()
            .any(|d| d.reference.is_local() && d.reference.name == reference.name);
        let is_rule = target_spec.rules.iter().any(|r| r.name == reference.name);

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
    /// Keys are relative to `spec`: `uses kind -> with salary: 1` is `["kind", "salary"]`.
    fn build_data_bindings(
        &mut self,
        spec: &'a LemmaSpec,
        spec_repository: &Arc<LemmaRepository>,
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
                let mut walk_repository = Arc::clone(spec_repository);
                let mut walk_effective = effective.clone();
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

                    match &seg_data.value {
                        ParsedDataValue::Import {
                            spec_ref,
                            effective_ref: Some(_),
                            ..
                        } => {
                            let targets = match self.dynamic_import_targets(
                                spec_ref,
                                walk_spec,
                                &walk_repository,
                                &binding.source_location,
                            ) {
                                Ok(targets) => targets,
                                Err(error) => {
                                    self.errors.push(error);
                                    binding_resolved = false;
                                    break;
                                }
                            };
                            let last_hop = binding_reference.segments.last() == Some(segment);
                            if last_hop {
                                let field = binding_reference.name.as_str();
                                let mut missing_on = None;
                                for target in &targets {
                                    if !target.spec.data.iter().any(|row| {
                                        row.reference.segments.is_empty()
                                            && row.reference.name == field
                                    }) {
                                        missing_on = Some(target.spec);
                                        break;
                                    }
                                }
                                if let Some(candidate) = missing_on {
                                    self.push_missing_with_data_field_error(
                                        import_row, binding, candidate, field,
                                    );
                                    binding_resolved = false;
                                    break;
                                }
                            }
                            let first = targets
                                .into_iter()
                                .next()
                                .expect("BUG: dynamic_import_targets returns at least one target");
                            walk_effective = first.at;
                            walk_repository = first.repository;
                            walk_spec = first.spec;
                        }
                        ParsedDataValue::Import { spec_ref, .. } => {
                            walk_spec = match self.resolve_spec_ref(
                                spec_ref,
                                &walk_effective,
                                walk_spec,
                                &walk_repository,
                            ) {
                                Ok((repository, arc)) => {
                                    walk_effective = spec_ref.at(&walk_effective);
                                    walk_repository = repository;
                                    arc
                                }
                                Err(error) => {
                                    self.errors.push(error);
                                    binding_resolved = false;
                                    break;
                                }
                            };
                        }
                        ParsedDataValue::Definition { .. }
                            if self.is_data_instance_list(
                                seg_data,
                                walk_spec,
                                &walk_repository,
                                &walk_effective,
                            ) =>
                        {
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
                    }
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

                let target_is_instance_list = walk_spec.data.iter().any(|field| {
                    field.reference.segments.is_empty()
                        && field.reference.name == binding_reference.name
                        && self.is_data_instance_list(
                            field,
                            walk_spec,
                            &walk_repository,
                            &walk_effective,
                        )
                });

                let mut binding_key: Vec<String> = binding_reference.segments.clone();
                binding_key.push(binding_reference.name.clone());

                let binding_value = match &binding.rhs {
                    ast::UsesBindingRhs::Literal(value) => {
                        if target_is_instance_list {
                            self.errors.push(Error::validation_with_context(
                                format!(
                                    "binding `{path}` replaces instance list `{name}` with a literal; list slots take `uses` aliases",
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
                        BindingValue::Literal(value.clone())
                    }
                    ast::UsesBindingRhs::References { targets } if target_is_instance_list => {
                        BindingValue::InstanceAliases(targets.clone())
                    }
                    ast::UsesBindingRhs::References { targets } => {
                        let [target] = targets.as_slice() else {
                            self.errors.push(Error::validation_with_context(
                                format!(
                                    "binding `{binding_reference}` targets a scalar slot but supplies {} references",
                                    targets.len()
                                ),
                                Some(binding.source_location.clone()),
                                None::<String>,
                                Some(self.main_spec),
                                None,
                            ));
                            continue;
                        };
                        let Some(resolved_target) = self.resolve_reference_target_in_spec(
                            target,
                            &binding.source_location,
                            spec,
                            spec_repository,
                            effective,
                        ) else {
                            continue;
                        };
                        BindingValue::Reference {
                            target: resolved_target,
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

    /// `data x: row_spec list` whose element names a loadable Spec (not a value-list type).
    fn is_data_instance_list(
        &self,
        data: &LemmaData,
        current_spec: &LemmaSpec,
        current_repository: &Arc<LemmaRepository>,
        effective: &EffectiveDate,
    ) -> bool {
        let Some(spec_ref) = discovery::data_instance_list_spec_ref(current_spec, data) else {
            return false;
        };
        discovery::resolve_spec_ref_or_self(
            self.context,
            &spec_ref,
            current_repository,
            current_spec,
            effective,
            None,
        )
        .is_ok()
    }

    /// Reports and returns true when `data_path` is already taken in this spec.
    fn is_duplicate_data(
        &mut self,
        data: &LemmaData,
        data_path: &DataPath,
        current_spec: &LemmaSpec,
    ) -> bool {
        let Some(existing) = self.data.get(data_path) else {
            return false;
        };
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
        true
    }

    /// Add a single scalar data or import marker to the graph. Instance lists are
    /// opened by the spec frame ([`Self::open_instance_list`]).
    fn add_data(
        &mut self,
        data: &LemmaData,
        current_spec: &'a LemmaSpec,
        current_repository: &Arc<LemmaRepository>,
        effective: &EffectiveDate,
    ) {
        let data_path = DataPath::local(data.reference.name.clone());
        if self.is_duplicate_data(data, &data_path, current_spec) {
            return;
        }

        if let ParsedDataValue::Definition {
            base: Some(ParentType::List { .. }),
            list,
            ..
        } = &data.value
        {
            let is_instance =
                self.is_data_instance_list(data, current_spec, current_repository, effective);
            if list.is_some() && !is_instance {
                self.errors.push(Error::validation_with_context(
                    "value lists are numbered; `-> key` is for instance lists".to_string(),
                    Some(data.source_location.clone()),
                    None::<String>,
                    Some(self.main_spec),
                    None,
                ));
                return;
            }
        }

        let (original_schema_type, original_declared_suggestion, original_declared_value) = if matches!(
            &data.value,
            ParsedDataValue::Definition { .. }
        )
            && data.value.definition_needs_type_resolution()
            && !self.is_data_instance_list(data, current_spec, current_repository, effective)
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
            ParsedDataValue::Definition { .. }
                if self.is_data_instance_list(
                    data,
                    current_spec,
                    current_repository,
                    effective,
                ) =>
            {
                unreachable!("BUG: instance list '{data_path}' is opened by its spec frame")
            }
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
                    DataOrigin::Declared,
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

                if resolved_type.specifications.is_unresolved_measure_range() {
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
            ParsedDataValue::ImportList { .. } => {
                panic!("BUG: uses list is rejected at parse and must not reach planning");
            }
            ParsedDataValue::Import { .. } => {
                unreachable!("BUG: `uses {data_path}` is added as an import row")
            }
        }
    }

    /// `data x: spec list`: validates the slot, resolves its row spec and key, builds
    /// the row store, checks row keys and installs the slot. Rows are data checked
    /// against the row spec's template plan; nothing nests under a list.
    fn plan_instance_list(
        &mut self,
        data: &'a LemmaData,
        current_spec: &'a LemmaSpec,
        current_repository: &Arc<LemmaRepository>,
        effective: &EffectiveDate,
        bindings: &DataBindings,
    ) {
        let data_path = DataPath::local(data.reference.name.clone());
        if self.is_duplicate_data(data, &data_path, current_spec) {
            return;
        }
        let Some(spec_ref) = discovery::data_instance_list_spec_ref(current_spec, data) else {
            panic!("BUG: plan_instance_list on non-instance-list data '{data_path}'");
        };
        let list = match &data.value {
            ParsedDataValue::Definition {
                list: Some(list), ..
            } => list,
            ParsedDataValue::Definition { list: None, .. } => &EMPTY_INSTANCE_LIST,
            _ => panic!("BUG: plan_instance_list requires Definition on '{data_path}'"),
        };
        let source = data.source_location.clone();
        let (row_repository, row_spec) = match discovery::resolve_spec_ref_or_self(
            self.context,
            &spec_ref,
            current_repository,
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
        let at = spec_ref.at(effective);
        let Some(identity) = self.identity_at(&row_repository, row_spec, &at) else {
            return;
        };

        if discovery::same_loaded_spec(row_spec, current_spec) && !list.rows.is_empty() {
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

        let template = if identity == self.instance_lists.self_identity {
            assert!(
                discovery::same_loaded_spec(row_spec, self.main_spec),
                "BUG: self-list row identity equals the planned spec's identity but is another spec"
            );
            None
        } else {
            match self.row_template(&identity, &row_repository, row_spec, &at) {
                Some(template) => Some(template),
                None => return,
            }
        };
        let key = match &template {
            Some(template) => self.instance_list_key(list, &template.data, &identity),
            None => self.instance_list_key(list, &self.data, &identity),
        };
        let key = match key {
            Ok(key) => key,
            Err(errors) => {
                self.errors.extend(errors);
                return;
            }
        };

        let store = match &template {
            Some(template) => {
                let row_identity = Arc::new(identity.clone());
                let rows = self.written_rows(
                    &ListSite {
                        list_path: &data_path,
                        identity: &row_identity,
                        template,
                        spec: current_spec,
                        repository: current_repository,
                        effective,
                        bindings,
                    },
                    list,
                );
                self.row_store(rows, &key, &source, template)
            }
            None => Arc::new(RowStore::unkeyed(Vec::new())),
        };

        let element = Arc::new(LemmaType::primitive(TypeSpecification::Spec {
            spec: identity.clone(),
        }));
        let mut resolved_type = LemmaType {
            name: Some(format!("{} list", identity.spec)),
            specifications: TypeSpecification::List {
                element,
                key: key.clone(),
                help: String::new(),
            },
            extends: TypeExtends::Primitive,
            options: TypeOptions::default(),
        };
        if let ParsedDataValue::Definition {
            constraints: Some(constraints),
            ..
        } = &data.value
        {
            let mut declared_suggestion = None;
            let mut declared_value = None;
            match apply_constraints_to_spec(
                current_spec,
                &data_path.data,
                &data_path.data,
                &mut resolved_type,
                constraints,
                &source,
                ConstraintSuggestionSink {
                    suggestion: &mut declared_suggestion,
                    value: &mut declared_value,
                    resolve_unit: None,
                },
            ) {
                Ok(()) => {
                    if let Some(raw) = declared_value {
                        self.errors.extend(instance_list_option_errors(
                            &resolved_type,
                            &raw,
                            &source,
                            current_spec,
                        ));
                    }
                }
                Err(errors) => self.errors.extend(errors),
            }
        }
        self.reject_spec_option_rows(list);
        let resolved_type = Arc::new(resolved_type);
        self.data.insert(
            data_path.clone(),
            DataDefinition::InstanceList {
                resolved_type,
                origin: DataOrigin::Declared,
                source,
            },
        );
        let source = InstanceListSource::new(identity, key, store);
        self.instance_lists.sources.insert(data_path, source);
    }

    fn reject_spec_option_rows(&mut self, list: &ast::InstanceListBody) {
        for row in &list.option_rows {
            let ast::InstanceListRow::Fields {
                source_location, ..
            } = row
            else {
                unreachable!("BUG: -> option rows are field rows");
            };
            self.errors
                .push(self.engine_error(SPEC_LIST_HAS_NO_OPTIONS.to_string(), source_location));
        }
    }

    /// Rows of `identity` as instance-list commands read them.
    ///
    /// `None` when the row Spec is not self and was never imported (`uses`), so no
    /// template plan exists. Callers must report a planning Error — never panicking.
    fn row_template_of(&self, identity: &SpecIdentity) -> Option<RowTemplate> {
        self.instance_lists.row_template_of(identity)
    }

    fn missing_row_template_error(&self, identity: &SpecIdentity, source: &Source) -> Error {
        self.engine_error(
            format!(
                "instance list of `{}` needs `uses {}` before `-> key` or `-> with`",
                identity.spec, identity.spec
            ),
            source,
        )
    }

    /// Top-level main-spec rules (forward refs while converting) then present
    /// graph rules.
    fn conversion_root_rules(&self) -> BTreeSet<RulePath> {
        let mut rules = BTreeSet::new();
        for rule in &self.main_spec.rules {
            rules.insert(RulePath {
                segments: Vec::new(),
                rule: rule.name.clone(),
            });
        }
        for path in self.rules.keys() {
            rules.insert(path.clone());
        }
        rules
    }

    /// Member `parts` on rows of `identity` read through `template`, hop by hop.
    fn template_row_chain(
        &self,
        identity: &SpecIdentity,
        template: &RowTemplate,
        parts: &[&str],
    ) -> Option<Vec<RowHop>> {
        match template {
            RowTemplate::Plan(plan) => instance_list::row_end_chain(
                identity,
                instance_list::RowEndView::Plan(plan),
                &self.instance_lists.templates,
                parts,
            ),
            RowTemplate::Root => {
                let rules = self.conversion_root_rules();
                instance_list::row_end_chain(
                    identity,
                    instance_list::RowEndView::Root {
                        data: &self.data,
                        rules: &rules,
                    },
                    &self.instance_lists.templates,
                    parts,
                )
            }
        }
    }

    /// Identity of `spec`, resolved at `at`, in the slice that holds `at`.
    fn identity_at(
        &mut self,
        repository: &Arc<LemmaRepository>,
        spec: &LemmaSpec,
        at: &EffectiveDate,
    ) -> Option<SpecIdentity> {
        match self
            .interners
            .slices
            .identity_at(self.context, repository, spec, at, self.limits)
        {
            Ok(identity) => Some(identity),
            Err(errors) => {
                self.nested_errors.extend(errors);
                self.nested_spec_failed = true;
                None
            }
        }
    }

    /// Template plan of `identity`, shared pass-wide and recorded on this consumer.
    ///
    /// `at` is the planning instant for this edge (`spec_ref.at(consumer_effective)`);
    /// `identity` is the slice that holds it ([`Self::identity_at`]).
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
        if let Some(template) = self.interners.spec_templates.get(identity) {
            self.record_template(identity.clone(), Arc::clone(&template));
            return Some(template);
        }
        match super::get_or_build_template(
            self.context,
            row_repository,
            row_spec,
            at,
            self.limits,
            self.interners,
        ) {
            Ok(template) => {
                assert_eq!(
                    template.identity, *identity,
                    "BUG: the template built at an instant is the plan of the slice that holds it"
                );
                self.record_template(identity.clone(), Arc::clone(&template));
                Some(template)
            }
            Err(errors) => {
                self.nested_errors.extend(errors);
                self.nested_spec_failed = true;
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

    /// `-> key a, b`: names and row-relative data paths of the key fields. `row_data` is
    /// the row spec's data: a template plan's, or this plan's for a self-list.
    fn instance_list_key(
        &self,
        list: &ast::InstanceListBody,
        row_data: &IndexMap<DataPath, DataDefinition>,
        identity: &SpecIdentity,
    ) -> Result<ListKey, Vec<Error>> {
        let Some(declared) = &list.key else {
            return Ok(ListKey::Numbered);
        };
        let mut key_fields: Vec<KeyField> = Vec::with_capacity(declared.fields.len());
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
            if key_fields
                .iter()
                .any(|key_field| key_field.name == field.name)
            {
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
            key_fields.push(KeyField {
                name: field.name.clone(),
                path: local,
            });
        }
        if errors.is_empty() {
            Ok(ListKey::Fields(key_fields))
        } else {
            Err(errors)
        }
    }
    /// Rows written under `list`. A field row binds the written fields; `-> with ada`
    /// is written with every binding of `uses ada`.
    fn written_rows<'r>(
        &mut self,
        site: &ListSite<'_, 'a>,
        list: &'r ast::InstanceListBody,
    ) -> Vec<Arc<RowInstance>> {
        let mut written_fields: HashMap<&'r ast::Reference, WrittenField> = HashMap::new();
        let list_name: Arc<str> = Arc::from(site.list_path.data.as_str());
        let spec_name: Arc<str> = Arc::from(site.identity.spec.as_str());
        let repository: Option<Arc<str>> = site.identity.repository.as_deref().map(Arc::from);
        let row_prefix = |number: usize| {
            vec![PathSegment::list_row_shared(
                &list_name,
                number,
                &spec_name,
                repository.as_ref(),
            )]
        };
        let mut rows = Vec::new();
        let mut number = 0;
        for row in &list.rows {
            match row {
                ast::InstanceListRow::Fields {
                    bindings: fields,
                    source_location,
                } => {
                    number += 1;
                    if let Some(row) = self.field_stored_row(
                        site,
                        fields,
                        &mut written_fields,
                        source_location,
                        row_prefix(number),
                    ) {
                        rows.push(row);
                    }
                }
                ast::InstanceListRow::Values {
                    values,
                    source_location,
                } => {
                    for value in values {
                        number += 1;
                        if let Some(row) =
                            self.named_stored_row(site, value, source_location, row_prefix(number))
                        {
                            rows.push(row);
                        }
                    }
                }
            }
        }
        rows
    }

    /// What the written field `path` names on rows of `site`.
    fn resolve_written_field(
        &self,
        site: &ListSite<'_, 'a>,
        path: &ast::Reference,
    ) -> WrittenField {
        let parts: Vec<&str> = path
            .segments
            .iter()
            .map(String::as_str)
            .chain(std::iter::once(path.name.as_str()))
            .collect();
        let Some(chain) = instance_list::row_end_chain(
            site.identity,
            instance_list::RowEndView::Plan(site.template),
            &self.instance_lists.templates,
            &parts,
        ) else {
            return WrittenField::NotData;
        };
        let last = chain.last().expect("BUG: a row end chain has a last hop");
        let RowEnd::Data(data_path) = &last.end else {
            return WrittenField::NotData;
        };
        let template = if chain.len() == 1 {
            Arc::clone(site.template)
        } else {
            self.template_of(&last.identity)
        };
        match template
            .data
            .get(data_path)
            .expect("BUG: row end chain data is template data")
        {
            DataDefinition::Import { row, .. } => WrittenField::Import {
                identity: Arc::clone(&row.identity),
            },
            DataDefinition::InstanceList { .. } => WrittenField::List,
            DataDefinition::Value { .. }
            | DataDefinition::TypeDeclaration { .. }
            | DataDefinition::Reference { .. } => WrittenField::Value {
                key: parts.iter().map(|part| (*part).to_string()).collect(),
            },
        }
    }

    /// `-> with name: "Ada", salary: s`: a row whose written fields are bound. A
    /// dotted field (`item.price`) writes a field of the row spec's `uses item`.
    /// `written_fields` holds the fields earlier rows of the list resolved.
    fn field_stored_row<'r>(
        &mut self,
        site: &ListSite<'_, 'a>,
        fields: &'r [ast::RowBinding],
        written_fields: &mut HashMap<&'r ast::Reference, WrittenField>,
        source: &Source,
        prefix: Vec<PathSegment>,
    ) -> Option<Arc<RowInstance>> {
        let mut bindings: DataBindings = IndexMap::new();
        let mut failed = false;
        for binding in fields {
            let field = match written_fields.get(&binding.path) {
                Some(field) => field,
                None => {
                    let resolved = self.resolve_written_field(site, &binding.path);
                    written_fields.entry(&binding.path).or_insert(resolved)
                }
            };
            let key = match field {
                WrittenField::Value { key } => key,
                WrittenField::NotData => {
                    self.errors.push(self.engine_error(
                        format!(
                            "row field `{}` is not data on `{}`",
                            binding.path, site.identity.spec
                        ),
                        &binding.source_location,
                    ));
                    failed = true;
                    continue;
                }
                WrittenField::Import { identity } => {
                    let name = &binding.path;
                    self.errors.push(self.engine_error(
                        format!(
                            "row field `{name}` is a `uses` instance of `{}`; bind its fields with `{name}.<field>`",
                            identity.spec
                        ),
                        &binding.source_location,
                    ));
                    failed = true;
                    continue;
                }
                WrittenField::List => {
                    self.errors.push(self.engine_error(
                        format!(
                            "row field `{}` is a list on `{}`; a row binds values, not lists",
                            binding.path, site.identity.spec
                        ),
                        &binding.source_location,
                    ));
                    failed = true;
                    continue;
                }
            };
            if bindings.contains_key(key) {
                self.errors.push(self.engine_error(
                    format!("row field `{}` appears twice in one row", binding.path),
                    &binding.source_location,
                ));
                failed = true;
                continue;
            }
            let key = key.clone();
            let value = match &binding.rhs {
                WithRhs::Literal(value) => BindingValue::Literal(value.clone()),
                WithRhs::Reference { target } => match self.resolve_reference_target_in_spec(
                    target,
                    &binding.source_location,
                    site.spec,
                    site.repository,
                    site.effective,
                ) {
                    Some(target) => BindingValue::Reference { target },
                    None => {
                        failed = true;
                        continue;
                    }
                },
            };
            bindings.insert(key, (value, binding.source_location.clone()));
        }
        if failed {
            return None;
        }
        Some(self.binding_row(RowWrite {
            identity: Arc::clone(site.identity),
            template: Arc::clone(site.template),
            origin: RowOrigin::Written {
                instance: SpecInstance {
                    prefix: prefix.clone(),
                },
                source: source.clone(),
            },
            base: vec![None; site.template.data.len()],
            bindings,
            prefix,
            hops: 0,
        }))
    }

    /// `-> with ada`: a row written with every binding of `uses ada`.
    fn named_stored_row(
        &mut self,
        site: &ListSite<'_, 'a>,
        value: &WithRhs,
        source: &Source,
        prefix: Vec<PathSegment>,
    ) -> Option<Arc<RowInstance>> {
        let list = &site.list_path.data;
        let target = match value {
            WithRhs::Reference { target } if target.segments.is_empty() => target,
            WithRhs::Reference { target } => {
                self.errors.push(self.engine_error(
                    format!(
                        "row `{target}` of `{list}` must name a `uses` instance of `{}` declared in this spec",
                        site.identity.spec
                    ),
                    source,
                ));
                return None;
            }
            WithRhs::Literal(literal) => {
                self.errors.push(self.engine_error(
                    format!(
                        "row `{literal}` of `{list}` is not a `{}` instance; write `-> with <field>: <value>` for an inline row",
                        site.identity.spec
                    ),
                    source,
                ));
                return None;
            }
        };
        let instance_identity = match self.data.get(&DataPath::local(target.name.clone())) {
            Some(DataDefinition::Import { row, .. }) => Arc::clone(&row.identity),
            Some(_) | None => {
                self.errors.push(self.engine_error(
                    format!(
                        "row `{target}` of `{list}` is not a `uses` instance of `{}`",
                        site.identity.spec
                    ),
                    source,
                ));
                return None;
            }
        };
        if instance_identity != *site.identity {
            self.errors.push(self.engine_error(
                format!(
                    "`{}` is a `{}` instance; rows of `{list}` are `{}`",
                    target.name, instance_identity, site.identity
                ),
                source,
            ));
            return None;
        }
        Some(self.binding_row(RowWrite {
            identity: instance_identity,
            template: Arc::clone(site.template),
            origin: RowOrigin::Written {
                instance: SpecInstance {
                    prefix: prefix.clone(),
                },
                source: source.clone(),
            },
            base: vec![None; site.template.data.len()],
            bindings: bindings_under(site.bindings, &target.name),
            prefix,
            hops: 0,
        }))
    }

    /// Row spec identity and template plan of the list at `path` in `template`.
    fn nested_row_template(
        &self,
        template: &ExecutionPlan,
        path: &DataPath,
    ) -> (Arc<SpecIdentity>, Arc<ExecutionPlan>) {
        let source = template
            .instance_lists
            .get(path)
            .unwrap_or_else(|| panic!("BUG: list binding '{path}' is not a list on the row spec"));
        let plan = match self.row_template_of(&source.identity) {
            Some(RowTemplate::Plan(plan)) => plan,
            Some(RowTemplate::Root) => {
                panic!(
                    "BUG: copied rows of the root spec need an import cycle, rejected at discovery"
                )
            }
            None => panic!(
                "BUG: row template of '{}' was planned before its consumers",
                source.identity.spec
            ),
        };
        (Arc::new(source.identity.clone()), plan)
    }

    /// Store of `rows`. Key values of a keyed list are written values and unique;
    /// a list whose keys fail that has an error and an empty index.
    fn row_store(
        &mut self,
        rows: Vec<Arc<RowInstance>>,
        key: &ListKey,
        source: &Source,
        template: &ExecutionPlan,
    ) -> Arc<RowStore> {
        let Some(store_key) = self.row_store_key(&rows, key, source, template) else {
            return Arc::new(RowStore::unkeyed(rows));
        };
        match RowStore::keyed(rows, store_key) {
            Ok(store) => Arc::new(store),
            Err((message, unkeyed)) => {
                self.errors.push(self.engine_error(message, source));
                Arc::new(unkeyed)
            }
        }
    }

    /// Key fields of the keyed list `rows` are written in, when every row writes a value
    /// for each (or the template has a default). `None` for an unkeyed list, and with an
    /// error when a row does not write one.
    fn row_store_key(
        &mut self,
        rows: &[Arc<RowInstance>],
        key: &ListKey,
        source: &Source,
        template: &ExecutionPlan,
    ) -> Option<Vec<StoreKeyField>> {
        let ListKey::Fields(key_fields) = key else {
            return None;
        };
        let store_key: Vec<StoreKeyField> = key_fields
            .iter()
            .map(|key_field| StoreKeyField {
                name: key_field.name.clone(),
                slot: template
                    .data
                    .get_index_of(&key_field.path)
                    .expect("BUG: key field is template data"),
                field_type: instance_list::row_end_type(
                    template,
                    &RowEnd::Data(key_field.path.clone()),
                ),
                default: instance_list::default_of(&template.data, &key_field.path),
            })
            .collect();
        for (index, row) in rows.iter().enumerate() {
            for field in &store_key {
                let name = &field.name;
                match &row.bindings[field.slot] {
                    Some(RowBinding::Bound(OperationResult::Value(_))) => {}
                    Some(RowBinding::Bound(OperationResult::Veto(_))) => {
                        unreachable!("BUG: stored row literal binding is never a Veto")
                    }
                    Some(RowBinding::Rows { .. } | RowBinding::Instance(_)) => {
                        unreachable!("BUG: key fields are values, never lists or instances")
                    }
                    Some(RowBinding::Deferred { target, .. }) => {
                        let target = match target {
                            ReferenceTarget::Data(path) => path.input_key(),
                            ReferenceTarget::Rule(path) => path.input_key(),
                        };
                        self.errors.push(self.engine_error(
                            format!(
                                "key field `{name}` of row {} is `{target}`; key values must be written as values",
                                index + 1
                            ),
                            source,
                        ));
                        return None;
                    }
                    None if field.default.is_some() => {}
                    None => {
                        self.errors.push(self.engine_error(
                            format!("row {} has no value for key field `{name}`", index + 1),
                            source,
                        ));
                        return None;
                    }
                }
            }
        }
        Some(store_key)
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
        origin: DataOrigin,
    ) {
        let Some((semantic_value, lemma_type)) =
            self.literal_to_typed(value, declared_schema_type, &effective_source, current_spec)
        else {
            return;
        };
        let literal_value = LiteralValue {
            value: semantic_value,
        };
        self.data.insert(
            data_path,
            DataDefinition::Value {
                value: literal_value,
                resolved_type: lemma_type,
                origin,
                source: effective_source,
            },
        );
    }

    /// Converts an AST literal to its semantic value and resolved type, against
    /// `declared_schema_type` when the slot declares one. Errors are recorded; `None`
    /// means the literal did not convert.
    fn literal_to_typed(
        &mut self,
        value: &ast::Value,
        declared_schema_type: Option<Arc<LemmaType>>,
        effective_source: &Source,
        current_spec: &LemmaSpec,
    ) -> Option<(ValueKind, Arc<LemmaType>)> {
        let semantic_value = if let Some(ref schema) = declared_schema_type {
            match value {
                Value::Range(left, right)
                    if matches!(
                        &schema.specifications,
                        TypeSpecification::MeasureRange { .. }
                            | TypeSpecification::RatioRange { .. }
                    ) =>
                {
                    let element = schema.range_element_type();
                    let resolve = unit_tokens_for_spec(&self.local_types, current_spec);
                    let left_lit = match lift_range_endpoint(left, element.as_ref(), Some(&resolve))
                    {
                        Ok(lit) => lit,
                        Err(e) => {
                            self.errors.push(self.engine_error(e, effective_source));
                            return None;
                        }
                    };
                    let right_lit =
                        match lift_range_endpoint(right, element.as_ref(), Some(&resolve)) {
                            Ok(lit) => lit,
                            Err(e) => {
                                self.errors.push(self.engine_error(e, effective_source));
                                return None;
                            }
                        };
                    ValueKind::Range(Box::new(left_lit), Box::new(right_lit))
                }
                _ => {
                    let resolve = unit_tokens_for_spec(&self.local_types, current_spec);
                    match parser_value_to_value_kind(value, schema.as_ref(), Some(&resolve)) {
                        Ok(s) => s,
                        Err(e) => {
                            self.errors.push(self.engine_error(e, effective_source));
                            return None;
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
                                .push(self.engine_error(message, effective_source));
                            return None;
                        }
                    };
                    match number_with_unit_to_value_kind(*magnitude, &bare, lt.as_ref()) {
                        Ok(s) => s,
                        Err(e) => {
                            self.errors.push(self.engine_error(e, effective_source));
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
                            current_spec,
                            *left_mag,
                            left_unit,
                            *right_mag,
                            right_unit,
                        ) {
                            Ok(s) => s,
                            Err(e) => {
                                self.errors.push(self.engine_error(e, effective_source));
                                return None;
                            }
                        }
                    }
                    _ => match value_to_semantic(value) {
                        Ok(s) => s,
                        Err(e) => {
                            self.errors.push(self.engine_error(e, effective_source));
                            return None;
                        }
                    },
                },
                _ => match value_to_semantic(value) {
                    Ok(s) => s,
                    Err(e) => {
                        self.errors.push(self.engine_error(e, effective_source));
                        return None;
                    }
                },
            }
        };
        let schema_type: Arc<LemmaType> = match declared_schema_type {
            Some(schema) => schema,
            None => match value {
                Value::Text(_) => primitive_text_arc().clone(),
                Value::Number(_) => primitive_number_arc().clone(),
                Value::NumberWithUnit(_, unit) => match self.resolve_unit_ref(current_spec, unit) {
                    Ok((_, lt)) => lt,
                    Err(message) => {
                        self.errors
                            .push(self.engine_error(message, effective_source));
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
            },
        };
        let lemma_type = match (value, &semantic_value) {
            (Value::NumberWithUnit(_, unit), ValueKind::Measure(_) | ValueKind::Ratio(_)) => {
                let bare = match self.resolve_unit_ref(current_spec, unit) {
                    Ok((bare, _)) => bare,
                    Err(message) => {
                        self.errors
                            .push(self.engine_error(message, effective_source));
                        return None;
                    }
                };
                Arc::new(schema_type.as_ref().clone().with_unit(bare))
            }
            _ => schema_type,
        };
        Some((semantic_value, lemma_type))
    }

    /// Returns (path_segments, last_resolved_spec) on success.
    fn resolve_path_segments(
        &mut self,
        segments: &[String],
        reference_source: &Source,
        mut path_segments: Vec<PathSegment>,
        mut spec_context: &'a LemmaSpec,
        spec_context_repository: &Arc<LemmaRepository>,
        effective: &EffectiveDate,
    ) -> Option<(Vec<PathSegment>, &'a LemmaSpec)> {
        let mut last_arc: Option<&LemmaSpec> = None;
        let mut walk_effective = effective.clone();
        let mut context_repository = Arc::clone(spec_context_repository);

        for segment in segments.iter() {
            let Some(data_ref) = spec_context
                .data
                .iter()
                .find(|data| data.reference.is_local() && data.reference.name == *segment)
            else {
                self.errors.push(
                    self.engine_error(format!("Data '{}' not found", segment), reference_source),
                );
                return None;
            };

            if let ParsedDataValue::Import {
                spec_ref,
                effective_ref: Some(_),
                ..
            } = &data_ref.value
            {
                // Dynamic pin: path segments name the dependency, not a fixed version.
                // Interface validation already requires every candidate share members;
                // walk the first candidate for typing.
                let targets = match self.dynamic_import_targets(
                    spec_ref,
                    spec_context,
                    &context_repository,
                    reference_source,
                ) {
                    Ok(targets) => targets,
                    Err(e) => {
                        self.errors.push(e);
                        return None;
                    }
                };
                let first = targets
                    .into_iter()
                    .next()
                    .expect("BUG: dynamic_import_targets returns at least one target");
                walk_effective = first.at;
                path_segments.push(PathSegment {
                    uses: Arc::from(segment.as_str()),
                    row: None,
                    repository: first.repository.name.as_deref().map(Arc::from),
                    spec: Arc::from(first.spec.name.as_str()),
                });
                last_arc = Some(first.spec);
                spec_context = first.spec;
                context_repository = first.repository;
            } else if let ParsedDataValue::Import { spec_ref, .. } = &data_ref.value {
                let (repo, arc) = match self.resolve_spec_ref(
                    spec_ref,
                    &walk_effective,
                    spec_context,
                    &context_repository,
                ) {
                    Ok(pair) => pair,
                    Err(e) => {
                        self.errors.push(e);
                        return None;
                    }
                };
                walk_effective = spec_ref.at(&walk_effective);
                path_segments.push(PathSegment {
                    uses: Arc::from(segment.as_str()),
                    row: None,
                    repository: repo.name.as_deref().map(Arc::from),
                    spec: Arc::from(arc.name.as_str()),
                });
                last_arc = Some(arc);
                spec_context = arc;
                context_repository = repo;
            } else if matches!(
                &data_ref.value,
                ParsedDataValue::Definition { list: Some(_), .. }
            ) {
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
            pending_targets: VecDeque::new(),
        })
    }

    /// Next import of `frame`'s spec that must be entered. Import errors are recorded and
    /// skipped.
    fn next_spec_import(
        &mut self,
        frame: &mut SpecTypesFrame<'a>,
    ) -> Option<(&'a LemmaSpec, Arc<LemmaRepository>, EffectiveDate)> {
        if let Some(dependency) = frame.pending_targets.pop_front() {
            return Some((dependency.spec, dependency.repository, dependency.at));
        }
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
                ParsedDataValue::ImportList { .. } => {
                    panic!("BUG: uses list is rejected at parse and must not reach planning");
                }
                ParsedDataValue::Import {
                    spec_ref,
                    effective_ref: Some(_),
                    ..
                } => {
                    match self.dynamic_import_targets(
                        spec_ref,
                        spec,
                        &frame.repository,
                        &data.source_location,
                    ) {
                        Ok(targets) => {
                            frame.pending_targets.extend(targets);
                            if let Some(dependency) = frame.pending_targets.pop_front() {
                                return Some((
                                    dependency.spec,
                                    dependency.repository,
                                    dependency.at,
                                ));
                            }
                        }
                        Err(error) => self.errors.push(error),
                    }
                }
                ParsedDataValue::Import { spec_ref, .. } => {
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
                ParsedDataValue::Definition { .. } => {
                    let Some(spec_ref) = discovery::data_instance_list_spec_ref(spec, data) else {
                        continue;
                    };
                    let import_effective = spec_ref.at(&frame.effective);
                    match self.resolve_spec_ref(
                        &spec_ref,
                        &import_effective,
                        spec,
                        &frame.repository,
                    ) {
                        Ok((source_repo, source_arc)) => {
                            return Some((source_arc, source_repo, import_effective));
                        }
                        Err(error) => {
                            // Instance-list body (`Definition.list`) names a missing Spec:
                            // surface the resolve Error. Local-type value lists (`money list`
                            // with no instance body) fall through without Error here.
                            if matches!(
                                &data.value,
                                ParsedDataValue::Definition { list: Some(_), .. }
                            ) {
                                self.errors.push(error);
                            }
                        }
                    }
                }
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

    /// Plan `spec` as this graph: its data, its `uses` imports, its lists' rows and its
    /// rules. An import is a row over its spec's template plan, planned on its own;
    /// nothing of the imported spec is planned here.
    fn build_spec(
        &mut self,
        spec: &'a LemmaSpec,
        spec_repository: &Arc<LemmaRepository>,
        effective: &EffectiveDate,
        type_resolver: &TypeResolver<'a>,
    ) {
        self.process_meta_fields(spec);
        let bindings = match self.build_data_bindings(spec, spec_repository, effective) {
            Ok(bindings) => bindings,
            Err(errors) => {
                self.errors.extend(errors);
                IndexMap::new()
            }
        };
        if !self.ensure_spec_types_resolved(spec, spec_repository, effective, type_resolver) {
            return;
        }

        let mut imports: HashMap<String, PlannedImport> = HashMap::new();
        for data in &spec.data {
            if !data.reference.is_local() || !matches!(data.value, ParsedDataValue::Import { .. }) {
                continue;
            }
            if let Some(import) = self.plan_import(data, spec, spec_repository, effective) {
                imports.entry(data.reference.name.clone()).or_insert(import);
            }
        }
        let bindings = self.resolve_instance_alias_bindings(bindings, spec, &imports);

        for data in &spec.data {
            if !data.reference.is_local()
                || self.is_data_instance_list(data, spec, spec_repository, effective)
            {
                continue;
            }
            if matches!(data.value, ParsedDataValue::Import { .. }) {
                if let Some(planned) = imports.get(&data.reference.name) {
                    self.add_import(data, planned, &bindings, spec);
                }
                continue;
            }
            self.add_data(data, spec, spec_repository, effective);
        }
        for data in &spec.data {
            if data.reference.is_local()
                && self.is_data_instance_list(data, spec, spec_repository, effective)
            {
                self.plan_instance_list(data, spec, spec_repository, effective, &bindings);
            }
        }

        let data_map: HashMap<String, &LemmaData> = spec
            .data
            .iter()
            .map(|data| (data.reference.name.clone(), data))
            .collect();
        let rule_names: HashSet<&str> = spec.rules.iter().map(|r| r.name.as_str()).collect();
        for rule in &spec.rules {
            self.add_rule(
                rule,
                spec,
                spec_repository,
                &data_map,
                &rule_names,
                effective,
            );
        }
    }

    /// Every temporal body a `uses … -> effective` import can select, without requiring
    /// the dependency to be active at the consumer's planning instant.
    fn dynamic_import_targets(
        &mut self,
        spec_ref: &ast::SpecRef,
        consumer: &LemmaSpec,
        consumer_repository: &Arc<LemmaRepository>,
        source: &Source,
    ) -> Result<Vec<discovery::ResolvedDependencyEdge<'a>>, Error> {
        let resolved = discovery::resolve_spec_ref_repository(
            self.context,
            spec_ref,
            Some(source),
            consumer.name.as_str(),
            Some(consumer),
        )?;
        let repository = match &resolved.repository {
            Some(explicit) => Arc::clone(explicit),
            None => Arc::clone(consumer_repository),
        };
        let Some(dep_set) = self.context.spec_set(&repository, &spec_ref.name) else {
            return Err(self.engine_error(
                format!(
                    "dependency '{}' does not exist",
                    discovery::format_dep_qualified_name(&repository, &spec_ref.name)
                ),
                source,
            ));
        };
        let mut targets = Vec::new();
        for version in dep_set.iter_specs() {
            let version_slices =
                match self
                    .interners
                    .slices
                    .of(self.context, &repository, version, self.limits)
                {
                    Ok(slices) => slices,
                    Err(errors) => {
                        self.errors.extend(errors);
                        return Err(self.engine_error(
                            format!(
                                "failed to plan temporal slices of '{}'",
                                discovery::format_dep_qualified_name(&repository, &spec_ref.name)
                            ),
                            source,
                        ));
                    }
                };
            for start in version_slices.starts() {
                if discovery::same_loaded_spec(version, consumer) {
                    continue;
                }
                targets.push(discovery::ResolvedDependencyEdge {
                    repository: Arc::clone(&repository),
                    spec: version,
                    at: start.clone(),
                });
            }
        }
        if targets.is_empty() {
            return Err(self.engine_error(
                format!(
                    "'{}' depends on '{}' via `-> effective`, but that dependency has no temporal versions",
                    consumer.name,
                    discovery::format_dep_qualified_name(&repository, &spec_ref.name)
                ),
                source,
            ));
        }
        Ok(targets)
    }

    /// Plan one `uses` import: a single body, or every candidate body for `-> effective`.
    fn plan_import(
        &mut self,
        data: &LemmaData,
        spec: &'a LemmaSpec,
        spec_repository: &Arc<LemmaRepository>,
        effective: &EffectiveDate,
    ) -> Option<PlannedImport> {
        let ParsedDataValue::Import {
            spec_ref,
            effective_ref,
            ..
        } = &data.value
        else {
            unreachable!("BUG: plan_import on non-import data");
        };
        let Some(pin_ref) = effective_ref else {
            let (repository, imported) =
                match self.resolve_spec_ref(spec_ref, effective, spec, spec_repository) {
                    Ok(pair) => pair,
                    Err(error) => {
                        self.errors.push(error);
                        return None;
                    }
                };
            let at = spec_ref.at(effective);
            let identity = self.identity_at(&repository, imported, &at)?;
            let template = self.row_template(&identity, &repository, imported, &at)?;
            return Some(PlannedImport::Static {
                identity: Arc::new(identity),
                template,
            });
        };

        if !pin_ref.segments.is_empty() {
            self.errors.push(self.engine_error(
                "`-> effective` names a local data or rule of this spec, not a path through an import"
                    .to_string(),
                &data.source_location,
            ));
            return None;
        }
        if pin_ref.name == data.reference.name {
            self.errors.push(self.engine_error(
                format!(
                    "`-> effective {}` cannot name the import alias '{}'",
                    pin_ref.name, data.reference.name
                ),
                &data.source_location,
            ));
            return None;
        }

        let pin = match self.resolve_effective_pin(pin_ref, spec, &data.source_location) {
            Ok(pin) => pin,
            Err(error) => {
                self.errors.push(error);
                return None;
            }
        };

        let targets = match self.dynamic_import_targets(
            spec_ref,
            spec,
            spec_repository,
            &data.source_location,
        ) {
            Ok(targets) => targets,
            Err(error) => {
                self.errors.push(error);
                return None;
            }
        };

        let mut candidates = Vec::new();
        for target in targets {
            let identity = self.identity_at(&target.repository, target.spec, &target.at)?;
            let template =
                self.row_template(&identity, &target.repository, target.spec, &target.at)?;
            candidates.push((Arc::new(identity), template));
        }
        Some(PlannedImport::Dynamic { pin, candidates })
    }

    /// Resolve `-> effective <ref>` to a date/datetime data or rule of `spec`.
    /// Type is checked in [`Self::validate_effective_pin_type`] / after rule inference.
    fn resolve_effective_pin(
        &self,
        pin_ref: &ast::Reference,
        spec: &LemmaSpec,
        source: &Source,
    ) -> Result<ImportEffectivePin, Error> {
        if spec
            .data
            .iter()
            .any(|data| data.reference.is_local() && data.reference.name == pin_ref.name)
        {
            return Ok(ImportEffectivePin::Data(DataPath::local(
                pin_ref.name.clone(),
            )));
        }
        if spec.rules.iter().any(|rule| rule.name == pin_ref.name) {
            return Ok(ImportEffectivePin::Rule(RulePath::new(
                Vec::new(),
                pin_ref.name.clone(),
            )));
        }
        Err(self.engine_error(
            format!(
                "`-> effective {}` does not name data or a rule of '{}'",
                pin_ref.name, spec.name
            ),
            source,
        ))
    }

    /// `uses alias: spec -> with ...`: the import row(s), bindings written over each
    /// candidate template's slots.
    fn add_import(
        &mut self,
        data: &LemmaData,
        planned: &PlannedImport,
        bindings: &DataBindings,
        spec: &LemmaSpec,
    ) {
        let alias = &data.reference.name;
        let data_path = DataPath::local(alias.clone());
        if self.is_duplicate_data(data, &data_path, spec) {
            return;
        }
        let under = bindings_under(bindings, alias);
        match planned {
            PlannedImport::Static { identity, template } => {
                let prefix = vec![PathSegment::import(alias, identity)];
                let row = self.binding_row(RowWrite {
                    base: vec![None; template.data.len()],
                    identity: Arc::clone(identity),
                    template: Arc::clone(template),
                    origin: RowOrigin::Import {
                        alias: Arc::from(alias.as_str()),
                    },
                    bindings: under,
                    prefix,
                    hops: 0,
                });
                self.data.insert(
                    data_path,
                    DataDefinition::Import {
                        row,
                        pin: None,
                        candidates: BTreeMap::new(),
                        source: data.source_location.clone(),
                    },
                );
            }
            PlannedImport::Dynamic { pin, candidates } => {
                if let Err(error) =
                    self.validate_effective_pin_type(pin, spec, &data.source_location)
                {
                    self.errors.push(error);
                    return;
                }
                let mut candidate_rows = BTreeMap::new();
                for (identity, template) in candidates {
                    for binding in under.keys() {
                        let slot = binding.last().expect("BUG: binding key is never empty");
                        let path = DataPath::local(slot.clone());
                        if !template.data.contains_key(&path)
                            && !template
                                .data
                                .keys()
                                .any(|data_path| data_path.input_key() == *slot)
                        {
                            // Binding paths may be multi-segment; binding_row reports missing slots.
                        }
                    }
                    let prefix = vec![PathSegment::import(alias, identity)];
                    let row = self.binding_row(RowWrite {
                        base: vec![None; template.data.len()],
                        identity: Arc::clone(identity),
                        template: Arc::clone(template),
                        origin: RowOrigin::Import {
                            alias: Arc::from(alias.as_str()),
                        },
                        bindings: under.clone(),
                        prefix,
                        hops: 0,
                    });
                    candidate_rows.insert(identity.as_ref().clone(), row);
                }
                let row = Arc::clone(
                    candidate_rows
                        .values()
                        .next()
                        .expect("BUG: dynamic import candidates are non-empty"),
                );
                self.data.insert(
                    data_path,
                    DataDefinition::Import {
                        row,
                        pin: Some(pin.clone()),
                        candidates: candidate_rows,
                        source: data.source_location.clone(),
                    },
                );
            }
        }
    }

    fn validate_effective_pin_type(
        &self,
        pin: &ImportEffectivePin,
        spec: &LemmaSpec,
        source: &Source,
    ) -> Result<(), Error> {
        let lemma_type: Option<Arc<LemmaType>> = match pin {
            ImportEffectivePin::Data(path) => self
                .data
                .get(path)
                .and_then(|definition| definition.resolved_type_arc().map(Arc::clone))
                .or_else(|| {
                    // Pin data may be declared after the uses in source order; look at AST.
                    let name = &path.data;
                    spec.data.iter().find_map(|data| {
                        if data.reference.name != *name {
                            return None;
                        }
                        match &data.value {
                            ParsedDataValue::Definition {
                                base:
                                    Some(ParentType::Primitive {
                                        primitive: PrimitiveKind::Date,
                                    }),
                                ..
                            } => Some(Arc::clone(semantics::primitive_date_arc())),
                            _ => None,
                        }
                    })
                }),
            ImportEffectivePin::Rule(path) => {
                self.rules.get(path).map(|rule| Arc::clone(&rule.rule_type))
            }
        };
        let Some(lemma_type) = lemma_type else {
            // Rule types are filled later; allow when the rule exists in the AST.
            // Rule types are filled after imports; validated again once inferred.
            if matches!(pin, ImportEffectivePin::Rule(_)) {
                return Ok(());
            }
            return Err(self.engine_error(
                "`-> effective` must name date or datetime data of this spec".to_string(),
                source,
            ));
        };
        if lemma_type.is_date() {
            return Ok(());
        }
        Err(self.engine_error(
            format!(
                "`-> effective` requires a date or datetime, got {}",
                lemma_type.specifications
            ),
            source,
        ))
    }

    /// The row `write` describes, with every row its bindings hold. A list binding's
    /// copies are rows owned by the writing plan; a binding through a nested `uses`
    /// (`-> with j.x: 2`) writes over that import's row, held as [`RowBinding::Instance`].
    /// A binding that does not type is an error and leaves its slot as the base has it.
    fn binding_row(&mut self, write: RowWrite) -> Arc<RowInstance> {
        /// A list binding of template slot `slot`, filled by the rows of its copies.
        struct PendingList {
            slot: usize,
            rows: Vec<Arc<RowInstance>>,
            row_type: Arc<LemmaType>,
            key: ListKey,
            template: Arc<ExecutionPlan>,
            source: Source,
        }

        struct Pending {
            identity: Arc<SpecIdentity>,
            origin: RowOrigin,
            hops: u32,
            bindings: Vec<Option<RowBinding>>,
            lists: Vec<PendingList>,
            /// Row and slot that holds this row.
            parent: Option<(usize, usize)>,
        }

        let mut pending: Vec<Pending> = Vec::new();
        let mut writes: Vec<(RowWrite, Option<(usize, usize)>)> = vec![(write, None)];
        while let Some((write, parent)) = writes.pop() {
            let row = pending.len();
            let mut bindings = write.base;
            let mut lists = Vec::new();
            let mut nested: IndexMap<String, DataBindings> = IndexMap::new();
            for (key, (value, source)) in &write.bindings {
                let (name, rest) = key.split_first().expect("BUG: binding key is never empty");
                let path = DataPath::local(name.clone());
                let (slot, _, definition) =
                    write.template.data.get_full(&path).unwrap_or_else(|| {
                        panic!(
                            "BUG: binding `{}` is not data of `{}`",
                            key.join("."),
                            write.template.identity.spec
                        )
                    });
                if !rest.is_empty() {
                    assert!(
                        matches!(definition, DataDefinition::Import { .. }),
                        "BUG: binding `{}` steps through `{name}`, which is not a `uses`",
                        key.join(".")
                    );
                    nested
                        .entry(name.clone())
                        .or_default()
                        .insert(rest.to_vec(), (value.clone(), source.clone()));
                    continue;
                }
                if matches!(definition, DataDefinition::Import { .. }) {
                    let bound = write
                        .prefix
                        .iter()
                        .map(PathSegment::input_key)
                        .chain(std::iter::once(name.clone()))
                        .collect::<Vec<_>>()
                        .join(".");
                    self.errors.push(self.engine_error(
                        format!("cannot bind import alias '{bound}' as a value"),
                        source,
                    ));
                    continue;
                }
                match value {
                    BindingValue::Literal(literal) => {
                        let field_type = definition
                            .resolved_type_arc()
                            .expect("BUG: a value slot of a template plan is typed");
                        match source_row_literal(literal, field_type, &write.template) {
                            Ok(bound) => {
                                bindings[slot] =
                                    Some(RowBinding::Bound(OperationResult::from_bound(bound)));
                            }
                            Err(message) => {
                                self.errors.push(self.engine_error(message, source));
                            }
                        }
                    }
                    BindingValue::Reference { target, .. } => {
                        bindings[slot] = Some(RowBinding::Deferred {
                            target: target.clone(),
                            hops: write.hops,
                        });
                    }
                    BindingValue::InstanceRows(copies) => {
                        let (nested_identity, nested_template) =
                            self.nested_row_template(&write.template, &path);
                        let row_type = Arc::new(LemmaType::primitive(TypeSpecification::Spec {
                            spec: nested_identity.as_ref().clone(),
                        }));
                        lists.push(PendingList {
                            slot,
                            rows: Vec::new(),
                            row_type,
                            key: write
                                .template
                                .instance_lists
                                .get(&path)
                                .expect(
                                    "BUG: a list binding slot is an instance list of its template",
                                )
                                .key
                                .clone(),
                            template: Arc::clone(&nested_template),
                            source: source.clone(),
                        });
                        let list_name: Arc<str> = Arc::from(name.as_str());
                        let spec_name: Arc<str> = Arc::from(nested_identity.spec.as_str());
                        let repository: Option<Arc<str>> =
                            nested_identity.repository.as_deref().map(Arc::from);
                        for (index, copy) in copies.iter().enumerate() {
                            if copy.identity != *nested_identity {
                                self.errors.push(self.engine_error(
                                    format!(
                                        "`{}` is a `{}` instance; rows of `{name}` are `{}`",
                                        copy.name, copy.identity, nested_identity
                                    ),
                                    &copy.source,
                                ));
                                continue;
                            }
                            let segment = PathSegment::list_row_shared(
                                &list_name,
                                index + 1,
                                &spec_name,
                                repository.as_ref(),
                            );
                            let prefix: Vec<PathSegment> =
                                write.prefix.iter().cloned().chain([segment]).collect();
                            writes.push((
                                RowWrite {
                                    identity: Arc::clone(&nested_identity),
                                    base: vec![None; nested_template.data.len()],
                                    template: Arc::clone(&nested_template),
                                    origin: RowOrigin::Written {
                                        instance: SpecInstance {
                                            prefix: prefix.clone(),
                                        },
                                        source: copy.source.clone(),
                                    },
                                    bindings: copy.bindings.clone(),
                                    prefix,
                                    hops: 0,
                                },
                                Some((row, slot)),
                            ));
                        }
                    }
                    BindingValue::InstanceAliases(_) => unreachable!(
                        "BUG: instance aliases are resolved to rows before rows are built"
                    ),
                }
            }
            for (name, sub) in nested {
                let path = DataPath::local(name.clone());
                let (slot, _, definition) = write
                    .template
                    .data
                    .get_full(&path)
                    .expect("BUG: nested binding slot is template data");
                let DataDefinition::Import {
                    row: template_row, ..
                } = definition
                else {
                    unreachable!("BUG: nested binding slot is a `uses`");
                };
                let base = match &bindings[slot] {
                    Some(RowBinding::Instance(written)) => written.bindings.to_vec(),
                    None => template_row.bindings.to_vec(),
                    Some(other) => panic!("BUG: `uses` slot `{name}` holds {other:?}"),
                };
                let identity = Arc::clone(&template_row.identity);
                let template = self.template_of(&identity);
                let prefix: Vec<PathSegment> = write
                    .prefix
                    .iter()
                    .cloned()
                    .chain([PathSegment::import(&name, &identity)])
                    .collect();
                writes.push((
                    RowWrite {
                        identity,
                        template,
                        origin: template_row.origin.clone(),
                        base,
                        bindings: sub,
                        prefix,
                        hops: write.hops + 1,
                    },
                    Some((row, slot)),
                ));
            }
            pending.push(Pending {
                identity: write.identity,
                origin: write.origin,
                hops: write.hops,
                bindings,
                lists,
                parent,
            });
        }

        // Rows complete last to first: every row completes before the row holding it,
        // and each list's rows arrive in order.
        loop {
            let row = pending.pop().expect("BUG: the written row is pending");
            let mut bindings = row.bindings;
            for list in row.lists {
                let rows =
                    match self.row_store_key(&list.rows, &list.key, &list.source, &list.template) {
                        None => list.rows,
                        Some(store_key) => match RowStore::keyed(list.rows, store_key) {
                            Ok(store) => store.rows,
                            Err((message, unkeyed)) => {
                                self.errors.push(self.engine_error(message, &list.source));
                                unkeyed.rows
                            }
                        },
                    };
                bindings[list.slot] = Some(RowBinding::Rows {
                    rows,
                    row_type: list.row_type,
                    hops: row.hops,
                });
            }
            let built = Arc::new(RowInstance {
                identity: row.identity,
                origin: row.origin,
                bindings: bindings.into(),
            });
            let Some((holder, slot)) = row.parent else {
                assert!(
                    pending.is_empty(),
                    "BUG: only the written row has no holder"
                );
                return built;
            };
            let holder = &mut pending[holder];
            match holder.lists.iter_mut().find(|list| list.slot == slot) {
                Some(list) => list.rows.push(built),
                None => holder.bindings[slot] = Some(RowBinding::Instance(built)),
            }
        }
    }

    /// Template plan of `identity`, recorded on this graph with a template that imports it.
    fn template_of(&self, identity: &SpecIdentity) -> Arc<ExecutionPlan> {
        Arc::clone(
            self.instance_lists
                .templates
                .get(identity)
                .unwrap_or_else(|| {
                    panic!(
                        "BUG: template of `{identity}` is recorded with the template importing it"
                    )
                }),
        )
    }

    /// Replace every [`BindingValue::InstanceAliases`] by the row copies it names: one
    /// per alias, each a `uses` instance of `spec` with all of that instance's bindings.
    ///
    /// A copied instance's own list bindings name aliases too (`root -> with children:
    /// mid_node`, `mid_node -> with children: leaf`), so aliases are resolved in
    /// dependency order and each copy takes the resolved bindings of its alias. An alias
    /// that reaches itself is a planning error: no row can hold itself.
    fn resolve_instance_alias_bindings(
        &mut self,
        bindings: DataBindings,
        spec: &LemmaSpec,
        imports: &HashMap<String, PlannedImport>,
    ) -> DataBindings {
        let mut resolved_aliases: HashMap<String, DataBindings> = HashMap::new();
        let mut visiting: Vec<String> = Vec::new();
        // `true` once the alias's dependencies are on the stack above it.
        let mut stack: Vec<(String, bool)> = aliases_named(&bindings)
            .into_iter()
            .rev()
            .map(|alias| (alias, false))
            .collect();
        while let Some((alias, entered)) = stack.last().cloned() {
            if entered {
                stack.pop();
                visiting.pop();
                let own = bindings_under(&bindings, &alias);
                let resolved = self.resolve_alias_layer(own, spec, imports, &resolved_aliases);
                resolved_aliases.insert(alias, resolved);
                continue;
            }
            if resolved_aliases.contains_key(&alias) {
                stack.pop();
                continue;
            }
            if visiting.contains(&alias) {
                let chain: Vec<&str> = visiting
                    .iter()
                    .skip_while(|seen| **seen != alias)
                    .map(String::as_str)
                    .chain(std::iter::once(alias.as_str()))
                    .collect();
                let source = alias_binding_source(&bindings, &alias)
                    .expect("BUG: an alias on the resolution stack is named by a binding");
                self.errors.push(self.engine_error(
                    format!(
                        "Circular instance-list source wiring: {}",
                        chain.join(" -> ")
                    ),
                    &source,
                ));
                resolved_aliases.insert(alias, IndexMap::new());
                stack.pop();
                continue;
            }
            stack.last_mut().expect("BUG: stack top cloned above").1 = true;
            visiting.push(alias.clone());
            let dependencies = aliases_named(&bindings_under(&bindings, &alias));
            for dependency in dependencies.into_iter().rev() {
                if !resolved_aliases.contains_key(&dependency) {
                    stack.push((dependency, false));
                }
            }
        }
        self.resolve_alias_layer(bindings, spec, imports, &resolved_aliases)
    }

    /// One binding map with its [`BindingValue::InstanceAliases`] replaced, each alias
    /// taking its bindings from `resolved_aliases`.
    fn resolve_alias_layer(
        &mut self,
        bindings: DataBindings,
        spec: &LemmaSpec,
        imports: &HashMap<String, PlannedImport>,
        resolved_aliases: &HashMap<String, DataBindings>,
    ) -> DataBindings {
        let mut resolved: DataBindings = IndexMap::with_capacity(bindings.len());
        for (key, (value, source)) in bindings {
            let BindingValue::InstanceAliases(aliases) = &value else {
                resolved.insert(key, (value, source));
                continue;
            };
            let mut rows = Vec::with_capacity(aliases.len());
            let mut failed = false;
            for alias in aliases {
                let is_local_import = alias.segments.is_empty()
                    && spec.data.iter().any(|data| {
                        data.reference.is_local()
                            && data.reference.name == alias.name
                            && matches!(data.value, ParsedDataValue::Import { .. })
                    });
                if !is_local_import {
                    let slot = key.last().expect("BUG: binding key is never empty");
                    self.errors.push(self.engine_error(
                        format!(
                            "binding `{}` replaces instance list `{slot}` with `{alias}`, which is not a `uses` instance of `{}`; list slots take `uses` aliases",
                            key.join("."),
                            spec.name
                        ),
                        &source,
                    ));
                    failed = true;
                    continue;
                }
                let Some(planned) = imports.get(&alias.name) else {
                    // The alias's import did not resolve: its error is recorded.
                    failed = true;
                    continue;
                };
                if planned.is_dynamic() {
                    self.errors.push(self.engine_error(
                        format!(
                            "cannot seal `uses {}` into a list: it uses `-> effective`",
                            alias.name
                        ),
                        &source,
                    ));
                    failed = true;
                    continue;
                }
                let identity = planned.primary_identity();
                let alias_bindings = resolved_aliases
                    .get(&alias.name)
                    .unwrap_or_else(|| {
                        panic!(
                            "BUG: alias `{}` named by a binding was not resolved",
                            alias.name
                        )
                    })
                    .clone();
                rows.push(RowCopy {
                    identity: identity.as_ref().clone(),
                    name: alias.name.clone(),
                    bindings: alias_bindings,
                    source: source.clone(),
                });
            }
            if failed {
                continue;
            }
            resolved.insert(key, (BindingValue::InstanceRows(rows), source));
        }
        resolved
    }

    fn add_rule(
        &mut self,
        rule: &LemmaRule,
        current_spec: &'a LemmaSpec,
        current_repository: &Arc<LemmaRepository>,
        data_map: &HashMap<String, &LemmaData>,
        rule_names: &HashSet<&str>,
        effective: &EffectiveDate,
    ) {
        let rule_path = RulePath::new(Vec::new(), rule.name.clone());

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
            repository: current_repository,
            data_map,
            rule_names,
            rule: &rule.name,
            effective,
            depends_on_rules: &mut depends_on_rules,
            row_scopes: Vec::new(),
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

        let expression = ExprTree::new(std::mem::take(&mut convert_ctx.table), root);
        let rule_node = RuleNode {
            expression,
            branches,
            source: rule.source_location.clone(),
            depends_on_rules,
            recursive: false,
            rule_type: Arc::new(LemmaType::veto_type()),
            scale_targets: Vec::new(),
            spec: current_spec,
        };

        self.rules.insert(rule_path, rule_node);
    }

    /// [`RuleNode::recursive`] for every local rule named on a self-list row during conversion.
    fn mark_self_list_row_rules_recursive(&mut self) {
        let targets: Vec<RulePath> = self.self_list_row_rules.iter().cloned().collect();
        for path in targets {
            if self.rules.contains_key(&path) {
                self.rules
                    .get_mut(&path)
                    .expect("BUG: self-list row rule missing from graph after contains_key")
                    .recursive = true;
            } else if self.dropped_rules.contains(&path) {
                continue;
            } else {
                panic!(
                    "BUG: self-list row rule '{path}' neither in graph nor dropped after conversion"
                )
            }
        }
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
        &mut self,
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
        let level = u32::try_from(ctx.row_scopes.len()).expect("BUG: row scope depth exceeds u32");
        let value_predicate = matches!(
            command,
            ast::ListCommand::Any {
                form: ast::ListPredicate::Value { .. }
            } | ast::ListCommand::All {
                form: ast::ListPredicate::Value { .. }
            } | ast::ListCommand::Where {
                form: ast::ListPredicate::Value { .. }
            }
        );
        let mut missing_row_template = false;
        let row_scope = if value_predicate {
            Some(RowScope::Value { level, list })
        } else {
            self.instance_list_identity(
                ExprRef {
                    table: &ctx.table,
                    id: list,
                },
                ctx.spec,
            )
            .and_then(|identity| {
                let Some(template) = self.row_template_of(&identity) else {
                    self.errors
                        .push(self.missing_row_template_error(&identity, &source));
                    missing_row_template = true;
                    return None;
                };
                let list_path = match node.child(*list_source).kind() {
                    ast::ExpressionKind::Reference(reference) => Some(reference.full_path()),
                    _ => None,
                };
                Some(RowScope::Instance {
                    list_path,
                    identity,
                    template,
                    level,
                })
            })
        };
        let row_level = row_scope.as_ref().map(RowScope::level);
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
            failed: missing_row_template,
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
            ast::ExpressionKind::ImplicitRowValue => {
                let scope = ctx
                    .row_scopes
                    .iter()
                    .rev()
                    .find_map(|scope| match scope {
                        RowScope::Value { level, list } => Some((*level, *list)),
                        RowScope::Instance { .. } => None,
                    })
                    .expect("BUG: implicit row value converted outside a value predicate");
                Some(ctx.push(
                    ExpressionKind::RowValue {
                        level: scope.0,
                        list: scope.1,
                    },
                    source,
                ))
            }
            ast::ExpressionKind::Reference(r) => {
                let expr_source = expr_src;
                if !ctx.row_scopes.is_empty() {
                    match self.resolve_row_field(r, expr_source, ctx) {
                        RowFieldLookup::Field { field, member } => {
                            let ExpressionKind::RowField { identity, end, .. } = field.as_ref()
                            else {
                                unreachable!("BUG: a row field lookup holds a RowField read");
                            };
                            self.note_row_end_read(identity, end);
                            let field = ctx.push(*field, source.clone());
                            let Some((path, name)) = member else {
                                return Some(field);
                            };
                            return Some(ctx.push(
                                ExpressionKind::SpecMember {
                                    base: field,
                                    path,
                                    name,
                                },
                                source.clone(),
                            ));
                        }
                        RowFieldLookup::Failed => return None,
                        RowFieldLookup::NotARowField => {}
                    }
                }
                if let Some((head, rest)) = r.segments.split_first() {
                    if ctx.rule_names.contains(head.as_str()) && !ctx.data_map.contains_key(head) {
                        let base = RulePath::new(Vec::new(), head.clone());
                        ctx.depends_on_rules.insert(base.clone());
                        let base = ctx.push(ExpressionKind::RulePath(base), source.clone());
                        self.note_row_member_read(base, rest, &r.name, ctx);
                        return Some(ctx.push(
                            ExpressionKind::SpecMember {
                                base,
                                path: rest.to_vec(),
                                name: r.name.clone(),
                            },
                            source.clone(),
                        ));
                    }
                }
                let target_arc_opt = if r.segments.is_empty() {
                    None
                } else {
                    let (_, arc) = self.resolve_path_segments(
                        &r.segments,
                        expr_source,
                        Vec::new(),
                        ctx.spec,
                        ctx.repository,
                        ctx.effective,
                    )?;
                    Some(arc)
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
                if (is_data || is_rule) && !r.segments.is_empty() {
                    let (head, rest) = r
                        .segments
                        .split_first()
                        .expect("BUG: segments checked non-empty");
                    let base = ctx.push(
                        ExpressionKind::DataPath(DataPath::local(head.clone())),
                        source.clone(),
                    );
                    return Some(ctx.push(
                        ExpressionKind::SpecMember {
                            base,
                            path: rest.to_vec(),
                            name: r.name.clone(),
                        },
                        source.clone(),
                    ));
                }
                if is_data {
                    let data_path = DataPath::local(r.name.clone());
                    return Some(ctx.push(ExpressionKind::DataPath(data_path), source.clone()));
                }
                if is_rule {
                    let rule_path = RulePath::new(Vec::new(), r.name.clone());
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

            ast::ExpressionKind::Logical {
                connective, steps, ..
            } => {
                let (first, steps) = convert_chain_steps(steps, children);
                Some(ctx.push(
                    ExpressionKind::Logical {
                        connective: *connective,
                        first,
                        steps,
                    },
                    source,
                ))
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

            ast::ExpressionKind::StepRounding { mode, .. } => {
                let mut children = children.iter();
                let operand = *children
                    .next()
                    .expect("BUG: step rounding always has an operand");
                let step = children.next().copied();
                Some(ctx.push(
                    ExpressionKind::StepRounding {
                        operand,
                        mode: *mode,
                        step,
                    },
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
                        let (bare, lt) = match self.resolve_unit_ref_in_scopes(
                            ctx.spec,
                            unit,
                            &ctx.row_scopes,
                        ) {
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
                            match self
                                .measure_ratio_range_from_number_with_unit_endpoints_in_scopes(
                                    ctx.spec,
                                    *left_mag,
                                    left_unit,
                                    *right_mag,
                                    right_unit,
                                    &ctx.row_scopes,
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
                    Value::NumberWithUnit(_, unit) => match self.resolve_unit_ref_in_scopes(
                        ctx.spec,
                        unit,
                        &ctx.row_scopes,
                    ) {
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
                    },
                    source,
                ))
            }
        }
    }

    /// Row spec of the rows a converted list expression yields, when it is an instance list.
    ///
    /// `spec` is the spec being converted: its rules that are not converted
    /// yet are read from source.
    fn instance_list_identity(
        &self,
        list: ExprRef<'_, ExpressionKind>,
        spec: &LemmaSpec,
    ) -> Option<SpecIdentity> {
        let scope = SourceScope { spec };
        self.search_identity(
            IdentitySearch::new(SearchKind::List, Pending::Sem(list), Continuation::Answer),
            &scope,
        )
    }

    /// Row spec of the single row a converted expression yields (`list -> first`), when
    /// it picks a row of an instance list.
    fn row_value_identity<'e>(
        &'e self,
        expression: ExprRef<'e, ExpressionKind>,
        scope: &SourceScope<'e>,
    ) -> Option<SpecIdentity> {
        self.search_identity(
            IdentitySearch::new(
                SearchKind::Row,
                Pending::Sem(expression),
                Continuation::Answer,
            ),
            scope,
        )
    }

    /// Runs `root` and the searches it nests, depth-first with explicit stacks. A search
    /// that runs out of work yields nothing to its parent, which goes on with its own work.
    fn search_identity<'e>(
        &'e self,
        root: IdentitySearch<'e>,
        scope: &SourceScope<'e>,
    ) -> Option<SpecIdentity> {
        let mut searches = vec![root];
        while let Some(search) = searches.last_mut() {
            let Some(item) = search.pending.pop() else {
                searches.pop();
                continue;
            };
            let step = match search.kind {
                SearchKind::List => self.list_identity_step(item, scope, search),
                SearchKind::Row => self.row_identity_step(item, scope, search),
            };
            let mut found = match step {
                SearchStep::Next => continue,
                SearchStep::Nested(nested) => {
                    searches.push(nested);
                    continue;
                }
                SearchStep::Found(identity) => identity,
            };
            loop {
                let done = searches
                    .pop()
                    .expect("BUG: the search that found an identity is on the stack");
                match done.on_found {
                    Continuation::Answer => return Some(found),
                    Continuation::RowOfList => {}
                    Continuation::Member { path, name } => {
                        let parent = searches
                            .last_mut()
                            .expect("BUG: a member search nests in a list search");
                        let chain = self.row_template_of(&found).and_then(|template| {
                            let mut parts: Vec<&str> = path.iter().map(String::as_str).collect();
                            parts.push(name);
                            self.template_row_chain(&found, &template, &parts)
                        });
                        match chain.and_then(|chain| {
                            let last = chain.last().expect("BUG: a row end chain has a last hop");
                            self.row_end_list_identity(
                                &last.identity,
                                &last.end,
                                &mut parent.pending,
                            )
                        }) {
                            Some(identity) => found = identity,
                            None => break,
                        }
                    }
                }
            }
        }
        None
    }

    /// One work item of a [`SearchKind::List`] search: pipeline commands that keep the row
    /// spec, rule bodies, conditional arms and member reads on a row.
    fn list_identity_step<'e>(
        &'e self,
        item: Pending<'e>,
        scope: &SourceScope<'e>,
        search: &mut IdentitySearch<'e>,
    ) -> SearchStep<'e> {
        let identity = match item {
            Pending::Sem(list) => match list.kind() {
                ExpressionKind::DataPath(path) => self.instance_list_data_identity(path),
                ExpressionKind::Literal(literal) => match &literal.lemma_type.specifications {
                    TypeSpecification::List { element, .. } => match &element.specifications {
                        TypeSpecification::Spec { spec } => Some(spec.clone()),
                        _ => None,
                    },
                    _ => None,
                },
                ExpressionKind::ListCommand {
                    list: inner, op, ..
                } => match op {
                    ListOp::Predicate {
                        kind: ListPredKind::Where,
                        ..
                    }
                    | ListOp::Sort { .. }
                    | ListOp::Top { .. }
                    | ListOp::With { .. }
                    | ListOp::WithRow { .. }
                    | ListOp::Key { .. } => {
                        search.pending.push(Pending::Sem(list.child(*inner)));
                        None
                    }
                    ListOp::Predicate { .. }
                    | ListOp::First
                    | ListOp::Last
                    | ListOp::Count
                    | ListOp::Find { .. }
                    | ListOp::Reduce { .. } => None,
                },
                ExpressionKind::RulePath(rule) => {
                    search.pending.push(Pending::Rule(rule.clone()));
                    None
                }
                ExpressionKind::Piecewise(arms) => {
                    for (_, result) in arms.iter().rev() {
                        search.pending.push(Pending::Sem(list.child(*result)));
                    }
                    None
                }
                ExpressionKind::RowField { identity, end, .. } => {
                    self.row_end_list_identity(identity, end, &mut search.pending)
                }
                ExpressionKind::SpecMember {
                    base, path, name, ..
                } => {
                    return SearchStep::Nested(search.nested(
                        SearchKind::Row,
                        Pending::Sem(list.child(*base)),
                        Continuation::Member { path, name },
                    ));
                }
                _ => None,
            },
            Pending::Ast(expr) => match expr.kind() {
                ast::ExpressionKind::Reference(reference) if reference.segments.is_empty() => {
                    let is_data = scope.spec.data.iter().any(|data| {
                        data.reference.is_local() && data.reference.name == reference.name
                    });
                    if is_data {
                        self.instance_list_data_identity(&DataPath::local(reference.name.clone()))
                    } else {
                        search.pending.push(Pending::Rule(RulePath::new(
                            Vec::new(),
                            reference.name.clone(),
                        )));
                        None
                    }
                }
                ast::ExpressionKind::ListCommand {
                    list,
                    command:
                        ast::ListCommand::Where { .. }
                        | ast::ListCommand::Sort { .. }
                        | ast::ListCommand::Top { .. }
                        | ast::ListCommand::With { .. }
                        | ast::ListCommand::Key { .. },
                } => {
                    search.pending.push(Pending::Ast(expr.child(*list)));
                    None
                }
                _ => None,
            },
            Pending::Rule(rule) => {
                if !search.enter(&rule) {
                    None
                } else if let Some(node) = self.rules.get(&rule) {
                    if let Some((_, body)) = node.branches.first() {
                        search.pending.push(Pending::Sem(node.expression.at(*body)));
                    }
                    None
                } else if !rule.segments.is_empty() {
                    None
                } else {
                    if let Some(source_rule) = scope.spec.rules.iter().find(|r| r.name == rule.rule)
                    {
                        search
                            .pending
                            .push(Pending::Ast(source_rule.expression.root()));
                    }
                    None
                }
            }
        };
        match identity {
            Some(identity) => SearchStep::Found(identity),
            None => SearchStep::Next,
        }
    }

    /// One work item of a [`SearchKind::Row`] search: `-> first` / `-> last` / `-> find`
    /// hand the list to a nested list search; rule bodies and conditional arms are followed.
    fn row_identity_step<'e>(
        &'e self,
        item: Pending<'e>,
        scope: &SourceScope<'e>,
        search: &mut IdentitySearch<'e>,
    ) -> SearchStep<'e> {
        match item {
            Pending::Sem(expression) => match expression.kind() {
                ExpressionKind::ListCommand {
                    list,
                    op: ListOp::First | ListOp::Last | ListOp::Find { .. },
                    ..
                } => SearchStep::Nested(search.nested(
                    SearchKind::List,
                    Pending::Sem(expression.child(*list)),
                    Continuation::RowOfList,
                )),
                ExpressionKind::RulePath(rule) => {
                    search.pending.push(Pending::Rule(rule.clone()));
                    SearchStep::Next
                }
                ExpressionKind::Piecewise(arms) => {
                    for (_, result) in arms.iter().rev() {
                        search.pending.push(Pending::Sem(expression.child(*result)));
                    }
                    SearchStep::Next
                }
                ExpressionKind::DataPath(path) => match self.data.get(path) {
                    Some(DataDefinition::Import { row, .. }) => {
                        SearchStep::Found(row.identity.as_ref().clone())
                    }
                    _ => SearchStep::Next,
                },
                _ => SearchStep::Next,
            },
            Pending::Ast(expr) => match expr.kind() {
                ast::ExpressionKind::Reference(reference) if reference.segments.is_empty() => {
                    search.pending.push(Pending::Rule(RulePath::new(
                        Vec::new(),
                        reference.name.clone(),
                    )));
                    SearchStep::Next
                }
                ast::ExpressionKind::ListCommand {
                    list,
                    command:
                        ast::ListCommand::First | ast::ListCommand::Last | ast::ListCommand::Find { .. },
                } => SearchStep::Nested(search.nested(
                    SearchKind::List,
                    Pending::Ast(expr.child(*list)),
                    Continuation::RowOfList,
                )),
                _ => SearchStep::Next,
            },
            Pending::Rule(rule) => {
                if !search.enter(&rule) {
                } else if let Some(node) = self.rules.get(&rule) {
                    for (_, body) in node.branches.iter().rev() {
                        search.pending.push(Pending::Sem(node.expression.at(*body)));
                    }
                } else if rule.segments.is_empty() {
                    if let Some(source_rule) = scope.spec.rules.iter().find(|r| r.name == rule.rule)
                    {
                        for unless in source_rule.unless_clauses.iter().rev() {
                            search.pending.push(Pending::Ast(unless.result.root()));
                        }
                        search
                            .pending
                            .push(Pending::Ast(source_rule.expression.root()));
                    }
                }
                SearchStep::Next
            }
        }
    }

    /// Row spec of the instance list field `end` names on rows of `identity`. A local
    /// rule of the spec being planned is searched through `pending`.
    fn row_end_list_identity<'e>(
        &'e self,
        identity: &SpecIdentity,
        end: &RowEnd,
        pending: &mut Vec<Pending<'e>>,
    ) -> Option<SpecIdentity> {
        let own_rows = *identity == self.instance_lists.self_identity;
        let lemma_type: &LemmaType = match end {
            RowEnd::Data(field) if own_rows => self.data.get(field)?.resolved_type_arc()?,
            RowEnd::Data(field) => self
                .instance_lists
                .templates
                .get(identity)?
                .data
                .get(field)?
                .resolved_type_arc()?,
            RowEnd::Rule(rule) if own_rows => {
                pending.push(Pending::Rule(rule.clone()));
                return None;
            }
            RowEnd::Rule(rule) => {
                &self
                    .instance_lists
                    .templates
                    .get(identity)?
                    .rules
                    .get(rule)?
                    .rule_type
            }
        };
        if !lemma_type.is_list() {
            return None;
        }
        lemma_type
            .list_element_lemma_type()
            .spec_identity()
            .cloned()
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
    /// recursion through the list, as for [`ExpressionKind::RowField`].
    fn note_row_member_read(
        &mut self,
        base: ExprId,
        path: &[String],
        name: &str,
        ctx: &mut RuleExpressionConversion<'a, '_>,
    ) {
        let scope = SourceScope { spec: ctx.spec };
        let base = ExprRef {
            table: &ctx.table,
            id: base,
        };
        let Some(identity) = self.row_value_identity(base, &scope) else {
            return;
        };
        if identity != self.instance_lists.self_identity {
            return;
        }
        let mut parts: Vec<&str> = path.iter().map(String::as_str).collect();
        parts.push(name);
        if let Some(chain) = self.template_row_chain(&identity, &RowTemplate::Root, &parts) {
            let first = chain.first().expect("BUG: a row end chain has a first hop");
            self.note_row_end_read(&first.identity, &first.end);
        }
    }

    /// A row field read of a self-list: Rule ends and data that reference a rule
    /// mark the target recursive. Never a same-instance dependency edge.
    fn note_row_end_read(&mut self, identity: &SpecIdentity, end: &RowEnd) {
        match end {
            RowEnd::Rule(rule) => self.note_row_rule_read(identity, rule),
            RowEnd::Data(path) => {
                if let Some(DataDefinition::Reference {
                    target: ReferenceTarget::Rule(rule),
                    ..
                }) = self.data.get(path)
                {
                    let rule = rule.clone();
                    self.note_row_rule_read(identity, &rule);
                }
            }
        }
    }

    /// Rule `rule` read on a row of `identity`. On a self-list row this names a child-row
    /// field (a rule of this plan, under its own hops or an import's), never a
    /// same-instance [`RuleNode::depends_on_rules`] edge. The target is marked
    /// [`RuleNode::recursive`] after conversion.
    fn note_row_rule_read(&mut self, identity: &SpecIdentity, rule: &RulePath) {
        if *identity != self.instance_lists.self_identity {
            return;
        }
        self.self_list_row_rules.insert(rule.clone());
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
            let RowScope::Instance {
                list_path,
                identity,
                template,
                level,
            } = scope
            else {
                continue;
            };
            let Some(list_path) = list_path else {
                continue;
            };
            let Some(rest_parts) = written.strip_prefix(list_path.as_slice()) else {
                continue;
            };
            if rest_parts.is_empty() {
                continue;
            }
            let rest: Vec<&str> = rest_parts.iter().map(String::as_str).collect();
            let list_name = list_path.join(".");
            return match self.template_row_chain(identity, template, &rest) {
                Some(chain) => {
                    row_field_lookup(*level, identity, chain, &rest, Some(list_path.clone()))
                }
                None => {
                    let rest_display = rest.join(".");
                    let message = format!(
                        "no data, rule or uses `{rest_display}` on rows of `{list_name}` (`{}`)",
                        identity.spec
                    );
                    self.errors.push(self.engine_error(message, source));
                    RowFieldLookup::Failed
                }
            };
        }
        let field_parts: Vec<&str> = r
            .segments
            .iter()
            .map(String::as_str)
            .chain(std::iter::once(r.name.as_str()))
            .collect();
        let row_matches: Vec<(&RowScope, Vec<RowHop>)> = ctx
            .row_scopes
            .iter()
            .rev()
            .filter_map(|scope| {
                let RowScope::Instance {
                    template, identity, ..
                } = scope
                else {
                    return None;
                };
                self.template_row_chain(identity, template, &field_parts)
                    .map(|chain| (scope, chain))
            })
            .collect();
        let Some((scope, chain)) = row_matches.first() else {
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
                let RowScope::Instance {
                    list_path,
                    identity,
                    ..
                } = other
                else {
                    unreachable!("BUG: row matches contain only instance scopes")
                };
                owners.push(match list_path {
                    Some(path) => format!("on rows of `{}`", path.join(".")),
                    None => format!("on `{}` rows", identity.spec),
                });
            }
            let RowScope::Instance {
                list_path,
                identity,
                ..
            } = scope
            else {
                unreachable!("BUG: row match is an instance scope")
            };
            let qualifier = list_path
                .as_ref()
                .map(|path| path.join("."))
                .unwrap_or_else(|| identity.spec.clone());
            let field = field_parts.join(".");
            self.errors.push(self.engine_error(
                format!(
                    "`{field}` exists {}; write `{qualifier}.{field}`",
                    owners.join(" and ")
                ),
                source,
            ));
            return RowFieldLookup::Failed;
        }
        let RowScope::Instance {
            identity, level, ..
        } = scope
        else {
            unreachable!("BUG: row match is an instance scope")
        };
        row_field_lookup(*level, identity, chain.clone(), &field_parts, None)
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
        let list_type = Arc::clone(list_type);
        let source = expr
            .source()
            .expect("BUG: AST expression missing source location");
        if let TypeSpecification::List { element: row, .. } = &list_type.specifications {
            if let TypeSpecification::Spec { spec: identity } = &row.specifications {
                // Rows this rule builds (`item list -> with ...`) run their rules in a
                // row context of the row spec's template, which `uses item` in this
                // spec brings in; without it, row commands report the missing `uses`.
                if *identity != self.instance_lists.self_identity && self.imports_in_scope(identity)
                {
                    let spec_ref = discovery::list_element_spec_ref(ctx.spec, element)
                        .expect("BUG: a rule list type of spec rows names the row spec");
                    let (row_repository, row_spec) = match discovery::resolve_spec_ref_or_self(
                        self.context,
                        &spec_ref,
                        ctx.repository,
                        ctx.spec,
                        ctx.effective,
                        Some(source.clone()),
                    ) {
                        Ok(pair) => pair,
                        Err(error) => {
                            self.errors.push(error);
                            return None;
                        }
                    };
                    let at = spec_ref.at(ctx.effective);
                    let resolved = self.identity_at(&row_repository, row_spec, &at)?;
                    assert_eq!(
                        resolved, *identity,
                        "BUG: rule list type `{element}` resolved to another slice than its type"
                    );
                    self.row_template(identity, &row_repository, row_spec, &at)?;
                }
            }
        }
        Some(ctx.push(
            ExpressionKind::Literal(Box::new(TypedLiteral {
                value: ValueKind::List(Vec::new().into()),
                lemma_type: list_type,
            })),
            Some(source.clone()),
        ))
    }

    /// Whether the spec being planned has a `uses` of `identity`.
    fn imports_in_scope(&self, identity: &SpecIdentity) -> bool {
        self.data.values().any(|definition| {
            matches!(definition, DataDefinition::Import { row, .. } if *row.identity == *identity)
        })
    }

    /// Path of the data field that `-> with` row field `name` names. Records the error and
    /// returns `None` when it is not data of the row spec or repeats a field of the row.
    fn row_field_path(&mut self, build: &ListCommandBuild<'_>, name: &str) -> Option<DataPath> {
        let scope = build
            .row_scope
            .as_ref()
            .expect("BUG: field arguments are queued only with a row scope");
        let RowScope::Instance {
            template, identity, ..
        } = scope
        else {
            panic!("BUG: field arguments are queued only for instance rows")
        };
        let field = self
            .template_row_chain(identity, template, &[name])
            .and_then(|chain| match chain.as_slice() {
                [RowHop {
                    end: RowEnd::Data(path),
                    ..
                }] => Some(path.clone()),
                _ => None,
            });
        let Some(path) = field else {
            self.errors.push(self.engine_error(
                format!("row field `{name}` is not data on `{}`", identity.spec),
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
                argument: argument.as_ref().map(|_| {
                    converted
                        .next()
                        .expect("BUG: one converted argument per list command argument")
                }),
            }),
            ast::ListCommand::Avg { argument } => Some(ListOp::Reduce {
                kind: ListReduce::Avg,
                argument: argument.as_ref().map(|_| {
                    converted
                        .next()
                        .expect("BUG: one converted argument per list command argument")
                }),
            }),
            ast::ListCommand::Product { argument } => Some(ListOp::Reduce {
                kind: ListReduce::Product,
                argument: argument.as_ref().map(|_| {
                    converted
                        .next()
                        .expect("BUG: one converted argument per list command argument")
                }),
            }),
            ast::ListCommand::Min { argument } => Some(ListOp::Reduce {
                kind: ListReduce::Min,
                argument: argument.as_ref().map(|_| {
                    converted
                        .next()
                        .expect("BUG: one converted argument per list command argument")
                }),
            }),
            ast::ListCommand::Max { argument } => Some(ListOp::Reduce {
                kind: ListReduce::Max,
                argument: argument.as_ref().map(|_| {
                    converted
                        .next()
                        .expect("BUG: one converted argument per list command argument")
                }),
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
                argument: argument.as_ref().map(|_| {
                    converted
                        .next()
                        .expect("BUG: one converted argument per list command argument")
                }),
            }),
            ast::ListCommand::Top { .. } => Some(ListOp::Top {
                n: converted
                    .next()
                    .expect("BUG: one converted argument per list command argument"),
            }),
            ast::ListCommand::With { rows } => match (rows, row_scope) {
                (ast::ListWithRows::Values(_), None) => Some(ListOp::With {
                    rows: converted.collect(),
                }),
                (ast::ListWithRows::Values(_), Some(_)) => {
                    // Named `uses` instances as rows: converted operands are Spec values.
                    (!failed).then_some(ListOp::With {
                        rows: converted.collect(),
                    })
                }
                (ast::ListWithRows::FieldPairs(_), None) => {
                    if !failed {
                        self.errors.push(self.engine_error(
                            "`-> with field: value` appends a row to an instance list; this is a value list, write `-> with <value>`",
                            source,
                        ));
                    }
                    None
                }
                (ast::ListWithRows::FieldPairs(_), Some(scope)) => {
                    let RowScope::Instance { identity, .. } = scope else {
                        unreachable!(
                            "BUG: `-> with field: value` converted under a value-list scope"
                        )
                    };
                    (!failed).then_some(ListOp::WithRow {
                        identity: identity.clone(),
                        fields,
                    })
                }
            },
            ast::ListCommand::Key { fields } => {
                let paths: Vec<DataPath> = fields
                    .iter()
                    .map(|field| DataPath {
                        segments: Vec::new(),
                        data: field.to_string(),
                    })
                    .collect();
                Some(ListOp::Key { fields: paths })
            }
        }
    }
}

static EMPTY_INSTANCE_LIST: ast::InstanceListBody = ast::InstanceListBody {
    key: None,
    rows: Vec::new(),
    option_rows: Vec::new(),
};

/// What a written row field path names on the list's row spec.
enum WrittenField {
    /// A value field; `key` is its binding key.
    Value {
        key: Vec<String>,
    },
    NotData,
    /// A `uses` instance of `identity`, bound field by field.
    Import {
        identity: Arc<SpecIdentity>,
    },
    List,
}

/// The `data x: spec list` whose rows are being built, in the spec that declares it.
struct ListSite<'s, 'a> {
    list_path: &'s DataPath,
    /// Row spec identity, shared by the rows.
    identity: &'s Arc<SpecIdentity>,
    /// Row spec template: names and types of row fields.
    template: &'s Arc<ExecutionPlan>,
    spec: &'a LemmaSpec,
    /// Repository the declaring spec is loaded under.
    repository: &'s Arc<LemmaRepository>,
    effective: &'s EffectiveDate,
    /// Bindings of the declaring spec, aliases resolved (relative keys).
    bindings: &'s DataBindings,
}

/// One row written by the plan being built: `bindings` (keys relative to the row)
/// written over `base`, the slots the row has before them.
struct RowWrite {
    identity: Arc<SpecIdentity>,
    template: Arc<ExecutionPlan>,
    origin: RowOrigin,
    base: Vec<Option<RowBinding>>,
    bindings: DataBindings,
    /// Hops naming the row from the writing plan (`i`, `staff.2`, `i.j`); rows of its
    /// list bindings are named under it.
    prefix: Vec<PathSegment>,
    /// Instances between the row's owner and the writing plan.
    hops: u32,
}

/// Aliases named by the [`BindingValue::InstanceAliases`] values of `bindings`, in
/// binding order, each once.
fn aliases_named(bindings: &DataBindings) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for (value, _) in bindings.values() {
        let BindingValue::InstanceAliases(aliases) = value else {
            continue;
        };
        for alias in aliases {
            if !names.contains(&alias.name) {
                names.push(alias.name.clone());
            }
        }
    }
    names
}

/// Source of the first binding whose [`BindingValue::InstanceAliases`] names `alias`.
fn alias_binding_source(bindings: &DataBindings, alias: &str) -> Option<Source> {
    bindings.values().find_map(|(value, source)| match value {
        BindingValue::InstanceAliases(aliases) if aliases.iter().any(|a| a.name == alias) => {
            Some(source.clone())
        }
        _ => None,
    })
}

/// Bindings under `alias` with the alias hop stripped: what the import `alias` and a
/// copy of it are written with.
fn bindings_under(bindings: &DataBindings, alias: &str) -> DataBindings {
    bindings
        .iter()
        .filter(|(key, _)| key.len() > 1 && key[0] == alias)
        .map(|(key, value)| (key[1..].to_vec(), value.clone()))
        .collect()
}

/// Literal source-row value read against the template field type.
fn source_row_literal(
    value: &Value,
    field_type: &Arc<LemmaType>,
    template: &ExecutionPlan,
) -> Result<BoundValueKind, String> {
    source_row_literal_in(
        value,
        field_type,
        template.expression_unit_index(),
        &template.resolved_types.resolved,
    )
}

fn source_row_literal_in(
    value: &Value,
    field_type: &Arc<LemmaType>,
    unit_index: &UnitIndex,
    named_types: &IndexMap<String, Arc<LemmaType>>,
) -> Result<BoundValueKind, String> {
    let typed = typed_literal_from_parsed(value, field_type, unit_index, named_types)?;
    execution_plan::validate_value_against_type(
        typed.lemma_type.as_ref(),
        &LiteralValue {
            value: typed.value.clone(),
        },
        unit_index,
        None,
    )?;
    Ok(BoundValueKind {
        value: typed.value,
        unit: typed.lemma_type.unit().map(Arc::from),
        instance_factors: None,
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

/// Whether `target` can be forced in this plan: a local data/rule of the plan, or a
/// uses-qualified member whose first hop is a `uses` Import of this plan. A bare
/// target that only exists inside a row template (list row factor), or a
/// uses-qualified target whose first hop is a list name / nested element's import,
/// is not forced here — the row context settles it.
fn scale_target_resolvable_in_graph(graph: &Graph<'_>, target: &ReferenceTarget) -> bool {
    let segments = match target {
        ReferenceTarget::Data(path) if path.segments.is_empty() => {
            return graph.data.contains_key(path);
        }
        ReferenceTarget::Rule(path) if path.segments.is_empty() => {
            return graph.rules.contains_key(path);
        }
        ReferenceTarget::Data(path) => path.segments.as_slice(),
        ReferenceTarget::Rule(path) => path.segments.as_slice(),
    };
    let Some(first) = segments.first() else {
        return false;
    };
    matches!(
        graph.data.get(&DataPath::local(first.uses.to_string())),
        Some(DataDefinition::Import { .. })
    )
}

/// Nodes of `unsorted` (those a topological sort left unordered) that lie on a cycle.
/// A node that only depends on a cycle is left unordered too, but is not part of it.
fn nodes_on_cycles<'k, K: Ord>(
    unsorted: &[&'k K],
    dependencies: impl Fn(&'k K) -> Vec<&'k K>,
) -> Vec<&'k K> {
    let remaining: BTreeSet<&K> = unsorted.iter().copied().collect();
    unsorted
        .iter()
        .copied()
        .filter(|&start| {
            let mut seen: BTreeSet<&K> = BTreeSet::new();
            let mut stack = dependencies(start);
            while let Some(node) = stack.pop() {
                if node == start {
                    return true;
                }
                if remaining.contains(node) && seen.insert(node) {
                    stack.extend(dependencies(node));
                }
            }
            false
        })
        .collect()
}

/// Scale targets of every unscaled measure in `value` of type `lemma_type`,
/// including range endpoints and the items of a value list.
fn scale_targets_of_unscaled(value: &ValueKind, lemma_type: &LemmaType) -> Vec<ReferenceTarget> {
    let mut found = Vec::new();
    let mut pending: Vec<(&ValueKind, &LemmaType)> = vec![(value, lemma_type)];
    while let Some((value, lemma_type)) = pending.pop() {
        match value {
            ValueKind::UnscaledMeasure { unit, .. } => {
                let target = lemma_type.unit_scale_target(unit).unwrap_or_else(|| {
                    panic!(
                        "BUG: unscaled measure unit '{unit}' on '{}' has no scale target",
                        lemma_type.name()
                    )
                });
                found.push(target.clone());
            }
            ValueKind::Range(left, right) => {
                pending.push((&right.value, right.lemma_type.as_ref()));
                pending.push((&left.value, left.lemma_type.as_ref()));
            }
            ValueKind::List(rows) => {
                if rows.stored().is_none() {
                    for item in rows.values().iter().rev() {
                        pending.push((&item.value, item.lemma_type.as_ref()));
                    }
                }
            }
            _ => {}
        }
    }
    found
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
    let main_spec = graph.main_spec;
    let types = find_types_by_spec(resolved_types, main_spec).unwrap_or_else(|| {
        panic!(
            "BUG: main spec '{}' typed before its types were resolved",
            main_spec.name
        )
    });
    MeasureScope {
        unit_index: &types.unit_index,
        signature_index: &types.signature_index,
        named_types: &types.resolved,
    }
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
            "Unless clause {} in rule '{}' (spec '{}') returns an anonymous intermediate with unresolved dimensions {:?}. Give the rule a named measure or ratio type with declared units, or rewrite the expression so dimensions resolve to a named type in scope.{}",
            index, rule_path.rule, spec.name, decomposition, candidates_hint
        ),
        None => format!(
            "Rule '{}' in spec '{}' returns an anonymous intermediate with unresolved dimensions {:?}. Give the rule a named measure or ratio type with declared units, or rewrite the expression so dimensions resolve to a named type in scope.{}",
            rule_path.rule, spec.name, decomposition, candidates_hint
        ),
    }
}

// =============================================================================
// Phase 1: Pure type inference (no validation, no error collection)
// =============================================================================

/// `RulePresence::Dropped` here means conversion already recorded a planning error for
/// the rule (see `Graph::rule_presence`). The dependent gets an undetermined type; the
/// error is reported once.
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
        RulePresence::Present => {
            panic!("BUG: rule type not inferred before dependent in topological order")
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
    if types.is_empty() {
        cache.typed.push(key);
    }
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

        ExpressionKind::DataPath(data_path) => Arc::new(
            infer_data_type(data_path, graph, computed_rule_types)
                .as_ref()
                .without_written_unit(),
        ),

        ExpressionKind::RulePath(rule_path) => Arc::new(
            inferred_or_dropped(graph, computed_rule_types, rule_path)
                .as_ref()
                .without_written_unit(),
        ),

        ExpressionKind::Logical { first, steps, .. } => steps.iter().fold(
            cached_expression_type(cache, expression.child(*first)),
            |left_type, step| {
                let right_type = cached_expression_type(cache, expression.child(step.operand));
                logical_connective_type(&left_type, &right_type)
            },
        ),

        ExpressionKind::LogicalNegation(operand, _) => {
            let operand_type = cached_expression_type(cache, expression.child(*operand));
            logical_not_type(&operand_type)
        }

        ExpressionKind::Comparison(left, op, right) => {
            let left_type = cached_expression_type(cache, expression.child(*left));
            let right_type = cached_expression_type(cache, expression.child(*right));
            comparison_type(&left_type, op, &right_type)
        }

        ExpressionKind::Arithmetic { first, steps } => {
            let scope = arithmetic_scope(resolved_types, graph);
            steps.iter().fold(
                cached_expression_type(cache, expression.child(*first)),
                |left_type, step| {
                    let right = expression.child(step.operand);
                    let right_type = cached_expression_type(cache, right);
                    arithmetic_step_type(left_type, &step.op, right, right_type, &scope)
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

        ExpressionKind::StepRounding { operand, step, .. } => step_rounding_type(
            &cached_expression_type(cache, expression.child(*operand)),
            step.map(|step| cached_expression_type(cache, expression.child(step)))
                .as_ref(),
        ),

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

        ExpressionKind::SpecMember { base, path, name } => {
            let base_expr = expression.child(*base);
            let base_type = cached_expression_type(cache, base_expr);
            if base_type.vetoed() {
                return Arc::new(LemmaType::veto_type());
            }
            if !base_type.is_spec() {
                return Arc::new(LemmaType::undetermined_type());
            }
            match row_member_end(graph, &base_type, path, name) {
                Some((fields, end)) => {
                    let field_type = fields.end_type(&end, computed_rule_types);
                    let field_type = match base_expr.kind() {
                        ExpressionKind::DataPath(data_path) => {
                            reanchor_member_type_through_import(graph, data_path, path, field_type)
                        }
                        _ => field_type,
                    };
                    Arc::new(field_type.as_ref().without_written_unit())
                }
                None => Arc::new(LemmaType::undetermined_type()),
            }
        }

        ExpressionKind::RowField { identity, end, .. } => row_fields(graph, identity)
            .map(|fields| {
                Arc::new(
                    fields
                        .end_type(end, computed_rule_types)
                        .as_ref()
                        .without_written_unit(),
                )
            })
            .unwrap_or_else(|| Arc::new(LemmaType::undetermined_type())),
        ExpressionKind::RowValue { list, .. } => {
            let list_type = cached_expression_type(cache, expression.child(*list));
            Arc::new(
                list_element_type(&list_type, graph, resolved_types)
                    .as_ref()
                    .without_written_unit(),
            )
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
    /// Member `parts` on rows of `identity` (the rows these fields are of), hop by hop.
    fn chain(
        &self,
        graph: &Graph<'_>,
        identity: &SpecIdentity,
        parts: &[&str],
    ) -> Option<Vec<RowHop>> {
        let templates = &graph.instance_lists.templates;
        match self {
            RowFields::Plan(plan) => instance_list::row_end_chain(
                identity,
                instance_list::RowEndView::Plan(plan),
                templates,
                parts,
            ),
            RowFields::Root(root) => {
                // Typing runs after conversion; present rules only (not main_spec
                // forward refs). Membership matches the completed plan.
                let rules: BTreeSet<RulePath> = root.rules.keys().cloned().collect();
                instance_list::row_end_chain(
                    identity,
                    instance_list::RowEndView::Root {
                        data: &root.data,
                        rules: &rules,
                    },
                    templates,
                    parts,
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
                if let Some(DataDefinition::Reference {
                    target: ReferenceTarget::Rule(rule),
                    ..
                }) = graph.data.get(path)
                {
                    return inferred_or_dropped(graph, computed_rule_types, rule);
                }
                instance_list::row_data_type(&graph.data, path)
            }
            (RowFields::Root(graph), RowEnd::Rule(rule)) => {
                inferred_or_dropped(graph, computed_rule_types, rule)
            }
        }
    }

    fn default(&self, path: &DataPath) -> Option<TypedLiteral> {
        match self {
            RowFields::Plan(plan) => instance_list::default_of(&plan.data, path),
            RowFields::Root(graph) => instance_list::default_of(&graph.data, path),
        }
    }
}

/// Fields of rows of row spec `identity`. Every instance-list identity has a template
/// plan, except a self-list's, whose rows are the spec being typed.
fn row_fields<'g>(graph: &'g Graph<'g>, identity: &SpecIdentity) -> Option<RowFields<'g>> {
    if *identity == graph.instance_lists.self_identity {
        return Some(RowFields::Root(graph));
    }
    graph
        .instance_lists
        .templates
        .get(identity)
        .map(|plan| RowFields::Plan(plan.as_ref()))
}

/// Member `path.name` read on a base of spec type: the fields of the spec of the last
/// hop and the end read there.
/// Prefix `uses` hops from a SpecMember's import base onto the field type's
/// dynamic scale references, so the consumer holds the factor in its own
/// coordinates.
fn reanchor_member_type_through_import(
    graph: &Graph<'_>,
    base_path: &DataPath,
    member_path: &[String],
    lemma_type: Arc<LemmaType>,
) -> Arc<LemmaType> {
    let Some(DataDefinition::Import { row, .. }) = graph.data.get(base_path) else {
        return lemma_type;
    };
    LemmaType::reanchor_through_import_path(
        lemma_type,
        &base_path.data,
        row.identity.as_ref(),
        member_path,
        |identity, segment| {
            let template = graph
                .instance_lists
                .templates
                .get(identity)
                .unwrap_or_else(|| {
                    panic!("BUG: SpecMember hop '{segment}' of '{identity}' has no template")
                });
            let slot = DataPath::local(segment.to_string());
            match template.data.get(&slot) {
                Some(DataDefinition::Import { row, .. }) => Some(row.identity.as_ref().clone()),
                _ => None,
            }
        },
    )
}

fn row_member_end<'g>(
    graph: &'g Graph<'g>,
    base_type: &LemmaType,
    path: &[String],
    name: &str,
) -> Option<(RowFields<'g>, RowEnd)> {
    let TypeSpecification::Spec { spec: identity } = &base_type.specifications else {
        return None;
    };
    let is_row = *identity == graph.instance_lists.self_identity
        || graph.instance_lists.templates.contains_key(identity);
    if !is_row {
        return None;
    }
    let fields = row_fields(graph, identity)
        .expect("BUG: row member identity already checked for a template");
    let mut parts: Vec<&str> = path.iter().map(String::as_str).collect();
    parts.push(name);
    let last = fields
        .chain(graph, identity, &parts)?
        .pop()
        .expect("BUG: a row end chain has a last hop");
    let fields = row_fields(graph, &last.identity)
        .expect("BUG: every hop of a row end chain has template fields");
    Some((fields, last.end))
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
        ListOp::Predicate { kind, .. } => match kind {
            ListPredKind::Any | ListPredKind::All => ListCommandResultKind::BooleanPredicate,
            ListPredKind::Where => ListCommandResultKind::KeepList,
        },
        ListOp::Sort { .. }
        | ListOp::Top { .. }
        | ListOp::With { .. }
        | ListOp::WithRow { .. }
        | ListOp::Key { .. } => ListCommandResultKind::KeepList,
    };
    let first_argument = match op {
        ListOp::Reduce {
            argument: Some(argument),
            ..
        } => Some(cached_expression_type(cache, node.child(*argument))),
        _ => None,
    };
    let result = list_command_result_type(
        &list_type,
        kind,
        first_argument.as_ref(),
        &arithmetic_scope(resolved_types, graph),
    );
    if let ListOp::Key { fields } = op {
        refine_list_key_type(&result, fields)
    } else {
        result
    }
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
/// Returns `LemmaType::undetermined_type()` when the data cannot be found. A `uses` slot
/// is an instance of its spec.
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
    let Some(definition) = graph.data.get(data_path) else {
        return Arc::new(LemmaType::undetermined_type());
    };
    match definition {
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
        DataDefinition::Import { row, .. } => {
            Arc::new(LemmaType::primitive(TypeSpecification::Spec {
                spec: row.identity.as_ref().clone(),
            }))
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

fn check_logical_operands(
    graph: &Graph,
    connective: ast::LogicalConnective,
    left_type: &LemmaType,
    right_type: &LemmaType,
    source: &Source,
) -> Result<(), Vec<Error>> {
    if left_type.vetoed() || right_type.vetoed() {
        return Ok(());
    }
    if left_type.is_undetermined() || right_type.is_undetermined() {
        return Ok(());
    }
    if logical_connective_type(left_type, right_type).is_undetermined() {
        let side = if !left_type.is_boolean() {
            ("left", left_type)
        } else {
            ("right", right_type)
        };
        return Err(vec![engine_error_at_graph(
            graph,
            source,
            format!(
                "Logical {} requires boolean {} operand, got {}",
                connective.keyword().to_ascii_uppercase(),
                side.0,
                side.1
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
    if operand_type.vetoed() || operand_type.is_undetermined() {
        return Ok(());
    }
    if logical_not_type(operand_type).is_undetermined() {
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
    if value_type.is_undetermined() || range_type.is_undetermined() {
        return Ok(());
    }
    if range_containment_type(value_type, range_type).is_undetermined() {
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
    if left_type.is_undetermined() || right_type.is_undetermined() {
        return Ok(());
    }
    if let Some(error) = spec_value_not_operable(graph, left_type, right_type, source) {
        return Err(vec![error]);
    }
    if comparison_type(left_type, op, right_type).is_undetermined() {
        if (left_type.is_boolean() && right_type.is_boolean())
            || (left_type.is_text() && right_type.is_text())
        {
            let kind = if left_type.is_boolean() {
                "booleans"
            } else {
                "text"
            };
            return Err(vec![engine_error_at_graph(
                graph,
                source,
                format!("Can only use 'is' and 'is not' with {kind} (got {op})"),
            )]);
        }
        if left_type.is_measure()
            && right_type.is_measure()
            && !left_type.same_measure_family(right_type)
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
        return Err(vec![engine_error_at_graph(
            graph,
            source,
            format!("Cannot compare {} with {}", left_type, right_type),
        )]);
    }
    Ok(())
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
            if rational_is_zero(number) {
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

enum IntegerExponent {
    NotLiteral,
    Value(i32),
    OutOfRange,
}

fn integer_exponent_literal(expr: ExprRef<'_, ExpressionKind>) -> IntegerExponent {
    let ExpressionKind::Literal(literal) = expr.kind() else {
        return IntegerExponent::NotLiteral;
    };
    let ValueKind::Number(number) = &literal.value else {
        return IntegerExponent::NotLiteral;
    };
    if !number.is_integer() {
        return IntegerExponent::NotLiteral;
    }
    match number.numer_to_i32() {
        Some(exponent) => IntegerExponent::Value(exponent),
        None => IntegerExponent::OutOfRange,
    }
}

/// Result type of one arithmetic step. A measure to an integer literal uses
/// [`quantity_integer_power_type`]; every other pair uses the shared kernel.
fn arithmetic_step_type(
    left_type: Arc<LemmaType>,
    op: &ArithmeticComputation,
    right: ExprRef<'_, ExpressionKind>,
    right_type: Arc<LemmaType>,
    scope: &MeasureScope<'_>,
) -> Arc<LemmaType> {
    if *op == ArithmeticComputation::Power && left_type.is_measure() {
        match integer_exponent_literal(right) {
            IntegerExponent::Value(exponent) => {
                if measure_power_scale(left_type.as_ref(), exponent).is_none() {
                    return Arc::new(LemmaType::undetermined_type());
                }
                return quantity_integer_power_type(&left_type, exponent, scope);
            }
            IntegerExponent::OutOfRange => return Arc::new(LemmaType::undetermined_type()),
            IntegerExponent::NotLiteral => {}
        }
    }
    compute_arithmetic_result_type(left_type, op, right_type, scope)
}

fn measure_power_scale(left: &LemmaType, exponent: i32) -> Option<BaseMeasureVector> {
    let decomposition = left
        .measure_type_decomposition()
        .expect("BUG: measure power requires a resolved decomposition");
    scale_decomposition(decomposition, exponent)
}

fn measure_power_cap_error(
    graph: &Graph,
    left_type: &LemmaType,
    op: &ArithmeticComputation,
    right: ExprRef<'_, ExpressionKind>,
    source: &Source,
) -> Option<Error> {
    if *op != ArithmeticComputation::Power || !left_type.is_measure() {
        return None;
    }
    let overflow = match integer_exponent_literal(right) {
        IntegerExponent::OutOfRange => true,
        IntegerExponent::Value(exponent) => measure_power_scale(left_type, exponent).is_none(),
        IntegerExponent::NotLiteral => false,
    };
    if !overflow {
        return None;
    }
    Some(engine_error_at_graph(
        graph,
        source,
        format!(
            "Cannot apply '^' to {} and number: a resulting unit exponent exceeds the maximum of {}.",
            left_type.name(),
            MAX_UNIT_EXPONENT
        ),
    ))
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
    right: ExprRef<'_, ExpressionKind>,
    scope: &MeasureScope<'_>,
    source: &Source,
) -> Result<(), Vec<Error>> {
    if left_type.vetoed() || right_type.vetoed() {
        return Ok(());
    }
    if left_type.is_undetermined() || right_type.is_undetermined() {
        return Ok(());
    }
    if let Some(error) = spec_value_not_operable(graph, left_type, right_type, source) {
        return Err(vec![error]);
    }

    let result = arithmetic_step_type(
        Arc::new(left_type.clone()),
        operator,
        right,
        Arc::new(right_type.clone()),
        scope,
    );
    if result.is_undetermined() {
        let message = if matches!(operator, ArithmeticComputation::Subtract)
            && left_type.is_date()
            && right_type.is_date()
        {
            "Cannot subtract dates. Use a date range instead: `start...end as day` or `start...end as year`."
                .to_string()
        } else if matches!(operator, ArithmeticComputation::Subtract)
            && left_type.is_time()
            && right_type.is_time()
        {
            "Cannot subtract times. Use a datetime range instead: `start...end as hour` or `start...end as second`."
                .to_string()
        } else if matches!(
            operator,
            ArithmeticComputation::Add | ArithmeticComputation::Subtract
        ) && left_type.is_measure()
            && right_type.is_measure()
            && !left_type.is_anonymous_measure()
            && !right_type.is_anonymous_measure()
            && !left_type.same_measure_family(right_type)
            && !left_type.compatible_with_anonymous_measure(right_type)
            && !right_type.compatible_with_anonymous_measure(left_type)
        {
            format!(
                "Cannot {} unrelated measure types: {} and {}.",
                if matches!(operator, ArithmeticComputation::Add) {
                    "add"
                } else {
                    "subtract"
                },
                left_type.name(),
                right_type.name()
            )
        } else if matches!(
            operator,
            ArithmeticComputation::Multiply | ArithmeticComputation::Divide
        ) && left_type.is_measure()
            && right_type.is_measure()
            && measure_combine_exceeds_exponent_cap(left_type, right_type, operator)
        {
            format!(
                "Cannot apply '{}' to {} and {}: a resulting unit exponent exceeds the maximum of {}.",
                operator,
                left_type.name(),
                right_type.name(),
                MAX_UNIT_EXPONENT
            )
        } else if matches!(
            operator,
            ArithmeticComputation::Add | ArithmeticComputation::Subtract
        ) && ((left_type.is_ratio()
            && (right_type.is_number()
                || right_type.is_measure()
                || right_type.is_duration_like()
                || right_type.is_calendar_like()))
            || (right_type.is_ratio()
                && (left_type.is_number()
                    || left_type.is_measure()
                    || left_type.is_duration_like()
                    || left_type.is_calendar_like())))
        {
            format!(
                "Cannot apply '{}' to {} and {}. \
                 Adding or subtracting a ratio is ambiguous; \
                 scale explicitly, e.g. 'price - discount * price' \
                 or 'price * (100% - discount)'.",
                operator,
                left_type.name(),
                right_type.name(),
            )
        } else if matches!(operator, ArithmeticComputation::Modulo)
            && (left_type.is_ratio()
                || left_type.is_measure()
                || left_type.is_duration_like()
                || left_type.is_calendar_like()
                || right_type.is_ratio()
                || right_type.is_measure()
                || right_type.is_duration_like()
                || right_type.is_calendar_like())
        {
            format!(
                "Cannot apply modulo to {} and {}. `mod` is defined for numbers only.",
                left_type.name(),
                right_type.name(),
            )
        } else {
            format!(
                "Cannot apply '{}' to {} and {}.",
                operator,
                left_type.name(),
                right_type.name(),
            )
        };
        return Err(vec![engine_error_at_graph(graph, source, message)]);
    }
    Ok(())
}

fn measure_combine_exceeds_exponent_cap(
    left_type: &LemmaType,
    right_type: &LemmaType,
    operator: &ArithmeticComputation,
) -> bool {
    let (Some(left), Some(right)) = (
        left_type.measure_type_decomposition(),
        right_type.measure_type_decomposition(),
    ) else {
        return false;
    };
    combine_decompositions(
        left,
        right,
        matches!(operator, ArithmeticComputation::Multiply),
    )
    .is_none()
}

/// `operand -> round` needs a number. `operand -> round step` needs a positive literal step
/// of the operand's kind.
fn check_step_rounding(
    graph: &Graph,
    mode: semantics::RoundingMode,
    operand: ExprRef<'_, ExpressionKind>,
    operand_type: &LemmaType,
    step: Option<ExprRef<'_, ExpressionKind>>,
    step_type: Option<&Arc<LemmaType>>,
    source: &Source,
) -> Result<(), Vec<Error>> {
    if operand_type.vetoed() || operand_type.is_undetermined() {
        return Ok(());
    }
    let Some(step) = step else {
        if step_rounding_type(&Arc::new(operand_type.clone()), None).is_undetermined() {
            let label = expression_suggestion_label(operand);
            if operand_type.is_anonymous_measure() {
                let decomposition = operand_type
                    .measure_type_decomposition()
                    .expect("BUG: anonymous measure carries a decomposition");
                return Err(vec![engine_error_at_graph(
                    graph,
                    source,
                    format!(
                        "Cannot use '{mode}' on '{label}'. Round to a step instead: write \
                         '{label} -> {mode}' followed by a step in a unit with dimensions {decomposition:?}"
                    ),
                )]);
            }
            if operand_type.is_measure() || operand_type.is_ratio() {
                let kind = if operand_type.is_measure() {
                    "measure"
                } else {
                    "ratio"
                };
                let unit = first_unit_suggestion(operand_type);
                return Err(vec![engine_error_at_graph(
                    graph,
                    source,
                    format!(
                        "Cannot use '{mode}' on {kind} '{}'. Round to a step instead, for example \
                         '{label} -> {mode} 1 {unit}'",
                        operand_type.name()
                    ),
                )]);
            }
            return Err(vec![engine_error_at_graph(
                graph,
                source,
                format!(
                    "Cannot use '{mode}' on {}; write '{label} -> {mode}' only for a number, \
                     or give a step of the same kind",
                    operand_type.name()
                ),
            )]);
        }
        return Ok(());
    };
    let step_type = step_type
        .expect("BUG: step carries a type when present")
        .as_ref();
    if step_type.vetoed() || step_type.is_undetermined() {
        return Ok(());
    }
    let ExpressionKind::Literal(literal) = step.kind() else {
        return Err(vec![engine_error_at_graph(
            graph,
            source,
            format!(
                "The step of '-> {mode}' must be a literal, not '{}'",
                expression_suggestion_label(step)
            ),
        )]);
    };
    if step_rounding_type(
        &Arc::new(operand_type.clone()),
        Some(&Arc::new(step_type.clone())),
    )
    .is_undetermined()
    {
        let hint = match (
            operand_type.is_anonymous_measure(),
            operand_type.measure_unit_names(),
        ) {
            (false, Some(units)) if !units.is_empty() => format!(
                ". Write the step in a unit of {}, for example '-> {mode} 1 {}'",
                operand_type.name(),
                units[0]
            ),
            _ => String::new(),
        };
        return Err(vec![engine_error_at_graph(
            graph,
            source,
            format!(
                "Cannot round {} to a step of {}{hint}",
                operand_type.name(),
                step_type.name()
            ),
        )]);
    }
    let positive = match &literal.value {
        ValueKind::Number(step)
        | ValueKind::Ratio(step)
        | ValueKind::Measure(step)
        | ValueKind::UnscaledMeasure {
            magnitude: step, ..
        } => step.numer_is_positive(),
        other => unreachable!("BUG: step type is number, ratio or measure, got {other:?}"),
    };
    if !positive {
        return Err(vec![engine_error_at_graph(
            graph,
            source,
            format!("The step of '-> {mode}' must be greater than zero"),
        )]);
    }
    Ok(())
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

fn suggestion_label(mut pending: Vec<SuggestionPiece<'_>>) -> String {
    use SuggestionPiece as Piece;
    let mut out = String::new();
    while let Some(piece) = pending.pop() {
        match piece {
            Piece::Text(text) => out.push_str(&text),
            Piece::Node(node) => match node.kind() {
                ExpressionKind::DataPath(p) => out.push_str(&p.data),
                ExpressionKind::RulePath(p) => out.push_str(&p.rule),
                ExpressionKind::SpecMember { base, path, name } => {
                    pending.push(Piece::Text(format!(".{name}")));
                    for segment in path.iter().rev() {
                        pending.push(Piece::Text(format!(".{segment}")));
                    }
                    pending.push(Piece::Node(node.child(*base)));
                }
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
        .or_else(|| source_type.ratio_unit_names())
        .and_then(|names| names.first().map(|u| (*u).to_string()))
        .unwrap_or_else(|| "<unit>".to_string())
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
    if source_type.vetoed() || source_type.is_undetermined() {
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

    // Planning-only obligations the typing kernel cannot express (unit index).
    if let SemanticConversionTarget::Unit {
        unit_name,
        owning_type,
    } = target
    {
        if source_type.is_number() {
            if !owning_type_declares_unit_name(owning_type, unit_name) {
                return Err(vec![engine_error_at_graph(
                    graph,
                    source_loc,
                    format!("Unknown unit '{unit_name}' in spec '{}'.", spec.name),
                )]);
            }
        } else if source_type.is_date_range()
            || source_type.is_time_range()
            || source_type.is_measure_range()
            || source_type.is_ratio_range()
        {
            if !is_valid_range_span_unit(source_type, unit_name, unit_index) {
                return Err(vec![engine_error_at_graph(
                    graph,
                    source_loc,
                    format!(
                        "Cannot convert {} span to unit '{unit_name}'.",
                        source_type.name()
                    ),
                )]);
            }
        } else if source_type.is_calendar_like() {
            if !owning_type_declares_unit_name(owning_type, unit_name) {
                return Err(vec![engine_error_at_graph(
                    graph,
                    source_loc,
                    format!("Cannot convert calendar to unit '{unit_name}'."),
                )]);
            }
        } else if source_type.is_ratio() && !owning_type_declares_unit_name(owning_type, unit_name)
        {
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
                    "Unknown unit '{unit_name}' for ratio type '{}'. Valid units: {valid}",
                    owning_type.name()
                ),
            )]);
        }
    }

    if unit_conversion_type(&Arc::new(source_type.clone()), target).is_undetermined() {
        let message = match target {
            SemanticConversionTarget::Type(ConversionTypeKind::Time) => {
                format!("Cannot convert {} to time.", source_type.name())
            }
            SemanticConversionTarget::Type(ConversionTypeKind::Boolean) => {
                format!("Cannot convert {} to boolean.", source_type.name())
            }
            SemanticConversionTarget::Type(ConversionTypeKind::Number) => {
                if source_type.is_date_range()
                    || source_type.is_time_range()
                    || source_type.is_measure_range()
                {
                    format!(
                        "Cannot use 'as number' on a {}. \
                         Express the span in a unit first, for example '<range> as <unit> as number'.",
                        source_type.name()
                    )
                } else if source_type.is_anonymous_measure() {
                    let decomp = source_type
                        .measure_type_decomposition()
                        .expect("BUG: anonymous measure carries a decomposition");
                    format!(
                        "Cannot use 'as number' to strip an anonymous intermediate with unresolved \
                         dimensions {decomp:?}. Ensure all dimensions cancel before converting to number."
                    )
                } else if source_type.is_measure() && source_type.unit().is_none() {
                    let expr_label = expression_suggestion_label(source);
                    let first_unit = first_unit_suggestion(source_type);
                    format!(
                        "Cannot use 'as number' on measure '{}' without a unit. \
                         Express it in a unit first, for example '{expr_label} as {first_unit} as number'.",
                        source_type.name()
                    )
                } else {
                    format!("Cannot convert {} to number.", source_type.name())
                }
            }
            SemanticConversionTarget::Unit { unit_name, .. } => {
                if source_type.is_measure() && source_type.unit().is_none() {
                    let expr_label = expression_suggestion_label(source);
                    let first_unit = first_unit_suggestion(source_type);
                    format!(
                        "Cannot convert '{}' to '{unit_name}' (different measure families). \
                         Express the source unit first, for example '{expr_label} as {first_unit} as {unit_name}'.",
                        source_type.name()
                    )
                } else if source_type.is_calendar_like() {
                    format!(
                        "Cannot convert calendar to unit '{unit_name}' (different measure families)."
                    )
                } else {
                    format!(
                        "Cannot convert {} to unit '{unit_name}'.",
                        source_type.name()
                    )
                }
            }
            SemanticConversionTarget::Type(target_kind) => {
                format!(
                    "Cannot convert {} to {:?}.",
                    source_type.name(),
                    target_kind
                )
            }
        };
        return Err(vec![engine_error_at_graph(graph, source_loc, message)]);
    }
    Ok(())
}
fn check_mathematical_operand(
    graph: &Graph,
    op: &semantics::MathematicalComputation,
    operand_type: &LemmaType,
    scope: &MeasureScope<'_>,
    source: &Source,
) -> Result<(), Vec<Error>> {
    if operand_type.vetoed() || operand_type.is_undetermined() {
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
    if math_op_type(op, &Arc::new(operand_type.clone()), scope).is_undetermined() {
        let message = if operand_type.is_measure() {
            format!(
                "Mathematical function '{op}' cannot be applied to {operand_type}; use 'as number' first"
            )
        } else {
            format!("Mathematical function '{op}' requires number operand, got {operand_type}")
        };
        return Err(vec![engine_error_at_graph(graph, source, message)]);
    }
    Ok(())
}

/// Check that all rule references in the graph point to existing rules.
fn check_all_rule_references_exist(graph: &Graph) -> Result<(), Vec<Error>> {
    let mut errors = Vec::new();
    for (rule_path, rule_node) in &graph.rules {
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
    for rule_path in graph.rules.keys() {
        let data_path = DataPath::new(rule_path.segments.clone(), rule_path.rule.clone());
        if graph.data.contains_key(&data_path) {
            let rule_node = graph.rules.get(rule_path).unwrap_or_else(|| {
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

/// Check that a data reference names data of this spec. A `uses` slot is the instance.
fn check_data_reference(
    data_path: &DataPath,
    graph: &Graph,
    data_source: &Source,
) -> Result<(), Vec<Error>> {
    if graph.data.contains_key(data_path) {
        return Ok(());
    }
    Err(vec![engine_error_at_graph(
        graph,
        data_source,
        format!("Unknown data reference '{}'", data_path),
    )])
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
    // Fill the typing cache once; check_steps only reads it (no second ruleset).
    let _ = infer_expression_type(expression, graph, inferred_types, resolved_types, cache);
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
    cache: &InferExpressionTypeCache,
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

        ExpressionKind::SpecMember { base, name, path } => {
            let expr_source = expression
                .source()
                .expect("BUG: expression missing source in check_expression");
            checks.visit(base);
            let base_type = cache.get(expression.child(*base));
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
            } else if base_type.is_spec() && row_member_end(graph, &base_type, path, name).is_none()
            {
                checks.push(engine_error_at_graph(
                    graph,
                    expr_source,
                    format!(
                        "no data, rule or uses '{name}' on spec {}",
                        base_type.name()
                    ),
                ));
            }
        }

        ExpressionKind::RowField { .. } | ExpressionKind::RowValue { .. } => {}

        ExpressionKind::Logical {
            connective,
            first,
            steps,
        } => {
            checks.visit(first);
            let mut left_type = cache.get(expression.child(*first));
            for step in steps {
                checks.visit(&step.operand);
                let right_type = cache.get(expression.child(step.operand));
                checks.collect(check_logical_operands(
                    graph,
                    *connective,
                    &left_type,
                    &right_type,
                    &step.source_location,
                ));
                left_type = logical_connective_type(&left_type, &right_type);
            }
        }

        ExpressionKind::LogicalNegation(operand, _) => {
            checks.visit(operand);

            let operand_type = cache.get(expression.child(*operand));
            let expr_source = expression
                .source()
                .expect("BUG: expression missing source in check_expression");
            checks.collect(check_logical_operand(graph, &operand_type, expr_source));
        }

        ExpressionKind::Comparison(left, op, right) => {
            checks.visit(left);
            checks.visit(right);

            let left_type = cache.get(expression.child(*left));
            let right_type = cache.get(expression.child(*right));
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
            let mut left_type = cache.get(expression.child(*first));
            for step in steps.iter() {
                let operator = &step.op;
                let right = &step.operand;
                checks.visit(right);
                let right_expr = expression.child(*right);
                let right_type = cache.get(right_expr);
                let expr_source = &step.source_location;
                checks.collect(check_arithmetic_types(
                    graph,
                    &left_type,
                    &right_type,
                    operator,
                    right_expr,
                    &scope,
                    expr_source,
                ));
                checks.collect(arithmetic_plan_time_exactness_planning_errors(
                    graph,
                    right_expr,
                    &left_type,
                    &right_type,
                    operator,
                    expr_source,
                ));
                if let Some(error) =
                    measure_power_cap_error(graph, &left_type, operator, right_expr, expr_source)
                {
                    checks.push(error);
                }
                left_type =
                    arithmetic_step_type(left_type, operator, right_expr, right_type, &scope);
            }
        }

        ExpressionKind::UnitConversion(source_expression, target) => {
            checks.visit(source_expression);

            let source_type = cache.get(expression.child(*source_expression));
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

            let operand_type = cache.get(expression.child(*operand));
            let expr_source = expression
                .source()
                .expect("BUG: expression missing source in check_expression");
            checks.collect(check_mathematical_operand(
                graph,
                op,
                &operand_type,
                &arithmetic_scope(resolved_types, graph),
                expr_source,
            ));
        }

        ExpressionKind::StepRounding {
            operand,
            mode,
            step,
        } => {
            checks.visit(operand);
            if let Some(step) = step {
                checks.visit(step);
            }
            let operand_type = cache.get(expression.child(*operand));
            let step_type = step.map(|step| cache.get(expression.child(step)));
            let expr_source = expression
                .source()
                .expect("BUG: expression missing source in check_expression");
            checks.collect(check_step_rounding(
                graph,
                *mode,
                expression.child(*operand),
                &operand_type,
                step.map(|step| expression.child(step)),
                step_type.as_ref(),
                expr_source,
            ));
        }

        ExpressionKind::RangeBound(bound, operand) => {
            checks.visit(operand);

            let operand_type = cache.get(expression.child(*operand));
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

            let date_type = cache.get(expression.child(*date_expr));
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

            let date_type = cache.get(expression.child(*date_expr));
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

            let left_type = cache.get(expression.child(*left));
            let right_type = cache.get(expression.child(*right));
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

            let offset_type = cache.get(expression.child(*offset_expr));
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

            let value_type = cache.get(expression.child(*value));
            let range_type = cache.get(expression.child(*range));
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
                let condition_type = cache.get(expression.child(*condition));
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
            let list_type = cache.get(expression.child(*list));
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
            let typing = TypingScope {
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
                &typing,
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
    typing: &TypingScope<'_>,
    source: &Source,
) -> Vec<CheckStep> {
    let TypingScope {
        graph,
        resolved_types,
        ..
    } = *typing;
    if let TypeSpecification::List { element, key, .. } = &list_type.specifications {
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
        ListOp::Predicate { form, expr, .. } => {
            checks.visit(expr);
            if *form == ast::ListPredicateForm::Instance {
                checks.push(engine_error_at_graph(
                    graph,
                    source,
                    "value-list `any`/`all`/`where` take a comparison (`>= 100 eur`), not a field expression"
                        .to_string(),
                ));
            }
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
            checks.collect(check_top_count(node.child(*n), graph, source, typing.cache));
        }
        ListOp::With { rows } => {
            for row in rows {
                checks.visit(row);
            }
        }
        ListOp::WithRow { .. } => {
            unreachable!("BUG: `-> with field: value` converted on a value list")
        }
        ListOp::Key { .. } => {
            checks.push(engine_error_at_graph(
                graph,
                source,
                "`-> key` is for instance lists".to_string(),
            ));
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
    source: &Source,
    cache: &InferExpressionTypeCache,
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
            let n_type = cache.get(n);
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
    typing: &TypingScope<'_>,
    source: &Source,
) -> Result<(), Vec<Error>> {
    let graph = typing.graph;
    if element_type.is_undetermined() || element_type.vetoed() {
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
        ListOp::Predicate { expr, .. } => {
            let predicate_type = typing.infer(node.child(*expr));
            if predicate_type.is_boolean()
                || predicate_type.is_undetermined()
                || predicate_type.vetoed()
            {
                Ok(())
            } else {
                error(format!(
                    "list predicate needs a boolean expression, got {}",
                    predicate_type.name()
                ))
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
    cache: &'t InferExpressionTypeCache,
}

impl TypingScope<'_> {
    fn infer(&self, expression: ExprRef<'_, ExpressionKind>) -> Arc<LemmaType> {
        self.cache.get(expression)
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
    typing: &TypingScope<'_>,
    source: &Source,
) -> Vec<CheckStep> {
    let mut steps = Vec::new();
    let mut checks = CheckSteps::new(&mut steps);
    let Some(template) = row_fields(typing.graph, identity) else {
        checks.collect(Err(vec![typing.error(
            source,
            format!(
                "instance list of `{}` needs `uses {}` before `-> key` or `-> with`",
                identity.spec, identity.spec
            ),
        )]));
        checks.finish();
        return steps;
    };
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
        ListOp::Predicate { kind, form, expr } => {
            checks.visit(expr);
            if *form == ast::ListPredicateForm::Value {
                checks.collect(Err(vec![typing.error(
                    source,
                    format!(
                        "`{}` on `{row_spec}` rows takes a row expression such as `price > 3 eur`, not a bare comparison",
                        kind.keyword()
                    ),
                )]));
            }
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
                source,
                typing.cache,
            ));
        }
        ListOp::With { rows } => {
            for row in rows {
                checks.visit(row);
                let value_type = typing.infer(node.child(*row));
                if value_type.is_undetermined() || value_type.vetoed() {
                    continue;
                }
                let ok = match &value_type.specifications {
                    TypeSpecification::Spec { spec } if spec == identity => true,
                    TypeSpecification::List { element, .. } => {
                        matches!(
                            &element.specifications,
                            TypeSpecification::Spec { spec } if spec == identity
                        )
                    }
                    _ => false,
                };
                if !ok {
                    checks.collect(Err(vec![typing.error(
                        source,
                        format!(
                            "`-> with` on `{row_spec}` rows takes `field: value` pairs, a `{row_spec}` instance, or a `{row_spec}` list"
                        ),
                    )]));
                }
            }
        }
        ListOp::WithRow { fields, .. } => {
            let mut fields_typed = true;
            for (path, value) in fields {
                checks.visit(value);
                let field_type =
                    template.end_type(&RowEnd::Data(path.clone()), typing.inferred_types);
                let value_type = typing.infer(node.child(*value));
                if !value_type.is_undetermined()
                    && !value_type.vetoed()
                    && !field_type.same_value_type(value_type.as_ref())
                {
                    fields_typed = false;
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
                for key_field in key_fields {
                    let path = &key_field.path;
                    let written = fields.iter().any(|(written, _)| *written == *path);
                    if !written && template.default(path).is_none() {
                        checks.collect(Err(vec![typing.error(
                            source,
                            format!("`-> with` row must set key field `{}`", key_field.name),
                        )]));
                    }
                }
            }
            if fields_typed {
                checks.collect(check_appended_row_key(
                    list, node, fields, key, typing, source,
                ));
            }
        }
        ListOp::Key { fields } => {
            checks.collect(check_key_list_spine(list, typing, source));
            for path in fields {
                assert!(
                    path.segments.is_empty(),
                    "BUG: `-> key` fields are names of the row spec"
                );
                let chain = template.chain(typing.graph, identity, &[path.data.as_str()]);
                let end = chain.as_deref().map(|chain| match chain {
                    [hop] => &hop.end,
                    _ => unreachable!("BUG: a one-part chain has one hop"),
                });
                let row_data = match &template {
                    RowFields::Plan(plan) => &plan.data,
                    RowFields::Root(graph) => &graph.data,
                };
                match end {
                    Some(RowEnd::Data(data))
                        if matches!(row_data.get(data), Some(DataDefinition::Import { .. })) =>
                    {
                        checks.collect(Err(vec![typing.error(
                            source,
                            format!(
                                "`-> key` field `{}` must be data on `{row_spec}`, not a uses instance",
                                path.input_key()
                            ),
                        )]))
                    }
                    Some(RowEnd::Data(_)) => {}
                    Some(RowEnd::Rule(_)) => checks.collect(Err(vec![typing.error(
                        source,
                        format!(
                            "`-> key` field `{}` must be data on `{row_spec}`, not a rule",
                            path.input_key()
                        ),
                    )])),
                    None => checks.collect(Err(vec![typing.error(
                        source,
                        format!(
                            "`-> key` field `{}` is not a member of `{row_spec}`",
                            path.input_key()
                        ),
                    )])),
                }
            }
        }
    }
    checks.finish();
    steps
}

/// `-> key` may only refine a bare list / prior `key`-free construction spine.
/// It must not follow `with` or a query command, and must not appear twice.
fn check_key_list_spine(
    list: ExprRef<'_, ExpressionKind>,
    typing: &TypingScope<'_>,
    source: &Source,
) -> Result<(), Vec<Error>> {
    match list.kind() {
        ExpressionKind::ListCommand { op, .. } => match op {
            ListOp::Key { .. } => Err(vec![typing.error(
                source,
                "`-> key` appears twice; list every key field in one `-> key`",
            )]),
            ListOp::With { .. } | ListOp::WithRow { .. } => Err(vec![typing.error(
                source,
                "`-> key` must come before the first `-> with`",
            )]),
            ListOp::Predicate { .. }
            | ListOp::Sort { .. }
            | ListOp::Top { .. }
            | ListOp::Reduce { .. }
            | ListOp::Find { .. }
            | ListOp::First
            | ListOp::Last
            | ListOp::Count => Err(vec![typing.error(
                source,
                "`-> key` declares list addressing; write it on the list before query commands such as `where`",
            )]),
        },
        _ => Ok(()),
    }
}

/// `-> find` on instance rows: one row number on a numbered list, one value per key field otherwise.
fn check_instance_find(
    keys: &[ExprId],
    node: ExprRef<'_, ExpressionKind>,
    key: &ListKey,
    template: RowFields<'_>,
    typing: &TypingScope<'_>,
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
        ListKey::Fields(fields) => fields.as_slice(),
    };
    if keys.len() != fields.len() {
        return Err(vec![typing.error(
            source,
            format!(
                "`-> find` takes {} key fields ({}), got {}",
                fields.len(),
                fields
                    .iter()
                    .map(|field| field.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                keys.len()
            ),
        )]);
    }
    let mut errors = Vec::new();
    for (key_expression, key_field) in keys.iter().zip(fields) {
        let key_expression = node.child(*key_expression);
        let end = RowEnd::Data(key_field.path.clone());
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
                    "key field `{}` is {}, got {}: {}",
                    key_field.name,
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
        } => format!(
            "{} (key {})",
            lemma_type.name(),
            fields
                .iter()
                .map(|field| field.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
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
    let Some(list_source) = typing.graph.instance_lists.sources.get(list_path) else {
        return Ok(());
    };
    let Some(template) = row_fields(typing.graph, &list_source.identity) else {
        return Ok(());
    };
    let key_paths: Vec<DataPath> = key_fields
        .iter()
        .map(|key_field| key_field.path.clone())
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
    let store = &list_source.store;
    if store.key.is_empty() {
        // The store's keys failed to check, which is an error already.
        return Ok(());
    }
    let stored = store.rows.len();
    let key_of = |row: usize| {
        if row == stored {
            appended.clone()
        } else {
            store.key_values(row)
        }
    };
    let built = KeyIndex::build(store.key_names(), stored + 1, |row, field| {
        if row == stored {
            &appended[field].value
        } else {
            store.key_value(row, field)
        }
    });
    match built {
        Ok(_) => Ok(()),
        Err((fields, conflict)) => {
            let message = conflict.message(&fields, |row| {
                let label = if row == stored {
                    "the appended row".to_string()
                } else {
                    format!("row {}", row + 1)
                };
                (label, key_of(row))
            });
            Err(vec![typing.error(source, message)])
        }
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
            .rules
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
                if !decomp.is_empty() {
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
                    if !decomp.is_empty() {
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
                        let Some(rule_node) = graph.rules.get(rule_path) else {
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
        if let Some(rule_node) = graph.rules.get_mut(&rule_path) {
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
    let mut seeds: HashMap<RulePath, Arc<LemmaType>> = HashMap::new();

    // 1. On-demand seed every recursive rule (and any locals their seeds name).
    for rule_path in rule_order {
        let rule_node = graph
            .rules
            .get(rule_path)
            .expect("BUG: rule from topological sort not in graph");
        if !rule_node.recursive || rule_node.branches.is_empty() {
            continue;
        }
        ensure_seed(rule_path, graph, &mut computed_types, resolved_types, cache);
        match computed_types.get(rule_path) {
            Some(seed) if !seed.vetoed() && !seed.is_undetermined() => {
                seeds.insert(rule_path.clone(), Arc::clone(seed));
            }
            _ => {
                errors.push(reference_error(
                    graph.main_spec,
                    &rule_node.source,
                    format!(
                        "recursive rule `{}` needs a non-recursive part to determine its type",
                        rule_path.rule
                    ),
                ));
                computed_types.insert(rule_path.clone(), Arc::new(LemmaType::undetermined_type()));
            }
        }
        cache.clear();
    }

    // 2. Infer every rule in same-instance topo (recursive full bodies against seeds).
    // `written_at` is the write count after a rule's type was last written, `inferred_at`
    // the write count when a rule was inferred; pass-1 entries were written at 0.
    let mut writes = 0usize;
    let mut written_at: HashMap<&RulePath, usize> = HashMap::new();
    let mut inferred_at: HashMap<&RulePath, usize> = HashMap::new();
    let mut overwrote_pass_one_entry = false;
    for rule_path in rule_order {
        let rule_node = graph
            .rules
            .get(rule_path)
            .expect("BUG: rule from topological sort not in graph");
        if rule_node.branches.is_empty() {
            continue;
        }
        if rule_node.recursive && !seeds.contains_key(rule_path) {
            continue;
        }
        inferred_at.insert(rule_path, writes);
        let rule_type =
            infer_rule_body_type(rule_node, graph, &computed_types, resolved_types, cache);
        if let Some(seed) = seeds.get(rule_path) {
            let determined = !rule_type.vetoed() && !rule_type.is_undetermined();
            if determined && !rule_type.same_value_type(seed) {
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
        writes += 1;
        written_at.insert(rule_path, writes);
        if computed_types
            .insert(rule_path.clone(), rule_type)
            .is_some()
        {
            overwrote_pass_one_entry = true;
        }
    }

    // 3. Re-infer non-recursive rules against full recursive types (not seeds). A rule's
    // type is a function of the rule types it reads; it changes only if one of them was
    // written after the rule was inferred. In pass 2 a rule reads only types already
    // written, so that needs a pass-1 entry that pass 2 overwrote later.
    if overwrote_pass_one_entry {
        for rule_path in rule_order {
            let rule_node = graph
                .rules
                .get(rule_path)
                .expect("BUG: rule from topological sort not in graph");
            if rule_node.recursive || rule_node.branches.is_empty() {
                continue;
            }
            let inferred = *inferred_at
                .get(rule_path)
                .expect("BUG: pass 2 inferred every non-recursive rule with branches");
            let read_a_later_write = rule_node
                .branches
                .iter()
                .flat_map(|(_, result)| {
                    local_rule_names_in(rule_node.expression.at(*result), graph)
                })
                .any(|read| written_at.get(&read).is_some_and(|at| *at > inferred));
            if !read_a_later_write {
                continue;
            }
            cache.clear();
            let rule_type =
                infer_rule_body_type(rule_node, graph, &computed_types, resolved_types, cache);
            if computed_types.get(rule_path) != Some(&rule_type) {
                writes += 1;
                written_at.insert(rule_path, writes);
                computed_types.insert(rule_path.clone(), rule_type);
            }
        }
        cache.clear();
    }

    (computed_types, errors)
}

fn infer_rule_body_type(
    rule_node: &RuleNode<'_>,
    graph: &Graph,
    computed_types: &HashMap<RulePath, Arc<LemmaType>>,
    resolved_types: &ResolvedTypesMap,
    cache: &mut InferExpressionTypeCache,
) -> Arc<LemmaType> {
    let body_types = rule_node.branches.iter().map(|(_, result)| {
        infer_expression_type(
            rule_node.expression.at(*result),
            graph,
            computed_types,
            resolved_types,
            cache,
        )
    });
    piecewise_type(body_types, &arithmetic_scope(resolved_types, graph))
}

/// Seed of `rule` from its non-recursive parts. Inserts a placeholder while visiting so
/// mutual row recursion does not loop; replaces it when a concrete seed is found.
fn ensure_seed(
    rule: &RulePath,
    graph: &Graph,
    computed_types: &mut HashMap<RulePath, Arc<LemmaType>>,
    resolved_types: &ResolvedTypesMap,
    cache: &mut InferExpressionTypeCache,
) {
    if computed_types.contains_key(rule) {
        return;
    }
    if !graph.rules.contains_key(rule) {
        return;
    }
    let mut reachable = Vec::new();
    let mut queued = HashSet::new();
    let mut pending = VecDeque::from([rule.clone()]);
    queued.insert(rule.clone());
    while let Some(path) = pending.pop_front() {
        let node = graph
            .rules
            .get(&path)
            .expect("BUG: local rule missing from graph in ensure_seed");
        reachable.push(path);
        for (_, result) in &node.branches {
            for dependency in local_rule_names_in(node.expression.at(*result), graph) {
                if queued.insert(dependency.clone()) {
                    pending.push_back(dependency);
                }
            }
        }
    }
    for path in &reachable {
        computed_types
            .entry(path.clone())
            .or_insert_with(|| Arc::new(LemmaType::undetermined_type()));
    }
    loop {
        let mut changed = false;
        for path in &reachable {
            let current = computed_types
                .get(path)
                .expect("BUG: reachable seed rule missing placeholder");
            if !current.vetoed() && !current.is_undetermined() {
                continue;
            }
            let node = graph
                .rules
                .get(path)
                .expect("BUG: reachable seed rule missing from graph");
            cache.clear();
            let seed = node.branches.iter().find_map(|(_, result)| {
                seed_type(
                    node.expression.at(*result),
                    graph,
                    computed_types,
                    resolved_types,
                    cache,
                )
            });
            if let Some(seed) = seed {
                computed_types.insert(path.clone(), seed);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
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
        let (fixed, children) = seed_candidates(node);
        if let Some(fixed) = fixed {
            return Some(fixed);
        }
        if !children.is_empty() {
            // Expand before full infer: helpers in `own + children_cost` may be untyped.
            pending.extend(children.into_iter().rev().map(|id| node.child(id)));
            continue;
        }
        match node.kind() {
            ExpressionKind::RulePath(rule) => {
                if let Some(lemma_type) = computed_types.get(rule) {
                    if !lemma_type.vetoed() && !lemma_type.is_undetermined() {
                        return Some(Arc::clone(lemma_type));
                    }
                }
            }
            _ => {
                let inferred =
                    infer_expression_type(node, graph, computed_types, resolved_types, cache);
                if !inferred.vetoed() && !inferred.is_undetermined() {
                    return Some(inferred);
                }
            }
        }
    }
    None
}

/// Local rules that `node` and its table prefix name (operands precede parents).
fn local_rule_names_in(node: ExprRef<'_, ExpressionKind>, graph: &Graph<'_>) -> Vec<RulePath> {
    let mut out = Vec::new();
    for index in 0..=node.id.index() {
        let child = node.child(ExprId::from_index(index));
        match child.kind() {
            ExpressionKind::RulePath(rule) => {
                if graph.rules.contains_key(rule) {
                    out.push(rule.clone());
                }
            }
            ExpressionKind::RowField { end, .. } => {
                push_local_rule_from_row_end(graph, end, &mut out);
            }
            ExpressionKind::SpecMember { path, name, .. } => {
                let identity = &graph.instance_lists.self_identity;
                if let Some(fields) = row_fields(graph, identity) {
                    let mut parts: Vec<&str> = path.iter().map(String::as_str).collect();
                    parts.push(name);
                    if let Some([hop]) = fields.chain(graph, identity, &parts).as_deref() {
                        push_local_rule_from_row_end(graph, &hop.end, &mut out);
                    }
                }
            }
            ExpressionKind::DataPath(path) => {
                if let Some(ReferenceEnd::Rule(rule)) = graph.reference_ends.get(path) {
                    if graph.rules.contains_key(rule) {
                        out.push(rule.clone());
                    }
                } else if let Some(DataDefinition::Reference {
                    target: ReferenceTarget::Rule(rule),
                    ..
                }) = graph.data.get(path)
                {
                    if graph.rules.contains_key(rule) {
                        out.push(rule.clone());
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn push_local_rule_from_row_end(graph: &Graph<'_>, end: &RowEnd, out: &mut Vec<RulePath>) {
    match end {
        RowEnd::Rule(rule) => {
            if graph.rules.contains_key(rule) {
                out.push(rule.clone());
            }
        }
        RowEnd::Data(path) => {
            if let Some(DataDefinition::Reference {
                target: ReferenceTarget::Rule(rule),
                ..
            }) = graph.data.get(path)
            {
                if graph.rules.contains_key(rule) {
                    out.push(rule.clone());
                }
            }
        }
    }
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
        | ExpressionKind::Logical { .. }
        | ExpressionKind::LogicalNegation(..) => {
            (Some(primitive_boolean_arc().clone()), Vec::new())
        }
        ExpressionKind::ListCommand { op, .. } => match op {
            ListOp::Count => (Some(primitive_number_arc().clone()), Vec::new()),
            ListOp::Predicate { kind, .. } => match kind {
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
#[cfg(test)]
mod tests {
    use super::*;

    use crate::parsing::ast::{BooleanValue, Reference, Span, Value};

    #[test]
    fn nodes_on_cycles_skips_nodes_that_only_depend_on_a_cycle() {
        // a <-> b is a cycle, c depends on it, d sits between it and the cycle e <-> f.
        let edges: BTreeMap<&str, Vec<&str>> = BTreeMap::from([
            ("a", vec!["b"]),
            ("b", vec!["a"]),
            ("c", vec!["a"]),
            ("d", vec!["b"]),
            ("e", vec!["d", "f"]),
            ("f", vec!["e"]),
            ("g", vec!["g"]),
        ]);
        let unsorted: Vec<&&str> = edges.keys().collect();
        let on_cycles = nodes_on_cycles(&unsorted, |node| edges[node].iter().collect());
        assert_eq!(on_cycles, vec![&"a", &"b", &"e", &"f", &"g"]);
    }

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
        let ordered_dependencies = discovery::discover_dependency_order(
            ctx,
            &repository,
            main_spec_ref,
            &effective,
            limits,
        )
        .map_err(|e| match e {
            discovery::DependencyDiscoveryError::Cycle(es)
            | discovery::DependencyDiscoveryError::Other(es) => es,
        })?;
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

                list: None,
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

                list: None,
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
            .rules
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
        let rule_node = graph.rules.values().next().unwrap();

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
            .rules
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

                    list: None,
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

                    list: None,
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

                    list: None,
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

                    list: None,
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
        let Some(DataDefinition::Import { row, .. }) = graph.data.get(&DataPath::local("m".into()))
        else {
            panic!("`m` is an import slot of the consumer graph");
        };
        assert_eq!(row.identity.spec, "myspec");
        assert!(
            matches!(
                graph
                    .template_of_row(row)
                    .data
                    .get(&DataPath::local("x".into())),
                Some(DataDefinition::Value { .. })
            ),
            "`m` reads `x` of the one `myspec` template"
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
        let Some(DataDefinition::Import { row, .. }) = graph.data.get(&alias_path) else {
            panic!(
                "alias path 'i' must be DataDefinition::Import, got {:?}",
                graph.data.get(&alias_path)
            );
        };
        assert_eq!(row.identity.spec, "inner");
        assert!(matches!(row.origin, RowOrigin::Import { ref alias } if alias.as_ref() == "i"));
        assert!(
            row.bindings.iter().all(Option::is_none),
            "`uses i: inner` without `-> with` binds no slot of the import row"
        );
        assert!(
            graph.data.keys().all(|path| path.segments.is_empty()),
            "the consumer graph holds its own data only, got {:?}",
            graph.data.keys().collect::<Vec<_>>()
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

    mod validation {
        use super::super::*;
        use crate::literals::UnitScale;
        use crate::parsing::ast::{CommandArg, TypeConstraintCommand};
        use crate::planning::semantics::{ConstraintNames, TypeSpecification};
        use crate::planning::types::validate_type_options;
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

        fn number_arg(n: i64) -> CommandArg {
            CommandArg::Literal(crate::literals::Value::Number(Decimal::from(n)))
        }

        #[test]
        fn text_minimum_command_is_rejected() {
            let mut specs = TypeSpecification::text();
            let res = specs.apply_constraint(
                ConstraintNames {
                    type_name: "test",
                    data_name: "test",
                },
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
                ConstraintNames {
                    type_name: "test",
                    data_name: "test",
                },
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
            let mut lemma_type = LemmaType::primitive(TypeSpecification::Text {
                length: None,
                help: String::new(),
            });
            lemma_type.options.values = vec![
                crate::planning::semantics::OptionValue {
                    value: crate::planning::semantics::BoundValueKind::unbound(ValueKind::Text(
                        Arc::from("red"),
                    )),
                },
                crate::planning::semantics::OptionValue {
                    value: crate::planning::semantics::BoundValueKind::unbound(ValueKind::Text(
                        Arc::from("blue"),
                    )),
                },
            ];
            let default = BoundValueKind::unbound(ValueKind::Text(Arc::from("green")));

            let src = test_source();
            let errors = validate_type_options(
                &lemma_type,
                Some(&default),
                "suggestion",
                "test",
                &src,
                None,
                &UnitIndex::new(),
            );
            assert_eq!(errors.len(), 1);
            assert!(
                errors[0].to_string().contains("green")
                    && errors[0].to_string().contains("allowed options")
            );
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
                    units: MeasureUnits(vec![crate::literals::MeasureUnit {
                        name: "eur".to_string(),
                        scale: UnitScale::Literal(rational_new(1, 1)),
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
                options: TypeOptions::default(),
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

        fn apply_n_text_options(option_count: usize) -> LemmaType {
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
            let mut lemma_type = LemmaType::primitive(TypeSpecification::text());
            apply_constraints_to_spec(
                &spec,
                "code",
                "code",
                &mut lemma_type,
                &constraints,
                &source,
                ConstraintSuggestionSink {
                    suggestion: &mut suggestion,
                    value: &mut value,
                    resolve_unit: None,
                },
            )
            .expect("options must apply");
            lemma_type
        }

        #[test]
        fn apply_constraints_many_text_options_preserve_count() {
            for count in [16_usize, 64] {
                assert_eq!(apply_n_text_options(count).options.values.len(), count);
            }
        }
    }

    mod decomposition_promotion {
        use super::super::*;
        use crate::engine::Context;
        use crate::literals::{MeasureUnit, MeasureUnits, UnitScale};
        use crate::parsing::parse;
        use crate::planning::semantics::{calendar_decomposition, duration_decomposition};
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

            let mut resolver = TypeResolver::new(ctx, HashMap::new());
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
                    units: MeasureUnits(vec![
                        MeasureUnit {
                            name: "gram".to_string(),
                            scale: UnitScale::Literal(rational_one()),
                            derived_measure_factors: Vec::new(),
                            decomposition: Default::default(),
                            minimum: None,
                            maximum: None,
                            suggestion_magnitude: None,
                        },
                        MeasureUnit {
                            name: "kilogram".to_string(),
                            scale: UnitScale::Literal(
                                decimal_to_rational(Decimal::from(1000))
                                    .expect("BUG: kilogram factor must be rational"),
                            ),
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
                options: TypeOptions::default(),
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
                    units: MeasureUnits(vec![MeasureUnit {
                        name: "month".to_string(),
                        scale: UnitScale::Literal(rational_one()),
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
                    scale: UnitScale::Literal(rational_one()),
                    derived_measure_factors: Vec::new(),
                    decomposition: Default::default(),
                    minimum: None,
                    maximum: None,
                    suggestion_magnitude: None,
                },
                MeasureUnit {
                    name: "kilogram".to_string(),
                    scale: UnitScale::Literal(
                        decimal_to_rational(Decimal::from(1000))
                            .expect("BUG: kilogram factor must be rational"),
                    ),
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
            let named = IndexMap::new();
            let scope = MeasureScope {
                unit_index: &unit_index,
                signature_index: &signature_index,
                named_types: &named,
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
            let named = IndexMap::new();
            let scope = MeasureScope {
                unit_index: &unit_index,
                signature_index: &signature_index,
                named_types: &named,
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
            let named = IndexMap::new();
            let scope = MeasureScope {
                unit_index: &unit_index,
                signature_index: &signature_index,
                named_types: &named,
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
            let named = IndexMap::new();
            let scope = MeasureScope {
                unit_index: &unit_index,
                signature_index: &signature_index,
                named_types: &named,
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

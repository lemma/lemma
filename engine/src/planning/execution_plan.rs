//! Execution plan for evaluated specs
//!
//! Provides a complete self-contained execution plan ready for the evaluator.
//! The plan holds an expanded shared normal-form graph in a dense table
//! ([`ExecutionPlan::normal_forms`]) addressed by [`NormalFormId`], plus all data —
//! no spec structure needed during evaluation.
//!
//! Imports are flattened at planning: every data and rule of an imported spec is an
//! entry of the consumer plan under its `uses` hop, and a row written in a data list is
//! an instance under the list path (`staff.1`). Evaluation has one context.
//!
//! Reliability model:
//! - [`Show`] is the IO contract surface for consumers (data and rule outputs).
//!   IO compatibility is the consumer-facing guarantee.

use crate::computation::arithmetic::SignatureIndex;
use crate::computation::rational::RationalInteger;
use crate::computation::units::{conversion_target_declares_unit, owning_type_declares_unit_name};
use crate::computation::{OperationResult, VetoType};
use crate::engine::write_effective_window;
use crate::limits::ResourceLimits;
use crate::literals::Value;
use crate::parsing::ast::{canonical_logical_name, DateTimeValue, EffectiveDate, LemmaSpec};
use crate::parsing::source::{Source, SourceType};
use crate::planning::graph::Graph;
use crate::planning::graph::ResolvedSpecTypes;
use crate::planning::instance_list::InstanceListSource;
use crate::planning::normalize::{
    build_normalized_rule, data_path_result_type, DispatchRegion, NormalForm, NormalFormId,
    NormalFormInterner, NormalizeContext, NormalizedRule,
};
use crate::planning::semantics::{
    lemma_type_with_display_binding, option_detail_line, primitive_number_arc, primitive_ratio_arc,
    value_kind_matches_spec, BoundValueKind, ComparisonComputation, DataDefinition, DataPath,
    LemmaType, LiteralValue, PathSegment, ReferenceEnd, ReferenceTarget, RowBinding, RowEnd,
    RowInstance, RulePath, SpecIdentity, TypeSpecification, TypedLiteral, ValueKind,
};
use crate::planning::show_expression::{project_depends_on_rules, show_branches_from, ShowScope};
use crate::planning::spec_set::LemmaSpecSet;
use crate::planning::spec_value::{RowFieldRead, SpecCandidates};
use crate::planning::typing::MeasureScope;
use crate::planning::unit_family::FamilyUnitCatalog;
use crate::planning::unit_index::UnitIndex;
use crate::result_value::{
    rule_result_value_failure_message, type_scoped_result_value_from_literal, RuleResultValue,
};
use crate::Error;
use indexmap::IndexMap;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

pub use crate::planning::show_expression::{
    ShowBranch, ShowConversionTarget, ShowExpression, ShowListCommand, ShowRule, SortDirection,
};

/// Rule positions that need one [`ExecutionPlan::data`] slot.
///
/// For an instance list, [`Self::fields`] is parallel to the row template's row fields
/// ([`ExecutionPlan::row_field_count`]): which rules need each row field of
/// evaluation-data rows through this list.
/// Otherwise `fields` is empty.
#[derive(Debug, Clone, Default)]
pub(crate) struct NeededByRules {
    pub rules: Vec<u32>,
    pub fields: Vec<Vec<u32>>,
}

/// A complete execution plan ready for the evaluator
///
/// Contains drift-free normal-form equations in a table, named rule roots, and all data.
/// Self-contained structure - no spec lookups required during evaluation.
///
/// A plan is immutable once stored. A committed plan is reused only when its own spec's
/// bytes are still the spec set's `Arc` (kept while the version's bytes are unchanged),
/// its version's boundaries (show-cache fields such as `effective_to`) and the
/// slices its edges resolve to are unchanged, and every one of those slices is reused.
#[derive(Debug, Clone)]
pub struct ExecutionPlan {
    /// Main spec name
    pub spec_name: String,

    /// Optional commentary from the `"""..."""` block in the spec source.
    pub commentary: Option<String>,

    /// This spec's own data slots in definition order, every path local. A `uses`
    /// import is one [`DataDefinition::Import`] slot holding its row over the
    /// template plan; spec-written list rows live in their [`RowStore`].
    pub data: IndexMap<DataPath, DataDefinition>,

    /// Dense table of interned NormalForm cells — the expanded shared graph.
    /// Index = [`NormalFormId`].
    pub normal_forms: Vec<NormalForm>,

    /// This spec's own rules → root of that rule in [`Self::normal_forms`].
    /// Insertion order is planning topo order (deps before consumers).
    pub rules: IndexMap<RulePath, ExecutableRule>,

    /// Data→data [`DataDefinition::Reference`] paths in dependency order for
    /// evaluation prepop (chained reference → reference → data). Empty when the
    /// plan has none. Rule-target references are not included.
    pub data_reference_order: Vec<DataPath>,

    /// Spec metadata, in declaration order.
    pub meta: IndexMap<String, Value>,

    /// Main-spec types from planning. [`ResolvedSpecTypes::unit_index`] is expression-scope
    /// units (local types plus direct `uses` imports). Rule-result unit maps use
    /// [`Self::family_units`]; Show data inputs stay declared-only.
    pub resolved_types: ResolvedSpecTypes,

    /// Precomputed measure/ratio family unit expansion for rule results and show rule schemas.
    pub(crate) family_units: Arc<FamilyUnitCatalog>,

    /// Reverse index: canonical-form unit signature `Vec<(unit_name, exponent)>` →
    /// (unit_name, owning type). Built from expression-scope units during planning so
    /// cross-type Multiply/Divide arithmetic can deterministically resolve a combined
    /// signature back to a single named unit. Ambiguous signatures (the same key matched
    /// by units in two distinct types) are rejected at planning time.
    pub signature_index: SignatureIndex,

    pub effective: EffectiveDate,

    /// Declared temporal window `[effective_from, effective_to)` of the LemmaSpec
    /// version this plan was built from. Filled by [`attach_show_cache`].
    pub effective_from: Option<DateTimeValue>,
    pub effective_to: Option<DateTimeValue>,

    /// Source start line of the LemmaSpec version this plan was built from.
    pub start_line: usize,

    /// Source type of the LemmaSpec version this plan was built from.
    pub source_type: Option<SourceType>,

    /// Per [`Self::data`] position: [`Self::rules`] positions that need that slot
    /// (through rule references). Non-promptable slots have empty `rules`.
    /// Instance-list slots also carry `fields` parallel to the row template's `data`.
    /// `len() == data.len()`. Built in [`build_execution_plan`].
    pub(crate) needed_by_rules: Vec<NeededByRules>,

    /// Prefill/suggestion [`BoundValueKind`]s for show, keyed by data path.
    /// Built once at plan time; [`RuleResultValue`] is expanded at show.
    pub(crate) data_display: IndexMap<DataPath, ShowDataCache>,

    /// Public Show rule catalog: this spec's own rules only (empty path). Built in
    /// [`build_execution_plan`]. Reachable import rules stay on Graph `show_edges` /
    /// `show_imports` for planning and nested-data attribution.
    pub(crate) show_rules: IndexMap<RulePath, ShowRule>,

    /// Positions in [`Self::show_rules`] of the Show rules that need each data path, in
    /// input-key order: every reader of data of `uses` imports by composed path (`i.x`),
    /// and the readers of data of this plan that [`Self::needed_by_rules`] does not list
    /// (local rules that need it only through an import).
    pub(crate) show_needed_by: IndexMap<DataPath, Vec<u32>>,

    /// Template identity of every import data path in [`Self::show_needed_by`].
    pub(crate) show_import_data: IndexMap<DataPath, SpecIdentity>,

    /// Every [`DataDefinition::Reference`] path → where its chain ends.
    /// Copied from [`Graph::reference_ends`] in [`build_execution_plan`].
    /// Missing key after planning is a bug.
    pub(crate) reference_ends: IndexMap<DataPath, ReferenceEnd>,

    /// Precomputed `input_key` → `DataPath` index for [`EvaluateData::resolve`].
    /// One entry per [`Self::data`] slot. Keyed lookups index this map; they do not
    /// walk the data tree.
    pub input_key_index: IndexMap<String, DataPath>,

    /// Precomputed `input_key` → `RulePath` for [`Self::show_rules`].
    /// Built once so [`Self::get_rule`] is O(1) without allocating per candidate.
    pub(crate) show_rule_index: IndexMap<String, RulePath>,

    /// Every [`Self::data`] path → its [`LeafKind::DataPath`] cell in
    /// [`Self::normal_forms`]. Built once so evaluation writes bindings into
    /// the value table by [`NormalFormId`] instead of a parallel path map.
    pub(crate) data_leaf: IndexMap<DataPath, NormalFormId>,

    /// Every reference target through `uses` hops (`i.x`, `i.j.r`) that a binding
    /// resolving in this plan's context names → its member cell: the read of the
    /// target on the import row this plan holds.
    pub(crate) member_targets: IndexMap<ReferenceTarget, NormalFormId>,

    /// Template plans of instance-list row specs, by row spec. Rows supplied by
    /// evaluation data (and option rows) are checked and evaluated against them.
    pub(crate) row_templates: BTreeMap<SpecIdentity, Arc<ExecutionPlan>>,

    /// This plan's own spec. Rows of its self-lists read this plan as their template.
    pub(crate) identity: SpecIdentity,

    /// The `Arc` of this plan's own spec version, shared with its [`LemmaSpecSet`].
    pub(crate) own_spec: Arc<LemmaSpec>,

    /// Slices this plan's dependency edges resolved to, in edge order, self-edges left
    /// out.
    pub(crate) edges: Vec<SpecIdentity>,

    /// Instances each rule can yield (empty when not spec-valued). Copied from
    /// [`Graph::rule_candidates`].
    pub(crate) rule_candidates: BTreeMap<RulePath, SpecCandidates>,

    /// Rules read on evaluation-data rows of this plan's self-lists, in [`Self::rules`]
    /// order. A row tree fills them bottom-up, so a parent reads its children's values
    /// without evaluating down the tree.
    pub(crate) self_row_rules: Vec<RulePath>,

    /// Source rows of every [`DataDefinition::InstanceList`] path.
    pub(crate) instance_lists: IndexMap<DataPath, InstanceListSource>,

    /// Per [`Self::normal_forms`] cell: bit `L` is set when the cell reads the current
    /// list row at level `L` that encloses it (`RowField` or `RowValue`). Such cells
    /// differ per row and are never cached in the value table.
    pub(crate) row_mask: Vec<u32>,

    /// Precomputed initial value table (length = `normal_forms.len()`): plan defaults
    /// and pre-evaluated data-target reference chains in their data-leaf slots, `None` elsewhere.
    pub(crate) initial_values: Vec<Option<OperationResult>>,

    /// Data-leaf cell ids that must run a walk step before `ready` returns their
    /// stored value: an options check, or a stored `UnscaledMeasure` to settle.
    pub(crate) walk_settled_leaves: Vec<NormalFormId>,

    /// Per [`Self::normal_forms`] cell: the [`RuleIndex`] of the rule a `rule_ref`
    /// cell reads, `None` for every other cell.
    pub(crate) rule_ref_indexes: Vec<Option<RuleIndex>>,

    /// Per [`Self::data`] slot: its [`Self::data_leaf`] cell. Row bindings travel by
    /// slot and land in the value table through this.
    pub(crate) data_slot_leaves: Vec<NormalFormId>,

    /// Scale targets of dynamic units on this plan's data-slot types (this plan's
    /// coordinates). Row scopes force these before a leaf body.
    pub(crate) data_slot_scale_targets: Vec<ReferenceTarget>,
}

/// Plan-time prefill/suggestion for one data path (show cache).
///
/// Stores domain [`BoundValueKind`]. [`RuleResultValue`] is built at
/// show time (JSON / API boundary).
#[derive(Debug, Clone)]
pub(crate) struct ShowDataCache {
    pub value: Option<BoundValueKind>,
    pub suggestion: Option<BoundValueKind>,
}

/// A named rule's root in the normal-form table, plus declaration metadata.
#[derive(Debug, Clone)]
pub struct ExecutableRule {
    /// Unique identifier for this rule
    pub path: RulePath,

    /// Root of this rule in the shared [`ExecutionPlan::normal_forms`] graph.
    pub normal_form: NormalFormId,

    /// Source location for error messages (always present for rules from parsed specs)
    pub source: Source,

    /// Type of the rule as written: the lowered source expression typed at
    /// planning, equal to validation's inferred type. The root cell at
    /// `normal_form` has the same value type but may carry a different name
    /// after rewrites (`x + 0` → `x`), so the API type reads this field.
    pub rule_type: Arc<LemmaType>,

    /// Scale targets of every dynamic unit this rule's nodes hold or write, in this
    /// plan's coordinates. Evaluated before the rule body; topological order puts
    /// local scale rules first.
    pub scale_dependencies: Vec<ReferenceTarget>,
}

impl ExecutableRule {
    pub fn name(&self) -> &str {
        &self.path.rule
    }
}

/// Position of a rule in [`ExecutionPlan::rules`]; indexes the evaluation
/// context's `rule_values` table. Plain `Copy` so the evaluation walk can
/// unwind to a rule without cloning its [`RulePath`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct RuleIndex(usize);

impl RuleIndex {
    pub(crate) fn index(self) -> usize {
        self.0
    }
}

/// Select the plan whose half-open `[effective, next.effective)` covers `instant`
/// (greatest key `<= instant` in the map).
pub(crate) fn plan_at<'a>(
    plans: &'a BTreeMap<EffectiveDate, std::sync::Arc<ExecutionPlan>>,
    instant: &EffectiveDate,
) -> Option<&'a ExecutionPlan> {
    plans
        .range(..=instant.clone())
        .next_back()
        .map(|(_, plan)| plan.as_ref())
}

fn assert_list_elements_determined(root: &LemmaType) {
    let mut stack = vec![root];
    while let Some(lemma_type) = stack.pop() {
        if let TypeSpecification::List { element, .. } = &lemma_type.specifications {
            assert!(
                !element.is_undetermined(),
                "BUG: validated List type '{}' has an Undetermined element type",
                lemma_type.name()
            );
            stack.push(element);
        }
    }
}

/// Builds an execution plan from a Graph for one temporal slice.
///
/// `interner` dedups cons cells while the plan is built. The plan's dense `normal_forms`
/// table is extracted from it with [`NormalFormInterner::extract_reachable`], so the
/// result does not depend on what else the interner holds.
pub(crate) fn build_execution_plan(
    graph: &Graph<'_>,
    resolved_types: ResolvedSpecTypes,
    effective: &EffectiveDate,
    limits: &ResourceLimits,
    interner: &mut NormalFormInterner,
    own_spec: Arc<LemmaSpec>,
    edges: Vec<SpecIdentity>,
) -> Result<ExecutionPlan, Vec<Error>> {
    let rule_order = &graph.rule_order;

    let main_spec = graph.main_spec;
    let data = graph.build_data(&resolved_types.resolved)?;

    // Planning gate: every data-target reference and plain data declaration
    // must carry a fully resolved type. Rule-target references are exempt:
    // they deliberately ship `Undetermined` so runtime veto propagation
    // surfaces the target rule's veto reason directly. A residual
    // `Undetermined` anywhere else would violate the invariant evaluation
    // and show consumers rely on — report it instead of shipping the plan.
    let undetermined_errors: Vec<Error> = data
        .iter()
        .filter_map(|(path, definition)| {
            let (resolved_type, source) = match definition {
                DataDefinition::TypeDeclaration {
                    resolved_type,
                    source,
                    ..
                } => (resolved_type, source),
                DataDefinition::Reference {
                    target: ReferenceTarget::Data(_),
                    resolved_type,
                    source,
                    ..
                } => (resolved_type, source),
                DataDefinition::Reference {
                    target: ReferenceTarget::Rule(_),
                    ..
                }
                | DataDefinition::Value { .. }
                | DataDefinition::InstanceList { .. }
                | DataDefinition::Import { .. } => return None,
            };
            if resolved_type.is_undetermined() {
                Some(Error::validation(
                    format!("could not determine the type of '{path}'"),
                    Some(source.clone()),
                    None::<String>,
                ))
            } else {
                None
            }
        })
        .collect();
    if !undetermined_errors.is_empty() {
        return Err(undetermined_errors);
    }

    for lemma_type in resolved_types.resolved.values() {
        assert_list_elements_determined(lemma_type);
    }

    let signature_index = resolved_types.signature_index.clone();

    let family_units = Arc::new(FamilyUnitCatalog::build(&resolved_types.unit_index));

    let rule_types: HashMap<RulePath, Arc<LemmaType>> = graph
        .rules
        .iter()
        .map(|(path, node)| (path.clone(), Arc::clone(&node.rule_type)))
        .collect();
    let mut rules: IndexMap<RulePath, ExecutableRule> = IndexMap::new();
    let mut completed_rules: HashMap<RulePath, NormalFormId> = HashMap::new();
    let mut local_show_rules: HashMap<RulePath, ShowRule> = HashMap::new();

    let show_imports = &graph.show_imports;
    let show_edges = &graph.show_edges;
    let show_order = &graph.show_order;
    let show_keys: HashSet<RulePath> = show_edges.keys().cloned().collect();
    let catalog_keys: HashSet<RulePath> = rule_order.iter().cloned().collect();

    for rule_path in rule_order {
        let rule_node = graph.rules.get(rule_path).expect(
            "bug: rule from topological sort not in graph - validation should have caught this",
        );

        let measure_scope = MeasureScope {
            unit_index: &resolved_types.unit_index,
            signature_index: &resolved_types.signature_index,
            named_types: &resolved_types.resolved,
        };
        let normalize_ctx = NormalizeContext {
            data: &data,
            row_templates: &graph.instance_lists.templates,
            self_identity: &graph.instance_lists.self_identity,
            rule_order,
            measure_scope,
            max_normalized_expression_nodes: limits.max_normalized_expression_nodes,
        };
        let normalized = build_normalized_rule(
            &normalize_ctx,
            &completed_rules,
            &rule_types,
            &rule_node.expression,
            Some(rule_node.source.clone()),
            interner,
        )
        .map_err(|error| vec![error])?;
        let NormalizedRule { body, source_type } = normalized;
        assert_eq!(
            &source_type, &rule_node.rule_type,
            "BUG: rule '{}' typed differently by planning and validation",
            rule_path.rule
        );
        assert!(
            interner
                .result_type(body)
                .same_value_type(&rule_node.rule_type),
            "BUG: rewrites changed the value type of rule '{}': {:?} from {:?}",
            rule_path.rule,
            interner.result_type(body),
            rule_node.rule_type
        );
        completed_rules.insert(rule_path.clone(), body);

        local_show_rules.insert(
            rule_path.clone(),
            ShowRule {
                lemma_type: family_units.rule_type_for_show(rule_node.rule_type.as_ref()),
                path: rule_path.segments.clone(),
                branches: show_branches_from(
                    &rule_node.expression,
                    &rule_node.branches,
                    &ShowScope {
                        show_keys: &show_keys,
                        data: &data,
                        templates: &graph.instance_lists.templates,
                    },
                ),
                depends_on_rules: project_depends_on_rules(&show_edges[rule_path], &catalog_keys),
            },
        );

        rules.insert(
            rule_path.clone(),
            ExecutableRule {
                path: rule_path.clone(),
                normal_form: body,
                source: rule_node.source.clone(),
                rule_type: source_type,
                scale_dependencies: Vec::new(),
            },
        );
    }

    for (rule_path, rule) in rules.iter_mut() {
        rule.scale_dependencies = graph
            .rules
            .get(rule_path)
            .expect("BUG: executable rule missing from its graph")
            .scale_targets
            .clone();
    }
    let mut member_target_paths: Vec<&ReferenceTarget> = Vec::new();
    crate::planning::graph::for_each_owned_reference(
        &data,
        &graph.instance_lists,
        |_, _, target| {
            if !reference_target_segments(target).is_empty()
                && !member_target_paths.contains(&target)
            {
                member_target_paths.push(target);
            }
        },
    );
    let mut owned_scale_member_targets: Vec<ReferenceTarget> = Vec::new();
    let mut push_uses_scale_member = |target: &ReferenceTarget| {
        if reference_target_segments(target).is_empty() {
            return;
        }
        if !first_hop_is_uses_import(&data, target) {
            return;
        }
        if member_target_paths.contains(&target) || owned_scale_member_targets.contains(target) {
            return;
        }
        owned_scale_member_targets.push(target.clone());
    };
    for rule in rules.values() {
        for target in &rule.scale_dependencies {
            push_uses_scale_member(target);
        }
    }
    for definition in data.values() {
        let Some(lemma_type) = definition.schema_type() else {
            continue;
        };
        for target in lemma_type.dynamic_scale_targets() {
            push_uses_scale_member(&target);
        }
    }
    for target in &owned_scale_member_targets {
        member_target_paths.push(target);
    }
    let mut member_target_roots: Vec<NormalFormId> = Vec::new();
    for target in &member_target_paths {
        let normalize_ctx = NormalizeContext {
            data: &data,
            row_templates: &graph.instance_lists.templates,
            self_identity: &graph.instance_lists.self_identity,
            rule_order,
            measure_scope: MeasureScope {
                unit_index: &resolved_types.unit_index,
                signature_index: &resolved_types.signature_index,
                named_types: &resolved_types.resolved,
            },
            max_normalized_expression_nodes: limits.max_normalized_expression_nodes,
        };
        let normalized = build_normalized_rule(
            &normalize_ctx,
            &completed_rules,
            &rule_types,
            &member_target_expression(target),
            None,
            interner,
        )
        .map_err(|error| vec![error])?;
        member_target_roots.push(normalized.body);
    }

    let root_ids: Vec<NormalFormId> = rules
        .values()
        .map(|rule| rule.normal_form)
        .chain(member_target_roots)
        .collect();
    let (normal_forms, mut remapped_roots) = interner.extract_reachable(&root_ids);
    let remapped_members = remapped_roots.split_off(rules.len());
    for (rule, remapped) in rules.values_mut().zip(remapped_roots) {
        rule.normal_form = remapped;
    }
    let member_targets: IndexMap<ReferenceTarget, NormalFormId> = member_target_paths
        .into_iter()
        .cloned()
        .zip(remapped_members)
        .collect();

    let mut show_rules: IndexMap<RulePath, ShowRule> =
        IndexMap::with_capacity(local_show_rules.len());
    for path in show_order {
        if let Some(local) = local_show_rules.remove(path) {
            show_rules.insert(path.clone(), local);
        }
    }
    assert!(
        local_show_rules.is_empty(),
        "BUG: local rules missing from the Show order"
    );

    let mut plan = ExecutionPlan {
        spec_name: main_spec.name.clone(),
        commentary: main_spec.commentary.clone(),
        data,
        normal_forms,
        rules,
        data_reference_order: graph.data_reference_order.to_vec(),
        meta: main_spec
            .meta_fields
            .iter()
            .map(|f| (f.key.clone(), f.value.clone()))
            .collect(),
        resolved_types,
        family_units,
        signature_index,
        effective: effective.clone(),
        // Filled by attach_show_cache after this returns.
        effective_from: None,
        effective_to: None,
        start_line: 1,
        source_type: None,
        needed_by_rules: Vec::new(),
        data_display: IndexMap::new(),
        show_rules,
        // Filled below after validation succeeds.
        show_needed_by: IndexMap::new(),
        show_import_data: show_imports
            .data
            .iter()
            .map(|(path, identity)| (path.clone(), identity.clone()))
            .collect(),
        reference_ends: IndexMap::new(),
        input_key_index: IndexMap::new(),
        show_rule_index: IndexMap::new(),
        data_leaf: IndexMap::new(),
        member_targets,
        row_templates: graph.instance_lists.templates.clone(),
        identity: graph.instance_lists.self_identity.clone(),
        own_spec,
        edges,
        rule_candidates: graph
            .rule_candidates
            .iter()
            .map(|(rule, candidates)| (rule.clone(), candidates.clone()))
            .collect(),
        self_row_rules: Vec::new(),
        instance_lists: graph.instance_lists.sources.clone(),
        row_mask: Vec::new(),
        initial_values: Vec::new(),
        walk_settled_leaves: Vec::new(),
        rule_ref_indexes: Vec::new(),
        data_slot_leaves: Vec::new(),
        data_slot_scale_targets: Vec::new(),
    };
    plan.data_slot_scale_targets = data_slot_scale_targets(&plan.data);

    let mut plan_errors = validate_literal_data_against_types(&plan);
    if let Err(error) = validate_unit_conversion_targets(&plan) {
        plan_errors.push(error);
    }
    if !plan_errors.is_empty() {
        return Err(plan_errors);
    }

    plan.reference_ends = graph.reference_ends.clone();
    let input_keys: Vec<DataPath> = plan.data.keys().cloned().collect();
    plan.data_leaf = ensure_data_leaves(&mut plan.normal_forms, &input_keys, &plan.data);
    plan.input_key_index = input_keys
        .iter()
        .map(|path| (path.input_key(), path.clone()))
        .collect();
    plan.show_rule_index = plan
        .show_rules
        .keys()
        .map(|path| (path.input_key(), path.clone()))
        .collect();
    for (path, &id) in &plan.data_leaf {
        debug_assert_eq!(
            plan.result_type(id).as_ref(),
            data_path_result_type(&plan.data, path).as_ref(),
            "BUG: DataPath leaf result_type drift for {path}"
        );
    }
    let bits_by_rule = plan.build_needed_by_rules();
    plan.show_needed_by =
        plan.build_show_needed_by(show_order, show_edges, show_imports, &bits_by_rule);
    plan.fill_row_field_needs(&bits_by_rule);
    plan.row_mask = build_row_mask(&plan.normal_forms).map_err(|error| vec![error])?;
    plan.self_row_rules = self_row_rules(&plan);
    plan.populate_evaluation_tables();
    validate_option_rules(&plan)?;
    Ok(plan)
}

/// The `uses` hops a reference target is read through; empty for a target of the plan itself.
pub(crate) fn reference_target_segments(target: &ReferenceTarget) -> &[PathSegment] {
    match target {
        ReferenceTarget::Data(path) => &path.segments,
        ReferenceTarget::Rule(rule) => &rule.segments,
    }
}

/// Whether `target`'s first hop is a `uses` Import of this plan (not a list name).
fn first_hop_is_uses_import(
    data: &IndexMap<DataPath, DataDefinition>,
    target: &ReferenceTarget,
) -> bool {
    let Some(first) = reference_target_segments(target).first() else {
        return false;
    };
    matches!(
        data.get(&DataPath::local(first.uses.to_string())),
        Some(DataDefinition::Import { .. })
    )
}

/// Dynamic scale targets declared on this plan's data-slot types, in slot order.
fn data_slot_scale_targets(data: &IndexMap<DataPath, DataDefinition>) -> Vec<ReferenceTarget> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for definition in data.values() {
        let Some(lemma_type) = definition.schema_type() else {
            continue;
        };
        for target in lemma_type.dynamic_scale_targets() {
            if seen.insert(target.clone()) {
                out.push(target);
            }
        }
    }
    out
}

/// `i.j.x` as a member read on the plan's import slot `i`: the expression a member
/// target cell lowers.
fn member_target_expression(target: &ReferenceTarget) -> crate::planning::semantics::Expression {
    let (first, rest) = reference_target_segments(target)
        .split_first()
        .expect("BUG: a member target has a `uses` hop");
    let name = match target {
        ReferenceTarget::Data(path) => path.data.clone(),
        ReferenceTarget::Rule(rule) => rule.rule.clone(),
    };
    let mut table = crate::parsing::arena::ExprTable::default();
    let base = table.push(
        crate::planning::semantics::ExpressionKind::DataPath(DataPath::local(
            first.uses.to_string(),
        )),
        None,
    );
    let root = table.push(
        crate::planning::semantics::ExpressionKind::SpecMember {
            base,
            path: rest
                .iter()
                .map(|segment| segment.uses.to_string())
                .collect(),
            name,
        },
        None,
    );
    crate::planning::semantics::Expression::new(table, root)
}

/// Rule the options of data `path` come from: the rule named by the slot's type, in
/// the spec that declared the slot (the hops of `path`). `None` when the type has no
/// options rule.
pub(crate) fn options_rule_path(data_path: &DataPath, lemma_type: &LemmaType) -> Option<RulePath> {
    let name = lemma_type.options_rule()?;
    Some(RulePath {
        segments: data_path.segments.clone(),
        rule: name.to_string(),
    })
}

fn validate_option_rules(plan: &ExecutionPlan) -> Result<(), Vec<Error>> {
    let mut errors = Vec::new();
    for (path, definition) in &plan.data {
        let Some(lemma_type) = definition.schema_type() else {
            continue;
        };
        let owners = option_owners(lemma_type);
        for owner in owners {
            // The owner's own rule: a list's element is checked as its own owner.
            let Some(name) = &owner.options.rule else {
                continue;
            };
            let rule_path = RulePath {
                segments: path.segments.clone(),
                rule: name.clone(),
            };
            let Some(rule) = plan.rules.get(&rule_path) else {
                errors.push(Error::validation(
                    format!("options rule '{name}' does not exist"),
                    None,
                    None::<String>,
                ));
                continue;
            };
            let TypeSpecification::List { element, .. } = &rule.rule_type.specifications else {
                errors.push(Error::validation(
                    format!("options rule '{name}' must return a list"),
                    None,
                    None::<String>,
                ));
                continue;
            };
            if !option_element_matches(owner, element) {
                errors.push(Error::validation(
                    format!(
                        "options rule '{name}' returns {}, not a list of {}",
                        rule.rule_type.name(),
                        owner.name()
                    ),
                    None,
                    None::<String>,
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

fn option_owners(lemma_type: &LemmaType) -> Vec<&LemmaType> {
    let mut owners = vec![lemma_type];
    if let TypeSpecification::List { element, .. } = &lemma_type.specifications {
        owners.push(element.as_ref());
    }
    owners
}

fn option_element_matches(owner: &LemmaType, element: &LemmaType) -> bool {
    match (&owner.specifications, &element.specifications) {
        (TypeSpecification::Spec { spec: left }, TypeSpecification::Spec { spec: right }) => {
            left == right
        }
        _ => {
            owner.has_same_base_type(element)
                && !matches!(owner.specifications, TypeSpecification::Spec { .. })
        }
    }
}

/// Bitset over the row fields ([`ExecutionPlan::row_field_position`]) of rows of `identity`.
fn row_bits_entry<'a>(
    plan: &ExecutionPlan,
    row_bits: &'a mut BTreeMap<SpecIdentity, Vec<u64>>,
    identity: &SpecIdentity,
) -> &'a mut Vec<u64> {
    let words = plan.row_template(identity).row_field_count().div_ceil(64);
    row_bits
        .entry(identity.clone())
        .or_insert_with(|| vec![0u64; words])
}

/// Set the bit of the slot an evaluation-data row binds when a rule reads `path` on it.
fn mark_template_data_path(template: &ExecutionPlan, path: &DataPath, bits: &mut [u64]) {
    if let Some(target) = template.bindable_row_field(path) {
        let data_pos = template
            .data
            .get_index_of(target)
            .unwrap_or_else(|| panic!("BUG: bindable row field '{target}' absent from plan.data"));
        bits[data_pos / 64] |= 1u64 << (data_pos % 64);
    }
}

/// Row fields a finished template's rule needs: its data (from `needed_by_rules`) and
/// the data of its `uses` imports (from `show_needed_by`).
fn mark_template_rule_data(template: &ExecutionPlan, rule: &RulePath, bits: &mut [u64]) {
    let rule_pos = u32::try_from(template.rules.get_index_of(rule).unwrap_or_else(|| {
        panic!(
            "BUG: row rule '{rule}' missing from template plan '{}'",
            template.spec_name
        )
    }))
    .expect("BUG: rule count exceeds u32");
    for (data_pos, consumers) in template.needed_by_rules.iter().enumerate() {
        if consumers.rules.contains(&rule_pos) {
            bits[data_pos / 64] |= 1u64 << (data_pos % 64);
        }
    }
    mark_template_import_data(template, rule, bits);
}

/// Row fields of `uses` imports of `template` that its Show rule `rule` (a path composed
/// from import hops) needs.
fn mark_template_import_data(template: &ExecutionPlan, rule: &RulePath, bits: &mut [u64]) {
    let Some(rule_pos) = template.show_rule_position(rule) else {
        return;
    };
    for (index, path) in template.show_import_data.keys().enumerate() {
        let needed = template
            .show_needed_by
            .get(path)
            .expect("BUG: Show import data has readers");
        if needed.contains(&rule_pos) {
            let field_pos = template.data.len() + index;
            bits[field_pos / 64] |= 1u64 << (field_pos % 64);
        }
    }
}

/// One `uses` level below a row spec in [`mark_row_end_needs`]: the instance the slot
/// holds there (a consumer-written override or the template's own) and its template.
#[derive(Clone)]
struct MemberLevel<'p> {
    segment: PathSegment,
    row: &'p RowInstance,
    template: &'p ExecutionPlan,
}

/// A read on rows of a row spec, at a chain of `uses` levels below it.
enum MemberNeed<'p> {
    Data(Vec<MemberLevel<'p>>, DataPath),
    Rule(Vec<MemberLevel<'p>>, RulePath),
}

/// `chain` extended by the `uses` slots `aliases`, starting at the template of the
/// chain's last level (or `holder` for an empty chain).
fn member_levels<'p, 'a>(
    plan: &'p ExecutionPlan,
    holder: &'p ExecutionPlan,
    mut chain: Vec<MemberLevel<'p>>,
    aliases: impl IntoIterator<Item = &'a str>,
) -> Vec<MemberLevel<'p>> {
    for alias in aliases {
        let template = chain.last().map_or(holder, |level| level.template);
        let slot = DataPath::local(alias.to_string());
        let Some(DataDefinition::Import { row, .. }) = template.data.get(&slot) else {
            panic!("BUG: member hop '{slot}' is not a `uses` slot");
        };
        let row: &RowInstance = match chain
            .last()
            .and_then(|level| level.row.get_binding(template, &slot))
        {
            Some(RowBinding::Instance(written)) => written,
            Some(other) => panic!("BUG: `uses` slot '{slot}' bound to {other:?}"),
            None => row,
        };
        chain.push(MemberLevel {
            segment: PathSegment::import(&slot.data, &row.identity),
            row,
            template: plan.row_template(&row.identity),
        });
    }
    chain
}

/// Record what reading the member chain `ends` on evaluation-data rows of `identity`
/// needs. Every end but the last reads a `uses` slot.
fn mark_row_end_needs(
    plan: &ExecutionPlan,
    identity: &SpecIdentity,
    ends: &[RowEnd],
    bits_by_rule: &[Vec<u64>],
    row_bits: &mut BTreeMap<SpecIdentity, Vec<u64>>,
    consumer_rule: &RulePath,
) {
    let bits = row_bits_entry(plan, row_bits, identity);
    let holder = plan.row_template(identity);
    let (end, hops) = ends.split_last().expect("BUG: a row read has a last end");
    let chain = member_levels(
        plan,
        holder,
        Vec::new(),
        hops.iter().map(|hop| match hop {
            RowEnd::Data(slot) => slot.data.as_str(),
            RowEnd::Rule(rule) => panic!("BUG: a row read hop is a `uses` slot, got rule '{rule}'"),
        }),
    );
    let mut pending = vec![match end {
        RowEnd::Data(path) => MemberNeed::Data(chain, path.clone()),
        RowEnd::Rule(rule) => MemberNeed::Rule(chain, rule.clone()),
    }];
    while let Some(need) = pending.pop() {
        match need {
            MemberNeed::Data(chain, path) if !path.segments.is_empty() => {
                let chain = member_levels(
                    plan,
                    holder,
                    chain,
                    path.segments.iter().map(|segment| &*segment.uses),
                );
                pending.push(MemberNeed::Data(chain, DataPath::local(path.data)));
            }
            MemberNeed::Rule(chain, rule) if !rule.segments.is_empty() => {
                let chain = member_levels(
                    plan,
                    holder,
                    chain,
                    rule.segments.iter().map(|segment| &*segment.uses),
                );
                pending.push(MemberNeed::Rule(
                    chain,
                    RulePath::new(Vec::new(), rule.rule),
                ));
            }
            MemberNeed::Data(chain, path) if chain.is_empty() => {
                mark_holder_data(plan, identity, &path, bits_by_rule, bits, consumer_rule);
            }
            MemberNeed::Rule(chain, rule) if chain.is_empty() => {
                mark_holder_rule(plan, identity, &rule, bits_by_rule, bits, consumer_rule);
            }
            MemberNeed::Data(mut chain, path) => {
                let level = chain.last().expect("BUG: non-empty member chain");
                match level.row.get_binding(level.template, &path) {
                    None => match level.template.data.get(&path) {
                        Some(DataDefinition::Reference { .. }) => {
                            match level.template.reference_ends.get(&path) {
                                Some(ReferenceEnd::Promptable(target)) => {
                                    pending.push(MemberNeed::Data(chain, target.clone()));
                                }
                                Some(ReferenceEnd::Rule(rule)) => {
                                    pending.push(MemberNeed::Rule(chain, rule.clone()));
                                }
                                None => panic!(
                                    "BUG: reference '{path}' missing from reference_ends after planning"
                                ),
                            }
                        }
                        Some(_) => {
                            let composed = DataPath {
                                segments: chain.iter().map(|level| level.segment.clone()).collect(),
                                data: path.data,
                            };
                            if let Some(index) = holder.show_import_data.get_index_of(&composed) {
                                let field_pos = holder.data.len() + index;
                                bits[field_pos / 64] |= 1u64 << (field_pos % 64);
                            }
                        }
                        None => panic!("BUG: member data '{path}' absent from its template"),
                    },
                    Some(RowBinding::Deferred { target, hops }) => {
                        let frame = chain
                            .len()
                            .checked_sub(1 + *hops as usize)
                            .expect("BUG: a `uses` binding hops above the row spec");
                        let target = target.clone();
                        chain.truncate(frame);
                        pending.push(match target {
                            ReferenceTarget::Data(path) => MemberNeed::Data(chain, path),
                            ReferenceTarget::Rule(rule) => MemberNeed::Rule(chain, rule),
                        });
                    }
                    Some(
                        RowBinding::Bound(_) | RowBinding::Rows { .. } | RowBinding::Instance(_),
                    ) => {}
                }
            }
            MemberNeed::Rule(chain, rule) => {
                let template = chain.last().expect("BUG: non-empty member chain").template;
                let rule_pos =
                    u32::try_from(template.rules.get_index_of(&rule).unwrap_or_else(|| {
                        panic!("BUG: member rule '{rule}' missing from its template")
                    }))
                    .expect("BUG: rule count exceeds u32");
                let show_pos = template
                    .show_rule_position(&rule)
                    .expect("BUG: a local rule is a Show rule of its plan");
                for ((path, _), needed) in template.data.iter().zip(&template.needed_by_rules) {
                    if needed.rules.contains(&rule_pos) {
                        pending.push(MemberNeed::Data(chain.clone(), path.clone()));
                    }
                }
                for (path, needed) in &template.show_needed_by {
                    if !path.segments.is_empty() && needed.contains(&show_pos) {
                        pending.push(MemberNeed::Data(chain.clone(), path.clone()));
                    }
                }
            }
        }
    }
}

/// What reading data `path` on rows of the row spec `identity` itself needs.
fn mark_holder_data(
    plan: &ExecutionPlan,
    identity: &SpecIdentity,
    path: &DataPath,
    bits_by_rule: &[Vec<u64>],
    bits: &mut [u64],
    consumer_rule: &RulePath,
) {
    let template = plan.row_template(identity);
    match template.reference_ends.get(path) {
        Some(ReferenceEnd::Rule(rule)) => {
            mark_holder_rule(plan, identity, rule, bits_by_rule, bits, consumer_rule);
        }
        Some(ReferenceEnd::Promptable(_)) | None => mark_template_data_path(template, path, bits),
    }
}

/// What reading rule `rule` on rows of the row spec `identity` itself needs.
fn mark_holder_rule(
    plan: &ExecutionPlan,
    identity: &SpecIdentity,
    rule: &RulePath,
    bits_by_rule: &[Vec<u64>],
    bits: &mut [u64],
    consumer_rule: &RulePath,
) {
    let template = plan.row_template(identity);
    if *identity == plan.identity {
        let rule_pos = plan.rules.get_index_of(rule).unwrap_or_else(|| {
            panic!("BUG: row rule '{rule}' missing from plan.rules (rule '{consumer_rule}')")
        });
        for (word, dep) in bits.iter_mut().zip(bits_by_rule[rule_pos].iter()) {
            *word |= *dep;
        }
        mark_template_import_data(template, rule, bits);
    } else {
        mark_template_rule_data(template, rule, bits);
    }
}

fn mark_list_key_fields(
    plan: &ExecutionPlan,
    list: NormalFormId,
    row_bits: &mut BTreeMap<SpecIdentity, Vec<u64>>,
    consumer_rule: &RulePath,
) {
    use crate::planning::semantics::ListKey;

    let list_type = plan.result_type(list);
    let TypeSpecification::List { element, key, .. } = &list_type.specifications else {
        panic!(
            "BUG: Find/WithRow list cell is not a list type (rule '{consumer_rule}'): {list_type:?}"
        );
    };
    let TypeSpecification::Spec { spec: identity } = &element.specifications else {
        return;
    };
    let ListKey::Fields(fields) = key else {
        return;
    };
    let template = plan.row_template(identity);
    let bits = row_bits_entry(plan, row_bits, identity);
    for key_field in fields {
        mark_template_data_path(template, &key_field.path, bits);
    }
}

/// [`ExecutionPlan::self_row_rules`]: every rule a row field or row member read of a
/// self-list names, directly or through a data reference to a rule.
fn self_row_rules(plan: &ExecutionPlan) -> Vec<RulePath> {
    use crate::planning::normalize::NormalFormKind;
    use crate::planning::semantics::RowEnd;

    let mut rules_read: HashSet<&RulePath> = HashSet::new();
    for (index, cell) in plan.normal_forms.iter().enumerate() {
        let end = match &cell.kind {
            NormalFormKind::RowField { identity, end, .. } if *identity == plan.identity => end,
            NormalFormKind::SpecMember { base, row, .. } => match &plan
                .result_type(*base)
                .specifications
            {
                TypeSpecification::Spec { spec } if *spec == plan.identity => {
                    row.ends.first().expect("BUG: a row read has a first end")
                }
                TypeSpecification::Spec { .. } => continue,
                other => panic!("BUG: row member read on cell {index} of non-spec type {other:?}"),
            },
            _ => continue,
        };
        match end {
            RowEnd::Rule(rule) => {
                rules_read.insert(rule);
            }
            RowEnd::Data(path) => {
                if let Some(ReferenceEnd::Rule(rule)) = plan.reference_ends.get(path) {
                    rules_read.insert(rule);
                }
            }
        }
    }
    plan.rules
        .keys()
        .filter(|rule| rules_read.contains(rule))
        .cloned()
        .collect()
}

impl ExecutionPlan {
    /// Template plan rows of `identity` read: this plan for its own self-lists.
    pub(crate) fn row_template(&self, identity: &SpecIdentity) -> &ExecutionPlan {
        if *identity == self.identity {
            return self;
        }
        self.row_templates
            .get(identity)
            .unwrap_or_else(|| panic!("BUG: rows of '{identity}' without a template plan"))
    }

    /// Row of this plan's `uses` slot `slot` as read on `row` of this plan (`None` for
    /// the plan's own context): the nested row the holder bound over it, else the
    /// plan's own import row.
    ///
    /// `dynamic_rows` supplies the body chosen for a `uses … -> effective` slot of this
    /// plan (keyed by the slot path). Nested `RowBinding::Instance` hops ignore it.
    /// Pass `None` when no dynamic imports are in play (or only nested instance hops).
    pub(crate) fn import_row<'a>(
        &'a self,
        row: Option<&'a RowInstance>,
        slot: &DataPath,
        dynamic_rows: Option<&'a HashMap<DataPath, Arc<RowInstance>>>,
    ) -> &'a Arc<RowInstance> {
        let (index, _, definition) = self
            .data
            .get_full(slot)
            .unwrap_or_else(|| panic!("BUG: `uses` slot '{slot}' is not data of its plan"));
        if let Some(RowBinding::Instance(nested)) = row.and_then(|row| row.bindings[index].as_ref())
        {
            return nested;
        }
        let DataDefinition::Import { row, pin, .. } = definition else {
            panic!("BUG: data '{slot}' read as a `uses` slot is not an import");
        };
        if pin.is_some() {
            return dynamic_rows
                .and_then(|rows| rows.get(slot))
                .unwrap_or_else(|| {
                    panic!("BUG: dynamic import '{slot}' hop without a selected body")
                });
        }
        row
    }

    /// Select the candidate body of a dynamic import active at `at`.
    pub(crate) fn select_dynamic_import_row(
        &self,
        slot: &DataPath,
        at: &crate::parsing::ast::EffectiveDate,
    ) -> Option<&Arc<RowInstance>> {
        let DataDefinition::Import {
            pin: Some(_),
            candidates,
            ..
        } = self.data.get(slot)?
        else {
            return None;
        };
        candidates
            .iter()
            .filter(|(identity, _)| identity.slice <= *at)
            .max_by(|a, b| a.0.slice.cmp(&b.0.slice))
            .map(|(_, row)| row)
    }

    /// Every [`Self::data`] slot that is not an Import marker: the members show and
    /// the row field sets expose.
    pub(crate) fn input_slots(&self) -> impl Iterator<Item = (&DataPath, &DataDefinition)> {
        self.data
            .iter()
            .filter(|(_, definition)| !matches!(definition, DataDefinition::Import { .. }))
    }

    /// Row-suppliable slots a read of `end` on a row of this plan touches: the slot
    /// `end` is, or every slot the rule `end` names (or the data reference `end`
    /// resolves to) reads. `unbound` marks slots the spec gives no value: a row
    /// appended inline must write each of them.
    pub(crate) fn row_end_reads(&self, end: &RowEnd) -> Vec<RowFieldRead> {
        let rule_reads = |rule: &RulePath| -> Vec<&DataPath> {
            let rule_pos = u32::try_from(self.rules.get_index_of(rule).unwrap_or_else(|| {
                panic!(
                    "BUG: row rule '{rule}' missing from plan '{}'",
                    self.spec_name
                )
            }))
            .expect("BUG: rule count exceeds u32");
            self.needed_by_rules
                .iter()
                .enumerate()
                .filter(|(_, consumers)| consumers.rules.contains(&rule_pos))
                .map(|(data_pos, _)| {
                    self.data
                        .get_index(data_pos)
                        .expect("BUG: needed_by_rules position outside plan.data")
                        .0
                })
                .collect()
        };
        let slots: Vec<&DataPath> = match end {
            RowEnd::Data(path) => match self.reference_ends.get(path) {
                Some(ReferenceEnd::Rule(rule)) => rule_reads(rule),
                Some(ReferenceEnd::Promptable(_)) | None => {
                    self.bindable_row_field(path).into_iter().collect()
                }
            },
            RowEnd::Rule(rule) => rule_reads(rule),
        };
        slots
            .into_iter()
            .map(|slot| RowFieldRead {
                slot: slot.clone(),
                unbound: self.data[slot].value().is_none(),
            })
            .collect()
    }

    /// Slot an evaluation-data row of this plan binds when a field read lands on
    /// `path`: the slot itself, or the end of its reference chain. `None` when the slot
    /// is bound by the spec, computed by a rule, or an instance marker.
    fn bindable_row_field<'a>(&'a self, path: &'a DataPath) -> Option<&'a DataPath> {
        match self
            .data
            .get(path)
            .unwrap_or_else(|| panic!("BUG: row field '{path}' absent from plan.data"))
        {
            DataDefinition::Value { .. }
            | DataDefinition::TypeDeclaration { .. }
            | DataDefinition::InstanceList { .. } => Some(path),
            DataDefinition::Reference { .. } => match self.reference_ends.get(path) {
                Some(ReferenceEnd::Promptable(target)) => Some(target),
                Some(ReferenceEnd::Rule(_)) => None,
                None => {
                    panic!("BUG: reference '{path}' missing from reference_ends after planning")
                }
            },
            DataDefinition::Import { .. } => None,
        }
        .filter(|target| !self.data[*target].is_bound())
    }
}

/// Nesting of list commands a plan supports: one bit per level in [`ExecutionPlan::row_mask`].
const MAX_ROW_LEVELS: u32 = u32::BITS;

/// [`ExecutionPlan::row_mask`]: a list-row leaf (`RowField` or `RowValue`) sets its
/// level, a list command clears its own level, every other cell takes the union of
/// its children.
fn build_row_mask(normal_forms: &[NormalForm]) -> Result<Vec<u32>, Error> {
    use crate::planning::normalize::{LeafKind, NormalFormKind};

    let mut mask: Vec<Option<u32>> = vec![None; normal_forms.len()];
    let mut stack: Vec<(usize, bool)> = Vec::new();
    for root in 0..normal_forms.len() {
        if mask[root].is_some() {
            continue;
        }
        stack.push((root, false));
        while let Some((index, children_done)) = stack.pop() {
            if mask[index].is_some() {
                continue;
            }
            let cell = &normal_forms[index];
            if !children_done {
                stack.push((index, true));
                cell.kind.for_each_child(|child| {
                    if mask[child.index()].is_none() {
                        stack.push((child.index(), false));
                    }
                });
                continue;
            }
            let mut union = 0u32;
            cell.kind.for_each_child(|child| {
                union |= mask[child.index()]
                    .expect("BUG: child row mask missing after post-order visit");
            });
            let level = match &cell.kind {
                NormalFormKind::RowField { level, .. }
                | NormalFormKind::Leaf(LeafKind::RowValue { level }) => Some(*level),
                _ => None,
            };
            let bits = match (level, &cell.kind) {
                (Some(level), _) => {
                    if level >= MAX_ROW_LEVELS {
                        return Err(Error::validation(
                            format!(
                                "list commands nest {} levels deep; at most {MAX_ROW_LEVELS} are supported",
                                level + 1
                            ),
                            cell.source.clone(),
                            None::<String>,
                        ));
                    }
                    union | (1u32 << level)
                }
                (
                    None,
                    NormalFormKind::ListCommand {
                        row_level: Some(level),
                        ..
                    },
                ) if *level < MAX_ROW_LEVELS => union & !(1u32 << level),
                (None, _) => union,
            };
            mask[index] = Some(bits);
        }
    }
    Ok(mask
        .into_iter()
        .map(|bits| bits.expect("BUG: row mask missing for a cell"))
        .collect())
}

/// What evaluating one input cell needs: a data slot or a rule.
enum InputNeed {
    Data(DataPath),
    Rule(RulePath),
}

/// Pass-1 accumulator of [`ExecutionPlan::build_needed_by_rules`] for one rule.
struct NeedsMarker<'a> {
    plan: &'a ExecutionPlan,
    rule_pos: usize,
    bits: &'a mut [u64],
    bits_by_rule: &'a [Vec<u64>],
}

impl NeedsMarker<'_> {
    fn mark(&mut self, need: InputNeed) {
        match need {
            InputNeed::Rule(rule) => {
                let ref_pos =
                    self.plan.rules.get_index_of(&rule).unwrap_or_else(|| {
                        panic!("BUG: needed rule '{rule}' missing from plan.rules")
                    });
                if ref_pos >= self.rule_pos {
                    panic!(
                        "BUG: needed rule '{rule}' must precede '{}' in topo order",
                        self.plan.rules[self.rule_pos].path
                    );
                }
                for (word, dep_word) in self.bits.iter_mut().zip(&self.bits_by_rule[ref_pos]) {
                    *word |= *dep_word;
                }
            }
            InputNeed::Data(path) => {
                let data_pos =
                    self.plan.data.get_index_of(&path).unwrap_or_else(|| {
                        panic!("BUG: needed data '{path}' missing from plan.data")
                    });
                self.bits[data_pos / 64] |= 1u64 << (data_pos % 64);
            }
        }
    }

    fn mark_scale_target(&mut self, stack: &mut Vec<NormalFormId>, target: &ReferenceTarget) {
        if let Some(&cell) = self.plan.member_targets.get(target) {
            stack.push(cell);
            return;
        }
        let segments = reference_target_segments(target);
        if segments.is_empty() {
            match target {
                ReferenceTarget::Rule(rule) => self.mark(InputNeed::Rule(rule.clone())),
                ReferenceTarget::Data(path) => {
                    if let Some(need) = self.plan.data_read_needs(path) {
                        self.mark(need);
                    }
                }
            }
            return;
        }
        // Uses-qualified target that is a `uses` Import of this plan must have a
        // member cell. A first hop that is not this plan's import (list name, nested
        // element's uses) settles in that row template — not as a SpecMember here.
        if first_hop_is_uses_import(&self.plan.data, target) {
            panic!(
                "BUG: uses-qualified scale target '{target}' missing from member_targets of plan '{}'",
                self.plan.spec_name
            );
        }
    }
}

/// Ensure every [`ExecutionPlan::data`] path has a [`LeafKind::DataPath`] cell in `normal_forms`
/// and return the path → id index. Expression lowering already creates leaves for
/// referenced paths; unused paths get orphan leaves appended so evaluation can
/// still store bindings/defaults/reference copies in the value table.
///
/// Rule reference cells share a body's Kind, so a `Leaf(DataPath)` cell may carry
/// `rule_ref`. Those cells hold the referenced rule's value in the value table and
/// must never be the binding slot for the path.
///
/// Fold survivors also share a body's Kind and carry `origin` (identity elim of
/// `x * 1` → DataPath `x` with the Product as pre-image). Those cells are the
/// rule body's explanation root, not the binding slot: intake must land on the
/// origin-free leaf so an explain walk still visits the pre-image.
fn ensure_data_leaves(
    normal_forms: &mut Vec<NormalForm>,
    input_keys: &[DataPath],
    data: &IndexMap<DataPath, DataDefinition>,
) -> IndexMap<DataPath, NormalFormId> {
    use crate::planning::normalize::LeafKind;
    use crate::planning::normalize::NormalFormKind;

    let mut data_leaf: IndexMap<DataPath, NormalFormId> = IndexMap::new();
    for (index, cell) in normal_forms.iter().enumerate() {
        if cell.rule_ref.is_some() || cell.origin.is_some() {
            continue;
        }
        if let NormalFormKind::Leaf(LeafKind::DataPath(path)) = &cell.kind {
            data_leaf.insert(path.clone(), NormalFormId::from_index(index));
        }
    }
    for path in input_keys {
        if data_leaf.contains_key(path) {
            continue;
        }
        let id = NormalFormId::from_index(normal_forms.len());
        normal_forms.push(NormalForm {
            kind: NormalFormKind::Leaf(LeafKind::DataPath(path.clone())),
            result_type: data_path_result_type(data, path),
            fold_types: Vec::new(),
            source: None,
            origin: None,
            rule_ref: None,
        });
        data_leaf.insert(path.clone(), id);
    }
    data_leaf
}

/// Fill show/response cache fields that are pure functions of the plan and its
/// owning LemmaSpec / LemmaSpecSet. Called once after [`build_execution_plan`]
/// succeeds so [`Engine::show`] / [`Engine::evaluate`] only look up the plan.
pub(crate) fn attach_show_cache(
    plan: &mut ExecutionPlan,
    lemma_spec_set: &LemmaSpecSet,
    spec: &LemmaSpec,
) {
    let (effective_from, effective_to) = lemma_spec_set.effective_range(spec);
    plan.effective_from = effective_from;
    plan.effective_to = effective_to;
    plan.start_line = spec.start_line;
    plan.source_type = spec.source_type.clone();
    plan.data_display = build_data_display(plan);
}

fn build_data_display(plan: &ExecutionPlan) -> IndexMap<DataPath, ShowDataCache> {
    let mut out = IndexMap::new();
    for (path, data) in &plan.data {
        insert_data_display_entry(&mut out, path, data);
    }
    out
}

fn insert_data_display_entry(
    out: &mut IndexMap<DataPath, ShowDataCache>,
    path: &DataPath,
    data: &DataDefinition,
) {
    if data.schema_type().is_none() || matches!(data, DataDefinition::Reference { .. }) {
        return;
    }
    let value = match data {
        DataDefinition::Value {
            value,
            resolved_type,
            ..
        } => Some(BoundValueKind {
            value: value.value.clone(),
            unit: resolved_type.unit().map(Arc::from),
            instance_factors: None,
        }),
        _ => data.bound_value().cloned(),
    };
    let suggestion = data.bound_suggestion().cloned();
    if value.is_some() || suggestion.is_some() {
        out.insert(path.clone(), ShowDataCache { value, suggestion });
    }
}

/// Whether a show bound value is the fill or the suggestion for a data path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShowBoundRole {
    Value,
    Suggestion,
}

pub(crate) fn show_bound_result_value(
    bound: &BoundValueKind,
    schema_type: &LemmaType,
    input_key: &str,
    role: ShowBoundRole,
) -> RuleResultValue {
    let (fill, suggestion) = match role {
        ShowBoundRole::Value => (Some(bound), None),
        ShowBoundRole::Suggestion => (None, Some(bound)),
    };
    let display_type = lemma_type_with_display_binding(schema_type, None, fill, suggestion);
    type_scoped_result_value_from_literal(&bound.to_literal(), &display_type)
        .unwrap_or_else(|failure| {
            let role_name = match role {
                ShowBoundRole::Value => "value",
                ShowBoundRole::Suggestion => "suggestion",
            };
            panic!(
                "BUG: show {role_name} value for '{input_key}' failed type_scoped_result_value_from_literal: {}",
                rule_result_value_failure_message(failure)
            )
        })
}

/// Show value of instance list `path`: its spec-written rows in order, each with the
/// fields the row binds and no `result` (a row has no value of its own to display).
/// Literal fields carry their value; referenced fields carry
/// [`RuleResultValue::reference`]. `None` when the list has no spec-written rows.
pub(crate) fn show_instance_list_value(
    plan: &ExecutionPlan,
    path: &DataPath,
) -> Option<RuleResultValue> {
    let source = plan
        .instance_lists
        .get(path)
        .expect("BUG: instance list data without a source");
    if source.store.rows.is_empty() {
        return None;
    }
    let template = plan
        .row_templates
        .get(&source.identity)
        .expect("BUG: list with spec-written rows has a row template");
    let mut items = Vec::with_capacity(source.store.rows.len());
    for row in &source.store.rows {
        items.push(RuleResultValue {
            fields: Some(show_written_row_fields(plan, row, template)),
            ..Default::default()
        });
    }
    Some(RuleResultValue {
        list: Some(items),
        ..Default::default()
    })
}

/// Fields a spec-written row binds, keyed by their path under the row. A nested `uses`
/// instance the row writes through (`-> with item.price: 3 eur`) shows the fields it
/// writes there, not those the row spec's own `uses` already binds.
fn show_written_row_fields(
    plan: &ExecutionPlan,
    row: &RowInstance,
    template: &ExecutionPlan,
) -> IndexMap<String, RuleResultValue> {
    struct Level<'r> {
        row: &'r RowInstance,
        base: Option<&'r RowInstance>,
        template: &'r ExecutionPlan,
        prefix: String,
        next: usize,
    }
    let mut fields = IndexMap::new();
    let mut levels: Vec<Level<'_>> = vec![Level {
        row,
        base: None,
        template,
        prefix: String::new(),
        next: 0,
    }];
    while let Some(level) = levels.last_mut() {
        let slot = level.next;
        let Some((field_path, definition)) = level.template.data.get_index(slot) else {
            levels.pop();
            continue;
        };
        level.next += 1;
        let binding = level.row.bindings[slot].as_ref();
        let base_binding = level.base.and_then(|base| base.bindings[slot].as_ref());
        let copied = match (binding, base_binding) {
            (Some(binding), Some(base)) => binding.is_copy_of(base),
            (None, _) => true,
            (Some(_), None) => false,
        };
        if copied {
            continue;
        }
        let key = format!("{}{}", level.prefix, field_path.input_key());
        let value = match binding.expect("BUG: unbound slots are skipped above") {
            RowBinding::Rows { .. } => continue,
            RowBinding::Bound(OperationResult::Value(bound)) => {
                let schema = definition
                    .resolved_type_arc()
                    .expect("BUG: stored row bound field has a resolved type");
                show_bound_result_value(bound, schema, &key, ShowBoundRole::Value)
            }
            RowBinding::Bound(OperationResult::Veto(_)) => {
                unreachable!("BUG: stored row literal binding is never a Veto")
            }
            RowBinding::Deferred { target, .. } => RuleResultValue {
                reference: Some(match target {
                    ReferenceTarget::Data(target) => target.input_key(),
                    ReferenceTarget::Rule(target) => target.input_key(),
                }),
                ..Default::default()
            },
            RowBinding::Instance(nested) => {
                let DataDefinition::Import { row: base, .. } = definition else {
                    panic!("BUG: nested instance binding on non-`uses` slot '{field_path}'");
                };
                let nested_template = plan.row_template(&nested.identity);
                levels.push(Level {
                    row: nested,
                    base: Some(base),
                    template: nested_template,
                    prefix: format!("{key}."),
                    next: 0,
                });
                continue;
            }
        };
        fields.insert(key, value);
    }
    fields
}

/// One data entry in a [`Show`].
///
/// A named struct instead of a tuple so JSON-native consumers (TypeScript, Python, ...)
/// get stable field names. `fill` is a spec literal or literal `with` binding;
/// `suggestion` is a `-> suggest ...` hint only.
/// Empty `needed_by_rules` means the slot is offered for reuse (`data x: alias.slot`),
/// not needed by this spec's remaining rules after normalize.
#[derive(Debug, Clone, PartialEq)]
pub struct ShowData {
    pub lemma_type: LemmaType,
    /// Import hops to this slot (`[]` = this spec). Clone of [`DataPath::segments`].
    pub path: Vec<PathSegment>,
    pub value: Option<RuleResultValue>,
    pub suggestion: Option<RuleResultValue>,
    /// Show.rules keys that transitively need this data after normalize.
    /// Show.data only includes slots where this is non-empty.
    pub needed_by_rules: Vec<String>,
}

/// Consumer [`Engine::show`] result: local offered types, inputs rules need,
/// this spec's own rules (empty path), and resolved temporal window.
/// Source: [`Engine::source`]. Branch Rule leaves may name reachable imports that
/// are not keys of [`Self::rules`]; follow those via `show` on the imported spec.
#[derive(Debug, Clone, PartialEq)]
pub struct Show {
    /// Interned repository name (`specs()` string). `None` = unnamed workspace.
    pub repository: Option<String>,
    pub spec: String,
    pub commentary: Option<String>,
    pub effective_from: Option<DateTimeValue>,
    pub effective_to: Option<DateTimeValue>,
    /// Planning instant of the execution-plan slice that produced this Show.
    /// Distinguishes temporal dependency slices that share the same declared window.
    pub plan_effective: EffectiveDate,
    pub start_line: usize,
    pub source_type: Option<SourceType>,
    /// Local `data` declarations (empty path; type or value), declaration order.
    pub types: indexmap::IndexMap<String, LemmaType>,
    /// Inputs rules need (`needed_by_rules` non-empty), local or imported.
    pub data: indexmap::IndexMap<String, ShowData>,
    pub rules: indexmap::IndexMap<String, ShowRule>,
    /// Spec metadata, in declaration order.
    pub meta: IndexMap<String, Value>,
}

impl std::fmt::Display for Show {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.repository {
            Some(repository) => write!(f, "Spec: {} {}", repository, self.spec)?,
            None => write!(f, "Spec: {}", self.spec)?,
        }
        write_effective_window(f, self.effective_from.as_ref(), self.effective_to.as_ref())?;

        if let Some(commentary) = &self.commentary {
            for line in commentary.lines() {
                if line.is_empty() {
                    writeln!(f)?;
                } else {
                    write!(f, "\n  {line}")?;
                }
            }
        }

        if !self.meta.is_empty() {
            write!(f, "\n\nMeta:")?;
            let mut entries: Vec<(&String, &Value)> = self.meta.iter().collect();
            entries.sort_by_key(|(k, _)| *k);
            for (key, value) in entries {
                write!(f, "\n  {}: {}", key, value)?;
            }
        }

        if !self.types.is_empty() {
            write!(f, "\n\nTypes:")?;
            for (name, lemma_type) in &self.types {
                write!(f, "\n  {} ({})", name, lemma_type.specifications)?;
                for line in type_detail_lines(&lemma_type.specifications) {
                    write!(f, "\n    {}", line)?;
                }
                let help = lemma_type.specifications.help();
                if !help.is_empty() {
                    write!(f, "\n    help: {}", help)?;
                }
            }
        }

        if !self.data.is_empty() {
            write!(f, "\n\nData:")?;
            for (name, entry) in &self.data {
                write!(f, "\n  {} ({})", name, entry.lemma_type.specifications)?;
                for line in type_detail_lines(&entry.lemma_type.specifications) {
                    write!(f, "\n    {}", line)?;
                }
                if let Some(line) = option_detail_line(&entry.lemma_type) {
                    write!(f, "\n    {}", line)?;
                }
                let help = entry.lemma_type.specifications.help();
                if !help.is_empty() {
                    write!(f, "\n    help: {}", help)?;
                }
                match &entry.value {
                    Some(RuleResultValue {
                        list: Some(rows), ..
                    }) if rows.iter().all(|row| row.fields.is_some()) => {
                        write!(f, "\n    value:")?;
                        for (index, row) in rows.iter().enumerate() {
                            let fields = row
                                .fields
                                .as_ref()
                                .expect("BUG: guard checked every row has fields")
                                .iter()
                                .map(|(name, field)| format!("{name} {field}"))
                                .collect::<Vec<_>>()
                                .join(", ");
                            write!(f, "\n      {}: {fields}", index + 1)?;
                        }
                    }
                    Some(val) => write!(f, "\n    value: {}", val)?,
                    None => {}
                }
                if let Some(val) = &entry.suggestion {
                    write!(f, "\n    suggestion: {}", val)?;
                }
                if !entry.needed_by_rules.is_empty() {
                    write!(
                        f,
                        "\n    needed_by_rules: {}",
                        entry.needed_by_rules.join(", ")
                    )?;
                }
            }
        }

        if !self.rules.is_empty() {
            write!(f, "\n\nRules:")?;
            for (name, rule) in &self.rules {
                write!(f, "\n  {} ({})", name, rule.lemma_type.specifications)?;
                if !rule.depends_on_rules.is_empty() {
                    write!(
                        f,
                        "\n    depends_on_rules: {}",
                        rule.depends_on_rules
                            .iter()
                            .map(|path| path.input_key())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )?;
                }
            }
        }

        if self.types.is_empty() && self.data.is_empty() && self.rules.is_empty() {
            write!(f, "\n  (no types, data, or rules)")?;
        }

        Ok(())
    }
}

/// Produce a human-readable summary of type constraints, or `None` when there
/// are no constraints worth showing (e.g. bare `boolean`).
/// Returns one formatted string per constraint or property of the type specification.
/// Uses `display_str` for all rational bounds so they render as decimals,
/// not as raw fractions.
pub fn type_detail_lines(spec: &TypeSpecification) -> Vec<String> {
    let mut lines = Vec::new();
    match spec {
        TypeSpecification::Measure {
            minimum,
            maximum,
            units,
            ..
        } => {
            let unit_names: Vec<&str> = units.0.iter().map(|u| u.name.as_str()).collect();
            if !unit_names.is_empty() {
                lines.push(format!("units: {}", unit_names.join(", ")));
            }
            if let Some((magnitude, unit_name)) = minimum {
                lines.push(format!(
                    "minimum: {} {}",
                    magnitude.display_str(),
                    unit_name
                ));
            }
            if let Some((magnitude, unit_name)) = maximum {
                lines.push(format!(
                    "maximum: {} {}",
                    magnitude.display_str(),
                    unit_name
                ));
            }
        }
        TypeSpecification::Number {
            minimum,
            maximum,
            decimals,
            ..
        } => {
            if let Some(d) = decimals {
                lines.push(format!("decimals: {}", d));
            }
            if let Some(v) = minimum {
                lines.push(format!("minimum: {}", v.display_str()));
            }
            if let Some(v) = maximum {
                lines.push(format!("maximum: {}", v.display_str()));
            }
        }
        TypeSpecification::Ratio {
            minimum,
            maximum,
            units,
            ..
        } => {
            let unit_names: Vec<&str> = units.0.iter().map(|u| u.name.as_str()).collect();
            if !unit_names.is_empty() {
                lines.push(format!("units: {}", unit_names.join(", ")));
            }
            if let Some(v) = minimum {
                lines.push(format!("minimum: {}", v.display_str()));
            }
            if let Some(v) = maximum {
                lines.push(format!("maximum: {}", v.display_str()));
            }
        }
        TypeSpecification::Text { length, .. } => {
            if let Some(l) = length {
                lines.push(format!("length: {}", l));
            }
        }
        TypeSpecification::Date {
            minimum, maximum, ..
        } => {
            if let Some(v) = minimum {
                lines.push(format!("minimum: {}", v));
            }
            if let Some(v) = maximum {
                lines.push(format!("maximum: {}", v));
            }
        }
        TypeSpecification::Time {
            minimum, maximum, ..
        } => {
            if let Some(v) = minimum {
                lines.push(format!("minimum: {}", v));
            }
            if let Some(v) = maximum {
                lines.push(format!("maximum: {}", v));
            }
        }
        TypeSpecification::MeasureRange {
            lower,
            upper,
            minimum,
            maximum,
            units,
            ..
        } => {
            let unit_names: Vec<&str> = units.0.iter().map(|u| u.name.as_str()).collect();
            if !unit_names.is_empty() {
                lines.push(format!("units: {}", unit_names.join(", ")));
            }
            if let Some((magnitude, unit_name)) = lower {
                lines.push(format!("lower: {} {}", magnitude.display_str(), unit_name));
            }
            if let Some((magnitude, unit_name)) = upper {
                lines.push(format!("upper: {} {}", magnitude.display_str(), unit_name));
            }
            if let Some((magnitude, unit_name)) = minimum {
                lines.push(format!(
                    "minimum: {} {}",
                    magnitude.display_str(),
                    unit_name
                ));
            }
            if let Some((magnitude, unit_name)) = maximum {
                lines.push(format!(
                    "maximum: {} {}",
                    magnitude.display_str(),
                    unit_name
                ));
            }
        }
        TypeSpecification::RatioRange {
            lower,
            upper,
            minimum,
            maximum,
            units,
            ..
        } => {
            let unit_names: Vec<&str> = units.0.iter().map(|u| u.name.as_str()).collect();
            if !unit_names.is_empty() {
                lines.push(format!("units: {}", unit_names.join(", ")));
            }
            if let Some(v) = lower {
                lines.push(format!("lower: {}", v.display_str()));
            }
            if let Some(v) = upper {
                lines.push(format!("upper: {}", v.display_str()));
            }
            if let Some(v) = minimum {
                lines.push(format!("minimum: {}", v.display_str()));
            }
            if let Some(v) = maximum {
                lines.push(format!("maximum: {}", v.display_str()));
            }
        }
        TypeSpecification::NumberRange {
            lower,
            upper,
            minimum,
            maximum,
            ..
        } => {
            if let Some(v) = lower {
                lines.push(format!("lower: {}", v.display_str()));
            }
            if let Some(v) = upper {
                lines.push(format!("upper: {}", v.display_str()));
            }
            if let Some(v) = minimum {
                lines.push(format!("minimum: {}", v.display_str()));
            }
            if let Some(v) = maximum {
                lines.push(format!("maximum: {}", v.display_str()));
            }
        }
        TypeSpecification::DateRange {
            lower,
            upper,
            minimum,
            maximum,
            ..
        } => {
            if let Some(v) = lower {
                lines.push(format!("lower: {}", v));
            }
            if let Some(v) = upper {
                lines.push(format!("upper: {}", v));
            }
            if let Some((magnitude, unit_name)) = minimum {
                lines.push(format!(
                    "minimum: {} {}",
                    magnitude.display_str(),
                    unit_name
                ));
            }
            if let Some((magnitude, unit_name)) = maximum {
                lines.push(format!(
                    "maximum: {} {}",
                    magnitude.display_str(),
                    unit_name
                ));
            }
        }
        TypeSpecification::TimeRange {
            lower,
            upper,
            minimum,
            maximum,
            ..
        } => {
            if let Some(v) = lower {
                lines.push(format!("lower: {}", v));
            }
            if let Some(v) = upper {
                lines.push(format!("upper: {}", v));
            }
            if let Some((magnitude, unit_name)) = minimum {
                lines.push(format!(
                    "minimum: {} {}",
                    magnitude.display_str(),
                    unit_name
                ));
            }
            if let Some((magnitude, unit_name)) = maximum {
                lines.push(format!(
                    "maximum: {} {}",
                    magnitude.display_str(),
                    unit_name
                ));
            }
        }
        TypeSpecification::Boolean { .. }
        | TypeSpecification::Veto { .. }
        | TypeSpecification::Undetermined
        | TypeSpecification::Spec { .. } => {}
        TypeSpecification::List { element, key, help } => {
            lines.push(format!("element: {element}"));
            if !help.is_empty() {
                lines.push(format!("help: {help}"));
            }
            match key {
                crate::planning::semantics::ListKey::Numbered => {
                    lines.push("key: numbered".to_string());
                }
                crate::planning::semantics::ListKey::Fields(fields) => {
                    lines.push(format!(
                        "key: {}",
                        fields
                            .iter()
                            .map(|field| field.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
            }
        }
    }
    lines
}

impl ExecutionPlan {
    /// Expression-scope unit index (local types plus direct `uses` imports).
    pub(crate) fn expression_unit_index(&self) -> &UnitIndex {
        &self.resolved_types.unit_index
    }

    /// [`Self::show_needed_by`], each list in input-key order: a catalog rule needs the
    /// data it reads that no binding fills, the data `bits_by_rule` gives a local rule,
    /// and the needs of every rule it reads (including reachable imports walked in
    /// `show_order`). A local rule is not listed for the data `bits_by_rule` already
    /// gives it. Reader positions are catalog ([`Self::show_rules`]) indices only.
    fn build_show_needed_by(
        &self,
        show_order: &[RulePath],
        show_edges: &BTreeMap<RulePath, BTreeSet<RulePath>>,
        show_imports: &crate::planning::graph::ShowImports,
        bits_by_rule: &[Vec<u64>],
    ) -> IndexMap<DataPath, Vec<u32>> {
        let mut paths: indexmap::IndexSet<&DataPath> = self.data.keys().collect();
        for reads in show_imports.data_reads.values() {
            paths.extend(reads);
        }
        let words = paths.len().div_ceil(64);
        let mut needs: HashMap<&RulePath, Vec<u64>> = HashMap::with_capacity(show_order.len());
        for path in show_order {
            let mut bits = match self.rules.get_index_of(path) {
                Some(rule_pos) => {
                    let mut bits = bits_by_rule[rule_pos].clone();
                    bits.resize(words, 0);
                    bits
                }
                None => vec![0u64; words],
            };
            for read in show_imports.data_reads.get(path).into_iter().flatten() {
                let position = paths
                    .get_index_of(read)
                    .expect("BUG: Show data read indexed above");
                bits[position / 64] |= 1u64 << (position % 64);
            }
            let reads = match show_imports.need_edges.get(path) {
                Some(edges) => edges,
                None => &show_edges[path],
            };
            for dependency in reads {
                let dependency_bits = needs
                    .get(dependency)
                    .expect("BUG: Show dependency needs computed before its reader");
                for (word, dependency_word) in bits.iter_mut().zip(dependency_bits) {
                    *word |= *dependency_word;
                }
            }
            needs.insert(path, bits);
        }
        let mut readers: Vec<(usize, &RulePath)> = self.show_rules.keys().enumerate().collect();
        readers.sort_by_cached_key(|(_, path)| path.input_key());
        let mut lists: Vec<Vec<u32>> = vec![Vec::new(); paths.len()];
        for (rule_pos, path) in readers {
            let rule_pos = u32::try_from(rule_pos).expect("BUG: Show rule count exceeds u32");
            // `bits_by_rule` covers this plan's data only; import data is never listed.
            let listed: &[u64] = match self.rules.get_index_of(path) {
                Some(local_pos) => &bits_by_rule[local_pos],
                None => &[],
            };
            for (word_idx, &word) in needs[path].iter().enumerate() {
                let mut remaining = word & !listed.get(word_idx).copied().unwrap_or(0);
                while remaining != 0 {
                    let bit = remaining.trailing_zeros() as usize;
                    lists[word_idx * 64 + bit].push(rule_pos);
                    remaining &= remaining - 1;
                }
            }
        }
        paths
            .into_iter()
            .zip(lists)
            .filter(|(_, list)| !list.is_empty())
            .map(|(path, list)| (path.clone(), list))
            .collect()
    }

    /// Per [`Self::data`] position: [`Self::rules`] positions that transitively
    /// need that slot (through rule references). Row-field columns are sized and left
    /// empty for [`Self::fill_row_field_needs`]. Returns the per-rule data-slot bitsets.
    ///
    /// Built in plan topological order: at a rule-ref cell, OR the target rule's
    /// already-computed bitsets instead of descending Kind (rule references are
    /// evaluation boundaries). Walks the normalized body so constant-dead unless
    /// arms removed by normalize stay out of the index. Fields read on
    /// evaluation-data rows land on the instance-list slot's [`NeededByRules::fields`]
    /// column; fields read on spec-written rows are ordinary slots of this plan.
    fn build_needed_by_rules(&mut self) -> Vec<Vec<u64>> {
        use crate::planning::normalize::{LeafKind, NormalFormKind};

        let data_len = self.data.len();
        let words = data_len.div_ceil(64);
        let mut bits_by_rule: Vec<Vec<u64>> = Vec::with_capacity(self.rules.len());

        let mut visited = HashSet::new();
        let mut stack = Vec::new();

        // Pass 1: data-slot bitsets (topo / rule_ref OR). Evaluation-data row reads ignored here.
        for (rule_pos, (_, rule)) in self.rules.iter().enumerate() {
            let mut bits = vec![0u64; words];
            visited.clear();
            stack.push(rule.normal_form);
            {
                let mut needs = NeedsMarker {
                    plan: self,
                    rule_pos,
                    bits: &mut bits,
                    bits_by_rule: &bits_by_rule,
                };
                for target in &rule.scale_dependencies {
                    needs.mark_scale_target(&mut stack, target);
                }
            }
            while let Some(id) = stack.pop() {
                if !visited.insert(id) {
                    continue;
                }
                let nf = self.normal_form(id);
                let mut needs = NeedsMarker {
                    plan: self,
                    rule_pos,
                    bits: &mut bits,
                    bits_by_rule: &bits_by_rule,
                };
                if let Some(ref_path) = &nf.rule_ref {
                    needs.mark(InputNeed::Rule(ref_path.clone()));
                    continue;
                }
                match &nf.kind {
                    NormalFormKind::Leaf(LeafKind::DataPath(path)) => {
                        // Local slots only. Uses-qualified paths are SpecMember /
                        // member-target cells; `data_read_needs` keys `plan.data` by
                        // local name and must not see nested import FX scales.
                        if path.segments.is_empty() {
                            if let Some(need) = self.data_read_needs(path) {
                                needs.mark(need);
                            }
                            if let Some(rule) = self.options_rule_of(path) {
                                needs.mark(InputNeed::Rule(rule));
                            }
                        }
                    }
                    NormalFormKind::OrderedDispatch { .. } => {
                        let origin = nf.origin.unwrap_or_else(|| {
                            panic!(
                                "BUG: non-rule_ref OrderedDispatch must carry origin (rule '{}')",
                                rule.path
                            )
                        });
                        stack.push(origin);
                    }
                    _ => nf.kind.for_each_child(|child| stack.push(child)),
                }
            }
            bits_by_rule.push(bits);
        }

        let mut consumers: Vec<usize> = (0..self.rules.len()).collect();
        consumers.sort_by(|&a, &b| {
            self.rules[a]
                .path
                .input_key()
                .cmp(&self.rules[b].path.input_key())
        });

        let mut needed_by_rules: Vec<NeededByRules> = self
            .data
            .iter()
            .map(|(path, definition)| {
                let fields = match definition {
                    DataDefinition::InstanceList { .. } => {
                        let identity = &self
                            .instance_lists
                            .get(path)
                            .unwrap_or_else(|| {
                                panic!("BUG: InstanceList '{path}' missing from instance_lists")
                            })
                            .identity;
                        vec![Vec::new(); self.row_template(identity).row_field_count()]
                    }
                    _ => Vec::new(),
                };
                NeededByRules {
                    rules: Vec::new(),
                    fields,
                }
            })
            .collect();

        for &rule_pos in &consumers {
            let rule_id = u32::try_from(rule_pos).expect("BUG: rule count exceeds u32");
            let bits = &bits_by_rule[rule_pos];
            for (word_idx, &word) in bits.iter().enumerate() {
                let mut remaining = word;
                while remaining != 0 {
                    let bit = remaining.trailing_zeros() as usize;
                    let data_pos = word_idx * 64 + bit;
                    needed_by_rules[data_pos].rules.push(rule_id);
                    remaining &= remaining - 1;
                }
            }
        }
        self.needed_by_rules = needed_by_rules;
        bits_by_rule
    }

    /// Fills the row-field columns of [`Self::needed_by_rules`]: which rules need each row
    /// field of evaluation-data rows. Runs after [`Self::show_needed_by`] so reads of a
    /// self-recursive row see the needs of its `uses` imports.
    fn fill_row_field_needs(&mut self, bits_by_rule: &[Vec<u64>]) {
        use crate::planning::normalize::{LeafKind, NormalFormKind, NormalFormListOp};

        let mut visited = HashSet::new();
        let mut stack = Vec::new();
        // Evaluation-data row-field bitsets per row spec. Self RowEnd::Rule
        // expands via completed bits_by_rule (row rule reads are not topo edges).
        let mut row_bits_by_rule: Vec<BTreeMap<SpecIdentity, Vec<u64>>> =
            Vec::with_capacity(self.rules.len());
        for (rule_pos, (_, rule)) in self.rules.iter().enumerate() {
            let mut row_bits: BTreeMap<SpecIdentity, Vec<u64>> = BTreeMap::new();
            visited.clear();
            stack.push(rule.normal_form);
            while let Some(id) = stack.pop() {
                if !visited.insert(id) {
                    continue;
                }
                let nf = self.normal_form(id);
                if let Some(ref_path) = &nf.rule_ref {
                    let ref_pos = self.rules.get_index_of(ref_path).unwrap_or_else(|| {
                        panic!(
                            "BUG: rule_ref target '{ref_path}' missing from plan.rules (rule '{}')",
                            rule.path
                        )
                    });
                    if ref_pos >= rule_pos {
                        panic!(
                            "BUG: rule_ref target '{ref_path}' must precede '{}' in topo order",
                            rule.path
                        );
                    }
                    for (identity, dep_row) in &row_bits_by_rule[ref_pos] {
                        let entry = row_bits_entry(self, &mut row_bits, identity);
                        for (word, dep_word) in entry.iter_mut().zip(dep_row.iter()) {
                            *word |= *dep_word;
                        }
                    }
                    continue;
                }
                match &nf.kind {
                    NormalFormKind::RowField { identity, end, .. } => {
                        if self.has_row_template(identity) {
                            mark_row_end_needs(
                                self,
                                identity,
                                std::slice::from_ref(end),
                                bits_by_rule,
                                &mut row_bits,
                                &rule.path,
                            );
                        }
                        nf.kind.for_each_child(|child| stack.push(child));
                    }
                    NormalFormKind::SpecMember { base, row, .. } => {
                        let on_import_slot = match &self.normal_form(*base).kind {
                            NormalFormKind::Leaf(LeafKind::DataPath(path)) => {
                                matches!(self.data.get(path), Some(DataDefinition::Import { .. }))
                            }
                            _ => false,
                        };
                        if !on_import_slot {
                            let identity = match &self.result_type(*base).specifications {
                                TypeSpecification::Spec { spec } => spec,
                                other => panic!(
                                    "BUG: SpecMember row read on non-spec type {other:?} (rule '{}')",
                                    rule.path
                                ),
                            };
                            mark_row_end_needs(
                                self,
                                identity,
                                &row.ends,
                                bits_by_rule,
                                &mut row_bits,
                                &rule.path,
                            );
                        }
                        nf.kind.for_each_child(|child| stack.push(child));
                    }
                    NormalFormKind::ListCommand {
                        list,
                        op: NormalFormListOp::Find | NormalFormListOp::WithRow { .. },
                        row_level: Some(_),
                        ..
                    } => {
                        mark_list_key_fields(self, *list, &mut row_bits, &rule.path);
                        nf.kind.for_each_child(|child| stack.push(child));
                    }
                    NormalFormKind::OrderedDispatch { .. } => {
                        let origin = nf.origin.unwrap_or_else(|| {
                            panic!(
                                "BUG: non-rule_ref OrderedDispatch must carry origin (rule '{}')",
                                rule.path
                            )
                        });
                        stack.push(origin);
                    }
                    _ => nf.kind.for_each_child(|child| stack.push(child)),
                }
            }
            row_bits_by_rule.push(row_bits);
        }

        let mut consumers: Vec<usize> = (0..self.rules.len()).collect();
        consumers.sort_by(|&a, &b| {
            self.rules[a]
                .path
                .input_key()
                .cmp(&self.rules[b].path.input_key())
        });
        let mut row_fields: Vec<(usize, usize, u32)> = Vec::new();
        for &rule_pos in &consumers {
            let rule_id = u32::try_from(rule_pos).expect("BUG: rule count exceeds u32");
            for (identity, row_bits) in &row_bits_by_rule[rule_pos] {
                let list_positions = self.instance_list_data_positions(identity);
                let template_len = self.row_template(identity).row_field_count();
                for (word_idx, &word) in row_bits.iter().enumerate() {
                    let mut remaining = word;
                    while remaining != 0 {
                        let bit = remaining.trailing_zeros() as usize;
                        let field_pos = word_idx * 64 + bit;
                        assert!(
                            field_pos < template_len,
                            "BUG: row field bit {field_pos} past the row fields of '{identity}'"
                        );
                        for &list_pos in &list_positions {
                            row_fields.push((list_pos, field_pos, rule_id));
                        }
                        remaining &= remaining - 1;
                    }
                }
            }
        }
        for (list_pos, field_pos, rule_id) in row_fields {
            self.needed_by_rules[list_pos].fields[field_pos].push(rule_id);
        }
    }

    /// Position of `rule` in [`Self::show_rules`].
    fn show_rule_position(&self, rule: &RulePath) -> Option<u32> {
        self.show_rules
            .get_index_of(rule)
            .map(|position| u32::try_from(position).expect("BUG: Show rule count exceeds u32"))
    }

    /// Row fields of rows of this spec: its data, then the data of its `uses` imports
    /// Show reads ([`Self::show_import_data`]).
    pub(crate) fn row_field_count(&self) -> usize {
        self.data.len() + self.show_import_data.len()
    }

    /// Position of row field `input_key` among [`Self::row_field_count`].
    fn row_field_position(&self, input_key: &str) -> Option<usize> {
        if let Some(path) = self.input_key_index.get(input_key) {
            return self.data.get_index_of(path);
        }
        self.show_import_data
            .keys()
            .position(|path| path.input_key() == input_key)
            .map(|index| self.data.len() + index)
    }

    /// Whether rows of `identity` can come from evaluation data on this plan: it is this
    /// spec or a row spec of one of its data lists.
    fn has_row_template(&self, identity: &SpecIdentity) -> bool {
        *identity == self.identity || self.row_templates.contains_key(identity)
    }

    /// [`Self::data`] positions of instance lists whose element identity is `identity`.
    fn instance_list_data_positions(&self, identity: &SpecIdentity) -> Vec<usize> {
        self.instance_lists
            .iter()
            .filter(|(_, source)| source.identity == *identity)
            .map(|(path, _)| {
                self.data
                    .get_index_of(path)
                    .unwrap_or_else(|| panic!("BUG: instance list '{path}' absent from plan.data"))
            })
            .collect()
    }

    /// Union of [`NeededByRules::fields`] across instance lists of `identity`
    /// (alphabetical rule-name order already on each list column).
    pub(crate) fn row_field_needed_by_rules(
        &self,
        identity: &SpecIdentity,
        field_input_key: &str,
    ) -> Vec<u32> {
        let field_pos = self
            .row_template(identity)
            .row_field_position(field_input_key)
            .unwrap_or_else(|| {
                panic!("BUG: row field '{field_input_key}' is not a row field of '{identity}'")
            });
        let mut seen = HashSet::new();
        let mut union = Vec::new();
        for list_pos in self.instance_list_data_positions(identity) {
            let fields = &self.needed_by_rules[list_pos].fields;
            let consumers = fields.get(field_pos).unwrap_or_else(|| {
                panic!(
                    "BUG: needed_by_rules.fields len {} < field {field_pos} for list data pos {list_pos}",
                    fields.len()
                )
            });
            for &rule_id in consumers {
                if seen.insert(rule_id) {
                    union.push(rule_id);
                }
            }
        }
        union.sort_by(|&a, &b| {
            self.rules
                .get_index(a as usize)
                .expect("BUG: needed_by_rules rule id out of range")
                .1
                .path
                .input_key()
                .cmp(
                    &self
                        .rules
                        .get_index(b as usize)
                        .expect("BUG: needed_by_rules rule id out of range")
                        .1
                        .path
                        .input_key(),
                )
        });
        union
    }

    /// Promptable [`DataPath`] for a normal-form data leaf, following reference
    /// chain ends for references. `None` when the leaf is a rule-target, import
    /// marker, instance list, or a slot bound by the spec (not caller-promptable).
    ///
    /// Reference paths must appear in [`Self::reference_ends`].
    pub(crate) fn promptable_data_path<'a>(&'a self, path: &'a DataPath) -> Option<&'a DataPath> {
        let target = match self.data.get(path).unwrap_or_else(|| {
            panic!("BUG: normal-form DataPath leaf absent from plan.data: {path}")
        }) {
            DataDefinition::Value { .. } | DataDefinition::TypeDeclaration { .. } => path,
            DataDefinition::Reference { .. } => match self.reference_ends.get(path) {
                Some(ReferenceEnd::Promptable(target)) => target,
                Some(ReferenceEnd::Rule(_)) => return None,
                None => {
                    panic!("BUG: reference '{path}' missing from reference_ends after planning")
                }
            },
            DataDefinition::Import { .. } | DataDefinition::InstanceList { .. } => return None,
        };
        match self
            .data
            .get(target)
            .unwrap_or_else(|| panic!("BUG: reference end '{target}' absent from plan.data"))
        {
            DataDefinition::Value { .. } | DataDefinition::TypeDeclaration { .. }
                if !self.data[target].is_bound() =>
            {
                Some(target)
            }
            DataDefinition::Value { .. } | DataDefinition::TypeDeclaration { .. } => None,
            DataDefinition::InstanceList { .. } => None,
            DataDefinition::Reference { .. } | DataDefinition::Import { .. } => {
                panic!("BUG: reference end '{target}' is not a slot")
            }
        }
    }

    /// What reading `path` needs: the slot itself, the end of its reference chain, or
    /// the rule a reference chain ends at. An import marker needs nothing (its value
    /// is the instance).
    fn data_read_needs(&self, path: &DataPath) -> Option<InputNeed> {
        match self
            .data
            .get(path)
            .unwrap_or_else(|| panic!("BUG: data '{path}' missing from plan '{}'", self.spec_name))
        {
            DataDefinition::Value { .. }
            | DataDefinition::TypeDeclaration { .. }
            | DataDefinition::InstanceList { .. } => Some(InputNeed::Data(path.clone())),
            DataDefinition::Import { .. } => None,
            DataDefinition::Reference { .. } => {
                match self.reference_ends.get(path).unwrap_or_else(|| {
                    panic!("BUG: reference '{path}' missing from reference_ends")
                }) {
                    ReferenceEnd::Promptable(target) => Some(InputNeed::Data(target.clone())),
                    ReferenceEnd::Rule(rule) => Some(InputNeed::Rule(rule.clone())),
                }
            }
        }
    }

    /// Options rule of data `path`, when its type names one ([`options_rule_path`]).
    pub(crate) fn options_rule_of(&self, path: &DataPath) -> Option<RulePath> {
        let lemma_type = self.data.get(path)?.schema_type()?;
        let rule = options_rule_path(path, lemma_type)?;
        assert!(
            self.rules.contains_key(&rule),
            "BUG: options rule '{rule}' missing after planning validated it"
        );
        Some(rule)
    }

    /// Validate caller-requested rule names and return resolved [`RulePath`]s in
    /// [`Self::show_rules`] order, each once.
    ///
    /// `None` means all local rules (this spec's outputs). `Some(&[])` is an error.
    /// Unknown names in `Some` slice error. Explicit names are this spec's Show.rules
    /// keys only (empty path).
    pub fn validated_response_rule_paths(
        &self,
        rules: Option<&[String]>,
    ) -> Result<Vec<&RulePath>, Error> {
        let show_position = |path: &RulePath| {
            self.show_rules
                .get_index_of(path)
                .unwrap_or_else(|| panic!("BUG: response rule '{path}' missing from show_rules"))
        };
        let mut positions: Vec<usize> = match rules {
            None => self.rules.keys().map(show_position).collect(),
            Some([]) => {
                return Err(Error::request(
                    "at least one rule required".to_string(),
                    None::<String>,
                ));
            }
            Some(rules) => {
                let mut positions = Vec::with_capacity(rules.len());
                for rule_name in rules {
                    let path = self
                        .show_rule_index
                        .get(canonical_logical_name(rule_name).as_ref())
                        .ok_or_else(|| {
                            Error::request(
                                format!(
                                    "Rule '{rule_name}' not found in spec '{}'",
                                    self.spec_name
                                ),
                                None::<String>,
                            )
                        })?;
                    positions.push(show_position(path));
                }
                positions
            }
        };
        positions.sort_unstable();
        positions.dedup();
        Ok(positions
            .into_iter()
            .map(|position| {
                self.show_rules
                    .get_index(position)
                    .expect("BUG: show_rules position taken from show_rules")
                    .0
            })
            .collect())
    }

    /// Look up a Show.rules key (`RulePath::input_key`) among this spec's local rules.
    pub fn get_rule(&self, name: &str) -> Option<&ExecutableRule> {
        let canonical = canonical_logical_name(name);
        let path = self.show_rule_index.get(canonical.as_ref())?;
        self.rules.get(path)
    }

    /// Look up a normal-form cell by id.
    pub fn normal_form(&self, id: NormalFormId) -> &NormalForm {
        self.normal_forms.get(id.index()).unwrap_or_else(|| {
            panic!(
                "BUG: NormalFormId {} out of range (table len {})",
                id.index(),
                self.normal_forms.len()
            )
        })
    }

    /// Stamped result type of the cell at `id`.
    pub(crate) fn result_type(&self, id: NormalFormId) -> &Arc<LemmaType> {
        &self.normal_form(id).result_type
    }

    /// Position of `path` in [`Self::rules`]. Every rule path a plan cell
    /// references was planned, so absence is a bug.
    pub(crate) fn rule_index(&self, path: &RulePath) -> RuleIndex {
        RuleIndex(
            self.rules
                .get_index_of(path)
                .unwrap_or_else(|| panic!("BUG: rule '{}' missing from execution plan", path.rule)),
        )
    }

    pub(crate) fn rule_at(&self, index: RuleIndex) -> &ExecutableRule {
        self.rules
            .get_index(index.0)
            .map(|(_, rule)| rule)
            .unwrap_or_else(|| {
                panic!(
                    "BUG: rule index {} out of plan.rules range ({} rules)",
                    index.0,
                    self.rules.len()
                )
            })
    }

    fn compute_initial_values(&self) -> Vec<Option<OperationResult>> {
        let mut values: Vec<Option<OperationResult>> = vec![None; self.normal_forms.len()];
        for (path, definition) in &self.data {
            let leaf = *self
                .data_leaf
                .get(path)
                .expect("BUG: every plan.data path must have a data_leaf entry");
            if let Some(fill) = definition.bound_value() {
                values[leaf.index()] = Some(OperationResult::from_bound(fill.clone()));
            } else if let Some(literal) = definition.value() {
                let unit = definition
                    .schema_type()
                    .and_then(|schema| schema.unit().map(Arc::from));
                values[leaf.index()] = Some(OperationResult::from_bound(BoundValueKind {
                    value: literal.value,
                    unit,
                    instance_factors: None,
                }));
            }
        }
        self.copy_reference_targets(&mut values, |_, _| false);
        values
    }

    /// Copy each data-target reference's target value into its leaf, in
    /// `data_reference_order`, checked against the reference's declared type. A
    /// reference bound by the caller (`is_bound` of its data slot and path) or
    /// holding its own value keeps it.
    pub(crate) fn copy_reference_targets(
        &self,
        values: &mut [Option<OperationResult>],
        is_bound: impl Fn(usize, &DataPath) -> bool,
    ) {
        for reference_path in &self.data_reference_order {
            let Some((
                slot,
                _,
                DataDefinition::Reference {
                    target: ReferenceTarget::Data(target_path),
                    resolved_type,
                    local_value,
                    ..
                },
            )) = self.data.get_full(reference_path)
            else {
                unreachable!("BUG: data_reference_order holds only data-target references");
            };
            if local_value.is_some() || is_bound(slot, reference_path) {
                continue;
            }
            let leaf = self.data_slot_leaves[slot];
            let target_leaf = *self
                .data_leaf
                .get(target_path)
                .expect("BUG: reference target missing from data_leaf");
            values[leaf.index()] =
                match &values[target_leaf.index()] {
                    None => None,
                    Some(OperationResult::Veto(veto)) => Some(OperationResult::Veto(veto.clone())),
                    Some(OperationResult::Value(value)) if value.value.has_unscaled_measure() => {
                        Some(OperationResult::from_bound(value.clone()))
                    }
                    Some(OperationResult::Value(value)) => Some(match validate_value_against_type(
                        resolved_type.as_ref(),
                        &value.to_literal(),
                        &self.resolved_types.unit_index,
                        None,
                    ) {
                        Ok(()) => OperationResult::from_bound(value.clone()),
                        Err(msg) => OperationResult::Veto(VetoType::computation(format!(
                            "Reference '{reference_path}' violates its declared constraint: {msg}"
                        ))),
                    }),
                };
        }
    }

    pub(crate) fn populate_evaluation_tables(&mut self) {
        self.data_slot_leaves = self
            .data
            .keys()
            .map(|path| {
                *self
                    .data_leaf
                    .get(path)
                    .expect("BUG: every plan.data path must have a data_leaf entry")
            })
            .collect();
        self.initial_values = self.compute_initial_values();
        self.rule_ref_indexes = self
            .normal_forms
            .iter()
            .map(|cell| cell.rule_ref.as_ref().map(|path| self.rule_index(path)))
            .collect();
        self.walk_settled_leaves = self
            .data
            .iter()
            .filter_map(|(path, definition)| {
                let options = definition
                    .schema_type()
                    .is_some_and(|schema| schema.needs_runtime_option_check());
                let unscaled = definition
                    .value()
                    .is_some_and(|literal| literal.value.has_unscaled_measure())
                    || definition
                        .bound_value()
                        .is_some_and(|bound| bound.value.has_unscaled_measure())
                    || definition
                        .schema_type()
                        .is_some_and(LemmaType::has_dynamic_units);
                if options || unscaled {
                    Some(
                        *self
                            .data_leaf
                            .get(path)
                            .expect("BUG: every plan.data path must have a data_leaf entry"),
                    )
                } else {
                    None
                }
            })
            .collect();
    }
}

/// Follow an [`NormalFormKind::OrderedDispatch`] region to the origin arm body it selects,
/// using filled slots: a nested dispatch by its scrutinee slot, a conjunctive region by
/// its residual arms. Stops at rule-reference cells: those borrow another rule's Kind and
/// must compare as the origin arm body, not the body that rule would itself select.
/// `None` when a slot the walk reads is unset or vetoed (not decided yet).
pub(crate) fn resolve_nested_dispatch_body(
    plan: &ExecutionPlan,
    values: &[Option<OperationResult>],
    mut region: DispatchRegion,
) -> Option<NormalFormId> {
    use crate::computation::OperationResult;
    use crate::planning::normalize::NormalFormKind;
    use crate::planning::ordered_dispatch::region_of_scrutinee;

    for _ in 0..=plan.normal_forms.len() {
        let body = match region {
            DispatchRegion::Body(body) => body,
            DispatchRegion::Conjunctive(group) => {
                region = DispatchRegion::Body(conjunctive_winner_body(plan, values, group)?);
                continue;
            }
        };
        let cell = plan.normal_form(body);
        if cell.rule_ref.is_some() {
            return Some(body);
        }
        let (scrutinee, boundaries, regions) = match &cell.kind {
            NormalFormKind::OrderedDispatch {
                scrutinee,
                boundaries,
                regions,
            } => (scrutinee, boundaries, regions),
            NormalFormKind::Piecewise(_) => {
                panic!("BUG: single-scrutinee region body is a Piecewise, not a conjunctive group")
            }
            NormalFormKind::Leaf(_)
            | NormalFormKind::RowField { .. }
            | NormalFormKind::Sum(_)
            | NormalFormKind::Product(_)
            | NormalFormKind::Subtract(_, _)
            | NormalFormKind::Divide(_, _)
            | NormalFormKind::Power(_, _)
            | NormalFormKind::Modulo(_, _)
            | NormalFormKind::Negate(_)
            | NormalFormKind::Reciprocal(_)
            | NormalFormKind::Comparison(_, _, _)
            | NormalFormKind::Logical(_, _)
            | NormalFormKind::Not(_)
            | NormalFormKind::MathOp(_, _)
            | NormalFormKind::StepRounding(_, _, _)
            | NormalFormKind::UnitConversion(_, _)
            | NormalFormKind::Veto(_)
            | NormalFormKind::DateRelative(_, _)
            | NormalFormKind::DateCalendar(_, _, _)
            | NormalFormKind::RangeLiteral(_, _)
            | NormalFormKind::PastFutureRange(_, _)
            | NormalFormKind::RangeContainment(_, _)
            | NormalFormKind::RangeBound(_, _)
            | NormalFormKind::ResultIsVeto(_)
            | NormalFormKind::Now
            | NormalFormKind::SpecMember { .. }
            | NormalFormKind::ListCommand { .. } => return Some(body),
        };
        let literal = match values.get(scrutinee.index()).and_then(|slot| slot.as_ref()) {
            Some(OperationResult::Value(literal)) => literal,
            Some(OperationResult::Veto(_)) | None => return None,
        };
        let index = region_of_scrutinee(boundaries, &literal.value).ok()?;
        region = regions[index];
    }
    panic!("BUG: nested OrderedDispatch chain longer than the normal-form table");
}

/// Origin arm body a conjunctive region's residual arms select, scanned like the origin
/// Piecewise. `None` while a residual condition is unset or vetoed. A group that
/// normalization rewrote (into a dispatch or a collapsed value) is itself the body.
fn conjunctive_winner_body(
    plan: &ExecutionPlan,
    values: &[Option<OperationResult>],
    group: NormalFormId,
) -> Option<NormalFormId> {
    use crate::evaluation::branch_semantics::{
        condition_outcome, piecewise_decision, PiecewiseDecision,
    };
    use crate::planning::normalize::NormalFormKind;

    let NormalFormKind::Piecewise(arms) = &plan.normal_form(group).kind else {
        return Some(group);
    };
    let outcome = |arm: usize| {
        values
            .get(arms[arm].0.index())
            .and_then(|slot| slot.as_ref())
            .map(condition_outcome)
    };
    match piecewise_decision(arms.len(), outcome) {
        PiecewiseDecision::Taken { arm } => Some(arms[arm].1),
        PiecewiseDecision::Default => Some(arms[0].1),
        PiecewiseDecision::Undecided { .. } => None,
    }
}

/// DataPath leaves reachable from `root` in evaluator decision-tree preorder.
///
/// Control liveness is derived from `values` (the evaluation value table), matching
/// the evaluator's own decisions:
/// - [`NormalFormKind::Logical`]: an absorbing operand decides the connective and is
///   the only live operand. Otherwise every operand not settled at the identity is live.
/// - [`NormalFormKind::Piecewise`]: arms decided from filled condition slots, high to
///   low. An unfilled or vetoed condition before a winner is found means no decision:
///   every arm stays live.
/// - [`NormalFormKind::OrderedDispatch`]: only the region for the scrutinee slot. An
///   unfilled scrutinee means every origin arm stays live.
///
/// Every cell the evaluator visits owns its slot, including rule reference cells
/// (they hold the referenced rule's value).
///
/// Order matches the evaluator walk (`tree.rs`): first-seen [`DataPath`] wins
/// (rule references may share a leaf under distinct cell ids). Iterative DFS with
/// pop-time visited; children pushed in reverse evaluation order. Instance-list
/// commands and row member reads appear as [`ReachableInput::RowReads`]: their
/// missing row fields are recorded during evaluation, not in the plan.
pub(crate) fn reachable_data_paths(
    plan: &ExecutionPlan,
    root: NormalFormId,
    values: &[Option<OperationResult>],
) -> Vec<ReachableInput> {
    use crate::computation::OperationResult;
    use crate::evaluation::branch_semantics::{
        condition_outcome, piecewise_decision, BranchOutcome, PiecewiseDecision,
    };
    use crate::planning::normalize::LeafKind;
    use crate::planning::normalize::NormalFormKind;
    use crate::planning::ordered_dispatch::region_of_scrutinee;

    fn slot_outcome(values: &[Option<OperationResult>], id: NormalFormId) -> Option<BranchOutcome> {
        values
            .get(id.index())
            .and_then(|slot| slot.as_ref())
            .map(condition_outcome)
    }

    /// Piecewise pre-image of a dispatch cell.
    ///
    /// A cell that borrows a rule's Kind (`rule_ref`) records no `origin` of its
    /// own, and the rule it names can itself be a bare reference to a further
    /// rule (`rule alias: country`), so the fold can sit any number of
    /// references away. Every hop moves to a rule earlier in topological order,
    /// so the chain is shorter than the rule table.
    fn dispatch_origin(plan: &ExecutionPlan, cell: &NormalForm) -> NormalFormId {
        let mut current = cell;
        for _ in 0..=plan.rules.len() {
            if let Some(origin) = current.origin {
                return origin;
            }
            let ref_path = current.rule_ref.as_ref().unwrap_or_else(|| {
                panic!("BUG: OrderedDispatch without origin must carry rule_ref")
            });
            let body = plan
                .rules
                .get(ref_path)
                .unwrap_or_else(|| {
                    panic!("BUG: rule reference '{ref_path}' missing from plan.rules")
                })
                .normal_form;
            current = plan.normal_form(body);
        }
        panic!("BUG: rule reference chain over a dispatch cell does not terminate");
    }

    /// Piecewise children the evaluator visits under `decision`, in evaluation
    /// order: `cond_n, body_n, …, cond_1, body_1, default`.
    fn live_piecewise_children(
        arms: &[(NormalFormId, NormalFormId)],
        decision: PiecewiseDecision,
    ) -> Vec<NormalFormId> {
        let mut live = Vec::new();
        match decision {
            PiecewiseDecision::Taken { arm } => {
                for (condition, _) in arms.iter().skip(arm + 1).rev() {
                    live.push(*condition);
                }
                live.push(arms[arm].0);
                live.push(arms[arm].1);
            }
            PiecewiseDecision::Default => {
                for (condition, _) in arms.iter().skip(1).rev() {
                    live.push(*condition);
                }
                live.push(arms[0].1);
            }
            PiecewiseDecision::Undecided { arm } => {
                for (condition, _) in arms.iter().skip(arm + 1).rev() {
                    live.push(*condition);
                }
                for (condition, body) in arms.iter().take(arm + 1).skip(1).rev() {
                    live.push(*condition);
                    live.push(*body);
                }
                live.push(arms[0].1);
            }
        }
        live
    }

    let mut out = Vec::new();
    let mut seen_paths = HashSet::new();
    let mut visited = HashSet::new();
    let mut stack = vec![root];

    while let Some(id) = stack.pop() {
        if !visited.insert(id) {
            continue;
        }
        let nf = plan.normal_form(id);
        let live: Vec<NormalFormId> = match &nf.kind {
            // A slot bound to a rule has that rule's inputs.
            NormalFormKind::Leaf(LeafKind::DataPath(path)) => match plan.reference_ends.get(path) {
                Some(ReferenceEnd::Rule(rule)) => vec![
                    plan.rules
                        .get(rule)
                        .unwrap_or_else(|| {
                            panic!("BUG: rule-bound slot '{path}' names missing rule '{rule}'")
                        })
                        .normal_form,
                ],
                Some(ReferenceEnd::Promptable(_)) | None => {
                    if seen_paths.insert(path.clone()) {
                        out.push(ReachableInput::Data(path.clone()));
                    }
                    Vec::new()
                }
            },
            NormalFormKind::RowField { .. } => Vec::new(),
            NormalFormKind::Leaf(LeafKind::RowValue { .. }) => Vec::new(),
            NormalFormKind::ListCommand { row_level, .. } => {
                if row_level.is_some() {
                    out.push(ReachableInput::RowReads(id));
                }
                nf.kind.children()
            }
            NormalFormKind::Logical(connective, children) => {
                let absorbing = connective.absorbing_operand();
                match children.iter().copied().find(|&child| {
                    matches!(
                        (absorbing, slot_outcome(values, child)),
                        (true, Some(BranchOutcome::Taken)) | (false, Some(BranchOutcome::NotTaken))
                    )
                }) {
                    Some(decider) => vec![decider],
                    None => children
                        .iter()
                        .copied()
                        .filter(|&child| {
                            !matches!(
                                (absorbing, slot_outcome(values, child)),
                                (true, Some(BranchOutcome::NotTaken))
                                    | (false, Some(BranchOutcome::Taken))
                            )
                        })
                        .collect(),
                }
            }
            NormalFormKind::Piecewise(arms) => {
                let decision =
                    piecewise_decision(arms.len(), |arm| slot_outcome(values, arms[arm].0));
                live_piecewise_children(arms, decision)
            }
            NormalFormKind::OrderedDispatch {
                scrutinee,
                boundaries,
                regions,
            } => {
                let origin_id = dispatch_origin(plan, nf);
                let NormalFormKind::Piecewise(arms) = &plan.normal_form(origin_id).kind else {
                    panic!("BUG: OrderedDispatch origin must be Piecewise");
                };
                visited.insert(origin_id);

                match values.get(scrutinee.index()).and_then(|slot| slot.as_ref()) {
                    Some(OperationResult::Value(literal)) => {
                        match region_of_scrutinee(boundaries, &literal.value) {
                            Ok(region) => vec![*scrutinee, regions[region].cell()],
                            Err(_) => vec![*scrutinee],
                        }
                    }
                    // Definitive veto settles the rule; gated bodies cannot un-veto.
                    Some(result) if !result.is_missing_data() => vec![*scrutinee],
                    // Open scrutinee (unbound DataPath leave-None, or expression
                    // MissingData stored on the cell): keep every origin arm live.
                    None | Some(_) => {
                        let mut live = vec![*scrutinee];
                        live.extend(live_piecewise_children(
                            arms,
                            PiecewiseDecision::Undecided {
                                arm: arms.len() - 1,
                            },
                        ));
                        live
                    }
                }
            }
            NormalFormKind::SpecMember { base, .. } => {
                out.push(ReachableInput::RowReads(id));
                vec![*base]
            }
            NormalFormKind::Leaf(LeafKind::Literal(_))
            | NormalFormKind::Now
            | NormalFormKind::Veto(_)
            | NormalFormKind::Sum(_)
            | NormalFormKind::Product(_)
            | NormalFormKind::Subtract(_, _)
            | NormalFormKind::Divide(_, _)
            | NormalFormKind::Power(_, _)
            | NormalFormKind::Modulo(_, _)
            | NormalFormKind::Comparison(_, _, _)
            | NormalFormKind::RangeLiteral(_, _)
            | NormalFormKind::RangeContainment(_, _)
            | NormalFormKind::StepRounding(_, _, _)
            | NormalFormKind::Negate(_)
            | NormalFormKind::Reciprocal(_)
            | NormalFormKind::Not(_)
            | NormalFormKind::MathOp(_, _)
            | NormalFormKind::UnitConversion(_, _)
            | NormalFormKind::DateRelative(_, _)
            | NormalFormKind::DateCalendar(_, _, _)
            | NormalFormKind::PastFutureRange(_, _)
            | NormalFormKind::RangeBound(_, _)
            | NormalFormKind::ResultIsVeto(_) => nf.kind.children(),
        };
        stack.extend(live.into_iter().rev());
    }
    out
}

/// One input [`reachable_data_paths`] found, in evaluation order.
pub(crate) enum ReachableInput {
    Data(DataPath),
    /// A cell that reads list rows; its missing row fields are in the request state.
    RowReads(NormalFormId),
}

pub(crate) fn validate_value_against_type(
    expected_type: &LemmaType,
    value: &LiteralValue,
    unit_index: &UnitIndex,
    runtime_scales: Option<
        &std::collections::HashMap<String, crate::computation::rational::RationalInteger>,
    >,
) -> Result<(), String> {
    use std::borrow::Cow;

    let mut pending = vec![(expected_type, Cow::Borrowed(value))];
    while let Some((expected, value)) = pending.pop() {
        match (&expected.specifications, &value.value) {
            (TypeSpecification::List { element, .. }, ValueKind::List(rows)) => {
                for row in rows.iter().rev() {
                    pending.push((element.as_ref(), Cow::Owned(row.to_literal())));
                }
            }
            (TypeSpecification::Measure { .. }, ValueKind::UnscaledMeasure { .. }) => {}
            (TypeSpecification::MeasureRange { .. }, ValueKind::Range(left, right))
                if matches!(
                    (&left.value, &right.value),
                    (
                        ValueKind::UnscaledMeasure { .. },
                        ValueKind::UnscaledMeasure { .. }
                    ) | (ValueKind::UnscaledMeasure { .. }, ValueKind::Measure(_))
                        | (ValueKind::Measure(_), ValueKind::UnscaledMeasure { .. })
                ) => {}
            (
                range_spec @ (TypeSpecification::NumberRange { .. }
                | TypeSpecification::DateRange { .. }
                | TypeSpecification::TimeRange { .. }
                | TypeSpecification::MeasureRange { .. }
                | TypeSpecification::RatioRange { .. }),
                ValueKind::Range(left, right),
            ) => validate_range_literal(
                expected,
                range_spec,
                left.as_ref(),
                right.as_ref(),
                unit_index,
            )?,
            _ => {
                validate_scalar_against_type(expected, &value, runtime_scales)?;
                expected.reject_disallowed(&value.value, runtime_scales)?;
            }
        }
    }
    Ok(())
}

fn validate_scalar_against_type(
    expected_type: &LemmaType,
    value: &LiteralValue,
    runtime_scales: Option<
        &std::collections::HashMap<String, crate::computation::rational::RationalInteger>,
    >,
) -> Result<(), String> {
    use crate::computation::bigint::BigInt;
    use crate::computation::rational::{
        checked_mul, rational_new, try_pow_integer, RationalInteger,
    };

    fn exceeds_decimal_places(magnitude: &RationalInteger, max_decimals: u8) -> bool {
        let scale = match try_pow_integer(
            &rational_new(10, 1),
            &BigInt::from_i64(i64::from(max_decimals)),
        ) {
            Ok(value) => value,
            Err(_) => return true,
        };
        let scaled = match checked_mul(magnitude, &scale) {
            Ok(value) => value,
            Err(_) => return true,
        };
        match RationalInteger::try_reduce_ref(&scaled) {
            Ok(reduced) => !reduced.is_integer(),
            Err(_) => true,
        }
    }

    fn format_rational_for_validation_message(
        expected_type: &LemmaType,
        magnitude: &RationalInteger,
    ) -> String {
        expected_type
            .try_rational_as_decimal(magnitude)
            .map(|decimal| decimal.to_string())
            .unwrap_or_else(|_| magnitude.display_str())
    }

    match (&expected_type.specifications, &value.value) {
        (
            TypeSpecification::Number {
                minimum,
                maximum,
                decimals,
                ..
            },
            ValueKind::Number(n),
        ) => {
            use std::cmp::Ordering;
            if let Some(d) = decimals {
                if exceeds_decimal_places(n, *d) {
                    return Err(format!(
                        "{} exceeds the decimals constraint of {d}.",
                        n.display_str()
                    ));
                }
            }
            if let Some(min) = minimum {
                if n.try_cmp(min).map_err(|failure| {
                    format!("Comparing against the minimum bound failed: {failure}.")
                })? == Ordering::Less
                {
                    return Err(format!(
                        "{} is below the minimum of {}.",
                        format_rational_for_validation_message(expected_type, n),
                        format_rational_for_validation_message(expected_type, min)
                    ));
                }
            }
            if let Some(max) = maximum {
                if n.try_cmp(max).map_err(|failure| {
                    format!("Comparing against the maximum bound failed: {failure}.")
                })? == Ordering::Greater
                {
                    return Err(format!(
                        "{} is above the maximum of {}.",
                        format_rational_for_validation_message(expected_type, n),
                        format_rational_for_validation_message(expected_type, max)
                    ));
                }
            }
            Ok(())
        }
        (
            TypeSpecification::Measure {
                minimum,
                maximum,
                units,
                ..
            },
            ValueKind::Measure(magnitude),
        ) => {
            use crate::computation::rational::{checked_div, checked_mul};
            use std::cmp::Ordering;
            for (bound, label, violation, relation) in [
                (minimum, "minimum", Ordering::Less, "below"),
                (maximum, "maximum", Ordering::Greater, "above"),
            ] {
                let Some((bound_magnitude, bound_unit)) = bound else {
                    continue;
                };
                let bound_unit_def = units.get(bound_unit.as_str())?;
                let canonical_bound = if let Some(factor) = bound_unit_def.literal_factor() {
                    checked_mul(bound_magnitude, factor).map_err(|failure| {
                        format!("{label}: unit conversion overflow: {failure}")
                    })?
                } else {
                    let factor = runtime_scales
                        .and_then(|scales| scales.get(bound_unit.as_str()))
                        .ok_or_else(|| {
                            format!(
                                "{label} cannot use unit '{bound_unit}' on '{}': its conversion factor is not settled.",
                                expected_type.name()
                            )
                        })?;
                    checked_mul(bound_magnitude, factor).map_err(|failure| {
                        format!("{label}: unit conversion overflow: {failure}")
                    })?
                };
                if magnitude.try_cmp(&canonical_bound).map_err(|failure| {
                    format!("Comparing against the {label} bound failed: {failure}.")
                })? != violation
                {
                    continue;
                }
                let display_unit = match expected_type.unit() {
                    Some(written) => units.get(written)?,
                    None => units.get(bound_unit)?,
                };
                let factor = if let Some(factor) = display_unit.literal_factor() {
                    factor.clone()
                } else {
                    runtime_scales
                        .and_then(|scales| scales.get(display_unit.name.as_str()))
                        .cloned()
                        .ok_or_else(|| {
                            format!(
                                "Cannot de-canonicalize the measure for validation: unit '{}' scale is not settled.",
                                display_unit.name
                            )
                        })?
                };
                let value_in_unit = checked_div(magnitude, &factor).map_err(|failure| {
                    format!("Cannot de-canonicalize the measure for validation: {failure}.")
                })?;
                let bound_in_unit = checked_div(&canonical_bound, &factor).map_err(|failure| {
                    format!("Cannot de-canonicalize the {label} for validation: {failure}.")
                })?;
                return Err(format!(
                    "{} {unit} is {relation} the {label} of {} {unit}.",
                    format_rational_for_validation_message(expected_type, &value_in_unit),
                    format_rational_for_validation_message(expected_type, &bound_in_unit),
                    unit = display_unit.name
                ));
            }
            Ok(())
        }
        (TypeSpecification::Text { length, .. }, ValueKind::Text(s)) => {
            let len = s.chars().count();
            if let Some(exact) = length {
                if len != *exact {
                    return Err(format!(
                        "'{}' has length {}, but the required length is {}.",
                        s, len, exact
                    ));
                }
            }
            Ok(())
        }
        (
            TypeSpecification::Ratio {
                minimum,
                maximum,
                units,
                ..
            },
            ValueKind::Ratio(r),
        ) => {
            use crate::computation::rational::checked_mul;
            use std::cmp::Ordering;

            let written_unit = expected_type.unit();

            if let Some(type_minimum) = minimum {
                if r.try_cmp(type_minimum).map_err(|failure| {
                    format!("Comparing against the minimum bound failed: {failure}.")
                })? == Ordering::Less
                {
                    let message = match written_unit {
                        Some(unit) => {
                            let ratio_unit = units.get(unit)?;
                            let value_per_unit = checked_mul(r, &ratio_unit.value)
                                .map_err(|failure| failure.to_string())?;
                            let bound_per_unit = ratio_unit.minimum.clone().expect(
                                "BUG: RatioUnit.minimum missing after type minimum set by sync_ratio_units_from_canonical",
                            );
                            format!(
                                "{} {unit} is below the minimum of {} {unit}.",
                                format_rational_for_validation_message(
                                    expected_type,
                                    &value_per_unit
                                ),
                                format_rational_for_validation_message(
                                    expected_type,
                                    &bound_per_unit.clone()
                                ),
                            )
                        }
                        None => format!(
                            "{} is below the minimum of {}.",
                            format_rational_for_validation_message(expected_type, r),
                            format_rational_for_validation_message(expected_type, type_minimum),
                        ),
                    };
                    return Err(message);
                }
            }
            if let Some(type_maximum) = maximum {
                if r.try_cmp(type_maximum).map_err(|failure| {
                    format!("Comparing against the maximum bound failed: {failure}.")
                })? == Ordering::Greater
                {
                    let message = match written_unit {
                        Some(unit) => {
                            let ratio_unit = units.get(unit)?;
                            let value_per_unit = checked_mul(r, &ratio_unit.value)
                                .map_err(|failure| failure.to_string())?;
                            let bound_per_unit = ratio_unit.maximum.clone().expect(
                                "BUG: RatioUnit.maximum missing after type maximum set by sync_ratio_units_from_canonical",
                            );
                            format!(
                                "{} {unit} is above the maximum of {} {unit}.",
                                format_rational_for_validation_message(
                                    expected_type,
                                    &value_per_unit
                                ),
                                format_rational_for_validation_message(
                                    expected_type,
                                    &bound_per_unit.clone()
                                ),
                            )
                        }
                        None => format!(
                            "{} is above the maximum of {}.",
                            format_rational_for_validation_message(expected_type, r),
                            format_rational_for_validation_message(expected_type, type_maximum),
                        ),
                    };
                    return Err(message);
                }
            }
            Ok(())
        }
        (
            TypeSpecification::Date {
                minimum, maximum, ..
            },
            ValueKind::Date(dt),
        ) => {
            use crate::planning::semantics::{compare_semantic_dates, date_time_to_semantic};
            use std::cmp::Ordering;
            if let Some(min) = minimum {
                let min_sem = date_time_to_semantic(min);
                if compare_semantic_dates(dt, &min_sem) == Ordering::Less {
                    return Err(format!("{} is below the minimum of {}.", dt, min));
                }
            }
            if let Some(max) = maximum {
                let max_sem = date_time_to_semantic(max);
                if compare_semantic_dates(dt, &max_sem) == Ordering::Greater {
                    return Err(format!("{} is above the maximum of {}.", dt, max));
                }
            }
            Ok(())
        }
        (
            TypeSpecification::Time {
                minimum, maximum, ..
            },
            ValueKind::Time(t),
        ) => {
            use crate::planning::semantics::{compare_semantic_times, time_to_semantic};
            use std::cmp::Ordering;
            if let Some(min) = minimum {
                let min_sem = time_to_semantic(min);
                if compare_semantic_times(t, &min_sem) == Ordering::Less {
                    return Err(format!("{} is below the minimum of {}.", t, min));
                }
            }
            if let Some(max) = maximum {
                let max_sem = time_to_semantic(max);
                if compare_semantic_times(t, &max_sem) == Ordering::Greater {
                    return Err(format!("{} is above the maximum of {}.", t, max));
                }
            }
            Ok(())
        }
        (TypeSpecification::Boolean { .. }, ValueKind::Boolean(_)) => Ok(()),
        (TypeSpecification::List { .. }, ValueKind::List(_))
        | (
            TypeSpecification::NumberRange { .. }
            | TypeSpecification::DateRange { .. }
            | TypeSpecification::TimeRange { .. }
            | TypeSpecification::MeasureRange { .. }
            | TypeSpecification::RatioRange { .. },
            ValueKind::Range(_, _),
        ) => unreachable!("BUG: list and range values are walked by validate_value_against_type"),
        (TypeSpecification::Veto { .. }, _) | (TypeSpecification::Undetermined, _) => Ok(()),
        (TypeSpecification::Spec { .. }, ValueKind::Spec(_)) => Ok(()),
        (spec, value_kind) if !value_kind_matches_spec(value_kind, spec) => unreachable!(
            "BUG: validate_value_against_type called with mismatched type/value: \
             spec={:?}, value={:?} — typing must be enforced before validation",
            spec, value_kind
        ),
        (spec, value_kind) => unreachable!(
            "BUG: validate_value_against_type missed a value_kind_matches_spec pair: \
             spec={:?}, value={:?}",
            spec, value_kind
        ),
    }
}

fn validate_range_literal(
    expected_type: &LemmaType,
    range_spec: &TypeSpecification,
    left: &TypedLiteral,
    right: &TypedLiteral,
    unit_index: &UnitIndex,
) -> Result<(), String> {
    use crate::computation::{comparison_operation, OperationResult};
    use crate::planning::semantics::{
        compare_semantic_dates, compare_semantic_times, measure_declared_bound_to_canonical,
        ValueKind,
    };
    use std::cmp::Ordering;
    use std::sync::Arc;

    let element_spec = range_spec
        .element_from_range()
        .expect("BUG: element_from_range missing arm for validated range");
    if let TypeSpecification::Measure {
        units,
        decomposition,
        ..
    } = &element_spec
    {
        assert!(
            decomposition.is_some() || units.is_empty(),
            "BUG: measure range '{}' with units must carry a decomposition after planning",
            expected_type.name()
        );
    }
    let element_type = Arc::new(LemmaType::primitive(element_spec));
    validate_scalar_against_type(element_type.as_ref(), &left.to_literal(), None)?;
    validate_scalar_against_type(element_type.as_ref(), &right.to_literal(), None)?;

    let ordering = match (&left.value, &right.value) {
        (ValueKind::Number(l), ValueKind::Number(r)) => l
            .try_cmp(r)
            .map_err(|failure| format!("Comparing range endpoints failed: {failure}."))?,
        (ValueKind::Date(l), ValueKind::Date(r)) => compare_semantic_dates(l, r),
        (ValueKind::Time(l), ValueKind::Time(r)) => compare_semantic_times(l, r),
        (ValueKind::Ratio(l), ValueKind::Ratio(r)) => l
            .try_cmp(r)
            .map_err(|failure| format!("Comparing range endpoints failed: {failure}."))?,
        (ValueKind::Measure(l), ValueKind::Measure(r)) => l
            .try_cmp(r)
            .map_err(|failure| format!("Comparing range endpoints failed: {failure}."))?,
        (left_kind, right_kind) => unreachable!(
            "BUG: range endpoints have mismatched value kinds after typing: {left_kind:?} vs {right_kind:?}"
        ),
    };
    if ordering == Ordering::Greater {
        return Err(format!(
            "The range left endpoint {} is above the right endpoint {}.",
            left.display_value(),
            right.display_value()
        ));
    }

    let range_lit = LiteralValue {
        value: ValueKind::Range(Box::new(left.clone()), Box::new(right.clone())),
    };
    let range_type = Arc::new(expected_type.clone());

    let compare_width = |bound: &LiteralValue,
                         bound_type: &Arc<LemmaType>,
                         op: ComparisonComputation,
                         fail_msg: String|
     -> Result<(), String> {
        match comparison_operation(&range_lit.value, &range_type, &op, &bound.value, bound_type) {
            OperationResult::Value(result) => match &result.value {
                ValueKind::Boolean(true) => Ok(()),
                ValueKind::Boolean(false) => Err(fail_msg),
                other => unreachable!("BUG: width comparison must return boolean, got {other:?}"),
            },
            OperationResult::Veto(veto) => Err(veto.to_string()),
        }
    };

    match range_spec {
        TypeSpecification::NumberRange {
            minimum, maximum, ..
        } => {
            if let Some(min_w) = minimum {
                compare_width(
                    &LiteralValue::number(min_w.clone()),
                    primitive_number_arc(),
                    ComparisonComputation::GreaterThanOrEqual,
                    format!(
                        "The span is below the minimum width of {}.",
                        min_w.display_str()
                    ),
                )?;
            }
            if let Some(max_w) = maximum {
                compare_width(
                    &LiteralValue::number(max_w.clone()),
                    primitive_number_arc(),
                    ComparisonComputation::LessThanOrEqual,
                    format!(
                        "The span is above the maximum width of {}.",
                        max_w.display_str()
                    ),
                )?;
            }
        }
        TypeSpecification::RatioRange {
            minimum, maximum, ..
        } => {
            if let Some(min_w) = minimum {
                compare_width(
                    &LiteralValue::ratio(min_w.clone()),
                    primitive_ratio_arc(),
                    ComparisonComputation::GreaterThanOrEqual,
                    format!(
                        "The span is below the minimum width of {}.",
                        min_w.display_str()
                    ),
                )?;
            }
            if let Some(max_w) = maximum {
                compare_width(
                    &LiteralValue::ratio(max_w.clone()),
                    primitive_ratio_arc(),
                    ComparisonComputation::LessThanOrEqual,
                    format!(
                        "The span is above the maximum width of {}.",
                        max_w.display_str()
                    ),
                )?;
            }
        }
        TypeSpecification::MeasureRange {
            minimum,
            maximum,
            units,
            ..
        } => {
            if let Some(min_w) = minimum {
                let canonical = measure_declared_bound_to_canonical(
                    &min_w.0, &min_w.1, units, "range", "minimum",
                )?;
                let bound = LiteralValue::measure(canonical);
                compare_width(
                    &bound,
                    &element_type,
                    ComparisonComputation::GreaterThanOrEqual,
                    format!(
                        "The span is below the minimum width of {} {}.",
                        min_w.0.display_str(),
                        min_w.1
                    ),
                )?;
            }
            if let Some(max_w) = maximum {
                let canonical = measure_declared_bound_to_canonical(
                    &max_w.0, &max_w.1, units, "range", "maximum",
                )?;
                let bound = LiteralValue::measure(canonical);
                compare_width(
                    &bound,
                    &element_type,
                    ComparisonComputation::LessThanOrEqual,
                    format!(
                        "The span is above the maximum width of {} {}.",
                        max_w.0.display_str(),
                        max_w.1
                    ),
                )?;
            }
        }
        TypeSpecification::DateRange {
            minimum, maximum, ..
        }
        | TypeSpecification::TimeRange {
            minimum, maximum, ..
        } => {
            let allow_calendar = matches!(range_spec, TypeSpecification::DateRange { .. });
            let resolve_bound = |bound: &(RationalInteger, String),
                                 command: &str|
             -> Result<(LiteralValue, Arc<LemmaType>), String> {
                let (bare, owner) = unit_index
                    .resolve(bound.1.as_str())
                    .map_err(|err| format!("{command} width unit '{}': {err}", bound.1))?;
                if allow_calendar {
                    if !owner.is_duration_like() && !owner.is_calendar_like() {
                        return Err(format!(
                                "The {command} width unit '{bare}' must be a duration or calendar unit.",
                            ));
                    }
                } else if !owner.is_duration_like() {
                    return Err(format!(
                        "The {command} width unit '{bare}' must be a duration unit.",
                    ));
                }
                let TypeSpecification::Measure { units, .. } = &owner.specifications else {
                    return Err(format!(
                        "The {command} width unit '{bare}' must resolve to a measure type.",
                    ));
                };
                let type_name = owner.name();
                let canonical = measure_declared_bound_to_canonical(
                    &bound.0,
                    &bare,
                    units,
                    type_name.as_str(),
                    command,
                )?;
                Ok((LiteralValue::measure(canonical), Arc::clone(&owner)))
            };
            if let Some(min_w) = minimum {
                let (bound, bound_type) = resolve_bound(min_w, "minimum")?;
                compare_width(
                    &bound,
                    &bound_type,
                    ComparisonComputation::GreaterThanOrEqual,
                    format!(
                        "The span is below the minimum width of {} {}.",
                        min_w.0.display_str(),
                        min_w.1
                    ),
                )?;
            }
            if let Some(max_w) = maximum {
                let (bound, bound_type) = resolve_bound(max_w, "maximum")?;
                compare_width(
                    &bound,
                    &bound_type,
                    ComparisonComputation::LessThanOrEqual,
                    format!(
                        "The span is above the maximum width of {} {}.",
                        max_w.0.display_str(),
                        max_w.1
                    ),
                )?;
            }
        }
        _ => {}
    }

    Ok(())
}

/// Literal plan data against its declared type. Written row values are validated
/// when their row is built (`source_row_literal` in the graph builder).
fn validate_literal_data_against_types(plan: &ExecutionPlan) -> Vec<Error> {
    let mut errors = Vec::new();

    for (data_path, data_definition) in &plan.data {
        let (expected_type, lit) = match data_definition {
            DataDefinition::Value {
                value,
                resolved_type,
                ..
            } => (resolved_type, value),
            DataDefinition::Import { .. }
            | DataDefinition::TypeDeclaration { .. }
            | DataDefinition::InstanceList { .. }
            | DataDefinition::Reference { .. } => continue,
        };

        if let Err(msg) =
            validate_value_against_type(expected_type, lit, plan.expression_unit_index(), None)
        {
            let source = data_definition.source().clone();
            errors.push(Error::validation(
                format!(
                    "Invalid value for data {} (expected {}): {}",
                    data_path,
                    expected_type.name().as_str(),
                    msg
                ),
                Some(source),
                None::<String>,
            ));
        }
    }

    errors
}

fn validate_unit_conversion_targets(plan: &ExecutionPlan) -> Result<(), Error> {
    use crate::planning::normalize::NormalFormKind;

    let mut errors: Vec<Error> = Vec::new();
    let mut visited = HashSet::new();
    let mut worklist: Vec<NormalFormId> =
        plan.rules.values().map(|rule| rule.normal_form).collect();
    while let Some(id) = worklist.pop() {
        if !visited.insert(id) {
            continue;
        }
        let nf = plan.normal_form(id);
        nf.kind.for_each_child(|child| worklist.push(child));
        if let NormalFormKind::UnitConversion(_, target) = &nf.kind {
            if let Some((unit_name, owning_type)) = conversion_target_declares_unit(target) {
                if !owning_type_declares_unit_name(owning_type.as_ref(), unit_name) {
                    errors.push(Error::validation(
                        format!(
                            "Unit conversion target '{unit_name}' is not declared on owning type '{}'",
                            owning_type.name()
                        ),
                        None::<Source>,
                        Some(plan.spec_name.clone()),
                    ));
                }
            }
        }
    }
    if let Some(error) = errors.into_iter().next() {
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computation::rational::rational_new;
    use crate::computation::{OperationResult, VetoType};
    use crate::evaluation::evaluate_data::EvaluateData;
    use crate::literals::DateGranularity;
    use crate::literals::TimezoneValue;
    use crate::parsing::ast::DateTimeValue;
    use crate::planning::semantics::{DataDefinition, DataPath, TypeSpecification};
    use crate::Engine;
    use crate::{EvaluateDataValue, ResourceLimits};
    use serde_json;
    use std::sync::Arc;

    fn default_limits() -> ResourceLimits {
        ResourceLimits::default()
    }

    fn resolve_evaluate_data(
        plan: &ExecutionPlan,
        values: IndexMap<String, EvaluateDataValue>,
    ) -> EvaluateData {
        EvaluateData::resolve(plan, values, &default_limits()).expect("resolve")
    }

    fn binding<'a>(
        plan: &ExecutionPlan,
        evaluate_data: &'a EvaluateData,
        path: &DataPath,
    ) -> Option<&'a OperationResult> {
        let slot = plan
            .data
            .get_index_of(path)
            .expect("BUG: test path is plan data");
        evaluate_data.bindings.get(&slot)
    }

    fn veto_reason<'a>(
        plan: &ExecutionPlan,
        evaluate_data: &'a EvaluateData,
        path: &DataPath,
    ) -> Option<&'a str> {
        match binding(plan, evaluate_data, path) {
            Some(OperationResult::Veto(veto)) => Some(match veto {
                VetoType::Computation { message } => message.as_str(),
                other => panic!("expected Computation veto, got {other:?}"),
            }),
            _ => None,
        }
    }

    fn bound_value<'a>(
        plan: &ExecutionPlan,
        evaluate_data: &'a EvaluateData,
        path: &DataPath,
    ) -> Option<&'a crate::planning::semantics::BoundValueKind> {
        binding(plan, evaluate_data, path).and_then(OperationResult::value)
    }

    fn input_data(pairs: &[(&str, &str)]) -> IndexMap<String, EvaluateDataValue> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), EvaluateDataValue::string(*v)))
            .collect()
    }

    #[test]
    fn test_with_raw_values() {
        let mut engine = Engine::new();
        engine
            .load([(
                crate::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                    "test.lemma",
                ))),
                r#"
                spec test
                data age: number -> suggest 25
                "#
                .to_string(),
            )])
            .unwrap();

        let plans = engine
            .plans
            .get_plans(None, "test")
            .expect("plans for test");
        let plan = plans.values().next().expect("plan");
        let data_path = DataPath::new(vec![], "age".to_string());

        let values = input_data(&[("age", "30")]);

        let evaluate_data = resolve_evaluate_data(plan, values);
        let updated_value = bound_value(plan, &evaluate_data, &data_path).expect("bound value");
        match &updated_value.value {
            crate::planning::semantics::ValueKind::Number(n) => {
                assert_eq!(n, &rational_new(30, 1));
            }
            other => panic!("Expected number literal, got {:?}", other),
        }
    }

    #[test]
    fn test_with_raw_values_type_mismatch() {
        let mut engine = Engine::new();
        engine
            .load([(
                crate::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                    "test.lemma",
                ))),
                r#"
                spec test
                data age: number
                "#
                .to_string(),
            )])
            .unwrap();

        let plans = engine
            .plans
            .get_plans(None, "test")
            .expect("plans for test");
        let plan = plans.values().next().expect("plan");

        let values = input_data(&[("age", "thirty")]);

        let evaluate_data = resolve_evaluate_data(plan, values);
        let data_path = DataPath::new(vec![], "age".to_string());
        match veto_reason(plan, &evaluate_data, &data_path) {
            Some(reason) => {
                assert!(
                    reason.contains("number"),
                    "type mismatch must record violation reason, got: {reason}"
                );
            }
            None => panic!("expected veto-bound data for age=thirty"),
        }
    }

    #[test]
    fn test_with_raw_values_unknown_data_ignored() {
        let mut engine = Engine::new();
        engine
            .load([(
                crate::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                    "test.lemma",
                ))),
                r#"
                spec test
                data known: number
                "#
                .to_string(),
            )])
            .unwrap();

        let plans = engine
            .plans
            .get_plans(None, "test")
            .expect("plans for test");
        let plan = plans.values().next().expect("plan");

        let values = input_data(&[("unknown", "30")]);

        let evaluate_data = resolve_evaluate_data(plan, values);
        assert!(evaluate_data.bindings.is_empty());
        assert!(evaluate_data.ignored_unknown.iter().any(|k| k == "unknown"));
    }

    #[test]
    fn test_with_raw_values_nested() {
        let mut engine = Engine::new();
        engine
            .load([(
                crate::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                    "test.lemma",
                ))),
                r#"
                spec private
                data base_price: number

                spec test
                uses rules: private
                "#
                .to_string(),
            )])
            .unwrap();

        let plans = engine
            .plans
            .get_plans(None, "test")
            .expect("plans for test");
        let plan = plans.values().next().expect("plan");

        let values = input_data(&[("rules.base_price", "100")]);

        let evaluate_data = resolve_evaluate_data(plan, values);
        let Some(DataDefinition::Import { row, .. }) =
            plan.data.get(&DataPath::local("rules".into()))
        else {
            panic!("`rules` is the import slot of the plan");
        };
        let instance = crate::planning::semantics::InstanceKey::new(Arc::clone(row), None);
        let slot = plan
            .row_template(&row.identity)
            .data
            .get_index_of(&DataPath::local("base_price".into()))
            .expect("`base_price` is data of the imported template");
        let updated_value = evaluate_data.row_overrides.fields[&instance][&slot]
            .value()
            .expect("bound value");
        match &updated_value.value {
            crate::planning::semantics::ValueKind::Number(n) => {
                assert_eq!(n, &rational_new(100, 1));
            }
            other => panic!("Expected number literal, got {:?}", other),
        }
    }

    #[test]
    fn evaluate_data_should_enforce_number_maximum_constraint() {
        // Higher-standard requirement: user input must be validated against type constraints.
        // If this test fails, Lemma accepts invalid values and gives false reassurance.
        let data_path = DataPath::new(vec![], "x".to_string());

        let max10 = crate::planning::semantics::LemmaType::primitive(
            crate::planning::semantics::TypeSpecification::Number {
                minimum: None,
                maximum: Some(rational_new(10, 1)),
                decimals: None,
                help: String::new(),
            },
        );
        let source = Source::new(
            crate::parsing::source::SourceType::Volatile,
            crate::parsing::ast::Span {
                start: 0,
                end: 0,
                line: 1,
                col: 0,
            },
        );
        let mut data = IndexMap::new();
        data.insert(
            data_path.clone(),
            crate::planning::semantics::DataDefinition::Value {
                value: crate::planning::semantics::LiteralValue::number(rational_new(0, 1)),
                resolved_type: Arc::new(max10.clone()),
                source: source.clone(),
                origin: crate::planning::semantics::DataOrigin::Declared,
            },
        );

        let input_key_index = data.keys().map(|p| (p.input_key(), p.clone())).collect();
        let data_len = data.len();
        let plan = ExecutionPlan {
            spec_name: "test".to_string(),
            commentary: None,
            data,
            normal_forms: Vec::new(),
            rules: IndexMap::new(),
            data_reference_order: Vec::new(),
            meta: IndexMap::new(),
            resolved_types: ResolvedSpecTypes::default(),
            family_units: Arc::new(FamilyUnitCatalog::default()),
            signature_index: IndexMap::new(),
            effective: EffectiveDate::Origin,
            effective_from: None,
            effective_to: None,
            start_line: 1,
            source_type: None,
            needed_by_rules: vec![NeededByRules::default(); data_len],
            data_display: IndexMap::new(),
            show_rules: IndexMap::new(),
            show_needed_by: IndexMap::new(),
            show_import_data: IndexMap::new(),
            reference_ends: IndexMap::new(),
            input_key_index,
            show_rule_index: IndexMap::new(),
            data_leaf: IndexMap::new(),
            member_targets: IndexMap::new(),
            row_templates: std::collections::BTreeMap::new(),
            own_spec: Arc::new(LemmaSpec::new("test".to_string())),
            edges: Vec::new(),
            identity: SpecIdentity {
                repository: None,
                spec: "test".to_string(),
                effective: EffectiveDate::Origin,
                slice: EffectiveDate::Origin,
            },
            rule_candidates: std::collections::BTreeMap::new(),
            self_row_rules: Vec::new(),
            instance_lists: IndexMap::new(),
            row_mask: Vec::new(),
            initial_values: Vec::new(),
            walk_settled_leaves: Vec::new(),
            rule_ref_indexes: Vec::new(),
            data_slot_leaves: Vec::new(),
            data_slot_scale_targets: Vec::new(),
        };

        let values = input_data(&[("x", "11")]);

        let evaluate_data = resolve_evaluate_data(&plan, values);
        match veto_reason(&plan, &evaluate_data, &data_path) {
            Some(reason) => {
                assert!(
                    reason.contains("maximum") || reason.contains("10"),
                    "x=11 must violate maximum 10, got: {reason}"
                );
            }
            None => panic!("expected veto-bound data for x=11"),
        }
    }

    #[test]
    fn evaluate_data_should_enforce_text_enum_options() {
        // Higher-standard requirement: enum options must be enforced for text types.
        let data_path = DataPath::new(vec![], "tier".to_string());

        let mut tier = crate::planning::semantics::LemmaType::primitive(
            crate::planning::semantics::TypeSpecification::Text {
                length: None,
                help: String::new(),
            },
        );
        tier.options.values = vec![
            crate::planning::semantics::OptionValue {
                value: crate::planning::semantics::BoundValueKind::unbound(
                    crate::planning::semantics::ValueKind::Text(Arc::from("silver")),
                ),
            },
            crate::planning::semantics::OptionValue {
                value: crate::planning::semantics::BoundValueKind::unbound(
                    crate::planning::semantics::ValueKind::Text(Arc::from("gold")),
                ),
            },
        ];
        let source = Source::new(
            crate::parsing::source::SourceType::Volatile,
            crate::parsing::ast::Span {
                start: 0,
                end: 0,
                line: 1,
                col: 0,
            },
        );
        let mut data = IndexMap::new();
        data.insert(
            data_path.clone(),
            crate::planning::semantics::DataDefinition::Value {
                value: crate::planning::semantics::LiteralValue::text("silver".to_string()),
                resolved_type: Arc::new(tier.clone()),
                origin: crate::planning::semantics::DataOrigin::Declared,
                source,
            },
        );

        let input_key_index = data.keys().map(|p| (p.input_key(), p.clone())).collect();
        let data_len = data.len();
        let plan = ExecutionPlan {
            spec_name: "test".to_string(),
            commentary: None,
            data,
            normal_forms: Vec::new(),
            rules: IndexMap::new(),
            data_reference_order: Vec::new(),
            meta: IndexMap::new(),
            resolved_types: ResolvedSpecTypes::default(),
            family_units: Arc::new(FamilyUnitCatalog::default()),
            signature_index: IndexMap::new(),
            effective: EffectiveDate::Origin,
            effective_from: None,
            effective_to: None,
            start_line: 1,
            source_type: None,
            needed_by_rules: vec![NeededByRules::default(); data_len],
            data_display: IndexMap::new(),
            show_rules: IndexMap::new(),
            show_needed_by: IndexMap::new(),
            show_import_data: IndexMap::new(),
            reference_ends: IndexMap::new(),
            input_key_index,
            show_rule_index: IndexMap::new(),
            data_leaf: IndexMap::new(),
            member_targets: IndexMap::new(),
            row_templates: std::collections::BTreeMap::new(),
            own_spec: Arc::new(LemmaSpec::new("test".to_string())),
            edges: Vec::new(),
            identity: SpecIdentity {
                repository: None,
                spec: "test".to_string(),
                effective: EffectiveDate::Origin,
                slice: EffectiveDate::Origin,
            },
            rule_candidates: std::collections::BTreeMap::new(),
            self_row_rules: Vec::new(),
            instance_lists: IndexMap::new(),
            row_mask: Vec::new(),
            initial_values: Vec::new(),
            walk_settled_leaves: Vec::new(),
            rule_ref_indexes: Vec::new(),
            data_slot_leaves: Vec::new(),
            data_slot_scale_targets: Vec::new(),
        };

        let values = input_data(&[("tier", "platinum")]);

        let evaluate_data = resolve_evaluate_data(&plan, values);
        match veto_reason(&plan, &evaluate_data, &data_path) {
            Some(reason) => {
                assert!(
                    reason.contains("allowed options") || reason.contains("platinum"),
                    "invalid enum must record violation, got: {reason}"
                );
            }
            None => panic!("expected veto-bound data for tier=platinum"),
        }
    }

    fn empty_plan(effective: crate::parsing::ast::EffectiveDate) -> ExecutionPlan {
        ExecutionPlan {
            spec_name: "s".into(),
            commentary: None,
            data: IndexMap::new(),
            normal_forms: Vec::new(),
            rules: IndexMap::new(),
            data_reference_order: Vec::new(),
            meta: IndexMap::new(),
            resolved_types: ResolvedSpecTypes::default(),
            family_units: Arc::new(FamilyUnitCatalog::default()),
            signature_index: IndexMap::new(),
            effective,
            effective_from: None,
            effective_to: None,
            start_line: 1,
            source_type: None,
            needed_by_rules: Vec::new(),
            data_display: IndexMap::new(),
            show_rules: IndexMap::new(),
            show_needed_by: IndexMap::new(),
            show_import_data: IndexMap::new(),
            reference_ends: IndexMap::new(),
            input_key_index: IndexMap::new(),
            show_rule_index: IndexMap::new(),
            data_leaf: IndexMap::new(),
            member_targets: IndexMap::new(),
            row_templates: std::collections::BTreeMap::new(),
            own_spec: Arc::new(LemmaSpec::new("test".to_string())),
            edges: Vec::new(),
            identity: SpecIdentity {
                repository: None,
                spec: "s".to_string(),
                effective: EffectiveDate::Origin,
                slice: EffectiveDate::Origin,
            },
            rule_candidates: std::collections::BTreeMap::new(),
            self_row_rules: Vec::new(),
            instance_lists: IndexMap::new(),
            row_mask: Vec::new(),
            initial_values: Vec::new(),
            walk_settled_leaves: Vec::new(),
            rule_ref_indexes: Vec::new(),
            data_slot_leaves: Vec::new(),
            data_slot_scale_targets: Vec::new(),
        }
    }

    fn plans_by_effective(
        plans: impl IntoIterator<Item = ExecutionPlan>,
    ) -> BTreeMap<EffectiveDate, std::sync::Arc<ExecutionPlan>> {
        plans
            .into_iter()
            .map(|plan| (plan.effective.clone(), std::sync::Arc::new(plan)))
            .collect()
    }

    /// Compiled plans for one spec name are ordered by `effective` key.
    #[test]
    fn plan_set_plans_are_in_ascending_effective_order() {
        let june = DateTimeValue {
            year: 2025,
            month: 6,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            microsecond: 0,
            timezone: None,
            granularity: DateGranularity::Full,
        };
        let dec = DateTimeValue {
            year: 2025,
            month: 12,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            microsecond: 0,
            timezone: None,
            granularity: DateGranularity::Full,
        };

        let plans = plans_by_effective([
            empty_plan(EffectiveDate::Origin),
            empty_plan(EffectiveDate::DateTimeValue(june)),
            empty_plan(EffectiveDate::DateTimeValue(dec)),
        ]);

        let effectives: Vec<_> = plans.keys().cloned().collect();
        for window in effectives.windows(2) {
            assert!(
                window[0] < window[1],
                "plans must be strictly ascending: {:?} >= {:?}",
                window[0],
                window[1]
            );
        }
    }

    #[test]
    fn plan_at_exact_boundary_selects_later_slice() {
        use crate::parsing::ast::{DateTimeValue, EffectiveDate};

        let june = DateTimeValue {
            year: 2025,
            month: 6,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            microsecond: 0,
            timezone: None,

            granularity: DateGranularity::Full,
        };
        let dec = DateTimeValue {
            year: 2025,
            month: 12,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            microsecond: 0,
            timezone: None,

            granularity: DateGranularity::Full,
        };

        let june_key = EffectiveDate::DateTimeValue(june.clone());
        let dec_key = EffectiveDate::DateTimeValue(dec.clone());
        let plans = plans_by_effective([
            empty_plan(EffectiveDate::Origin),
            empty_plan(june_key.clone()),
            empty_plan(dec_key.clone()),
        ]);

        let june_plan = plan_at(&plans, &june_key).expect("boundary instant");
        assert!(std::ptr::eq(
            june_plan,
            plans.get(&june_key).expect("june slice").as_ref()
        ));

        let dec_plan = plan_at(&plans, &dec_key).expect("dec boundary");
        assert!(std::ptr::eq(
            dec_plan,
            plans.get(&dec_key).expect("dec slice").as_ref()
        ));
    }

    #[test]
    fn plan_at_day_before_boundary_stays_in_earlier_slice() {
        use crate::parsing::ast::{DateTimeValue, EffectiveDate};

        let june = DateTimeValue {
            year: 2025,
            month: 6,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            microsecond: 0,
            timezone: None,

            granularity: DateGranularity::Full,
        };
        let may_end = DateTimeValue {
            year: 2025,
            month: 5,
            day: 31,
            hour: 23,
            minute: 59,
            second: 59,
            microsecond: 0,
            timezone: None,

            granularity: DateGranularity::DateTime,
        };

        let origin = EffectiveDate::Origin;
        let plans = plans_by_effective([
            empty_plan(origin.clone()),
            empty_plan(EffectiveDate::DateTimeValue(june)),
        ]);

        let may_instant = EffectiveDate::DateTimeValue(may_end);
        let may_plan = plan_at(&plans, &may_instant).expect("may 31");
        assert!(std::ptr::eq(
            may_plan,
            plans.get(&origin).expect("origin slice").as_ref()
        ));
    }

    #[test]
    fn plan_at_single_plan_matches_any_instant_after_start() {
        use crate::parsing::ast::{DateTimeValue, EffectiveDate};

        let t = DateTimeValue {
            year: 2025,
            month: 3,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            microsecond: 0,
            timezone: None,

            granularity: DateGranularity::Full,
        };
        let start = EffectiveDate::DateTimeValue(DateTimeValue {
            year: 2025,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            microsecond: 0,
            timezone: None,

            granularity: DateGranularity::Full,
        });
        let plans = plans_by_effective([empty_plan(start.clone())]);
        let instant = EffectiveDate::DateTimeValue(t);
        let selected = plan_at(&plans, &instant).expect("inside single slice");
        assert!(std::ptr::eq(
            selected,
            plans.get(&start).expect("single slice").as_ref()
        ));
    }

    /// The show JSON shape is the IO contract for every non-Rust consumer
    /// (WASM playground, Hex, HTTP, TypeScript). Nail the exact envelope.
    #[test]
    fn show_json_shape_contract() {
        let mut engine = Engine::new();
        engine
            .load([(
                crate::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                    "test.lemma",
                ))),
                r#"
                spec pricing
                data bridge_height: measure
                  -> unit meter: 1
                  -> suggest 100 meter
                data quantity: number -> minimum 0
                rule cost: bridge_height * quantity
                "#
                .to_string(),
            )])
            .unwrap();
        let now = DateTimeValue::now();
        let schema = engine.show(None, "pricing", Some(&now)).unwrap();

        let value = crate::api::show_document(&engine, &schema);

        let bh = &value["properties"]["bridge_height"];
        assert!(
            bh.is_object(),
            "data entry must be a named object, not tuple"
        );
        assert!(
            bh.get("x-lemma-type").is_some(),
            "data entry must expose `x-lemma-type` field"
        );
        assert!(
            bh.get("x-lemma-suggestion").is_some(),
            "bridge_height exposes `-> suggest` as schema suggestion"
        );
        assert!(
            bh.get("x-lemma-value").is_none(),
            "bridge_height is not filled from spec"
        );

        let ty = &bh["x-lemma-type"];
        assert_eq!(
            ty["kind"], "measure",
            "kind tag sits on the type object itself"
        );
        assert!(
            ty["units"].is_array(),
            "measure-only fields flatten up to top level"
        );
        assert!(
            ty.get("options").is_none(),
            "text-only fields must not leak"
        );

        let quantity = &value["properties"]["quantity"];
        assert_eq!(quantity["x-lemma-type"]["kind"], "number");
        assert!(
            quantity.get("x-lemma-suggestion").is_none(),
            "quantity has no suggestion"
        );
        assert!(
            quantity.get("x-lemma-value").is_none(),
            "quantity has no fill literal"
        );

        let cost = &value["x-lemma-rules"]["cost"];
        assert_eq!(
            cost["type"]["kind"], "measure",
            "rule types nest under type"
        );
        assert!(
            cost["type"]["units"].is_array()
                && !cost["type"]["units"].as_array().unwrap().is_empty(),
            "measure rule result types expose declared units"
        );
        assert!(
            cost["type"]["units"][0].get("factor").is_some(),
            "measure rule units use factor field"
        );
        assert!(cost.get("branches").is_some(), "ShowRule exposes branches");
        assert!(
            cost.get("depends_on_rules").is_some(),
            "ShowRule exposes depends_on_rules"
        );
    }

    #[test]
    fn show_rule_result_units_contract() {
        let mut engine = Engine::new();
        engine
            .load([(
                crate::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                    "units_contract.lemma",
                ))),
                r#"
                spec units_contract
                data money: measure
                  -> unit eur: 1
                  -> unit usd: 0.91
                data rate: ratio
                  -> unit basis_points: 10000
                  -> unit percent: 100
                  -> suggest 500 basis_points
                rule total: money
                rule rate_out: rate
                "#
                .to_string(),
            )])
            .unwrap();
        let now = DateTimeValue::now();
        let schema = engine.show(None, "units_contract", Some(&now)).unwrap();
        let value = crate::api::show_document(&engine, &schema);

        let money_units = &value["properties"]["money"]["x-lemma-type"]["units"];
        assert!(money_units.is_array() && !money_units.as_array().unwrap().is_empty());
        assert!(money_units[0].get("name").is_some());
        assert!(money_units[0].get("factor").is_some());
        assert!(money_units[0]["factor"].get("numer").is_some());
        assert!(money_units[0]["factor"].get("denom").is_some());

        let rate_units = &value["properties"]["rate"]["x-lemma-type"]["units"];
        assert!(rate_units.is_array() && !rate_units.as_array().unwrap().is_empty());
        assert!(rate_units[0].get("name").is_some());
        assert!(rate_units[0].get("value").is_some());
        assert!(rate_units[0]["value"].get("numer").is_some());
        assert!(rate_units[0]["value"].get("denom").is_some());

        let total_rule_units = &value["x-lemma-rules"]["total"]["type"]["units"];
        let money_unit_names: Vec<_> = money_units
            .as_array()
            .unwrap()
            .iter()
            .map(|u| u["name"].as_str().unwrap())
            .collect();
        let total_rule_unit_names: Vec<_> = total_rule_units
            .as_array()
            .unwrap()
            .iter()
            .map(|u| u["name"].as_str().unwrap())
            .collect();
        assert_eq!(total_rule_unit_names, money_unit_names);

        let rate_out_rule_units = &value["x-lemma-rules"]["rate_out"]["type"]["units"];
        let rate_unit_names: Vec<_> = rate_units
            .as_array()
            .unwrap()
            .iter()
            .map(|u| u["name"].as_str().unwrap())
            .collect();
        let rate_out_rule_unit_names: Vec<_> = rate_out_rule_units
            .as_array()
            .unwrap()
            .iter()
            .map(|u| u["name"].as_str().unwrap())
            .collect();
        assert_eq!(rate_out_rule_unit_names, rate_unit_names);
    }

    #[test]
    fn show_document_rules_round_trip_as_show_rule() {
        let mut engine = Engine::new();
        engine
            .load([(
                crate::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("s.lemma"))),
                r#"
                spec s
                data age: number -> minimum 0 -> suggest 18
                data grade: text -> options "A", "B", "C"
                rule adult: age >= 18
                "#
                .to_string(),
            )])
            .unwrap();
        let now = DateTimeValue::now();
        let schema = engine.show(None, "s", Some(&now)).unwrap();

        let document = crate::api::show_document(&engine, &schema);
        let rule_json = document["x-lemma-rules"]["adult"].clone();
        let rule: crate::api::ShowRule = serde_json::from_value(rule_json.clone()).unwrap();
        assert_eq!(serde_json::to_value(&rule).unwrap(), rule_json);
        assert!(
            document["properties"].get("grade").is_none(),
            "unused grade is Types-only, not Data"
        );
        let grade = &document["x-lemma-types"]["grade"];
        assert_eq!(grade["options"], serde_json::json!(["A", "B", "C"]));
        assert_eq!(grade["kind"], "text");
    }

    const COST_PRICE_SPEC: &str = r#"
spec cost_price
uses lemma units

data money: measure
  -> unit eur: 1.00
  -> unit inr: 0.0092

data labor_cost: measure
  -> unit eur_per_hour: eur/hour
  -> unit inr_per_hour: inr/hour
  -> suggest 25 eur_per_hour

data product_cost: measure
  -> unit eur_per_kg: eur/kilogram
  -> unit inr_per_kg: inr/kilogram
  -> suggest 4 eur_per_kg

data throughput: measure
  -> unit kg_per_hour: kilogram/hour
  -> suggest 12 kg_per_hour

rule cost_price: product_cost + labor_cost / throughput
"#;

    fn cost_price_inputs() -> IndexMap<String, EvaluateDataValue> {
        let mut data = IndexMap::new();
        data.insert(
            "product_cost".into(),
            EvaluateDataValue::string("4 eur_per_kg"),
        );
        data.insert(
            "labor_cost".into(),
            EvaluateDataValue::string("25 eur_per_hour"),
        );
        data.insert(
            "throughput".into(),
            EvaluateDataValue::string("12 kg_per_hour"),
        );
        data
    }

    const FILM_ACCESS: &str = r#"
spec premium_membership
uses lemma units
data start: date
data length: units.calendar
rule valid: now in start...start + length

spec film_access
uses premium_membership
data type: text
  -> option "rental"
  -> option "purchase"
data views_consumed: number
data premium_member: boolean
rule max_views: 3
  unless premium_membership.valid then 10
  unless premium_member then 5
rule can_view: no
  unless type is "rental" and views_consumed < max_views then yes
  unless type is "purchase" then yes
"#;

    fn film_access_effective() -> DateTimeValue {
        DateTimeValue {
            year: 2027,
            month: 2,
            day: 14,
            hour: 12,
            minute: 0,
            second: 0,
            microsecond: 0,
            timezone: Some(TimezoneValue { offset_minutes: 0 }),
            granularity: DateGranularity::DateTime,
        }
    }

    #[test]
    fn evaluate_data_accepts_per_unit_measure_equivalent_to_canonical_magnitude() {
        let mut engine = Engine::new();
        engine
            .load([(
                crate::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                    "cost_price.lemma",
                ))),
                COST_PRICE_SPEC.to_string(),
            )])
            .expect("load");
        let plans = engine
            .plans
            .get_plans(None, "cost_price")
            .expect("plans for cost_price");
        let plan = plans.values().next().expect("plan");
        let mut data = IndexMap::new();
        data.insert(
            "product_cost".into(),
            EvaluateDataValue::string("4 eur_per_kg"),
        );
        data.insert(
            "labor_cost".into(),
            EvaluateDataValue::string("0.0069444444444444444444444444 eur_per_hour"),
        );
        data.insert(
            "throughput".into(),
            EvaluateDataValue::string("0.0033333333333333333333333333 kg_per_hour"),
        );
        let evaluate_data = resolve_evaluate_data(plan, data);
        assert!(
            !evaluate_data
                .bindings
                .values()
                .any(|b| matches!(b, OperationResult::Veto(_))),
            "parsed decimal run data values must not be veto-bound after input boundary: {:?}",
            evaluate_data.bindings
        );
    }

    #[test]
    fn evaluate_data_accepts_per_unit_measure() {
        let mut engine = Engine::new();
        engine
            .load([(
                crate::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                    "cost_price.lemma",
                ))),
                COST_PRICE_SPEC.to_string(),
            )])
            .expect("load");
        let plans = engine
            .plans
            .get_plans(None, "cost_price")
            .expect("plans for cost_price");
        let plan = plans.values().next().expect("plan");
        let evaluate_data = resolve_evaluate_data(plan, cost_price_inputs());
        assert!(!evaluate_data
            .bindings
            .values()
            .any(|b| matches!(b, OperationResult::Veto(_))));
    }

    #[test]
    fn evaluate_data_rejects_oversize_input() {
        let mut engine = Engine::new();
        engine
            .load([(
                crate::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                    "cost_price.lemma",
                ))),
                COST_PRICE_SPEC.to_string(),
            )])
            .expect("load");
        let plans = engine
            .plans
            .get_plans(None, "cost_price")
            .expect("plans for cost_price");
        let plan = plans.values().next().expect("plan");
        let mut data = cost_price_inputs();
        data.insert(
            "labor_cost".into(),
            EvaluateDataValue::string(
                "1000000000000000000000000000000000000000000000000000000000000 eur_per_hour",
            ),
        );
        let evaluate_data = resolve_evaluate_data(plan, data);
        assert!(matches!(
            binding(plan, &evaluate_data, &DataPath::local("labor_cost".into())),
            Some(OperationResult::Veto(_))
        ));
    }
    #[test]
    fn typedecl_default_stays_typedecl_on_immutable_plan() {
        let mut engine = Engine::new();
        engine
            .load([(
                crate::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("s.lemma"))),
                r#"
        spec s
        data n: number -> suggest 42
        rule r: n
    "#
                .to_string(),
            )])
            .expect("load");

        let plans = engine.plans.get_plans(None, "s").expect("plans for s");
        let plan = plans.values().next().expect("plan");
        let path = DataPath::local("n".into());
        match plan.data.get(&path).expect("n") {
            DataDefinition::TypeDeclaration {
                declared_suggestion: Some(_),
                ..
            } => {}
            other => panic!("expected TypeDeclaration with default, got {other:?}"),
        }
    }

    fn response_missing_data_union(response: &crate::Response) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        let mut names = Vec::new();
        for result in response.results.values() {
            for key in result.missing_data() {
                if seen.insert(key.clone()) {
                    names.push(key.clone());
                }
            }
        }
        names
    }

    #[test]
    fn evaluate_prunes_inactive_kind_branches_for_total_price() {
        let code = r#"
spec bag
uses lemma units

data weight: measure
  -> unit kg: 1

data money: measure
  -> unit eur: 1

data price_per_weight: measure
  -> unit eur_per_kg: eur/kg

data item_cost: price_per_weight
data finish_a: price_per_weight
data finish_b: price_per_weight

rule total_price: weight * (item_cost + finish_a + finish_b)

spec calc
uses bag
  -> with item_cost: item_cost
  -> with finish_a: finish_a

data kind: text -> options "red", "blue"

rule price_red: 1.5 eur_per_kg
rule price_red_finish_a: 0.45 eur_per_kg

rule price_blue: 2.0 eur_per_kg
rule price_blue_finish_a: 0.55 eur_per_kg

rule item_cost: veto "No item cost"
  unless kind is "red" then price_red
  unless kind is "blue" then price_blue

rule finish_a: veto "No finish_a"
  unless kind is "red" then price_red_finish_a
  unless kind is "blue" then price_blue_finish_a

rule total_price: bag.total_price
"#;

        let mut engine = Engine::new();
        engine
            .load([(
                crate::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                    "calc.lemma",
                ))),
                code.to_string(),
            )])
            .unwrap();

        let now = DateTimeValue::now();
        let mut inputs = IndexMap::new();
        inputs.insert("kind".to_string(), "red".into());
        let response = engine
            .evaluate(
                None,
                "calc",
                Some(&now),
                inputs,
                Some(&["total_price".to_string()]),
                false,
            )
            .expect("evaluate must succeed");

        assert_eq!(
            response_missing_data_union(&response),
            vec!["bag.weight".to_string(), "bag.finish_b".to_string(),],
            "supplied kind and with-bound finish_a/item_cost omitted"
        );
    }

    #[test]
    fn evaluate_includes_membership_dates_when_premium_member_false() {
        let mut engine = Engine::new();
        engine
            .load([(crate::SourceType::Volatile, FILM_ACCESS.to_string())])
            .expect("film_access spec must load");
        let now = film_access_effective();
        let mut inputs = IndexMap::new();
        inputs.insert("type".to_string(), "rental".into());
        inputs.insert("views_consumed".to_string(), "6".into());
        inputs.insert("premium_member".to_string(), "false".into());
        let response = engine
            .evaluate(
                None,
                "film_access",
                Some(&now),
                inputs,
                Some(&["can_view".to_string()]),
                false,
            )
            .expect("evaluate must succeed");

        assert_eq!(
            response_missing_data_union(&response),
            vec![
                "premium_membership.start".to_string(),
                "premium_membership.length".to_string(),
            ],
            "membership dates when premium_member is false"
        );
    }

    #[test]
    fn evaluate_includes_membership_dates_when_premium_member_unknown() {
        let mut engine = Engine::new();
        engine
            .load([(crate::SourceType::Volatile, FILM_ACCESS.to_string())])
            .expect("film_access spec must load");
        let now = film_access_effective();
        let mut inputs = IndexMap::new();
        inputs.insert("type".to_string(), "rental".into());
        inputs.insert("views_consumed".to_string(), "6".into());
        let response = engine
            .evaluate(
                None,
                "film_access",
                Some(&now),
                inputs,
                Some(&["can_view".to_string()]),
                false,
            )
            .expect("evaluate must succeed");

        assert_eq!(
            response_missing_data_union(&response),
            vec![
                "premium_member".to_string(),
                "premium_membership.start".to_string(),
                "premium_membership.length".to_string(),
            ],
            "premium_member plus membership dates when unknown"
        );
    }

    const UNITS_SPEC: &str = r#"
spec units
uses lemma units
data money: measure
  -> unit eur: 1
"#;

    const WAREHOUSING_SPEC: &str = r#"
spec warehousing
uses units
uses si: lemma units

data units_per_pallet: number
  -> minimum 1
  -> suggest 1

data storage_duration: si.duration
  -> minimum 0 week
  -> suggest 10 day

data interbranch_transport_per_pallet: units.money
  -> minimum 0 eur
  -> suggest 0 eur

data inbound_handling_per_pallet: units.money
  -> minimum 0 eur
  -> suggest 0 eur

data storage_per_pallet_per_week: units.money
  -> minimum 0 eur
  -> suggest 10 eur

data labeling_per_pallet: units.money
  -> minimum 0 eur
  -> suggest 0 eur

data outbound_handling_per_pallet: units.money
  -> minimum 0 eur
  -> suggest 0 eur

rule storage_cost_per_pallet:
  storage_per_pallet_per_week
  * (storage_duration as week as number -> ceil)

rule total_logistics_per_pallet:
  interbranch_transport_per_pallet
  + inbound_handling_per_pallet
  + storage_cost_per_pallet
  + labeling_per_pallet
  + outbound_handling_per_pallet

rule total_logistics_per_ce:
  total_logistics_per_pallet / units_per_pallet
"#;

    const QUOTATION_SPEC: &str = r#"
spec quotation
uses wh: warehousing
rule total: wh.total_logistics_per_ce
"#;

    fn load_cross_spec_fixtures(engine: &mut Engine) {
        engine
            .load([(crate::SourceType::Volatile, UNITS_SPEC.to_string())])
            .expect("units spec must load");
        engine
            .load([(crate::SourceType::Volatile, WAREHOUSING_SPEC.to_string())])
            .expect("warehousing spec must load");
    }

    #[test]
    fn quotation_inherits_unit_scope_from_warehousing_import() {
        let mut engine = Engine::new();
        load_cross_spec_fixtures(&mut engine);
        engine
            .load([(crate::SourceType::Volatile, QUOTATION_SPEC.to_string())])
            .expect("quotation must plan without uses lemma units");
        let plans = engine
            .plans
            .get_plans(None, "quotation")
            .expect("plans for quotation");
        let plan = plans.values().next().expect("plan");

        let expression_units = &plan.resolved_types.unit_index;
        expression_units
            .resolve("week")
            .expect("week reaches quotation via wh → si");
        expression_units
            .resolve("wh.week")
            .expect("alias.unit resolves week in warehousing scope");
        expression_units
            .resolve("minute")
            .expect("minute reaches quotation via transitive unit scope");
    }

    fn warehousing_default_inputs(prefix: &str) -> IndexMap<String, EvaluateDataValue> {
        let key = |name: &str| {
            if prefix.is_empty() {
                name.to_string()
            } else {
                format!("{prefix}.{name}")
            }
        };
        IndexMap::from([
            (key("units_per_pallet"), EvaluateDataValue::string("1")),
            (key("storage_duration"), EvaluateDataValue::string("10 day")),
            (
                key("interbranch_transport_per_pallet"),
                EvaluateDataValue::string("0 eur"),
            ),
            (
                key("inbound_handling_per_pallet"),
                EvaluateDataValue::string("0 eur"),
            ),
            (
                key("storage_per_pallet_per_week"),
                EvaluateDataValue::string("10 eur"),
            ),
            (
                key("labeling_per_pallet"),
                EvaluateDataValue::string("0 eur"),
            ),
            (
                key("outbound_handling_per_pallet"),
                EvaluateDataValue::string("0 eur"),
            ),
        ])
    }

    #[test]
    fn quotation_evaluates_cross_spec_duration_conversion() {
        let mut engine = Engine::new();
        load_cross_spec_fixtures(&mut engine);
        engine
            .load([(crate::SourceType::Volatile, QUOTATION_SPEC.to_string())])
            .expect("quotation must load");
        let plans = engine
            .plans
            .get_plans(None, "quotation")
            .expect("plans for quotation");
        let plan = plans.values().next().expect("plan");
        assert!(
            plan.resolved_types
                .unit_index
                .unique_owner("week")
                .is_none(),
            "consumer unit_index must not contain week"
        );
        let now = DateTimeValue::now();
        let response = engine
            .evaluate(
                None,
                "quotation",
                Some(&now),
                warehousing_default_inputs("wh"),
                None,
                false,
            )
            .expect("quotation must evaluate");
        let display = response
            .results
            .get("total")
            .expect("rule total must be present")
            .result()
            .expect("total must have display")
            .to_string();
        assert_eq!(
            display, "20 eur",
            "10 eur/week * ceil(10 day as week) / 1 CE must be 20 eur, got: {display}"
        );
    }

    #[test]
    fn ratio_range_default_endpoints_must_be_ratio_not_measure() {
        let code = r#"
spec policy
data allowed_band: ratio range -> suggest 10%...50%
rule band: allowed_band
"#;
        let mut engine = Engine::new();
        engine
            .load([(
                crate::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                    "ratio_range_endpoint_typing.lemma",
                ))),
                code.to_string(),
            )])
            .unwrap();

        let plans = engine
            .plans
            .get_plans(None, "policy")
            .expect("plans for policy");
        let plan = plans.values().next().expect("plan");
        let path = DataPath::local("allowed_band".into());
        let def = plan.data.get(&path).expect("allowed_band in plan.data");
        let suggestion = def
            .bound_suggestion()
            .expect("declared default must exist")
            .to_literal();

        let (left, right) = match &suggestion.value {
            crate::planning::semantics::ValueKind::Range(l, r) => (l.as_ref(), r.as_ref()),
            other => panic!("expected Range, got {other:?}"),
        };
        for (label, endpoint) in [("left", left), ("right", right)] {
            assert!(
                matches!(
                    &endpoint.value,
                    crate::planning::semantics::ValueKind::Ratio(_)
                ),
                "{label} endpoint must be Ratio for a percent literal in a ratio range default, got {:?}",
                endpoint.value
            );
            assert!(
                matches!(
                    &endpoint.value,
                    crate::planning::semantics::ValueKind::Ratio(_)
                ),
                "{label} endpoint ValueKind must be Ratio (got {:?})",
                endpoint.value
            );
        }
    }

    #[test]
    fn ratio_range_typedef_with_second_ratio_field_loads() {
        let code = r#"
spec policy
data margin_pct: ratio -> suggest 15%
data allowed_band: ratio range
rule margin: margin_pct
rule band_slot: allowed_band
"#;
        let mut engine = Engine::new();
        engine
            .load([(
                crate::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                    "ratio_range_load.lemma",
                ))),
                code.to_string(),
            )])
            .unwrap();

        let plans = engine
            .plans
            .get_plans(None, "policy")
            .expect("plans for policy");
        let plan = plans.values().next().expect("plan");
        let path = DataPath::local("allowed_band".into());
        let def = plan.data.get(&path).expect("allowed_band in plan.data");
        let lemma_type = def
            .schema_type()
            .expect("allowed_band must be a typed data slot");
        match &lemma_type.specifications {
            TypeSpecification::RatioRange { units, .. } => {
                let names: Vec<&str> = units.iter().map(|u| u.name.as_str()).collect();
                assert!(
                    names.contains(&"percent"),
                    "ratio range must inherit builtin percent, got {names:?}"
                );
            }
            other => panic!("allowed_band must be RatioRange, got {other:?}"),
        }
    }

    #[test]
    fn show_display_indents_every_commentary_line() {
        let mut engine = Engine::new();
        engine
            .load([(
                crate::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                    "alpha2.lemma",
                ))),
                "spec alpha2\n\"\"\"\nAdded:\nSS: South Sudan\n\nRemoved:\nYU: Yugoslavia\n\"\"\"\ndata code: text\nrule name: code\n"
                    .to_string(),
            )])
            .unwrap();
        let now = DateTimeValue::now();
        let text = engine.show(None, "alpha2", Some(&now)).unwrap().to_string();
        assert!(
            text.starts_with(
                "Spec: alpha2\n  Added:\n  SS: South Sudan\n\n  Removed:\n  YU: Yugoslavia\n"
            ),
            "{text}"
        );
    }

    #[test]
    fn show_display_puts_effective_window_on_header_without_other_versions() {
        let mut engine = Engine::new();
        engine
            .load([(
                crate::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                    "rates.lemma",
                ))),
                "spec rates 2024-01-01\n\"\"\"\nStandard rate\n\"\"\"\ndata x: 1\nrule r: x\n\nspec rates 2025-01-01\ndata x: 2\nrule r: x\n"
                    .to_string(),
            )])
            .unwrap();
        let effective: DateTimeValue = "2024-06-01".parse().expect("date must parse");
        let text = engine
            .show(None, "rates", Some(&effective))
            .unwrap()
            .to_string();
        assert!(
            text.starts_with("Spec: rates 2024-01-01...2025-01-01\n  Standard rate\n"),
            "{text}"
        );
        assert!(!text.contains("versions"), "{text}");
        assert!(!text.contains("effective_"), "{text}");
    }

    #[test]
    fn show_display_undated_predecessor_uses_until() {
        let mut engine = Engine::new();
        engine
            .load([(
                crate::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                    "calc.lemma",
                ))),
                "spec calc\ndata x: 1\nrule r: x\n\nspec calc 2027\ndata x: 2\nrule r: x\n"
                    .to_string(),
            )])
            .unwrap();
        let effective: DateTimeValue = "2026-01-01".parse().expect("date must parse");
        let text = engine
            .show(None, "calc", Some(&effective))
            .unwrap()
            .to_string();
        assert!(text.starts_with("Spec: calc ...2027\n"), "{text}");
        assert!(!text.contains('→'), "{text}");
    }

    fn nested_import_plan() -> (Engine, Arc<ExecutionPlan>) {
        let mut engine = Engine::new();
        engine
            .load([(
                crate::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                    "peel.lemma",
                ))),
                r#"
spec leaf
data x: number
rule rx: x

spec mid
uses nested: leaf
data y: number
rule ry: y

spec outer
uses m: mid
"#
                .to_string(),
            )])
            .expect("load nested import fixture");
        let plans = engine
            .plans
            .get_plans(None, "outer")
            .expect("plans for outer");
        let plan = Arc::clone(plans.values().next().expect("outer plan"));
        (engine, plan)
    }

    /// Template plan of import `alias` of `plan`.
    fn import_template<'a>(plan: &'a ExecutionPlan, alias: &str) -> &'a ExecutionPlan {
        let Some(DataDefinition::Import { row, .. }) =
            plan.data.get(&DataPath::local(alias.to_string()))
        else {
            panic!("`{alias}` must be an import of `{}`", plan.spec_name);
        };
        plan.row_template(&row.identity)
    }

    #[test]
    fn input_key_index_covers_own_slots_only() {
        let (_, plan) = nested_import_plan();

        let mut entry_keys: Vec<String> = plan.data.keys().map(DataPath::input_key).collect();
        let mut index_keys: Vec<String> = plan.input_key_index.keys().cloned().collect();
        entry_keys.sort();
        index_keys.sort();
        assert_eq!(entry_keys, index_keys);
        assert_eq!(entry_keys, vec!["m".to_string()]);
        assert_eq!(plan.input_slots().count(), 0);
    }

    #[test]
    fn import_rows_carry_their_template_data() {
        let (_, plan) = nested_import_plan();

        let mid = import_template(&plan, "m");
        assert_eq!(mid.spec_name, "mid");
        assert!(matches!(
            mid.data.get(&DataPath::local("y".into())),
            Some(DataDefinition::Value { .. } | DataDefinition::TypeDeclaration { .. })
        ));
        let leaf = import_template(mid, "nested");
        assert_eq!(leaf.spec_name, "leaf");
        assert!(matches!(
            leaf.data.get(&DataPath::local("x".into())),
            Some(DataDefinition::Value { .. } | DataDefinition::TypeDeclaration { .. })
        ));
        assert!(plan
            .data
            .keys()
            .chain(mid.data.keys())
            .chain(leaf.data.keys())
            .all(|path| path.segments.is_empty()));
    }

    #[test]
    fn rules_of_imports_stay_in_their_templates() {
        let (_, plan) = nested_import_plan();

        assert!(plan.rules.is_empty());
        let mid = import_template(&plan, "m");
        let leaf = import_template(mid, "nested");
        let keys = |template: &ExecutionPlan| -> Vec<String> {
            template.rules.keys().map(RulePath::input_key).collect()
        };
        assert_eq!(keys(mid), vec!["ry".to_string()]);
        assert_eq!(keys(leaf), vec!["rx".to_string()]);
        assert!(plan.get_rule("m.ry").is_none());
    }

    #[test]
    fn dotted_key_resolves_through_import_templates() {
        let (_, plan) = nested_import_plan();
        let (template, definition, _, field) =
            crate::evaluation::evaluate_data::row_field_definition(&plan, "m.nested.x")
                .expect("`m.nested.x` names data of the leaf template");
        assert_eq!(template.spec_name, "leaf");
        assert_eq!(field.data, "x");
        assert!(matches!(
            definition,
            DataDefinition::Value { .. } | DataDefinition::TypeDeclaration { .. }
        ));
        assert!(crate::evaluation::evaluate_data::row_field_definition(&plan, "m.x").is_none());
        let (template, definition, _, _) =
            crate::evaluation::evaluate_data::row_field_definition(&plan, "m.nested")
                .expect("`m.nested` names the `uses` slot of the mid template");
        assert_eq!(template.spec_name, "mid");
        assert!(matches!(definition, DataDefinition::Import { .. }));
    }
}

//! Execution plan for evaluated specs
//!
//! Provides a complete self-contained execution plan ready for the evaluator.
//! The plan holds an expanded shared normal-form graph in a dense table
//! ([`ExecutionPlan::normal_forms`]) addressed by [`NormalFormId`], plus all data —
//! no spec structure needed during evaluation.
//!
//! Reliability model:
//! - [`Show`] is the IO contract surface for consumers (data and rule outputs).
//!   IO compatibility is the consumer-facing guarantee.

use crate::literals::Value;
use crate::parsing::ast::{DateTimeValue, EffectiveDate, LemmaSpec};
use crate::parsing::source::{Source, SourceType};
use crate::planning::graph::absolute_import_member_path;
use crate::planning::graph::peel_rule_from;
use crate::planning::graph::Graph;
use crate::planning::graph::ResolvedSpecTypes;
use crate::planning::instance_list::{row_end_type, DataSource, InstanceListSource};
use crate::planning::normalize::{
    data_path_result_type, lookup_data_definition, DispatchRegion, NormalForm, NormalFormId,
    NormalFormInterner, NormalizeContext, NormalizedRule,
};
use crate::planning::semantics::{
    value_kind_matches_spec, ComparisonComputation, DataDefinition, DataPath, ImportOverlay,
    LemmaType, LiteralValue, PathSegment, ReferenceEnd, ReferenceTarget, RowEnd, RulePath,
    SpecIdentity, SpecInstance, TypeSpecification, ValueKind,
};
use crate::planning::show_expression::{
    prefix_show_expression, project_depends_on_rules, show_branches_from,
};
use crate::planning::spec_set::LemmaSpecSet;
use crate::planning::unit_family::FamilyUnitCatalog;
use crate::result_value::RuleResultValue;
use crate::Error;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

pub use crate::planning::show_expression::{
    ShowBranch, ShowConversionTarget, ShowExpression, ShowListCommand, ShowRule, SortDirection,
};

/// Show-rule positions that need one [`ExecutionPlan::data`] slot.
///
/// For an instance list, [`Self::fields`] is parallel to that list's row-template
/// `data` (which rules need each row field through this list). Otherwise `fields`
/// is empty.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct NeededByRules {
    pub rules: Vec<u32>,
    pub fields: Vec<Vec<u32>>,
}

/// A complete execution plan ready for the evaluator
///
/// Contains drift-free normal-form equations in a table, named rule roots, and all data.
/// Self-contained structure - no spec lookups required during evaluation.
///
/// A plan is immutable once stored. Show-cache fields such as `effective_to` depend on
/// neighboring versions, and those boundaries are part of `planning_input`, so a plan is
/// reused exactly when its inputs are unchanged and rebuilt otherwise.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionPlan {
    /// Main spec name
    pub spec_name: String,

    /// Optional commentary from the `"""..."""` block in the spec source.
    pub commentary: Option<String>,

    /// Per-data data in definition order: value, type-only, or spec reference.
    pub data: IndexMap<DataPath, DataDefinition>,

    /// Dense table of interned NormalForm cells — the expanded shared graph.
    /// Index = [`NormalFormId`].
    pub(crate) normal_forms: Vec<NormalForm>,

    /// Named rules → root of each rule in [`Self::normal_forms`].
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
    #[serde(with = "crate::snapshot::shared::arc")]
    pub family_units: Arc<FamilyUnitCatalog>,

    /// Reverse index: canonical-form unit signature `Vec<(unit_name, exponent)>` →
    /// (unit_name, owning type). Built from expression-scope units during planning so
    /// cross-type Multiply/Divide arithmetic can deterministically resolve a combined
    /// signature back to a single named unit. Ambiguous signatures (the same key matched
    /// by units in two distinct types) are rejected at planning time.
    pub signature_index: crate::computation::arithmetic::SignatureIndex,

    pub effective: EffectiveDate,

    /// Declared temporal window `[effective_from, effective_to)` of the LemmaSpec
    /// version this plan was built from. Filled by [`attach_show_cache`].
    pub effective_from: Option<DateTimeValue>,
    pub effective_to: Option<DateTimeValue>,

    /// Source start line of the LemmaSpec version this plan was built from.
    pub start_line: usize,

    /// Source type of the LemmaSpec version this plan was built from.
    pub source_type: Option<SourceType>,

    /// Per [`Self::data`] position: show-rule positions that need that slot (through
    /// rule references), alphabetical rule-name order. Non-promptable slots have empty
    /// `rules`. Instance-list slots also carry `fields` parallel to the row template's
    /// bindable fields. `len() == data.len()`. Built in [`build_execution_plan`].
    pub(crate) needed_by_rules: Vec<NeededByRules>,

    /// Named-`uses` members (not in [`Self::data`]): show-rule positions that need each
    /// nested path. Built with [`Self::needed_by_rules`].
    pub(crate) nested_needed_by_rules: IndexMap<DataPath, Vec<u32>>,

    /// Prefill/suggestion [`BoundValueKind`]s for show, keyed by data path.
    /// Built once at plan time; [`RuleResultValue`] is expanded at show.
    pub(crate) data_display: IndexMap<DataPath, ShowDataCache>,

    /// Show rule graph (type, branches, depends_on_rules), keyed by local rule path.
    /// Built in [`build_execution_plan`].
    pub(crate) show_rules: IndexMap<RulePath, ShowRule>,

    /// Every [`DataDefinition::Reference`] path → where its chain ends.
    /// Copied from [`Graph::reference_ends`] in [`build_execution_plan`].
    /// Missing key after planning is a bug.
    pub(crate) reference_ends: IndexMap<DataPath, ReferenceEnd>,

    /// Precomputed `input_key` → `DataPath` index for [`EvaluateData::resolve`].
    /// Built once at plan time from [`ExecutionPlan::input_key_entries`]. Keyed lookups
    /// index this map; they do not walk the data tree.
    pub(crate) input_key_index: IndexMap<String, DataPath>,

    /// Precomputed `input_key` → `RulePath` for [`Self::show_rules`].
    /// Built once so [`Self::get_rule`] is O(1) without allocating per candidate.
    pub(crate) show_rule_index: IndexMap<String, RulePath>,

    /// Every [`Self::data`] path → its [`LeafKind::DataPath`] cell in
    /// [`Self::normal_forms`]. Built once so evaluation writes bindings into
    /// the value table by [`NormalFormId`] instead of a parallel path map.
    pub(crate) data_leaf: IndexMap<DataPath, NormalFormId>,

    /// Template plans of instance-list row specs, by row spec. Each row of an
    /// instance list is evaluated in its own context over its template.
    #[serde(with = "crate::snapshot::shared::arc_map")]
    pub(crate) row_templates: BTreeMap<SpecIdentity, Arc<ExecutionPlan>>,

    /// This plan's own spec. Rows of its self-lists read this plan as their template.
    pub(crate) identity: SpecIdentity,

    /// Canonical bytes this plan was built from (see [`crate::planning::planning_input`]).
    /// A committed plan is reused only when these bytes equal the next pass's bytes.
    #[serde(with = "crate::snapshot::shared::arc")]
    pub(crate) planning_input: Arc<Vec<u8>>,

    /// Instances each local rule can yield, relative to this plan (empty when not
    /// spec-valued). Copied from [`Graph::rule_candidates`]; a consumer re-qualifies
    /// them under its `uses` hops.
    pub(crate) rule_candidates: BTreeMap<RulePath, Vec<SpecInstance>>,

    /// Rules read on rows of this plan's self-lists, in [`Self::rules`] order. A row
    /// tree fills them bottom-up, so a parent reads its children's values without
    /// evaluating down the tree.
    pub(crate) self_row_rules: Vec<RulePath>,

    /// Source rows of every [`DataDefinition::InstanceList`] path.
    pub(crate) instance_lists: IndexMap<DataPath, InstanceListSource>,

    /// Per [`Self::normal_forms`] cell: bit `L` is set when the cell reads the current
    /// row of the instance-list command at level `L` that encloses it. Such cells
    /// differ per row and are never cached in the value table.
    pub(crate) row_mask: Vec<u32>,
}

/// Plan-time prefill/suggestion for one data path (show cache).
///
/// Stores domain [`BoundValueKind`] — postcard-safe. [`RuleResultValue`] is built at
/// show time (JSON / API boundary), not snapshotted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ShowDataCache {
    pub value: Option<crate::planning::semantics::BoundValueKind>,
    pub suggestion: Option<crate::planning::semantics::BoundValueKind>,
}

/// A named rule's root in the normal-form table, plus declaration metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    #[serde(with = "crate::snapshot::shared::arc")]
    pub rule_type: Arc<LemmaType>,
}

impl ExecutableRule {
    pub fn name(&self) -> &str {
        &self.path.rule
    }
}

/// Position of a rule in [`ExecutionPlan::rules`]; indexes the evaluation
/// context's `rule_values` table. Plain `Copy` so the evaluation walk can
/// unwind to a rule without cloning its [`RulePath`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// Builds an execution plan from a Graph for one temporal slice.
///
/// `interner` dedups cons cells while the plan is built. The plan's dense `normal_forms`
/// table is extracted from it with [`NormalFormInterner::extract_reachable`], so the
/// result does not depend on what else the interner holds.
pub(crate) fn build_execution_plan(
    graph: &Graph<'_>,
    resolved_types: ResolvedSpecTypes,
    effective: &EffectiveDate,
    limits: &crate::limits::ResourceLimits,
    interner: &mut NormalFormInterner,
    planning_input: Arc<Vec<u8>>,
) -> Result<ExecutionPlan, Vec<Error>> {
    let rule_order = graph.rule_order();

    let main_spec = graph.main_spec();
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

    let signature_index = resolved_types.signature_index.clone();

    let family_units = Arc::new(FamilyUnitCatalog::build(&resolved_types.unit_index));

    let reference_ends = graph.reference_ends();
    let rule_types: HashMap<RulePath, Arc<LemmaType>> = graph
        .rules()
        .iter()
        .map(|(path, node)| (path.clone(), Arc::clone(&node.rule_type)))
        .collect();
    let mut rules: IndexMap<RulePath, ExecutableRule> = IndexMap::new();
    let mut completed_rules: HashMap<RulePath, NormalFormId> = HashMap::new();
    let mut show_rules: IndexMap<RulePath, ShowRule> = IndexMap::new();

    // Local rules plus depends_on_rules closure = Show.rules universe.
    let mut reachable: HashSet<RulePath> = HashSet::new();
    let mut stack: Vec<RulePath> = graph
        .rules()
        .keys()
        .filter(|path| path.segments.is_empty())
        .cloned()
        .collect();
    while let Some(path) = stack.pop() {
        if !reachable.insert(path.clone()) {
            continue;
        }
        let node = graph
            .rules()
            .get(&path)
            .expect("BUG: reachable rule path missing from graph.rules");
        for dep in &node.depends_on_rules {
            // Named-`uses` template rules are outside this graph; peel at eval.
            if graph.has_local_rule(dep) && !reachable.contains(dep) {
                stack.push(dep.clone());
            }
        }
    }
    let (show_keys, import_show_rules) = import_show_universe(graph, &reachable);

    let list_element_identities: HashSet<SpecIdentity> = graph
        .instance_lists()
        .sources
        .values()
        .map(|source| source.identity.clone())
        .collect();

    for rule_path in rule_order {
        if !reachable.contains(rule_path) {
            continue;
        }
        let rule_node = graph.rules().get(rule_path).expect(
            "bug: rule from topological sort not in graph - validation should have caught this",
        );

        let measure_scope = crate::planning::typing::MeasureScope {
            unit_index: &resolved_types.unit_index,
            signature_index: &resolved_types.signature_index,
        };
        let normalize_ctx = NormalizeContext {
            data: &data,
            row_templates: &graph.instance_lists().templates,
            self_identity: &graph.instance_lists().self_identity,
            list_element_identities: &list_element_identities,
            rule_order,
            measure_scope,
            max_normalized_expression_nodes: limits.max_normalized_expression_nodes,
        };
        let normalized = crate::planning::normalize::build_normalized_rule(
            &normalize_ctx,
            &completed_rules,
            &rule_types,
            reference_ends,
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

        show_rules.insert(
            rule_path.clone(),
            ShowRule {
                lemma_type: family_units.rule_type_for_show(rule_node.rule_type.as_ref()),
                path: rule_path.segments.clone(),
                branches: show_branches_from(
                    &rule_node.expression,
                    &rule_node.branches,
                    &show_keys,
                ),
                depends_on_rules: project_depends_on_rules(&rule_node.depends_on_rules, &show_keys),
            },
        );

        rules.insert(
            rule_path.clone(),
            ExecutableRule {
                path: rule_path.clone(),
                normal_form: body,
                source: rule_node.source.clone(),
                rule_type: source_type,
            },
        );
    }

    for (path, show_rule) in import_show_rules {
        show_rules.insert(path, show_rule);
    }

    let root_ids: Vec<NormalFormId> = rules.values().map(|rule| rule.normal_form).collect();
    let (mut normal_forms, remapped_roots) = interner.extract_reachable(&root_ids);
    for (rule, remapped) in rules.values_mut().zip(remapped_roots) {
        rule.normal_form = remapped;
    }

    // Reachable named-`uses` rules: ExecutableRule + ImportRule leaf so run/show
    // can address `alias.rule` like a local rule (eval peels into the template).
    let import_rule_paths: Vec<RulePath> = show_rules
        .keys()
        .filter(|path| !path.segments.is_empty())
        .cloned()
        .collect();
    for path in import_rule_paths {
        if rules.contains_key(&path) {
            continue;
        }
        let (template, local) = peel_rule_from(&data, &graph.instance_lists().templates, &path)
            .unwrap_or_else(|| {
                panic!(
                    "BUG: import show rule '{}' does not peel while building ExecutableRule",
                    path.input_key()
                )
            });
        let template_rule = template.rules.get(local).unwrap_or_else(|| {
            panic!(
                "BUG: peeled import rule '{}' missing from template.rules",
                path.input_key()
            )
        });
        let id = NormalFormId::from_index(normal_forms.len());
        normal_forms.push(NormalForm {
            kind: crate::planning::normalize::NormalFormKind::Leaf(
                crate::planning::normalize::LeafKind::ImportRule(path.clone()),
            ),
            result_type: Arc::clone(&template_rule.rule_type),
            fold_types: Vec::new(),
            source: Some(template_rule.source.clone()),
            origin: None,
            rule_ref: None,
        });
        rules.insert(
            path.clone(),
            ExecutableRule {
                path: path.clone(),
                normal_form: id,
                source: template_rule.source.clone(),
                rule_type: Arc::clone(&template_rule.rule_type),
            },
        );
    }

    let mut ordered: IndexMap<RulePath, ExecutableRule> = IndexMap::with_capacity(rules.len());
    for path in graph.execution_order() {
        match rules.shift_remove(path) {
            Some(rule) => {
                ordered.insert(path.clone(), rule);
            }
            None => assert!(
                graph.has_local_rule(path) && !reachable.contains(path),
                "BUG: execution-order rule '{}' has no ExecutableRule",
                path.input_key()
            ),
        }
    }
    // Template-internal named-`uses` rules (Show deps of reached ones): every owner
    // rule their overlays bind is already bound by a reached one, so already ordered.
    ordered.extend(rules);
    let rules = ordered;

    let mut plan = ExecutionPlan {
        spec_name: main_spec.name.clone(),
        commentary: main_spec.commentary.clone(),
        data,
        normal_forms,
        rules,
        data_reference_order: graph.data_reference_order().to_vec(),
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
        nested_needed_by_rules: IndexMap::new(),
        data_display: IndexMap::new(),
        show_rules,
        // Filled below after validation succeeds.
        reference_ends: IndexMap::new(),
        input_key_index: IndexMap::new(),
        show_rule_index: IndexMap::new(),
        data_leaf: IndexMap::new(),
        row_templates: graph.instance_lists().templates.clone(),
        identity: graph.instance_lists().self_identity.clone(),
        planning_input,
        rule_candidates: graph
            .rule_candidates()
            .iter()
            .map(|(rule, candidates)| (rule.clone(), candidates.clone()))
            .collect(),
        self_row_rules: Vec::new(),
        instance_lists: graph.instance_lists().sources.clone(),
        row_mask: Vec::new(),
    };

    let mut plan_errors = validate_literal_data_against_types(&plan);
    if let Err(error) = validate_unit_conversion_targets(&plan) {
        plan_errors.push(error);
    }
    if !plan_errors.is_empty() {
        return Err(plan_errors);
    }

    plan.reference_ends = graph.reference_ends().clone();
    let input_keys: Vec<DataPath> = plan
        .input_key_entries()
        .into_iter()
        .map(|(path, _)| path.clone())
        .collect();
    plan.data_leaf = ensure_data_leaves(
        &mut plan.normal_forms,
        &input_keys,
        &plan.data,
        &plan.row_templates,
    );
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
            data_path_result_type(&plan.data, path, &plan.row_templates).as_ref(),
            "BUG: DataPath leaf result_type drift for {path}"
        );
    }
    let (needed, nested_needed) = plan.build_needed_by_rules();
    plan.needed_by_rules = needed;
    plan.nested_needed_by_rules = nested_needed;
    plan.row_mask = build_row_mask(&plan.normal_forms).map_err(|error| vec![error])?;
    plan.self_row_rules = self_row_rules(&plan);
    Ok(plan)
}

/// Bitset for `identity`'s bindable template fields ([`ExecutionPlan::nested_input_keys`]).
fn row_bits_entry<'a>(
    plan: &ExecutionPlan,
    row_bits: &'a mut BTreeMap<SpecIdentity, Vec<u64>>,
    identity: &SpecIdentity,
) -> &'a mut Vec<u64> {
    let words = plan
        .row_template(identity)
        .nested_input_keys()
        .len()
        .div_ceil(64);
    row_bits
        .entry(identity.clone())
        .or_insert_with(|| vec![0u64; words])
}

fn template_bindable_field_index(template: &ExecutionPlan, path: &DataPath) -> Option<usize> {
    template
        .nested_input_keys()
        .iter()
        .position(|(candidate, _)| candidate == path)
}

fn mark_template_data_path(template: &ExecutionPlan, path: &DataPath, bits: &mut [u64]) {
    let target = match template.peel_data(path) {
        Some((_, _, DataDefinition::InstanceList { .. })) => Some(path),
        Some((_, _, DataDefinition::Value { .. } | DataDefinition::TypeDeclaration { .. })) => {
            Some(path)
        }
        Some((_, _, DataDefinition::Reference { .. })) => template.promptable_data_path(path),
        Some((_, _, DataDefinition::Import { .. })) | None => {
            if path.segments.is_empty() {
                template.promptable_data_path(path)
            } else {
                None
            }
        }
    };
    if let Some(target) = target {
        let data_pos = template_bindable_field_index(template, target).unwrap_or_else(|| {
            panic!(
                "BUG: promptable row field '{}' absent from template bindable fields",
                target.input_key()
            )
        });
        bits[data_pos / 64] |= 1u64 << (data_pos % 64);
    }
}

/// Data positions a finished template's show rule needs (from its `needed_by_rules`).
///
/// Bit indices match [`template_bindable_field_index`] / [`ExecutionPlan::nested_input_keys`],
/// not raw `template.data` positions.
fn mark_template_rule_data(template: &ExecutionPlan, rule: &RulePath, bits: &mut [u64]) {
    let rule_pos = u32::try_from(template.rules.get_index_of(rule).unwrap_or_else(|| {
        panic!(
            "BUG: row rule '{rule}' missing from template plan '{}'",
            template.spec_name
        )
    }))
    .expect("BUG: rule count exceeds u32");
    for (data_pos, consumers) in template.needed_by_rules.iter().enumerate() {
        if !consumers.rules.contains(&rule_pos) {
            continue;
        }
        let (path, definition) = template
            .data
            .get_index(data_pos)
            .expect("BUG: needed_by_rules index past template.data");
        if matches!(definition, DataDefinition::Import { .. }) {
            panic!(
                "BUG: Import marker '{}' carries needed_by bits in template '{}'",
                path.input_key(),
                template.spec_name
            );
        }
        let field_pos = template_bindable_field_index(template, path).unwrap_or_else(|| {
            panic!(
                "BUG: needed template data '{}' absent from template bindable fields",
                path.input_key()
            )
        });
        bits[field_pos / 64] |= 1u64 << (field_pos % 64);
    }
    for (nested_path, rule_positions) in &template.nested_needed_by_rules {
        if !rule_positions.contains(&rule_pos) {
            continue;
        }
        let field_pos = template_bindable_field_index(template, nested_path).unwrap_or_else(|| {
            panic!(
                "BUG: nested needed_by path '{}' absent from template bindable fields",
                nested_path.input_key()
            )
        });
        bits[field_pos / 64] |= 1u64 << (field_pos % 64);
    }
}

fn mark_row_end_needs(
    plan: &ExecutionPlan,
    identity: &SpecIdentity,
    end: &RowEnd,
    bits_by_rule: &[Vec<u64>],
    row_bits: &mut BTreeMap<SpecIdentity, Vec<u64>>,
    consumer_rule: &RulePath,
) {
    let bits = row_bits_entry(plan, row_bits, identity);
    match end {
        RowEnd::Data(path) => {
            if *identity == plan.identity {
                if let Some(ReferenceEnd::Rule(rule)) = plan.reference_ends.get(path) {
                    let rule_pos = plan.rules.get_index_of(rule).unwrap_or_else(|| {
                        panic!(
                            "BUG: reference rule '{rule}' missing from plan.rules (rule '{consumer_rule}')"
                        )
                    });
                    for (word, dep) in bits.iter_mut().zip(bits_by_rule[rule_pos].iter()) {
                        *word |= *dep;
                    }
                } else {
                    mark_template_data_path(plan, path, bits);
                }
            } else {
                let template = plan.row_template(identity);
                if let Some(ReferenceEnd::Rule(rule)) = template.reference_ends.get(path) {
                    mark_template_rule_data(template, rule, bits);
                } else {
                    mark_template_data_path(template, path, bits);
                }
            }
        }
        RowEnd::Rule(rule) => {
            if *identity == plan.identity {
                let rule_pos = plan.rules.get_index_of(rule).unwrap_or_else(|| {
                    panic!(
                        "BUG: row rule '{rule}' missing from plan.rules (rule '{consumer_rule}')"
                    )
                });
                for (word, dep) in bits.iter_mut().zip(bits_by_rule[rule_pos].iter()) {
                    *word |= *dep;
                }
            } else {
                mark_template_rule_data(plan.row_template(identity), rule, bits);
            }
        }
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
    let TypeSpecification::List { element, key } = &list_type.specifications else {
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
    for field in fields {
        let path = template.input_key_index.get(field).unwrap_or_else(|| {
            panic!(
                "BUG: list key field '{field}' missing from template '{}' (rule '{consumer_rule}')",
                identity
            )
        });
        mark_template_data_path(template, path, bits);
    }
}

/// [`ExecutionPlan::self_row_rules`]: every rule a row leaf or row member read of a
/// self-list names, directly or through a data reference to a rule.
fn self_row_rules(plan: &ExecutionPlan) -> Vec<RulePath> {
    use crate::planning::normalize::{LeafKind, NormalFormKind};
    use crate::planning::semantics::RowEnd;

    let mut rules_read: HashSet<&RulePath> = HashSet::new();
    for (index, cell) in plan.normal_forms.iter().enumerate() {
        let end = match &cell.kind {
            NormalFormKind::Leaf(LeafKind::Row { identity, end, .. })
                if *identity == plan.identity =>
            {
                end
            }
            NormalFormKind::SpecMember {
                base,
                row: Some(row_read),
                ..
            } => match &plan.result_type(*base).specifications {
                TypeSpecification::Spec { spec } if *spec == plan.identity => &row_read.end,
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

    /// Identity of the local `uses` Import named `name` on this plan.
    fn import_identity(&self, name: &str) -> Option<&SpecIdentity> {
        let import_path = self.input_key_index.get(name)?;
        let DataDefinition::Import { identity, .. } = self.data.get(import_path)? else {
            return None;
        };
        Some(identity)
    }

    /// Walk leading Import hops on this plan; each hop must be a local `uses` Import.
    pub(crate) fn peel_hops(&self, hops: &[PathSegment]) -> Option<&ExecutionPlan> {
        let mut plan = self;
        for hop in hops {
            plan = plan.row_template(plan.import_identity(&hop.uses)?);
        }
        Some(plan)
    }

    /// Walk `hop_names` as leading `uses` Imports, qualifying each hop. Returns the template
    /// plan reached and the qualified hops.
    fn qualify_hops<'a>(
        &'a self,
        hop_names: &[&str],
    ) -> Option<(&'a ExecutionPlan, Vec<PathSegment>)> {
        let mut plan = self;
        let mut segments = Vec::with_capacity(hop_names.len());
        for hop_name in hop_names {
            let identity = plan.import_identity(hop_name)?;
            segments.push(PathSegment {
                uses: (*hop_name).to_string(),
                repository: identity.repository.clone(),
                spec: identity.spec.clone(),
            });
            plan = plan.row_template(identity);
        }
        Some((plan, segments))
    }

    /// Qualified [`DataPath`] of the data named by `names` (`uses` hops, then the data name),
    /// `None` when a hop or the data name does not exist.
    pub(crate) fn qualify_data_names(&self, names: &[&str]) -> Option<DataPath> {
        let (data_name, hop_names) = names.split_last()?;
        let (plan, segments) = self.qualify_hops(hop_names)?;
        plan.data.get(plan.input_key_index.get(*data_name)?)?;
        Some(DataPath {
            segments,
            data: (*data_name).to_string(),
        })
    }

    /// Qualified [`RulePath`] of the rule named by `names` (`uses` hops, then the rule name),
    /// `None` when a hop or the rule does not exist.
    pub(crate) fn qualify_rule_names(&self, names: &[&str]) -> Option<RulePath> {
        let (rule_name, hop_names) = names.split_last()?;
        let (plan, segments) = self.qualify_hops(hop_names)?;
        let rule = RulePath {
            segments,
            rule: (*rule_name).to_string(),
        };
        plan.peel_rule(&RulePath {
            segments: Vec::new(),
            rule: (*rule_name).to_string(),
        })?;
        Some(rule)
    }

    /// Local data on this plan, or after peeling `path.segments` Import hops into a nested
    /// template. Returns the nested plan and that plan's local data path.
    pub(crate) fn peel_data<'a>(
        &'a self,
        path: &DataPath,
    ) -> Option<(&'a ExecutionPlan, &'a DataPath, &'a DataDefinition)> {
        let plan = self.peel_hops(&path.segments)?;
        let local = plan.input_key_index.get(path.data.as_str())?;
        let definition = plan.data.get(local)?;
        Some((plan, local, definition))
    }

    /// Local rule on this plan, or after peeling `path.segments` Import hops.
    pub(crate) fn peel_rule<'a>(
        &'a self,
        path: &RulePath,
    ) -> Option<(&'a ExecutionPlan, &'a RulePath)> {
        let plan = self.peel_hops(&path.segments)?;
        let local = plan
            .rules
            .keys()
            .find(|rule| rule.segments.is_empty() && rule.rule == path.rule)?;
        Some((plan, local))
    }

    /// Every bindable input key of this plan, relative to this plan: local data (Import
    /// markers included) and the nested non-marker members of each Import, through its
    /// row template. The only enumeration: leaves, `input_key_index`, show and the row
    /// bit sets all derive from it.
    pub(crate) fn input_key_entries(&self) -> Vec<(DataPath, &DataDefinition)> {
        let mut out = Vec::new();
        Self::push_input_key_entries(self, &mut out);
        out
    }

    /// [`Self::input_key_entries`] without the root Import markers: the members show and
    /// the row bit sets expose.
    pub(crate) fn nested_input_keys(&self) -> Vec<(DataPath, &DataDefinition)> {
        self.input_key_entries()
            .into_iter()
            .filter(|(_, definition)| !matches!(definition, DataDefinition::Import { .. }))
            .collect()
    }

    fn push_input_key_entries<'a>(
        root: &'a ExecutionPlan,
        out: &mut Vec<(DataPath, &'a DataDefinition)>,
    ) {
        struct PlanLevel<'a> {
            plan: &'a ExecutionPlan,
            next: usize,
            prefix: Vec<PathSegment>,
        }

        let mut levels = vec![PlanLevel {
            plan: root,
            next: 0,
            prefix: Vec::new(),
        }];
        while let Some(top) = levels.last_mut() {
            let Some((path, definition)) = top.plan.data.get_index(top.next) else {
                levels.pop();
                continue;
            };
            top.next += 1;
            if !path.segments.is_empty() {
                continue;
            }
            let child = match definition {
                DataDefinition::Import { identity, .. } => {
                    if top.prefix.is_empty() {
                        out.push((path.clone(), definition));
                    }
                    let mut nested_prefix = top.prefix.clone();
                    nested_prefix.push(PathSegment {
                        uses: path.data.clone(),
                        repository: identity.repository.clone(),
                        spec: identity.spec.clone(),
                    });
                    PlanLevel {
                        plan: top.plan.row_template(identity),
                        next: 0,
                        prefix: nested_prefix,
                    }
                }
                _ => {
                    out.push((
                        DataPath {
                            segments: top.prefix.clone(),
                            data: path.data.clone(),
                        },
                        definition,
                    ));
                    continue;
                }
            };
            levels.push(child);
        }
    }
}

/// Nesting of instance-list commands a plan supports: one bit per level in [`ExecutionPlan::row_mask`].
const MAX_ROW_LEVELS: u32 = u32::BITS;

/// [`ExecutionPlan::row_mask`]: a row leaf sets its level, a list command clears its
/// own level, every other cell takes the union of its children.
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
            let bits = match &cell.kind {
                NormalFormKind::Leaf(LeafKind::Row { level, .. }) => {
                    if *level >= MAX_ROW_LEVELS {
                        return Err(Error::validation(
                            format!(
                                "list commands nest {} levels deep; at most {MAX_ROW_LEVELS} are supported",
                                level + 1
                            ),
                            cell.source.clone(),
                            None::<String>,
                        ));
                    }
                    1u32 << level
                }
                NormalFormKind::ListCommand {
                    row_level: Some(level),
                    ..
                } if *level < MAX_ROW_LEVELS => union & !(1u32 << level),
                _ => union,
            };
            mask[index] = Some(bits);
        }
    }
    Ok(mask
        .into_iter()
        .map(|bits| bits.expect("BUG: row mask missing for a cell"))
        .collect())
}

/// Local reachable Show keys plus ShowRule entries for named-`uses` template rules
/// those locals depend on (absolute `alias…rule` keys).
fn import_show_universe(
    graph: &Graph<'_>,
    local_reachable: &HashSet<RulePath>,
) -> (HashSet<String>, IndexMap<RulePath, ShowRule>) {
    let mut show_keys: HashSet<String> = local_reachable
        .iter()
        .map(|path| path.input_key())
        .collect();
    let mut import_shows: IndexMap<RulePath, ShowRule> = IndexMap::new();
    let mut stack: Vec<RulePath> = local_reachable
        .iter()
        .flat_map(|path| {
            graph
                .rules()
                .get(path)
                .expect("BUG: reachable local rule missing from graph.rules")
                .depends_on_rules
                .iter()
                .cloned()
        })
        .filter(|dep| !graph.has_local_rule(dep))
        .collect();

    while let Some(path) = stack.pop() {
        let key = path.input_key();
        if !show_keys.insert(key.clone()) {
            continue;
        }
        let (template, local) =
            peel_rule_from(graph.data(), &graph.instance_lists().templates, &path).unwrap_or_else(
                || {
                    panic!(
                        "BUG: import Show rule '{}' does not peel after planning accepted it",
                        path.input_key()
                    )
                },
            );
        let template_show = template.show_rules.get(local).unwrap_or_else(|| {
            panic!(
                "BUG: peeled import rule '{}' missing from template show_rules",
                path.input_key()
            )
        });
        let prefix = path
            .segments
            .iter()
            .map(|segment| segment.uses.as_str())
            .collect::<Vec<_>>()
            .join(".");
        let mut abs_deps = Vec::with_capacity(template_show.depends_on_rules.len());
        for dep_key in &template_show.depends_on_rules {
            let abs_key = format!("{prefix}.{dep_key}");
            abs_deps.push(abs_key.clone());
            let abs_path = absolute_import_rule_path(&path.segments, dep_key, template);
            if !show_keys.contains(&abs_key) {
                stack.push(abs_path);
            }
        }
        let branches = template_show
            .branches
            .iter()
            .map(|branch| ShowBranch {
                condition: branch
                    .condition
                    .as_ref()
                    .map(|expression| prefix_show_expression(&prefix, expression)),
                result: prefix_show_expression(&prefix, &branch.result),
            })
            .collect();
        import_shows.insert(
            path.clone(),
            ShowRule {
                lemma_type: template_show.lemma_type.clone(),
                path: path.segments.clone(),
                branches,
                depends_on_rules: abs_deps,
            },
        );
    }

    (show_keys, import_shows)
}

fn absolute_import_rule_path(
    prefix: &[crate::planning::semantics::PathSegment],
    local_key: &str,
    template: &ExecutionPlan,
) -> RulePath {
    let local = template
        .show_rules
        .keys()
        .find(|path| path.input_key() == local_key)
        .unwrap_or_else(|| {
            panic!(
                "BUG: template depends_on_rules entry '{local_key}' missing from template show_rules"
            )
        });
    RulePath {
        segments: prefix
            .iter()
            .cloned()
            .chain(local.segments.iter().cloned())
            .collect(),
        rule: local.rule.clone(),
    }
}

/// Owner-frame input of a read, references resolved. Named-`uses` reads peel into
/// templates and apply each hop's site overlay; the outermost site overlay wins over
/// inner template binds on the same path, as in evaluation
/// ([`crate::evaluation::imports::eval_import_member`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ImportNeed {
    /// Non-reference local slot, or a nested member the template declares
    /// (unbound or bound to an overlay literal).
    Data(DataPath),
    /// Local rule.
    Rule(RulePath),
}

/// A planning view that resolves named-`uses` members: a finished [`ExecutionPlan`]
/// or the planning graph. Static dispatch only.
pub(crate) trait ReferenceOwner {
    fn owner_data(&self) -> &IndexMap<DataPath, DataDefinition>;
    fn owner_templates(&self) -> &BTreeMap<SpecIdentity, Arc<ExecutionPlan>>;
}

impl ReferenceOwner for ExecutionPlan {
    fn owner_data(&self) -> &IndexMap<DataPath, DataDefinition> {
        &self.data
    }

    fn owner_templates(&self) -> &BTreeMap<SpecIdentity, Arc<ExecutionPlan>> {
        &self.row_templates
    }
}

struct ImportHop<'p> {
    marker: DataPath,
    identity: &'p SpecIdentity,
    overlay: &'p IndexMap<DataPath, ImportOverlay>,
    template: &'p ExecutionPlan,
}

impl ImportHop<'_> {
    fn absolute(&self, relative: &DataPath) -> DataPath {
        absolute_import_member_path(&self.marker, self.identity, relative)
    }

    fn absolute_rule(&self, relative: &RulePath) -> RulePath {
        let absolute = self.absolute(&DataPath {
            segments: relative.segments.clone(),
            data: relative.rule.clone(),
        });
        RulePath {
            segments: absolute.segments,
            rule: absolute.data,
        }
    }
}

/// First hop of a named-`uses` path; `None` when `data` has no Import (or no
/// template) for it.
fn import_hop<'p>(
    data: &'p IndexMap<DataPath, DataDefinition>,
    templates: &'p BTreeMap<SpecIdentity, Arc<ExecutionPlan>>,
    segment: &PathSegment,
) -> Option<ImportHop<'p>> {
    let marker = DataPath::local(segment.uses.clone());
    let DataDefinition::Import {
        identity, overlay, ..
    } = data.get(&marker)?
    else {
        return None;
    };
    let template = templates.get(identity)?;
    Some(ImportHop {
        marker,
        identity,
        overlay,
        template,
    })
}

/// Owner-frame [`ReferenceEnd`] of named-`uses` member `path` (non-empty segments):
/// the first hop's site overlay entry, else the template's own end re-qualified
/// under the hop. `None` when `path` does not peel.
/// Frame owner of a named-`uses` walk: the caller's owner, or a row template plan reached
/// through a hop.
enum ReferenceFrameOwner<'a, O: ReferenceOwner> {
    Root(&'a O),
    Template(&'a ExecutionPlan),
}

impl<O: ReferenceOwner> Clone for ReferenceFrameOwner<'_, O> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<O: ReferenceOwner> Copy for ReferenceFrameOwner<'_, O> {}

impl<'a, O: ReferenceOwner> ReferenceFrameOwner<'a, O> {
    fn data(self) -> &'a IndexMap<DataPath, DataDefinition> {
        match self {
            Self::Root(owner) => owner.owner_data(),
            Self::Template(plan) => plan.owner_data(),
        }
    }

    fn templates(self) -> &'a BTreeMap<SpecIdentity, Arc<ExecutionPlan>> {
        match self {
            Self::Root(owner) => owner.owner_templates(),
            Self::Template(plan) => plan.owner_templates(),
        }
    }

    fn reference_end_on(self, path: &DataPath) -> Option<ReferenceEnd> {
        match self {
            Self::Root(owner) => reference_end_on(owner, path),
            Self::Template(plan) => reference_end_on(plan, path),
        }
    }
}

struct ReferenceFrame<'a, O: ReferenceOwner> {
    owner: ReferenceFrameOwner<'a, O>,
    path: DataPath,
    hop: ImportHop<'a>,
    relative: DataPath,
}

enum ReferenceStep<'a, O: ReferenceOwner> {
    /// Resolution finished with this value (`None`: the path does not peel).
    Value(Option<ReferenceEnd>),
    /// Resolve this path on this owner; its value is the value of the current frame.
    Next(ReferenceFrameOwner<'a, O>, DataPath),
}

/// [`ReferenceEnd`] of data path `path` on `owner`: local reference chains and
/// named-`uses` members (first hop's site overlay entry, else the template's own end
/// re-qualified under the hop), resolved in one worklist. `None` when `path` does not
/// peel or reaches missing data.
pub(crate) fn reference_end_on<O: ReferenceOwner>(
    owner: &O,
    path: &DataPath,
) -> Option<ReferenceEnd> {
    let mut frames: Vec<ReferenceFrame<'_, O>> = Vec::new();
    let mut owner = ReferenceFrameOwner::Root(owner);
    let mut path = path.clone();
    let mut chain = 0usize;
    loop {
        let depth = frames.len();
        let data_len = owner.data().len();
        let mut end = match resolve_reference_step(owner, &path, &mut frames) {
            ReferenceStep::Value(end) => end,
            ReferenceStep::Next(next_owner, next_path) => {
                chain = if frames.len() == depth { chain + 1 } else { 0 };
                assert!(
                    chain <= data_len,
                    "BUG: data reference chain from '{path}' outlasts plan data (cycles are rejected)"
                );
                owner = next_owner;
                path = next_path;
                continue;
            }
        };
        loop {
            let Some(frame) = frames.pop() else {
                return end;
            };
            let template_end = end?;
            match finish_reference_frame(frame, template_end) {
                ReferenceStep::Value(value) => end = value,
                ReferenceStep::Next(next_owner, next_path) => {
                    chain = 0;
                    owner = next_owner;
                    path = next_path;
                    break;
                }
            }
        }
    }
}

fn resolve_reference_step<'a, O: ReferenceOwner>(
    owner: ReferenceFrameOwner<'a, O>,
    path: &DataPath,
    frames: &mut Vec<ReferenceFrame<'a, O>>,
) -> ReferenceStep<'a, O> {
    match owner.data().get(path) {
        Some(
            DataDefinition::Value { .. }
            | DataDefinition::TypeDeclaration { .. }
            | DataDefinition::InstanceList { .. },
        ) => return ReferenceStep::Value(Some(ReferenceEnd::Promptable(path.clone()))),
        Some(DataDefinition::Import { .. }) => {
            return ReferenceStep::Value(Some(ReferenceEnd::Import));
        }
        Some(DataDefinition::Reference {
            target: ReferenceTarget::Rule(rule),
            ..
        }) => return ReferenceStep::Value(Some(ReferenceEnd::Rule(rule.clone()))),
        Some(DataDefinition::Reference {
            target: ReferenceTarget::Data(next),
            ..
        }) => return ReferenceStep::Next(owner, next.clone()),
        None => {}
    }
    if path.segments.is_empty() {
        return ReferenceStep::Value(None);
    }
    let (first, rest) = path
        .segments
        .split_first()
        .unwrap_or_else(|| panic!("BUG: reference_end_on empty member path '{path}'"));
    let Some(hop) = import_hop(owner.data(), owner.templates(), first) else {
        return ReferenceStep::Value(None);
    };
    let relative = DataPath {
        segments: rest.to_vec(),
        data: path.data.clone(),
    };
    if let Some(entry) = hop.overlay.get(&relative) {
        return match entry {
            ImportOverlay::Literal(_) => {
                ReferenceStep::Value(Some(ReferenceEnd::Promptable(path.clone())))
            }
            ImportOverlay::Reference {
                target: ReferenceTarget::Data(target),
                ..
            } => ReferenceStep::Next(owner, target.clone()),
            ImportOverlay::Reference {
                target: ReferenceTarget::Rule(rule),
                ..
            } => ReferenceStep::Value(Some(ReferenceEnd::Rule(rule.clone()))),
        };
    }
    let template = hop.template;
    if relative.segments.is_empty() && !template.data.contains_key(&relative) {
        return ReferenceStep::Value(None);
    }
    frames.push(ReferenceFrame {
        owner,
        path: path.clone(),
        hop,
        relative: relative.clone(),
    });
    ReferenceStep::Next(ReferenceFrameOwner::Template(template), relative)
}

fn finish_reference_frame<'a, O: ReferenceOwner>(
    frame: ReferenceFrame<'a, O>,
    template_end: ReferenceEnd,
) -> ReferenceStep<'a, O> {
    match template_end {
        ReferenceEnd::Promptable(target) if target == frame.relative => {
            ReferenceStep::Value(Some(ReferenceEnd::Promptable(frame.path)))
        }
        ReferenceEnd::Promptable(target) => {
            ReferenceStep::Next(frame.owner, frame.hop.absolute(&target))
        }
        ReferenceEnd::Rule(rule) => {
            ReferenceStep::Value(Some(ReferenceEnd::Rule(frame.hop.absolute_rule(&rule))))
        }
        ReferenceEnd::Import => ReferenceStep::Value(Some(ReferenceEnd::Import)),
    }
}

/// The need a resolved end contributes by itself, or the non-empty-`uses` rule it still
/// has to expand through its owner.
fn need_of_end(end: ReferenceEnd) -> Result<Option<ImportNeed>, RulePath> {
    match end {
        ReferenceEnd::Promptable(path) => Ok(Some(ImportNeed::Data(path))),
        ReferenceEnd::Rule(rule) if rule.segments.is_empty() => Ok(Some(ImportNeed::Rule(rule))),
        ReferenceEnd::Rule(rule) => Err(rule),
        ReferenceEnd::Import => Ok(None),
    }
}

fn needs_of_end<O: ReferenceOwner>(owner: &O, end: ReferenceEnd) -> Vec<ImportNeed> {
    match need_of_end(end) {
        Ok(need) => need.into_iter().collect(),
        Err(rule) => import_rule_needs(owner, &rule),
    }
}

/// Owner-frame needs of evaluating named-`uses` rule `path` (non-empty segments):
/// the template rule's needs, each re-qualified under the hop and resolved through
/// [`reference_end_on`].
pub(crate) fn import_rule_needs<O: ReferenceOwner>(owner: &O, path: &RulePath) -> Vec<ImportNeed> {
    let mut frames = vec![NeedsFrame::new(
        ReferenceFrameOwner::Root(owner),
        path.clone(),
        false,
    )];
    loop {
        let top = frames
            .last_mut()
            .expect("BUG: import rule frames empty before the root finished");
        if top.hop.is_none() {
            let (first, rest) = top.rule.segments.split_first().unwrap_or_else(|| {
                panic!("BUG: import_rule_needs on local rule '{}'", top.rule.rule)
            });
            let hop =
                import_hop(top.owner.data(), top.owner.templates(), first).unwrap_or_else(|| {
                    panic!(
                        "BUG: import rule '{}' first hop does not peel",
                        top.rule.input_key()
                    )
                });
            let relative = RulePath {
                segments: rest.to_vec(),
                rule: top.rule.rule.clone(),
            };
            let template = hop.template;
            top.hop = Some(hop);
            if relative.segments.is_empty() {
                top.needs = Some(template.rule_needs(&relative));
            } else {
                frames.push(NeedsFrame::new(
                    ReferenceFrameOwner::Template(template),
                    relative,
                    true,
                ));
                continue;
            }
        }

        let needs = top
            .needs
            .as_ref()
            .expect("BUG: entered import rule frame has no template needs");
        if top.next == needs.len() {
            let finished = frames
                .pop()
                .expect("BUG: finished import rule frame missing from stack");
            match frames.last_mut() {
                None => return finished.out,
                Some(parent) if finished.feeds_parent_needs => {
                    parent.needs = Some(finished.out);
                    parent.next = 0;
                }
                Some(parent) => parent.out.extend(finished.out),
            }
            continue;
        }

        let item = needs[top.next].clone();
        top.next += 1;
        let owner = top.owner;
        let hop = top
            .hop
            .as_ref()
            .expect("BUG: entered import rule frame has no hop");
        let child = match item {
            ImportNeed::Data(field) => {
                let absolute = hop.absolute(&field);
                let end = owner.reference_end_on(&absolute).unwrap_or_else(|| {
                    panic!("BUG: template need '{absolute}' does not peel on its owner")
                });
                match need_of_end(end) {
                    Ok(Some(need)) => {
                        top.out.push(need);
                        None
                    }
                    Ok(None) => None,
                    Err(rule) => Some(NeedsFrame::new(owner, rule, false)),
                }
            }
            ImportNeed::Rule(rule) => Some(NeedsFrame::new(owner, hop.absolute_rule(&rule), false)),
        };
        if let Some(child) = child {
            frames.push(child);
        }
    }
}

/// One named-`uses` rule being expanded by [`import_rule_needs`]. `hop` and `needs` are set
/// once the frame is entered; `needs` are the template-level needs it maps through `hop`.
/// A frame with `feeds_parent_needs` supplies its parent's template-level needs instead of
/// its output.
struct NeedsFrame<'a, O: ReferenceOwner> {
    owner: ReferenceFrameOwner<'a, O>,
    rule: RulePath,
    hop: Option<ImportHop<'a>>,
    needs: Option<Vec<ImportNeed>>,
    next: usize,
    out: Vec<ImportNeed>,
    feeds_parent_needs: bool,
}

impl<'a, O: ReferenceOwner> NeedsFrame<'a, O> {
    fn new(owner: ReferenceFrameOwner<'a, O>, rule: RulePath, feeds_parent_needs: bool) -> Self {
        Self {
            owner,
            rule,
            hop: None,
            needs: None,
            next: 0,
            out: Vec::new(),
            feeds_parent_needs,
        }
    }
}

/// Pass-1 accumulator of [`ExecutionPlan::build_needed_by_rules`] for one rule.
struct NeedsMarker<'a> {
    plan: &'a ExecutionPlan,
    rule_pos: usize,
    bits: &'a mut [u64],
    nested_hits: &'a mut HashSet<DataPath>,
    bits_by_rule: &'a [Vec<u64>],
    nested_hits_by_rule: &'a [HashSet<DataPath>],
}

impl NeedsMarker<'_> {
    fn mark_slot(&mut self, data_pos: usize) {
        self.bits[data_pos / 64] |= 1u64 << (data_pos % 64);
    }

    fn mark(&mut self, need: ImportNeed) {
        match need {
            ImportNeed::Rule(rule) => {
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
                self.nested_hits
                    .extend(self.nested_hits_by_rule[ref_pos].iter().cloned());
            }
            ImportNeed::Data(path) if path.segments.is_empty() => {
                let data_pos =
                    self.plan.data.get_index_of(&path).unwrap_or_else(|| {
                        panic!("BUG: needed data '{path}' missing from plan.data")
                    });
                self.mark_slot(data_pos);
            }
            ImportNeed::Data(path) => match self.plan.data.get_index_of(&path) {
                Some(data_pos) => self.mark_slot(data_pos),
                None => {
                    self.nested_hits.insert(path);
                }
            },
        }
    }
}

/// Ensure every input key (see [`ExecutionPlan::input_key_entries`]) has a [`LeafKind::DataPath`] cell in `normal_forms`
/// and return the path → id index. Expression lowering already creates leaves for
/// referenced paths; unused paths get orphan leaves appended so evaluation can
/// still store bindings/defaults/reference copies in the value table.
///
/// Rule reference cells share a body's Kind, so a `Leaf(DataPath)` cell may carry
/// `rule_ref`. Those cells hold the referenced rule's value in the value table and
/// must never be the binding slot for the path.
fn ensure_data_leaves(
    normal_forms: &mut Vec<NormalForm>,
    input_keys: &[DataPath],
    data: &IndexMap<DataPath, DataDefinition>,
    row_templates: &std::collections::BTreeMap<
        crate::planning::semantics::SpecIdentity,
        std::sync::Arc<ExecutionPlan>,
    >,
) -> IndexMap<DataPath, NormalFormId> {
    use crate::planning::normalize::LeafKind;
    use crate::planning::normalize::NormalFormKind;

    let mut data_leaf: IndexMap<DataPath, NormalFormId> = IndexMap::new();
    for (index, cell) in normal_forms.iter().enumerate() {
        if cell.rule_ref.is_some() {
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
            result_type: data_path_result_type(data, path, row_templates),
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
    for (path, data) in plan.nested_input_keys() {
        if out.contains_key(&path) {
            continue;
        }
        // Overlay literal on the owning Import prefills the nested slot.
        if let Some(bound) = overlay_literal_for(plan, &path) {
            out.insert(
                path,
                ShowDataCache {
                    value: Some(bound),
                    suggestion: None,
                },
            );
            continue;
        }
        insert_data_display_entry(&mut out, &path, data);
    }
    out
}

fn overlay_literal_for(
    plan: &ExecutionPlan,
    path: &DataPath,
) -> Option<crate::planning::semantics::BoundValueKind> {
    let first = path.segments.first()?;
    let import_marker = DataPath::local(first.uses.clone());
    let DataDefinition::Import { overlay, .. } = plan.data.get(&import_marker)? else {
        return None;
    };
    let relative = DataPath {
        segments: path.segments[1..].to_vec(),
        data: path.data.clone(),
    };
    match overlay.get(&relative)? {
        ImportOverlay::Literal(bound) => Some(bound.clone()),
        ImportOverlay::Reference { .. } => None,
    }
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
        } => Some(crate::planning::semantics::BoundValueKind {
            value: value.value.clone(),
            unit: resolved_type.unit().map(Arc::from),
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
    bound: &crate::planning::semantics::BoundValueKind,
    schema_type: &crate::planning::semantics::LemmaType,
    input_key: &str,
    role: ShowBoundRole,
) -> crate::result_value::RuleResultValue {
    let (fill, suggestion) = match role {
        ShowBoundRole::Value => (Some(bound), None),
        ShowBoundRole::Suggestion => (None, Some(bound)),
    };
    let display_type = crate::planning::semantics::lemma_type_with_display_binding(
        schema_type,
        None,
        fill,
        suggestion,
    );
    crate::result_value::type_scoped_result_value_from_literal(&bound.to_literal(), &display_type)
        .unwrap_or_else(|failure| {
            let role_name = match role {
                ShowBoundRole::Value => "value",
                ShowBoundRole::Suggestion => "suggestion",
            };
            panic!(
                "BUG: show {role_name} value for '{input_key}' failed type_scoped_result_value_from_literal: {}",
                crate::result_value::rule_result_value_failure_message(failure)
            )
        })
}

/// Show value of instance list `path`: its source rows in order, each with the fields
/// it sets and no `result` (a row has no value of its own to display). Literal fields
/// carry their value; referenced fields carry [`RuleResultValue::reference`]. `None`
/// when the list has no source rows.
pub(crate) fn show_instance_list_value(
    plan: &ExecutionPlan,
    path: &DataPath,
) -> Option<RuleResultValue> {
    let source = plan
        .instance_lists
        .get(path)
        .expect("BUG: instance list data without a source");
    if source.rows.is_empty() {
        return None;
    }
    let template = plan.row_template(&source.identity);
    let mut items = Vec::with_capacity(source.rows.len());
    for row in source.rows.iter() {
        let mut fields = IndexMap::with_capacity(row.fields.len());
        for (field, row_source) in &row.fields {
            let key = field.input_key();
            let value = match row_source {
                DataSource::Literal(bound) => {
                    let field_type = row_end_type(template, &RowEnd::Data(field.clone()));
                    show_bound_result_value(bound, &field_type, &key, ShowBoundRole::Value)
                }
                DataSource::Reference(relative) => {
                    let consumer =
                        crate::planning::instance_list::absolute_reference(path, relative);
                    RuleResultValue {
                        reference: Some(consumer.input_key()),
                        ..Default::default()
                    }
                }
            };
            fields.insert(key, value);
        }
        items.push(RuleResultValue {
            fields: Some(fields),
            ..Default::default()
        });
    }
    Some(RuleResultValue {
        list: Some(items),
        ..Default::default()
    })
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
    pub path: Vec<crate::planning::semantics::PathSegment>,
    pub value: Option<RuleResultValue>,
    pub suggestion: Option<RuleResultValue>,
    /// Show.rules keys that transitively need this data after normalize.
    /// Show.data only includes slots where this is non-empty.
    pub needed_by_rules: Vec<String>,
}

/// Consumer [`Engine::show`] result: local offered types, inputs rules need,
/// this spec's rule graph (local rules plus reachable imports), and resolved
/// temporal window. Source: [`Engine::source`].
#[derive(Debug, Clone, PartialEq)]
pub struct Show {
    /// Interned repository name (`specs()` string). `None` = unnamed workspace.
    pub repository: Option<String>,
    pub spec: String,
    pub commentary: Option<String>,
    pub effective_from: Option<crate::parsing::ast::DateTimeValue>,
    pub effective_to: Option<crate::parsing::ast::DateTimeValue>,
    /// Planning instant of the execution-plan slice that produced this Show.
    /// Distinguishes temporal dependency slices that share the same declared window.
    pub plan_effective: EffectiveDate,
    pub start_line: usize,
    pub source_type: Option<crate::parsing::source::SourceType>,
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
        crate::engine::write_effective_window(
            f,
            self.effective_from.as_ref(),
            self.effective_to.as_ref(),
        )?;

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
                        rule.depends_on_rules.join(", ")
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
            decimals,
            units,
            ..
        } => {
            let unit_names: Vec<&str> = units.0.iter().map(|u| u.name.as_str()).collect();
            if !unit_names.is_empty() {
                lines.push(format!("units: {}", unit_names.join(", ")));
            }
            if let Some(d) = decimals {
                lines.push(format!("decimals: {}", d));
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
            decimals,
            units,
            ..
        } => {
            let unit_names: Vec<&str> = units.0.iter().map(|u| u.name.as_str()).collect();
            if !unit_names.is_empty() {
                lines.push(format!("units: {}", unit_names.join(", ")));
            }
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
        TypeSpecification::Text {
            options, length, ..
        } => {
            if let Some(l) = length {
                lines.push(format!("length: {}", l));
            }
            if !options.is_empty() {
                let quoted: Vec<String> = options.iter().map(|o| format!("\"{}\"", o)).collect();
                lines.push(format!("options: {}", quoted.join(", ")));
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
        TypeSpecification::List { element, key } => {
            lines.push(format!("element: {element}"));
            match key {
                crate::planning::semantics::ListKey::Numbered => {
                    lines.push("key: numbered".to_string());
                }
                crate::planning::semantics::ListKey::Fields(fields) => {
                    lines.push(format!("key: {}", fields.join(", ")));
                }
            }
        }
    }
    lines
}

impl ExecutionPlan {
    /// Expression-scope unit index (local types plus direct `uses` imports).
    pub(crate) fn expression_unit_index(&self) -> &crate::planning::unit_index::UnitIndex {
        &self.resolved_types.unit_index
    }

    /// Per [`Self::data`] position: local [`Self::rules`] positions that transitively
    /// need that slot / row field (through rule references), alphabetical rule-name order.
    ///
    /// Built in plan topological order: at a rule-ref cell, OR the target rule's
    /// already-computed bitsets instead of descending Kind (rule references are
    /// evaluation boundaries). Walks the normalized body so constant-dead unless
    /// arms removed by normalize stay out of the index. Row fields land on the
    /// instance-list slot's [`NeededByRules::fields`] column.
    fn build_needed_by_rules(&self) -> (Vec<NeededByRules>, IndexMap<DataPath, Vec<u32>>) {
        use crate::planning::normalize::{LeafKind, NormalFormKind, NormalFormListOp};

        let data_len = self.data.len();
        let words = data_len.div_ceil(64);
        let mut bits_by_rule: Vec<Vec<u64>> = Vec::with_capacity(self.rules.len());
        let mut nested_hits_by_rule: Vec<HashSet<DataPath>> = Vec::with_capacity(self.rules.len());

        let mut visited = HashSet::new();
        let mut stack = Vec::new();

        // Pass 1: data-slot bitsets (topo / rule_ref OR). Row field reads ignored here.
        for (rule_pos, (_, rule)) in self.rules.iter().enumerate() {
            let mut bits = vec![0u64; words];
            let mut nested_hits: HashSet<DataPath> = HashSet::new();
            visited.clear();
            stack.push(rule.normal_form);
            while let Some(id) = stack.pop() {
                if !visited.insert(id) {
                    continue;
                }
                let nf = self.normal_form(id);
                let mut needs = NeedsMarker {
                    plan: self,
                    rule_pos,
                    bits: &mut bits,
                    nested_hits: &mut nested_hits,
                    bits_by_rule: &bits_by_rule,
                    nested_hits_by_rule: &nested_hits_by_rule,
                };
                if let Some(ref_path) = &nf.rule_ref {
                    needs.mark(ImportNeed::Rule(ref_path.clone()));
                    continue;
                }
                match &nf.kind {
                    NormalFormKind::Leaf(LeafKind::DataPath(path)) => {
                        for need in self.data_read_needs(path) {
                            needs.mark(need);
                        }
                    }
                    NormalFormKind::Leaf(LeafKind::ImportRule(path)) if *path == rule.path => {
                        for need in self.rule_needs(path) {
                            needs.mark(need);
                        }
                    }
                    NormalFormKind::Leaf(LeafKind::ImportRule(path)) => {
                        needs.mark(ImportNeed::Rule(path.clone()));
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
            nested_hits_by_rule.push(nested_hits);
        }

        // Pass 2: row-field bitsets. Self RowEnd::Rule expands via completed bits_by_rule
        // (row rule reads are not topo edges).
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
                    NormalFormKind::Leaf(LeafKind::Row { identity, end, .. }) => {
                        mark_row_end_needs(
                            self,
                            identity,
                            end,
                            &bits_by_rule,
                            &mut row_bits,
                            &rule.path,
                        );
                    }
                    NormalFormKind::SpecMember {
                        base,
                        row: Some(row_read),
                        ..
                    } => {
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
                            &row_read.end,
                            &bits_by_rule,
                            &mut row_bits,
                            &rule.path,
                        );
                        stack.push(*base);
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

        let mut consumers: Vec<usize> = self
            .rules
            .keys()
            .enumerate()
            .filter(|(_, path)| self.show_rules.contains_key(*path))
            .map(|(pos, _)| pos)
            .collect();
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
                        vec![Vec::new(); self.row_template(identity).nested_input_keys().len()]
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
            for (identity, row_bits) in &row_bits_by_rule[rule_pos] {
                let list_positions = self.instance_list_data_positions(identity);
                if list_positions.is_empty() {
                    panic!(
                        "BUG: row field needs for '{identity}' but no instance list of that type"
                    );
                }
                let template_len = self.row_template(identity).nested_input_keys().len();
                for (word_idx, &word) in row_bits.iter().enumerate() {
                    let mut remaining = word;
                    while remaining != 0 {
                        let bit = remaining.trailing_zeros() as usize;
                        let field_pos = word_idx * 64 + bit;
                        if field_pos < template_len {
                            for &list_pos in &list_positions {
                                needed_by_rules[list_pos].fields[field_pos].push(rule_id);
                            }
                        }
                        remaining &= remaining - 1;
                    }
                }
            }
        }
        let mut nested_needed: IndexMap<DataPath, Vec<u32>> = IndexMap::new();
        for &rule_pos in &consumers {
            let rule_id = u32::try_from(rule_pos).expect("BUG: rule count exceeds u32");
            let mut paths: Vec<DataPath> = nested_hits_by_rule[rule_pos].iter().cloned().collect();
            paths.sort();
            for path in paths {
                nested_needed.entry(path).or_default().push(rule_id);
            }
        }
        (needed_by_rules, nested_needed)
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
        let template = self.row_template(identity);
        let path = template
            .input_key_index
            .get(field_input_key)
            .unwrap_or_else(|| {
                panic!(
                    "BUG: row field '{field_input_key}' absent from template data of '{identity}'"
                )
            });
        let field_pos = template_bindable_field_index(template, path).unwrap_or_else(|| {
            panic!(
                "BUG: row field '{field_input_key}' path missing from template bindable fields of '{identity}'"
            )
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
    /// alias, instance list, or a named-`uses` member covered by site overlay /
    /// template default (not caller-promptable).
    ///
    /// Reference paths must appear in [`Self::reference_ends`].
    pub(crate) fn promptable_data_path<'a>(&'a self, path: &'a DataPath) -> Option<&'a DataPath> {
        match self.data.get(path) {
            Some(DataDefinition::Value { .. } | DataDefinition::TypeDeclaration { .. }) => {
                Some(path)
            }
            Some(DataDefinition::Reference { .. }) => match self.reference_ends.get(path) {
                Some(ReferenceEnd::Promptable(target)) => Some(target),
                Some(ReferenceEnd::Rule(_) | ReferenceEnd::Import) => None,
                None => {
                    panic!("BUG: reference '{path}' missing from reference_ends after planning")
                }
            },
            Some(DataDefinition::Import { .. } | DataDefinition::InstanceList { .. }) => None,
            None => {
                if let Some(end) = self.reference_ends.get(path) {
                    return match end {
                        ReferenceEnd::Promptable(target) => Some(target),
                        ReferenceEnd::Rule(_) | ReferenceEnd::Import => None,
                    };
                }
                match lookup_data_definition(&self.data, &self.row_templates, path) {
                    Some(DataDefinition::TypeDeclaration { .. } | DataDefinition::Value { .. })
                        if !self.import_overlay_binds(path) =>
                    {
                        Some(path)
                    }
                    Some(
                        DataDefinition::Value { .. }
                        | DataDefinition::TypeDeclaration { .. }
                        | DataDefinition::Reference { .. }
                        | DataDefinition::Import { .. }
                        | DataDefinition::InstanceList { .. },
                    ) => None,
                    None => panic!("BUG: normal-form DataPath leaf absent from plan.data: {path}"),
                }
            }
        }
    }

    /// Needs of evaluating `path` on this plan: local rules from
    /// [`Self::needed_by_rules`] / [`Self::nested_needed_by_rules`], named-`uses`
    /// rules through [`import_rule_needs`].
    pub(crate) fn rule_needs(&self, path: &RulePath) -> Vec<ImportNeed> {
        if !path.segments.is_empty() {
            return import_rule_needs(self, path);
        }
        let rule_pos = self.rules.get_index_of(path).unwrap_or_else(|| {
            panic!(
                "BUG: rule '{}' missing from plan '{}' while reading its needs",
                path.rule, self.spec_name
            )
        });
        let rule_id = u32::try_from(rule_pos).expect("BUG: rule count exceeds u32");
        let mut out: Vec<ImportNeed> = self
            .needed_by_rules
            .iter()
            .zip(self.data.keys())
            .filter(|(needed, _)| needed.rules.contains(&rule_id))
            .map(|(_, data_path)| ImportNeed::Data(data_path.clone()))
            .collect();
        out.extend(
            self.nested_needed_by_rules
                .iter()
                .filter(|(_, rules)| rules.contains(&rule_id))
                .map(|(nested, _)| ImportNeed::Data(nested.clone())),
        );
        out
    }

    /// Rule positions whose evaluation needs input `path`: [`Self::needed_by_rules`] for
    /// paths in [`Self::data`] (locals, projected lists), else
    /// [`Self::nested_needed_by_rules`], as [`NeedsMarker::mark`] stores them.
    pub(crate) fn data_needed_by(&self, path: &DataPath) -> &[u32] {
        match self.data.get_index_of(path) {
            Some(position) => {
                &self
                    .needed_by_rules
                    .get(position)
                    .unwrap_or_else(|| {
                        panic!(
                            "BUG: needed_by_rules len {} < data position {position}",
                            self.needed_by_rules.len()
                        )
                    })
                    .rules
            }
            None => self
                .nested_needed_by_rules
                .get(path)
                .map_or(&[], Vec::as_slice),
        }
    }

    /// Needs of reading `path` on this plan (local slot or named-`uses` member).
    pub(crate) fn data_read_needs(&self, path: &DataPath) -> Vec<ImportNeed> {
        needs_of_end(self, self.reference_end_of(path))
    }

    /// [`ReferenceEnd`] of any data path on this plan: local slots by definition and
    /// [`Self::reference_ends`], named-`uses` members by [`reference_end_on`].
    pub(crate) fn reference_end_of(&self, path: &DataPath) -> ReferenceEnd {
        if !path.segments.is_empty() {
            return reference_end_on(self, path).unwrap_or_else(|| {
                panic!(
                    "BUG: member '{path}' does not peel on plan '{}'",
                    self.spec_name
                )
            });
        }
        match self
            .data
            .get(path)
            .unwrap_or_else(|| panic!("BUG: data '{path}' missing from plan '{}'", self.spec_name))
        {
            DataDefinition::Value { .. }
            | DataDefinition::TypeDeclaration { .. }
            | DataDefinition::InstanceList { .. } => ReferenceEnd::Promptable(path.clone()),
            DataDefinition::Import { .. } => ReferenceEnd::Import,
            DataDefinition::Reference { .. } => {
                self.reference_ends.get(path).cloned().unwrap_or_else(|| {
                    panic!("BUG: reference '{path}' missing from reference_ends")
                })
            }
        }
    }

    /// Whether a site overlay on a leading Import hop binds `path`'s relative member.
    pub(crate) fn import_overlay_binds(&self, path: &DataPath) -> bool {
        let mut hops = path.segments.iter();
        let Some(first) = hops.next() else {
            return false;
        };
        let import_marker = DataPath::local(first.uses.clone());
        let Some(DataDefinition::Import { overlay, .. }) = self.data.get(&import_marker) else {
            return false;
        };
        let relative = DataPath {
            segments: hops.cloned().collect(),
            data: path.data.clone(),
        };
        overlay.contains_key(&relative)
    }

    /// Validate caller-requested rule names and return resolved [`RulePath`]s.
    ///
    /// `None` means all local rules (this spec's outputs). `Some(&[])` is an error.
    /// Unknown names in `Some` slice error. Explicit names may be any Show.rules key.
    pub fn validated_response_rule_paths(
        &self,
        rules: Option<&[String]>,
    ) -> Result<std::collections::HashSet<&RulePath>, Error> {
        let Some(rules) = rules else {
            return Ok(self
                .rules
                .values()
                .filter(|r| r.path.segments.is_empty())
                .map(|r| &r.path)
                .collect());
        };
        if rules.is_empty() {
            return Err(Error::request(
                "at least one rule required".to_string(),
                None::<String>,
            ));
        }
        let mut paths = std::collections::HashSet::new();
        for rule_name in rules {
            let rule = self.get_rule(rule_name).ok_or_else(|| {
                Error::request(
                    format!("Rule '{rule_name}' not found in spec '{}'", self.spec_name),
                    None::<String>,
                )
            })?;
            paths.insert(&rule.path);
        }
        Ok(paths)
    }

    /// Look up a Show.rules key (`RulePath::input_key`) in the reachable graph.
    pub fn get_rule(&self, name: &str) -> Option<&ExecutableRule> {
        let canonical = crate::parsing::ast::canonical_logical_name(name);
        let path = self.show_rule_index.get(canonical.as_ref())?;
        self.rules.get(path)
    }

    /// Look up a normal-form cell by id.
    pub(crate) fn normal_form(&self, id: NormalFormId) -> &NormalForm {
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

    /// Data paths a caller can be prompted for, in declaration order.
    ///
    /// A path is promptable when it carries its own value slot: [`DataDefinition::Value`]
    /// (spec prefill, overridable) or [`DataDefinition::TypeDeclaration`] (typed input).
    /// [`DataDefinition::Reference`] paths are not promptable — a data target copies from
    /// its ultimate target (see [`ExecutionPlan::reference_ends`]) and a rule target is
    /// computed — and [`DataDefinition::Import`] paths are owned by the imported spec.
    ///
    /// Domain of keys that may appear in `RuleResult.missing_data` (as a set).
    /// Evaluation decides which unbound members are live and in what order; this
    /// iterator does not fix list order.
    pub(crate) fn promptable_data_paths(&self) -> impl Iterator<Item = &DataPath> {
        self.data
            .iter()
            .filter_map(|(path, definition)| match definition {
                DataDefinition::Value { .. } | DataDefinition::TypeDeclaration { .. } => Some(path),
                DataDefinition::Reference { .. }
                | DataDefinition::Import { .. }
                | DataDefinition::InstanceList { .. } => None,
            })
    }
}

/// Follow an [`NormalFormKind::OrderedDispatch`] region to the origin arm body it selects,
/// using filled slots: a nested dispatch by its scrutinee slot, a conjunctive region by
/// its residual arms. Stops at rule-reference cells: those borrow another rule's Kind and
/// must compare as the origin arm body, not the body that rule would itself select.
/// `None` when a slot the walk reads is unset or vetoed (not decided yet).
pub(crate) fn resolve_nested_dispatch_body(
    plan: &ExecutionPlan,
    values: &[Option<crate::computation::OperationResult>],
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
            | NormalFormKind::Sum(_)
            | NormalFormKind::Product(_)
            | NormalFormKind::Subtract(_, _)
            | NormalFormKind::Divide(_, _)
            | NormalFormKind::Power(_, _)
            | NormalFormKind::Modulo(_, _)
            | NormalFormKind::Negate(_)
            | NormalFormKind::Reciprocal(_)
            | NormalFormKind::Comparison(_, _, _)
            | NormalFormKind::And(_)
            | NormalFormKind::Not(_)
            | NormalFormKind::MathOp(_, _)
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
    values: &[Option<crate::computation::OperationResult>],
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
/// - [`NormalFormKind::And`]: a filled `false` operand decides the conjunction, so it
///   is the only live operand; a missing sibling is never reported. Otherwise every
///   operand not settled `true` is live, since a later `false` can still decide.
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
    values: &[Option<crate::computation::OperationResult>],
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
            NormalFormKind::Leaf(LeafKind::DataPath(path)) => {
                if seen_paths.insert(path.clone()) {
                    out.push(ReachableInput::Data(path.clone()));
                }
                Vec::new()
            }
            NormalFormKind::Leaf(LeafKind::ImportRule(path)) => {
                out.push(ReachableInput::ImportRule(path.clone()));
                Vec::new()
            }
            NormalFormKind::Leaf(LeafKind::Row { .. }) => Vec::new(),
            NormalFormKind::ListCommand { row_level, .. } => {
                if row_level.is_some() {
                    out.push(ReachableInput::RowReads(id));
                }
                nf.kind.children()
            }
            NormalFormKind::And(children) => {
                match children.iter().copied().find(|&child| {
                    matches!(slot_outcome(values, child), Some(BranchOutcome::NotTaken))
                }) {
                    Some(decider) => vec![decider],
                    None => children
                        .iter()
                        .copied()
                        .filter(|&child| {
                            !matches!(slot_outcome(values, child), Some(BranchOutcome::Taken))
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
                    Some(OperationResult::Veto(_)) => vec![*scrutinee],
                    // Origin Piecewise conditions are not filled under the dispatch
                    // value walk, so an open scrutinee keeps every origin arm live.
                    None => {
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
            NormalFormKind::SpecMember {
                base, targets, row, ..
            } => {
                if row.is_some() {
                    out.push(ReachableInput::RowReads(id));
                }
                let chosen = match values.get(base.index()).and_then(|slot| slot.as_ref()) {
                    Some(OperationResult::Value(bound)) => match &bound.value {
                        crate::planning::semantics::ValueKind::Spec(
                            crate::planning::semantics::SpecValue::Fixed(instance),
                        ) => targets
                            .iter()
                            .find(|(candidate, _)| candidate == instance)
                            .map(|(_, target)| *target),
                        _ => None,
                    },
                    Some(OperationResult::Veto(_)) | None => None,
                };
                match chosen {
                    Some(target) => vec![*base, target],
                    None => vec![*base],
                }
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
    /// A named-`uses` rule: its inputs follow the template's own walk under the
    /// import context (see `EvaluationContext::missing_data_for_rule`).
    ImportRule(RulePath),
}

pub(crate) fn validate_value_against_type(
    expected_type: &LemmaType,
    value: &LiteralValue,
    unit_index: &crate::planning::unit_index::UnitIndex,
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
            _ => validate_scalar_against_type(expected, &value)?,
        }
    }
    Ok(())
}

fn validate_scalar_against_type(
    expected_type: &LemmaType,
    value: &LiteralValue,
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
        expected_type: &crate::planning::semantics::LemmaType,
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
                        "{} exceeds decimals constraint {d}",
                        n.display_str()
                    ));
                }
            }
            if let Some(min) = minimum {
                if n.try_cmp(min)
                    .map_err(|failure| format!("minimum bound compare failed: {failure}"))?
                    == Ordering::Less
                {
                    return Err(format!(
                        "{} is below minimum {}",
                        format_rational_for_validation_message(expected_type, n),
                        format_rational_for_validation_message(expected_type, min)
                    ));
                }
            }
            if let Some(max) = maximum {
                if n.try_cmp(max)
                    .map_err(|failure| format!("maximum bound compare failed: {failure}"))?
                    == Ordering::Greater
                {
                    return Err(format!(
                        "{} is above maximum {}",
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
                decimals,
                units,
                ..
            },
            ValueKind::Measure(magnitude),
        ) => {
            use crate::computation::rational::checked_div;
            use crate::planning::semantics::measure_declared_bound_to_canonical;
            use std::cmp::Ordering;
            let unit = expected_type
                .unit()
                .or_else(|| {
                    units
                        .iter()
                        .find(|u| u.is_canonical_factor())
                        .or_else(|| units.iter().next())
                        .map(|u| u.name.as_str())
                })
                .ok_or_else(|| {
                    format!(
                        "measure type '{}' has no declared units for validation",
                        expected_type.name()
                    )
                })?;
            let measure_unit = units.get(unit)?;
            let factor = &measure_unit.factor;
            let in_unit = checked_div(magnitude, factor).map_err(|failure| {
                format!("cannot de-canonicalize measure for validation: {failure}")
            })?;
            if let Some(d) = decimals {
                if exceeds_decimal_places(&in_unit, *d) {
                    return Err(format!(
                        "{} {unit} exceeds decimals constraint {d}",
                        in_unit.display_str()
                    ));
                }
            }
            if let Some(bound) = minimum {
                let canonical_min = measure_declared_bound_to_canonical(
                    &bound.0,
                    &bound.1,
                    units,
                    expected_type.name().as_str(),
                    "minimum",
                )?;
                if magnitude
                    .try_cmp(&canonical_min)
                    .map_err(|failure| format!("minimum bound compare failed: {failure}"))?
                    == Ordering::Less
                {
                    let min_in_unit = checked_div(&canonical_min, factor).map_err(|failure| {
                        format!("cannot de-canonicalize minimum for validation: {failure}")
                    })?;
                    let value_display = format!(
                        "{} {}",
                        format_rational_for_validation_message(expected_type, &in_unit),
                        unit
                    );
                    let bound_display = format!(
                        "{} {}",
                        format_rational_for_validation_message(expected_type, &min_in_unit),
                        measure_unit.name
                    );
                    return Err(format!("{value_display} is below minimum {bound_display}"));
                }
            }
            if let Some(bound) = maximum {
                let canonical_max = measure_declared_bound_to_canonical(
                    &bound.0,
                    &bound.1,
                    units,
                    expected_type.name().as_str(),
                    "maximum",
                )?;
                if magnitude
                    .try_cmp(&canonical_max)
                    .map_err(|failure| format!("maximum bound compare failed: {failure}"))?
                    == Ordering::Greater
                {
                    let max_in_unit = checked_div(&canonical_max, factor).map_err(|failure| {
                        format!("cannot de-canonicalize maximum for validation: {failure}")
                    })?;
                    let value_display = format!(
                        "{} {}",
                        format_rational_for_validation_message(expected_type, &in_unit),
                        unit
                    );
                    let bound_display = format!(
                        "{} {}",
                        format_rational_for_validation_message(expected_type, &max_in_unit),
                        measure_unit.name
                    );
                    return Err(format!("{value_display} is above maximum {bound_display}"));
                }
            }
            Ok(())
        }
        (
            TypeSpecification::Text {
                length, options, ..
            },
            ValueKind::Text(s),
        ) => {
            let len = s.chars().count();
            if let Some(exact) = length {
                if len != *exact {
                    return Err(format!(
                        "'{}' has length {} but required length is {}",
                        s, len, exact
                    ));
                }
            }
            if !options.is_empty() && !options.iter().any(|opt| opt == s) {
                return Err(format!(
                    "'{}' is not in allowed options: {}",
                    s,
                    options.join(", ")
                ));
            }
            Ok(())
        }
        (
            TypeSpecification::Ratio {
                minimum,
                maximum,
                decimals,
                units,
                ..
            },
            ValueKind::Ratio(r),
        ) => {
            use crate::computation::rational::checked_mul;
            use std::cmp::Ordering;

            let primary_unit = expected_type
                .unit()
                .or_else(|| expected_type.ratio_primary_unit());

            if let Some(d) = decimals {
                let magnitude_for_decimals = match primary_unit {
                    Some(unit) => {
                        let ratio_unit = units.get(unit)?;
                        checked_mul(r, &ratio_unit.value).map_err(|failure| failure.to_string())?
                    }
                    None => r.clone(),
                };
                if exceeds_decimal_places(&magnitude_for_decimals, *d) {
                    return Err(format!(
                        "{} exceeds decimals constraint {d}",
                        magnitude_for_decimals.display_str()
                    ));
                }
            }
            if let Some(type_minimum) = minimum {
                if r.try_cmp(type_minimum)
                    .map_err(|failure| format!("minimum bound compare failed: {failure}"))?
                    == Ordering::Less
                {
                    let message = match primary_unit {
                        Some(unit) => {
                            let ratio_unit = units.get(unit)?;
                            let value_per_unit = checked_mul(r, &ratio_unit.value)
                                .map_err(|failure| failure.to_string())?;
                            let bound_per_unit = ratio_unit.minimum.clone().expect(
                                "BUG: RatioUnit.minimum missing after type minimum set by sync_ratio_units_from_canonical",
                            );
                            format!(
                                "{} {unit} is below minimum {} {unit}",
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
                            "{} is below minimum {}",
                            format_rational_for_validation_message(expected_type, r),
                            format_rational_for_validation_message(expected_type, type_minimum),
                        ),
                    };
                    return Err(message);
                }
            }
            if let Some(type_maximum) = maximum {
                if r.try_cmp(type_maximum)
                    .map_err(|failure| format!("maximum bound compare failed: {failure}"))?
                    == Ordering::Greater
                {
                    let message = match primary_unit {
                        Some(unit) => {
                            let ratio_unit = units.get(unit)?;
                            let value_per_unit = checked_mul(r, &ratio_unit.value)
                                .map_err(|failure| failure.to_string())?;
                            let bound_per_unit = ratio_unit.maximum.clone().expect(
                                "BUG: RatioUnit.maximum missing after type maximum set by sync_ratio_units_from_canonical",
                            );
                            format!(
                                "{} {unit} is above maximum {} {unit}",
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
                            "{} is above maximum {}",
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
            TypeSpecification::Ratio {
                minimum,
                maximum,
                decimals,
                units: _,
                ..
            },
            ValueKind::Number(n),
        ) => {
            use std::cmp::Ordering;
            if let Some(d) = decimals {
                if exceeds_decimal_places(n, *d) {
                    return Err(format!(
                        "{} exceeds decimals constraint {d}",
                        n.display_str()
                    ));
                }
            }
            if let Some(type_minimum) = minimum {
                if n.try_cmp(type_minimum)
                    .map_err(|failure| format!("minimum bound compare failed: {failure}"))?
                    == Ordering::Less
                {
                    return Err(format!(
                        "{} is below minimum {}",
                        format_rational_for_validation_message(expected_type, n),
                        format_rational_for_validation_message(expected_type, type_minimum)
                    ));
                }
            }
            if let Some(type_maximum) = maximum {
                if n.try_cmp(type_maximum)
                    .map_err(|failure| format!("maximum bound compare failed: {failure}"))?
                    == Ordering::Greater
                {
                    return Err(format!(
                        "{} is above maximum {}",
                        format_rational_for_validation_message(expected_type, n),
                        format_rational_for_validation_message(expected_type, type_maximum)
                    ));
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
                    return Err(format!("{} is below minimum {}", dt, min));
                }
            }
            if let Some(max) = maximum {
                let max_sem = date_time_to_semantic(max);
                if compare_semantic_dates(dt, &max_sem) == Ordering::Greater {
                    return Err(format!("{} is above maximum {}", dt, max));
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
                    return Err(format!("{} is below minimum {}", t, min));
                }
            }
            if let Some(max) = maximum {
                let max_sem = time_to_semantic(max);
                if compare_semantic_times(t, &max_sem) == Ordering::Greater {
                    return Err(format!("{} is above maximum {}", t, max));
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
    left: &crate::planning::semantics::TypedLiteral,
    right: &crate::planning::semantics::TypedLiteral,
    unit_index: &crate::planning::unit_index::UnitIndex,
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
    validate_scalar_against_type(element_type.as_ref(), &left.to_literal())?;
    validate_scalar_against_type(element_type.as_ref(), &right.to_literal())?;

    let ordering = match (&left.value, &right.value) {
        (ValueKind::Number(l), ValueKind::Number(r)) => l
            .try_cmp(r)
            .map_err(|failure| format!("range endpoint compare failed: {failure}"))?,
        (ValueKind::Date(l), ValueKind::Date(r)) => compare_semantic_dates(l, r),
        (ValueKind::Time(l), ValueKind::Time(r)) => compare_semantic_times(l, r),
        (ValueKind::Ratio(l), ValueKind::Ratio(r)) => l
            .try_cmp(r)
            .map_err(|failure| format!("range endpoint compare failed: {failure}"))?,
        (ValueKind::Measure(l), ValueKind::Measure(r)) => l
            .try_cmp(r)
            .map_err(|failure| format!("range endpoint compare failed: {failure}"))?,
        (left_kind, right_kind) => unreachable!(
            "BUG: range endpoints have mismatched value kinds after typing: {left_kind:?} vs {right_kind:?}"
        ),
    };
    if ordering == Ordering::Greater {
        return Err(format!(
            "range left endpoint {} is above right endpoint {}",
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
                    crate::planning::semantics::primitive_number_arc(),
                    ComparisonComputation::GreaterThanOrEqual,
                    format!("span is below minimum width {}", min_w.display_str()),
                )?;
            }
            if let Some(max_w) = maximum {
                compare_width(
                    &LiteralValue::number(max_w.clone()),
                    crate::planning::semantics::primitive_number_arc(),
                    ComparisonComputation::LessThanOrEqual,
                    format!("span is above maximum width {}", max_w.display_str()),
                )?;
            }
        }
        TypeSpecification::RatioRange {
            minimum, maximum, ..
        } => {
            if let Some(min_w) = minimum {
                compare_width(
                    &LiteralValue::ratio(min_w.clone()),
                    crate::planning::semantics::primitive_ratio_arc(),
                    ComparisonComputation::GreaterThanOrEqual,
                    format!("span is below minimum width {}", min_w.display_str()),
                )?;
            }
            if let Some(max_w) = maximum {
                compare_width(
                    &LiteralValue::ratio(max_w.clone()),
                    crate::planning::semantics::primitive_ratio_arc(),
                    ComparisonComputation::LessThanOrEqual,
                    format!("span is above maximum width {}", max_w.display_str()),
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
                        "span is below minimum width {} {}",
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
                        "span is above maximum width {} {}",
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
            let resolve_bound =
                |bound: &(crate::computation::rational::RationalInteger, String),
                 command: &str|
                 -> Result<(LiteralValue, Arc<LemmaType>), String> {
                    let (bare, owner) = unit_index
                        .resolve(bound.1.as_str())
                        .map_err(|err| format!("{command} width unit '{}': {err}", bound.1))?;
                    if allow_calendar {
                        if !owner.is_duration_like() && !owner.is_calendar_like() {
                            return Err(format!(
                                "{command} width unit '{bare}' must be a duration or calendar unit",
                            ));
                        }
                    } else if !owner.is_duration_like() {
                        return Err(format!(
                            "{command} width unit '{bare}' must be a duration unit",
                        ));
                    }
                    let TypeSpecification::Measure { units, .. } = &owner.specifications else {
                        return Err(format!(
                            "{command} width unit '{bare}' must resolve to a measure type",
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
                        "span is below minimum width {} {}",
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
                        "span is above maximum width {} {}",
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

fn validate_literal_data_against_types(plan: &ExecutionPlan) -> Vec<Error> {
    let mut errors = Vec::new();

    for (data_path, data_definition) in &plan.data {
        let (expected_type, lit) = match data_definition {
            DataDefinition::Value {
                value,
                resolved_type,
                ..
            } => (resolved_type, value),
            DataDefinition::TypeDeclaration { .. }
            | DataDefinition::Import { .. }
            | DataDefinition::InstanceList { .. }
            | DataDefinition::Reference { .. } => continue,
        };

        if let Err(msg) =
            validate_value_against_type(expected_type, lit, plan.expression_unit_index())
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
            if let Some((unit_name, owning_type)) =
                crate::computation::units::conversion_target_declares_unit(target)
            {
                if !crate::computation::units::owning_type_declares_unit_name(
                    owning_type.as_ref(),
                    unit_name,
                ) {
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
    use crate::computation::rational::{rational_new, rational_zero};
    use crate::computation::{OperationResult, VetoType};
    use crate::evaluation::evaluate_data::EvaluateData;
    use crate::literals::DateGranularity;
    use crate::literals::TimezoneValue;
    use crate::parsing::ast::DateTimeValue;
    use crate::planning::semantics::{DataDefinition, DataPath, PathSegment, TypeSpecification};
    use crate::Engine;
    use crate::{EvaluateDataValue, ResourceLimits};
    use serde_json;
    use std::str::FromStr;
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

    fn veto_reason<'a>(evaluate_data: &'a EvaluateData, path: &DataPath) -> Option<&'a str> {
        match evaluate_data.bindings.get(path) {
            Some(OperationResult::Veto(veto)) => Some(match veto {
                VetoType::Computation { message } => message.as_str(),
                other => panic!("expected Computation veto, got {other:?}"),
            }),
            _ => None,
        }
    }

    fn bound_value<'a>(
        evaluate_data: &'a EvaluateData,
        path: &DataPath,
    ) -> Option<&'a crate::planning::semantics::BoundValueKind> {
        evaluate_data
            .bindings
            .get(path)
            .and_then(OperationResult::value)
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
        let updated_value = bound_value(&evaluate_data, &data_path).expect("bound value");
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
        match veto_reason(&evaluate_data, &data_path) {
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
        let data_path = DataPath {
            segments: vec![PathSegment {
                uses: "rules".to_string(),
                repository: None,
                spec: "private".to_string(),
            }],
            data: "base_price".to_string(),
        };
        let updated_value = bound_value(&evaluate_data, &data_path).expect("bound value");
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
            nested_needed_by_rules: IndexMap::new(),
            data_display: IndexMap::new(),
            show_rules: IndexMap::new(),
            reference_ends: IndexMap::new(),
            input_key_index,
            show_rule_index: IndexMap::new(),
            data_leaf: IndexMap::new(),
            row_templates: std::collections::BTreeMap::new(),
            planning_input: Arc::new(Vec::new()),
            identity: SpecIdentity {
                repository: None,
                spec: "test".to_string(),
                effective: EffectiveDate::Origin,
            },
            rule_candidates: std::collections::BTreeMap::new(),
            self_row_rules: Vec::new(),
            instance_lists: IndexMap::new(),
            row_mask: Vec::new(),
        };

        let values = input_data(&[("x", "11")]);

        let evaluate_data = resolve_evaluate_data(&plan, values);
        match veto_reason(&evaluate_data, &data_path) {
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

        let tier = crate::planning::semantics::LemmaType::primitive(
            crate::planning::semantics::TypeSpecification::Text {
                length: None,
                options: vec!["silver".to_string(), "gold".to_string()],
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
                value: crate::planning::semantics::LiteralValue::text("silver".to_string()),
                resolved_type: Arc::new(tier.clone()),
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
            nested_needed_by_rules: IndexMap::new(),
            data_display: IndexMap::new(),
            show_rules: IndexMap::new(),
            reference_ends: IndexMap::new(),
            input_key_index,
            show_rule_index: IndexMap::new(),
            data_leaf: IndexMap::new(),
            row_templates: std::collections::BTreeMap::new(),
            planning_input: Arc::new(Vec::new()),
            identity: SpecIdentity {
                repository: None,
                spec: "test".to_string(),
                effective: EffectiveDate::Origin,
            },
            rule_candidates: std::collections::BTreeMap::new(),
            self_row_rules: Vec::new(),
            instance_lists: IndexMap::new(),
            row_mask: Vec::new(),
        };

        let values = input_data(&[("tier", "platinum")]);

        let evaluate_data = resolve_evaluate_data(&plan, values);
        match veto_reason(&evaluate_data, &data_path) {
            Some(reason) => {
                assert!(
                    reason.contains("allowed options") || reason.contains("platinum"),
                    "invalid enum must record violation, got: {reason}"
                );
            }
            None => panic!("expected veto-bound data for tier=platinum"),
        }
    }

    #[test]
    fn evaluate_data_should_enforce_measure_decimals() {
        // Higher-standard requirement: decimals should be enforced on measure inputs,
        // unless the language explicitly defines rounding semantics.
        let data_path = DataPath::new(vec![], "price".to_string());

        let money = crate::planning::semantics::LemmaType::primitive(
            crate::planning::semantics::TypeSpecification::Measure {
                minimum: None,
                maximum: None,
                decimals: Some(2),
                units: crate::planning::semantics::MeasureUnits::from(vec![
                    crate::planning::semantics::MeasureUnit::from_decimal_factor(
                        "eur".to_string(),
                        rust_decimal::Decimal::from_str("1.0").unwrap(),
                        Vec::new(),
                    )
                    .expect("eur unit factor must be exact decimal"),
                ]),
                traits: Vec::new(),
                decomposition: None,
                unit: None,
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
                value: crate::planning::semantics::LiteralValue::measure(rational_zero()),
                resolved_type: Arc::new(money.clone()),
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
            nested_needed_by_rules: IndexMap::new(),
            data_display: IndexMap::new(),
            show_rules: IndexMap::new(),
            reference_ends: IndexMap::new(),
            input_key_index,
            show_rule_index: IndexMap::new(),
            data_leaf: IndexMap::new(),
            row_templates: std::collections::BTreeMap::new(),
            planning_input: Arc::new(Vec::new()),
            identity: SpecIdentity {
                repository: None,
                spec: "test".to_string(),
                effective: EffectiveDate::Origin,
            },
            rule_candidates: std::collections::BTreeMap::new(),
            self_row_rules: Vec::new(),
            instance_lists: IndexMap::new(),
            row_mask: Vec::new(),
        };

        let values = input_data(&[("price", "1.234 eur")]);

        let evaluate_data = resolve_evaluate_data(&plan, values);
        match veto_reason(&evaluate_data, &data_path) {
            Some(reason) => {
                assert!(
                    reason.contains("decimals") || reason.contains("decimal"),
                    "1.234 eur must violate decimals=2, got: {reason}"
                );
            }
            None => panic!("expected veto-bound data for price=1.234 eur"),
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
            nested_needed_by_rules: IndexMap::new(),
            data_display: IndexMap::new(),
            show_rules: IndexMap::new(),
            reference_ends: IndexMap::new(),
            input_key_index: IndexMap::new(),
            show_rule_index: IndexMap::new(),
            data_leaf: IndexMap::new(),
            row_templates: std::collections::BTreeMap::new(),
            planning_input: Arc::new(Vec::new()),
            identity: SpecIdentity {
                repository: None,
                spec: "s".to_string(),
                effective: EffectiveDate::Origin,
            },
            rule_candidates: std::collections::BTreeMap::new(),
            self_row_rules: Vec::new(),
            instance_lists: IndexMap::new(),
            row_mask: Vec::new(),
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
  -> decimals 2

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
            evaluate_data
                .bindings
                .get(&DataPath::local("labor_cost".into())),
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
    fn evaluate_prunes_inactive_nut_branches_for_total_price() {
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
data roasting: price_per_weight
data chocolatizing: price_per_weight

rule total_price: weight * (item_cost + roasting + chocolatizing)

spec calc
uses bag
  -> with item_cost: item_cost
  -> with roasting: roasting

data type_of_nut: text -> options "peanut", "cashew"

rule price_peanut: 1.5 eur_per_kg
rule price_peanut_roasting: 0.45 eur_per_kg

rule price_cashew: 2.0 eur_per_kg
rule price_cashew_roasting: 0.55 eur_per_kg

rule item_cost: veto "No item cost"
  unless type_of_nut is "peanut" then price_peanut
  unless type_of_nut is "cashew" then price_cashew

rule roasting: veto "No roasting"
  unless type_of_nut is "peanut" then price_peanut_roasting
  unless type_of_nut is "cashew" then price_cashew_roasting

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
        inputs.insert("type_of_nut".to_string(), "peanut".into());
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
            vec!["bag.weight".to_string(), "bag.chocolatizing".to_string(),],
            "supplied type_of_nut and with-bound roasting/item_cost omitted"
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
  -> decimals 2
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
  * ceil (storage_duration as week as number)

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
            display, "20.00 eur",
            "10 eur/week * ceil(10 day as week) / 1 CE must be 20.00 eur, got: {display}"
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

    #[test]
    fn input_key_index_matches_input_key_entries_for_nested_plan() {
        let (_, plan) = nested_import_plan();

        let mut entry_keys: Vec<String> = plan
            .input_key_entries()
            .into_iter()
            .map(|(path, _)| path.input_key())
            .collect();
        let mut index_keys: Vec<String> = plan.input_key_index.keys().cloned().collect();
        entry_keys.sort();
        index_keys.sort();
        assert_eq!(entry_keys, index_keys);

        let show_keys: Vec<String> = plan
            .nested_input_keys()
            .into_iter()
            .map(|(path, _)| path.input_key())
            .collect();
        assert!(!show_keys.iter().any(|key| key == "m"), "{show_keys:?}");
        assert!(entry_keys.iter().any(|key| key == "m"), "{entry_keys:?}");
        assert!(
            show_keys.len() < entry_keys.len(),
            "{show_keys:?} vs {entry_keys:?}"
        );
    }

    #[test]
    fn peel_data_local_one_hop_two_hops_and_non_import() {
        let (_, plan) = nested_import_plan();

        let local = DataPath::local("m".into());
        let (_, path, def) = plan
            .peel_data(&local)
            .expect("local import marker peels on outer");
        assert_eq!(path.data, "m");
        assert!(matches!(def, DataDefinition::Import { .. }));

        let one = DataPath {
            segments: vec![PathSegment {
                uses: "m".into(),
                repository: None,
                spec: "mid".into(),
            }],
            data: "y".into(),
        };
        let (body, path, def) = plan.peel_data(&one).expect("one hop");
        assert_eq!(body.identity.spec, "mid");
        assert_eq!(path.data, "y");
        assert!(matches!(
            def,
            DataDefinition::Value { .. } | DataDefinition::TypeDeclaration { .. }
        ));

        let two = DataPath {
            segments: vec![
                PathSegment {
                    uses: "m".into(),
                    repository: None,
                    spec: "mid".into(),
                },
                PathSegment {
                    uses: "nested".into(),
                    repository: None,
                    spec: "leaf".into(),
                },
            ],
            data: "x".into(),
        };
        let (body, path, def) = plan.peel_data(&two).expect("two hops");
        assert_eq!(body.identity.spec, "leaf");
        assert_eq!(path.data, "x");
        assert!(matches!(
            def,
            DataDefinition::Value { .. } | DataDefinition::TypeDeclaration { .. }
        ));

        let bad = DataPath {
            segments: vec![PathSegment {
                uses: "y".into(),
                repository: None,
                spec: "mid".into(),
            }],
            data: "x".into(),
        };
        assert!(plan.peel_data(&bad).is_none());

        let (mid, _, _) = plan.peel_data(&one).expect("mid for non-import hop");
        let non_import_hop = DataPath {
            segments: vec![PathSegment {
                uses: "y".into(),
                repository: None,
                spec: "mid".into(),
            }],
            data: "x".into(),
        };
        assert!(mid.peel_data(&non_import_hop).is_none());
    }

    #[test]
    fn peel_rule_local_one_hop_two_hops() {
        let (_, plan) = nested_import_plan();

        let one = RulePath {
            segments: vec![PathSegment {
                uses: "m".into(),
                repository: None,
                spec: "mid".into(),
            }],
            rule: "ry".into(),
        };
        let (body, local) = plan.peel_rule(&one).expect("one hop rule");
        assert_eq!(body.identity.spec, "mid");
        assert_eq!(local.rule, "ry");
        assert!(local.segments.is_empty());

        let two = RulePath {
            segments: vec![
                PathSegment {
                    uses: "m".into(),
                    repository: None,
                    spec: "mid".into(),
                },
                PathSegment {
                    uses: "nested".into(),
                    repository: None,
                    spec: "leaf".into(),
                },
            ],
            rule: "rx".into(),
        };
        let (body, local) = plan.peel_rule(&two).expect("two hop rule");
        assert_eq!(body.identity.spec, "leaf");
        assert_eq!(local.rule, "rx");
    }

    #[test]
    fn qualify_data_names_returns_qualified_path_that_peels() {
        let (_, plan) = nested_import_plan();
        let qualified = plan
            .qualify_data_names(&["m", "nested", "x"])
            .expect("qualified path");
        assert_eq!(qualified.input_key(), "m.nested.x");
        assert_eq!(qualified.segments.len(), 2);
        assert_eq!(qualified.data, "x");
        let (body, _, _) = plan.peel_data(&qualified).expect("qualified path peels");
        assert_eq!(body.identity.spec, "leaf");
    }
}

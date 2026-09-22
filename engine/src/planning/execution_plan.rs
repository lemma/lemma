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
use crate::planning::graph::Graph;
use crate::planning::graph::ResolvedSpecTypes;
use crate::planning::normalize::{
    data_path_result_type, NormalForm, NormalFormId, NormalFormInterner, NormalizeContext,
    NormalizedRule,
};
use crate::planning::semantics::{
    value_kind_matches_spec, ComparisonComputation, DataDefinition, DataPath, LemmaType,
    LiteralValue, ReferenceEnd, ReferenceTarget, RulePath, TypeSpecification, ValueKind,
};
use crate::planning::show_expression::{project_depends_on_rules, show_branches_from};
use crate::planning::spec_set::LemmaSpecSet;
use crate::planning::unit_family::FamilyUnitCatalog;
use crate::result_value::RuleResultValue;
use crate::Error;
use indexmap::{IndexMap, IndexSet};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

pub use crate::planning::show_expression::{
    ShowBranch, ShowConversionTarget, ShowExpression, ShowRule,
};

/// A complete execution plan ready for the evaluator
///
/// Contains drift-free normal-form equations in a table, named rule roots, and all data.
/// Self-contained structure - no spec lookups required during evaluation.
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

    /// All loaded temporal versions for this spec name (same list on every slice).
    /// Shared via [`Arc`] so every slice of a set points at one allocation.
    /// Filled by [`attach_show_cache`].
    pub versions: Arc<[ShowVersion]>,

    /// Source start line of the LemmaSpec version this plan was built from.
    pub start_line: usize,

    /// Source type of the LemmaSpec version this plan was built from.
    pub source_type: Option<SourceType>,

    /// Per [`Self::data`] position: positions in [`Self::rules`] of local rules that
    /// transitively need that slot (through rule references), in alphabetical rule-name order.
    /// Non-promptable slots (references, imports) have empty lists: leaves are mapped to
    /// their promptable target before recording. `len() == data.len()`.
    /// Built in [`build_execution_plan`].
    pub(crate) needed_by_rules: Vec<Vec<u32>>,

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

    /// Precomputed `input_key` → `DataPath` index for [`RunData::resolve`].
    /// Built once at plan time so every `Engine::run` call avoids rebuilding it.
    pub(crate) input_key_index: IndexMap<String, DataPath>,

    /// Precomputed `input_key` → `RulePath` for [`Self::show_rules`].
    /// Built once so [`Self::get_rule`] is O(1) without allocating per candidate.
    pub(crate) show_rule_index: IndexMap<String, RulePath>,

    /// Every [`Self::data`] path → its [`LeafKind::DataPath`] cell in
    /// [`Self::normal_forms`]. Built once so evaluation writes bindings into
    /// the value table by [`NormalFormId`] instead of a parallel path map.
    pub(crate) data_leaf: IndexMap<DataPath, NormalFormId>,
}

/// Plan-time prefill/suggestion for one data path (show cache).
///
/// Stores domain [`BoundValueKind`] — postcard-safe. [`RuleResultValue`] is built at
/// show time (JSON / API boundary), not snapshotted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ShowDataCache {
    pub fill: Option<crate::planning::semantics::BoundValueKind>,
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
    plans: &'a BTreeMap<EffectiveDate, ExecutionPlan>,
    instant: &EffectiveDate,
) -> Option<&'a ExecutionPlan> {
    plans
        .range(..=instant.clone())
        .next_back()
        .map(|(_, plan)| plan)
}

/// Builds an execution plan from a Graph for one temporal slice.
///
/// `interner` is shared across every slice of the enclosing spec set for one
/// planning pass: cons keys are span-insensitive, so identical subexpressions
/// in different slices share cells. Each slice still extracts its own dense
/// `normal_forms` table via [`NormalFormInterner::extract_reachable`].
pub(crate) fn build_execution_plan(
    graph: &Graph<'_>,
    resolved_types: ResolvedSpecTypes,
    effective: &EffectiveDate,
    limits: &crate::limits::ResourceLimits,
    interner: &mut NormalFormInterner,
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
            if !reachable.contains(dep) {
                stack.push(dep.clone());
            }
        }
    }
    let show_keys: HashSet<String> = reachable.iter().map(|path| path.input_key()).collect();

    for rule_path in rule_order {
        let rule_node = graph.rules().get(rule_path).expect(
            "bug: rule from topological sort not in graph - validation should have caught this",
        );

        let measure_scope = crate::planning::typing::MeasureScope {
            unit_index: &resolved_types.unit_index,
            signature_index: &resolved_types.signature_index,
        };
        let normalize_ctx = NormalizeContext {
            data: &data,
            measure_scope,
            max_normalized_expression_nodes: limits.max_normalized_expression_nodes,
            max_normal_form_depth: limits.max_normal_form_depth,
        };
        let normalized = crate::planning::normalize::build_normalized_rule(
            &normalize_ctx,
            &completed_rules,
            &rule_types,
            reference_ends,
            &rule_node.branches,
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

        if reachable.contains(rule_path) {
            show_rules.insert(
                rule_path.clone(),
                ShowRule {
                    lemma_type: family_units.rule_type_for_show(rule_node.rule_type.as_ref()),
                    path: rule_path.segments.clone(),
                    branches: show_branches_from(&rule_node.branches, &show_keys),
                    depends_on_rules: project_depends_on_rules(
                        &rule_node.depends_on_rules,
                        &show_keys,
                    ),
                },
            );
        }

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

    let root_ids: Vec<NormalFormId> = rules.values().map(|rule| rule.normal_form).collect();
    let (normal_forms, remapped_roots) = interner.extract_reachable(&root_ids);
    for (rule, remapped) in rules.values_mut().zip(remapped_roots) {
        rule.normal_form = remapped;
    }

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
        versions: Arc::from(Vec::new().into_boxed_slice()),
        start_line: 1,
        source_type: None,
        needed_by_rules: Vec::new(),
        data_display: IndexMap::new(),
        show_rules,
        // Filled below after validation succeeds.
        reference_ends: IndexMap::new(),
        input_key_index: IndexMap::new(),
        show_rule_index: IndexMap::new(),
        data_leaf: IndexMap::new(),
    };

    let mut plan_errors = validate_literal_data_against_types(&plan);
    if let Err(error) = validate_unit_conversion_targets(&plan) {
        plan_errors.push(error);
    }
    if !plan_errors.is_empty() {
        return Err(plan_errors);
    }

    plan.reference_ends = graph.reference_ends().clone();
    plan.input_key_index = plan
        .data
        .keys()
        .map(|path| (path.input_key(), path.clone()))
        .collect();
    plan.show_rule_index = plan
        .show_rules
        .keys()
        .map(|path| (path.input_key(), path.clone()))
        .collect();
    plan.data_leaf = ensure_data_leaves(&mut plan.normal_forms, &plan.data);
    for (path, &id) in &plan.data_leaf {
        debug_assert_eq!(
            plan.result_type(id).as_ref(),
            data_path_result_type(&plan.data, path).as_ref(),
            "BUG: DataPath leaf result_type drift for {path}"
        );
    }
    plan.needed_by_rules = plan.build_needed_by_rules();
    Ok(plan)
}

/// Ensure every `plan.data` path has a [`LeafKind::DataPath`] cell in `normal_forms`
/// and return the path → id index. Expression lowering already creates leaves for
/// referenced paths; unused paths get orphan leaves appended so evaluation can
/// still store bindings/defaults/reference copies in the value table.
///
/// Rule reference cells share a body's Kind, so a `Leaf(DataPath)` cell may carry
/// `rule_ref`. Those cells hold the referenced rule's value in the value table and
/// must never be the binding slot for the path.
fn ensure_data_leaves(
    normal_forms: &mut Vec<NormalForm>,
    data: &IndexMap<DataPath, DataDefinition>,
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
    for path in data.keys() {
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
/// succeeds so [`Engine::show`] / [`Engine::run`] only look up the plan.
pub(crate) fn attach_show_cache(
    plan: &mut ExecutionPlan,
    lemma_spec_set: &LemmaSpecSet,
    spec: &LemmaSpec,
    versions: &Arc<[ShowVersion]>,
) {
    let (effective_from, effective_to) = lemma_spec_set.effective_range(spec);
    plan.effective_from = effective_from;
    plan.effective_to = effective_to;
    plan.versions = Arc::clone(versions);
    plan.start_line = spec.start_line;
    plan.source_type = spec.source_type.clone();
    plan.data_display = build_data_display(plan);
}

fn build_data_display(plan: &ExecutionPlan) -> IndexMap<DataPath, ShowDataCache> {
    let mut out = IndexMap::new();
    for (path, data) in &plan.data {
        if data.schema_type().is_none() || matches!(data, DataDefinition::Reference { .. }) {
            continue;
        }
        let fill = match data {
            DataDefinition::Value {
                value,
                resolved_type,
                ..
            } => Some(crate::planning::semantics::BoundValueKind {
                value: value.value.clone(),
                measure_binding_unit: resolved_type.measure_binding_unit.as_deref().map(Arc::from),
            }),
            _ => data.bound_fill().cloned(),
        };
        let suggestion = data.bound_suggestion().cloned();
        if fill.is_some() || suggestion.is_some() {
            out.insert(path.clone(), ShowDataCache { fill, suggestion });
        }
    }
    out
}

/// Whether a show bound value is the fill or the suggestion for a data path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShowBoundRole {
    Fill,
    Suggestion,
}

pub(crate) fn show_bound_result_value(
    bound: &crate::planning::semantics::BoundValueKind,
    schema_type: &crate::planning::semantics::LemmaType,
    input_key: &str,
    role: ShowBoundRole,
) -> crate::result_value::RuleResultValue {
    let (fill, suggestion) = match role {
        ShowBoundRole::Fill => (Some(bound), None),
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
                ShowBoundRole::Fill => "fill",
                ShowBoundRole::Suggestion => "suggestion",
            };
            panic!(
                "BUG: show {role_name} value for '{input_key}' failed type_scoped_result_value_from_literal: {}",
                crate::result_value::rule_result_value_failure_message(failure)
            )
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
    pub fill: Option<RuleResultValue>,
    pub suggestion: Option<RuleResultValue>,
    /// Show.rules keys that transitively need this data after normalize.
    /// Empty = reuse catalog only (not an eval intake key for this spec).
    pub needed_by_rules: Vec<String>,
}

/// Half-open `[effective_from, effective_to)` for one loaded temporal row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShowVersion {
    pub effective_from: Option<crate::parsing::ast::DateTimeValue>,
    pub effective_to: Option<crate::parsing::ast::DateTimeValue>,
}

impl std::fmt::Display for ShowVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (&self.effective_from, &self.effective_to) {
            (Some(from), Some(to)) => write!(f, "{from} → {to}"),
            (Some(from), None) => write!(f, "{from} →"),
            (None, Some(to)) => write!(f, "→ {to}"),
            (None, None) => write!(f, "—"),
        }
    }
}

/// Consumer [`Engine::show`] result: declared promptable data catalog (with
/// [`ShowData::needed_by_rules`] for intake vs reuse), this spec's rule graph
/// (local rules plus reachable imports), and resolved temporal window.
/// Source: [`Engine::source`].
#[derive(Debug, Clone, PartialEq)]
pub struct Show {
    /// Interned repository name (`list()` string). `None` = unnamed workspace.
    pub repository: Option<String>,
    pub spec: String,
    pub commentary: Option<String>,
    pub effective_from: Option<crate::parsing::ast::DateTimeValue>,
    pub effective_to: Option<crate::parsing::ast::DateTimeValue>,
    pub versions: Vec<ShowVersion>,
    pub start_line: usize,
    pub source_type: Option<crate::parsing::source::SourceType>,
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

        if let Some(commentary) = &self.commentary {
            write!(f, "\n  {}", commentary)?;
        }

        if let Some(from) = &self.effective_from {
            write!(f, "\n  effective_from: {}", from)?;
        }
        if let Some(to) = &self.effective_to {
            write!(f, "\n  effective_to: {}", to)?;
        }

        if self.versions.len() > 1 {
            let version_strs: Vec<String> = self
                .versions
                .iter()
                .map(|v| match (&v.effective_from, &v.effective_to) {
                    (Some(f), Some(t)) => format!("{f} → {t}"),
                    (Some(f), None) => format!("{f} →"),
                    (None, Some(t)) => format!("→ {t}"),
                    (None, None) => "—".to_string(),
                })
                .collect();
            write!(f, "\n  versions: {}", version_strs.join(", "))?;
        }

        if !self.meta.is_empty() {
            write!(f, "\n\nMeta:")?;
            let mut entries: Vec<(&String, &Value)> = self.meta.iter().collect();
            entries.sort_by_key(|(k, _)| *k);
            for (key, value) in entries {
                write!(f, "\n  {}: {}", key, value)?;
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
                if let Some(val) = &entry.fill {
                    write!(f, "\n    fill: {}", val)?;
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

        if self.data.is_empty() && self.rules.is_empty() {
            write!(f, "\n  (no data or rules)")?;
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
        | TypeSpecification::Undetermined => {}
    }
    lines
}

impl ExecutionPlan {
    /// Expression-scope unit index (local types plus direct `uses` imports).
    pub(crate) fn expression_unit_index(&self) -> &crate::planning::unit_index::UnitIndex {
        &self.resolved_types.unit_index
    }

    /// Per [`Self::data`] position: local [`Self::rules`] positions that transitively need
    /// that slot (through rule references), in alphabetical rule-name order.
    ///
    /// Built in plan topological order: at a rule-ref cell, OR the target rule's
    /// already-computed bitset instead of descending Kind (rule references are evaluation
    /// boundaries). Walks the normalized body so constant-dead unless arms removed
    /// by normalize stay out of the index.
    fn build_needed_by_rules(&self) -> Vec<Vec<u32>> {
        use crate::planning::normalize::{LeafKind, NormalFormKind};

        let data_len = self.data.len();
        let words = data_len.div_ceil(64);
        let mut bits_by_rule: Vec<Vec<u64>> = Vec::with_capacity(self.rules.len());

        for (rule_pos, (_path, rule)) in self.rules.iter().enumerate() {
            let mut bits = vec![0u64; words];
            let mut visited = HashSet::new();
            let mut stack = vec![rule.normal_form];
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
                    assert!(
                        ref_pos < rule_pos,
                        "BUG: rule_ref target '{ref_path}' must precede '{}' in topo order",
                        rule.path
                    );
                    let dep = &bits_by_rule[ref_pos];
                    for (word, dep_word) in bits.iter_mut().zip(dep.iter()) {
                        *word |= *dep_word;
                    }
                    continue;
                }
                match &nf.kind {
                    NormalFormKind::Leaf(LeafKind::DataPath(path)) => {
                        if let Some(target) = self.promptable_data_path(path) {
                            let data_pos = self.data.get_index_of(target).unwrap_or_else(|| {
                                panic!(
                                    "BUG: promptable target '{target}' absent from plan.data (rule '{}')",
                                    rule.path
                                )
                            });
                            bits[data_pos / 64] |= 1u64 << (data_pos % 64);
                        }
                    }
                    NormalFormKind::OrderedDispatch { .. } => {
                        // Match static reachable_data_paths: walk origin Piecewise
                        // so shadowed arms stay in the Show needed set.
                        let origin = nf.origin.unwrap_or_else(|| {
                            panic!(
                                "BUG: non-rule_ref OrderedDispatch must carry origin (rule '{}')",
                                rule.path
                            )
                        });
                        stack.push(origin);
                    }
                    _ => stack.extend(nf.kind.children()),
                }
            }
            bits_by_rule.push(bits);
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

        let mut needed_by_rules = vec![Vec::new(); data_len];
        for &rule_pos in &consumers {
            let rule_id = u32::try_from(rule_pos).expect("BUG: rule count exceeds u32");
            let bits = &bits_by_rule[rule_pos];
            for (word_idx, &word) in bits.iter().enumerate() {
                let mut remaining = word;
                while remaining != 0 {
                    let bit = remaining.trailing_zeros() as usize;
                    let data_pos = word_idx * 64 + bit;
                    needed_by_rules[data_pos].push(rule_id);
                    remaining &= remaining - 1;
                }
            }
        }
        needed_by_rules
    }

    /// Promptable [`DataPath`] for a normal-form data leaf, following reference
    /// chain ends for references. `None` when the leaf is a rule-target or import
    /// reference (not caller-promptable).
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
            Some(DataDefinition::Import { .. }) => None,
            None => panic!("BUG: normal-form DataPath leaf absent from plan.data: {path}"),
        }
    }

    /// Validate caller-requested rule names and return resolved [`RulePath`]s.
    ///
    /// `None` means all local rules (this spec's outputs). `Some(&[])` is an error.
    /// Unknown names in `Some` slice error. Explicit names may be any Show.rules key.
    pub fn validated_response_rule_paths(
        &self,
        rules: Option<&[String]>,
    ) -> Result<std::collections::HashSet<RulePath>, Error> {
        let Some(rules) = rules else {
            return Ok(self
                .rules
                .values()
                .filter(|r| r.path.segments.is_empty())
                .map(|r| r.path.clone())
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
            paths.insert(rule.path.clone());
        }
        Ok(paths)
    }

    /// Look up a Show.rules key (`RulePath::input_key`) in the reachable graph.
    pub fn get_rule(&self, name: &str) -> Option<&ExecutableRule> {
        let canonical = crate::parsing::ast::ascii_lowercase_logical_name(name.to_string());
        let path = self.show_rule_index.get(&canonical)?;
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
                DataDefinition::Reference { .. } | DataDefinition::Import { .. } => None,
            })
    }
}

/// Follow nested [`NormalFormKind::OrderedDispatch`] region bodies using filled
/// scrutinee slots until a non-dispatch cell. Stops at rule-reference cells:
/// those borrow another rule's Kind and must compare as the origin arm body, not
/// the body that rule would itself select. `None` when a nested scrutinee is
/// unset or vetoed (the walk has not decided that nest yet).
pub(crate) fn resolve_nested_dispatch_body(
    plan: &ExecutionPlan,
    values: &[Option<crate::computation::OperationResult>],
    mut body: NormalFormId,
) -> Option<NormalFormId> {
    use crate::computation::OperationResult;
    use crate::planning::normalize::NormalFormKind;
    use crate::planning::ordered_dispatch::region_of_scrutinee;

    for _ in 0..=plan.normal_forms.len() {
        let cell = plan.normal_form(body);
        if cell.rule_ref.is_some() {
            return Some(body);
        }
        let NormalFormKind::OrderedDispatch {
            scrutinee,
            boundaries,
            regions,
        } = &cell.kind
        else {
            return Some(body);
        };
        let literal = match values.get(scrutinee.index()).and_then(|slot| slot.as_ref()) {
            Some(OperationResult::Value(literal)) => literal,
            Some(OperationResult::Veto(_)) | None => return None,
        };
        let region = region_of_scrutinee(boundaries, &literal.value).ok()?;
        body = regions[region];
    }
    panic!("BUG: nested OrderedDispatch chain longer than the normal-form table");
}

/// DataPath leaves reachable from `root` in evaluator decision-tree preorder.
///
/// Control liveness is derived from `values` (the evaluation value table), matching
/// the evaluator's own decisions:
/// - [`NormalFormKind::And`]: right child live only when the left slot is `true`.
///   A `false`, vetoed, or unfilled left slot means the evaluator never visits the
///   right child.
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
/// pop-time visited; children pushed in reverse evaluation order.
pub(crate) fn reachable_data_paths(
    plan: &ExecutionPlan,
    root: NormalFormId,
    values: &[Option<crate::computation::OperationResult>],
) -> IndexSet<DataPath> {
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

    let mut out = IndexSet::new();
    let mut visited = HashSet::new();
    let mut stack = vec![root];

    while let Some(id) = stack.pop() {
        if !visited.insert(id) {
            continue;
        }
        let nf = plan.normal_form(id);
        let live: Vec<NormalFormId> = match &nf.kind {
            NormalFormKind::Leaf(LeafKind::DataPath(path)) => {
                out.insert(path.clone());
                Vec::new()
            }
            NormalFormKind::And(left, right) => {
                // Match evaluate_and: only a settled `true` left keeps the right live.
                match slot_outcome(values, *left) {
                    Some(BranchOutcome::Taken) => vec![*left, *right],
                    Some(BranchOutcome::NotTaken | BranchOutcome::Propagate(_)) | None => {
                        vec![*left]
                    }
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

                let selected = match values.get(scrutinee.index()).and_then(|slot| slot.as_ref()) {
                    Some(OperationResult::Value(literal)) => {
                        region_of_scrutinee(boundaries, &literal.value)
                            .ok()
                            .map(|region| regions[region])
                    }
                    Some(OperationResult::Veto(_)) | None => None,
                };
                let resolved =
                    selected.and_then(|body| resolve_nested_dispatch_body(plan, values, body));
                let decision = match resolved {
                    Some(resolved) => {
                        let winner = arms.iter().rposition(|(_, body)| *body == resolved);
                        match winner {
                            Some(0) => PiecewiseDecision::Default,
                            Some(arm) => PiecewiseDecision::Taken { arm },
                            None => panic!(
                                "BUG: OrderedDispatch selected region body is not an origin arm body"
                            ),
                        }
                    }
                    // Outer key decided but nest open, or outer scrutinee open:
                    // keep every origin arm live (safe over-approx).
                    None => PiecewiseDecision::Undecided {
                        arm: arms.len() - 1,
                    },
                };
                let mut live = vec![*scrutinee];
                // Origin Piecewise conditions are not filled under the dispatch
                // value walk, so an undecided nest must be visited directly —
                // otherwise And short-circuit on empty slots drops the residual
                // scrutinee (e.g. weight in `zone is K and weight >= N`).
                if let (Some(selected_body), None) = (selected, resolved) {
                    live.push(selected_body);
                }
                live.extend(live_piecewise_children(arms, decision));
                live
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
            | NormalFormKind::ResultIsVeto(_) => nf.kind.children(),
        };
        stack.extend(live.into_iter().rev());
    }
    out
}

pub(crate) fn validate_value_against_type(
    expected_type: &LemmaType,
    value: &LiteralValue,
    unit_index: &crate::planning::unit_index::UnitIndex,
) -> Result<(), String> {
    use crate::computation::rational::{checked_mul, rational_new, try_pow_i32, RationalInteger};
    use crate::planning::semantics::TypeSpecification;

    fn exceeds_decimal_places(magnitude: &RationalInteger, max_decimals: u8) -> bool {
        let scale = match try_pow_i32(&rational_new(10, 1), i32::from(max_decimals)) {
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
                .measure_binding_unit
                .as_deref()
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
                .measure_binding_unit
                .as_deref()
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
        (
            range_spec @ (TypeSpecification::NumberRange { .. }
            | TypeSpecification::DateRange { .. }
            | TypeSpecification::TimeRange { .. }
            | TypeSpecification::MeasureRange { .. }
            | TypeSpecification::RatioRange { .. }),
            ValueKind::Range(left, right),
        ) => validate_range_literal(
            expected_type,
            range_spec,
            left.as_ref(),
            right.as_ref(),
            unit_index,
        ),
        (TypeSpecification::Veto { .. }, _) | (TypeSpecification::Undetermined, _) => Ok(()),
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
    left: &LiteralValue,
    right: &LiteralValue,
    unit_index: &crate::planning::unit_index::UnitIndex,
) -> Result<(), String> {
    use crate::computation::{comparison_operation, OperationResult};
    use crate::planning::semantics::{
        compare_semantic_dates, compare_semantic_times, measure_declared_bound_to_canonical,
        ValueKind,
    };
    use std::cmp::Ordering;
    use std::sync::Arc;

    let mut element_spec = range_spec
        .element_from_range()
        .expect("BUG: element_from_range missing arm for validated range");
    if let TypeSpecification::Measure {
        units,
        decomposition,
        ..
    } = &mut element_spec
    {
        if decomposition.is_none() && !units.0.is_empty() {
            *decomposition = Some([(expected_type.name(), 1i32)].into_iter().collect());
        }
    }
    let element_type = Arc::new(LemmaType::primitive(element_spec));
    let left = LiteralValue {
        value: left.value.clone(),
    };
    let right = LiteralValue {
        value: right.value.clone(),
    };
    validate_value_against_type(element_type.as_ref(), &left, unit_index)?;
    validate_value_against_type(element_type.as_ref(), &right, unit_index)?;

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
            "range left endpoint {left} is above right endpoint {right}"
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
                let bound = LiteralValue::measure_with_type(canonical, Arc::clone(&element_type));
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
                let bound = LiteralValue::measure_with_type(canonical, Arc::clone(&element_type));
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
                    Ok((
                        LiteralValue::measure_with_type(canonical, Arc::clone(&owner)),
                        Arc::clone(&owner),
                    ))
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
        worklist.extend(nf.kind.children());
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
    use crate::evaluation::run_data::RunData;
    use crate::literals::DateGranularity;
    use crate::literals::TimezoneValue;
    use crate::parsing::ast::DateTimeValue;
    use crate::planning::semantics::{DataDefinition, DataPath, PathSegment, TypeSpecification};
    use crate::Engine;
    use crate::{ResourceLimits, RunDataValue};
    use serde_json;
    use std::collections::HashMap;
    use std::str::FromStr;
    use std::sync::Arc;

    fn default_limits() -> ResourceLimits {
        ResourceLimits::default()
    }

    fn resolve_run_data(plan: &ExecutionPlan, values: HashMap<String, RunDataValue>) -> RunData {
        RunData::resolve(plan, values, &default_limits()).expect("resolve")
    }

    fn veto_reason<'a>(run_data: &'a RunData, path: &DataPath) -> Option<&'a str> {
        match run_data.bindings.get(path) {
            Some(OperationResult::Veto(veto)) => Some(match veto {
                VetoType::Computation { message } => message.as_str(),
                other => panic!("expected Computation veto, got {other:?}"),
            }),
            _ => None,
        }
    }

    fn bound_value<'a>(
        run_data: &'a RunData,
        path: &DataPath,
    ) -> Option<&'a crate::planning::semantics::BoundValueKind> {
        run_data.bindings.get(path).and_then(OperationResult::value)
    }

    fn input_data(pairs: &[(&str, &str)]) -> HashMap<String, RunDataValue> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), RunDataValue::string(*v)))
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

        let run_data = resolve_run_data(plan, values);
        let updated_value = bound_value(&run_data, &data_path).expect("bound value");
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

        let run_data = resolve_run_data(plan, values);
        let data_path = DataPath::new(vec![], "age".to_string());
        match veto_reason(&run_data, &data_path) {
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

        let run_data = resolve_run_data(plan, values);
        assert!(run_data.bindings.is_empty());
        assert!(run_data.ignored_unknown.iter().any(|k| k == "unknown"));
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

        let run_data = resolve_run_data(plan, values);
        let data_path = DataPath {
            segments: vec![PathSegment {
                uses: "rules".to_string(),
                repository: None,
                spec: "private".to_string(),
            }],
            data: "base_price".to_string(),
        };
        let updated_value = bound_value(&run_data, &data_path).expect("bound value");
        match &updated_value.value {
            crate::planning::semantics::ValueKind::Number(n) => {
                assert_eq!(n, &rational_new(100, 1));
            }
            other => panic!("Expected number literal, got {:?}", other),
        }
    }

    #[test]
    fn run_data_should_enforce_number_maximum_constraint() {
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
                value: crate::planning::semantics::LiteralValue::number_with_type(
                    rational_new(0, 1),
                    Arc::new(max10.clone()),
                ),
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
            versions: std::sync::Arc::from([]),
            start_line: 1,
            source_type: None,
            needed_by_rules: vec![Vec::new(); data_len],
            data_display: IndexMap::new(),
            show_rules: IndexMap::new(),
            reference_ends: IndexMap::new(),
            input_key_index,
            show_rule_index: IndexMap::new(),
            data_leaf: IndexMap::new(),
        };

        let values = input_data(&[("x", "11")]);

        let run_data = resolve_run_data(&plan, values);
        match veto_reason(&run_data, &data_path) {
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
    fn run_data_should_enforce_text_enum_options() {
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
                value: crate::planning::semantics::LiteralValue::text_with_type(
                    "silver".to_string(),
                    Arc::new(tier.clone()),
                ),
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
            versions: std::sync::Arc::from([]),
            start_line: 1,
            source_type: None,
            needed_by_rules: vec![Vec::new(); data_len],
            data_display: IndexMap::new(),
            show_rules: IndexMap::new(),
            reference_ends: IndexMap::new(),
            input_key_index,
            show_rule_index: IndexMap::new(),
            data_leaf: IndexMap::new(),
        };

        let values = input_data(&[("tier", "platinum")]);

        let run_data = resolve_run_data(&plan, values);
        match veto_reason(&run_data, &data_path) {
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
    fn run_data_should_enforce_measure_decimals() {
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
                value: crate::planning::semantics::LiteralValue::measure_with_type(
                    rational_zero(),
                    Arc::new(money.clone()),
                ),
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
            versions: std::sync::Arc::from([]),
            start_line: 1,
            source_type: None,
            needed_by_rules: vec![Vec::new(); data_len],
            data_display: IndexMap::new(),
            show_rules: IndexMap::new(),
            reference_ends: IndexMap::new(),
            input_key_index,
            show_rule_index: IndexMap::new(),
            data_leaf: IndexMap::new(),
        };

        let values = input_data(&[("price", "1.234 eur")]);

        let run_data = resolve_run_data(&plan, values);
        match veto_reason(&run_data, &data_path) {
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
            versions: std::sync::Arc::from([]),
            start_line: 1,
            source_type: None,
            needed_by_rules: Vec::new(),
            data_display: IndexMap::new(),
            show_rules: IndexMap::new(),
            reference_ends: IndexMap::new(),
            input_key_index: IndexMap::new(),
            show_rule_index: IndexMap::new(),
            data_leaf: IndexMap::new(),
        }
    }

    fn plans_by_effective(
        plans: impl IntoIterator<Item = ExecutionPlan>,
    ) -> BTreeMap<EffectiveDate, ExecutionPlan> {
        plans
            .into_iter()
            .map(|plan| (plan.effective.clone(), plan))
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
            plans.get(&june_key).expect("june slice")
        ));

        let dec_plan = plan_at(&plans, &dec_key).expect("dec boundary");
        assert!(std::ptr::eq(
            dec_plan,
            plans.get(&dec_key).expect("dec slice")
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
            plans.get(&origin).expect("origin slice")
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
            plans.get(&start).expect("single slice")
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

        let value: serde_json::Value =
            serde_json::to_value(crate::api::Show::from(&schema)).unwrap();

        let bh = &value["data"]["bridge_height"];
        assert!(
            bh.is_object(),
            "data entry must be a named object, not tuple"
        );
        assert!(
            bh.get("type").is_some(),
            "data entry must expose `type` field"
        );
        assert!(
            bh.get("suggestion").is_some(),
            "bridge_height exposes `-> suggest` as schema suggestion"
        );
        assert!(
            bh.get("fill").is_none(),
            "bridge_height is not filled from spec"
        );

        let ty = &bh["type"];
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

        let quantity = &value["data"]["quantity"];
        assert_eq!(quantity["type"]["kind"], "number");
        assert!(
            quantity.get("suggestion").is_none(),
            "quantity has no suggestion"
        );
        assert!(
            quantity.get("fill").is_none(),
            "quantity has no fill literal"
        );

        let cost = &value["rules"]["cost"];
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
        let value: serde_json::Value =
            serde_json::to_value(crate::api::Show::from(&schema)).unwrap();

        let money_units = &value["data"]["money"]["type"]["units"];
        assert!(money_units.is_array() && !money_units.as_array().unwrap().is_empty());
        assert!(money_units[0].get("name").is_some());
        assert!(money_units[0].get("factor").is_some());
        assert!(money_units[0]["factor"].get("numer").is_some());
        assert!(money_units[0]["factor"].get("denom").is_some());

        let rate_units = &value["data"]["rate"]["type"]["units"];
        assert!(rate_units.is_array() && !rate_units.as_array().unwrap().is_empty());
        assert!(rate_units[0].get("name").is_some());
        assert!(rate_units[0].get("value").is_some());
        assert!(rate_units[0]["value"].get("numer").is_some());
        assert!(rate_units[0]["value"].get("denom").is_some());

        let total_rule_units = &value["rules"]["total"]["type"]["units"];
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

        let rate_out_rule_units = &value["rules"]["rate_out"]["type"]["units"];
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
    fn show_json_round_trip_preserves_shape() {
        let mut engine = Engine::new();
        engine
            .load([(
                crate::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("s.lemma"))),
                r#"
                spec s
                data age: number -> minimum 0 -> suggest 18
                data grade: text -> options "A" "B" "C"
                rule adult: age >= 18
                "#
                .to_string(),
            )])
            .unwrap();
        let now = DateTimeValue::now();
        let schema = engine.show(None, "s", Some(&now)).unwrap();

        let api_show = crate::api::Show::from(&schema);
        let json = serde_json::to_string(&api_show).unwrap();
        let round_tripped: crate::api::Show = serde_json::from_str(&json).unwrap();
        assert_eq!(api_show, round_tripped);
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

    fn cost_price_inputs() -> HashMap<String, RunDataValue> {
        let mut data = HashMap::new();
        data.insert("product_cost".into(), RunDataValue::string("4 eur_per_kg"));
        data.insert("labor_cost".into(), RunDataValue::string("25 eur_per_hour"));
        data.insert("throughput".into(), RunDataValue::string("12 kg_per_hour"));
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
            timezone: Some(TimezoneValue {
                offset_hours: 0,
                offset_minutes: 0,
            }),
            granularity: DateGranularity::DateTime,
        }
    }

    #[test]
    fn run_data_accepts_per_unit_measure_equivalent_to_canonical_magnitude() {
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
        let mut data = HashMap::new();
        data.insert("product_cost".into(), RunDataValue::string("4 eur_per_kg"));
        data.insert(
            "labor_cost".into(),
            RunDataValue::string("0.0069444444444444444444444444 eur_per_hour"),
        );
        data.insert(
            "throughput".into(),
            RunDataValue::string("0.0033333333333333333333333333 kg_per_hour"),
        );
        let run_data = resolve_run_data(plan, data);
        assert!(
            !run_data
                .bindings
                .values()
                .any(|b| matches!(b, OperationResult::Veto(_))),
            "parsed decimal run data values must not be veto-bound after input boundary: {:?}",
            run_data.bindings
        );
    }

    #[test]
    fn run_data_accepts_per_unit_measure() {
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
        let run_data = resolve_run_data(plan, cost_price_inputs());
        assert!(!run_data
            .bindings
            .values()
            .any(|b| matches!(b, OperationResult::Veto(_))));
    }

    #[test]
    fn run_data_rejects_oversize_input() {
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
            RunDataValue::string(
                "1000000000000000000000000000000000000000000000000000000000000 eur_per_hour",
            ),
        );
        let run_data = resolve_run_data(plan, data);
        assert!(matches!(
            run_data.bindings.get(&DataPath::local("labor_cost".into())),
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
    fn run_prunes_inactive_nut_branches_for_total_price() {
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

data type_of_nut: text -> options "peanut" "cashew"

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
        let mut inputs = HashMap::new();
        inputs.insert("type_of_nut".to_string(), "peanut".to_string());
        let response = engine
            .run(
                None,
                "calc",
                Some(&now),
                inputs,
                Some(&["total_price".to_string()]),
                false,
            )
            .expect("run must succeed");

        let names = response_missing_data_union(&response);
        assert!(
            !names.contains(&"type_of_nut".to_string()),
            "supplied type_of_nut is bound and must not appear in missing_data: {names:?}"
        );
        assert!(names.contains(&"bag.weight".to_string()));
        assert!(names.contains(&"bag.chocolatizing".to_string()));
        assert!(!names.contains(&"bag.item_cost".to_string()));
        assert!(!names.contains(&"bag.roasting".to_string()));
    }

    #[test]
    fn run_includes_membership_dates_when_premium_member_false() {
        let mut engine = Engine::new();
        engine
            .load([(crate::SourceType::Volatile, FILM_ACCESS.to_string())])
            .expect("film_access spec must load");
        let now = film_access_effective();
        let mut inputs = HashMap::new();
        inputs.insert("type".to_string(), "rental".to_string());
        inputs.insert("views_consumed".to_string(), "6".to_string());
        inputs.insert("premium_member".to_string(), "false".to_string());
        let response = engine
            .run(
                None,
                "film_access",
                Some(&now),
                inputs,
                Some(&["can_view".to_string()]),
                false,
            )
            .expect("run must succeed");

        let names = response_missing_data_union(&response);
        assert!(names.contains(&"premium_membership.start".to_string()));
        assert!(names.contains(&"premium_membership.length".to_string()));
    }

    #[test]
    fn run_includes_membership_dates_when_premium_member_unknown() {
        let mut engine = Engine::new();
        engine
            .load([(crate::SourceType::Volatile, FILM_ACCESS.to_string())])
            .expect("film_access spec must load");
        let now = film_access_effective();
        let mut inputs = HashMap::new();
        inputs.insert("type".to_string(), "rental".to_string());
        inputs.insert("views_consumed".to_string(), "6".to_string());
        let response = engine
            .run(
                None,
                "film_access",
                Some(&now),
                inputs,
                Some(&["can_view".to_string()]),
                false,
            )
            .expect("run must succeed");

        let names = response_missing_data_union(&response);
        assert!(names.contains(&"premium_member".to_string()));
        assert!(names.contains(&"premium_membership.start".to_string()));
        assert!(names.contains(&"premium_membership.length".to_string()));
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
  * ceil storage_duration as week as Number

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
    fn quotation_plans_without_consumer_stdlib_units() {
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
        assert!(
            expression_units.unique_owner("week").is_none(),
            "consumer expression scope must not contain week: {:?}",
            expression_units.keys().collect::<Vec<_>>()
        );
        assert!(
            expression_units.unique_owner("minute").is_none(),
            "consumer expression scope must not contain minute: {:?}",
            expression_units.keys().collect::<Vec<_>>()
        );
        let mut keys: Vec<_> = expression_units.keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            ["percent", "permille"],
            "consumer expression scope must only have builtin ratio units, not dependency units"
        );
    }

    fn warehousing_default_inputs(prefix: &str) -> HashMap<String, String> {
        let key = |name: &str| {
            if prefix.is_empty() {
                name.to_string()
            } else {
                format!("{prefix}.{name}")
            }
        };
        HashMap::from([
            (key("units_per_pallet"), "1".to_string()),
            (key("storage_duration"), "10 day".to_string()),
            (key("interbranch_transport_per_pallet"), "0 eur".to_string()),
            (key("inbound_handling_per_pallet"), "0 eur".to_string()),
            (key("storage_per_pallet_per_week"), "10 eur".to_string()),
            (key("labeling_per_pallet"), "0 eur".to_string()),
            (key("outbound_handling_per_pallet"), "0 eur".to_string()),
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
            .run(
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
}

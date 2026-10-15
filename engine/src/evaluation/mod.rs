//! Pure Rust evaluation engine for Lemma
//!
//! Executes pre-validated execution plans by walking each rule's
//! [`NormalForm`] equation DAG into one value table ([`tree`]). When `explain`
//! is true the walk is exhaustive (visits every cell narration displays) and a
//! second pass ([`narration`]) builds explanation trees from the filled table.
//!
//! Request state is one value table indexed by [`NormalFormId`]: the plan's
//! node table *is* the arena.

pub(crate) mod branch_semantics;
pub(crate) mod conversion_trace;
pub mod evaluate_data;
pub mod explanations;
pub mod expression;
pub(crate) mod imports;
pub(crate) mod list_ops;
pub(crate) mod narration;
pub mod response;
pub(crate) mod rows;
pub(crate) mod tree;

pub use crate::computation::OperationResult;
use crate::computation::VetoType;
use crate::evaluation::response::EvaluatedRule;
use crate::planning::execution_plan::{
    reachable_data_paths, validate_value_against_type, ExecutionPlan, ReachableInput,
};
use crate::planning::graph::absolute_import_member_path;
use crate::planning::normalize::NormalFormId;
use crate::planning::semantics::{
    BoundValueKind, DataDefinition, DataPath, ImportOverlay, LemmaType, LiteralValue,
    ReferenceTarget, RowInstance, RulePath, SpecIdentity, ValueKind,
};
pub use evaluate_data::{EvaluateData, EvaluateDataValue};
use indexmap::IndexMap;
pub use response::{Response, RuleResult};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

fn closest_ignored_key(needed: &str, ignored: &[String]) -> Option<String> {
    crate::string_distance::closest_name(needed, ignored)
}

/// Unknown keys under `prefix.`, with the prefix stripped, for a body the prefix addresses.
pub(super) fn ignored_under_prefix(ignored: &[String], prefix: &str) -> Vec<String> {
    let dotted = format!("{prefix}.");
    ignored
        .iter()
        .filter_map(|key| {
            let canonical = crate::parsing::ast::ascii_lowercase_logical_name(key.clone());
            canonical
                .strip_prefix(&dotted)
                .filter(|rest| !rest.is_empty())
                .map(str::to_string)
        })
        .collect()
}

/// Constant value of a DataPath that peels through named `uses` into a template: a site
/// overlay literal or a template default. `None` when the member has no constant.
fn resolve_import_member_value(plan: &ExecutionPlan, path: &DataPath) -> Option<OperationResult> {
    match plan.import_overlay_entry(path) {
        Some(ImportOverlay::Literal(bound)) => {
            return Some(OperationResult::from_bound(bound.clone()));
        }
        // Reference overlays validate against the template field type in the
        // import scope; do not prefill here.
        Some(ImportOverlay::Reference { .. }) => return None,
        None => {}
    }
    let mut plan: &ExecutionPlan = plan;
    let mut path = path.clone();
    loop {
        let mut hops = path.segments.iter();
        let first = hops
            .next()
            .unwrap_or_else(|| panic!("BUG: resolve_import_member_value on local path '{path}'"));
        let import_marker = DataPath::local(first.uses.clone());
        let Some(DataDefinition::Import { identity, .. }) = plan.data.get(&import_marker) else {
            panic!("BUG: member '{path}' first hop is not a local Import marker");
        };
        let relative = DataPath {
            segments: hops.cloned().collect(),
            data: path.data.clone(),
        };
        let template = plan.row_template(identity);
        if relative.segments.is_empty() {
            let definition = template.data.get(&relative).unwrap_or_else(|| {
                panic!(
                    "BUG: member '{path}' absent from template '{}'",
                    template.spec_name
                )
            });
            if let Some(fill) = definition.bound_value() {
                return Some(OperationResult::from_bound(fill.clone()));
            }
            if let Some(literal) = definition.value() {
                let unit = definition
                    .schema_type()
                    .and_then(|schema| schema.unit().map(Arc::from));
                return Some(OperationResult::from_bound(BoundValueKind {
                    value: literal.value,
                    unit,
                }));
            }
            return None;
        }
        plan = template;
        path = relative;
    }
}

/// Request-local mutable state for one plan run.
///
/// The value table is indexed by [`NormalFormId`] and doubles as memo and data
/// store. Rule reference values live in [`Self::rule_values`], filled by
/// [`tree::ensure_rule_values`] before a consumer rule reference reads them. Control decisions
/// for missing-data walks are read from filled condition / scrutinee slots —
/// there is no separate log.
pub(crate) struct EvaluationContext {
    /// One slot per `plan.normal_forms` cell. Filled by data resolve and by
    /// `eval`; never cleared mid-request (values are a function of data).
    pub(crate) values: Vec<Option<OperationResult>>,
    /// One slot per `plan.rules` entry (same index as `IndexMap`). Filled by
    /// [`tree::ensure_rule_values`] before consumers read rule references.
    pub(crate) rule_values: Vec<Option<OperationResult>>,
    now: LiteralValue,
    /// Ignored input keys from run data (typo hints for MissingData).
    ignored_unknown: Vec<String>,
    /// Whether this run data left any of the plan's promptable data paths unbound.
    any_promptable_data_unbound: bool,
    /// Evaluation policy: visit every cell narration displays (rewrite
    /// pre-images, arithmetic siblings after a definitive veto). Set for explain
    /// runs. Never changes a value. Unless arms after the winner are already
    /// visited by the last-wins reverse scan.
    pub(crate) exhaustive: bool,
    /// Explain runs only: arena ids of narrated rules in this context, filled in
    /// plan order after the walk.
    pub(crate) rule_explanations: HashMap<RulePath, narration::ArenaId>,
    /// Current row of each enclosing instance-list command, by level.
    pub(crate) row_stack: Vec<Arc<RowInstance>>,
    /// Template-plan context of every row read so far.
    pub(crate) row_contexts: HashMap<Arc<RowInstance>, EvaluationContext>,
    /// Missing row field keys in read order; may repeat.
    pub(crate) missing_row_keys: Vec<String>,
    /// Missing row field keys read under each row-reading cell, deduplicated.
    pub(crate) row_missing: HashMap<NormalFormId, Vec<String>>,
    /// Exhaustive runs only: row field reads in read order; may repeat.
    pub(crate) row_reads_log: Vec<rows::RowRead>,
    /// Exhaustive runs only: row field reads under each row-reading cell, deduplicated.
    pub(crate) row_reads: HashMap<NormalFormId, Vec<rows::RowRead>>,
    /// Template contexts for named `uses` aliases, keyed by local Import path on the
    /// plan that owns the alias.
    pub(crate) import_contexts: HashMap<DataPath, EvaluationContext>,
}

/// Lengths of the row logs before a row-reading cell evaluates.
#[derive(Clone, Copy)]
pub(crate) struct RowLogMark {
    missing: usize,
    reads: usize,
}

impl EvaluationContext {
    fn new(
        plan: &ExecutionPlan,
        evaluate_data: &EvaluateData,
        now: LiteralValue,
        exhaustive: bool,
    ) -> Self {
        let mut values: Vec<Option<OperationResult>> = vec![None; plan.normal_forms.len()];

        // Caller bindings into their data-leaf slots.
        for (path, binding) in &evaluate_data.bindings {
            let leaf = *plan
                .data_leaf
                .get(path)
                .unwrap_or_else(|| panic!("BUG: run binding for '{path}' has no data_leaf entry"));
            values[leaf.index()] = Some(binding.clone());
        }

        // Plan defaults into unbound data-leaf slots.
        for (path, definition) in &plan.data {
            let leaf = *plan
                .data_leaf
                .get(path)
                .expect("BUG: every plan.data path must have a data_leaf entry");
            if values[leaf.index()].is_some() {
                continue;
            }
            if let Some(fill) = definition.bound_value() {
                values[leaf.index()] = Some(OperationResult::from_bound(fill.clone()));
            } else if let Some(literal) = definition.value() {
                let unit = definition
                    .schema_type()
                    .and_then(|schema| schema.unit().map(Arc::from));
                values[leaf.index()] = Some(OperationResult::from_bound(BoundValueKind {
                    value: literal.value,
                    unit,
                }));
            }
        }

        // Named-`uses` members: site overlay and template defaults into expression leaves.
        // Skip when the plan has no Import rows (no segmented data_leaf keys to fill).
        if plan
            .data
            .values()
            .any(|definition| matches!(definition, DataDefinition::Import { .. }))
        {
            for (path, &leaf) in &plan.data_leaf {
                if values[leaf.index()].is_some() || path.segments.is_empty() {
                    continue;
                }
                if let Some(result) = resolve_import_member_value(plan, path) {
                    values[leaf.index()] = Some(result);
                }
            }
        }

        // Reference chains: copy target into reference leaf (data_reference_order).
        for reference_path in &plan.data_reference_order {
            let leaf = *plan
                .data_leaf
                .get(reference_path)
                .expect("BUG: reference path missing from data_leaf");
            if values[leaf.index()].is_some() {
                continue;
            }
            match plan.data.get(reference_path) {
                Some(DataDefinition::Reference {
                    target: ReferenceTarget::Data(target_path),
                    resolved_type,
                    ..
                }) => {
                    let target_leaf = *plan
                        .data_leaf
                        .get(target_path)
                        .expect("BUG: reference target missing from data_leaf");
                    match values[target_leaf.index()].as_ref() {
                        Some(OperationResult::Veto(veto)) => {
                            values[leaf.index()] = Some(OperationResult::Veto(veto.clone()));
                        }
                        Some(OperationResult::Value(bound)) => {
                            let copied = bound.to_literal();
                            match validate_value_against_type(
                                resolved_type.as_ref(),
                                &copied,
                                &plan.resolved_types.unit_index,
                            ) {
                                Ok(()) => {
                                    values[leaf.index()] =
                                        Some(OperationResult::from_bound(bound.clone()));
                                }
                                Err(msg) => {
                                    values[leaf.index()] = Some(OperationResult::Veto(
                                        VetoType::computation(format!(
                                            "Reference '{}' violates its declared constraint: {}",
                                            reference_path, msg
                                        )),
                                    ));
                                }
                            }
                        }
                        None => {}
                    }
                }
                Some(DataDefinition::Reference {
                    target: ReferenceTarget::Rule(_),
                    ..
                }) => unreachable!(
                    "BUG: data_reference_order holds only data-target references; '{}' targets a rule",
                    reference_path
                ),
                Some(
                    DataDefinition::Value { .. }
                    | DataDefinition::TypeDeclaration { .. }
                    | DataDefinition::Import { .. }
                    | DataDefinition::InstanceList { .. },
                ) => unreachable!(
                    "BUG: data_reference_order entry '{}' is not a reference",
                    reference_path
                ),
                None => unreachable!(
                    "BUG: data_reference_order references missing data path '{}'",
                    reference_path
                ),
            }
        }

        let any_promptable_data_unbound = plan.promptable_data_paths().any(|path| {
            let leaf = *plan
                .data_leaf
                .get(path)
                .expect("BUG: promptable path missing from data_leaf");
            values[leaf.index()].is_none()
        }) || plan.data_leaf.keys().any(|path| {
            if path.segments.is_empty() {
                return false;
            }
            let Some(promptable) = plan.promptable_data_path(path) else {
                return false;
            };
            let leaf = *plan
                .data_leaf
                .get(promptable)
                .unwrap_or_else(|| panic!("BUG: promptable '{promptable}' has no data_leaf"));
            values[leaf.index()].is_none()
        });

        Self {
            values,
            rule_values: vec![None; plan.rules.len()],
            now,
            ignored_unknown: evaluate_data.ignored_unknown.clone(),
            any_promptable_data_unbound,
            exhaustive,
            rule_explanations: HashMap::new(),
            row_stack: Vec::new(),
            row_contexts: HashMap::new(),
            missing_row_keys: Vec::new(),
            row_missing: HashMap::new(),
            row_reads_log: Vec::new(),
            row_reads: HashMap::new(),
            import_contexts: HashMap::new(),
        }
    }

    pub(crate) fn row_log_mark(&self) -> RowLogMark {
        RowLogMark {
            missing: self.missing_row_keys.len(),
            reads: self.row_reads_log.len(),
        }
    }

    /// Record the row logs written since `mark` against row-reading cell `id`.
    pub(crate) fn record_row_log(&mut self, id: NormalFormId, mark: RowLogMark) {
        if self.missing_row_keys.len() > mark.missing {
            let keys = self.row_missing.entry(id).or_default();
            for key in &self.missing_row_keys[mark.missing..] {
                if !keys.contains(key) {
                    keys.push(key.clone());
                }
            }
        }
        if self.row_reads_log.len() > mark.reads {
            let reads = self.row_reads.entry(id).or_default();
            for read in &self.row_reads_log[mark.reads..] {
                if !reads.iter().any(|seen| seen.same_field(read)) {
                    reads.push(read.clone());
                }
            }
        }
    }

    /// Stored value for a rule previously evaluated in this request.
    pub(crate) fn rule_value<'a>(
        &'a self,
        plan: &ExecutionPlan,
        path: &RulePath,
    ) -> &'a OperationResult {
        self.rule_values[plan.rule_index(path).index()]
            .as_ref()
            .unwrap_or_else(|| {
                panic!(
                    "BUG: rule '{}' rule reference read before evaluation",
                    path.rule
                )
            })
    }

    pub(crate) fn now(&self) -> &LiteralValue {
        &self.now
    }

    /// Planned schema type for displaying `settled`, the value read at data path `path`.
    ///
    /// Binding priority: schema/`as` > settled > fill > suggest > first declared.
    /// Reuses the plan's [`Arc`] when no binding overrides the unit.
    #[must_use]
    pub(crate) fn data_display_type(
        plan: &ExecutionPlan,
        path: &DataPath,
        settled: &OperationResult,
    ) -> Arc<LemmaType> {
        let def = crate::planning::normalize::lookup_data_definition(
            &plan.data,
            &plan.row_templates,
            path,
        )
        .expect("BUG: data path leaf missing from plan.data and import templates");
        let schema = def
            .resolved_type_arc()
            .expect("BUG: data path leaf missing schema type");
        crate::planning::semantics::lemma_type_arc_with_display_binding(
            schema,
            settled.value(),
            def.bound_value(),
            def.bound_suggestion(),
        )
    }

    /// Rule result type for display.
    ///
    /// Binding priority: `as` on planned type > settled > fill > suggest on a data-path leaf >
    /// first declared. Reuses the planned [`Arc`] when no binding overrides the unit.
    #[must_use]
    pub(crate) fn rule_result_type(
        &self,
        plan: &ExecutionPlan,
        rule: &crate::planning::execution_plan::ExecutableRule,
    ) -> Arc<LemmaType> {
        let planned = &rule.rule_type;
        if planned.unit().is_some() {
            return Arc::clone(planned);
        }
        let settled = self
            .rule_values
            .get(plan.rule_index(&rule.path).index())
            .and_then(|slot| slot.as_ref())
            .and_then(OperationResult::value);
        match &plan.normal_form(rule.normal_form).kind {
            crate::planning::normalize::NormalFormKind::Leaf(
                crate::planning::normalize::LeafKind::DataPath(path),
            ) => {
                let def = crate::planning::normalize::lookup_data_definition(
                    &plan.data,
                    &plan.row_templates,
                    path,
                )
                .expect("BUG: rule DataPath leaf missing from plan.data and import templates");
                crate::planning::semantics::lemma_type_arc_with_display_binding(
                    planned,
                    settled,
                    def.bound_value(),
                    def.bound_suggestion(),
                )
            }
            _ => crate::planning::semantics::lemma_type_arc_with_display_binding(
                planned, settled, None, None,
            ),
        }
    }

    /// Slot for a data path's leaf cell.
    pub(crate) fn data_slot(
        &self,
        plan: &ExecutionPlan,
        data_path: &DataPath,
    ) -> Option<&OperationResult> {
        let leaf = *plan
            .data_leaf
            .get(data_path)
            .unwrap_or_else(|| panic!("BUG: data path '{data_path}' has no data_leaf entry"));
        self.values[leaf.index()].as_ref()
    }

    pub(crate) fn missing_data_suggestion(&self, data_path: &DataPath) -> Option<String> {
        closest_ignored_key(&data_path.input_key(), &self.ignored_unknown)
    }

    /// Whether this evaluation has a value or a veto for `data_path`.
    fn is_data_bound(&self, plan: &ExecutionPlan, data_path: &DataPath) -> bool {
        self.data_slot(plan, data_path).is_some()
    }

    /// Whether any row-reading cell in this context or a named `uses` context beneath it
    /// recorded a missing row field.
    fn has_row_missing_logs(&self) -> bool {
        !self.row_missing.is_empty()
            || self
                .import_contexts
                .values()
                .any(EvaluationContext::has_row_missing_logs)
    }

    /// Promptable data paths this rule still needs, in evaluation / decision-tree order.
    ///
    /// First key is the next fact the live tree needs. Returns immediately when the run
    /// data bound every promptable path: no rule can report missing data. Otherwise walks
    /// from `rule_root` deriving liveness from filled condition/scrutinee slots, maps each
    /// leaf through [`ExecutionPlan::promptable_data_path`], and keeps unbound keys.
    /// A named-`uses` rule contributes its template's walk under the import context,
    /// each field mapped through the site overlay (see [`missing_candidates`]).
    pub(crate) fn missing_data_for_rule(
        &self,
        plan: &ExecutionPlan,
        rule_root: NormalFormId,
    ) -> Vec<String> {
        if !self.any_promptable_data_unbound && !self.has_row_missing_logs() {
            return Vec::new();
        }
        let mut candidates = Vec::new();
        missing_candidates(plan, Some(self), rule_root, &mut candidates);
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for candidate in candidates {
            let key = match candidate {
                MissingCandidate::Data(path) => {
                    if self.is_data_bound(plan, &path) {
                        continue;
                    }
                    path.input_key()
                }
                MissingCandidate::Row(key) => key,
            };
            if seen.insert(key.clone()) {
                out.push(key);
            }
        }
        out
    }
}

/// Live input of a rule walk, before the root context's bound check.
enum MissingCandidate {
    /// Promptable path in the walked plan's owner frame.
    Data(DataPath),
    /// Missing row field key recorded during evaluation.
    Row(String),
}

/// One unit of the missing-candidate walk. Tasks run depth first; a task that has
/// inner candidates pushes its follow-up tasks, so no Rust call stack grows with the
/// named-`uses` nesting.
enum MissingTask<'a> {
    /// Walk the rule root `root` of `plan`, writing candidates to buffer `dest`.
    Walk {
        plan: &'a ExecutionPlan,
        ctx: Option<&'a EvaluationContext>,
        root: NormalFormId,
        dest: usize,
    },
    /// Walk named-`uses` rule `path` of `plan`: its template walk is collected into a
    /// fresh buffer, then mapped through the import overlay by [`MissingTask::MapInner`].
    ImportRule {
        plan: &'a ExecutionPlan,
        ctx: Option<&'a EvaluationContext>,
        path: RulePath,
        dest: usize,
    },
    /// Map the candidates of buffer `inner` (collected for the import `marker` of `plan`)
    /// through its overlay into buffer `dest`.
    MapInner {
        plan: &'a ExecutionPlan,
        ctx: Option<&'a EvaluationContext>,
        marker: DataPath,
        identity: &'a SpecIdentity,
        overlay: &'a IndexMap<DataPath, ImportOverlay>,
        inner: usize,
        dest: usize,
    },
    Emit {
        candidate: MissingCandidate,
        dest: usize,
    },
}

/// Live inputs under `root` on `plan`, in evaluation order. `ctx` is the context that
/// evaluated `plan`; `None` when the enclosing import rule cell never ran, so no
/// slot is filled and liveness is that of unevaluated cells.
fn missing_candidates(
    plan: &ExecutionPlan,
    ctx: Option<&EvaluationContext>,
    root: NormalFormId,
    out: &mut Vec<MissingCandidate>,
) {
    let mut buffers: Vec<Vec<MissingCandidate>> = vec![Vec::new()];
    let mut stack = vec![MissingTask::Walk {
        plan,
        ctx,
        root,
        dest: 0,
    }];
    while let Some(task) = stack.pop() {
        let mut next = Vec::new();
        match task {
            MissingTask::Walk {
                plan,
                ctx,
                root,
                dest,
            } => {
                let values: &[Option<OperationResult>] = match ctx {
                    Some(ctx) => &ctx.values,
                    None => &[],
                };
                for input in reachable_data_paths(plan, root, values) {
                    match input {
                        ReachableInput::Data(path) => {
                            if let Some(promptable) = plan.promptable_data_path(&path).cloned() {
                                next.push(MissingTask::Emit {
                                    candidate: MissingCandidate::Data(promptable),
                                    dest,
                                });
                            }
                        }
                        ReachableInput::RowReads(id) => {
                            if let Some(ctx) = ctx {
                                for key in ctx.row_missing.get(&id).into_iter().flatten() {
                                    next.push(MissingTask::Emit {
                                        candidate: MissingCandidate::Row(key.clone()),
                                        dest,
                                    });
                                }
                            }
                        }
                        ReachableInput::ImportRule(path) => {
                            next.push(MissingTask::ImportRule {
                                plan,
                                ctx,
                                path,
                                dest,
                            });
                        }
                    }
                }
            }
            MissingTask::ImportRule {
                plan,
                ctx,
                path,
                dest,
            } => {
                let (first, rest) = path.segments.split_first().unwrap_or_else(|| {
                    panic!("BUG: import rule walk on local rule '{}'", path.rule)
                });
                let marker = DataPath::local(first.uses.clone());
                let Some(DataDefinition::Import {
                    identity, overlay, ..
                }) = plan.data.get(&marker)
                else {
                    panic!(
                        "BUG: import rule '{}' first hop is not Import",
                        path.input_key()
                    );
                };
                let template = plan.row_template(identity);
                let import_ctx = ctx.and_then(|ctx| ctx.import_contexts.get(&marker));
                let relative = RulePath {
                    segments: rest.to_vec(),
                    rule: path.rule.clone(),
                };
                let inner = buffers.len();
                buffers.push(Vec::new());
                let child = if relative.segments.is_empty() {
                    let root = template
                        .rules
                        .get(&relative)
                        .unwrap_or_else(|| {
                            panic!(
                                "BUG: import rule '{}' missing from its template",
                                path.input_key()
                            )
                        })
                        .normal_form;
                    MissingTask::Walk {
                        plan: template,
                        ctx: import_ctx,
                        root,
                        dest: inner,
                    }
                } else {
                    MissingTask::ImportRule {
                        plan: template,
                        ctx: import_ctx,
                        path: relative,
                        dest: inner,
                    }
                };
                stack.push(MissingTask::MapInner {
                    plan,
                    ctx,
                    marker,
                    identity,
                    overlay,
                    inner,
                    dest,
                });
                stack.push(child);
                continue;
            }
            MissingTask::MapInner {
                plan,
                ctx,
                marker,
                identity,
                overlay,
                inner,
                dest,
            } => {
                assert_eq!(
                    buffers.len(),
                    inner + 1,
                    "BUG: missing-candidate buffers closed out of order"
                );
                let collected = buffers.pop().expect("BUG: inner candidate buffer exists");
                for candidate in collected {
                    let field = match candidate {
                        MissingCandidate::Data(field) => field,
                        row @ MissingCandidate::Row(_) => {
                            next.push(MissingTask::Emit {
                                candidate: row,
                                dest,
                            });
                            continue;
                        }
                    };
                    let absolute = absolute_import_member_path(&marker, identity, &field);
                    match overlay.get(&field) {
                        Some(ImportOverlay::Literal(_)) => {}
                        Some(ImportOverlay::Reference { .. })
                            if ctx.is_some_and(|c| c.is_data_bound(plan, &absolute)) =>
                        {
                            // Evaluate override on the nested LHS; wire source not needed.
                        }
                        Some(ImportOverlay::Reference {
                            target: ReferenceTarget::Data(target),
                            ..
                        }) => {
                            if let Some(promptable) = plan.promptable_data_path(target).cloned() {
                                next.push(MissingTask::Emit {
                                    candidate: MissingCandidate::Data(promptable),
                                    dest,
                                });
                            }
                        }
                        Some(ImportOverlay::Reference {
                            target: ReferenceTarget::Rule(rule),
                            ..
                        }) => {
                            let root = plan
                                .rules
                                .get(rule)
                                .unwrap_or_else(|| {
                                    panic!(
                                        "BUG: overlay rule target '{rule}' missing from owner plan"
                                    )
                                })
                                .normal_form;
                            next.push(MissingTask::Walk {
                                plan,
                                ctx,
                                root,
                                dest,
                            });
                        }
                        None => next.push(MissingTask::Emit {
                            candidate: MissingCandidate::Data(absolute),
                            dest,
                        }),
                    }
                }
            }
            MissingTask::Emit { candidate, dest } => {
                buffers[dest].push(candidate);
                continue;
            }
        }
        stack.extend(next.into_iter().rev());
    }
    out.extend(buffers.pop().expect("BUG: root candidate buffer exists"));
}

/// Evaluates Lemma rules within their spec context
#[derive(Default)]
pub(crate) struct Evaluator;

impl Evaluator {
    /// Evaluate an execution plan: walk each requested local rule's NormalForm,
    /// then report requested results.
    ///
    /// Rule references are evaluation boundaries: a dependency's value is read from
    /// [`EvaluationContext::rule_values`], never by re-entering its body.
    /// Unbound live inputs are reported per rule as `missing_data` (reachable
    /// under control decisions derived from filled slots, intersected with
    /// unbound promptable paths). When `explain` is true the walk is exhaustive
    /// and every rule with a value is narrated in plan order
    /// ([`narration::narrate_rule`]) before the response is built.
    pub(crate) fn evaluate(
        &self,
        plan: &ExecutionPlan,
        evaluate_data: &EvaluateData,
        now: LiteralValue,
        response_rules: &std::collections::HashSet<&RulePath>,
        explain: bool,
    ) -> Response {
        let effective = match &now.value {
            ValueKind::Date(date) => date.to_string(),
            other => panic!("BUG: evaluation now must be a date, got {other:?}"),
        };
        let mut context = EvaluationContext::new(plan, evaluate_data, now, explain);

        let requested: Vec<&crate::planning::execution_plan::ExecutableRule> = plan
            .rules
            .values()
            .filter(|rule| response_rules.contains(&rule.path))
            .collect();

        let mut response = Response {
            spec_name: plan.spec_name.clone(),
            effective,
            // Set by `Engine::evaluate` after evaluation from the plan's cached
            // version window (`ExecutionPlan::effective_from` / `effective_to`).
            spec_effective_from: None,
            spec_effective_to: None,
            results: IndexMap::with_capacity(requested.len()),
            rows: Vec::new(),
        };
        let mut rows = response::RowTable::default();

        for exec_rule in &requested {
            tree::ensure_rule_values(plan.rule_index(&exec_rule.path), plan, &mut context);
        }

        let mut arena = narration::ExplanationArena::default();
        if explain {
            narration::narrate_row_contexts(plan, &mut context, &mut arena);
            for (index, rule) in plan.rules.values().enumerate() {
                if context.rule_values[index].is_some() {
                    narration::narrate_rule(rule, plan, &mut context, &mut arena);
                }
            }
        }

        for exec_rule in requested {
            let result = context.rule_value(plan, &exec_rule.path).clone();
            let rule_type = context.rule_result_type(plan, exec_rule);

            let explanation = explain.then(|| {
                narration::explanation_for(
                    exec_rule,
                    plan,
                    &context,
                    &arena,
                    &result,
                    Arc::clone(&rule_type),
                )
            });

            let missing_data = if result.is_missing_data() {
                context.missing_data_for_rule(plan, exec_rule.normal_form)
            } else {
                Vec::new()
            };

            let mut rule_result = RuleResult::from_operation_result(
                EvaluatedRule {
                    name: exec_rule.path.input_key(),
                    path: exec_rule.path.clone(),
                    source_location: exec_rule.source.clone(),
                    rule_type: Arc::clone(&rule_type),
                },
                &result,
                rule_type.as_ref(),
                &plan.family_units,
                explanation,
                missing_data,
            );
            if let (OperationResult::Value(bound), Some(value)) =
                (&result, rule_result.result.as_mut())
            {
                response::fill_spec_values(
                    &bound.value,
                    value,
                    plan,
                    &mut context,
                    &mut rows,
                    &mut Vec::new(),
                );
            }
            response.add_result(rule_result);
        }
        response.rows = rows.into_rows();

        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsing::ast::DateTimeValue;
    use crate::Engine;

    #[test]
    fn reference_runtime_value_carries_resolved_type_not_target_type() {
        let code = r#"
spec inner
data slot: number -> minimum 0 -> maximum 100

spec source_spec
data v: 5

spec outer
uses i: inner
  -> with slot: src.v
uses src: source_spec
rule r: i.slot
"#;
        let mut engine = Engine::new();
        engine
            .load([(
                crate::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                    "ref_invariant.lemma",
                ))),
                code.to_string(),
            )])
            .expect("must load");

        let plan_basis = engine
            .plans
            .get_plans(None, "outer")
            .and_then(|plans| plans.values().next())
            .expect("must plan");

        let reference_path = DataPath {
            segments: vec![crate::planning::semantics::PathSegment {
                uses: "i".to_string(),
                repository: None,
                spec: "inner".to_string(),
            }],
            data: "slot".to_string(),
        };
        assert!(
            !plan_basis.data.contains_key(&reference_path),
            "named-`uses` member must not be materialized into consumer data"
        );

        let resolved_type = crate::planning::normalize::lookup_data_definition(
            &plan_basis.data,
            &plan_basis.row_templates,
            &reference_path,
        )
        .and_then(DataDefinition::resolved_type_arc)
        .map(Arc::clone)
        .expect("`i.slot` must peel to typed template data");

        let evaluate_data = EvaluateData::default();

        let now = DateTimeValue::now();
        let now_lit = LiteralValue {
            value: crate::planning::semantics::ValueKind::Date(
                crate::planning::semantics::date_time_to_semantic(&now),
            ),
        };
        let mut context = EvaluationContext::new(plan_basis, &evaluate_data, now_lit, false);

        let stored = crate::evaluation::imports::eval_import_member(
            &reference_path,
            plan_basis,
            &mut context,
        );

        let OperationResult::Value(value) = stored else {
            panic!("reference path must hold a value, got {stored:?}");
        };

        // Type lives on the template field (LHS of the binding), not LiteralValue.
        assert!(
            matches!(
                resolved_type.specifications,
                crate::planning::semantics::TypeSpecification::Number {
                    minimum: Some(_),
                    maximum: Some(_),
                    ..
                }
            ),
            "reference resolved_type must keep LHS constraints, got {:?}",
            resolved_type.specifications
        );
        assert!(
            matches!(
                value.value,
                crate::planning::semantics::ValueKind::Number(_)
            ),
            "stored value must be the copied number, got {:?}",
            value.value
        );
    }

    #[test]
    fn ignored_under_prefix_strips_dotted_prefix_only() {
        let ignored = vec![
            "p.amountt".to_string(),
            "P.Qty".to_string(),
            "pq.other".to_string(),
            "p.".to_string(),
            "p".to_string(),
        ];
        assert_eq!(
            ignored_under_prefix(&ignored, "p"),
            vec!["amountt".to_string(), "qty".to_string()]
        );
    }
}

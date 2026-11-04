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
pub(crate) mod list_ops;
pub(crate) mod narration;
pub mod response;
pub(crate) mod rows;
pub(crate) mod tree;

pub use crate::computation::OperationResult;
use crate::computation::VetoType;
use crate::evaluation::response::EvaluatedRule;
use crate::planning::execution_plan::{
    reachable_data_paths, ExecutableRule, ExecutionPlan, ReachableInput,
};
use crate::planning::normalize::{LeafKind, NormalFormId, NormalFormKind};
use crate::planning::semantics::{
    lemma_type_arc_with_display_binding, BoundValueKind, DataDefinition, DataPath, InstanceKey,
    LemmaType, ListValue, LiteralValue, ReferenceEnd, ReferenceTarget, RowBinding, RowEnd,
    RulePath, TypedLiteral, ValueKind,
};
use crate::string_distance::closest_name;
pub use evaluate_data::{EvaluateData, EvaluateDataValue};
use indexmap::IndexMap;
pub use response::{Response, RuleResult};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

fn closest_ignored_key(needed: &str, ignored: &[String]) -> Option<String> {
    closest_name(needed, ignored)
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
    /// Evaluation policy: visit every cell narration displays (rewrite
    /// pre-images, arithmetic siblings after a definitive veto). Set for explain
    /// runs. Never changes a value. Unless arms after the winner are already
    /// visited by the last-wins reverse scan.
    pub(crate) exhaustive: bool,
    /// Explain runs only: arena ids of narrated rules in this context, filled in
    /// plan order after the walk. Interior mutability: row contexts narrate
    /// through shared borrows of [`RequestCaches::row_contexts`].
    pub(crate) rule_explanations: RefCell<HashMap<RulePath, narration::ArenaId>>,
    /// Current row of each enclosing list command, by level (`Value` or `Instance`).
    pub(crate) row_stack: Vec<CurrentRow>,
    /// The caches of this request. Cleared when the response is built: a cached
    /// row context holds the same handle, so it would otherwise cycle and leak.
    pub(crate) caches: Rc<RequestCaches>,
    /// The row instance whose context this is; `None` for the root context. Stored-row
    /// references defer with this owner so their targets resolve in the scope
    /// that materialized the list even when the read lands in a deeper scope.
    pub(crate) owner_row: Option<InstanceKey>,
    /// Missing row field keys in read order; may repeat.
    pub(crate) missing_row_keys: Vec<String>,
    /// Data-leaf cells whose rule-backed options have not been applied yet.
    /// `ready` does not return their prefilled binding until that check runs.
    unchecked_option_leaves: HashSet<usize>,
    /// Missing row field keys read under each row-reading cell, deduplicated.
    pub(crate) row_missing: HashMap<NormalFormId, Vec<String>>,
    /// Exhaustive runs only: row field reads in read order; may repeat.
    pub(crate) row_reads_log: Vec<rows::RowRead>,
    /// Exhaustive runs only: row field reads under each row-reading cell, deduplicated.
    pub(crate) row_reads: HashMap<NormalFormId, Vec<rows::RowRead>>,
}

#[derive(Clone)]
pub(crate) enum CurrentRow {
    Value(TypedLiteral),
    /// A row of an instance list, read through its template context. `position` is its
    /// 0-based place in the iterated list.
    Instance {
        key: InstanceKey,
        position: usize,
    },
}

/// Lengths of the row logs before a row-reading cell evaluates.
#[derive(Clone, Copy)]
pub(crate) struct RowLogMark {
    missing: usize,
    reads: usize,
}

/// The caches of one request, shared by every layer of the walk so a lookup never
/// depends on which scope filled the cache.
pub(crate) struct RequestCaches {
    /// Template-plan context of every row read so far: a row's context is cached
    /// here no matter how deep the scope that produced it, and re-entered from
    /// wherever a read lands.
    pub(crate) row_contexts: RefCell<IndexMap<InstanceKey, EvaluationContext>>,
    /// Run-data values for unset fields of rows the plan holds (`i.x`, `staff.1.salary`),
    /// by row and template slot. Bound in the row's context when it is built.
    row_overrides: HashMap<InstanceKey, Vec<(usize, RowBinding)>>,
    /// Stored lists, by owning row and path, whose rows have a key field set by run
    /// data: their store index does not hold the run-data keys.
    key_overridden_lists: HashSet<(Option<InstanceKey>, DataPath)>,
}

impl RequestCaches {
    /// Caches of a request of `plan` with `evaluate_data` bound.
    fn new(evaluate_data: &EvaluateData) -> Rc<Self> {
        let overrides = &evaluate_data.row_overrides;
        Rc::new(Self {
            row_contexts: RefCell::new(IndexMap::new()),
            row_overrides: overrides
                .fields
                .iter()
                .map(|(key, fields)| {
                    (
                        key.clone(),
                        fields
                            .iter()
                            .map(|(&slot, value)| (slot, RowBinding::Bound(value.clone())))
                            .collect(),
                    )
                })
                .collect(),
            key_overridden_lists: overrides.keyed_lists.clone(),
        })
    }

    /// Binding of template slot `slot` of row `key` in this request: the row's own,
    /// or the run-data value of a field the row leaves unset.
    pub(crate) fn row_binding<'a>(
        &'a self,
        key: &'a InstanceKey,
        slot: usize,
    ) -> Option<&'a RowBinding> {
        match &key.row.bindings[slot] {
            Some(binding) => Some(binding),
            None if self.row_overrides.is_empty() => None,
            None => self
                .row_overrides
                .get(key)?
                .iter()
                .find(|(overridden, _)| *overridden == slot)
                .map(|(_, binding)| binding),
        }
    }

    pub(crate) fn key_overridden(&self, owner: &Option<InstanceKey>, list: &DataPath) -> bool {
        !self.key_overridden_lists.is_empty()
            && self
                .key_overridden_lists
                .contains(&(owner.clone(), list.clone()))
    }
}

impl EvaluationContext {
    /// Context of a row of `template`, nested in this one, with `bindings` bound:
    /// values by template data slot, in slot order.
    pub(crate) fn child_row(
        &self,
        template: &ExecutionPlan,
        bindings: &[(usize, OperationResult)],
    ) -> Self {
        assert!(
            bindings.windows(2).all(|pair| pair[0].0 < pair[1].0),
            "BUG: row bindings of '{}' out of slot order",
            template.spec_name
        );
        let mut values = Self::template_values(template);
        if !bindings.is_empty() {
            for (slot, binding) in bindings {
                values[template.data_slot_leaves[*slot].index()] = Some(binding.clone());
            }
            template.copy_reference_targets(&mut values, |slot, _| {
                bindings
                    .binary_search_by_key(&slot, |(bound, _)| *bound)
                    .is_ok()
            });
        }
        Self::with_values(
            template,
            values,
            self.now.clone(),
            Vec::new(),
            self.exhaustive,
            Rc::clone(&self.caches),
        )
    }

    /// Context of `plan` with `evaluate_data` bound.
    fn new(
        plan: &ExecutionPlan,
        evaluate_data: &EvaluateData,
        now: LiteralValue,
        exhaustive: bool,
        caches: Rc<RequestCaches>,
    ) -> Self {
        let mut values = Self::template_values(plan);
        if !evaluate_data.bindings.is_empty() {
            for (slot, binding) in &evaluate_data.bindings {
                values[plan.data_slot_leaves[*slot].index()] = Some(binding.clone());
            }
            plan.copy_reference_targets(&mut values, |slot, _| {
                evaluate_data.bindings.contains_key(&slot)
            });
        }
        Self::with_values(
            plan,
            values,
            now,
            evaluate_data.ignored_unknown.clone(),
            exhaustive,
            caches,
        )
    }

    fn template_values(plan: &ExecutionPlan) -> Vec<Option<OperationResult>> {
        assert_eq!(
            plan.initial_values.len(),
            plan.normal_forms.len(),
            "BUG: plan evaluation tables not populated"
        );
        plan.initial_values.clone()
    }

    fn with_values(
        plan: &ExecutionPlan,
        values: Vec<Option<OperationResult>>,
        now: LiteralValue,
        ignored_unknown: Vec<String>,
        exhaustive: bool,
        caches: Rc<RequestCaches>,
    ) -> Self {
        let mut unchecked_option_leaves = HashSet::new();
        for &leaf in &plan.option_checked_leaves {
            if values[leaf.index()].is_some() {
                unchecked_option_leaves.insert(leaf.index());
            }
        }

        Self {
            values,
            rule_values: vec![None; plan.rules.len()],
            now,
            ignored_unknown,
            exhaustive,
            rule_explanations: RefCell::new(HashMap::new()),
            row_stack: Vec::new(),
            caches,
            owner_row: None,
            missing_row_keys: Vec::new(),
            unchecked_option_leaves,
            row_missing: HashMap::new(),
            row_reads_log: Vec::new(),
            row_reads: HashMap::new(),
        }
    }

    pub(crate) fn option_check_pending(&self, leaf: usize) -> bool {
        self.unchecked_option_leaves.contains(&leaf)
    }

    pub(crate) fn option_check_finished(&mut self, leaf: usize) {
        self.unchecked_option_leaves.remove(&leaf);
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
        let def = plan
            .data
            .get(path)
            .expect("BUG: data path leaf missing from plan.data");
        let schema = def
            .resolved_type_arc()
            .expect("BUG: data path leaf missing schema type");
        lemma_type_arc_with_display_binding(
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
        rule: &ExecutableRule,
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
                let def = plan
                    .data
                    .get(path)
                    .expect("BUG: rule DataPath leaf missing from plan.data");
                lemma_type_arc_with_display_binding(
                    planned,
                    settled,
                    def.bound_value(),
                    def.bound_suggestion(),
                )
            }
            _ => lemma_type_arc_with_display_binding(planned, settled, None, None),
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

    /// Bind a data-leaf slot left unbound when the context was built (a deferred row
    /// binding resolved after the row context already existed). Already-bound slots
    /// keep their value: resolution is deterministic, so it is the same value.
    pub(crate) fn bind_data_slot(
        &mut self,
        plan: &ExecutionPlan,
        data_path: &DataPath,
        value: OperationResult,
    ) {
        let leaf = *plan
            .data_leaf
            .get(data_path)
            .unwrap_or_else(|| panic!("BUG: data path '{data_path}' has no data_leaf entry"));
        if self.values[leaf.index()].is_none() {
            self.values[leaf.index()] = Some(value);
        }
    }

    pub(crate) fn missing_data_suggestion(&self, data_path: &DataPath) -> Option<String> {
        closest_ignored_key(&data_path.input_key(), &self.ignored_unknown)
    }

    /// Promptable data paths this rule still needs, in evaluation / decision-tree order.
    ///
    /// First key is the next fact the live tree needs. Walks from `rule_root` deriving
    /// liveness from filled condition/scrutinee slots, maps each leaf through
    /// [`ExecutionPlan::promptable_data_path`], and keeps unbound keys, and the key of a
    /// slot a deferred binding filled with missing data of the context that wrote it.
    /// When this context is `instance`'s, its own data keys are named under the
    /// instance's path; an unset field of an inline row has no key (the rule's veto
    /// names it). Row keys are recorded named already.
    pub(crate) fn missing_data_for_rule(
        &self,
        plan: &ExecutionPlan,
        rule_root: NormalFormId,
        instance: Option<&InstanceKey>,
    ) -> Vec<String> {
        self.collect_missing(
            missing_candidates(plan, self, rule_root, None),
            plan,
            instance,
        )
    }

    /// Live inputs of `rule_root` on `template` when `instance`'s context was never
    /// built: the walk names each input under the instance.
    pub(crate) fn missing_data_for_unreached_instance(
        &self,
        root_plan: &ExecutionPlan,
        template: &ExecutionPlan,
        rule_root: NormalFormId,
        instance: &InstanceKey,
    ) -> Vec<String> {
        self.collect_missing(
            missing_candidates(root_plan, self, rule_root, Some((instance, template, None))),
            template,
            Some(instance),
        )
    }

    fn collect_missing(
        &self,
        candidates: Vec<MissingCandidate>,
        plan: &ExecutionPlan,
        instance: Option<&InstanceKey>,
    ) -> Vec<String> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for candidate in candidates {
            let key = match candidate {
                MissingCandidate::Data(path) => match self.data_slot(plan, &path) {
                    Some(OperationResult::Veto(VetoType::MissingRowData { key })) => key.clone(),
                    Some(OperationResult::Veto(VetoType::MissingData { data, .. })) => {
                        data.input_key()
                    }
                    Some(_) => continue,
                    None => match instance {
                        None => path.input_key(),
                        Some(instance) => match rows::row_key(instance, &path.input_key()) {
                            Some(key) => key,
                            None => continue,
                        },
                    },
                },
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

/// Live inputs under `root` on `plan`, in evaluation order, as evaluated in `ctx`.
///
/// A member read of a `uses` instance the evaluation did not reach (an arm a missing
/// condition left undecided) has the live inputs of the member in the instance, walked
/// in its context when built, else over its template: each named under the instance.
///
/// `at` starts the walk on a `uses` instance: its template and context (`None` when
/// the context was never built).
fn missing_candidates<'a>(
    plan: &'a ExecutionPlan,
    ctx: &'a EvaluationContext,
    root: NormalFormId,
    at: Option<(
        &'a InstanceKey,
        &'a ExecutionPlan,
        Option<&'a EvaluationContext>,
    )>,
) -> Vec<MissingCandidate> {
    struct Walk<'c> {
        inputs: std::vec::IntoIter<ReachableInput>,
        template: &'c ExecutionPlan,
        /// The instance walked; `None` for `ctx` itself.
        instance: Option<InstanceKey>,
        /// Context the walk reads values in: `ctx`, or the instance's when built.
        context: Option<&'c EvaluationContext>,
    }

    let contexts = ctx.caches.row_contexts.borrow();
    let mut out = Vec::new();
    let mut walks = match at {
        None => vec![Walk {
            inputs: reachable_data_paths(plan, root, &ctx.values).into_iter(),
            template: plan,
            instance: None,
            context: Some(ctx),
        }],
        Some((instance, template, context)) => {
            let values: &[Option<OperationResult>] = match context {
                Some(context) => &context.values,
                None => &[],
            };
            vec![Walk {
                inputs: reachable_data_paths(template, root, values).into_iter(),
                template,
                instance: Some(instance.clone()),
                context,
            }]
        }
    };
    while let Some(walk) = walks.last_mut() {
        let Some(input) = walk.inputs.next() else {
            walks.pop();
            continue;
        };
        let template = walk.template;
        match input {
            ReachableInput::Data(path) => {
                let Some(promptable) = template.promptable_data_path(&path) else {
                    continue;
                };
                let Some(instance) = &walk.instance else {
                    out.push(MissingCandidate::Data(promptable.clone()));
                    continue;
                };
                let key = rows::row_key(instance, &promptable.input_key());
                match walk.context {
                    Some(context) => match context.data_slot(template, promptable) {
                        Some(OperationResult::Veto(VetoType::MissingRowData { key })) => {
                            out.push(MissingCandidate::Row(key.clone()));
                        }
                        Some(OperationResult::Veto(VetoType::MissingData { data, .. })) => {
                            out.push(MissingCandidate::Row(data.input_key()));
                        }
                        Some(_) => {}
                        None => out.extend(key.map(MissingCandidate::Row)),
                    },
                    None => {
                        let slot = template
                            .data
                            .get_index_of(promptable)
                            .expect("BUG: promptable path is data of its template");
                        match ctx.caches.row_binding(instance, slot) {
                            None => {
                                let leaf = template
                                    .data_leaf
                                    .get(promptable)
                                    .expect("BUG: promptable path has a data_leaf entry");
                                if template.initial_values[leaf.index()].is_none() {
                                    out.extend(key.map(MissingCandidate::Row));
                                }
                            }
                            Some(RowBinding::Deferred { target, hops }) => {
                                let owner = instance.scope(*hops);
                                let owner_plan = match owner {
                                    None => plan,
                                    Some(owner_key) => plan.row_template(&owner_key.row.identity),
                                };
                                let owner_ctx = match owner {
                                    None => Some(ctx),
                                    Some(owner_key) => contexts.get(owner_key),
                                };
                                let owner_instance = owner.cloned();
                                match target {
                                    ReferenceTarget::Data(path) => {
                                        if let Some(promptable) =
                                            owner_plan.promptable_data_path(path)
                                        {
                                            match &owner_instance {
                                                None => out.push(MissingCandidate::Data(
                                                    promptable.clone(),
                                                )),
                                                Some(owner_key) => {
                                                    out.extend(
                                                        rows::row_key(
                                                            owner_key,
                                                            &promptable.input_key(),
                                                        )
                                                        .map(MissingCandidate::Row),
                                                    );
                                                }
                                            }
                                        } else if let Some(ReferenceEnd::Rule(rule)) =
                                            owner_plan.reference_ends.get(path)
                                        {
                                            let body = owner_plan
                                                .rules
                                                .get(rule)
                                                .unwrap_or_else(|| {
                                                    panic!(
                                                        "BUG: deferred data target '{path}' ends at missing rule '{rule}'"
                                                    )
                                                })
                                                .normal_form;
                                            let values: &[Option<OperationResult>] = match owner_ctx
                                            {
                                                Some(context) => &context.values,
                                                None => &[],
                                            };
                                            walks.push(Walk {
                                                inputs: reachable_data_paths(
                                                    owner_plan, body, values,
                                                )
                                                .into_iter(),
                                                template: owner_plan,
                                                instance: owner_instance,
                                                context: owner_ctx,
                                            });
                                        }
                                    }
                                    ReferenceTarget::Rule(rule) => {
                                        let body = owner_plan
                                            .rules
                                            .get(rule)
                                            .unwrap_or_else(|| {
                                                panic!(
                                                    "BUG: deferred rule target '{rule}' missing from its owner plan"
                                                )
                                            })
                                            .normal_form;
                                        let values: &[Option<OperationResult>] = match owner_ctx {
                                            Some(context) => &context.values,
                                            None => &[],
                                        };
                                        walks.push(Walk {
                                            inputs: reachable_data_paths(owner_plan, body, values)
                                                .into_iter(),
                                            template: owner_plan,
                                            instance: owner_instance,
                                            context: owner_ctx,
                                        });
                                    }
                                }
                            }
                            Some(
                                RowBinding::Bound(_)
                                | RowBinding::Rows { .. }
                                | RowBinding::Instance(_),
                            ) => {}
                        }
                    }
                }
            }
            ReachableInput::RowReads(id) => {
                if let Some(context) = walk.context {
                    if let Some(keys) = context.row_missing.get(&id) {
                        out.extend(keys.iter().cloned().map(MissingCandidate::Row));
                        continue;
                    }
                    if context.values[id.index()].is_some() {
                        continue;
                    }
                }
                let NormalFormKind::SpecMember { base, row, .. } = &template.normal_form(id).kind
                else {
                    continue;
                };
                let NormalFormKind::Leaf(LeafKind::DataPath(slot)) =
                    &template.normal_form(*base).kind
                else {
                    continue;
                };
                if !matches!(template.data.get(slot), Some(DataDefinition::Import { .. })) {
                    continue;
                }
                let holder = match &walk.instance {
                    Some(instance) => Some(instance.clone()),
                    None => ctx.owner_row.clone(),
                };
                let (end, hops) = row
                    .ends
                    .split_last()
                    .expect("BUG: a member read has a last end");
                let mut instance = InstanceKey::new(
                    Arc::clone(
                        template.import_row(holder.as_ref().map(|key| key.row.as_ref()), slot),
                    ),
                    holder,
                );
                let mut member_template = template.row_template(&instance.row.identity);
                for hop in hops {
                    let RowEnd::Data(hop) = hop else {
                        panic!("BUG: member hop '{hop:?}' is a rule, not a `uses` slot");
                    };
                    instance = InstanceKey::new(
                        Arc::clone(member_template.import_row(Some(&instance.row), hop)),
                        Some(instance),
                    );
                    member_template = template.row_template(&instance.row.identity);
                }
                let context = contexts.get(&instance);
                let inputs = match end {
                    RowEnd::Data(path) => vec![ReachableInput::Data(path.clone())],
                    RowEnd::Rule(rule) => {
                        let body = member_template
                            .rules
                            .get(rule)
                            .unwrap_or_else(|| {
                                panic!("BUG: member rule '{rule}' missing from its template")
                            })
                            .normal_form;
                        let values: &[Option<OperationResult>] = match context {
                            Some(context) => &context.values,
                            None => &[],
                        };
                        reachable_data_paths(member_template, body, values)
                    }
                };
                walks.push(Walk {
                    inputs: inputs.into_iter(),
                    template: member_template,
                    instance: Some(instance),
                    context,
                });
            }
        }
    }
    out
}

fn closed_missing(
    ctx: &mut EvaluationContext,
    plan: &ExecutionPlan,
    keys: Vec<String>,
) -> (Vec<String>, IndexMap<String, Vec<EvaluateDataValue>>) {
    let mut keys = keys;
    let mut missing_data_options = IndexMap::new();
    let mut index = 0;
    while index < keys.len() {
        let key = keys[index].clone();
        let Some(path) = plan
            .data
            .keys()
            .find(|path| path.input_key() == key)
            .cloned()
        else {
            if let Some((template, definition, instance, field)) =
                evaluate_data::row_field_definition(plan, &key)
            {
                if let Some(lemma_type) = definition.schema_type() {
                    if let Some(rule_name) = lemma_type.options_rule() {
                        let rule_path = template.options_rule_of(field).unwrap_or_else(|| {
                            panic!(
                                "BUG: options rule '{rule_name}' of '{key}' missing after planning"
                            )
                        });
                        let rule = template.rules.get(&rule_path).unwrap_or_else(|| {
                            panic!(
                                "BUG: options rule '{rule_path}' missing after options_rule_of asserted it"
                            )
                        });
                        let built = ctx.caches.row_contexts.borrow().contains_key(&instance);
                        if built {
                            let contexts = ctx.caches.row_contexts.borrow();
                            let instance_ctx = contexts.get(&instance).unwrap_or_else(|| {
                                panic!("BUG: instance context of '{key}' vanished under borrow")
                            });
                            if let Some(rows) =
                                options_rule_list(instance_ctx, template, &rule_path)
                            {
                                missing_data_options
                                    .insert(key, options_for(lemma_type, Some(&rows)));
                                index += 1;
                                continue;
                            }
                            let blockers = instance_ctx.missing_data_for_rule(
                                template,
                                rule.normal_form,
                                Some(&instance),
                            );
                            drop(contexts);
                            keys.remove(index);
                            for blocker in blockers.into_iter().rev() {
                                if !keys.iter().any(|existing| existing == &blocker) {
                                    keys.insert(index, blocker);
                                }
                            }
                        } else {
                            keys.remove(index);
                            let blockers = ctx.missing_data_for_unreached_instance(
                                plan,
                                template,
                                rule.normal_form,
                                &instance,
                            );
                            for blocker in blockers.into_iter().rev() {
                                if !keys.iter().any(|existing| existing == &blocker) {
                                    keys.insert(index, blocker);
                                }
                            }
                        }
                        continue;
                    }
                    let options = options_for(lemma_type, None);
                    if !options.is_empty() {
                        missing_data_options.insert(key, options);
                    }
                }
            }
            index += 1;
            continue;
        };
        let Some(lemma_type) = plan
            .data
            .get(&path)
            .and_then(|definition| definition.schema_type())
        else {
            index += 1;
            continue;
        };
        let Some(rule_name) = lemma_type.options_rule() else {
            let options = options_for(lemma_type, None);
            if !options.is_empty() {
                missing_data_options.insert(key, options);
            }
            index += 1;
            continue;
        };
        let rule_path = plan.options_rule_of(&path).unwrap_or_else(|| {
            panic!("BUG: options rule '{rule_name}' of '{key}' missing after planning")
        });
        match options_rule_list(ctx, plan, &rule_path) {
            Some(rows) => {
                missing_data_options.insert(key, options_for(lemma_type, Some(&rows)));
                index += 1;
            }
            None => {
                keys.remove(index);
                let rule = plan
                    .rules
                    .get(&rule_path)
                    .expect("BUG: options rule missing after options_rule_of asserted it");
                let blockers = ctx.missing_data_for_rule(plan, rule.normal_form, None);
                for blocker in blockers.into_iter().rev() {
                    if !keys.iter().any(|existing| existing == &blocker) {
                        keys.insert(index, blocker);
                    }
                }
            }
        }
    }
    (keys, missing_data_options)
}

fn options_rule_list(
    ctx: &EvaluationContext,
    plan: &ExecutionPlan,
    rule_path: &RulePath,
) -> Option<ListValue> {
    let index = plan.rules.get_index_of(rule_path)?;
    let result = ctx.rule_values.get(index)?.as_ref()?;
    match result.value().map(|bound| &bound.value) {
        Some(ValueKind::List(rows)) => Some(rows.clone()),
        _ => None,
    }
}

fn options_for(lemma_type: &LemmaType, rule_rows: Option<&ListValue>) -> Vec<EvaluateDataValue> {
    let mut options = Vec::new();
    push_type_options(lemma_type, &mut options);
    if let crate::planning::semantics::TypeSpecification::List { element, .. } =
        &lemma_type.specifications
    {
        push_type_options(element, &mut options);
    }
    if let Some(rows) = rule_rows {
        for row in rows.iter() {
            push_option(&mut options, publish_typed(&row));
        }
    }
    options
}

fn push_type_options(lemma_type: &LemmaType, options: &mut Vec<EvaluateDataValue>) {
    for option in &lemma_type.options.values {
        push_option(options, publish_bound(lemma_type, &option.value));
    }
}

fn publish_bound(lemma_type: &LemmaType, bound: &BoundValueKind) -> EvaluateDataValue {
    match &bound.value {
        ValueKind::Boolean(bit) => EvaluateDataValue::Boolean(*bit),
        ValueKind::Spec(_) => unreachable!("BUG: {SPEC_OPTIONS_NEVER_MISSING}"),
        _ => EvaluateDataValue::Literal(lemma_type.display_bound(bound)),
    }
}

fn publish_typed(typed: &TypedLiteral) -> EvaluateDataValue {
    match &typed.value {
        ValueKind::Boolean(bit) => EvaluateDataValue::Boolean(*bit),
        ValueKind::Spec(_) => unreachable!("BUG: {SPEC_OPTIONS_NEVER_MISSING}"),
        _ => EvaluateDataValue::Literal(typed.display_value()),
    }
}

const SPEC_OPTIONS_NEVER_MISSING: &str = "missing-data options hold no spec value: a spec type is only a list element, a list of spec instances takes no options, and a spec list has its rows (possibly none) and is never missing data";

fn push_option(options: &mut Vec<EvaluateDataValue>, option: EvaluateDataValue) {
    if options.iter().any(|existing| existing == &option) {
        return;
    }
    options.push(option);
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
        requested: Vec<&RulePath>,
        explain: bool,
    ) -> Response {
        let effective = match &now.value {
            ValueKind::Date(date) => date.to_string(),
            other => panic!("BUG: evaluation now must be a date, got {other:?}"),
        };
        let mut context = EvaluationContext::new(
            plan,
            evaluate_data,
            now,
            explain,
            RequestCaches::new(evaluate_data),
        );

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

        // A requested `uses` import rule reads on its instance: the key, the template and
        // the value the read delivers.
        let mut import_reads: HashMap<&RulePath, (InstanceKey, &ExecutionPlan, OperationResult)> =
            HashMap::new();
        for path in &requested {
            if plan.rules.contains_key(*path) {
                tree::ensure_rule_values(plan.rule_index(path), plan, &mut context);
                continue;
            }
            let (identity, rule) = plan
                .show_import_rules
                .get(*path)
                .expect("BUG: a Show rule is local or a reached import rule");
            let mut key: Option<InstanceKey> = None;
            let mut holder = plan;
            for segment in &path.segments {
                let slot = DataPath::local(segment.uses.to_string());
                let row =
                    Arc::clone(holder.import_row(key.as_ref().map(|key| key.row.as_ref()), &slot));
                holder = plan.row_template(&row.identity);
                key = Some(InstanceKey::new(row, key));
            }
            let key = key.expect("BUG: an import rule path has `uses` hops");
            assert_eq!(
                &*key.row.identity, identity,
                "BUG: import rule '{path}' hops land on another spec"
            );
            let value = tree::read_instance_rule(&key, rule, plan, &mut context);
            import_reads.insert(path, (key, holder, value));
        }

        let mut arena = narration::ExplanationArena::default();
        if explain {
            narration::narrate_row_contexts(plan, &context, &mut arena);
            for (index, rule) in plan.rules.values().enumerate() {
                if context.rule_values[index].is_some() {
                    narration::narrate_rule(rule, plan, &context, &mut arena, None);
                }
            }
        }

        for path in requested {
            let caches = Rc::clone(&context.caches);
            let (rule_type, explanation, missing_keys, source, result) = {
                let contexts = caches.row_contexts.borrow();
                let (rule_plan, rule_context, exec_rule, result, instance) =
                    match import_reads.get(path) {
                        None => (
                            plan,
                            &context,
                            &plan.rules[path],
                            context.rule_value(plan, path).clone(),
                            None,
                        ),
                        Some((key, template, value)) => {
                            let (_, rule) = &plan.show_import_rules[path];
                            (
                                *template,
                                contexts
                                    .get(key)
                                    .expect("BUG: import rule context cached by its read"),
                                &template.rules[rule],
                                value.clone(),
                                Some(key),
                            )
                        }
                    };
                let rule_type = rule_context.rule_result_type(rule_plan, exec_rule);
                let explanation = explain.then(|| {
                    narration::explanation_for(
                        path,
                        exec_rule,
                        rule_plan,
                        rule_context,
                        &arena,
                        &result,
                        Arc::clone(&rule_type),
                    )
                });
                let missing_keys = result.is_missing_data().then(|| {
                    let mut keys = rule_context.missing_data_for_rule(
                        rule_plan,
                        exec_rule.normal_form,
                        instance,
                    );
                    // The veto itself names the live hole when the walk did not re-derive it
                    // (e.g. instance-list `where` short-circuit on a row MissingRowData).
                    if keys.is_empty() {
                        match &result {
                            OperationResult::Veto(VetoType::MissingRowData { key }) => {
                                keys.push(key.clone());
                            }
                            OperationResult::Veto(VetoType::MissingData { data, .. }) => {
                                keys.push(data.input_key());
                            }
                            OperationResult::Value(_) | OperationResult::Veto(_) => {}
                        }
                    }
                    keys
                });
                (
                    rule_type,
                    explanation,
                    missing_keys,
                    exec_rule.source.clone(),
                    result,
                )
            };

            let (missing_data, missing_data_options) = match missing_keys {
                Some(keys) => closed_missing(&mut context, plan, keys),
                None => (Vec::new(), IndexMap::new()),
            };

            let mut rule_result = RuleResult::from_operation_result(
                EvaluatedRule {
                    name: path.input_key(),
                    path: path.clone(),
                    source_location: source,
                    rule_type: Arc::clone(&rule_type),
                },
                &result,
                rule_type.as_ref(),
                &plan.family_units,
                explanation,
                missing_data,
            )
            .with_missing_data_options(missing_data_options);
            if let (OperationResult::Value(bound), Some(value)) =
                (&result, rule_result.result.as_mut())
            {
                response::fill_spec_values(&bound.value, value, plan, &mut context, &mut rows);
            }
            response.add_result(rule_result);
        }
        response.rows = rows.into_rows();
        // Break the handle cycle: cached contexts hold this same map.
        context.caches.row_contexts.borrow_mut().clear();

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

        let crate::planning::semantics::DataDefinition::Import { row, .. } = plan_basis
            .data
            .get(&DataPath::local("i".to_string()))
            .expect("`uses i` is data of the consumer plan")
        else {
            panic!("`i` must be an import");
        };
        let template = plan_basis.row_template(&row.identity);
        let slot_path = DataPath::local("slot".to_string());
        assert!(
            matches!(
                row.get_binding(template, &slot_path),
                Some(RowBinding::Deferred { hops: 0, .. })
            ),
            "`slot` of `i` is bound by `uses -> with` to a reference of the consumer"
        );
        let resolved_type = template
            .data
            .get(&slot_path)
            .and_then(|definition| definition.resolved_type_arc())
            .map(Arc::clone)
            .expect("`slot` must be typed by the imported field");

        let evaluate_data = EvaluateData::default();

        let now = DateTimeValue::now();
        let now_lit = LiteralValue {
            value: crate::planning::semantics::ValueKind::Date(
                crate::planning::semantics::date_time_to_semantic(&now),
            ),
        };
        let mut context = EvaluationContext::new(
            plan_basis,
            &evaluate_data,
            now_lit,
            false,
            RequestCaches::new(&evaluate_data),
        );
        let rule = RulePath::new(Vec::new(), "r".to_string());
        tree::ensure_rule_values(plan_basis.rule_index(&rule), plan_basis, &mut context);
        let stored = context.rule_value(plan_basis, &rule).clone();

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
}

//! Narration: builds [`ExplanationNode`] trees from a filled value table.
//!
//! Runs after the value walk ([`crate::evaluation::tree`]) under the exhaustive
//! policy. Pure over the plan, `values`, `rule_values` and the Rule nodes of
//! already narrated rules; evaluates nothing. Every cell it displays was visited
//! by the walk, so an empty slot here is a bug.
//!
//! Structure follows the expression the user wrote: a rewritten cell narrates
//! its pre-image (`origin`). A cell whose pre-image is a Piecewise takes its
//! causes from that record: not-taken arms after the winner (last-wins scan),
//! then the winner. An exclusive point OrderedDispatch keeps only the winner.

use crate::computation::OperationResult;
use crate::evaluation::branch_semantics::{
    condition_outcome, piecewise_decision, BranchOutcome, PiecewiseDecision,
};
use crate::evaluation::conversion_trace::build_conversion_steps;
use crate::evaluation::explanations::{
    explanation_result_value, format_operation_result, Explanation,
};
use crate::evaluation::expression::resolve_data_path_value;
use crate::evaluation::rows::{ReadRow, RowRead};
use crate::evaluation::tree::{self, borrow_value};
use crate::evaluation::EvaluationContext;
use crate::formatting::expression::{display_comparison, CommandSyntax, SyntaxNode, SyntaxView};
use crate::parsing::ast::{ArithmeticComputation, ComparisonComputation, LogicalConnective};
use crate::planning::execution_plan::{
    resolve_nested_dispatch_body, ExecutableRule, ExecutionPlan,
};
use crate::planning::explanation::{
    Cause, ExplainedListOp, ExplainedRule, ExplainedRuleId, ExplanationNode,
};
use crate::planning::instance_list::{default_of, row_data_type, row_end_type};
use crate::planning::normalize::{
    explanation_display, LeafKind, NameScope, NfNode, NormalForm, NormalFormId, NormalFormKind,
    NormalFormListOp,
};
use crate::planning::ordered_dispatch::{is_exclusive_point_table, region_of_scrutinee};
use crate::planning::semantics::{
    negated_comparison, spec_member_input_key, BoundValueKind, DataDefinition, DataPath,
    InstanceKey, KeyField, LemmaType, ListKey, ListPredKind, PathSegment, ReferenceEnd,
    ReferenceTarget, RowBinding, RowEnd, RowOrigin, RulePath, SpecValue, TypeSpecification,
    ValueKind,
};
use crate::result_value::RuleResultValue;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Internal id of a narrated rule in an [`ExplanationArena`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ArenaId(usize);

/// One narrated rule body before public id remapping.
struct ArenaEntry {
    name: RulePath,
    result: RuleResultValue,
    /// `None` while reserved for a cyclic RuleRef; filled after the body walk.
    body: Option<String>,
    causes: Vec<Cause>,
    children: Vec<ExplanationNode>,
}

/// Shared store of narrated rules for one explain run (root + every row context).
#[derive(Default)]
pub(crate) struct ExplanationArena {
    entries: Vec<ArenaEntry>,
}

impl ExplanationArena {
    fn push(&mut self, entry: ArenaEntry) -> ArenaId {
        let id = ArenaId(self.entries.len());
        self.entries.push(entry);
        id
    }

    fn get(&self, id: ArenaId) -> &ArenaEntry {
        self.entries
            .get(id.0)
            .unwrap_or_else(|| panic!("BUG: arena id {} out of range", id.0))
    }

    fn get_mut(&mut self, id: ArenaId) -> &mut ArenaEntry {
        self.entries
            .get_mut(id.0)
            .unwrap_or_else(|| panic!("BUG: arena id {} out of range", id.0))
    }
}

/// Root plan/context and shared arena for one explain run. Deferred `uses -> with`
/// rule wires resolve in the owner scope (often the request root).
pub(crate) struct NarrationSession<'a> {
    pub(crate) root_plan: &'a ExecutionPlan,
    pub(crate) root_ctx: &'a EvaluationContext,
    pub(crate) arena: &'a mut ExplanationArena,
    /// Rules reserved during a body walk; drained iteratively so RuleRef does not
    /// call back into [`narrate_rule`] (recursion-check).
    pub(crate) pending: Vec<PendingNarration>,
}

/// A rule whose arena entry is reserved but whose body is not filled yet.
pub(crate) enum PendingNarration {
    Root { path: RulePath },
    Instance { key: InstanceKey, path: RulePath },
}

/// Narration of one cell.
#[derive(Clone)]
struct Narrated {
    /// Expression text shown under the rule line.
    body: String,
    causes: Vec<Cause>,
    /// How this cell appears as an operand of an enclosing Compose.
    as_operand: Option<ExplanationNode>,
}

/// Rule children from the cell's as_operand shape. Empty-operand Compose
/// (bare literal) yields no children; operation Compose stays intact.
fn operands_of(as_operand: &Option<ExplanationNode>) -> Vec<ExplanationNode> {
    match as_operand {
        Some(ExplanationNode::Compose { operands, .. }) if operands.is_empty() => Vec::new(),
        Some(node) => vec![node.clone()],
        None => Vec::new(),
    }
}

/// Narrate a rule whose value is stored and insert its arena id into
/// `rule_explanations`. Reserves the arena entry before the body so a cyclic
/// RuleRef (deferred wire through a root rule that reads back into this instance)
/// can resolve while the body is still being filled. Further rules discovered
/// mid-body are queued on [`NarrationSession::pending`] and filled in a loop.
pub(crate) fn narrate_rule(
    rule: &ExecutableRule,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
    qualifier: Option<&NameScope>,
    session: &mut NarrationSession<'_>,
) {
    fill_rule_body(rule, plan, ctx, qualifier, session);
    drain_pending(session);
}

fn drain_pending(session: &mut NarrationSession<'_>) {
    while let Some(item) = session.pending.pop() {
        fill_pending_rule(item, session);
    }
}

fn fill_pending_rule(item: PendingNarration, session: &mut NarrationSession<'_>) {
    match item {
        PendingNarration::Root { path } => {
            let plan = session.root_plan;
            let ctx = session.root_ctx;
            let Some(rule) = plan.rules.get(&path) else {
                panic!("BUG: pending root rule '{path}' missing from plan");
            };
            fill_rule_body(rule, plan, ctx, None, session);
        }
        PendingNarration::Instance { key, path } => {
            let qualifier = NameScope {
                data: key.segments().unwrap_or_else(|| key.import_hops()),
                rules: key.import_hops(),
            };
            let plan = session.root_plan.row_template(&key.row.identity);
            let contexts = session.root_ctx.caches.row_contexts.borrow();
            let ctx = contexts
                .get(&key)
                .unwrap_or_else(|| panic!("BUG: pending instance {key} missing from row_contexts"));
            let Some(rule) = plan.rules.get(&path) else {
                panic!("BUG: pending rule '{path}' of {key} missing from plan");
            };
            fill_rule_body(rule, plan, ctx, Some(&qualifier), session);
        }
    }
}

fn fill_rule_body(
    rule: &ExecutableRule,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
    qualifier: Option<&NameScope>,
    session: &mut NarrationSession<'_>,
) {
    let existing = ctx.rule_explanations.borrow().get(&rule.path).copied();
    let id = match existing {
        Some(id) if session.arena.get(id).body.is_some() => return,
        Some(id) => id,
        None => {
            let result_type = ctx.rule_result_type(plan, rule);
            let result = explained_value(
                ctx.rule_value(plan, &rule.path),
                result_type.as_ref(),
                plan,
                ctx,
            );
            let id = session.arena.push(ArenaEntry {
                name: qualified_rule_path(qualifier, &rule.path),
                result,
                body: None,
                causes: Vec::new(),
                children: Vec::new(),
            });
            ctx.rule_explanations
                .borrow_mut()
                .insert(rule.path.clone(), id);
            id
        }
    };
    let narrated = narrate(rule.normal_form, plan, ctx, qualifier, session);
    let entry = session.arena.get_mut(id);
    entry.body = Some(narrated.body);
    entry.causes = narrated.causes;
    entry.children = operands_of(&narrated.as_operand);
}

fn reserve_rule_entry(
    path: &RulePath,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
    qualifier: Option<&NameScope>,
    session: &mut NarrationSession<'_>,
) -> ArenaId {
    if let Some(id) = ctx.rule_explanations.borrow().get(path).copied() {
        return id;
    }
    let rule = plan
        .rules
        .get(path)
        .unwrap_or_else(|| panic!("BUG: reserved rule '{path}' missing from plan"));
    let result_type = ctx.rule_result_type(plan, rule);
    let result = explained_value(ctx.rule_value(plan, path), result_type.as_ref(), plan, ctx);
    let id = session.arena.push(ArenaEntry {
        name: qualified_rule_path(qualifier, path),
        result,
        body: None,
        causes: Vec::new(),
        children: Vec::new(),
    });
    ctx.rule_explanations.borrow_mut().insert(path.clone(), id);
    match qualifier {
        None => session
            .pending
            .push(PendingNarration::Root { path: path.clone() }),
        Some(_) => {
            let key = ctx.owner_row.clone().unwrap_or_else(|| {
                panic!("BUG: qualified reserve needs the instance owner_row for '{path}'")
            });
            session.pending.push(PendingNarration::Instance {
                key,
                path: path.clone(),
            });
        }
    }
    id
}

/// Narrate every cached row context (rows at every nesting level share one map).
/// A row's rules may reference rules of rows its read log names, and a rule
/// reference narrates as a [`ExplanationNode::RuleRef`] into the referenced
/// context's `rule_explanations`, so referenced rows narrate first: post-order
/// over the read-log edges. Reads complete after the rules they consume, so
/// the graph is acyclic.
pub(crate) fn narrate_row_contexts(
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
    arena: &mut ExplanationArena,
) {
    let mut session = NarrationSession {
        root_plan: plan,
        root_ctx: ctx,
        arena,
        pending: Vec::new(),
    };
    let contexts = ctx.caches.row_contexts.borrow();
    let mut starts: Vec<InstanceKey> = contexts.keys().cloned().collect();
    starts.sort_by_cached_key(|key| key.to_string());
    let mut done: std::collections::HashSet<InstanceKey> = std::collections::HashSet::new();
    let mut stack: Vec<(InstanceKey, bool)> = Vec::new();
    for start in starts {
        if done.contains(&start) {
            continue;
        }
        stack.push((start, false));
        while let Some((key, expand)) = stack.pop() {
            if expand {
                narrate_one_row(&key, plan, &contexts, &mut session);
                continue;
            }
            if !done.insert(key.clone()) {
                continue;
            }
            stack.push((key.clone(), true));
            let deps: Vec<InstanceKey> = contexts
                .get(&key)
                .expect("BUG: narrated row context missing from the cache")
                .row_reads_log
                .iter()
                .filter_map(|read| {
                    let dep = &read.row.key;
                    (*dep != key && contexts.contains_key(dep)).then(|| dep.clone())
                })
                .collect();
            for dep in deps {
                if !done.contains(&dep) {
                    stack.push((dep, false));
                }
            }
        }
    }
}

/// Narrate the evaluated rules of one row's context, in its template's plan order.
fn narrate_one_row(
    key: &InstanceKey,
    plan: &ExecutionPlan,
    contexts: &indexmap::IndexMap<InstanceKey, EvaluationContext>,
    session: &mut NarrationSession<'_>,
) {
    // A planned instance's rule bodies name data by its path (`people.1.age`,
    // `i.x`); under an evaluation-data or inline row only the `uses` hops below it.
    let qualifier = NameScope {
        data: key.segments().unwrap_or_else(|| key.import_hops()),
        rules: key.import_hops(),
    };
    let scope_plan = plan.row_template(&key.row.identity);
    let scope_ctx = contexts
        .get(key)
        .expect("BUG: narrated row context missing from the cache");
    for (index, rule) in scope_plan.rules.values().enumerate() {
        if scope_ctx.rule_values[index].is_some() {
            narrate_rule(rule, scope_plan, scope_ctx, Some(&qualifier), session);
        }
    }
}

/// Root [`Explanation`] for requested rule `name`: rule `rule` of `plan`, narrated in
/// `ctx` (the root context, or the context of the `uses` instance it belongs to). Remaps
/// reachable arena entries to compact public ids in first-reach order.
pub(crate) fn explanation_for(
    name: &RulePath,
    rule: &ExecutableRule,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
    arena: &ExplanationArena,
    rule_value: &OperationResult,
    rule_result_type: Arc<LemmaType>,
) -> Explanation {
    let root_id = *ctx
        .rule_explanations
        .borrow()
        .get(&rule.path)
        .unwrap_or_else(|| {
            panic!(
                "BUG: rule '{}' narrated after its use-site (plan order is topological)",
                rule.path.rule
            )
        });
    let root = arena.get(root_id);
    let (children, causes, rules) = remap_reachable(arena, &root.children, &root.causes);
    let runtime_scales =
        tree::display_scale_factors(rule_value.value(), rule_result_type.as_ref(), plan, ctx);
    Explanation {
        name: name.clone(),
        result: rule_value.clone(),
        result_type: rule_result_type,
        family_units: Arc::clone(&plan.family_units),
        runtime_scales,
        body: root.body.clone().unwrap_or_else(|| {
            panic!(
                "BUG: rule '{}' explanation root has an unfilled arena body",
                rule.path.rule
            )
        }),
        causes,
        children,
        rules,
    }
}

fn explained_value(
    result: &OperationResult,
    lemma_type: &LemmaType,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
) -> RuleResultValue {
    let scales = tree::display_scale_factors(result.value(), lemma_type, plan, ctx);
    explanation_result_value(result, lemma_type, &plan.family_units, Some(&scales))
}

/// `result` of a field of row or instance `key`, presented in that row's own scope: a
/// dynamic unit takes its factor from the row's context. A row whose context was
/// never built settled no scales.
fn row_field_value(
    result: &OperationResult,
    field_type: &LemmaType,
    key: &InstanceKey,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
) -> RuleResultValue {
    let template = plan.row_template(&key.row.identity);
    let contexts = ctx.caches.row_contexts.borrow();
    let scales = match contexts.get(key) {
        Some(row_ctx) => tree::display_scale_factors(result.value(), field_type, template, row_ctx),
        None => match result
            .value()
            .and_then(|bound| bound.instance_factors.as_ref())
        {
            Some(factors) => factors
                .iter()
                .map(|(unit, factor)| (unit.clone(), factor.clone()))
                .collect(),
            None => panic!(
                "BUG: row field narration for '{key}' has neither a row context nor carried instance factors"
            ),
        },
    };
    explanation_result_value(result, field_type, &template.family_units, Some(&scales))
}

/// Assign public ids in DFS first-reach order and rewrite RuleRef ids.
fn remap_reachable(
    arena: &ExplanationArena,
    root_children: &[ExplanationNode],
    root_causes: &[Cause],
) -> (Vec<ExplanationNode>, Vec<Cause>, Vec<ExplainedRule>) {
    let mut arena_to_public: HashMap<ArenaId, ExplainedRuleId> = HashMap::new();
    let mut order: Vec<ArenaId> = Vec::new();
    let mut stack: Vec<ArenaId> = Vec::new();
    push_rule_refs_rev(&mut stack, root_children);
    for cause in root_causes.iter().rev() {
        push_rule_refs_rev(&mut stack, &cause.children);
    }
    while let Some(arena_id) = stack.pop() {
        if arena_to_public.contains_key(&arena_id) {
            continue;
        }
        let public =
            ExplainedRuleId(u32::try_from(order.len()).expect("BUG: explanation rules exceed u32"));
        arena_to_public.insert(arena_id, public);
        order.push(arena_id);
        let entry = arena.get(arena_id);
        push_rule_refs_rev(&mut stack, &entry.children);
        for cause in entry.causes.iter().rev() {
            push_rule_refs_rev(&mut stack, &cause.children);
        }
    }
    let mut rules: Vec<ExplainedRule> = Vec::with_capacity(order.len());
    for arena_id in &order {
        let entry = arena.get(*arena_id);
        let mut children = entry.children.clone();
        let mut causes = entry.causes.clone();
        remap_in_place(
            children.iter_mut().chain(
                causes
                    .iter_mut()
                    .flat_map(|cause| cause.children.iter_mut()),
            ),
            &arena_to_public,
        );
        rules.push(ExplainedRule {
            id: arena_to_public[arena_id],
            name: entry.name.clone(),
            result: entry.result.clone(),
            body: entry.body.clone().unwrap_or_else(|| {
                panic!(
                    "BUG: arena entry '{}' reached remap with an unfilled body",
                    entry.name.input_key()
                )
            }),
            causes,
            children,
        });
    }
    let mut children = root_children.to_vec();
    let mut causes = root_causes.to_vec();
    remap_in_place(
        children.iter_mut().chain(
            causes
                .iter_mut()
                .flat_map(|cause| cause.children.iter_mut()),
        ),
        &arena_to_public,
    );
    (children, causes, rules)
}

/// Pushes the RuleRef ids under `nodes` so the first one in pre-order (row key before
/// row fields) is on top of the stack.
fn push_rule_refs_rev(stack: &mut Vec<ArenaId>, nodes: &[ExplanationNode]) {
    let mut found: Vec<ArenaId> = Vec::new();
    let mut pending: Vec<&ExplanationNode> = nodes.iter().rev().collect();
    while let Some(node) = pending.pop() {
        match node {
            ExplanationNode::RuleRef { id, .. } => found.push(ArenaId(id.0 as usize)),
            ExplanationNode::Compose { operands, .. }
            | ExplanationNode::Conversion { operands, .. }
            | ExplanationNode::ListCommand { operands, .. } => {
                pending.extend(operands.iter().rev());
            }
            ExplanationNode::Row { key, fields, .. } => {
                pending.extend(fields.iter().rev());
                pending.extend(key.iter().rev());
            }
            ExplanationNode::Filter { children, .. } => pending.extend(children.iter().rev()),
            ExplanationNode::Data { .. }
            | ExplanationNode::DataUnused { .. }
            | ExplanationNode::Veto { .. } => {}
        }
    }
    stack.extend(found.into_iter().rev());
}

/// Rewrites every RuleRef id under `roots` from arena ids to public ids, in place.
fn remap_in_place<'a>(
    roots: impl Iterator<Item = &'a mut ExplanationNode>,
    map: &HashMap<ArenaId, ExplainedRuleId>,
) {
    let mut pending: Vec<&mut ExplanationNode> = roots.collect();
    while let Some(node) = pending.pop() {
        match node {
            ExplanationNode::RuleRef { id, .. } => {
                let arena_id = ArenaId(id.0 as usize);
                let public = *map.get(&arena_id).unwrap_or_else(|| {
                    panic!("BUG: RuleRef arena id {} missing from remap", arena_id.0)
                });
                *id = public;
            }
            ExplanationNode::Compose { operands, .. }
            | ExplanationNode::Conversion { operands, .. }
            | ExplanationNode::ListCommand { operands, .. } => pending.extend(operands.iter_mut()),
            ExplanationNode::Row { key, fields, .. } => {
                pending.extend(key.iter_mut());
                pending.extend(fields.iter_mut());
            }
            ExplanationNode::Filter { children, .. } => pending.extend(children.iter_mut()),
            ExplanationNode::Data { .. }
            | ExplanationNode::DataUnused { .. }
            | ExplanationNode::Veto { .. } => {}
        }
    }
}

/// Reference to rule `path` of `plan`, narrated in `ctx`, displayed as `name`.
/// Reserves an arena entry (and queues body fill) when a deferred wire or cyclic
/// read reaches the rule before plan-order narration finished the entry.
fn rule_ref_node(
    path: &RulePath,
    name: RulePath,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
    qualifier: Option<&NameScope>,
    session: &mut NarrationSession<'_>,
) -> ExplanationNode {
    let arena_id = reserve_rule_entry(path, plan, ctx, qualifier, session);
    let rule = plan
        .rules
        .get(path)
        .unwrap_or_else(|| panic!("BUG: narrated rule '{path}' missing from plan"));
    let result_type = ctx.rule_result_type(plan, rule);
    let value = ctx.rule_value(plan, path);
    ExplanationNode::RuleRef {
        id: ExplainedRuleId(u32::try_from(arena_id.0).expect("BUG: arena id exceeds u32")),
        name,
        result: in_row_context(
            explained_value(value, result_type.as_ref(), plan, ctx),
            value,
            ctx,
        ),
    }
}

/// When data `path` of instance `key` is sealed by a deferred rule wire
/// (`uses -> with` to a rule), the RuleRef that narrates that rule and whether
/// the wired rule itself vetoed. `display_name` overrides the default label (the
/// wired rule path); list row fields pass a row-local label.
///
/// Lifted from the deferred arm of [`row_field_node`]: same resolution for
/// in-rule DataPath and import-member Data reads.
fn deferred_binding_rule_ref(
    key: &InstanceKey,
    path: &DataPath,
    display_name: Option<RulePath>,
    caches_ctx: &EvaluationContext,
    session: &mut NarrationSession<'_>,
) -> Option<(ExplanationNode, bool)> {
    let template = session.root_plan.row_template(&key.row.identity);
    let slot = template.data.get_index_of(path)?;
    let RowBinding::Deferred {
        target: target @ ReferenceTarget::Rule(rule),
        hops,
    } = caches_ctx.caches.row_binding(key, slot)?
    else {
        return None;
    };
    let name = display_name.unwrap_or_else(|| rule.clone());
    let owner = key.scope(*hops);
    if rule.segments.is_empty() {
        match owner {
            Some(owner_key) => {
                let qualifier = NameScope {
                    data: owner_key
                        .segments()
                        .unwrap_or_else(|| owner_key.import_hops()),
                    rules: owner_key.import_hops(),
                };
                let owner_plan = session.root_plan.row_template(&owner_key.row.identity);
                let contexts = caches_ctx.caches.row_contexts.borrow();
                let owner_ctx = contexts.get(owner_key).unwrap_or_else(|| {
                    panic!(
                        "BUG: owner context of deferred rule wire on '{path}' of {key} is cached"
                    )
                });
                let vetoed = owner_ctx.rule_value(owner_plan, rule).vetoed();
                let node =
                    rule_ref_node(rule, name, owner_plan, owner_ctx, Some(&qualifier), session);
                return Some((node, vetoed));
            }
            None => {
                let vetoed = session
                    .root_ctx
                    .rule_value(session.root_plan, rule)
                    .vetoed();
                return Some((
                    rule_ref_node(
                        rule,
                        name,
                        session.root_plan,
                        session.root_ctx,
                        None,
                        session,
                    ),
                    vetoed,
                ));
            }
        }
    }
    let owner_key = owner;
    let contexts = caches_ctx.caches.row_contexts.borrow();
    let (owner_plan, owner_ctx) = match owner_key {
        Some(owner) => (
            session.root_plan.row_template(&owner.row.identity),
            contexts.get(owner).unwrap_or_else(|| {
                panic!("BUG: owner context of deferred rule wire on '{path}' of {key} is cached")
            }),
        ),
        None => (session.root_plan, session.root_ctx),
    };
    let cell = owner_plan.member_targets.get(target).unwrap_or_else(|| {
        panic!("BUG: deferred member target '{rule}' has no member cell in its owner")
    });
    let member_read = owner_ctx
        .row_reads
        .get(cell)
        .and_then(|reads| reads.last())
        .unwrap_or_else(|| {
            panic!("BUG: deferred member target '{rule}' narrated without its read")
        });
    let RowEnd::Rule(member) = &member_read.end else {
        panic!("BUG: deferred rule target '{rule}' read a data member");
    };
    let instance_key = &member_read.row.key;
    let instance_ctx = contexts
        .get(instance_key)
        .unwrap_or_else(|| panic!("BUG: instance of '{rule}' read without its context"));
    let member_plan = session.root_plan.row_template(&instance_key.row.identity);
    let member_qualifier = NameScope {
        data: instance_key
            .segments()
            .unwrap_or_else(|| instance_key.import_hops()),
        rules: instance_key.import_hops(),
    };
    let vetoed = instance_ctx.rule_value(member_plan, member).vetoed();
    Some((
        rule_ref_node(
            member,
            name,
            member_plan,
            instance_ctx,
            Some(&member_qualifier),
            session,
        ),
        vetoed,
    ))
}

/// Value the walk produced for a cell. A data leaf reads through its binding
/// slot: an unbound leaf has no entry (that is what "unbound" means to
/// `missing_data`) and reads as its MissingData veto, exactly as the walk did.
fn slot(id: NormalFormId, plan: &ExecutionPlan, ctx: &EvaluationContext) -> OperationResult {
    let cell = plan.normal_form(id);
    if cell.rule_ref.is_none() {
        if let NormalFormKind::Leaf(LeafKind::DataPath(path)) = &cell.kind {
            return resolve_data_path_value(path, plan, ctx);
        }
    }
    ctx.values[id.index()].clone().unwrap_or_else(|| {
        panic!(
            "BUG: narrated cell {} ({:?}) was never evaluated (exhaustive walk missed it)",
            id.index(),
            cell.kind
        )
    })
}

/// Narrations finished during one [`narrate`] walk, by cell.
type Done = HashMap<NormalFormId, Narrated>;

/// `Err(child)`: `child` has no narration in `Done` yet; the step re-runs after it does.
type Need<T> = Result<T, NormalFormId>;

fn child(done: &Done, id: NormalFormId) -> Need<&Narrated> {
    done.get(&id).ok_or(id)
}

/// Narrate `root` with an explicit stack. A step that needs a child narration
/// pushes that child and is re-run once it is done. Steps read only finished
/// narrations and the pure plan and context, so a re-run gives the same result.
///
/// The explanation tree owns its subtrees, so a parent copies each child's
/// operand. Cost is O(subtree) per parent, O(n * depth) along a chain.
fn narrate(
    root: NormalFormId,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
    qualifier: Option<&NameScope>,
    session: &mut NarrationSession<'_>,
) -> Narrated {
    let mut done: Done = HashMap::new();
    let mut stack = vec![root];
    while let Some(&top) = stack.last() {
        if done.contains_key(&top) {
            stack.pop();
            continue;
        }
        match narrate_step(top, plan, ctx, &done, qualifier, session) {
            Ok(narrated) => {
                done.insert(top, narrated);
                stack.pop();
            }
            Err(needed) => stack.push(needed),
        }
    }
    done.remove(&root)
        .expect("BUG: root narration missing after walk")
}

fn narrate_step(
    id: NormalFormId,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
    done: &Done,
    qualifier: Option<&NameScope>,
    session: &mut NarrationSession<'_>,
) -> Need<Narrated> {
    let cell = plan.normal_form(id);
    if let Some(path) = &cell.rule_ref {
        let name = qualified_rule_path(qualifier, path);
        return Ok(Narrated {
            body: name.input_key(),
            causes: Vec::new(),
            as_operand: Some(rule_ref_node(path, name, plan, ctx, qualifier, session)),
        });
    }
    if let Some(origin) = cell.origin {
        if let NormalFormKind::OrderedDispatch {
            scrutinee, regions, ..
        } = &cell.kind
        {
            assert_dispatch_agrees_with_record(id, plan, ctx, done, qualifier, session)?;
            let mut narrated = child(done, origin)?.clone();
            if is_exclusive_point_table(regions, |region| {
                region_body_is_nested(plan, region.cell())
            }) {
                apply_exclusive_point_causes(&mut narrated, origin, *scrutinee, plan, ctx, done)?;
            }
            return Ok(narrated);
        }
        return Ok(child(done, origin)?.clone());
    }
    narrate_shape(id, plan, ctx, done, qualifier, session)
}

/// Point region body is itself a Piecewise or OrderedDispatch (conjunctive nest).
fn region_body_is_nested(plan: &ExecutionPlan, id: NormalFormId) -> bool {
    matches!(
        &plan.normal_form(id).kind,
        NormalFormKind::Piecewise(_) | NormalFormKind::OrderedDispatch { .. }
    )
}

/// Exclusive `scrutinee is K` table: keep only the winning cause, or clear
/// causes and attach the scrutinee when the default wins.
fn apply_exclusive_point_causes(
    narrated: &mut Narrated,
    origin: NormalFormId,
    scrutinee: NormalFormId,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
    done: &Done,
) -> Need<()> {
    let NormalFormKind::Piecewise(arms) = &plan.normal_form(origin).kind else {
        panic!("BUG: OrderedDispatch origin must be Piecewise");
    };
    match piecewise_decision(arms.len(), |arm| {
        Some(condition_outcome(&slot(arms[arm].0, plan, ctx)))
    }) {
        PiecewiseDecision::Taken { .. } => {
            let Some(winner) = narrated.causes.pop() else {
                panic!("BUG: exclusive point table Taken with empty causes");
            };
            narrated.causes = vec![winner];
        }
        PiecewiseDecision::Default => {
            narrated.causes.clear();
            let Some(scrutinee_node) = child(done, scrutinee)?.as_operand.clone() else {
                panic!("BUG: exclusive point default needs a scrutinee operand");
            };
            narrated.as_operand = match narrated.as_operand.take() {
                Some(ExplanationNode::Veto { message }) => Some(ExplanationNode::Compose {
                    expression: narrated.body.clone(),
                    operator: None,
                    operands: vec![scrutinee_node, ExplanationNode::Veto { message }],
                    result: cell_result_value(origin, plan, ctx),
                }),
                Some(ExplanationNode::Compose { operands, .. }) if operands.is_empty() => {
                    Some(scrutinee_node)
                }
                Some(other) => Some(ExplanationNode::Compose {
                    expression: narrated.body.clone(),
                    operator: None,
                    operands: vec![scrutinee_node, other],
                    result: cell_result_value(origin, plan, ctx),
                }),
                None => Some(scrutinee_node),
            };
        }
        PiecewiseDecision::Undecided { .. } => {
            // Vetoed condition: origin narration already carries the veto.
        }
    }
    Ok(())
}

/// Narrate the cell's own Kind (no origin redirection).
fn narrate_shape(
    id: NormalFormId,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
    done: &Done,
    qualifier: Option<&NameScope>,
    session: &mut NarrationSession<'_>,
) -> Need<Narrated> {
    let forms = plan.normal_forms.as_slice();
    Ok(match &plan.normal_form(id).kind {
        NormalFormKind::Leaf(LeafKind::Literal(literal)) => {
            let expression = literal.display_value_with_type(plan.result_type(id).as_ref());
            // Empty operands mark a bare literal; ASCII formatting skips these nodes.
            Narrated {
                body: expression.clone(),
                causes: Vec::new(),
                as_operand: Some(ExplanationNode::Compose {
                    expression,
                    operator: None,
                    operands: Vec::new(),
                    result: cell_result_value(id, plan, ctx),
                }),
            }
        }
        NormalFormKind::Leaf(LeafKind::DataPath(path))
            if matches!(plan.data.get(path), Some(DataDefinition::Import { .. })) =>
        {
            let expression = qualified_input_key(qualifier, path);
            // Empty operands mark a bare spec value; ASCII formatting skips these nodes.
            Narrated {
                body: expression.clone(),
                causes: Vec::new(),
                as_operand: Some(ExplanationNode::Compose {
                    expression,
                    operator: None,
                    operands: Vec::new(),
                    result: cell_result_value(id, plan, ctx),
                }),
            }
        }
        NormalFormKind::Leaf(LeafKind::DataPath(path)) => {
            let node = match plan.reference_ends.get(path) {
                // A slot bound to a rule reads as that rule, unless the rule's value
                // violates the slot's declared type: then the slot's veto is the value.
                Some(ReferenceEnd::Rule(rule)) => match slot(id, plan, ctx) {
                    OperationResult::Veto(veto) if !ctx.rule_value(plan, rule).vetoed() => {
                        ExplanationNode::Veto {
                            message: Some(veto.to_string()),
                        }
                    }
                    _ => rule_ref_node(
                        rule,
                        qualified_rule_path(qualifier, rule),
                        plan,
                        ctx,
                        qualifier,
                        session,
                    ),
                },
                Some(ReferenceEnd::Promptable(_)) | None => {
                    match ctx
                        .owner_row
                        .as_ref()
                        .and_then(|key| deferred_binding_rule_ref(key, path, None, ctx, session))
                    {
                        Some((node, rule_vetoed)) => match slot(id, plan, ctx) {
                            OperationResult::Veto(veto) if !rule_vetoed => ExplanationNode::Veto {
                                message: Some(veto.to_string()),
                            },
                            _ => node,
                        },
                        None => data_node(path, plan, ctx, qualifier),
                    }
                }
            };
            Narrated {
                body: qualified_input_key(qualifier, path),
                causes: Vec::new(),
                as_operand: Some(node),
            }
        }
        NormalFormKind::Now => Narrated {
            body: String::new(),
            causes: Vec::new(),
            as_operand: None,
        },
        NormalFormKind::Veto(veto) => {
            let node = ExplanationNode::Veto {
                message: veto.message.clone(),
            };
            Narrated {
                body: "veto".to_string(),
                causes: Vec::new(),
                as_operand: Some(node),
            }
        }
        NormalFormKind::Sum(terms) | NormalFormKind::Product(terms) => {
            let operands: Vec<NormalFormId> = terms.iter().map(|term| term.operand).collect();
            compose(id, &operands, plan, ctx, done, qualifier)?
        }
        NormalFormKind::Logical(connective, children) => {
            let absorbing = connective.absorbing_operand();
            let mut operands = Vec::with_capacity(children.len());
            for &child_id in children {
                operands.push(child_id);
                if matches!(
                    (absorbing, condition_outcome(&slot(child_id, plan, ctx))),
                    (true, BranchOutcome::Taken) | (false, BranchOutcome::NotTaken)
                ) {
                    break;
                }
            }
            compose(id, &operands, plan, ctx, done, qualifier)?
        }
        NormalFormKind::Subtract(a, b)
        | NormalFormKind::Divide(a, b)
        | NormalFormKind::Power(a, b)
        | NormalFormKind::Modulo(a, b)
        | NormalFormKind::Comparison(a, _, b)
        | NormalFormKind::RangeLiteral(a, b)
        | NormalFormKind::RangeContainment(a, b) => {
            compose(id, &[*a, *b], plan, ctx, done, qualifier)?
        }
        NormalFormKind::StepRounding(_, a, step) => match step {
            None => compose(id, &[*a], plan, ctx, done, qualifier)?,
            Some(b) => compose(id, &[*a, *b], plan, ctx, done, qualifier)?,
        },
        NormalFormKind::Negate(x)
        | NormalFormKind::Reciprocal(x)
        | NormalFormKind::Not(x)
        | NormalFormKind::MathOp(_, x)
        | NormalFormKind::DateRelative(_, x)
        | NormalFormKind::DateCalendar(_, _, x)
        | NormalFormKind::PastFutureRange(_, x)
        | NormalFormKind::RangeBound(_, x)
        | NormalFormKind::ResultIsVeto(x) => compose(id, &[*x], plan, ctx, done, qualifier)?,
        NormalFormKind::UnitConversion(inner, target) => {
            let expression = explanation_display(forms, id, qualifier);
            let inner_operand = child(done, *inner)?.as_operand.clone();
            let source = slot(*inner, plan, ctx);
            if source.vetoed() {
                return compose(id, &[*inner], plan, ctx, done, qualifier);
            }
            let result = slot(id, plan, ctx);
            let node = match &result {
                OperationResult::Veto(_) => ExplanationNode::Veto {
                    message: Some(format_operation_result(
                        &result,
                        plan.result_type(id).as_ref(),
                    )),
                },
                OperationResult::Value(result_literal) => {
                    let owning = match target {
                        crate::planning::semantics::SemanticConversionTarget::Unit {
                            owning_type,
                            ..
                        } => owning_type.as_ref(),
                        _ => plan.result_type(*inner).as_ref(),
                    };
                    let scales =
                        crate::evaluation::tree::runtime_scale_factors_for(owning, plan, ctx);
                    let steps = build_conversion_steps(
                        &borrow_value(&source, "conversion source").to_literal(),
                        plan.result_type(*inner),
                        target,
                        &result_literal.to_literal(),
                        plan.result_type(id),
                        Some(&scales),
                    );
                    ExplanationNode::Conversion {
                        expression: expression.clone(),
                        steps,
                        operands: inner_operand.into_iter().collect(),
                        result: cell_result_value(id, plan, ctx),
                    }
                }
            };
            Narrated {
                body: expression,
                causes: Vec::new(),
                as_operand: Some(node),
            }
        }
        NormalFormKind::Piecewise(arms) => {
            match piecewise_causes(arms, plan, ctx, done, qualifier, session)? {
                PiecewiseNarration::Causes {
                    causes,
                    winner_body,
                } => {
                    let body = child(done, winner_body)?;
                    Narrated {
                        body: body.body.clone(),
                        causes,
                        as_operand: body.as_operand.clone(),
                    }
                }
                PiecewiseNarration::Veto(narrated) => *narrated,
            }
        }
        NormalFormKind::OrderedDispatch { .. } => {
            unreachable!("BUG: OrderedDispatch always carries its Piecewise pre-image as origin")
        }
        NormalFormKind::SpecMember {
            base, path, name, ..
        } => {
            let member = spec_member_input_key(path, name);
            let base_result = slot(*base, plan, ctx);
            if base_result.vetoed() {
                return Ok(child(done, *base)?.clone());
            }
            let instance = match &borrow_value(&base_result, "spec member").value {
                ValueKind::Spec(SpecValue::Row(key)) => key.clone(),
                other => panic!("BUG: spec member base was {other}, not a spec value"),
            };
            if is_root_import(&instance) {
                let read = ctx
                    .row_reads
                    .get(&id)
                    .and_then(|reads| reads.last())
                    .unwrap_or_else(|| {
                        panic!("BUG: member '{member}' of {instance} narrated without its read")
                    });
                let mut narrated = import_member_narration(read, plan, ctx, session);
                narrated.body = format!("{instance}.{member}");
                return Ok(narrated);
            }
            let mut operands: Vec<ExplanationNode> =
                child(done, *base)?.as_operand.clone().into_iter().collect();
            let member_fields = row_read_nodes(id, None, plan, ctx, session);
            merge_member_fields_into_list_rows(&mut operands, member_fields);
            drop_list_data_when_rows(&mut operands);
            let expression = explanation_display(forms, id, qualifier);
            Narrated {
                body: expression.clone(),
                causes: Vec::new(),
                as_operand: Some(ExplanationNode::Compose {
                    expression,
                    operator: None,
                    operands,
                    result: cell_result_value(id, plan, ctx),
                }),
            }
        }
        NormalFormKind::ListCommand {
            list,
            op,
            args,
            row_level,
        } => narrate_list_command(
            id,
            ListCommand {
                list: *list,
                op,
                args,
                row_level: *row_level,
            },
            plan,
            ctx,
            done,
            qualifier,
            session,
        )?,
        NormalFormKind::RowField { .. } | NormalFormKind::Leaf(LeafKind::RowValue { .. }) => {
            unreachable!("BUG: row leaf narrated outside its list command")
        }
    })
}

/// The fields of a [`NormalFormKind::ListCommand`] cell, borrowed from the plan.
struct ListCommand<'a> {
    list: NormalFormId,
    op: &'a NormalFormListOp,
    args: &'a [NormalFormId],
    row_level: Option<u32>,
}

/// Narrate a list command: peel `where` filters, emit row nodes, wrap filters.
fn narrate_list_command(
    id: NormalFormId,
    command: ListCommand<'_>,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
    done: &Done,
    qualifier: Option<&NameScope>,
    session: &mut NarrationSession<'_>,
) -> Need<Narrated> {
    let ListCommand {
        list,
        op,
        args,
        row_level,
    } = command;
    let forms = plan.normal_forms.as_slice();
    let this_is_where = is_where_op(op);
    let (base, filters) = if this_is_where && !slot(id, plan, ctx).vetoed() {
        let (base, inner) = peel_where_chain(list, plan, ctx, qualifier, session);
        let mut filters = inner;
        filters.push(filter_info(id, list, plan, ctx, qualifier, session));
        (base, filters)
    } else {
        peel_where_chain(list, plan, ctx, qualifier, session)
    };

    let base_operand = child(done, base)?.as_operand.clone();
    let mut arg_operands = Vec::new();
    for arg in args {
        if row_level.is_some() && plan.row_mask[arg.index()] != 0 {
            continue;
        }
        if let Some(node) = child(done, *arg)?.as_operand.clone() {
            arg_operands.push(node);
        }
    }

    let expression = explanation_display(forms, id, qualifier);
    let stage = list_command_stage(forms, id, qualifier);
    let explained_op = explained_list_op(op);
    let key_fields = list_key_fields(plan, list);
    // `with` / `key` fields are already on the stage line; reductions / find / any keep reads.
    // End-of-chain `where` rows live under Filter (matched only).
    let rows = if this_is_where
        || matches!(
            op,
            NormalFormListOp::With
                | NormalFormListOp::WithRow { .. }
                | NormalFormListOp::Key { .. }
        ) {
        Vec::new()
    } else if matches!(op, NormalFormListOp::Find) && row_level.is_some() {
        let mut rows = row_read_nodes(id, key_fields, plan, ctx, session);
        let keep = result_row_instances(&slot(id, plan, ctx));
        rows.retain(|node| row_node_in_instances(node, &keep));
        if rows.is_empty() && key_fields.is_some() {
            for instance in &keep {
                let (number, appended) = instance.list_position();
                rows.push(ExplanationNode::Row {
                    number,
                    appended,
                    key: row_key_nodes(instance, key_fields, plan, ctx),
                    fields: Vec::new(),
                });
            }
        }
        rows
    } else if row_level.is_some() {
        row_read_nodes(id, key_fields, plan, ctx, session)
    } else {
        Vec::new()
    };

    let mut operands = Vec::new();
    if let Some(base_node) = base_operand {
        let drop_data = match &base_node {
            ExplanationNode::Data { .. } => {
                !rows.is_empty()
                    || filters.iter().any(|filter| !filter.rows.is_empty())
                    || is_empty_list_result(&slot(base, plan, ctx))
            }
            _ => false,
        };
        if !drop_data {
            operands.push(base_node);
        }
    }
    operands.extend(arg_operands);
    operands.extend(wrap_filters(filters, rows));

    Ok(Narrated {
        body: expression.clone(),
        causes: Vec::new(),
        as_operand: Some(ExplanationNode::ListCommand {
            expression,
            stage,
            op: explained_op,
            operands,
            result: cell_result_value(id, plan, ctx),
        }),
    })
}

fn list_command_stage(
    forms: &[NormalForm],
    id: NormalFormId,
    qualifier: Option<&NameScope>,
) -> String {
    match (NfNode {
        forms,
        id,
        qualifier,
    })
    .view()
    {
        SyntaxView::ListCommand { command, .. } => CommandSyntax(&command).to_string(),
        _ => panic!("BUG: list_command_stage on a cell that is not a list command"),
    }
}

/// Attach SpecMember field reads (e.g. `.price`) onto the hit Row inside a find ListCommand.
fn merge_member_fields_into_list_rows(
    operands: &mut Vec<ExplanationNode>,
    member_fields: Vec<ExplanationNode>,
) {
    if member_fields.is_empty() {
        return;
    }
    if merge_fields_into_first_row(operands, &member_fields) {
        return;
    }
    operands.extend(member_fields);
}

fn merge_fields_into_first_row(
    operands: &mut [ExplanationNode],
    member_fields: &[ExplanationNode],
) -> bool {
    let mut pending: Vec<&mut ExplanationNode> = operands.iter_mut().collect();
    while let Some(node) = pending.pop() {
        match node {
            ExplanationNode::ListCommand { operands, .. }
            | ExplanationNode::Compose { operands, .. }
            | ExplanationNode::Conversion { operands, .. } => {
                pending.extend(operands.iter_mut());
            }
            ExplanationNode::Filter { children, .. } => pending.extend(children.iter_mut()),
            ExplanationNode::Row { fields, .. } => {
                for member in member_fields {
                    match member {
                        ExplanationNode::Row { fields: extra, .. } => {
                            fields.extend(extra.iter().cloned())
                        }
                        other => fields.push(other.clone()),
                    }
                }
                return true;
            }
            ExplanationNode::RuleRef { .. }
            | ExplanationNode::Data { .. }
            | ExplanationNode::DataUnused { .. }
            | ExplanationNode::Veto { .. } => {}
        }
    }
    false
}

fn explained_list_op(op: &NormalFormListOp) -> ExplainedListOp {
    match op {
        NormalFormListOp::Find => ExplainedListOp::Find,
        NormalFormListOp::First => ExplainedListOp::First,
        NormalFormListOp::Last => ExplainedListOp::Last,
        NormalFormListOp::Count => ExplainedListOp::Count,
        NormalFormListOp::Sum => ExplainedListOp::Sum,
        NormalFormListOp::Avg => ExplainedListOp::Avg,
        NormalFormListOp::Product => ExplainedListOp::Product,
        NormalFormListOp::Min => ExplainedListOp::Min,
        NormalFormListOp::Max => ExplainedListOp::Max,
        NormalFormListOp::Predicate {
            kind: ListPredKind::Any,
            ..
        } => ExplainedListOp::Any,
        NormalFormListOp::Predicate {
            kind: ListPredKind::All,
            ..
        } => ExplainedListOp::All,
        NormalFormListOp::Predicate {
            kind: ListPredKind::Where,
            ..
        } => ExplainedListOp::Where,
        NormalFormListOp::Sort { descending } => ExplainedListOp::Sort {
            descending: *descending,
        },
        NormalFormListOp::Top => ExplainedListOp::Top,
        NormalFormListOp::With | NormalFormListOp::WithRow { .. } => ExplainedListOp::With,
        NormalFormListOp::Key { .. } => ExplainedListOp::Key,
    }
}

fn list_key_fields(plan: &ExecutionPlan, list: NormalFormId) -> Option<&[KeyField]> {
    match &plan.result_type(list).specifications {
        TypeSpecification::List {
            key: ListKey::Fields(fields),
            ..
        } => Some(fields.as_slice()),
        _ => None,
    }
}

fn is_where_op(op: &NormalFormListOp) -> bool {
    matches!(
        op,
        NormalFormListOp::Predicate {
            kind: ListPredKind::Where,
            ..
        }
    )
}

fn is_empty_list_result(result: &OperationResult) -> bool {
    match result {
        OperationResult::Value(bound) => {
            matches!(&bound.value, ValueKind::List(items) if items.is_empty())
        }
        OperationResult::Veto(_) => false,
    }
}

/// Rows of a list result, with the 0-based position of each.
fn result_row_instances(result: &OperationResult) -> Vec<ReadRow> {
    let row_of = |value: &ValueKind, position: usize| match value {
        ValueKind::Spec(SpecValue::Row(key)) => Some(ReadRow::listed(key.clone(), position)),
        _ => None,
    };
    match result {
        OperationResult::Value(bound) => match &bound.value {
            ValueKind::List(items) => items
                .iter()
                .enumerate()
                .filter_map(|(position, item)| row_of(&item.value, position))
                .collect(),
            value => row_of(value, 0).into_iter().collect(),
        },
        OperationResult::Veto(_) => Vec::new(),
    }
}

fn row_node_in_instances(node: &ExplanationNode, instances: &[ReadRow]) -> bool {
    let ExplanationNode::Row {
        number, appended, ..
    } = node
    else {
        panic!("BUG: row_node_in_instances on non-Row");
    };
    instances.iter().any(|row| {
        let (row_number, row_appended) = row.list_position();
        row_appended == *appended && row_number == *number
    })
}

struct FilterInfo {
    condition: String,
    matched: usize,
    total: usize,
    rows: Vec<ExplanationNode>,
}

fn peel_where_chain(
    mut id: NormalFormId,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
    qualifier: Option<&NameScope>,
    session: &mut NarrationSession<'_>,
) -> (NormalFormId, Vec<FilterInfo>) {
    let mut filters = Vec::new();
    loop {
        let cell = plan.normal_form(id);
        let NormalFormKind::ListCommand { list, op, .. } = &cell.kind else {
            return (id, filters);
        };
        if !is_where_op(op) {
            return (id, filters);
        }
        if slot(id, plan, ctx).vetoed() {
            return (id, filters);
        }
        filters.push(filter_info(id, *list, plan, ctx, qualifier, session));
        id = *list;
    }
}

fn filter_info(
    where_id: NormalFormId,
    input_list: NormalFormId,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
    qualifier: Option<&NameScope>,
    session: &mut NarrationSession<'_>,
) -> FilterInfo {
    let forms = plan.normal_forms.as_slice();
    let NormalFormKind::ListCommand { op, args, .. } = &plan.normal_form(where_id).kind else {
        panic!("BUG: filter_info on a non-list-command cell");
    };
    let condition = match op {
        NormalFormListOp::Predicate {
            kind: ListPredKind::Where,
            ..
        } => {
            let arg = *args
                .first()
                .expect("BUG: where command has no condition argument");
            explanation_display(forms, arg, qualifier)
        }
        other => panic!("BUG: filter_info on non-where op {other:?}"),
    };
    let key_fields = list_key_fields(plan, input_list);
    let condition_arg = *args
        .first()
        .expect("BUG: where command has no condition argument");
    let columns = where_row_column_names(condition_arg, plan);
    let keep = result_row_instances(&slot(where_id, plan, ctx));
    let read_rows = row_read_nodes(where_id, key_fields, plan, ctx, session);
    let rows = keep
        .iter()
        .map(|instance| where_matched_row(instance, key_fields, &columns, &read_rows, plan, ctx))
        .collect();
    FilterInfo {
        condition,
        matched: list_len(&slot(where_id, plan, ctx)),
        total: list_len(&slot(input_list, plan, ctx)),
        rows,
    }
}

/// Matched `where` row: key fields, predicate-dependent data columns, then RuleRefs.
fn where_matched_row(
    instance: &ReadRow,
    key: Option<&[KeyField]>,
    columns: &std::collections::HashSet<String>,
    read_rows: &[ExplanationNode],
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
) -> ExplanationNode {
    let (number, appended) = instance.list_position();
    let key_nodes = row_key_nodes(instance, key, plan, ctx);
    let key_names: std::collections::HashSet<String> = key
        .unwrap_or(&[])
        .iter()
        .map(|key_field| key_field.name.clone())
        .collect();
    let mut fields = match &instance.key.row.origin {
        RowOrigin::Import { .. } => {
            import_data_field_nodes(&instance.key, columns, &key_names, plan, ctx)
        }
        RowOrigin::Numbered { .. } | RowOrigin::Written { .. } | RowOrigin::Inline { .. } => {
            bound_data_field_nodes(&instance.key, columns, &key_names, plan, ctx)
        }
    };
    if let Some(ExplanationNode::Row {
        fields: read_fields,
        ..
    }) = read_rows.iter().find(|node| match node {
        ExplanationNode::Row {
            number: n,
            appended: a,
            ..
        } => *n == number && *a == appended,
        _ => false,
    }) {
        for field in read_fields {
            if is_row_data_node(field) {
                continue;
            }
            fields.push(field.clone());
        }
    }
    ExplanationNode::Row {
        number,
        appended,
        key: key_nodes,
        fields,
    }
}

fn is_row_data_node(node: &ExplanationNode) -> bool {
    matches!(
        node,
        ExplanationNode::Data { .. } | ExplanationNode::DataUnused { .. }
    )
}

fn bound_data_field_nodes(
    key: &InstanceKey,
    columns: &std::collections::HashSet<String>,
    skip: &std::collections::HashSet<String>,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
) -> Vec<ExplanationNode> {
    let template = plan.row_template(&key.row.identity);
    let mut fields = Vec::new();
    for slot in 0..key.row.bindings.len() {
        let Some(binding) = ctx.caches.row_binding(key, slot) else {
            continue;
        };
        let (path, _) = template
            .data
            .get_index(slot)
            .expect("BUG: row binding slot is template data");
        let name = path.input_key();
        if skip.contains(&name) || !columns.contains(&name) {
            continue;
        }
        let field_type = row_end_type(template, &RowEnd::Data(path.clone()));
        // A deferred binding shows its value only when the field was read here:
        // the row's context eagerly binds slots whose rule already has a value,
        // so slot presence is not a read. An unread sealed field stays unshown,
        // as a fixed row's unread cell does.
        let value = match binding {
            RowBinding::Bound(value) => Some(value.clone()),
            RowBinding::Rows {
                rows,
                row_type,
                hops,
            } => Some(RowBinding::rows_value(rows, row_type, key.scope(*hops))),
            RowBinding::Instance(_) => continue,
            RowBinding::Deferred { .. } => ctx
                .row_reads_log
                .iter()
                .find(|read| {
                    read.row.key == *key
                        && matches!(&read.end, RowEnd::Data(end_path) if end_path == path)
                })
                .map(|read| read.result.clone()),
        };
        match value {
            Some(value) => fields.push(ExplanationNode::Data {
                name: DataPath::local(name),
                result: row_field_value(&value, field_type.as_ref(), key, plan, ctx),
            }),
            None => fields.push(ExplanationNode::DataUnused {
                name: DataPath::local(name),
            }),
        }
    }
    fields
}

/// Data columns of a `uses` instance and of the `uses` instances under it (`sub.x`),
/// read from their contexts. A field of an instance whose context was never built was
/// not read.
fn import_data_field_nodes(
    key: &InstanceKey,
    columns: &std::collections::HashSet<String>,
    skip: &std::collections::HashSet<String>,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
) -> Vec<ExplanationNode> {
    let contexts = ctx.caches.row_contexts.borrow();
    let mut fields = Vec::new();
    let mut pending: Vec<(InstanceKey, String)> = vec![(key.clone(), String::new())];
    while let Some((instance, prefix)) = pending.pop() {
        let template = plan.row_template(&instance.row.identity);
        let instance_ctx = contexts.get(&instance);
        let mut nested = Vec::new();
        for (path, definition) in &template.data {
            let name = format!("{prefix}{}", path.input_key());
            match definition {
                DataDefinition::Import { .. } => {
                    nested.push((
                        InstanceKey::new(
                            Arc::clone(template.import_row(Some(&instance.row), path, None)),
                            Some(instance.clone()),
                        ),
                        format!("{name}."),
                    ));
                    continue;
                }
                DataDefinition::InstanceList { .. } => continue,
                DataDefinition::Value { .. }
                | DataDefinition::TypeDeclaration { .. }
                | DataDefinition::Reference { .. } => {}
            }
            if skip.contains(&name) || !columns.contains(&name) {
                continue;
            }
            match instance_ctx.and_then(|instance_ctx| instance_ctx.data_slot(template, path)) {
                Some(value) => fields.push(ExplanationNode::Data {
                    name: DataPath::local(name),
                    result: row_field_value(
                        value,
                        &row_data_type(&template.data, path),
                        &instance,
                        plan,
                        ctx,
                    ),
                }),
                None => fields.push(ExplanationNode::DataUnused {
                    name: DataPath::local(name),
                }),
            }
        }
        pending.extend(nested.into_iter().rev());
    }
    fields
}

/// Data column names the where predicate depends on (row fields and referenced rule bodies).
fn where_row_column_names(
    condition: NormalFormId,
    plan: &ExecutionPlan,
) -> std::collections::HashSet<String> {
    let mut names = std::collections::HashSet::new();
    let mut pending = vec![(plan, condition)];
    let mut seen = std::collections::HashSet::new();
    while let Some((current, id)) = pending.pop() {
        if !seen.insert((current as *const ExecutionPlan, id)) {
            continue;
        }
        let cell = current.normal_form(id);
        match &cell.kind {
            NormalFormKind::RowField { identity, end, .. } => match end {
                RowEnd::Data(path) => {
                    names.insert(path.input_key());
                }
                RowEnd::Rule(rule) => {
                    if let Some(template) = current.row_templates.get(identity) {
                        let body = template
                            .rules
                            .get(rule)
                            .unwrap_or_else(|| {
                                panic!(
                                    "BUG: where predicate rule `{}` missing from row template",
                                    rule.input_key()
                                )
                            })
                            .normal_form;
                        pending.push((template.as_ref(), body));
                    }
                }
            },
            NormalFormKind::Leaf(LeafKind::DataPath(path)) => {
                names.insert(path.input_key());
            }
            _ => {
                cell.kind
                    .for_each_child(|child| pending.push((current, child)));
            }
        }
    }
    names
}

fn list_len(result: &OperationResult) -> usize {
    match result {
        OperationResult::Value(bound) => match &bound.value {
            ValueKind::List(items) => items.len(),
            other => panic!("BUG: where value is not a list: {other:?}"),
        },
        OperationResult::Veto(_) => panic!("BUG: where value is not a list"),
    }
}

fn wrap_filters(filters: Vec<FilterInfo>, children: Vec<ExplanationNode>) -> Vec<ExplanationNode> {
    let mut nodes = children;
    for filter in filters {
        let children = if nodes.is_empty() { filter.rows } else { nodes };
        nodes = vec![ExplanationNode::Filter {
            condition: filter.condition,
            matched: filter.matched,
            total: filter.total,
            children,
        }];
    }
    nodes
}

fn drop_list_data_when_rows(operands: &mut Vec<ExplanationNode>) {
    let has_rows = operands
        .iter()
        .any(|node| matches!(node, ExplanationNode::Row { .. }));
    if !has_rows {
        return;
    }
    operands.retain(|node| !matches!(node, ExplanationNode::Data { .. }));
}

/// Whether `key` is a `uses` instance reached from the root through `uses` hops only.
/// A member of it narrates as the member itself, named by its full path (`i.sub.total`),
/// as the root plan names it.
fn is_root_import(key: &InstanceKey) -> bool {
    key.segments()
        .is_some_and(|segments| segments.len() == key.import_hops().len())
}

/// Narration of the member read `read` on a `uses` instance reached from the root: the
/// rule it is, or the data it is, named under the instance's hops.
fn import_member_narration(
    read: &RowRead,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
    session: &mut NarrationSession<'_>,
) -> Narrated {
    let key = &read.row.key;
    let template = plan.row_template(&key.row.identity);
    let scope = NameScope {
        data: key.import_hops(),
        rules: key.import_hops(),
    };
    let (body, node) = match &read.end {
        RowEnd::Rule(rule) => {
            let contexts = ctx.caches.row_contexts.borrow();
            let instance_ctx = contexts
                .get(key)
                .unwrap_or_else(|| panic!("BUG: rule '{rule}' of {key} read without its context"));
            (
                qualified_rule_path(Some(&scope), rule).input_key(),
                rule_ref_node(
                    rule,
                    qualified_rule_path(Some(&scope), rule),
                    template,
                    instance_ctx,
                    Some(&scope),
                    session,
                ),
            )
        }
        RowEnd::Data(path) => {
            let name = qualified_data_path(Some(&scope), path);
            let node = match template.reference_ends.get(path) {
                Some(ReferenceEnd::Rule(rule)) => match &read.result {
                    OperationResult::Veto(veto) if !rule_read_vetoed(key, template, rule, ctx) => {
                        ExplanationNode::Veto {
                            message: Some(veto.to_string()),
                        }
                    }
                    _ => {
                        let contexts = ctx.caches.row_contexts.borrow();
                        let instance_ctx = contexts.get(key).unwrap_or_else(|| {
                            panic!("BUG: rule '{rule}' of {key} read without its context")
                        });
                        rule_ref_node(
                            rule,
                            qualified_rule_path(Some(&scope), rule),
                            template,
                            instance_ctx,
                            Some(&scope),
                            session,
                        )
                    }
                },
                Some(ReferenceEnd::Promptable(_)) | None => {
                    match deferred_binding_rule_ref(key, path, None, ctx, session) {
                        Some((node, rule_vetoed)) => match &read.result {
                            OperationResult::Veto(veto) if !rule_vetoed => ExplanationNode::Veto {
                                message: Some(veto.to_string()),
                            },
                            _ => node,
                        },
                        None => {
                            let data_type =
                                EvaluationContext::data_display_type(template, path, &read.result);
                            ExplanationNode::Data {
                                name: name.clone(),
                                result: row_field_value(
                                    &read.result,
                                    data_type.as_ref(),
                                    key,
                                    plan,
                                    ctx,
                                ),
                            }
                        }
                    }
                }
            };
            (name.input_key(), node)
        }
    };
    Narrated {
        body,
        causes: Vec::new(),
        as_operand: Some(node),
    }
}

/// Whether rule `rule` of instance `key` vetoed in the instance's context.
fn rule_read_vetoed(
    key: &InstanceKey,
    template: &ExecutionPlan,
    rule: &RulePath,
    ctx: &EvaluationContext,
) -> bool {
    let contexts = ctx.caches.row_contexts.borrow();
    contexts
        .get(key)
        .unwrap_or_else(|| panic!("BUG: rule '{rule}' of {key} read without its context"))
        .rule_value(template, rule)
        .vetoed()
}

/// One [`ExplanationNode::Row`] per row the cell read, in read order.
fn row_read_nodes(
    id: NormalFormId,
    key: Option<&[KeyField]>,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
    session: &mut NarrationSession<'_>,
) -> Vec<ExplanationNode> {
    let Some(reads) = ctx.row_reads.get(&id) else {
        return Vec::new();
    };
    let mut nodes: Vec<(ReadRow, Vec<ExplanationNode>)> = Vec::new();
    for read in reads {
        if is_root_import(&read.row.key) {
            continue;
        }
        let (row, below) = labelled_row(&read.row, ctx.owner_row.as_ref());
        let field = row_field_node(read, &row, below.as_deref(), plan, ctx, session);
        match nodes.iter_mut().find(|(labelled, _)| *labelled == row) {
            Some((_, fields)) => fields.push(field),
            None => nodes.push((row, vec![field])),
        }
    }
    nodes
        .into_iter()
        .map(|(row, fields)| {
            let (number, appended) = row.list_position();
            ExplanationNode::Row {
                number,
                appended,
                key: row_key_nodes(&row, key, plan, ctx),
                fields,
            }
        })
        .collect()
}

/// Row a field read is labelled under, with the read instance named from it when it is
/// not that row: a `uses` instance under a row (`lines.1.item`) is labelled under the
/// row; an instance written under a run-data or appended row (`lines.2.holder.items.1`)
/// under that row, unless the narrating context's row `scope` holds it.
fn labelled_row(read: &ReadRow, scope: Option<&InstanceKey>) -> (ReadRow, Option<String>) {
    if read.position.is_some() {
        return (read.clone(), None);
    }
    let mut row = &read.key;
    while matches!(row.row.origin, RowOrigin::Import { .. }) {
        row = row.owner().unwrap_or_else(|| {
            panic!(
                "BUG: {} is a `uses` instance of the root, read as a member, not a row",
                read.key
            )
        });
    }
    if matches!(row.row.origin, RowOrigin::Written { .. }) {
        let mut table = row.owner();
        while let Some(owner) = table {
            if Some(owner) == scope {
                break;
            }
            match owner.row.origin {
                RowOrigin::Numbered { .. } | RowOrigin::Inline { .. } => {
                    row = owner;
                    break;
                }
                RowOrigin::Import { .. } | RowOrigin::Written { .. } => table = owner.owner(),
            }
        }
    }
    let below = (row != &read.key).then(|| {
        read.key
            .label_under(row)
            .expect("BUG: a labelled row holds the instance read")
    });
    (
        ReadRow {
            key: row.clone(),
            position: None,
        },
        below,
    )
}

/// Field node of `read`, labelled under `row` with the instance read named `below` it:
/// a rule reference when the field is a rule of the instance read, a reference chain
/// ending in one, or a binding deferred to a rule; otherwise its data, named from the
/// row.
fn row_field_node(
    read: &RowRead,
    row: &ReadRow,
    below: Option<&str>,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
    session: &mut NarrationSession<'_>,
) -> ExplanationNode {
    let key = &read.row.key;
    let template = plan.row_template(&key.row.identity);
    let rule_path = match &read.end {
        RowEnd::Rule(rule) => Some(rule),
        RowEnd::Data(path) => match template.reference_ends.get(path) {
            Some(ReferenceEnd::Rule(rule)) => Some(rule),
            Some(ReferenceEnd::Promptable(_)) | None => None,
        },
    };
    if let Some(rule_path) = rule_path {
        let contexts = ctx.caches.row_contexts.borrow();
        let row_ctx = contexts.get(key).expect("BUG: narrated row was never read");
        let qualifier = NameScope {
            data: key.segments().unwrap_or_else(|| key.import_hops()),
            rules: key.import_hops(),
        };
        return rule_ref_node(
            rule_path,
            rule_path.clone(),
            template,
            row_ctx,
            Some(&qualifier),
            session,
        );
    }
    if let RowEnd::Data(path) = &read.end {
        let slot = template
            .data
            .get_index_of(path)
            .expect("BUG: a row field read is template data");
        if let Some(RowBinding::Deferred {
            target: ReferenceTarget::Rule(rule),
            hops,
        }) = ctx.caches.row_binding(key, slot)
        {
            let owner = key.scope(*hops);
            let name = match owner.and_then(|owner| owner.label_under(&row.key)) {
                Some(prefix) if !prefix.is_empty() => {
                    RulePath::new(Vec::new(), format!("{prefix}.{}", rule.input_key()))
                }
                _ => rule.clone(),
            };
            if let Some((node, _)) = deferred_binding_rule_ref(key, path, Some(name), ctx, session)
            {
                return node;
            }
            panic!("BUG: deferred row field '{path}' of {key} failed to resolve");
        }
    }
    let name = match below {
        Some(below) => format!("{below}.{}", read.end),
        None => read.end.to_string(),
    };
    ExplanationNode::Data {
        name: DataPath::local(name),
        result: row_field_value(&read.result, read.result_type.as_ref(), key, plan, ctx),
    }
}

fn row_key_nodes(
    row: &ReadRow,
    key: Option<&[KeyField]>,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
) -> Vec<ExplanationNode> {
    let Some(key_fields) = key else {
        return Vec::new();
    };
    key_fields
        .iter()
        .map(|key_field| {
            let path = &key_field.path;
            let (field_type, value) = {
                let key = &row.key;
                {
                    let template = plan.row_template(&key.row.identity);
                    let field_type = row_end_type(template, &RowEnd::Data(path.clone()));
                    let slot = template
                        .data
                        .get_index_of(path)
                        .expect("BUG: key field is template data");
                    let value = match ctx.caches.row_binding(key, slot) {
                        Some(RowBinding::Bound(value)) => Some(value.clone()),
                        Some(RowBinding::Deferred { .. }) => {
                            let contexts = ctx.caches.row_contexts.borrow();
                            let row_ctx = contexts
                                .get(key)
                                .expect("BUG: a deferred key field is read in its row context");
                            row_ctx.data_slot(template, path).cloned()
                        }
                        Some(RowBinding::Rows { .. } | RowBinding::Instance(_)) => {
                            unreachable!("BUG: key fields are values, never lists or instances")
                        }
                        None => default_of(&template.data, path).map(|typed| {
                            OperationResult::from_bound(BoundValueKind::with_binding(
                                typed.value,
                                typed.lemma_type.unit().map(Arc::from),
                            ))
                        }),
                    };
                    (field_type, value)
                }
            };
            let result = match value {
                Some(ref value) => row_field_value(value, &field_type, &row.key, plan, ctx),
                None => panic!(
                    "BUG: key field `{}` of a narrated row is unbound",
                    key_field.name
                ),
            };
            ExplanationNode::Data {
                name: DataPath::local(key_field.name.clone()),
                result,
            }
        })
        .collect()
}

/// Compose node over the operands the walk visited: expression text from the
/// plan, operands from each child's narration (including bare literals for JSON).
/// Arithmetic cells stamp [`ArithmeticComputation`] on the node; other kinds leave
/// `operator` unset.
fn compose(
    id: NormalFormId,
    operands: &[NormalFormId],
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
    done: &Done,
    qualifier: Option<&NameScope>,
) -> Need<Narrated> {
    let mut operand_nodes = Vec::with_capacity(operands.len());
    for operand in operands {
        if let Some(node) = child(done, *operand)?.as_operand.clone() {
            operand_nodes.push(node);
        }
    }
    let expression = explanation_display(plan.normal_forms.as_slice(), id, qualifier);
    let operator = arithmetic_operator_of(&plan.normal_form(id).kind);
    let result = cell_result_value(id, plan, ctx);
    Ok(Narrated {
        body: expression.clone(),
        causes: Vec::new(),
        as_operand: Some(ExplanationNode::Compose {
            expression,
            operator,
            operands: operand_nodes,
            result,
        }),
    })
}

/// Flattened [`RuleResultValue`] for a narrated cell, via [`slot`].
fn cell_result_value(
    id: NormalFormId,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
) -> RuleResultValue {
    let result = slot(id, plan, ctx);
    in_row_context(
        explained_value(&result, plan.result_type(id).as_ref(), plan, ctx),
        &result,
        ctx,
    )
}

/// `value` of `result` as narrated in `ctx`: an instance held by the context's row is
/// named from that row (`holder.items.1`), as the row's own reads name it.
fn in_row_context(
    mut value: RuleResultValue,
    result: &OperationResult,
    ctx: &EvaluationContext,
) -> RuleResultValue {
    let (Some(row), OperationResult::Value(bound)) = (&ctx.owner_row, result) else {
        return value;
    };
    let label = |kind: &ValueKind| match kind {
        ValueKind::Spec(SpecValue::Row(key)) => {
            key.label_under(row).filter(|label| !label.is_empty())
        }
        _ => None,
    };
    match &bound.value {
        ValueKind::Spec(SpecValue::Row(_)) => {
            if let Some(label) = label(&bound.value) {
                value.result = Some(label);
            }
        }
        ValueKind::List(rows) => {
            let entries = value
                .list
                .as_mut()
                .expect("BUG: a list value narrates with its list arm");
            let mut relabelled = false;
            for (entry, row_value) in entries.iter_mut().zip(rows.iter()) {
                if let Some(label) = label(&row_value.value) {
                    entry.result = Some(label);
                    relabelled = true;
                }
            }
            if relabelled {
                value.result = Some(
                    entries
                        .iter()
                        .map(|entry| {
                            entry
                                .result
                                .clone()
                                .expect("BUG: a list entry narrates with its display")
                        })
                        .collect::<Vec<_>>()
                        .join(", "),
                );
            }
        }
        _ => {}
    }
    value
}

fn arithmetic_operator_of(kind: &NormalFormKind) -> Option<ArithmeticComputation> {
    match kind {
        NormalFormKind::Sum(_) => Some(ArithmeticComputation::Add),
        NormalFormKind::Product(_) => Some(ArithmeticComputation::Multiply),
        NormalFormKind::Subtract(_, _) => Some(ArithmeticComputation::Subtract),
        NormalFormKind::Divide(_, _) => Some(ArithmeticComputation::Divide),
        NormalFormKind::Power(_, _) => Some(ArithmeticComputation::Power),
        NormalFormKind::Modulo(_, _) => Some(ArithmeticComputation::Modulo),
        _ => None,
    }
}

/// Display key of a data path: qualified by the written row whose context it
/// narrates in (`people.1.age`), bare anywhere else.
fn qualified_input_key(qualifier: Option<&NameScope>, path: &DataPath) -> String {
    qualified_data_path(qualifier, path).input_key()
}

/// Data `path` of the context narrated as the plan holding its instance names it.
fn qualified_data_path(qualifier: Option<&NameScope>, path: &DataPath) -> DataPath {
    match qualifier {
        Some(scope) if !scope.data.is_empty() => DataPath {
            segments: scope
                .data
                .iter()
                .chain(path.segments.iter())
                .cloned()
                .collect(),
            data: path.data.clone(),
        },
        Some(_) | None => path.clone(),
    }
}

/// Rule `path` of the context narrated as the plan holding its instance names it.
fn qualified_rule_path(qualifier: Option<&NameScope>, path: &RulePath) -> RulePath {
    match qualifier {
        Some(scope) if !scope.rules.is_empty() => RulePath {
            segments: scope
                .rules
                .iter()
                .chain(path.segments.iter())
                .cloned()
                .collect(),
            rule: path.rule.clone(),
        },
        Some(_) | None => path.clone(),
    }
}

fn data_node(
    path: &DataPath,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
    qualifier: Option<&NameScope>,
) -> ExplanationNode {
    let result = resolve_data_path_value(path, plan, ctx);
    let data_type = EvaluationContext::data_display_type(plan, path, &result);
    let name = qualified_data_path(qualifier, path);
    ExplanationNode::Data {
        name,
        result: explained_value(&result, data_type.as_ref(), plan, ctx),
    }
}

enum PiecewiseNarration {
    /// Not-taken arms after the winner in source order, then the winner (or
    /// every not-taken arm when the default wins), plus the body that won.
    Causes {
        causes: Vec<Cause>,
        winner_body: NormalFormId,
    },
    /// A condition vetoed: the veto is the value and the whole narration.
    Veto(Box<Narrated>),
}

/// One cause builder for every Piecewise record: the live arms of a Piecewise
/// cell, or the Piecewise pre-image of a collapsed / dispatched cell. Winner from
/// [`piecewise_decision`] over the condition slots the walk filled.
fn piecewise_causes(
    arms: &[(NormalFormId, NormalFormId)],
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
    done: &Done,
    qualifier: Option<&NameScope>,
    session: &mut NarrationSession<'_>,
) -> Need<PiecewiseNarration> {
    let decision = piecewise_decision(arms.len(), |arm| {
        Some(condition_outcome(&slot(arms[arm].0, plan, ctx)))
    });
    let winner: Option<usize> = match decision {
        PiecewiseDecision::Taken { arm } => Some(arm),
        PiecewiseDecision::Default => None,
        PiecewiseDecision::Undecided { arm } => {
            let condition = arms[arm].0;
            let result = slot(condition, plan, ctx);
            assert!(
                result.vetoed(),
                "BUG: piecewise scan undecided on a filled non-veto condition slot"
            );
            let node = ExplanationNode::Veto {
                message: Some(format_operation_result(
                    &result,
                    plan.result_type(condition).as_ref(),
                )),
            };
            return Ok(PiecewiseNarration::Veto(Box::new(Narrated {
                body: explanation_display(plan.normal_forms.as_slice(), condition, qualifier),
                causes: Vec::new(),
                as_operand: Some(node),
            })));
        }
    };
    let mut causes = Vec::new();
    // Arms after the winner (every unless when the default wins), source order.
    let after = winner.map(|arm| arm + 1).unwrap_or(1);
    for (condition, _) in arms.iter().skip(after) {
        match condition_outcome(&slot(*condition, plan, ctx)) {
            BranchOutcome::NotTaken => causes.push(cause(
                *condition, false, plan, ctx, done, qualifier, session,
            )?),
            BranchOutcome::Taken | BranchOutcome::Propagate(_) => {}
        }
    }
    let winner_body = match winner {
        Some(arm) => {
            causes.push(cause(
                arms[arm].0,
                true,
                plan,
                ctx,
                done,
                qualifier,
                session,
            )?);
            arms[arm].1
        }
        None => arms[0].1,
    };
    Ok(PiecewiseNarration::Causes {
        causes,
        winner_body,
    })
}

/// Cross-check a dispatched cell with a value: the region its scrutinee slot
/// selects must resolve (through any nested dispatch) to the body the Piecewise
/// record's winner names. Two decision procedures (planning fold, source record)
/// must not disagree.
fn assert_dispatch_agrees_with_record(
    id: NormalFormId,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
    done: &Done,
    qualifier: Option<&NameScope>,
    session: &mut NarrationSession<'_>,
) -> Need<()> {
    let cell = plan.normal_form(id);
    let NormalFormKind::OrderedDispatch {
        scrutinee,
        boundaries,
        regions,
    } = &cell.kind
    else {
        unreachable!("BUG: assert_dispatch_agrees_with_record on a non-dispatch cell")
    };
    if slot(id, plan, ctx).vetoed() {
        return Ok(());
    }
    let origin = cell
        .origin
        .expect("BUG: OrderedDispatch always carries its Piecewise pre-image as origin");
    let NormalFormKind::Piecewise(arms) = &plan.normal_form(origin).kind else {
        panic!("BUG: OrderedDispatch origin must be Piecewise");
    };
    let scrutinee_result = slot(*scrutinee, plan, ctx);
    let scrutinee_value = borrow_value(&scrutinee_result, "dispatch scrutinee");
    let region = region_of_scrutinee(boundaries, &scrutinee_value.value).unwrap_or_else(|veto| {
        panic!("BUG: OrderedDispatch region check vetoed after a non-veto dispatch value: {veto}")
    });
    let PiecewiseNarration::Causes { winner_body, .. } =
        piecewise_causes(arms, plan, ctx, done, qualifier, session)?
    else {
        panic!("BUG: OrderedDispatch value is not a veto but its record scan is undecided")
    };
    let resolved =
        resolve_nested_dispatch_body(plan, &ctx.values, regions[region]).unwrap_or_else(|| {
            panic!("BUG: OrderedDispatch nested region body undecided after a non-veto value")
        });
    assert_eq!(
        resolved, winner_body,
        "BUG: OrderedDispatch region disagrees with Piecewise origin"
    );
    Ok(())
}

fn cause(
    condition: NormalFormId,
    held: bool,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
    done: &Done,
    qualifier: Option<&NameScope>,
    session: &mut NarrationSession<'_>,
) -> Need<Cause> {
    let root = peel_bool_leaf_origins(condition, plan);
    let root_is_logical = matches!(plan.normal_form(root).kind, NormalFormKind::Logical(..));
    // Not-taken Logical causes show the full authored expression so two
    // unless arms that share a failing left conjunct stay distinguishable.
    let show_full_logical = !held && root_is_logical;

    let mut focus = root;
    loop {
        let NormalFormKind::Logical(connective, children) = &plan.normal_form(focus).kind else {
            break;
        };
        if held != connective.absorbing_operand() {
            break;
        }
        let deciding = deciding_logical_operand(*connective, children, plan, ctx);
        focus = peel_bool_leaf_origins(deciding, plan);
    }

    let (condition_text, value) = if show_full_logical {
        let forms = plan.normal_forms.as_slice();
        (
            explanation_display(forms, root, qualifier),
            "false".to_string(),
        )
    } else {
        condition_statement(focus, held, plan, qualifier)
    };

    let children = if show_full_logical {
        let NormalFormKind::Logical(connective, _) = &plan.normal_form(root).kind else {
            unreachable!("BUG: show_full_logical requires a Logical root")
        };
        if held == connective.absorbing_operand() {
            short_circuit_cause_children(root, plan, ctx, qualifier, session)
        } else {
            premise_cause_children(root, plan, ctx, qualifier, session)
        }
    } else {
        match (&plan.normal_form(focus).kind, held) {
            (NormalFormKind::Logical(connective, ..), polarity)
                if polarity != connective.absorbing_operand() =>
            {
                premise_cause_children(focus, plan, ctx, qualifier, session)
            }
            (NormalFormKind::Logical(..), _) => {
                unreachable!("BUG: absorbing Logical must be peeled before cause children")
            }
            (NormalFormKind::Comparison(..), true) => {
                premise_cause_children(focus, plan, ctx, qualifier, session)
            }
            _ => cause_children(child(done, focus)?.as_operand.clone()),
        }
    };

    Ok(Cause {
        condition: condition_text,
        value,
        children,
    })
}

/// The absorbing operand that decides a settled `Logical`: the first child in source
/// order whose outcome matches the connective's absorbing polarity (`Taken` for `or`,
/// `NotTaken` for `and`). Operands before it were walked, so their outcomes are known;
/// a missing or vetoed operand never decides it.
fn deciding_logical_operand(
    connective: LogicalConnective,
    children: &[NormalFormId],
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
) -> NormalFormId {
    let absorbing = connective.absorbing_operand();
    children
        .iter()
        .copied()
        .find(|&child| {
            matches!(
                (absorbing, try_condition_outcome(child, plan, ctx)),
                (true, Some(BranchOutcome::Taken)) | (false, Some(BranchOutcome::NotTaken))
            )
        })
        .unwrap_or_else(|| panic!("BUG: absorbing Logical without a deciding operand"))
}

/// Condition outcome when the cell was walked, or a data leaf that resolves
/// through its binding. `None` if a non-data cell was never evaluated.
fn try_condition_outcome(
    id: NormalFormId,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
) -> Option<BranchOutcome> {
    let cell = plan.normal_form(id);
    if cell.rule_ref.is_none() {
        if let NormalFormKind::Leaf(LeafKind::DataPath(path)) = &cell.kind {
            return Some(condition_outcome(&resolve_data_path_value(path, plan, ctx)));
        }
    }
    ctx.values[id.index()].as_ref().map(condition_outcome)
}

fn is_bool_leaf(plan: &ExecutionPlan, id: NormalFormId) -> Option<bool> {
    match &plan.normal_form(id).kind {
        NormalFormKind::Leaf(LeafKind::Literal(literal)) => match &literal.value {
            ValueKind::Boolean(b) => Some(*b),
            _ => None,
        },
        _ => None,
    }
}

/// Peel bool-leaf origins until Comparison, Not, And, or a non-bool-leaf.
fn peel_bool_leaf_origins(condition: NormalFormId, plan: &ExecutionPlan) -> NormalFormId {
    let mut id = condition;
    loop {
        if is_bool_leaf(plan, id).is_none() {
            return id;
        }
        let Some(origin) = plan.normal_form(id).origin else {
            return id;
        };
        match &plan.normal_form(origin).kind {
            NormalFormKind::Comparison(_, _, _)
            | NormalFormKind::Not(_)
            | NormalFormKind::Logical(..) => return origin,
            NormalFormKind::Leaf(LeafKind::Literal(literal))
                if matches!(literal.value, ValueKind::Boolean(_)) =>
            {
                id = origin;
            }
            _ => return origin,
        }
    }
}

fn condition_statement(
    mut condition: NormalFormId,
    mut held: bool,
    plan: &ExecutionPlan,
    qualifier: Option<&NameScope>,
) -> (String, String) {
    while let NormalFormKind::Not(inner) = &plan.normal_form(condition).kind {
        condition = *inner;
        held = !held;
    }
    let forms = plan.normal_forms.as_slice();
    match &plan.normal_form(condition).kind {
        NormalFormKind::Comparison(a, op, b) => {
            let op = if held {
                op.clone()
            } else {
                negated_comparison(op.clone())
            };
            (
                display_comparison(
                    NfNode {
                        forms,
                        id: *a,
                        qualifier,
                    },
                    &op,
                    NfNode {
                        forms,
                        id: *b,
                        qualifier,
                    },
                ),
                "true".to_string(),
            )
        }
        NormalFormKind::Not(_) => unreachable!("BUG: Not peeled before the condition match"),
        NormalFormKind::Leaf(LeafKind::DataPath(path)) => (
            if held {
                qualified_input_key(qualifier, path)
            } else {
                format!("{} is false", qualified_input_key(qualifier, path))
            },
            "true".to_string(),
        ),
        NormalFormKind::Leaf(LeafKind::Literal(literal))
            if matches!(literal.value, ValueKind::Boolean(_)) =>
        {
            (
                explanation_display(forms, condition, qualifier),
                if held { "true" } else { "false" }.to_string(),
            )
        }
        NormalFormKind::Logical(..)
        | NormalFormKind::RangeContainment(..)
        | NormalFormKind::DateRelative(..)
        | NormalFormKind::DateCalendar(..)
        | NormalFormKind::ResultIsVeto(_)
        | NormalFormKind::UnitConversion(..)
        | NormalFormKind::Piecewise(_)
        | NormalFormKind::OrderedDispatch { .. }
        | NormalFormKind::SpecMember { .. }
        | NormalFormKind::ListCommand { .. } => (
            explanation_display(forms, condition, qualifier),
            if held { "true" } else { "false" }.to_string(),
        ),
        NormalFormKind::Sum(_)
        | NormalFormKind::Product(_)
        | NormalFormKind::Subtract(..)
        | NormalFormKind::Divide(..)
        | NormalFormKind::Power(..)
        | NormalFormKind::Modulo(..)
        | NormalFormKind::Negate(_)
        | NormalFormKind::Reciprocal(_)
        | NormalFormKind::MathOp(..)
        | NormalFormKind::StepRounding(..)
        | NormalFormKind::Now
        | NormalFormKind::Veto(_)
        | NormalFormKind::RangeLiteral(..)
        | NormalFormKind::PastFutureRange(..)
        | NormalFormKind::RangeBound(..)
        | NormalFormKind::Leaf(LeafKind::Literal(_)) => {
            unreachable!("BUG: non-boolean condition in condition_statement")
        }
        NormalFormKind::RowField { .. } | NormalFormKind::Leaf(LeafKind::RowValue { .. }) => {
            unreachable!("BUG: row leaf narrated outside its list command")
        }
    }
}

/// Operands of a condition worth listing under its cause line.
fn cause_children(operand: Option<ExplanationNode>) -> Vec<ExplanationNode> {
    match operand {
        // Bare data ref is fully stated in the condition text.
        Some(ExplanationNode::Data { .. } | ExplanationNode::DataUnused { .. }) => Vec::new(),
        Some(ExplanationNode::Compose { operands, .. })
        | Some(ExplanationNode::ListCommand { operands, .. }) => operands
            .into_iter()
            .filter(|node| {
                matches!(
                    node,
                    ExplanationNode::Data { .. }
                        | ExplanationNode::DataUnused { .. }
                        | ExplanationNode::RuleRef { .. }
                        | ExplanationNode::Conversion { .. }
                )
            })
            .collect(),
        Some(node @ ExplanationNode::RuleRef { .. }) => vec![node],
        Some(node @ ExplanationNode::Conversion { .. }) => vec![node],
        Some(node @ ExplanationNode::Veto { .. }) => vec![node],
        Some(ExplanationNode::Row { .. } | ExplanationNode::Filter { .. }) => {
            unreachable!("BUG: row/filter node as bare cause operand")
        }
        None => Vec::new(),
    }
}

/// Held `and` / comparison premises: surface facts only, stopping at `rule_ref`
/// cuts (same cut as [`narrate_step`]). Does not walk into a referenced rule's
/// body, so `views_consumed < max_views` lists `max_views` as a RuleRef rather
/// than leaking `max_views`'s own data (e.g. `premium_member`).
fn premise_cause_children(
    focus: NormalFormId,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
    qualifier: Option<&NameScope>,
    session: &mut NarrationSession<'_>,
) -> Vec<ExplanationNode> {
    let mut children = Vec::new();
    let mut seen_data = HashSet::new();
    let mut seen_rules = HashSet::new();
    collect_premise_facts(
        focus,
        plan,
        ctx,
        &mut PremiseSink {
            out: &mut children,
            seen_data: &mut seen_data,
            seen_rules: &mut seen_rules,
        },
        qualifier,
        session,
    );
    children
}

/// Children for a not-taken `Logical` cause shown in full: premises of the
/// deciding (walked) conjunct, then `DataUnused` for data paths in the
/// operands the walk skipped (short-circuit RHS). The deciding operand is
/// the first absorbing one (`false` for `and`, `true` for `or`); operands
/// after it were never evaluated.
fn short_circuit_cause_children(
    root: NormalFormId,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
    qualifier: Option<&NameScope>,
    session: &mut NarrationSession<'_>,
) -> Vec<ExplanationNode> {
    let NormalFormKind::Logical(connective, children) = &plan.normal_form(root).kind else {
        unreachable!("BUG: short_circuit_cause_children on a non-Logical root")
    };
    let absorbing = connective.absorbing_operand();
    let deciding_index = children
        .iter()
        .position(|&child| {
            matches!(
                (absorbing, try_condition_outcome(child, plan, ctx)),
                (true, Some(BranchOutcome::Taken)) | (false, Some(BranchOutcome::NotTaken))
            )
        })
        .unwrap_or_else(|| panic!("BUG: absorbing Logical without a deciding operand"));
    let deciding = peel_bool_leaf_origins(children[deciding_index], plan);

    let mut out = Vec::new();
    let mut seen_data = HashSet::new();
    let mut seen_rules = HashSet::new();
    collect_premise_facts(
        deciding,
        plan,
        ctx,
        &mut PremiseSink {
            out: &mut out,
            seen_data: &mut seen_data,
            seen_rules: &mut seen_rules,
        },
        qualifier,
        session,
    );
    for skipped in &children[deciding_index + 1..] {
        collect_unused_data_paths(*skipped, plan, &mut out, &mut seen_data, qualifier);
    }
    out
}

/// Walk `id` and add every `DataPath` leaf as `DataUnused`, skipping ones
/// already seen. Used for the operands a short-circuit walk never evaluated.
fn collect_unused_data_paths(
    id: NormalFormId,
    plan: &ExecutionPlan,
    out: &mut Vec<ExplanationNode>,
    seen_data: &mut HashSet<DataPath>,
    qualifier: Option<&NameScope>,
) {
    let mut stack: Vec<(&ExecutionPlan, NormalFormId, Vec<PathSegment>)> =
        vec![(plan, id, Vec::new())];
    while let Some((holder, id, hops)) = stack.pop() {
        let cell = holder.normal_form(id);
        match &cell.kind {
            NormalFormKind::Leaf(LeafKind::DataPath(path))
                if matches!(holder.data.get(path), Some(DataDefinition::Import { .. })) => {}
            NormalFormKind::Leaf(LeafKind::DataPath(path)) => {
                let path = DataPath {
                    segments: hops.iter().chain(&path.segments).cloned().collect(),
                    data: path.data.clone(),
                };
                let name = qualified_data_path(qualifier, &path);
                if seen_data.insert(path) {
                    out.push(ExplanationNode::DataUnused { name });
                }
            }
            NormalFormKind::SpecMember { base, row, .. }
                if matches!(
                    &holder.normal_form(*base).kind,
                    NormalFormKind::Leaf(LeafKind::DataPath(path))
                        if matches!(holder.data.get(path), Some(DataDefinition::Import { .. }))
                ) =>
            {
                let NormalFormKind::Leaf(LeafKind::DataPath(slot)) =
                    &holder.normal_form(*base).kind
                else {
                    unreachable!("BUG: matched an import slot base above");
                };
                let (end, member_hops) = row
                    .ends
                    .split_last()
                    .expect("BUG: a member read has a last end");
                let mut hops = hops.clone();
                let mut template = holder;
                for slot in std::iter::once(slot).chain(member_hops.iter().map(|hop| match hop {
                    RowEnd::Data(slot) => slot,
                    RowEnd::Rule(rule) => {
                        panic!("BUG: member hop '{rule}' is a rule, not a `uses` slot")
                    }
                })) {
                    let Some(DataDefinition::Import { row, .. }) = template.data.get(slot) else {
                        panic!("BUG: member hop '{slot}' is not a `uses` slot");
                    };
                    hops.push(PathSegment::import(&slot.data, &row.identity));
                    template = plan.row_template(&row.identity);
                }
                match end {
                    RowEnd::Data(path) => {
                        let path = DataPath {
                            segments: hops,
                            data: path.data.clone(),
                        };
                        let name = qualified_data_path(qualifier, &path);
                        if seen_data.insert(path) {
                            out.push(ExplanationNode::DataUnused { name });
                        }
                    }
                    RowEnd::Rule(rule) => {
                        let body = template
                            .rules
                            .get(rule)
                            .unwrap_or_else(|| {
                                panic!("BUG: member rule '{rule}' missing from its template")
                            })
                            .normal_form;
                        stack.push((template, body, hops));
                    }
                }
            }
            _ => {
                for child in cell.kind.children().into_iter().rev() {
                    stack.push((holder, child, hops.clone()));
                }
            }
        }
    }
}

/// Accumulator for [`collect_premise_facts`]: emitted nodes plus dedupe sets.
struct PremiseSink<'a> {
    out: &'a mut Vec<ExplanationNode>,
    seen_data: &'a mut HashSet<DataPath>,
    seen_rules: &'a mut HashSet<RulePath>,
}

fn collect_premise_facts(
    id: NormalFormId,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
    sink: &mut PremiseSink<'_>,
    qualifier: Option<&NameScope>,
    session: &mut NarrationSession<'_>,
) {
    enum Task {
        Facts(NormalFormId),
        Side { id: NormalFormId, omit_data: bool },
    }
    let mut stack = vec![Task::Facts(id)];
    while let Some(task) = stack.pop() {
        match task {
            Task::Facts(id) => {
                let cell = plan.normal_form(id);
                if let Some(path) = &cell.rule_ref {
                    if sink.seen_rules.insert(path.clone()) {
                        sink.out.push(rule_ref_node(
                            path,
                            qualified_rule_path(qualifier, path),
                            plan,
                            ctx,
                            qualifier,
                            session,
                        ));
                    }
                    continue;
                }
                match &cell.kind {
                    NormalFormKind::Logical(_, children) => {
                        for &child in children.iter().rev() {
                            stack.push(Task::Facts(child));
                        }
                    }
                    NormalFormKind::Comparison(a, op, b) => {
                        let omit_data = *op == ComparisonComputation::Is
                            && comparison_is_data_versus_literal(*a, *b, plan);
                        stack.push(Task::Side { id: *b, omit_data });
                        stack.push(Task::Side { id: *a, omit_data });
                    }
                    NormalFormKind::Not(inner) => {
                        stack.push(Task::Facts(*inner));
                    }
                    NormalFormKind::Leaf(LeafKind::DataPath(path)) => {
                        push_premise_data(path, plan, ctx, sink.out, sink.seen_data, qualifier);
                    }
                    NormalFormKind::Leaf(LeafKind::Literal(_)) => {}
                    _ if import_member_read(id, plan, ctx).is_some() => {
                        push_premise_import_member(
                            id,
                            plan,
                            ctx,
                            sink.out,
                            sink.seen_data,
                            sink.seen_rules,
                            session,
                        );
                    }
                    _ => {
                        for child in cell.kind.children().into_iter().rev() {
                            stack.push(Task::Facts(child));
                        }
                    }
                }
            }
            Task::Side { id, omit_data } => {
                let cell = plan.normal_form(id);
                if let Some(path) = &cell.rule_ref {
                    if sink.seen_rules.insert(path.clone()) {
                        sink.out.push(rule_ref_node(
                            path,
                            qualified_rule_path(qualifier, path),
                            plan,
                            ctx,
                            qualifier,
                            session,
                        ));
                    }
                    continue;
                }
                match &cell.kind {
                    NormalFormKind::Leaf(LeafKind::DataPath(path)) => {
                        if !omit_data {
                            push_premise_data(path, plan, ctx, sink.out, sink.seen_data, qualifier);
                        }
                    }
                    NormalFormKind::Leaf(LeafKind::Literal(_)) => {}
                    _ if import_member_read(id, plan, ctx)
                        .is_some_and(|read| matches!(read.end, RowEnd::Data(_))) =>
                    {
                        if !omit_data {
                            push_premise_import_member(
                                id,
                                plan,
                                ctx,
                                sink.out,
                                sink.seen_data,
                                sink.seen_rules,
                                session,
                            );
                        }
                    }
                    _ => stack.push(Task::Facts(id)),
                }
            }
        }
    }
}

/// The member read of cell `id` when it reads a member of a `uses` instance reached from
/// the root (`order.is_rush`): such a member is the rule or data of the instance, not a
/// row field.
fn import_member_read<'c>(
    id: NormalFormId,
    plan: &ExecutionPlan,
    ctx: &'c EvaluationContext,
) -> Option<&'c RowRead> {
    let NormalFormKind::SpecMember { base, .. } = &plan.normal_form(id).kind else {
        return None;
    };
    let base_result = slot(*base, plan, ctx);
    let OperationResult::Value(value) = &base_result else {
        return None;
    };
    let ValueKind::Spec(SpecValue::Row(instance)) = &value.value else {
        return None;
    };
    if !is_root_import(instance) {
        return None;
    }
    Some(
        ctx.row_reads
            .get(&id)
            .and_then(|reads| reads.last())
            .unwrap_or_else(|| panic!("BUG: member of {instance} narrated without its read")),
    )
}

fn push_premise_import_member(
    id: NormalFormId,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
    out: &mut Vec<ExplanationNode>,
    seen_data: &mut HashSet<DataPath>,
    seen_rules: &mut HashSet<RulePath>,
    session: &mut NarrationSession<'_>,
) {
    let read = import_member_read(id, plan, ctx).expect("BUG: premise cell is an import member");
    let node = import_member_narration(read, plan, ctx, session)
        .as_operand
        .expect("BUG: an import member narrates as an operand");
    let first = match &node {
        ExplanationNode::RuleRef { name, .. } => seen_rules.insert(name.clone()),
        ExplanationNode::Data { name, .. } => seen_data.insert(name.clone()),
        _ => true,
    };
    if first {
        out.push(node);
    }
}

fn push_premise_data(
    path: &DataPath,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
    out: &mut Vec<ExplanationNode>,
    seen_data: &mut HashSet<DataPath>,
    qualifier: Option<&NameScope>,
) {
    if matches!(plan.data.get(path), Some(DataDefinition::Import { .. }))
        || !seen_data.insert(path.clone())
    {
        return;
    }
    if ctx.data_slot(plan, path).is_some() {
        out.push(data_node(path, plan, ctx, qualifier));
    } else {
        out.push(ExplanationNode::DataUnused {
            name: qualified_data_path(qualifier, path),
        });
    }
}

fn comparison_is_data_versus_literal(
    a: NormalFormId,
    b: NormalFormId,
    plan: &ExecutionPlan,
) -> bool {
    let left_data = cell_is_bare_data(a, plan);
    let right_data = cell_is_bare_data(b, plan);
    let left_literal = cell_is_bare_literal(a, plan);
    let right_literal = cell_is_bare_literal(b, plan);
    (left_data && right_literal) || (left_literal && right_data)
}

fn cell_is_bare_data(id: NormalFormId, plan: &ExecutionPlan) -> bool {
    let cell = plan.normal_form(id);
    if cell.rule_ref.is_some() {
        return false;
    }
    match &cell.kind {
        NormalFormKind::Leaf(LeafKind::DataPath(_)) => true,
        NormalFormKind::SpecMember { base, row, .. } => {
            let base_is_import = match &plan.normal_form(*base).kind {
                NormalFormKind::Leaf(LeafKind::DataPath(path)) => {
                    matches!(plan.data.get(path), Some(DataDefinition::Import { .. }))
                }
                _ => false,
            };
            base_is_import && matches!(row.ends.last(), Some(RowEnd::Data(_)))
        }
        _ => false,
    }
}

fn cell_is_bare_literal(id: NormalFormId, plan: &ExecutionPlan) -> bool {
    let cell = plan.normal_form(id);
    cell.rule_ref.is_none() && matches!(cell.kind, NormalFormKind::Leaf(LeafKind::Literal(_)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsing::ast::EffectiveDate;
    use crate::planning::semantics::{PathSegment, SpecIdentity};

    fn identity(spec: &str) -> SpecIdentity {
        SpecIdentity {
            repository: None,
            spec: spec.to_string(),
            effective: EffectiveDate::Origin,
            slice: EffectiveDate::Origin,
        }
    }

    #[test]
    fn rules_of_a_uses_instance_are_named_under_its_hops() {
        let sub = identity("sub");
        let scope = NameScope {
            data: vec![PathSegment::import("i", &sub)],
            rules: vec![PathSegment::import("i", &sub)],
        };
        let rule = RulePath::new(Vec::new(), "total".to_string());
        assert_eq!(
            qualified_rule_path(Some(&scope), &rule).input_key(),
            "i.total"
        );
        let data = DataPath::local("x".to_string());
        assert_eq!(qualified_data_path(Some(&scope), &data).input_key(), "i.x");
    }

    #[test]
    fn rules_of_a_written_row_stay_bare_and_its_data_names_the_row() {
        let employee = identity("employee");
        let scope = NameScope {
            data: vec![PathSegment::list_row_shared(
                &Arc::from("staff"),
                2,
                &Arc::from(employee.spec.as_str()),
                None,
            )],
            rules: Vec::new(),
        };
        let rule = RulePath::new(Vec::new(), "bonus".to_string());
        assert_eq!(qualified_rule_path(Some(&scope), &rule), rule);
        let data = DataPath::local("age".to_string());
        assert_eq!(
            qualified_data_path(Some(&scope), &data).input_key(),
            "staff.2.age"
        );
    }
}

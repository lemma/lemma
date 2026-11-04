//! Value walk: evaluates [`NormalForm`] cells by [`NormalFormId`] into the
//! request value table. Produces values only; narration is a separate pass
//! over the filled table ([`crate::evaluation::narration`]).
//!
//! Policy: [`EvaluationContext::exhaustive`] (set for explain runs) makes the
//! walk visit every cell narration displays: the pre-image (`origin`) of every
//! rewritten cell, and arithmetic siblings after a definitive veto. It never
//! changes a value. `and` is order-independent in both policies: a `false`
//! operand ends the `and`; a missing or vetoed operand is kept while the walk
//! continues, so a later `false` can still decide. Unless arms after the
//! winner are already visited by the last-wins reverse scan.
//!
//! Machine: every evaluation is a [`Frame`] on one heap stack. A frame's step either
//! finishes with a value, spawns a child frame and waits for it, or enters / leaves the
//! context of an instance-list row supplied by evaluation data. Scopes are owned by a
//! second heap stack, so no Rust call stack grows with expression depth, rule chains,
//! self-referencing row trees or list nesting. A `uses` import is a row: its slot reads
//! as the instance, and member reads enter the instance's context like any row.

use crate::computation::datetime::{
    compute_date_calendar, compute_date_relative, evaluate_past_future_range,
};
use crate::computation::measure_math::{integer_rounding, step_rounding};
use crate::computation::range::{check_containment, ordered_endpoints};
use crate::computation::rational::{rational_one, rational_zero};
use crate::computation::{
    arithmetic_operation, comparison_operation, convert_unit_operand, OperationResult, VetoType,
};
use crate::evaluation::branch_semantics::{condition_outcome, BranchOutcome};
use crate::evaluation::expression::{evaluate_mathematical_operator, resolve_data_path_value};
use crate::evaluation::list_ops::{self, ListFold};
use crate::evaluation::rows::{self, RowRead};
use crate::evaluation::{CurrentRow, EvaluationContext, RowLogMark};
use crate::parsing::ast::LogicalConnective;
use crate::planning::execution_plan::{ExecutionPlan, RuleIndex};
use crate::planning::instance_list::{
    row_end_type, typed_from_bound, InstanceListSource, KeyIndex,
};
use crate::planning::normalize::{
    ArithmeticTerm, LeafKind, NormalFormId, NormalFormKind, NormalFormListOp,
};
use crate::planning::ordered_dispatch::{region_count, region_of_scrutinee};
use crate::planning::semantics::{
    primitive_boolean_arc, primitive_number_arc, typed_literal_from_bound, ArithmeticComputation,
    BoundValueKind, ComparisonComputation, DataDefinition, DataPath, InstanceKey, KeyField,
    LemmaType, ListKey, ListValue, LiteralValue, RangeBound, ReferenceEnd, ReferenceTarget,
    RowBinding, RowEnd, RowOrigin, RulePath, SemanticDateTime, SpecValue, TypedLiteral, ValueKind,
};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub(crate) fn borrow_value<'a>(result: &'a OperationResult, operand: &str) -> &'a BoundValueKind {
    match result {
        OperationResult::Value(v) => v,
        OperationResult::Veto(_) => panic!("BUG: {operand} passed veto check but has no value"),
    }
}

fn now_date(ctx: &EvaluationContext) -> &SemanticDateTime {
    match &ctx.now().value {
        ValueKind::Date(dt) => dt,
        other => panic!("BUG: context.now() must be a date, got {other:?}"),
    }
}

/// Fill `rule_values` for `rule` and every rule reference its walk visits.
pub(crate) fn ensure_rule_values(
    rule: RuleIndex,
    plan: &ExecutionPlan,
    ctx: &mut EvaluationContext,
) {
    run(plan, ctx, Frame::rule(rule));
}

/// Value of rule `rule` of `uses` instance `key`, read from the root scope of `plan` as a
/// member read of it is. Leaves the instance's context in the row cache.
pub(crate) fn read_instance_rule(
    key: &InstanceKey,
    rule: &RulePath,
    plan: &ExecutionPlan,
    ctx: &mut EvaluationContext,
) -> OperationResult {
    run(
        plan,
        ctx,
        Frame::read_row(key.clone(), RowEnd::Rule(rule.clone()), None),
    )
    .expect("BUG: a row read delivers a value")
}

/// Context of `row` over `template`, taken from `ctx` when built or built
/// bottom-up. `plan` owns the list the row was read from: deferred bindings resolve
/// against its rules and data. The row is materialized: every binding resolves.
pub(crate) fn build_row_context(
    key: &InstanceKey,
    plan: &ExecutionPlan,
    template: &ExecutionPlan,
    ctx: &mut EvaluationContext,
) -> EvaluationContext {
    run(
        plan,
        ctx,
        Frame::row_scope(key.clone(), template, RowBody::Materialize),
    );
    ctx.caches
        .row_contexts
        .borrow_mut()
        .swap_remove(key)
        .expect("BUG: row context missing after its scope was built")
}

/// Finished cells of one walk, by cell id. Values not stored in the value table live here.
type WalkDone = HashMap<NormalFormId, OperationResult>;

/// Result a frame hands to the frame that spawned it.
type Delivered = Option<OperationResult>;

fn ready(id: NormalFormId, ctx: &EvaluationContext, walk: &WalkDone) -> Option<OperationResult> {
    if ctx.option_check_pending(id.index()) {
        return None;
    }
    walk.get(&id)
        .cloned()
        .or_else(|| ctx.values[id.index()].clone())
}

/// Store policy for a finished cell. Unbound data leaves must stay None so
/// missing_data walks still see them as unbound. Cells that read a list row change
/// value per row and are never cached.
fn finish(
    id: NormalFormId,
    result: OperationResult,
    plan: &ExecutionPlan,
    ctx: &mut EvaluationContext,
) -> OperationResult {
    let cell = plan.normal_form(id);
    let store = plan.row_mask[id.index()] == 0
        && !matches!(
            (&cell.kind, &result),
            (
                NormalFormKind::Leaf(LeafKind::DataPath(_)),
                OperationResult::Veto(VetoType::MissingData { .. }),
            )
        );
    if store {
        ctx.values[id.index()] = Some(result.clone());
    }
    result
}

/// A nested context that is current while its frames run.
struct Scope<'p> {
    plan: &'p ExecutionPlan,
    ctx: EvaluationContext,
}

#[derive(Clone, Copy)]
enum WalkMode {
    /// Child shares the spawner's walk memo.
    Same,
    /// Child starts a walk memo of its own, as a separate evaluation call did.
    New,
}

enum Outcome<'p> {
    /// The frame is complete with this value.
    Finish(Delivered),
    /// The frame waits for this child; the child's result comes back to its inbox.
    Spawn(Frame<'p>, WalkMode),
    /// The frame's context becomes current; the frame runs again inside it.
    Enter(Scope<'p>),
    /// The current row scope is left, its context returned to the row's cache; the
    /// frame completes.
    Exit(Delivered, InstanceKey),
}

fn spawn<'p>(frame: Frame<'p>, mode: WalkMode) -> Outcome<'p> {
    Outcome::Spawn(frame, mode)
}

struct Frame<'p> {
    kind: Kind<'p>,
    /// Index of the walk memo this frame reads and writes.
    walk: usize,
    owns_walk: bool,
    /// Scope this frame steps in: 0 for the root scope, `n` for the `n`-th
    /// entered scope. Suspended frames keep their depth while deeper scopes
    /// run above them, so a deferred row reference can spawn its target rule
    /// in the ancestor scope that owns it. [`SCOPE_INHERIT`] until placed.
    depth: usize,
    /// Value delivered by the child this frame waits for.
    inbox: Delivered,
}

/// [`Frame::depth`] of a frame not yet placed: the spawner's depth, or the root
/// scope for the run's first frame.
const SCOPE_INHERIT: usize = usize::MAX;

enum Kind<'p> {
    Cell(CellFrame),
    Rule(RuleFrame),
    RowScope(RowScopeFrame<'p>),
    ReadRow(ReadRowFrame),
    ListOp(ListOpFrame<'p>),
}

impl<'p> Frame<'p> {
    fn of(kind: Kind<'p>) -> Self {
        Self {
            kind,
            walk: 0,
            owns_walk: false,
            depth: SCOPE_INHERIT,
            inbox: None,
        }
    }

    fn cell(id: NormalFormId) -> Self {
        Self::of(Kind::Cell(CellFrame {
            id,
            cursor: 0,
            acc: None,
            veto: None,
            settled: None,
            resume: None,
            member_mark: None,
        }))
    }

    fn rule(index: RuleIndex) -> Self {
        Self::of(Kind::Rule(RuleFrame {
            index,
            depth: 0,
            saved_rows: None,
        }))
    }

    fn row_scope(key: InstanceKey, template: &'p ExecutionPlan, body: RowBody) -> Self {
        Self::of(Kind::RowScope(RowScopeFrame {
            key,
            template,
            body,
            phase: RowPhase::Acquire,
            read_slots: Vec::new(),
        }))
    }

    /// Field `end` of row `key`, read in the row's context. `position` is the row's
    /// 0-based place in the list a list command iterates.
    fn read_row(key: InstanceKey, end: RowEnd, position: Option<usize>) -> Self {
        let read = rows::ReadRow {
            key: key.clone(),
            position,
        };
        Self::of(Kind::ReadRow(ReadRowFrame {
            read,
            key,
            end,
            rule_root: None,
        }))
    }

    fn list_op(
        id: NormalFormId,
        list: NormalFormId,
        op: &'p NormalFormListOp,
        args: &'p [NormalFormId],
        row_level: Option<u32>,
        result_type: &'p Arc<LemmaType>,
    ) -> Self {
        Self::of(Kind::ListOp(ListOpFrame {
            id,
            list,
            op,
            args,
            row_level,
            result_type,
            state: ListState::Start,
        }))
    }
}

/// One cell being evaluated. Children are evaluated by spawning frames, so no Rust call
/// stack grows with expression depth.
struct CellFrame {
    id: NormalFormId,
    /// Progress through a sequence of children: piecewise arms from the top, fold
    /// operands from the first, list command arguments from the first.
    cursor: usize,
    acc: Option<OperationResult>,
    veto: Option<OperationResult>,
    /// Value computed while the exhaustive origin visit is still pending.
    settled: Option<OperationResult>,
    /// Set while an operator child (not a cell of this walk) is awaited.
    resume: Option<Resume>,
    /// Row log mark of a member read past its first hop.
    member_mark: Option<RowLogMark>,
}

enum Resume {
    Plain,
    /// Row read or list command: the row logs written since `mark` are recorded.
    RowLog(RowLogMark),
    /// One hop of a member read: the next hop reads on the instance it yields; the row
    /// logs written since `mark` are recorded when the last hop is read.
    Member(RowLogMark),
}

struct RuleFrame {
    index: RuleIndex,
    /// Depth of the active scope stack; names the current evaluation context.
    depth: usize,
    /// Row stack of the requesting walk, cleared while the rule evaluates.
    saved_rows: Option<Vec<CurrentRow>>,
}

enum RowBody {
    /// Only the row context is built; nothing is forced beyond the row's bindings.
    Nothing,
    /// The row is materialized: every binding resolves, the whole self-list tree is
    /// built. Used by response and options walks, never mid-evaluation.
    Materialize,
    /// The row's rule value, read after the context is built.
    Rule(RuleIndex),
    /// A row data leaf, read after the context is built.
    Leaf(NormalFormId),
}

enum RowPhase {
    Acquire,
    /// Deferred bindings of the fields the upcoming body reads, whose rule targets
    /// are not evaluated yet; forced one at a time from this index.
    Resolve(usize),
    /// Self-list rows still to build, last first.
    Children(Vec<InstanceKey>),
    Body,
    Await,
}

struct RowScopeFrame<'p> {
    key: InstanceKey,
    template: &'p ExecutionPlan,
    body: RowBody,
    phase: RowPhase,
    /// Fields the body reads; deferred bindings there resolve in [`RowPhase::Resolve`]
    /// before the context opens.
    read_slots: Vec<DataPath>,
}

struct ReadRowFrame {
    key: InstanceKey,
    /// Field read in the row's plan.
    end: RowEnd,
    /// The row as narration sees the read.
    read: rows::ReadRow,
    /// Root of the row's rule when the field is a rule, for missing-data keys.
    rule_root: Option<NormalFormId>,
}

struct ListData {
    rows: ListValue,
    unit: Option<Arc<str>>,
}

enum ListState {
    /// Before the list operand is requested.
    Start,
    AwaitList,
    /// Operands collected in order; `count` of them are needed.
    Args {
        data: ListData,
        values: Vec<OperationResult>,
        count: usize,
        key_fields: Option<Vec<KeyField>>,
    },
    /// Per-row argument values of an instance list, in row order.
    PerRow {
        data: ListData,
        cursor: usize,
        values: Vec<TypedLiteral>,
        first_veto: Option<OperationResult>,
    },
    /// Deferred rule targets inside `WithRow` operand values, forced one at a time in
    /// the command's scope before the inline row binds them: the row spec's rules read
    /// the bound rows from the row's template scope, where only values survive.
    InlineDeferred {
        data: ListData,
        values: Vec<OperationResult>,
        count: usize,
        key_fields: Option<Vec<KeyField>>,
        pending: std::vec::IntoIter<TargetForce>,
    },
    /// Key-field reads over rows of an instance list.
    Scan(KeyScan),
}

/// Key values of a keyed list's rows, read one field at a time, then checked for
/// conflicts and indexed.
struct KeyScan {
    rows: ListValue,
    fields: Vec<KeyField>,
    row: usize,
    field: usize,
    key: Vec<TypedLiteral>,
    keyed: Vec<Vec<TypedLiteral>>,
    then: ScanThen,
}

/// What a finished [`KeyScan`] yields.
enum ScanThen {
    /// The scanned list itself, as `-> with` on a keyed list returns it.
    List(Option<Arc<str>>),
    /// The row with these key values, for `find` on a list not indexed yet.
    Find(Vec<TypedLiteral>),
}

struct ListOpFrame<'p> {
    /// The list command cell.
    id: NormalFormId,
    list: NormalFormId,
    op: &'p NormalFormListOp,
    args: &'p [NormalFormId],
    row_level: Option<u32>,
    result_type: &'p Arc<LemmaType>,
    state: ListState,
}

/// Runs `first` and everything it spawns until it completes; returns its value.
fn run<'p>(
    root_plan: &'p ExecutionPlan,
    root: &mut EvaluationContext,
    first: Frame<'p>,
) -> Delivered {
    let mut active_rules: HashSet<(usize, RuleIndex)> = HashSet::new();
    if let Kind::Rule(rule) = &first.kind {
        active_rules.insert((rule.depth, rule.index));
    }
    let mut frames = vec![first];
    let mut walks: Vec<WalkDone> = Vec::new();
    let mut scopes: Vec<Scope<'p>> = Vec::new();
    loop {
        let frame = frames
            .last_mut()
            .expect("BUG: evaluation machine ran out of frames");
        if frame.depth == SCOPE_INHERIT {
            frame.depth = 0;
        }
        let outcome = step(frame, root_plan, &mut *root, &mut scopes, &walks);
        if let Outcome::Spawn(child, WalkMode::New) = &outcome {
            if let Kind::Cell(cell) = &child.kind {
                let depth = if child.depth == SCOPE_INHERIT {
                    frame.depth
                } else {
                    child.depth
                };
                let (_, ctx) = layer(depth, root_plan, &mut *root, &mut scopes);
                if let Some(cached) = ctx.values[cell.id.index()].clone() {
                    if !ctx.option_check_pending(cell.id.index()) {
                        frame.inbox = Some(cached);
                        continue;
                    }
                }
            }
        }
        match outcome {
            Outcome::Spawn(mut child, mode) => {
                if child.depth == SCOPE_INHERIT {
                    child.depth = frame.depth;
                }
                match mode {
                    WalkMode::Same => child.walk = frame.walk,
                    WalkMode::New => {
                        walks.push(WalkDone::new());
                        child.walk = walks.len() - 1;
                        child.owns_walk = true;
                    }
                }
                if let Kind::Rule(rule) = &mut child.kind {
                    rule.depth = child.depth;
                    if !active_rules.insert((rule.depth, rule.index)) {
                        let (plan, _) = layer(rule.depth, root_plan, &mut *root, &mut scopes);
                        panic!(
                            "BUG: cyclic rule reference while ensuring values for '{}'",
                            plan.rule_at(rule.index).path
                        );
                    }
                }
                frames.push(child);
            }
            Outcome::Enter(scope) => {
                scopes.push(scope);
                walks.push(WalkDone::new());
                frame.walk = walks.len() - 1;
                frame.owns_walk = true;
                frame.depth = scopes.len();
            }
            Outcome::Finish(value) => {
                let frame = frames.pop().expect("BUG: the stepped frame is on top");
                if let Kind::Rule(rule) = &frame.kind {
                    assert!(
                        active_rules.remove(&(rule.depth, rule.index)),
                        "BUG: finished rule frame was not active"
                    );
                }
                let stored = match &frame.kind {
                    Kind::Cell(cell) => {
                        let depth = frame.depth;
                        let (_, ctx) = layer(depth, root_plan, &mut *root, &mut scopes);
                        ctx.values[cell.id.index()].is_some()
                    }
                    _ => false,
                };
                if let Some(result) = complete(frame, value, stored, &mut frames, &mut walks) {
                    return result;
                }
            }
            Outcome::Exit(value, row) => {
                let frame = frames.pop().expect("BUG: the stepped frame is on top");
                assert!(
                    scopes.len() == frame.depth,
                    "BUG: row scope at depth {} exited above {} entered scopes",
                    frame.depth,
                    scopes.len()
                );
                let scope = scopes.pop().expect("BUG: scope left without being entered");
                root.caches.row_contexts.borrow_mut().insert(row, scope.ctx);
                if let Some(result) = complete(frame, value, false, &mut frames, &mut walks) {
                    return result;
                }
            }
        }
    }
}

/// Plan and context of scope `depth`: the root scope at 0, the `depth`-th
/// entered scope above it. A suspended frame's scope stays on the stack while
/// deeper scopes run, so any depth a live frame holds is valid.
fn layer<'a, 'p>(
    depth: usize,
    root_plan: &'p ExecutionPlan,
    root: &'a mut EvaluationContext,
    scopes: &'a mut [Scope<'p>],
) -> (&'p ExecutionPlan, &'a mut EvaluationContext) {
    if depth == 0 {
        return (root_plan, root);
    }
    let scope = scopes
        .get_mut(depth - 1)
        .expect("BUG: frame holds a scope that already left");
    (scope.plan, &mut scope.ctx)
}

/// Scope owning a deferred binding's target: a live layer on the scope stack
/// (0 = root), or the owner row's cached context when its scope already left;
/// a row scope frame re-enters the cache entry.
enum DeferredOwner {
    Layer(usize),
    Cached,
}

/// Where the target of `owner` lives: the root scope for root-born
/// bindings; the current or a suspended ancestor scope while the owner row's
/// context is on the stack; the owner row's cache entry after its scope left.
fn deferred_owner(
    owner: Option<&InstanceKey>,
    depth: usize,
    root: &EvaluationContext,
    scopes: &[Scope],
) -> DeferredOwner {
    let Some(owner_row) = owner else {
        return DeferredOwner::Layer(0);
    };
    let current = if depth == 0 {
        &root.owner_row
    } else {
        &scopes
            .get(depth - 1)
            .expect("BUG: frame holds a scope that already left")
            .ctx
            .owner_row
    };
    if current.as_ref() == Some(owner_row) {
        return DeferredOwner::Layer(depth);
    }
    if let Some(position) = scopes
        .iter()
        .position(|scope| scope.ctx.owner_row.as_ref() == Some(owner_row))
    {
        return DeferredOwner::Layer(position + 1);
    }
    DeferredOwner::Cached
}

/// Finishes `frame` with `value` and delivers the value to the frame that spawned it.
/// `Some` when `frame` was the root and the run is over.
fn complete<'p>(
    frame: Frame<'p>,
    value: Delivered,
    stored: bool,
    frames: &mut Vec<Frame<'p>>,
    walks: &mut Vec<WalkDone>,
) -> Option<Delivered> {
    if frame.owns_walk {
        walks.pop().expect("BUG: owned walk memo missing");
    }
    let Some(parent) = frames.last_mut() else {
        return Some(value);
    };
    match &frame.kind {
        Kind::Cell(cell) if !frame.owns_walk => {
            let result = value.expect("BUG: cell finished without a value");
            if !stored {
                walks[frame.walk].insert(cell.id, result);
            }
        }
        _ => parent.inbox = value,
    }
    None
}

fn step<'p>(
    frame: &mut Frame<'p>,
    root_plan: &'p ExecutionPlan,
    root: &mut EvaluationContext,
    scopes: &mut Vec<Scope<'p>>,
    walks: &[WalkDone],
) -> Outcome<'p> {
    let inbox = frame.inbox.take();
    let walk = frame.walk;
    if let Kind::RowScope(scope) = &mut frame.kind {
        return step_row_scope(scope, inbox, frame.depth, root_plan, root, scopes);
    }
    let (plan, ctx) = layer(frame.depth, root_plan, root, scopes);
    match &mut frame.kind {
        Kind::Cell(cell) => step_cell(cell, inbox, &walks[walk], plan, ctx),
        Kind::Rule(rule) => step_rule(rule, inbox, plan, ctx),
        Kind::ReadRow(read) => step_read_row(read, inbox, plan, ctx),
        Kind::ListOp(list) => step_list_op(list, inbox, plan, ctx),
        Kind::RowScope(_) => unreachable!("BUG: row scope frame stepped outside its arm"),
    }
}

/// Rule `index` of `plan`: its normal form is evaluated once per request and stored in
/// `rule_values`.
fn step_rule<'p>(
    frame: &mut RuleFrame,
    inbox: Delivered,
    plan: &'p ExecutionPlan,
    ctx: &mut EvaluationContext,
) -> Outcome<'p> {
    let index = frame.index;
    let rule = plan.rule_at(index);
    if let Some(value) = inbox {
        if let Some(rows) = frame.saved_rows.take() {
            ctx.row_stack = rows;
        }
        ctx.rule_values[index.index()] = Some(value);
        return Outcome::Finish(None);
    }
    if ctx.rule_values[index.index()].is_some() {
        return Outcome::Finish(None);
    }
    frame.saved_rows = Some(std::mem::take(&mut ctx.row_stack));
    spawn(Frame::cell(rule.normal_form), WalkMode::New)
}

/// Data paths of the row spec the row scope reads once open: exactly the body's
/// fields. Deferred bindings at these slots resolve in [`RowPhase::Resolve`];
/// bindings elsewhere stay unresolved until a read lands there. `Materialize`
/// resolves every binding of the row.
fn row_scope_read_slots(frame: &RowScopeFrame) -> Vec<DataPath> {
    let template = frame.template;
    let mut slots: Vec<DataPath> = Vec::new();
    let mut push = |slot: DataPath| {
        if !slots.contains(&slot) {
            slots.push(slot);
        }
    };
    match &frame.body {
        RowBody::Nothing => {}
        RowBody::Materialize => {
            for (slot, binding) in frame.key.row.bindings.iter().enumerate() {
                if binding.is_some() {
                    let (path, _) = template
                        .data
                        .get_index(slot)
                        .expect("BUG: row binding slot is template data");
                    push(path.clone());
                }
            }
        }
        RowBody::Rule(index) => {
            let end = RowEnd::Rule(template.rule_at(*index).path.clone());
            for read in template.row_end_reads(&end) {
                push(read.slot);
            }
        }
        RowBody::Leaf(id) => {
            if let NormalFormKind::Leaf(LeafKind::DataPath(path)) = &template.normal_form(*id).kind
            {
                push(path.clone());
            }
        }
    }
    slots
}

/// Rule a deferred binding's target ends at: the rule itself, or the rule end of the
/// data target's reference chain.
fn deferred_target_rule<'a>(target: &'a ReferenceTarget, plan: &'a ExecutionPlan) -> &'a RulePath {
    match target {
        ReferenceTarget::Rule(rule) => rule,
        ReferenceTarget::Data(path) => match plan.reference_ends.get(path) {
            Some(ReferenceEnd::Rule(rule)) => rule,
            _ => panic!("BUG: deferred binding '{path}' does not end at a rule"),
        },
    }
}

/// What resolving a deferred target in its owner's context evaluates first.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TargetForce {
    /// The rule the target ends at.
    Rule(RuleIndex),
    /// The member cell of a target read through `uses` hops (`i.x`).
    Member(NormalFormId),
}

/// What `target` still needs evaluated in `ctx` over `plan`, its owner, before it
/// resolves; `None` when it resolves as is.
fn unforced_target(
    target: &ReferenceTarget,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
) -> Option<TargetForce> {
    if let Some(&cell) = plan.member_targets.get(target) {
        return ctx.values[cell.index()]
            .is_none()
            .then_some(TargetForce::Member(cell));
    }
    if !rule_bound_target(target, plan) {
        return None;
    }
    let index = plan.rule_index(deferred_target_rule(target, plan));
    ctx.rule_values[index.index()]
        .is_none()
        .then_some(TargetForce::Rule(index))
}

/// The frame that evaluates `force` in its owner's context.
fn force_frame<'p>(force: TargetForce) -> (Frame<'p>, WalkMode) {
    match force {
        TargetForce::Rule(index) => (Frame::rule(index), WalkMode::Same),
        TargetForce::Member(cell) => (Frame::cell(cell), WalkMode::New),
    }
}

/// Row body that evaluates `force` in the owner row's context.
fn force_body(force: TargetForce) -> RowBody {
    match force {
        TargetForce::Rule(index) => RowBody::Rule(index),
        TargetForce::Member(cell) => RowBody::Leaf(cell),
    }
}

/// Targets a deferred row binding anywhere in `values` ends at that are not
/// evaluated yet, in first-seen order.
fn unforced_deferred_in_values(
    values: &[OperationResult],
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
) -> Vec<TargetForce> {
    let rows = values
        .iter()
        .filter_map(OperationResult::value)
        .flat_map(|bound| value_rows(&bound.value))
        .collect();
    unforced_deferred_targets(rows, true, plan, ctx)
        .into_iter()
        .map(|(_, force)| force)
        .collect()
}

/// Where a deferred target of [`unforced_deferred_targets`] is evaluated.
#[derive(Clone, PartialEq, Eq)]
enum ForceAt {
    /// The context the targets were gathered in.
    Here,
    /// The cached context of this row.
    Row(InstanceKey),
}

/// Targets that deferred bindings of `rows`, and of the rows their bindings hold, end
/// at and their owner context has not evaluated, in first-seen order. With
/// `owned_only`, only bindings `ctx` owns count: another scope resolves the rest.
/// Otherwise a binding owned by a cached row context is forced there.
fn unforced_deferred_targets(
    rows: Vec<InstanceKey>,
    owned_only: bool,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
) -> Vec<(ForceAt, TargetForce)> {
    let mut pending = Vec::new();
    let mut stack = rows;
    stack.reverse();
    while let Some(key) = stack.pop() {
        let mut held = Vec::new();
        for binding in key.row.bindings.iter().flatten() {
            match binding {
                RowBinding::Deferred { target, hops } => {
                    let owner = key.scope(*hops);
                    let unforced = if owner == ctx.owner_row.as_ref() {
                        unforced_target(target, plan, ctx).map(|force| (ForceAt::Here, force))
                    } else if owned_only {
                        None
                    } else {
                        owner.and_then(|owner| {
                            let contexts = ctx.caches.row_contexts.borrow();
                            let owner_ctx = contexts.get(owner)?;
                            let owner_plan = plan.row_template(&owner.row.identity);
                            unforced_target(target, owner_plan, owner_ctx)
                                .map(|force| (ForceAt::Row(owner.clone()), force))
                        })
                    };
                    if let Some(force) = unforced {
                        if !pending.contains(&force) {
                            pending.push(force);
                        }
                    }
                }
                RowBinding::Rows { rows, hops, .. } => {
                    let owner = key.scope(*hops);
                    held.extend(
                        rows.iter()
                            .map(|nested| InstanceKey::new(Arc::clone(nested), owner.cloned())),
                    );
                }
                RowBinding::Instance(nested) => {
                    held.push(InstanceKey::new(Arc::clone(nested), Some(key.clone())));
                }
                RowBinding::Bound(OperationResult::Value(bound)) => {
                    held.extend(value_rows(&bound.value));
                }
                RowBinding::Bound(OperationResult::Veto(_)) => {}
            }
        }
        stack.extend(held.into_iter().rev());
    }
    pending
}

/// Rows `value` holds: the row it is, or the rows of a list.
fn value_rows(value: &ValueKind) -> Vec<InstanceKey> {
    fn row(value: &ValueKind) -> Option<InstanceKey> {
        match value {
            ValueKind::Spec(SpecValue::Row(key)) => Some(key.clone()),
            _ => None,
        }
    }
    match value {
        ValueKind::List(items) => match items.stored() {
            Some(stored) => stored
                .rows
                .iter()
                .map(|row| InstanceKey::new(Arc::clone(row), stored.owner.cloned()))
                .collect(),
            None => items
                .values()
                .iter()
                .filter_map(|item| row(&item.value))
                .collect(),
        },
        other => row(other).into_iter().collect(),
    }
}

/// Row context of `row`: built bottom-up over its self-list rows, then the row's
/// body (rule or data leaf) read inside it. The context returns home when the scope is left.
fn step_row_scope<'p>(
    frame: &mut RowScopeFrame<'p>,
    mut inbox: Delivered,
    depth: usize,
    root_plan: &'p ExecutionPlan,
    root: &mut EvaluationContext,
    scopes: &mut Vec<Scope<'p>>,
) -> Outcome<'p> {
    loop {
        match &mut frame.phase {
            RowPhase::Acquire => {
                frame.read_slots = row_scope_read_slots(frame);
                frame.phase = RowPhase::Resolve(0);
            }
            RowPhase::Resolve(cursor) => {
                while let Some(slot) = frame.read_slots.get(*cursor) {
                    *cursor += 1;
                    let Some(RowBinding::Deferred { target, hops }) =
                        frame.key.row.get_binding(frame.template, slot)
                    else {
                        continue;
                    };
                    let owner_key = frame.key.scope(*hops);
                    match deferred_owner(owner_key, depth, root, scopes) {
                        DeferredOwner::Layer(owner_depth) => {
                            let (oplan, octx) =
                                layer(owner_depth, root_plan, &mut *root, &mut *scopes);
                            if let Some(force) = unforced_target(target, oplan, octx) {
                                let (mut child, mode) = force_frame(force);
                                child.depth = owner_depth;
                                return spawn(child, mode);
                            }
                        }
                        DeferredOwner::Cached => {
                            let owner_key =
                                owner_key.expect("BUG: cached deferred owner without a row");
                            let oplan = root_plan.row_template(&owner_key.row.identity);
                            let force = {
                                let contexts = root.caches.row_contexts.borrow();
                                let octx = contexts.get(owner_key).expect(
                                    "BUG: deferred owner row's context missing from the cache",
                                );
                                unforced_target(target, oplan, octx)
                            };
                            if let Some(force) = force {
                                return spawn(
                                    Frame::row_scope(owner_key.clone(), oplan, force_body(force)),
                                    WalkMode::Same,
                                );
                            }
                        }
                    }
                }
                // Deferred bindings the read touches (or whose target is already
                // evaluated) resolve against their owner scope before the row's
                // context is seeded.
                let mut resolved: Vec<(DataPath, OperationResult)> = Vec::new();
                for (slot_idx, binding) in frame.key.row.bindings.iter().enumerate() {
                    let Some(RowBinding::Deferred { target, hops }) = binding else {
                        continue;
                    };
                    let (path, _) = frame
                        .template
                        .data
                        .get_index(slot_idx)
                        .expect("BUG: row binding slot is template data");
                    let owner_key = frame.key.scope(*hops);
                    match deferred_owner(owner_key, depth, root, scopes) {
                        DeferredOwner::Layer(owner_depth) => {
                            let (oplan, octx) =
                                layer(owner_depth, root_plan, &mut *root, &mut *scopes);
                            if frame.read_slots.contains(path)
                                || unforced_target(target, oplan, octx).is_none()
                            {
                                resolved.push((
                                    path.clone(),
                                    rows::checked_deferred_field(
                                        &frame.key,
                                        frame.template,
                                        path,
                                        rows::owner_result(
                                            resolve_stored_target(target, oplan, octx),
                                            owner_key,
                                            oplan,
                                        ),
                                    ),
                                ));
                            }
                        }
                        DeferredOwner::Cached => {
                            let owner_key =
                                owner_key.expect("BUG: cached deferred owner without a row");
                            let oplan = root_plan.row_template(&owner_key.row.identity);
                            let contexts = root.caches.row_contexts.borrow();
                            let octx = contexts
                                .get(owner_key)
                                .expect("BUG: deferred owner row's context missing from the cache");
                            if frame.read_slots.contains(path)
                                || unforced_target(target, oplan, octx).is_none()
                            {
                                resolved.push((
                                    path.clone(),
                                    rows::checked_deferred_field(
                                        &frame.key,
                                        frame.template,
                                        path,
                                        rows::owner_result(
                                            resolve_stored_target(target, oplan, octx),
                                            Some(owner_key),
                                            oplan,
                                        ),
                                    ),
                                ));
                            }
                        }
                    }
                }
                let template = frame.template;
                let (_, ctx) = layer(depth, root_plan, &mut *root, &mut *scopes);
                if let Some(mut cached) =
                    ctx.caches.row_contexts.borrow_mut().swap_remove(&frame.key)
                {
                    for (path, value) in resolved {
                        cached.bind_data_slot(template, &path, value);
                    }
                    cached.owner_row = Some(frame.key.clone());
                    frame.phase = RowPhase::Body;
                    return Outcome::Enter(Scope {
                        plan: template,
                        ctx: cached,
                    });
                }
                let bindings: Vec<(usize, OperationResult)> = (0..frame.key.row.bindings.len())
                    .filter_map(|slot_idx| {
                        let binding = ctx.caches.row_binding(&frame.key, slot_idx)?;
                        let value = match binding {
                            RowBinding::Bound(value) => value.clone(),
                            RowBinding::Rows {
                                rows,
                                row_type,
                                hops,
                            } => RowBinding::rows_value(rows, row_type, frame.key.scope(*hops)),
                            RowBinding::Instance(_) => return None,
                            RowBinding::Deferred { .. } => {
                                let (path, _) = template
                                    .data
                                    .get_index(slot_idx)
                                    .expect("BUG: row binding slot is template data");
                                resolved
                                    .iter()
                                    .find(|(slot, _)| slot == path)
                                    .map(|(_, value)| value.clone())?
                            }
                        };
                        Some((slot_idx, value))
                    })
                    .collect();
                let children = rows::self_rows(&frame.key, template, &frame.read_slots, &bindings);
                let mut fresh = ctx.child_row(template, &bindings);
                fresh.owner_row = Some(frame.key.clone());
                frame.phase = if children.is_empty() {
                    RowPhase::Body
                } else {
                    RowPhase::Children(children)
                };
                return Outcome::Enter(Scope {
                    plan: template,
                    ctx: fresh,
                });
            }
            RowPhase::Children(children) => {
                let Some(child) = children.pop() else {
                    frame.phase = RowPhase::Body;
                    continue;
                };
                let (_, ctx) = layer(depth, root_plan, &mut *root, &mut *scopes);
                if ctx.caches.row_contexts.borrow().contains_key(&child) {
                    continue;
                }
                return spawn(
                    Frame::row_scope(child, frame.template, RowBody::Nothing),
                    WalkMode::Same,
                );
            }
            RowPhase::Body => {
                let (_, ctx) = layer(depth, root_plan, &mut *root, &mut *scopes);
                match &frame.body {
                    RowBody::Nothing | RowBody::Materialize => {
                        return Outcome::Exit(None, frame.key.clone());
                    }
                    RowBody::Rule(index) => match &ctx.rule_values[index.index()] {
                        Some(value) => {
                            return Outcome::Exit(Some(value.clone()), frame.key.clone());
                        }
                        None => return spawn(Frame::rule(*index), WalkMode::Same),
                    },
                    RowBody::Leaf(id) => {
                        frame.phase = RowPhase::Await;
                        return spawn(Frame::cell(*id), WalkMode::New);
                    }
                }
            }
            RowPhase::Await => {
                let result = inbox
                    .take()
                    .expect("BUG: row scope resumed without its field value");
                return Outcome::Exit(Some(result), frame.key.clone());
            }
        }
    }
}

/// Body evaluating data `path` of a row's template, and the rule root when the path
/// ends at a rule (for missing-data keys).
fn data_row_body(template: &ExecutionPlan, path: &DataPath) -> (RowBody, Option<NormalFormId>) {
    match template.reference_ends.get(path) {
        Some(ReferenceEnd::Rule(rule)) => {
            let executable = template
                .rules
                .get(rule)
                .expect("BUG: row reference rule missing from its template");
            (
                RowBody::Rule(template.rule_index(rule)),
                Some(executable.normal_form),
            )
        }
        Some(ReferenceEnd::Promptable(_)) | None => (
            RowBody::Leaf(
                *template
                    .data_leaf
                    .get(path)
                    .expect("BUG: row data end has no data_leaf entry in its template"),
            ),
            None,
        ),
    }
}

/// Field `end` of a row, read in the row's own context (see [`step_row_scope`]). The
/// missing row fields of the context are logged in the consumer and the read is logged
/// for narration.
fn step_read_row<'p>(
    frame: &mut ReadRowFrame,
    inbox: Delivered,
    plan: &'p ExecutionPlan,
    ctx: &mut EvaluationContext,
) -> Outcome<'p> {
    let template = plan.row_template(&frame.key.row.identity);
    let Some(raw) = inbox else {
        let (body, rule_root) = match &frame.end {
            RowEnd::Rule(rule) => {
                let executable = template
                    .rules
                    .get(rule)
                    .expect("BUG: row rule end missing from its template");
                (
                    RowBody::Rule(template.rule_index(rule)),
                    Some(executable.normal_form),
                )
            }
            RowEnd::Data(path) => {
                let slot = template
                    .data
                    .get_index_of(path)
                    .expect("BUG: row data end is template data");
                let bound = match ctx.caches.row_binding(&frame.key, slot) {
                    Some(RowBinding::Bound(value)) => Some(value.clone()),
                    Some(RowBinding::Rows {
                        rows,
                        row_type,
                        hops,
                    }) => Some(RowBinding::rows_value(
                        rows,
                        row_type,
                        frame.key.scope(*hops),
                    )),
                    Some(RowBinding::Instance(nested)) => Some(OperationResult::from_literal(
                        LiteralValue::new(ValueKind::Spec(SpecValue::Row(InstanceKey::new(
                            Arc::clone(nested),
                            Some(frame.key.clone()),
                        )))),
                    )),
                    Some(RowBinding::Deferred { .. }) | None => None,
                };
                if let Some(value) = bound {
                    let needs_options = template
                        .data
                        .get(path)
                        .and_then(|definition| definition.schema_type())
                        .is_some_and(|schema| schema.needs_runtime_option_check());
                    if !needs_options {
                        let result = rows::from_row(value, &frame.key, template, ctx);
                        if ctx.exhaustive {
                            ctx.row_reads_log.push(RowRead {
                                row: frame.read.clone(),
                                end: frame.end.clone(),
                                result: result.clone(),
                                result_type: row_end_type(template, &frame.end),
                            });
                        }
                        return Outcome::Finish(Some(result));
                    }
                }
                data_row_body(template, path)
            }
        };
        frame.rule_root = rule_root;
        return spawn(
            Frame::row_scope(frame.key.clone(), template, body),
            WalkMode::Same,
        );
    };
    if let Some(root) = frame.rule_root {
        if raw.is_missing_data() {
            let keys = ctx
                .caches
                .row_contexts
                .borrow()
                .get(&frame.key)
                .expect("BUG: row context missing after its read")
                .missing_data_for_rule(template, root, Some(&frame.key));
            ctx.missing_row_keys.extend(keys);
        }
    }
    let result = rows::from_row(raw, &frame.key, template, ctx);
    if ctx.exhaustive {
        ctx.row_reads_log.push(RowRead {
            row: frame.read.clone(),
            end: frame.end.clone(),
            result: result.clone(),
            result_type: row_end_type(template, &frame.end),
        });
    }
    Outcome::Finish(Some(result))
}

/// The stored list at `path`, its rows owned by `ctx.owner_row`. The store's index
/// serves the list unless run data set a key field.
fn stored_list(
    path: &DataPath,
    source: &InstanceListSource,
    ctx: &EvaluationContext,
) -> OperationResult {
    let mut list = source.list(ctx.owner_row.clone());
    if ctx.caches.key_overridden(&ctx.owner_row, path) {
        list = list.unindexed();
    }
    OperationResult::from_literal(LiteralValue {
        value: ValueKind::List(list),
    })
}

/// The target forces a rule when resolved: a rule directly, or data whose reference
/// chain ends at a rule.
fn rule_bound_target(target: &ReferenceTarget, plan: &ExecutionPlan) -> bool {
    match target {
        ReferenceTarget::Rule(_) => true,
        ReferenceTarget::Data(path) => {
            matches!(plan.reference_ends.get(path), Some(ReferenceEnd::Rule(_)))
        }
    }
}

/// Value of a stored reference target in this context. Rules are already evaluated;
/// data targets read their slot, materialize a referenced list's rows, or veto as
/// missing exactly as a data-path cell would.
pub(crate) fn resolve_stored_target(
    target: &ReferenceTarget,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
) -> OperationResult {
    if let Some(&cell) = plan.member_targets.get(target) {
        return ctx.values[cell.index()]
            .clone()
            .expect("BUG: member target resolved before its cell was evaluated");
    }
    match target {
        ReferenceTarget::Rule(rule) => ctx.rule_value(plan, rule).clone(),
        ReferenceTarget::Data(path) => resolve_stored_data_target(path, plan, ctx),
    }
}

fn resolve_stored_data_target(
    path: &DataPath,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
) -> OperationResult {
    if let Some(slot) = ctx.data_slot(plan, path) {
        return slot.clone();
    }
    match plan.reference_ends.get(path) {
        Some(ReferenceEnd::Rule(_)) => resolve_data_path_value(path, plan, ctx),
        Some(ReferenceEnd::Promptable(end)) => {
            if let Some(slot) = ctx.data_slot(plan, end) {
                return slot.clone();
            }
            if let Some(nested_source) = plan.instance_lists.get(end) {
                return stored_list(end, nested_source, ctx);
            }
            resolve_data_path_value(path, plan, ctx)
        }
        None => {
            if let Some(nested_source) = plan.instance_lists.get(path) {
                return stored_list(path, nested_source, ctx);
            }
            resolve_data_path_value(path, plan, ctx)
        }
    }
}

/// A field an inline row (`-> with field: value`) writes: the value the command's own
/// plan computed, read as that plan planned it and never through the row context, so
/// instances of this plan inside it stay what the plan knows. `None` on every other
/// row or field, which read through the row's context.
fn inline_row_binding(
    key: &InstanceKey,
    position: Option<usize>,
    end: &RowEnd,
    plan: &ExecutionPlan,
    ctx: &mut EvaluationContext,
) -> Option<OperationResult> {
    let row = &key.row;
    if !row.origin.is_inline() {
        return None;
    }
    let RowEnd::Data(path) = end else {
        return None;
    };
    let template = plan.row_template(&row.identity);
    let value = match row.get_binding(template, path)? {
        RowBinding::Bound(value) => value,
        RowBinding::Deferred { .. } | RowBinding::Rows { .. } | RowBinding::Instance(_) => {
            unreachable!("BUG: an inline `-> with` row binds command values")
        }
    };
    if ctx.exhaustive {
        ctx.row_reads_log.push(RowRead {
            row: rows::ReadRow {
                key: key.clone(),
                position,
            },
            end: end.clone(),
            result: value.clone(),
            result_type: row_end_type(template, end),
        });
    }
    Some(value.clone())
}

/// The row a per-row operand evaluates against: row `position` (0-based) of the list.
fn current_instance_row(row: &TypedLiteral, position: usize) -> CurrentRow {
    match &row.value {
        ValueKind::Spec(SpecValue::Row(key)) => CurrentRow::Instance {
            key: key.clone(),
            position,
        },
        other => panic!("BUG: instance list row is {other:?}, not a row"),
    }
}

/// The row whose key field `key_field` a key scan reads, the field read on it, and
/// the field's type.
fn key_field_read(
    plan: &ExecutionPlan,
    row: &TypedLiteral,
    key_field: &KeyField,
) -> (InstanceKey, RowEnd, Arc<LemmaType>) {
    match &row.value {
        ValueKind::Spec(SpecValue::Row(key)) => {
            let (end, field_type) = list_ops::key_field_type(plan, &key.row, key_field);
            (key.clone(), end, field_type)
        }
        other => panic!("BUG: instance list row is {other:?}, not a row"),
    }
}

/// Instance-list command (`list` operand plus its operands), evaluated in the frame's scope.
fn step_list_op<'p>(
    frame: &mut ListOpFrame<'p>,
    mut inbox: Delivered,
    plan: &'p ExecutionPlan,
    ctx: &mut EvaluationContext,
) -> Outcome<'p> {
    let op = frame.op;
    let result_type = frame.result_type;
    loop {
        match std::mem::replace(&mut frame.state, ListState::Start) {
            ListState::Start => {
                frame.state = ListState::AwaitList;
                return spawn(Frame::cell(frame.list), WalkMode::New);
            }
            ListState::AwaitList => {
                let list_value = inbox
                    .take()
                    .expect("BUG: list command resumed without its list value");
                let bound = match list_value {
                    OperationResult::Value(bound) => bound,
                    veto => return Outcome::Finish(Some(veto)),
                };
                let ValueKind::List(rows) = &bound.value else {
                    panic!(
                        "BUG: list command operand is not a list (type {})",
                        plan.normal_form(frame.list).result_type.name()
                    );
                };
                let data = ListData {
                    rows: rows.clone(),
                    unit: bound.unit.clone(),
                };
                let key_fields = match frame.row_level {
                    Some(_) => match list_ops::list_key(plan.result_type(frame.list)) {
                        ListKey::Fields(fields) => Some(fields.clone()),
                        ListKey::Numbered => None,
                    },
                    None => None,
                };
                match (frame.row_level, op) {
                    (_, NormalFormListOp::Count) => {
                        return Outcome::Finish(Some(list_ops::count_rows(&data.rows)));
                    }
                    (_, NormalFormListOp::First) => {
                        return Outcome::Finish(Some(list_ops::first_or_last(&data.rows, true)));
                    }
                    (_, NormalFormListOp::Last) => {
                        return Outcome::Finish(Some(list_ops::first_or_last(&data.rows, false)));
                    }
                    (None, NormalFormListOp::Sum) => {
                        return Outcome::Finish(Some(list_ops::reduce(
                            data.rows.values(),
                            ListFold::Sum,
                            result_type,
                            plan,
                        )));
                    }
                    (None, NormalFormListOp::Product) => {
                        return Outcome::Finish(Some(list_ops::reduce(
                            data.rows.values(),
                            ListFold::Product,
                            result_type,
                            plan,
                        )));
                    }
                    (None, NormalFormListOp::Avg) => {
                        return Outcome::Finish(Some(list_ops::reduce(
                            data.rows.values(),
                            ListFold::Avg,
                            result_type,
                            plan,
                        )));
                    }
                    (None, NormalFormListOp::Min) => {
                        return Outcome::Finish(Some(list_ops::extrema(
                            data.rows.values(),
                            Ordering::Less,
                        )));
                    }
                    (None, NormalFormListOp::Max) => {
                        return Outcome::Finish(Some(list_ops::extrema(
                            data.rows.values(),
                            Ordering::Greater,
                        )));
                    }
                    (None, NormalFormListOp::Sort { descending }) => {
                        return Outcome::Finish(Some(list_ops::sort_rows(
                            data.rows.values(),
                            *descending,
                            data.unit.clone(),
                        )));
                    }
                    (Some(_), NormalFormListOp::Key { .. }) => {
                        let ListKey::Fields(fields) =
                            list_ops::list_key(plan.result_type(frame.id))
                        else {
                            unreachable!("BUG: `-> key` result type is keyed by fields");
                        };
                        frame.state = ListState::Scan(KeyScan {
                            rows: data.rows,
                            fields: fields.clone(),
                            row: 0,
                            field: 0,
                            key: Vec::new(),
                            keyed: Vec::new(),
                            then: ScanThen::List(data.unit),
                        });
                    }
                    (None, NormalFormListOp::Key { .. }) => {
                        unreachable!("BUG: planning rejects `-> key` on value lists")
                    }
                    (None, NormalFormListOp::With) | (Some(_), NormalFormListOp::With) => {
                        let count = frame.args.len();
                        frame.state = ListState::Args {
                            data,
                            values: Vec::new(),
                            count,
                            key_fields,
                        };
                    }
                    (None, NormalFormListOp::Find)
                    | (None, NormalFormListOp::Top)
                    | (Some(_), NormalFormListOp::Top) => {
                        frame.state = ListState::Args {
                            data,
                            values: Vec::new(),
                            count: 1,
                            key_fields: None,
                        };
                    }
                    (Some(_), NormalFormListOp::Find) => {
                        let count = if key_fields.is_some() {
                            frame.args.len()
                        } else {
                            1
                        };
                        frame.state = ListState::Args {
                            data,
                            values: Vec::new(),
                            count,
                            key_fields,
                        };
                    }
                    (Some(_), NormalFormListOp::WithRow { .. }) => {
                        let count = frame.args.len();
                        frame.state = ListState::Args {
                            data,
                            values: Vec::new(),
                            count,
                            key_fields,
                        };
                    }
                    (Some(_), NormalFormListOp::Sum)
                    | (Some(_), NormalFormListOp::Product)
                    | (Some(_), NormalFormListOp::Avg)
                    | (Some(_), NormalFormListOp::Min)
                    | (Some(_), NormalFormListOp::Max)
                    | (Some(_), NormalFormListOp::Sort { .. })
                    | (Some(_), NormalFormListOp::Predicate { .. }) => {
                        frame.state = ListState::PerRow {
                            data,
                            cursor: 0,
                            values: Vec::new(),
                            first_veto: None,
                        };
                    }
                    (None, NormalFormListOp::Predicate { .. })
                    | (None, NormalFormListOp::WithRow { .. }) => {
                        unreachable!("BUG: row command {op:?} has no row level")
                    }
                }
            }
            ListState::Args {
                data,
                mut values,
                count,
                key_fields,
            } => {
                if let Some(value) = inbox.take() {
                    if !matches!(value, OperationResult::Value(_)) {
                        return Outcome::Finish(Some(value));
                    }
                    values.push(value);
                }
                if values.len() < count {
                    let arg = frame.args[values.len()];
                    frame.state = ListState::Args {
                        data,
                        values,
                        count,
                        key_fields,
                    };
                    return spawn(Frame::cell(arg), WalkMode::New);
                }
                match (frame.row_level, op) {
                    (_, NormalFormListOp::Top) => {
                        return Outcome::Finish(Some(list_ops::top_rows(
                            &data.rows,
                            first_operand(&values),
                            data.unit.clone(),
                        )));
                    }
                    (None, NormalFormListOp::Find) => {
                        return Outcome::Finish(Some(list_ops::find_numbered(
                            &data.rows,
                            first_operand(&values),
                        )));
                    }
                    (Some(_), NormalFormListOp::Find) => match key_fields {
                        None => {
                            return Outcome::Finish(Some(list_ops::find_numbered(
                                &data.rows,
                                first_operand(&values),
                            )));
                        }
                        Some(fields) => {
                            let wanted: Vec<TypedLiteral> = values
                                .into_iter()
                                .zip(frame.args)
                                .map(|(value, arg)| {
                                    list_ops::typed_operand(value, plan.result_type(*arg))
                                })
                                .collect();
                            if let Some(index) = data.rows.index().filter(|index| {
                                index
                                    .fields
                                    .iter()
                                    .eq(fields.iter().map(|field| &field.name))
                            }) {
                                return Outcome::Finish(Some(find_keyed(
                                    &data.rows, index, &fields, &wanted,
                                )));
                            }
                            frame.state = ListState::Scan(KeyScan {
                                rows: data.rows,
                                fields,
                                row: 0,
                                field: 0,
                                key: Vec::new(),
                                keyed: Vec::new(),
                                then: ScanThen::Find(wanted),
                            });
                        }
                    },
                    (None, NormalFormListOp::With) => {
                        return Outcome::Finish(Some(list_ops::append_values(
                            data.rows.values(),
                            values,
                            frame.args,
                            plan,
                            data.unit.clone(),
                        )));
                    }
                    (Some(_), NormalFormListOp::With) => {
                        let appended = list_ops::append_instance_operands(
                            &data.rows,
                            values,
                            plan.result_type(frame.list),
                            data.unit.clone(),
                        );
                        match key_fields {
                            None => return Outcome::Finish(Some(appended)),
                            Some(fields) => {
                                let OperationResult::Value(bound) = appended else {
                                    return Outcome::Finish(Some(appended));
                                };
                                let ValueKind::List(rows) = bound.value else {
                                    unreachable!("BUG: append_instance_operands returned non-list");
                                };
                                frame.state = ListState::Scan(KeyScan {
                                    rows,
                                    fields,
                                    row: 0,
                                    field: 0,
                                    key: Vec::new(),
                                    keyed: Vec::new(),
                                    then: ScanThen::List(bound.unit),
                                });
                            }
                        }
                    }
                    (
                        Some(_),
                        NormalFormListOp::WithRow {
                            identity,
                            fields: append_fields,
                        },
                    ) => {
                        let pending = unforced_deferred_in_values(&values, plan, ctx);
                        if !pending.is_empty() {
                            frame.state = ListState::InlineDeferred {
                                data,
                                values,
                                count,
                                key_fields,
                                pending: pending.into_iter(),
                            };
                            continue;
                        }
                        let inline = list_ops::inline_row(
                            plan,
                            identity,
                            append_fields,
                            values,
                            RowOrigin::Inline {
                                node: frame.id,
                                position: data.rows.len() + 1,
                            },
                        );
                        let mut rows = data.rows.to_vec();
                        rows.push(inline);
                        match key_fields {
                            None => {
                                return Outcome::Finish(Some(list_ops::list_result(
                                    rows, data.unit,
                                )));
                            }
                            Some(fields) => {
                                frame.state = ListState::Scan(KeyScan {
                                    rows: rows.into(),
                                    fields,
                                    row: 0,
                                    field: 0,
                                    key: Vec::new(),
                                    keyed: Vec::new(),
                                    then: ScanThen::List(data.unit),
                                });
                            }
                        }
                    }
                    (_, other) => unreachable!("BUG: {other:?} does not take operands here"),
                }
            }
            ListState::InlineDeferred {
                data,
                values,
                count,
                key_fields,
                mut pending,
            } => {
                let Some(force) = pending.next() else {
                    frame.state = ListState::Args {
                        data,
                        values,
                        count,
                        key_fields,
                    };
                    continue;
                };
                frame.state = ListState::InlineDeferred {
                    data,
                    values,
                    count,
                    key_fields,
                    pending,
                };
                let (child, mode) = force_frame(force);
                return spawn(child, mode);
            }
            ListState::PerRow {
                data,
                mut cursor,
                mut values,
                mut first_veto,
            } => {
                let level = usize::try_from(
                    frame
                        .row_level
                        .expect("BUG: per-row command without a row level"),
                )
                .expect("BUG: row level fits usize");
                let argument = *frame
                    .args
                    .first()
                    .expect("BUG: row reduction must have a row argument");
                if let Some(value) = inbox.take() {
                    ctx.row_stack.truncate(level);
                    match value {
                        OperationResult::Value(bound) => {
                            values.push(typed_from_bound(&bound, plan.result_type(argument)));
                        }
                        veto => {
                            first_veto.get_or_insert(veto);
                        }
                    }
                    cursor += 1;
                }
                if cursor < data.rows.len() {
                    assert_eq!(
                        ctx.row_stack.len(),
                        level,
                        "BUG: list command entered at the wrong row depth"
                    );
                    let current = match op {
                        NormalFormListOp::Predicate {
                            form: crate::parsing::ast::ListPredicateForm::Value,
                            ..
                        } => CurrentRow::Value(
                            data.rows
                                .get(cursor)
                                .expect("BUG: cursor below the list length")
                                .into_owned(),
                        ),
                        NormalFormListOp::Predicate {
                            form: crate::parsing::ast::ListPredicateForm::Instance,
                            ..
                        }
                        | NormalFormListOp::Sum
                        | NormalFormListOp::Product
                        | NormalFormListOp::Avg
                        | NormalFormListOp::Min
                        | NormalFormListOp::Max
                        | NormalFormListOp::Sort { .. } => current_instance_row(
                            &data
                                .rows
                                .get(cursor)
                                .expect("BUG: cursor below the list length"),
                            cursor,
                        ),
                        other => unreachable!("BUG: {other:?} is not a per-row operation"),
                    };
                    ctx.row_stack.push(current);
                    frame.state = ListState::PerRow {
                        data,
                        cursor,
                        values,
                        first_veto,
                    };
                    return spawn(Frame::cell(argument), WalkMode::New);
                }
                return Outcome::Finish(Some(match first_veto {
                    Some(veto) => veto,
                    None => list_ops::per_row_result(
                        op,
                        &data.rows,
                        &values,
                        data.unit.clone(),
                        result_type,
                        plan,
                    ),
                }));
            }
            ListState::Scan(mut scan) => {
                if let Some(value) = inbox.take() {
                    match value {
                        OperationResult::Value(bound) => {
                            let row = scan
                                .rows
                                .get(scan.row)
                                .expect("BUG: scanned row below the list length");
                            let (_, _, field_type) =
                                key_field_read(plan, &row, &scan.fields[scan.field]);
                            scan.key.push(typed_from_bound(&bound, &field_type));
                        }
                        veto => return Outcome::Finish(Some(veto)),
                    }
                    scan.field += 1;
                }
                loop {
                    if scan.row == scan.rows.len() {
                        let names: Vec<String> =
                            scan.fields.iter().map(|field| field.name.clone()).collect();
                        let index = match KeyIndex::build_numbered(names, &scan.keyed) {
                            Ok(index) => Arc::new(index),
                            Err(conflict) => {
                                return Outcome::Finish(Some(OperationResult::Veto(
                                    VetoType::computation(conflict),
                                )));
                            }
                        };
                        return Outcome::Finish(Some(match scan.then {
                            ScanThen::List(unit) => {
                                list_ops::list_result(scan.rows.with_index(index), unit)
                            }
                            ScanThen::Find(wanted) => {
                                find_keyed(&scan.rows, &index, &scan.fields, &wanted)
                            }
                        }));
                    }
                    if scan.field == scan.fields.len() {
                        let key = std::mem::take(&mut scan.key);
                        scan.keyed.push(key);
                        scan.row += 1;
                        scan.field = 0;
                        continue;
                    }
                    let (row, end, _) = key_field_read(
                        plan,
                        &scan
                            .rows
                            .get(scan.row)
                            .expect("BUG: scanned row below the list length"),
                        &scan.fields[scan.field],
                    );
                    let read = rows::ReadRow::listed(row.clone(), scan.row);
                    frame.state = ListState::Scan(scan);
                    return spawn(Frame::read_row(row, end, read.position), WalkMode::Same);
                }
            }
        }
    }
}

/// The row of `rows` whose key values are `wanted`, through `index`.
fn find_keyed(
    rows: &ListValue,
    index: &KeyIndex,
    fields: &[KeyField],
    wanted: &[TypedLiteral],
) -> OperationResult {
    match index.probe(wanted) {
        Ok(Some(position)) => OperationResult::from_literal(
            rows.get(position)
                .expect("BUG: index position is a row of the list")
                .to_literal(),
        ),
        Ok(None) => list_ops::no_row_with_key(fields, wanted),
        Err(veto) => veto,
    }
}

fn first_operand(values: &[OperationResult]) -> OperationResult {
    values
        .first()
        .cloned()
        .expect("BUG: command operand collected before use")
}

fn options_rule_blocker(
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
    path: &DataPath,
) -> Option<OperationResult> {
    let rule_path = plan.options_rule_of(path)?;
    let index = plan.rule_index(&rule_path);
    let result = ctx.rule_values.get(index.index())?.as_ref()?;
    match result.value().map(|bound| &bound.value) {
        Some(crate::planning::semantics::ValueKind::List(_)) => None,
        _ => Some(result.clone()),
    }
}

fn check_data_options(
    id: NormalFormId,
    slot: OperationResult,
    plan: &ExecutionPlan,
    ctx: &mut EvaluationContext,
) -> OperationResult {
    if slot.vetoed() || slot.is_missing_data() {
        return slot;
    }
    let path = match &plan.normal_form(id).kind {
        NormalFormKind::Leaf(LeafKind::DataPath(path)) => path,
        other => panic!("BUG: options check on {other:?}"),
    };
    let lemma_type = plan
        .data
        .get(path)
        .and_then(|definition| definition.schema_type())
        .expect("BUG: options check on data without a type");
    let rule_index = plan
        .options_rule_of(path)
        .map(|rule_path| plan.rule_index(&rule_path));
    let rule_result = rule_index.map(|index| {
        ctx.rule_values[index.index()]
            .as_ref()
            .expect("BUG: options rule checked before it was evaluated")
    });
    if let Some(rule_result) = rule_result {
        if rule_result.vetoed() || rule_result.is_missing_data() {
            return rule_result.clone();
        }
    }
    let Some(slot_bound) = slot.value() else {
        return slot;
    };
    let rule_rows = match rule_result {
        Some(rule_result) => match rule_result.value().map(|bound| &bound.value) {
            Some(ValueKind::List(rows)) => Some(rows),
            _ => {
                return OperationResult::Veto(VetoType::computation(
                    "options rule did not return a list".to_string(),
                ));
            }
        },
        None => None,
    };
    match lemma_type.explain_disallowed(&slot_bound.value, rule_rows) {
        None => slot,
        Some(message) => OperationResult::Veto(VetoType::computation(message)),
    }
}

/// Evaluate a cell of the walk. Returns the step outcome: a value, or a child to wait for.
fn step_cell<'p>(
    cell: &mut CellFrame,
    mut inbox: Delivered,
    walk: &WalkDone,
    plan: &'p ExecutionPlan,
    ctx: &mut EvaluationContext,
) -> Outcome<'p> {
    let id = cell.id;
    if let Some(resume) = cell.resume.take() {
        let value = inbox.take().expect("BUG: resumed cell has no child value");
        match resume {
            Resume::Plain => return cell_done(cell, walk, value, plan, ctx),
            Resume::RowLog(mark) => {
                ctx.record_row_log(id, mark);
                return cell_done(cell, walk, value, plan, ctx);
            }
            Resume::Member(mark) => {
                let NormalFormKind::SpecMember { row, .. } = &plan.normal_form(id).kind else {
                    unreachable!("BUG: a member hop resumed a cell that is not a member read");
                };
                if cell.cursor + 1 == row.ends.len() || value.vetoed() {
                    ctx.record_row_log(id, mark);
                    return cell_done(cell, walk, value, plan, ctx);
                }
                cell.cursor += 1;
                cell.acc = Some(value);
                cell.member_mark = Some(mark);
            }
        }
    }
    if let Some(settled) = cell.settled.take() {
        return Outcome::Finish(Some(finish(id, settled, plan, ctx)));
    }

    macro_rules! need {
        ($child:expr) => {{
            let child: NormalFormId = $child;
            match ready(child, ctx, walk) {
                Some(value) => value,
                None => return spawn(Frame::cell(child), WalkMode::Same),
            }
        }};
    }
    macro_rules! settle {
        ($outcome:expr) => {
            match $outcome {
                Ok(value) => return cell_done(cell, walk, value, plan, ctx),
                Err(child) => return spawn(Frame::cell(child), WalkMode::Same),
            }
        };
    }
    macro_rules! done {
        ($value:expr) => {
            return cell_done(cell, walk, $value, plan, ctx)
        };
    }

    let cell_def = plan.normal_form(id);
    if let Some(index) = plan.rule_ref_indexes[id.index()] {
        return match &ctx.rule_values[index.index()] {
            None => spawn(Frame::rule(index), WalkMode::Same),
            Some(result) => {
                let result = result.clone();
                ctx.values[id.index()] = Some(result.clone());
                Outcome::Finish(Some(result))
            }
        };
    }
    match &cell_def.kind {
        NormalFormKind::Leaf(LeafKind::Literal(literal)) => {
            let unit = plan.result_type(id).unit().map(Arc::<str>::from);
            done!(OperationResult::from_bound(BoundValueKind::with_binding(
                literal.value.clone(),
                unit,
            )))
        }
        NormalFormKind::Leaf(LeafKind::DataPath(path)) => {
            if let Some(DataDefinition::Import { .. }) = plan.data.get(path) {
                let holder = ctx.owner_row.as_ref().map(|key| key.row.as_ref());
                let row = Arc::clone(plan.import_row(holder, path));
                done!(OperationResult::from_literal(LiteralValue::new(
                    ValueKind::Spec(SpecValue::Row(InstanceKey::new(row, ctx.owner_row.clone())))
                )))
            }
            let needs_options = plan
                .data
                .get(path)
                .and_then(|definition| definition.schema_type())
                .is_some_and(|schema| schema.needs_runtime_option_check());
            if needs_options {
                if let Some(rule_path) = plan.options_rule_of(path) {
                    let index = plan.rule_index(&rule_path);
                    if ctx.rule_values[index.index()].is_none() {
                        return spawn(Frame::rule(index), WalkMode::Same);
                    }
                }
            }
            if let Some(slot) = ctx.data_slot(plan, path).cloned() {
                if needs_options {
                    let checked = check_data_options(id, slot, plan, ctx);
                    ctx.option_check_finished(id.index());
                    done!(checked)
                }
                done!(slot)
            }
            let list_path = match plan.reference_ends.get(path) {
                Some(ReferenceEnd::Promptable(target)) => target,
                Some(ReferenceEnd::Rule(rule)) => {
                    let index = plan.rule_index(rule);
                    if ctx.rule_values[index.index()].is_none() {
                        return spawn(Frame::rule(index), WalkMode::Same);
                    }
                    let value = resolve_data_path_value(path, plan, ctx);
                    ctx.values[id.index()] = Some(value.clone());
                    return Outcome::Finish(Some(value));
                }
                None => path,
            };
            if let Some(blocked) = options_rule_blocker(plan, ctx, path) {
                done!(blocked);
            }
            if let Some(source) = plan.instance_lists.get(list_path) {
                if list_path == path {
                    done!(stored_list(path, source, ctx))
                }
                let leaf = *plan
                    .data_leaf
                    .get(list_path)
                    .expect("BUG: instance list has no data_leaf entry");
                cell.resume = Some(Resume::Plain);
                return spawn(Frame::cell(leaf), WalkMode::New);
            }
            done!(resolve_data_path_value(path, plan, ctx))
        }
        NormalFormKind::RowField { level, end, .. } => {
            let level = usize::try_from(*level).expect("BUG: row level fits usize");
            let (key, position) = match ctx
                .row_stack
                .get(level)
                .expect("BUG: row field evaluated outside its list command")
            {
                CurrentRow::Instance { key, position } => (key.clone(), *position),
                CurrentRow::Value(_) => {
                    panic!("BUG: row field evaluated against a value-list row")
                }
            };
            let read = rows::ReadRow::listed(key, position);
            if let Some(written) = inline_row_binding(&read.key, read.position, end, plan, ctx) {
                done!(written)
            }
            cell.resume = Some(Resume::Plain);
            spawn(
                Frame::read_row(read.key, end.clone(), read.position),
                WalkMode::Same,
            )
        }
        NormalFormKind::Leaf(LeafKind::RowValue { level }) => {
            let level = usize::try_from(*level).expect("BUG: row level fits usize");
            let row = match ctx
                .row_stack
                .get(level)
                .expect("BUG: row value evaluated outside its list command")
            {
                CurrentRow::Value(row) => row,
                CurrentRow::Instance { .. } => {
                    panic!("BUG: RowValue evaluated against an instance-list row")
                }
            };
            done!(OperationResult::from_literal(row.to_literal()))
        }
        NormalFormKind::Now => done!(OperationResult::from_literal(ctx.now().clone())),
        NormalFormKind::SpecMember { base, row, .. } => {
            let instance = match cell.acc.take() {
                Some(instance) => instance,
                None => {
                    let base_result = need!(*base);
                    if base_result.vetoed() {
                        done!(base_result)
                    }
                    base_result
                }
            };
            let key = match &borrow_value(&instance, "spec member").value {
                ValueKind::Spec(SpecValue::Row(key)) => key.clone(),
                other => panic!("BUG: spec member base was {other}, not a spec value"),
            };
            let end = row
                .ends
                .get(cell.cursor)
                .expect("BUG: member read past its last hop");
            let mark = match cell.member_mark.take() {
                Some(mark) => mark,
                None => ctx.row_log_mark(),
            };
            if cell.cursor + 1 == row.ends.len() {
                if let Some(written) = inline_row_binding(&key, None, end, plan, ctx) {
                    ctx.record_row_log(id, mark);
                    done!(written)
                }
            }
            cell.resume = Some(Resume::Member(mark));
            spawn(Frame::read_row(key, end.clone(), None), WalkMode::Same)
        }
        NormalFormKind::ListCommand {
            list,
            op,
            args,
            row_level,
        } => {
            if row_level.is_some() {
                // Narrated operands log their own row reads; keep them out of this cell's span.
                if let OperationResult::Value(_) = need!(*list) {
                    let scoped: Vec<NormalFormId> = args
                        .iter()
                        .copied()
                        .filter(|arg| plan.row_mask[arg.index()] == 0)
                        .collect();
                    while cell.cursor < scoped.len() {
                        need!(scoped[cell.cursor]);
                        cell.cursor += 1;
                    }
                }
            }
            cell.resume = Some(match row_level {
                Some(_) => Resume::RowLog(ctx.row_log_mark()),
                None => Resume::Plain,
            });
            spawn(
                Frame::list_op(id, *list, op, args, *row_level, plan.result_type(id)),
                WalkMode::Same,
            )
        }
        NormalFormKind::Veto(veto) => done!(OperationResult::Veto(VetoType::UserDefined {
            message: veto.message.clone().filter(|m| !m.is_empty()),
        })),
        NormalFormKind::Sum(terms) => settle!(fold_step(
            cell,
            terms,
            (ArithmeticComputation::Add, ArithmeticComputation::Subtract),
            plan,
            ctx,
            walk,
        )),
        NormalFormKind::Product(terms) => settle!(fold_step(
            cell,
            terms,
            (
                ArithmeticComputation::Multiply,
                ArithmeticComputation::Divide
            ),
            plan,
            ctx,
            walk,
        )),
        NormalFormKind::Subtract(left, right) => {
            settle!(binary_settle(*left, *right, ctx, walk, |l, r| {
                binary_arithmetic_result(
                    l,
                    plan.result_type(*left),
                    r,
                    plan.result_type(*right),
                    ArithmeticComputation::Subtract,
                    plan,
                )
            },))
        }
        NormalFormKind::Divide(left, right) => {
            settle!(binary_settle(*left, *right, ctx, walk, |l, r| {
                binary_arithmetic_result(
                    l,
                    plan.result_type(*left),
                    r,
                    plan.result_type(*right),
                    ArithmeticComputation::Divide,
                    plan,
                )
            },))
        }
        NormalFormKind::Power(left, right) => {
            settle!(binary_settle(*left, *right, ctx, walk, |l, r| {
                binary_arithmetic_result(
                    l,
                    plan.result_type(*left),
                    r,
                    plan.result_type(*right),
                    ArithmeticComputation::Power,
                    plan,
                )
            },))
        }
        NormalFormKind::Modulo(left, right) => {
            settle!(binary_settle(*left, *right, ctx, walk, |l, r| {
                binary_arithmetic_result(
                    l,
                    plan.result_type(*left),
                    r,
                    plan.result_type(*right),
                    ArithmeticComputation::Modulo,
                    plan,
                )
            },))
        }
        NormalFormKind::Negate(inner) => {
            let value = need!(*inner);
            if value.vetoed() {
                done!(value)
            }
            let zero = OperationResult::from_literal(LiteralValue::number(rational_zero()));
            done!(binary_arithmetic_result(
                &zero,
                primitive_number_arc(),
                &value,
                plan.result_type(*inner),
                ArithmeticComputation::Subtract,
                plan,
            ))
        }
        NormalFormKind::Reciprocal(inner) => {
            let value = need!(*inner);
            if value.vetoed() {
                done!(value)
            }
            let one = OperationResult::from_literal(LiteralValue::number(rational_one()));
            done!(binary_arithmetic_result(
                &one,
                primitive_number_arc(),
                &value,
                plan.result_type(*inner),
                ArithmeticComputation::Divide,
                plan,
            ))
        }
        NormalFormKind::Comparison(left, op, right) => settle!(binary_settle(
            *left,
            *right,
            ctx,
            walk,
            |left_result, right_result| {
                comparison_operation(
                    &borrow_value(left_result, "left operand").value,
                    plan.result_type(*left),
                    op,
                    &borrow_value(right_result, "right operand").value,
                    plan.result_type(*right),
                )
            },
        )),
        NormalFormKind::Logical(connective, children) => {
            settle!(logical_step(cell, *connective, children, ctx, walk))
        }
        NormalFormKind::Not(inner) => {
            let value = need!(*inner);
            if value.vetoed() {
                done!(value)
            }
            let false_lit = OperationResult::from_literal(LiteralValue::from_bool(false));
            done!(comparison_operation(
                &borrow_value(&value, "not operand").value,
                plan.result_type(*inner),
                &ComparisonComputation::Is,
                &borrow_value(&false_lit, "not operand").value,
                primitive_boolean_arc(),
            ))
        }
        NormalFormKind::MathOp(op, inner) => {
            let value = need!(*inner);
            if value.vetoed() {
                done!(value)
            }
            done!(evaluate_mathematical_operator(
                op,
                borrow_value(&value, "operand"),
                plan.result_type(id),
            ))
        }
        NormalFormKind::StepRounding(mode, operand, step) => match step {
            None => {
                let value = need!(*operand);
                if value.vetoed() {
                    done!(value)
                }
                done!(integer_rounding(
                    *mode,
                    borrow_value(&value, "rounded operand"),
                ))
            }
            Some(step) => settle!(binary_settle(
                *operand,
                *step,
                ctx,
                walk,
                |operand_result, step_result| {
                    step_rounding(
                        *mode,
                        borrow_value(operand_result, "rounded operand"),
                        borrow_value(step_result, "rounding step"),
                        plan.result_type(id),
                    )
                },
            )),
        },
        NormalFormKind::UnitConversion(inner, target) => {
            let value = need!(*inner);
            if value.vetoed() {
                done!(value)
            }
            done!(convert_unit_operand(
                borrow_value(&value, "conversion operand"),
                plan.result_type(*inner).as_ref(),
                target,
            ))
        }
        NormalFormKind::DateRelative(kind, inner) => {
            let value = need!(*inner);
            if value.vetoed() {
                done!(value)
            }
            let date = match &borrow_value(&value, "date operand").value {
                ValueKind::Date(dt) => dt,
                other => panic!("BUG: date-relative operand expected date, got {other:?}"),
            };
            done!(compute_date_relative(kind, date, now_date(ctx),))
        }
        NormalFormKind::DateCalendar(kind, unit, inner) => {
            let value = need!(*inner);
            if value.vetoed() {
                done!(value)
            }
            let date = match &borrow_value(&value, "date operand").value {
                ValueKind::Date(dt) => dt,
                other => panic!("BUG: date-calendar operand expected date, got {other:?}"),
            };
            done!(compute_date_calendar(kind, unit, date, now_date(ctx),))
        }
        NormalFormKind::RangeLiteral(left_id, right_id) => settle!(binary_settle(
            *left_id,
            *right_id,
            ctx,
            walk,
            |left_result, right_result| {
                let left_bound = borrow_value(left_result, "left endpoint");
                let right_bound = borrow_value(right_result, "right endpoint");
                let left_typed = typed_literal_from_bound(left_bound, plan.result_type(*left_id));
                let right_typed =
                    typed_literal_from_bound(right_bound, plan.result_type(*right_id));
                OperationResult::from_literal(LiteralValue::range(left_typed, right_typed))
            },
        )),
        NormalFormKind::RangeBound(bound, inner) => {
            let value = need!(*inner);
            if value.vetoed() {
                done!(value)
            }
            let range_literal = borrow_value(&value, "range operand");
            match &range_literal.value {
                ValueKind::Range(left, right) => {
                    let endpoint_type = plan
                        .result_type(*inner)
                        .specifications
                        .element_from_range()
                        .map(|element| Arc::new(LemmaType::primitive(element)))
                        .expect("BUG: lower/upper requires a range result type");
                    match ordered_endpoints(&left.value, &right.value, &endpoint_type) {
                        Ok((lo, hi)) => {
                            let picked = match bound {
                                RangeBound::Lower => lo,
                                RangeBound::Upper => hi,
                            };
                            let chosen = if std::ptr::eq(picked, &left.value) {
                                left.as_ref()
                            } else {
                                right.as_ref()
                            };
                            let unit = chosen.lemma_type.unit().map(Arc::<str>::from);
                            done!(OperationResult::from_bound(BoundValueKind::with_binding(
                                chosen.value.clone(),
                                unit,
                            )))
                        }
                        Err(veto) => done!(OperationResult::Veto(veto)),
                    }
                }
                other => panic!("BUG: lower/upper expected a range value, got {other:?}"),
            }
        }
        NormalFormKind::PastFutureRange(kind, inner) => {
            let value = need!(*inner);
            if value.vetoed() {
                done!(value)
            }
            done!(evaluate_past_future_range(
                kind,
                &borrow_value(&value, "offset operand").to_literal(),
                plan.result_type(*inner),
                now_date(ctx),
            ))
        }
        NormalFormKind::RangeContainment(value, range) => settle!(binary_settle(
            *value,
            *range,
            ctx,
            walk,
            |value_result, range_result| {
                let range_literal = borrow_value(range_result, "range operand");
                match &range_literal.value {
                    ValueKind::Range(range_left, range_right) => {
                        let endpoint_type = plan
                            .result_type(*range)
                            .specifications
                            .element_from_range()
                            .map(|element| Arc::new(LemmaType::primitive(element)))
                            .expect("BUG: range containment requires a range result type");
                        check_containment(
                            &borrow_value(value_result, "value operand").value,
                            plan.result_type(*value),
                            &range_left.value,
                            &range_right.value,
                            &endpoint_type,
                        )
                    }
                    other => {
                        panic!("BUG: range containment expected range operand, got {other:?}")
                    }
                }
            },
        )),
        NormalFormKind::ResultIsVeto(inner) => {
            let value = need!(*inner);
            done!(OperationResult::from_literal(LiteralValue::from_bool(
                value.vetoed(),
            )))
        }
        NormalFormKind::Piecewise(arms) => {
            assert!(!arms.is_empty(), "BUG: empty piecewise");
            while cell.cursor + 1 < arms.len() {
                let (condition, body) = arms[arms.len() - 1 - cell.cursor];
                match condition_outcome(&need!(condition)) {
                    BranchOutcome::Propagate(result) => done!(result),
                    BranchOutcome::Taken => {
                        let value = need!(body);
                        done!(value)
                    }
                    BranchOutcome::NotTaken => cell.cursor += 1,
                }
            }
            let value = need!(arms[0].1);
            done!(value)
        }
        NormalFormKind::OrderedDispatch {
            scrutinee,
            boundaries,
            regions,
        } => {
            assert_eq!(
                regions.len(),
                region_count(boundaries.len()),
                "BUG: OrderedDispatch region table does not match its boundary list"
            );
            let scrutinee_result = need!(*scrutinee);
            if scrutinee_result.vetoed() {
                done!(scrutinee_result)
            }
            let value = borrow_value(&scrutinee_result, "dispatch scrutinee");
            let region = match region_of_scrutinee(boundaries, &value.value) {
                Ok(region) => region,
                Err(veto) => done!(OperationResult::Veto(veto)),
            };
            let value = need!(regions[region].cell());
            done!(value)
        }
    }
}

/// Completes a cell: an exhaustive run first visits the cell's origin, then the value is
/// stored by the store policy.
fn cell_done<'p>(
    cell: &mut CellFrame,
    walk: &WalkDone,
    value: OperationResult,
    plan: &'p ExecutionPlan,
    ctx: &mut EvaluationContext,
) -> Outcome<'p> {
    let id = cell.id;
    let origin = if ctx.exhaustive {
        plan.normal_form(id).origin
    } else {
        None
    };
    if let Some(origin) = origin {
        if ready(origin, ctx, walk).is_none() {
            cell.settled = Some(value);
            return spawn(Frame::cell(origin), WalkMode::Same);
        }
    }
    Outcome::Finish(Some(finish(id, value, plan, ctx)))
}

/// N-ary Sum / Product. A MissingData child does not end the fold: a later
/// definitive veto is the answer instead. After a definitive veto the remaining
/// children are visited only under the exhaustive policy; the value is fixed.
/// Progressive accumulator types come from the Sum/Product cell's `fold_types`.
/// Operands left to right. Returns the child a step still needs when a child is pending.
fn fold_step(
    cell: &mut CellFrame,
    terms: &[ArithmeticTerm],
    (direct_op, inverse_op): (ArithmeticComputation, ArithmeticComputation),
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
    walk: &WalkDone,
) -> Result<OperationResult, NormalFormId> {
    assert!(!terms.is_empty(), "BUG: empty n-ary arithmetic");
    let fold_types = &plan.normal_form(cell.id).fold_types;
    assert_eq!(
        fold_types.len(),
        terms.len(),
        "BUG: Sum/Product fold_types length must match terms"
    );
    while cell.cursor < terms.len() {
        if let Some(settled) = &cell.veto {
            if !settled.is_missing_data() && !ctx.exhaustive {
                break;
            }
        }
        let i = cell.cursor;
        let term = terms[i];
        let child = term.operand;
        let Some(result) = ready(child, ctx, walk) else {
            return Err(child);
        };
        cell.cursor += 1;
        if let Some(settled) = &cell.veto {
            if settled.is_missing_data() && result.vetoed() && !result.is_missing_data() {
                cell.veto = Some(result);
            }
            continue;
        }
        if result.vetoed() {
            cell.veto = Some(result);
            continue;
        }
        cell.acc = Some(match cell.acc.take() {
            None => result,
            Some(left) => {
                // left type = fold_types[i-1]; next left (if any) = fold_types[i].
                let op = if term.inverse {
                    inverse_op.clone()
                } else {
                    direct_op.clone()
                };
                let combined = binary_arithmetic_result(
                    &left,
                    &fold_types[i - 1],
                    &result,
                    plan.result_type(child),
                    op,
                    plan,
                );
                if combined.vetoed() {
                    cell.veto = Some(combined);
                    continue;
                }
                combined
            }
        });
    }

    let veto = cell.veto.take();
    let acc = cell.acc.take();
    Ok(veto
        .or(acc)
        .expect("BUG: n-ary arithmetic produced neither value nor veto after evaluating children"))
}

/// Operands left to right. The absorbing literal of `connective` decides regardless
/// of position (`true` for `or`, `false` for `and`). Without an absorbing operand,
/// the first missing or vetoed operand in source order propagates; remaining operands
/// are still visited so a later absorbing literal can decide. Otherwise the identity
/// literal (`false` for `or`, `true` for `and`).
///
/// An absorbing decision does not visit remaining operands (including under the
/// exhaustive explain policy): unread siblings must not become required inputs.
fn logical_step(
    cell: &mut CellFrame,
    connective: LogicalConnective,
    children: &[NormalFormId],
    ctx: &EvaluationContext,
    walk: &WalkDone,
) -> Result<OperationResult, NormalFormId> {
    assert!(
        children.len() >= 2,
        "BUG: Logical has at least two operands"
    );
    let absorbing = connective.absorbing_operand();
    loop {
        let child = children[cell.cursor];
        let result = ready(child, ctx, walk).ok_or(child)?;
        match condition_outcome(&result) {
            BranchOutcome::Taken if absorbing => {
                return Ok(OperationResult::from_literal(LiteralValue::from_bool(true)));
            }
            BranchOutcome::NotTaken if !absorbing => {
                return Ok(OperationResult::from_literal(LiteralValue::from_bool(
                    false,
                )));
            }
            BranchOutcome::Propagate(propagated) => {
                cell.veto.get_or_insert(propagated);
            }
            BranchOutcome::Taken | BranchOutcome::NotTaken => {}
        }
        cell.cursor += 1;
        if cell.cursor == children.len() {
            return Ok(cell.veto.take().unwrap_or_else(|| {
                OperationResult::from_literal(LiteralValue::from_bool(!absorbing))
            }));
        }
    }
}

/// Binary operator with the sibling rule of [`fold_step`]: a MissingData left still
/// visits the right operand, and a definitive veto there wins. A definitive left
/// veto is the value; the right operand is visited only under the exhaustive policy.
/// `Err(child)` names the child that must be evaluated first.
fn binary_settle<F>(
    left: NormalFormId,
    right: NormalFormId,
    ctx: &EvaluationContext,
    walk: &WalkDone,
    combine: F,
) -> Result<OperationResult, NormalFormId>
where
    F: FnOnce(&OperationResult, &OperationResult) -> OperationResult,
{
    let left_result = ready(left, ctx, walk).ok_or(left)?;
    if left_result.vetoed() && !left_result.is_missing_data() {
        if ctx.exhaustive {
            ready(right, ctx, walk).ok_or(right)?;
        }
        return Ok(left_result);
    }
    let right_result = ready(right, ctx, walk).ok_or(right)?;
    if left_result.vetoed() {
        if right_result.vetoed() && !right_result.is_missing_data() {
            return Ok(right_result);
        }
        return Ok(left_result);
    }
    if right_result.vetoed() {
        return Ok(right_result);
    }
    Ok(combine(&left_result, &right_result))
}

fn binary_arithmetic_result(
    left: &OperationResult,
    left_type: &Arc<LemmaType>,
    right: &OperationResult,
    right_type: &Arc<LemmaType>,
    op: ArithmeticComputation,
    plan: &ExecutionPlan,
) -> OperationResult {
    arithmetic_operation(
        borrow_value(left, "left operand"),
        left_type,
        &op,
        borrow_value(right, "right operand"),
        right_type,
        &plan.signature_index,
    )
}

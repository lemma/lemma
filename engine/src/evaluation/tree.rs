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
//! finishes with a value, spawns a child frame and waits for it, or enters / leaves a
//! nested context scope (a named `uses` import or an instance-list row). Scopes are
//! owned by a second heap stack, so no Rust call stack grows with expression depth,
//! rule chains, import nesting, self-referencing row trees or list nesting.

use crate::computation::{
    arithmetic_operation, comparison_operation, convert_unit_operand, OperationResult, VetoType,
};
use crate::evaluation::branch_semantics::{condition_outcome, BranchOutcome};
use crate::evaluation::expression::{evaluate_mathematical_operator, resolve_data_path_value};
use crate::evaluation::imports;
use crate::evaluation::list_ops::{self, ListFold};
use crate::evaluation::rows::{self, RowRead};
use crate::evaluation::{ignored_under_prefix, EvaluateData, EvaluationContext, RowLogMark};
use crate::planning::execution_plan::{ExecutionPlan, RuleIndex};
use crate::planning::instance_list::{
    absolute_reference, row_end_type, typed_from_bound, DataSource, InstanceListSource, KeyedRow,
};
use crate::planning::normalize::{
    ArithmeticTerm, LeafKind, NormalFormId, NormalFormKind, NormalFormListOp,
};
use crate::planning::ordered_dispatch::{region_count, region_of_scrutinee};
use crate::planning::semantics::{
    ArithmeticComputation, BoundValueKind, ComparisonComputation, DataDefinition, DataPath,
    ImportOverlay, LemmaType, ListKey, LiteralValue, RangeBound, ReferenceEnd, ReferenceTarget,
    RowEnd, RowInstance, RowOrigin, RulePath, SpecInstance, SpecValue, TypeSpecification,
    TypedLiteral, ValueKind,
};
use indexmap::IndexMap;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::Arc;

pub(crate) fn borrow_value<'a>(
    result: &'a OperationResult,
    operand: &str,
) -> &'a crate::planning::semantics::BoundValueKind {
    match result {
        OperationResult::Value(v) => v,
        OperationResult::Veto(_) => panic!("BUG: {operand} passed veto check but has no value"),
    }
}

fn now_date(ctx: &EvaluationContext) -> &crate::planning::semantics::SemanticDateTime {
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

/// Value of the named-`uses` member `path` of `plan`, peeled into its template scope.
pub(crate) fn evaluate_import_member(
    path: &DataPath,
    plan: &ExecutionPlan,
    ctx: &mut EvaluationContext,
) -> OperationResult {
    run(plan, ctx, Frame::import_member(path))
        .expect("BUG: import member evaluation produced no value")
}

/// Context of `row` over `template`, taken from `ctx` when built or built bottom-up.
pub(crate) fn build_row_context(
    row: &Arc<RowInstance>,
    template: &ExecutionPlan,
    ctx: &mut EvaluationContext,
) -> EvaluationContext {
    run(
        template,
        ctx,
        Frame::row_scope(Arc::clone(row), template, RowBody::Nothing),
    );
    ctx.row_contexts
        .remove(row)
        .expect("BUG: row context missing after its scope was built")
}

/// Finished cells of one walk, by cell id. Values not stored in the value table live here.
type WalkDone = HashMap<NormalFormId, OperationResult>;

/// Result a frame hands to the frame that spawned it.
type Delivered = Option<OperationResult>;

fn ready(id: NormalFormId, ctx: &EvaluationContext, walk: &WalkDone) -> Option<OperationResult> {
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

/// Where a scope's context goes back to when the scope is left.
enum Home {
    Import(DataPath),
    Row(Arc<RowInstance>),
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
    /// The current scope is left, its context returned home; the frame completes.
    Exit(Delivered, Home),
}

struct Frame<'p> {
    kind: Kind<'p>,
    /// Index of the walk memo this frame reads and writes.
    walk: usize,
    owns_walk: bool,
    /// Value delivered by the child this frame waits for.
    inbox: Delivered,
}

enum Kind<'p> {
    Cell(CellFrame<'p>),
    Rule(RuleFrame),
    ImportRule(&'p RulePath),
    ImportMember(&'p DataPath),
    ImportScope(ImportScopeFrame),
    Relative(RelativeFrame),
    ImportRuleFrom(ImportRuleFromFrame),
    RowScope(RowScopeFrame<'p>),
    ReadRow(ReadRowFrame),
    Materialize(MaterializeFrame<'p>),
    ListOp(ListOpFrame<'p>),
}

impl<'p> Frame<'p> {
    fn of(kind: Kind<'p>) -> Self {
        Self {
            kind,
            walk: 0,
            owns_walk: false,
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
        }))
    }

    fn rule(index: RuleIndex) -> Self {
        Self::of(Kind::Rule(RuleFrame {
            index,
            depth: 0,
            saved_rows: None,
        }))
    }

    fn import_rule(path: &'p RulePath) -> Self {
        Self::of(Kind::ImportRule(path))
    }

    fn import_member(path: &'p DataPath) -> Self {
        Self::of(Kind::ImportMember(path))
    }

    fn import_scope(
        import_path: DataPath,
        extras: IndexMap<DataPath, OperationResult>,
        body: Option<Body>,
    ) -> Self {
        Self::of(Kind::ImportScope(ImportScopeFrame {
            import_path,
            extras,
            body,
            phase: ScopePhase::Overlay,
            overlay_cursor: 0,
            overlay_values: IndexMap::new(),
            seeds: Vec::new(),
            seed_cursor: 0,
        }))
    }

    fn relative(relative: DataPath, overlay: IndexMap<DataPath, OperationResult>) -> Self {
        Self::of(Kind::Relative(RelativeFrame { relative, overlay }))
    }

    fn import_rule_from(path: RulePath, overlay: IndexMap<DataPath, OperationResult>) -> Self {
        Self::of(Kind::ImportRuleFrom(ImportRuleFromFrame { path, overlay }))
    }

    fn row_scope(row: Arc<RowInstance>, template: &'p ExecutionPlan, body: RowBody) -> Self {
        Self::of(Kind::RowScope(RowScopeFrame {
            row,
            template,
            body,
            phase: RowPhase::Acquire,
        }))
    }

    fn read_row(row: Arc<RowInstance>, end: RowEnd) -> Self {
        Self::of(Kind::ReadRow(ReadRowFrame {
            row,
            end,
            rule_root: None,
        }))
    }

    fn materialize(path: &'p DataPath, source: &'p InstanceListSource) -> Self {
        Self::of(Kind::Materialize(MaterializeFrame {
            path,
            source,
            row_type: Arc::new(LemmaType::primitive(TypeSpecification::Spec {
                spec: source.identity.clone(),
            })),
            rows: Vec::with_capacity(source.rows.len()),
            row: 0,
            field: 0,
            bindings: IndexMap::new(),
        }))
    }

    fn list_op(
        list: NormalFormId,
        op: &'p NormalFormListOp,
        args: &'p [NormalFormId],
        row_level: Option<u32>,
        result_type: &'p Arc<LemmaType>,
    ) -> Self {
        Self::of(Kind::ListOp(ListOpFrame {
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
struct CellFrame<'p> {
    id: NormalFormId,
    /// Progress through a sequence of children: piecewise arms from the top, fold
    /// operands from the first, list command arguments from the first.
    cursor: usize,
    acc: Option<OperationResult>,
    veto: Option<OperationResult>,
    /// Value computed while the exhaustive origin visit is still pending.
    settled: Option<OperationResult>,
    /// Set while an operator child (not a cell of this walk) is awaited.
    resume: Option<Resume<'p>>,
}

enum Resume<'p> {
    Plain,
    /// Import member read: a value that is not missing is stored in the leaf slot.
    Import(&'p DataPath),
    /// Row read or list command: the row logs written since `mark` are recorded.
    RowLog(RowLogMark),
}

struct RuleFrame {
    index: RuleIndex,
    /// Scope depth the rule runs in; a rule active twice in one scope is a cycle.
    depth: usize,
    /// Row stack of the requesting walk, cleared while the rule evaluates.
    saved_rows: Option<Vec<Arc<RowInstance>>>,
}

#[derive(Clone)]
enum Body {
    /// Relative member read inside the import scope.
    Relative(DataPath),
    /// Named-`uses` rule read inside the import scope.
    ImportRuleFrom(RulePath),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScopePhase {
    /// Overlay values resolved in the owner scope (rule overlays may spawn rules).
    Overlay,
    /// Nested overlay import scopes built inside the new scope.
    Seed,
    Body,
    Await,
}

struct ImportScopeFrame {
    import_path: DataPath,
    extras: IndexMap<DataPath, OperationResult>,
    body: Option<Body>,
    phase: ScopePhase,
    overlay_cursor: usize,
    overlay_values: IndexMap<DataPath, OperationResult>,
    seeds: Vec<(DataPath, IndexMap<DataPath, OperationResult>)>,
    seed_cursor: usize,
}

struct RelativeFrame {
    relative: DataPath,
    overlay: IndexMap<DataPath, OperationResult>,
}

struct ImportRuleFromFrame {
    path: RulePath,
    overlay: IndexMap<DataPath, OperationResult>,
}

#[derive(Clone, Copy)]
enum RowBody {
    /// Only the row context is built.
    Nothing,
    /// The row's rule value, read after the context is built.
    Rule(RuleIndex),
    /// A row data leaf, read after the context is built.
    Leaf(NormalFormId),
}

enum RowPhase {
    Acquire,
    /// Self-list rows still to build, last first.
    Children(Vec<Arc<RowInstance>>),
    /// Self-list rules of the row, from this index.
    Rules(usize),
    Body,
    Await,
}

struct RowScopeFrame<'p> {
    row: Arc<RowInstance>,
    template: &'p ExecutionPlan,
    body: RowBody,
    phase: RowPhase,
}

struct ReadRowFrame {
    row: Arc<RowInstance>,
    end: RowEnd,
    /// Root of the row's rule when the field is a rule, for missing-data keys.
    rule_root: Option<NormalFormId>,
}

struct MaterializeFrame<'p> {
    path: &'p DataPath,
    source: &'p InstanceListSource,
    row_type: Arc<LemmaType>,
    rows: Vec<TypedLiteral>,
    row: usize,
    field: usize,
    bindings: IndexMap<DataPath, OperationResult>,
}

struct ListData {
    rows: Vec<TypedLiteral>,
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
        key_fields: Option<Vec<String>>,
    },
    /// Per-row argument values of an instance list, in row order.
    PerRow {
        data: ListData,
        cursor: usize,
        values: Vec<TypedLiteral>,
        first_veto: Option<OperationResult>,
    },
    /// Key-field reads over rows of an instance list.
    Scan(KeyScan),
}

struct KeyScan {
    rows: Vec<TypedLiteral>,
    fields: Vec<String>,
    row: usize,
    field: usize,
    key: Vec<TypedLiteral>,
    purpose: ScanPurpose,
}

enum ScanPurpose {
    Find {
        wanted: Vec<TypedLiteral>,
    },
    Append {
        keyed: Vec<KeyedRow>,
        unit: Option<Arc<str>>,
    },
}

struct ListOpFrame<'p> {
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
    let mut frames = vec![first];
    let mut walks: Vec<WalkDone> = Vec::new();
    let mut scopes: Vec<Scope<'p>> = Vec::new();
    loop {
        let mut frame = frames
            .pop()
            .expect("BUG: evaluation machine ran out of frames");
        let outcome = {
            let (plan, ctx) = top(root_plan, &mut *root, &mut scopes);
            step(&mut frame, plan, ctx, &walks)
        };
        if let Outcome::Spawn(child, WalkMode::New) = &outcome {
            if let Kind::Cell(cell) = &child.kind {
                let (_, ctx) = top(root_plan, &mut *root, &mut scopes);
                if let Some(cached) = ctx.values[cell.id.index()].clone() {
                    frame.inbox = Some(cached);
                    frames.push(frame);
                    continue;
                }
            }
        }
        match outcome {
            Outcome::Spawn(mut child, mode) => {
                match mode {
                    WalkMode::Same => child.walk = frame.walk,
                    WalkMode::New => {
                        walks.push(WalkDone::new());
                        child.walk = walks.len() - 1;
                        child.owns_walk = true;
                    }
                }
                if let Kind::Rule(rule) = &mut child.kind {
                    let depth = scopes.len();
                    rule.depth = depth;
                    let index = rule.index;
                    let cyclic = frames
                        .iter()
                        .chain(std::iter::once(&frame))
                        .any(|active| {
                            matches!(&active.kind, Kind::Rule(r) if r.index == index && r.depth == depth)
                        });
                    if cyclic {
                        let (plan, _) = top(root_plan, &mut *root, &mut scopes);
                        panic!(
                            "BUG: cyclic rule reference while ensuring values for '{}'",
                            plan.rule_at(index).path.rule
                        );
                    }
                }
                frames.push(frame);
                frames.push(child);
            }
            Outcome::Enter(scope) => {
                scopes.push(scope);
                walks.push(WalkDone::new());
                frame.walk = walks.len() - 1;
                frame.owns_walk = true;
                frames.push(frame);
            }
            Outcome::Finish(value) => {
                let stored = match &frame.kind {
                    Kind::Cell(cell) => {
                        let (_, ctx) = top(root_plan, &mut *root, &mut scopes);
                        ctx.values[cell.id.index()].is_some()
                    }
                    _ => false,
                };
                if let Some(result) = complete(frame, value, stored, &mut frames, &mut walks) {
                    return result;
                }
            }
            Outcome::Exit(value, home) => {
                let scope = scopes.pop().expect("BUG: scope left without being entered");
                let (_, parent) = top(root_plan, &mut *root, &mut scopes);
                match home {
                    Home::Import(marker) => {
                        parent.import_contexts.insert(marker, scope.ctx);
                    }
                    Home::Row(row) => {
                        parent.row_contexts.insert(row, scope.ctx);
                    }
                }
                if let Some(result) = complete(frame, value, false, &mut frames, &mut walks) {
                    return result;
                }
            }
        }
    }
}

/// Plan and context of the innermost scope; the root scope when none is entered.
fn top<'a, 'p>(
    root_plan: &'p ExecutionPlan,
    root: &'a mut EvaluationContext,
    scopes: &'a mut Vec<Scope<'p>>,
) -> (&'p ExecutionPlan, &'a mut EvaluationContext) {
    match scopes.last_mut() {
        Some(scope) => (scope.plan, &mut scope.ctx),
        None => (root_plan, root),
    }
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
    plan: &'p ExecutionPlan,
    ctx: &mut EvaluationContext,
    walks: &[WalkDone],
) -> Outcome<'p> {
    let inbox = frame.inbox.take();
    let walk = frame.walk;
    match &mut frame.kind {
        Kind::Cell(cell) => step_cell(cell, inbox, &walks[walk], plan, ctx),
        Kind::Rule(rule) => step_rule(rule, inbox, plan, ctx),
        Kind::ImportRule(path) => step_import_rule(path, inbox, plan, ctx),
        Kind::ImportMember(path) => step_import_member(path, inbox, plan, ctx),
        Kind::ImportScope(scope) => step_import_scope(scope, inbox, plan, ctx),
        Kind::Relative(relative) => step_relative(relative, inbox, plan, ctx),
        Kind::ImportRuleFrom(from) => step_import_rule_from(from, inbox, plan, ctx),
        Kind::RowScope(scope) => step_row_scope(scope, inbox, ctx),
        Kind::ReadRow(read) => step_read_row(read, inbox, plan, ctx),
        Kind::Materialize(materialize) => step_materialize(materialize, inbox, plan),
        Kind::ListOp(list) => step_list_op(list, inbox, plan, ctx),
    }
}

/// Rule `index` of `plan`: its normal form is evaluated once per request and stored in
/// `rule_values`. A rule that is its own named `uses` import evaluates through the import.
fn step_rule<'p>(
    frame: &mut RuleFrame,
    inbox: Delivered,
    plan: &'p ExecutionPlan,
    ctx: &mut EvaluationContext,
) -> Outcome<'p> {
    let index = frame.index;
    let rule = plan.rule_at(index);
    let self_import = matches!(
        &plan.normal_form(rule.normal_form).kind,
        NormalFormKind::Leaf(LeafKind::ImportRule(path)) if *path == rule.path
    );
    if let Some(value) = inbox {
        if let Some(rows) = frame.saved_rows.take() {
            ctx.row_stack = rows;
        }
        if self_import {
            ctx.values[rule.normal_form.index()] = Some(value.clone());
        }
        ctx.rule_values[index.index()] = Some(value);
        return Outcome::Finish(None);
    }
    if ctx.rule_values[index.index()].is_some() {
        return Outcome::Finish(None);
    }
    frame.saved_rows = Some(std::mem::take(&mut ctx.row_stack));
    match &plan.normal_form(rule.normal_form).kind {
        NormalFormKind::Leaf(LeafKind::ImportRule(path)) if *path == rule.path => {
            Outcome::Spawn(Frame::import_rule(path), WalkMode::Same)
        }
        _ => Outcome::Spawn(Frame::cell(rule.normal_form), WalkMode::New),
    }
}

/// Named `uses` rule `path` read from the owner scope: the rule's value inside the
/// import scope, mapped back into the owner's frame.
fn step_import_rule<'p>(
    path: &'p RulePath,
    inbox: Delivered,
    plan: &'p ExecutionPlan,
    ctx: &mut EvaluationContext,
) -> Outcome<'p> {
    if let Some(result) = inbox {
        return Outcome::Finish(Some(imports::into_owner_frame(path, plan, result)));
    }
    let mut hops = path.segments.iter();
    let first = hops
        .next()
        .unwrap_or_else(|| panic!("BUG: eval_import_rule on local rule '{}'", path.rule));
    let marker = DataPath::local(first.uses.clone());
    if !matches!(plan.data.get(&marker), Some(DataDefinition::Import { .. })) {
        panic!("BUG: import rule '{}' first hop is not Import", path.rule);
    }
    let extras = imports::consumer_values_under_hop(plan, ctx, &first.uses);
    let nested = RulePath {
        segments: hops.cloned().collect(),
        rule: path.rule.clone(),
    };
    Outcome::Spawn(
        Frame::import_scope(marker, extras, Some(Body::ImportRuleFrom(nested))),
        WalkMode::Same,
    )
}

/// Member `path` of a named `uses` import, read from the owner scope.
fn step_import_member<'p>(
    path: &'p DataPath,
    inbox: Delivered,
    plan: &'p ExecutionPlan,
    ctx: &mut EvaluationContext,
) -> Outcome<'p> {
    if let Some(result) = inbox {
        return Outcome::Finish(Some(imports::qualify_import_member_missing(
            path, plan, result,
        )));
    }
    let mut hops = path.segments.iter();
    let first = hops.next().unwrap_or_else(|| {
        panic!(
            "BUG: eval_import_member on local path '{}'",
            path.input_key()
        )
    });
    let marker = DataPath::local(first.uses.clone());
    if !matches!(plan.data.get(&marker), Some(DataDefinition::Import { .. })) {
        panic!(
            "BUG: import member '{}' first hop is not Import",
            path.input_key()
        );
    }
    let relative = DataPath {
        segments: hops.cloned().collect(),
        data: path.data.clone(),
    };
    let extras = imports::consumer_values_under_hop(plan, ctx, &first.uses);
    Outcome::Spawn(
        Frame::import_scope(marker, extras, Some(Body::Relative(relative))),
        WalkMode::Same,
    )
}

/// Enters the import context of `import_path`: the overlay is resolved in the owner scope
/// (rule overlays spawn rules there), the context is taken from the owner's cache or built
/// with its nested overlay imports, the body runs inside it, and the context goes back to
/// the owner's cache.
fn step_import_scope<'p>(
    frame: &mut ImportScopeFrame,
    mut inbox: Delivered,
    plan: &'p ExecutionPlan,
    ctx: &mut EvaluationContext,
) -> Outcome<'p> {
    loop {
        match frame.phase {
            ScopePhase::Overlay => {
                let Some(DataDefinition::Import {
                    identity, overlay, ..
                }) = plan.data.get(&frame.import_path)
                else {
                    panic!(
                        "BUG: import context for non-import '{}'",
                        frame.import_path.input_key()
                    );
                };
                let template = plan.row_template(identity);
                while frame.overlay_cursor < overlay.len() {
                    let (path, entry) = overlay
                        .get_index(frame.overlay_cursor)
                        .expect("BUG: overlay cursor is within the overlay");
                    let raw = match entry {
                        ImportOverlay::Literal(bound) => OperationResult::from_bound(bound.clone()),
                        ImportOverlay::Reference {
                            target: ReferenceTarget::Data(target),
                            ..
                        } => ctx.data_slot(plan, target).cloned().unwrap_or_else(|| {
                            // Owner slot still open: imported field carries that MissingData.
                            OperationResult::Veto(VetoType::missing_data(
                                target.clone(),
                                ctx.missing_data_suggestion(target),
                            ))
                        }),
                        ImportOverlay::Reference {
                            target: ReferenceTarget::Rule(rule),
                            ..
                        } => {
                            let index = plan.rule_index(rule);
                            match &ctx.rule_values[index.index()] {
                                Some(value) => value.clone(),
                                None => return Outcome::Spawn(Frame::rule(index), WalkMode::Same),
                            }
                        }
                    };
                    let value = imports::check_overlay_value(
                        entry,
                        raw,
                        plan,
                        template,
                        &frame.import_path,
                        path,
                    );
                    frame.overlay_values.insert(path.clone(), value);
                    frame.overlay_cursor += 1;
                }
                // Parent-frame values for this alias override the nested site overlay: the outermost binding wins.
                for (path, value) in std::mem::take(&mut frame.extras) {
                    frame.overlay_values.insert(path, value);
                }
                if let Some(cached) = ctx.import_contexts.remove(&frame.import_path) {
                    frame.phase = ScopePhase::Body;
                    return Outcome::Enter(Scope {
                        plan: template,
                        ctx: cached,
                    });
                }
                let mut bindings = IndexMap::new();
                for (path, value) in &frame.overlay_values {
                    if path.segments.is_empty() {
                        bindings.insert(path.clone(), value.clone());
                    }
                }
                let mut body = EvaluationContext::new(
                    template,
                    &EvaluateData {
                        bindings,
                        ignored_unknown: ignored_under_prefix(
                            &ctx.ignored_unknown,
                            &frame.import_path.input_key(),
                        ),
                    },
                    ctx.now().clone(),
                    ctx.exhaustive,
                );
                for (path, value) in &frame.overlay_values {
                    if path.segments.is_empty() {
                        continue;
                    }
                    if let Some(&leaf) = template.data_leaf.get(path) {
                        body.values[leaf.index()] = Some(value.clone());
                    }
                }
                frame.seeds = imports::seed_queue(template, &frame.overlay_values);
                frame.phase = ScopePhase::Seed;
                return Outcome::Enter(Scope {
                    plan: template,
                    ctx: body,
                });
            }
            ScopePhase::Seed => {
                if let Some((marker, extras)) = frame.seeds.get_mut(frame.seed_cursor) {
                    let marker = marker.clone();
                    let extras = std::mem::take(extras);
                    frame.seed_cursor += 1;
                    return Outcome::Spawn(
                        Frame::import_scope(marker, extras, None),
                        WalkMode::Same,
                    );
                }
                frame.phase = ScopePhase::Body;
            }
            ScopePhase::Body => match frame.body.take() {
                None => {
                    return Outcome::Exit(None, Home::Import(frame.import_path.clone()));
                }
                Some(Body::Relative(relative)) => {
                    frame.phase = ScopePhase::Await;
                    let overlay = std::mem::take(&mut frame.overlay_values);
                    return Outcome::Spawn(Frame::relative(relative, overlay), WalkMode::Same);
                }
                Some(Body::ImportRuleFrom(path)) => {
                    frame.phase = ScopePhase::Await;
                    let overlay = std::mem::take(&mut frame.overlay_values);
                    return Outcome::Spawn(Frame::import_rule_from(path, overlay), WalkMode::Same);
                }
            },
            ScopePhase::Await => {
                let result = inbox
                    .take()
                    .expect("BUG: import scope resumed without its body value");
                return Outcome::Exit(Some(result), Home::Import(frame.import_path.clone()));
            }
        }
    }
}

/// Relative member `relative` inside an import scope: a value from the overlay, a data
/// slot, or the next named import hop entered as its own scope.
fn step_relative<'p>(
    frame: &mut RelativeFrame,
    inbox: Delivered,
    plan: &'p ExecutionPlan,
    ctx: &mut EvaluationContext,
) -> Outcome<'p> {
    if let Some(result) = inbox {
        return Outcome::Finish(Some(result));
    }
    if let Some(value) = frame.overlay.get(&frame.relative) {
        return Outcome::Finish(Some(value.clone()));
    }
    if frame.relative.segments.is_empty() {
        if let Some(slot) = ctx.data_slot(plan, &frame.relative) {
            return Outcome::Finish(Some(slot.clone()));
        }
        let target = plan
            .promptable_data_path(&frame.relative)
            .unwrap_or(&frame.relative);
        return Outcome::Finish(Some(OperationResult::Veto(VetoType::missing_data(
            target.clone(),
            ctx.missing_data_suggestion(target),
        ))));
    }
    let hop = &frame.relative.segments[0];
    let marker = DataPath::local(hop.uses.clone());
    if !matches!(plan.data.get(&marker), Some(DataDefinition::Import { .. })) {
        panic!(
            "BUG: nested hop '{}' is not Import on template while peeling '{}'",
            hop.uses,
            frame.relative.input_key()
        );
    }
    let extras = imports::values_under_hop(&frame.overlay, &hop.uses);
    let nested = DataPath {
        segments: frame.relative.segments[1..].to_vec(),
        data: frame.relative.data.clone(),
    };
    Outcome::Spawn(
        Frame::import_scope(marker, extras, Some(Body::Relative(nested))),
        WalkMode::Same,
    )
}

/// Named `uses` rule `path` inside an import scope: the template's rule value, or the
/// next named import hop entered as its own scope.
fn step_import_rule_from<'p>(
    frame: &mut ImportRuleFromFrame,
    inbox: Delivered,
    plan: &'p ExecutionPlan,
    ctx: &mut EvaluationContext,
) -> Outcome<'p> {
    if let Some(result) = inbox {
        return Outcome::Finish(Some(result));
    }
    if frame.path.segments.is_empty() {
        let index = plan.rule_index(&frame.path);
        return match &ctx.rule_values[index.index()] {
            Some(value) => Outcome::Finish(Some(value.clone())),
            None => Outcome::Spawn(Frame::rule(index), WalkMode::Same),
        };
    }
    let hop = &frame.path.segments[0];
    let marker = DataPath::local(hop.uses.clone());
    if !matches!(plan.data.get(&marker), Some(DataDefinition::Import { .. })) {
        panic!("BUG: nested hop for import rule is not Import");
    }
    let extras = imports::values_under_hop(&frame.overlay, &hop.uses);
    let nested = RulePath {
        segments: frame.path.segments[1..].to_vec(),
        rule: frame.path.rule.clone(),
    };
    Outcome::Spawn(
        Frame::import_scope(marker, extras, Some(Body::ImportRuleFrom(nested))),
        WalkMode::Same,
    )
}

/// Row context of `row`: built bottom-up over its self-list rows, then the row's
/// body (rule or data leaf) read inside it. The context returns home when the scope is left.
fn step_row_scope<'p>(
    frame: &mut RowScopeFrame<'p>,
    mut inbox: Delivered,
    ctx: &mut EvaluationContext,
) -> Outcome<'p> {
    loop {
        match &mut frame.phase {
            RowPhase::Acquire => {
                let template = frame.template;
                if let Some(cached) = ctx.row_contexts.remove(&frame.row) {
                    frame.phase = RowPhase::Body;
                    return Outcome::Enter(Scope {
                        plan: template,
                        ctx: cached,
                    });
                }
                let fresh = EvaluationContext::new(
                    template,
                    &EvaluateData {
                        bindings: frame
                            .row
                            .bindings
                            .iter()
                            .map(|(path, value)| (path.clone(), value.clone()))
                            .collect(),
                        ignored_unknown: Vec::new(),
                    },
                    ctx.now().clone(),
                    ctx.exhaustive,
                );
                frame.phase = if template.self_row_rules.is_empty() {
                    RowPhase::Body
                } else {
                    RowPhase::Children(rows::self_rows(&frame.row, template))
                };
                return Outcome::Enter(Scope {
                    plan: template,
                    ctx: fresh,
                });
            }
            RowPhase::Children(children) => {
                let Some(child) = children.pop() else {
                    frame.phase = RowPhase::Rules(0);
                    continue;
                };
                if ctx.row_contexts.contains_key(&child) {
                    continue;
                }
                return Outcome::Spawn(
                    Frame::row_scope(child, frame.template, RowBody::Nothing),
                    WalkMode::Same,
                );
            }
            RowPhase::Rules(cursor) => {
                let template = frame.template;
                let Some(rule) = template.self_row_rules.get(*cursor) else {
                    frame.phase = RowPhase::Body;
                    continue;
                };
                let index = template.rule_index(rule);
                if ctx.rule_values[index.index()].is_some() {
                    *cursor += 1;
                    continue;
                }
                return Outcome::Spawn(Frame::rule(index), WalkMode::Same);
            }
            RowPhase::Body => match frame.body {
                RowBody::Nothing => {
                    return Outcome::Exit(None, Home::Row(Arc::clone(&frame.row)));
                }
                RowBody::Rule(index) => match &ctx.rule_values[index.index()] {
                    Some(value) => {
                        return Outcome::Exit(
                            Some(value.clone()),
                            Home::Row(Arc::clone(&frame.row)),
                        );
                    }
                    None => return Outcome::Spawn(Frame::rule(index), WalkMode::Same),
                },
                RowBody::Leaf(id) => {
                    frame.phase = RowPhase::Await;
                    return Outcome::Spawn(Frame::cell(id), WalkMode::New);
                }
            },
            RowPhase::Await => {
                let result = inbox
                    .take()
                    .expect("BUG: row scope resumed without its field value");
                return Outcome::Exit(Some(result), Home::Row(Arc::clone(&frame.row)));
            }
        }
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
    let template = plan.row_template(&frame.row.identity);
    let Some(raw) = inbox else {
        let rule = match &frame.end {
            RowEnd::Rule(rule) => Some(rule),
            RowEnd::Data(path) => match template.reference_ends.get(path) {
                Some(ReferenceEnd::Rule(rule)) => Some(rule),
                Some(ReferenceEnd::Promptable(_) | ReferenceEnd::Import) | None => None,
            },
        };
        let body = match (rule, &frame.end) {
            (Some(rule), _) => RowBody::Rule(template.rule_index(rule)),
            (None, RowEnd::Data(path)) => RowBody::Leaf(
                *template
                    .data_leaf
                    .get(path)
                    .expect("BUG: row data end has no data_leaf entry in its template"),
            ),
            (None, RowEnd::Rule(_)) => unreachable!("BUG: rule row end always names a rule"),
        };
        frame.rule_root = rule.map(|rule| {
            template
                .rules
                .get(rule)
                .expect("BUG: row rule end missing from its template")
                .normal_form
        });
        return Outcome::Spawn(
            Frame::row_scope(Arc::clone(&frame.row), template, body),
            WalkMode::Same,
        );
    };
    if let Some(root) = frame.rule_root {
        if raw.is_missing_data() {
            let keys = ctx
                .row_contexts
                .get(&frame.row)
                .expect("BUG: row context missing after its read")
                .missing_data_for_rule(template, root);
            for key in keys {
                ctx.missing_row_keys.push(rows::row_key(&frame.row, &key));
            }
        }
    }
    let result = rows::from_row(raw, &frame.row, template, ctx);
    if ctx.exhaustive {
        ctx.row_reads_log.push(RowRead {
            row: Arc::clone(&frame.row),
            end: frame.end.clone(),
            result: result.clone(),
            result_type: row_end_type(template, &frame.end),
        });
    }
    Outcome::Finish(Some(result))
}

/// Instance-list rows of a list declared at `path`: one row per source row, its fields
/// bound from literals or from consumer cells.
fn step_materialize<'p>(
    frame: &mut MaterializeFrame<'p>,
    inbox: Delivered,
    plan: &'p ExecutionPlan,
) -> Outcome<'p> {
    let template = plan.row_template(&frame.source.identity);
    if let Some(value) = inbox {
        let (name, _) = &frame.source.rows[frame.row].fields[frame.field];
        frame
            .bindings
            .insert(name.clone(), rows::checked_row_field(template, name, value));
        frame.field += 1;
    }
    loop {
        let Some(source_row) = frame.source.rows.get(frame.row) else {
            return Outcome::Finish(Some(OperationResult::from_literal(LiteralValue {
                value: ValueKind::List(std::mem::take(&mut frame.rows)),
            })));
        };
        let Some((name, row_source)) = source_row.fields.get(frame.field) else {
            let bindings = std::mem::take(&mut frame.bindings);
            frame.rows.push(crate::planning::semantics::row_literal(
                frame.source.identity.clone(),
                RowOrigin::Numbered {
                    list: frame.path.input_key(),
                    number: frame.row + 1,
                },
                bindings,
                Arc::clone(&frame.row_type),
            ));
            frame.row += 1;
            frame.field = 0;
            continue;
        };
        match row_source {
            DataSource::Literal(bound) => {
                frame
                    .bindings
                    .insert(name.clone(), OperationResult::from_bound(bound.clone()));
                frame.field += 1;
            }
            DataSource::Reference(relative) => {
                let consumer_path = absolute_reference(frame.path, relative);
                let leaf = *plan
                    .data_leaf
                    .get(&consumer_path)
                    .expect("BUG: row field source path has no data_leaf entry");
                return Outcome::Spawn(Frame::cell(leaf), WalkMode::New);
            }
        }
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
                return Outcome::Spawn(Frame::cell(frame.list), WalkMode::New);
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
                            &data.rows,
                            ListFold::Sum,
                            result_type,
                            plan,
                        )));
                    }
                    (None, NormalFormListOp::Product) => {
                        return Outcome::Finish(Some(list_ops::reduce(
                            &data.rows,
                            ListFold::Product,
                            result_type,
                            plan,
                        )));
                    }
                    (None, NormalFormListOp::Avg) => {
                        return Outcome::Finish(Some(list_ops::reduce(
                            &data.rows,
                            ListFold::Avg,
                            result_type,
                            plan,
                        )));
                    }
                    (None, NormalFormListOp::Min) => {
                        return Outcome::Finish(Some(list_ops::extrema(
                            &data.rows,
                            Ordering::Less,
                        )));
                    }
                    (None, NormalFormListOp::Max) => {
                        return Outcome::Finish(Some(list_ops::extrema(
                            &data.rows,
                            Ordering::Greater,
                        )));
                    }
                    (None, NormalFormListOp::Sort { descending }) => {
                        return Outcome::Finish(Some(list_ops::sort_rows(
                            &data.rows,
                            *descending,
                            data.unit.clone(),
                        )));
                    }
                    (None, NormalFormListOp::Find)
                    | (None, NormalFormListOp::Top)
                    | (None, NormalFormListOp::Any(_))
                    | (None, NormalFormListOp::All(_))
                    | (None, NormalFormListOp::Where(_))
                    | (None, NormalFormListOp::With) => {
                        let count = match op {
                            NormalFormListOp::With => frame.args.len(),
                            _ => 1,
                        };
                        frame.state = ListState::Args {
                            data,
                            values: Vec::new(),
                            count,
                            key_fields: None,
                        };
                    }
                    (Some(_), NormalFormListOp::Top) => {
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
                    | (Some(_), NormalFormListOp::AnyRow)
                    | (Some(_), NormalFormListOp::AllRow)
                    | (Some(_), NormalFormListOp::WhereRow) => {
                        frame.state = ListState::PerRow {
                            data,
                            cursor: 0,
                            values: Vec::new(),
                            first_veto: None,
                        };
                    }
                    (None, NormalFormListOp::AnyRow)
                    | (None, NormalFormListOp::AllRow)
                    | (None, NormalFormListOp::WhereRow)
                    | (None, NormalFormListOp::WithRow { .. }) => {
                        unreachable!("BUG: instance-row command {op:?} on a value list")
                    }
                    (Some(_), NormalFormListOp::Any(_))
                    | (Some(_), NormalFormListOp::All(_))
                    | (Some(_), NormalFormListOp::Where(_))
                    | (Some(_), NormalFormListOp::With) => {
                        unreachable!("BUG: value-list command {op:?} on instance rows")
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
                    return Outcome::Spawn(Frame::cell(arg), WalkMode::New);
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
                            let wanted = values
                                .into_iter()
                                .zip(frame.args)
                                .map(|(value, arg)| {
                                    list_ops::typed_operand(value, plan.result_type(*arg))
                                })
                                .collect();
                            frame.state = ListState::Scan(KeyScan {
                                rows: data.rows,
                                fields,
                                row: 0,
                                field: 0,
                                key: Vec::new(),
                                purpose: ScanPurpose::Find { wanted },
                            });
                        }
                    },
                    (None, NormalFormListOp::Any(condition))
                    | (None, NormalFormListOp::All(condition))
                    | (None, NormalFormListOp::Where(condition)) => {
                        let right_type = plan.result_type(
                            *frame
                                .args
                                .first()
                                .expect("BUG: list condition has a right cell"),
                        );
                        let flags = match list_ops::condition_flags(
                            &data.rows,
                            condition,
                            &first_operand(&values),
                            right_type,
                        ) {
                            Ok(flags) => flags,
                            Err(veto) => return Outcome::Finish(Some(veto)),
                        };
                        return Outcome::Finish(Some(match op {
                            NormalFormListOp::Where(_) => {
                                list_ops::filter_rows(&data.rows, flags, data.unit.clone())
                            }
                            NormalFormListOp::All(_) => list_ops::predicate_result(&flags, false),
                            _ => list_ops::predicate_result(&flags, true),
                        }));
                    }
                    (None, NormalFormListOp::With) => {
                        return Outcome::Finish(Some(list_ops::append_values(
                            &data.rows,
                            values,
                            frame.args,
                            plan,
                            data.unit.clone(),
                        )));
                    }
                    (
                        Some(_),
                        NormalFormListOp::WithRow {
                            fields: append_fields,
                        },
                    ) => {
                        let appended = list_ops::appended_instance_row(
                            plan,
                            plan.result_type(frame.list),
                            append_fields,
                            values,
                            data.rows.len(),
                        );
                        let mut rows = data.rows;
                        rows.push(appended);
                        match key_fields {
                            None => {
                                return Outcome::Finish(Some(list_ops::list_result(
                                    rows, data.unit,
                                )));
                            }
                            Some(fields) => {
                                frame.state = ListState::Scan(KeyScan {
                                    rows,
                                    fields,
                                    row: 0,
                                    field: 0,
                                    key: Vec::new(),
                                    purpose: ScanPurpose::Append {
                                        keyed: Vec::new(),
                                        unit: data.unit,
                                    },
                                });
                            }
                        }
                    }
                    (_, other) => unreachable!("BUG: {other:?} does not take operands here"),
                }
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
                let argument = *frame.args.first().unwrap_or_else(|| {
                    panic!("BUG: {op:?} over instance rows has no row argument")
                });
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
                        "BUG: instance-list command entered at the wrong row depth"
                    );
                    ctx.row_stack
                        .push(Arc::clone(list_ops::row_instance(&data.rows[cursor])));
                    frame.state = ListState::PerRow {
                        data,
                        cursor,
                        values,
                        first_veto,
                    };
                    return Outcome::Spawn(Frame::cell(argument), WalkMode::New);
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
                            let (_, field_type) = list_ops::key_field_type(
                                plan,
                                list_ops::row_instance(&scan.rows[scan.row]),
                                &scan.fields[scan.field],
                            );
                            scan.key.push(typed_from_bound(&bound, &field_type));
                        }
                        veto => return Outcome::Finish(Some(veto)),
                    }
                    scan.field += 1;
                }
                loop {
                    if scan.row == scan.rows.len() {
                        return Outcome::Finish(Some(match scan.purpose {
                            ScanPurpose::Find { wanted } => {
                                list_ops::no_row_with_key(&scan.fields, &wanted)
                            }
                            ScanPurpose::Append { keyed, unit } => {
                                match list_ops::key_conflict_result(&scan.fields, &keyed) {
                                    Some(conflict) => conflict,
                                    None => list_ops::list_result(scan.rows, unit),
                                }
                            }
                        }));
                    }
                    if scan.field == scan.fields.len() {
                        let key = std::mem::take(&mut scan.key);
                        match &mut scan.purpose {
                            ScanPurpose::Find { wanted } => {
                                match list_ops::keys_match(&key, wanted) {
                                    Ok(true) => {
                                        return Outcome::Finish(Some(
                                            OperationResult::from_literal(
                                                scan.rows[scan.row].to_literal(),
                                            ),
                                        ));
                                    }
                                    Ok(false) => {}
                                    Err(veto) => return Outcome::Finish(Some(veto)),
                                }
                            }
                            ScanPurpose::Append { keyed, .. } => keyed.push(KeyedRow {
                                label: format!("row {}", keyed.len() + 1),
                                values: key,
                            }),
                        }
                        scan.row += 1;
                        scan.field = 0;
                        continue;
                    }
                    let row = Arc::clone(list_ops::row_instance(&scan.rows[scan.row]));
                    let (end, _) = list_ops::key_field_type(plan, &row, &scan.fields[scan.field]);
                    frame.state = ListState::Scan(scan);
                    return Outcome::Spawn(Frame::read_row(row, end), WalkMode::Same);
                }
            }
        }
    }
}

fn first_operand(values: &[OperationResult]) -> OperationResult {
    values
        .first()
        .cloned()
        .expect("BUG: command operand collected before use")
}

/// Evaluate a cell of the walk. Returns the step outcome: a value, or a child to wait for.
fn step_cell<'p>(
    cell: &mut CellFrame<'p>,
    mut inbox: Delivered,
    walk: &WalkDone,
    plan: &'p ExecutionPlan,
    ctx: &mut EvaluationContext,
) -> Outcome<'p> {
    let id = cell.id;
    if let Some(resume) = cell.resume.take() {
        let value = inbox.take().expect("BUG: resumed cell has no child value");
        return match resume {
            Resume::Plain => cell_done(cell, walk, value, plan, ctx),
            Resume::RowLog(mark) => {
                ctx.record_row_log(id, mark);
                cell_done(cell, walk, value, plan, ctx)
            }
            Resume::Import(path) => {
                if !value.is_missing_data() {
                    let leaf = *plan.data_leaf.get(path).unwrap_or_else(|| {
                        panic!(
                            "BUG: import member '{}' has no data_leaf entry after ensure_data_leaves",
                            path.input_key()
                        )
                    });
                    ctx.values[leaf.index()] = Some(value.clone());
                }
                cell_done(cell, walk, value, plan, ctx)
            }
        };
    }
    if let Some(settled) = cell.settled.take() {
        return Outcome::Finish(Some(finish(id, settled, plan, ctx)));
    }

    macro_rules! need {
        ($child:expr) => {{
            let child: NormalFormId = $child;
            match ready(child, ctx, walk) {
                Some(value) => value,
                None => return Outcome::Spawn(Frame::cell(child), WalkMode::Same),
            }
        }};
    }
    macro_rules! settle {
        ($outcome:expr) => {
            match $outcome {
                Ok(value) => return cell_done(cell, walk, value, plan, ctx),
                Err(child) => return Outcome::Spawn(Frame::cell(child), WalkMode::Same),
            }
        };
    }
    macro_rules! done {
        ($value:expr) => {
            return cell_done(cell, walk, $value, plan, ctx)
        };
    }

    let cell_def = plan.normal_form(id);
    if let Some(path) = &cell_def.rule_ref {
        let index = plan.rule_index(path);
        return match &ctx.rule_values[index.index()] {
            None => Outcome::Spawn(Frame::rule(index), WalkMode::Same),
            Some(result) => {
                let result = result.clone();
                ctx.values[id.index()] = Some(result.clone());
                Outcome::Finish(Some(result))
            }
        };
    }
    if let NormalFormKind::Leaf(LeafKind::ImportRule(path)) = &cell_def.kind {
        if plan.rules.contains_key(path) {
            let index = plan.rule_index(path);
            if plan.rule_at(index).normal_form != id {
                return match &ctx.rule_values[index.index()] {
                    None => Outcome::Spawn(Frame::rule(index), WalkMode::Same),
                    Some(result) => {
                        let result = result.clone();
                        ctx.values[id.index()] = Some(result.clone());
                        Outcome::Finish(Some(result))
                    }
                };
            }
        }
    }

    match &cell_def.kind {
        NormalFormKind::Leaf(LeafKind::Literal(literal)) => {
            done!(OperationResult::from_literal(literal.clone()))
        }
        NormalFormKind::Leaf(LeafKind::DataPath(path)) => {
            if let Some(slot) = ctx.data_slot(plan, path) {
                done!(slot.clone())
            }
            if !path.segments.is_empty() {
                cell.resume = Some(Resume::Import(path));
                return Outcome::Spawn(Frame::import_member(path), WalkMode::Same);
            }
            let list_path = match plan.reference_ends.get(path) {
                Some(ReferenceEnd::Promptable(target)) => target,
                Some(ReferenceEnd::Rule(_) | ReferenceEnd::Import) | None => path,
            };
            if let Some(source) = plan.instance_lists.get(list_path) {
                if list_path == path {
                    cell.resume = Some(Resume::Plain);
                    return Outcome::Spawn(Frame::materialize(path, source), WalkMode::Same);
                }
                let leaf = *plan
                    .data_leaf
                    .get(list_path)
                    .expect("BUG: instance list has no data_leaf entry");
                cell.resume = Some(Resume::Plain);
                return Outcome::Spawn(Frame::cell(leaf), WalkMode::New);
            }
            done!(resolve_data_path_value(path, plan, ctx))
        }
        NormalFormKind::Leaf(LeafKind::ImportRule(path)) => {
            cell.resume = Some(Resume::Plain);
            Outcome::Spawn(Frame::import_rule(path), WalkMode::Same)
        }
        NormalFormKind::Leaf(LeafKind::Row { level, end, .. }) => {
            let level = usize::try_from(*level).expect("BUG: row level fits usize");
            let row = Arc::clone(
                ctx.row_stack
                    .get(level)
                    .expect("BUG: row leaf evaluated outside its list command"),
            );
            cell.resume = Some(Resume::Plain);
            Outcome::Spawn(Frame::read_row(row, end.clone()), WalkMode::Same)
        }
        NormalFormKind::Now => done!(OperationResult::from_literal(ctx.now().clone())),
        NormalFormKind::SpecMember {
            base, targets, row, ..
        } => {
            let base_result = need!(*base);
            if base_result.vetoed() {
                done!(base_result)
            }
            let value = match &borrow_value(&base_result, "spec member").value {
                ValueKind::Spec(value) => value.clone(),
                other => panic!("BUG: spec member base was {other}, not a spec value"),
            };
            match value {
                SpecValue::Fixed(instance) => {
                    let Some((_, target)) =
                        targets.iter().find(|(candidate, _)| candidate == &instance)
                    else {
                        let candidates: Vec<&SpecInstance> =
                            targets.iter().map(|(candidate, _)| candidate).collect();
                        unreachable!(
                            "BUG: spec member instance {instance:?} is not among {candidates:?}"
                        );
                    };
                    let value = need!(*target);
                    done!(value)
                }
                SpecValue::Row(instance) => {
                    let read = row
                        .as_ref()
                        .expect("BUG: spec member on a row value has no row read");
                    let mark = ctx.row_log_mark();
                    cell.resume = Some(Resume::RowLog(mark));
                    Outcome::Spawn(Frame::read_row(instance, read.end.clone()), WalkMode::Same)
                }
            }
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
            Outcome::Spawn(
                Frame::list_op(*list, op, args, *row_level, plan.result_type(id)),
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
            let zero = OperationResult::from_literal(LiteralValue::number(
                crate::computation::rational::rational_zero(),
            ));
            done!(binary_arithmetic_result(
                &zero,
                crate::planning::semantics::primitive_number_arc(),
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
            let one = OperationResult::from_literal(LiteralValue::number(
                crate::computation::rational::rational_one(),
            ));
            done!(binary_arithmetic_result(
                &one,
                crate::planning::semantics::primitive_number_arc(),
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
        NormalFormKind::And(children) => settle!(and_step(cell, children, ctx, walk)),
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
                crate::planning::semantics::primitive_boolean_arc(),
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
            done!(crate::computation::datetime::compute_date_relative(
                kind,
                date,
                now_date(ctx),
            ))
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
            done!(crate::computation::datetime::compute_date_calendar(
                kind,
                unit,
                date,
                now_date(ctx),
            ))
        }
        NormalFormKind::RangeLiteral(left_id, right_id) => settle!(binary_settle(
            *left_id,
            *right_id,
            ctx,
            walk,
            |left_result, right_result| {
                let left_bound = borrow_value(left_result, "left endpoint");
                let right_bound = borrow_value(right_result, "right endpoint");
                let left_typed = crate::planning::semantics::typed_literal_from_bound(
                    left_bound,
                    plan.result_type(*left_id),
                );
                let right_typed = crate::planning::semantics::typed_literal_from_bound(
                    right_bound,
                    plan.result_type(*right_id),
                );
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
                    match crate::computation::range::ordered_endpoints(
                        &left.value,
                        &right.value,
                        &endpoint_type,
                    ) {
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
            done!(crate::computation::datetime::evaluate_past_future_range(
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
                        crate::computation::range::check_containment(
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
    cell: &mut CellFrame<'p>,
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
            return Outcome::Spawn(Frame::cell(origin), WalkMode::Same);
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
    cell: &mut CellFrame<'_>,
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

/// Operands left to right, order-independent in value. Any `false` operand decides
/// `false`, whatever its position and whatever the other operands are. Otherwise the
/// first missing or vetoed operand in source order propagates; the remaining operands
/// are still visited so a later `false` can decide. All true is `true`.
fn and_step(
    cell: &mut CellFrame<'_>,
    children: &[NormalFormId],
    ctx: &EvaluationContext,
    walk: &WalkDone,
) -> Result<OperationResult, NormalFormId> {
    assert!(children.len() >= 2, "BUG: And has at least two operands");
    loop {
        let child = children[cell.cursor];
        let result = ready(child, ctx, walk).ok_or(child)?;
        match condition_outcome(&result) {
            BranchOutcome::NotTaken => {
                return Ok(OperationResult::from_literal(LiteralValue::from_bool(
                    false,
                )));
            }
            BranchOutcome::Propagate(propagated) => {
                cell.veto.get_or_insert(propagated);
            }
            BranchOutcome::Taken => {}
        }
        cell.cursor += 1;
        if cell.cursor == children.len() {
            return Ok(cell
                .veto
                .take()
                .unwrap_or_else(|| OperationResult::from_literal(LiteralValue::from_bool(true))));
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

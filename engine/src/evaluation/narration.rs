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
use crate::evaluation::tree::borrow_value;
use crate::evaluation::EvaluationContext;
use crate::formatting::expression::display_comparison;
use crate::parsing::ast::{ArithmeticComputation, RowCondition};
use crate::planning::execution_plan::{
    resolve_nested_dispatch_body, ExecutableRule, ExecutionPlan,
};
use crate::planning::explanation::{Cause, ExplainedRule, ExplainedRuleId, ExplanationNode};
use crate::planning::instance_list::{row_end, row_end_type};
use crate::planning::normalize::{
    explanation_display, LeafKind, NfNode, NormalFormId, NormalFormKind, NormalFormListOp,
};
use crate::planning::ordered_dispatch::{is_exclusive_point_table, region_of_scrutinee};
use crate::planning::semantics::{
    negated_comparison, DataPath, LemmaType, ListKey, ReferenceEnd, RowEnd, RowInstance, RowOrigin,
    RulePath, SpecValue, ValueKind,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Internal id of a narrated rule in an [`ExplanationArena`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ArenaId(usize);

/// One narrated rule body before public id remapping.
struct ArenaEntry {
    name: RulePath,
    result: crate::result_value::RuleResultValue,
    body: String,
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
/// `rule_explanations`. Rules narrate in plan order (topological), so every rule
/// reference under this rule already has its arena entry.
pub(crate) fn narrate_rule(
    rule: &ExecutableRule,
    plan: &ExecutionPlan,
    ctx: &mut EvaluationContext,
    arena: &mut ExplanationArena,
) {
    let result_type = ctx.rule_result_type(plan, rule);
    let result = explanation_result_value(
        ctx.rule_value(plan, &rule.path),
        result_type.as_ref(),
        &plan.family_units,
    );
    // Import ExecutableRule: its body is the peel cut naming itself.
    if let NormalFormKind::Leaf(LeafKind::ImportRule(path)) =
        &plan.normal_form(rule.normal_form).kind
    {
        if *path == rule.path {
            let id = arena.push(ArenaEntry {
                name: rule.path.clone(),
                result,
                body: rule.path.input_key(),
                causes: Vec::new(),
                children: Vec::new(),
            });
            ctx.rule_explanations.insert(rule.path.clone(), id);
            return;
        }
    }
    let narrated = narrate(rule.normal_form, plan, ctx);
    let id = arena.push(ArenaEntry {
        name: rule.path.clone(),
        result,
        body: narrated.body,
        causes: narrated.causes,
        children: operands_of(&narrated.as_operand),
    });
    ctx.rule_explanations.insert(rule.path.clone(), id);
}

/// Narrate every row context bottom-up so nested rows exist before their parents.
pub(crate) fn narrate_row_contexts(
    plan: &ExecutionPlan,
    ctx: &mut EvaluationContext,
    arena: &mut ExplanationArena,
) {
    let rows: Vec<Arc<RowInstance>> = ctx.row_contexts.keys().cloned().collect();
    for row in rows {
        let mut row_ctx = ctx
            .row_contexts
            .remove(&row)
            .expect("BUG: row context vanished while narrating");
        narrate_row_tree(plan.row_template(&row.identity), &mut row_ctx, arena);
        ctx.row_contexts.insert(row, row_ctx);
    }
}

/// Row context at `path` below `template` / `ctx`, with the template that plans it.
fn row_context_at<'a>(
    mut template: &'a ExecutionPlan,
    mut ctx: &'a mut EvaluationContext,
    path: &[Arc<RowInstance>],
) -> (&'a ExecutionPlan, &'a mut EvaluationContext) {
    for row in path {
        template = template.row_template(&row.identity);
        ctx = ctx
            .row_contexts
            .get_mut(row)
            .expect("BUG: nested row context missing while narrating");
    }
    (template, ctx)
}

/// Narrate every row context below `row_ctx` (children before parents), then its own rules.
fn narrate_row_tree(
    template: &ExecutionPlan,
    row_ctx: &mut EvaluationContext,
    arena: &mut ExplanationArena,
) {
    let mut post_order: Vec<Vec<Arc<RowInstance>>> = Vec::new();
    let mut pending: Vec<(Vec<Arc<RowInstance>>, bool)> = vec![(Vec::new(), false)];
    while let Some((path, expanded)) = pending.pop() {
        if expanded {
            post_order.push(path);
            continue;
        }
        let (_, node_ctx) = row_context_at(template, row_ctx, &path);
        let children: Vec<Arc<RowInstance>> = node_ctx.row_contexts.keys().cloned().collect();
        pending.push((path.clone(), true));
        for child in children {
            let mut child_path = path.clone();
            child_path.push(child);
            pending.push((child_path, false));
        }
    }
    for path in post_order {
        let (node_template, node_ctx) = row_context_at(template, row_ctx, &path);
        for (index, rule) in node_template.rules.values().enumerate() {
            if node_ctx.rule_values[index].is_some() {
                narrate_rule(rule, node_template, node_ctx, arena);
            }
        }
    }
}

/// Root [`Explanation`] for a requested rule: remap reachable arena entries to
/// compact public ids in first-reach order.
pub(crate) fn explanation_for(
    rule: &ExecutableRule,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
    arena: &ExplanationArena,
    rule_value: &OperationResult,
    rule_result_type: Arc<LemmaType>,
) -> Explanation {
    let root_id = *ctx.rule_explanations.get(&rule.path).unwrap_or_else(|| {
        panic!(
            "BUG: rule '{}' narrated after its use-site (plan order is topological)",
            rule.path.rule
        )
    });
    let root = arena.get(root_id);
    let (children, causes, rules) = remap_reachable(arena, &root.children, &root.causes);
    Explanation {
        name: rule.path.clone(),
        result: rule_value.clone(),
        result_type: rule_result_type,
        family_units: Arc::clone(&plan.family_units),
        body: root.body.clone(),
        causes,
        children,
        rules,
    }
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
            body: entry.body.clone(),
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
            | ExplanationNode::Conversion { operands, .. } => {
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
            | ExplanationNode::Conversion { operands, .. } => pending.extend(operands.iter_mut()),
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

fn rule_ref_node(
    path: &RulePath,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
) -> ExplanationNode {
    let arena_id = *ctx.rule_explanations.get(path).unwrap_or_else(|| {
        panic!(
            "BUG: rule '{}' narrated after its use-site (plan order is topological)",
            path.rule
        )
    });
    let rule = plan
        .rules
        .get(path)
        .unwrap_or_else(|| panic!("BUG: narrated rule '{path}' missing from plan"));
    let result_type = ctx.rule_result_type(plan, rule);
    ExplanationNode::RuleRef {
        id: ExplainedRuleId(u32::try_from(arena_id.0).expect("BUG: arena id exceeds u32")),
        name: path.clone(),
        result: explanation_result_value(
            ctx.rule_value(plan, path),
            result_type.as_ref(),
            &plan.family_units,
        ),
    }
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
fn narrate(root: NormalFormId, plan: &ExecutionPlan, ctx: &EvaluationContext) -> Narrated {
    let mut done: Done = HashMap::new();
    let mut stack = vec![root];
    while let Some(&top) = stack.last() {
        if done.contains_key(&top) {
            stack.pop();
            continue;
        }
        match narrate_step(top, plan, ctx, &done) {
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
) -> Need<Narrated> {
    let cell = plan.normal_form(id);
    if let Some(path) = &cell.rule_ref {
        return Ok(Narrated {
            body: path.input_key(),
            causes: Vec::new(),
            as_operand: Some(rule_ref_node(path, plan, ctx)),
        });
    }
    if let Some(origin) = cell.origin {
        if let NormalFormKind::OrderedDispatch {
            scrutinee, regions, ..
        } = &cell.kind
        {
            assert_dispatch_agrees_with_record(id, plan, ctx, done)?;
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
    narrate_shape(id, plan, ctx, done)
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
                }),
                Some(ExplanationNode::Compose { operands, .. }) if operands.is_empty() => {
                    Some(scrutinee_node)
                }
                Some(other) => Some(ExplanationNode::Compose {
                    expression: narrated.body.clone(),
                    operator: None,
                    operands: vec![scrutinee_node, other],
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
                }),
            }
        }
        NormalFormKind::Leaf(LeafKind::DataPath(path)) => {
            let node = data_node(path, plan, ctx);
            Narrated {
                body: path.input_key(),
                causes: Vec::new(),
                as_operand: Some(node),
            }
        }
        NormalFormKind::Leaf(LeafKind::ImportRule(path)) => Narrated {
            body: path.input_key(),
            causes: Vec::new(),
            as_operand: Some(rule_ref_node(path, plan, ctx)),
        },
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
            compose(id, &operands, plan, done)?
        }
        NormalFormKind::And(children) => {
            let mut operands = Vec::with_capacity(children.len());
            for &child in children {
                operands.push(child);
                if matches!(
                    condition_outcome(&slot(child, plan, ctx)),
                    BranchOutcome::NotTaken
                ) {
                    break;
                }
            }
            compose(id, &operands, plan, done)?
        }
        NormalFormKind::Subtract(a, b)
        | NormalFormKind::Divide(a, b)
        | NormalFormKind::Power(a, b)
        | NormalFormKind::Modulo(a, b)
        | NormalFormKind::Comparison(a, _, b)
        | NormalFormKind::RangeLiteral(a, b)
        | NormalFormKind::RangeContainment(a, b) => compose(id, &[*a, *b], plan, done)?,
        NormalFormKind::Negate(x)
        | NormalFormKind::Reciprocal(x)
        | NormalFormKind::Not(x)
        | NormalFormKind::MathOp(_, x)
        | NormalFormKind::DateRelative(_, x)
        | NormalFormKind::DateCalendar(_, _, x)
        | NormalFormKind::PastFutureRange(_, x)
        | NormalFormKind::RangeBound(_, x)
        | NormalFormKind::ResultIsVeto(x) => compose(id, &[*x], plan, done)?,
        NormalFormKind::UnitConversion(inner, target) => {
            let expression = explanation_display(forms, id);
            let inner_operand = child(done, *inner)?.as_operand.clone();
            let source = slot(*inner, plan, ctx);
            if source.vetoed() {
                return compose(id, &[*inner], plan, done);
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
                    let steps = build_conversion_steps(
                        &borrow_value(&source, "conversion source").to_literal(),
                        plan.result_type(*inner),
                        target,
                        &result_literal.to_literal(),
                        plan.result_type(id),
                    );
                    ExplanationNode::Conversion {
                        expression: expression.clone(),
                        steps,
                        operands: inner_operand.into_iter().collect(),
                    }
                }
            };
            Narrated {
                body: expression,
                causes: Vec::new(),
                as_operand: Some(node),
            }
        }
        NormalFormKind::Piecewise(arms) => match piecewise_causes(arms, plan, ctx, done)? {
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
        },
        NormalFormKind::OrderedDispatch { .. } => {
            unreachable!("BUG: OrderedDispatch always carries its Piecewise pre-image as origin")
        }
        NormalFormKind::SpecMember {
            base,
            member,
            targets,
            ..
        } => {
            let base_result = slot(*base, plan, ctx);
            if base_result.vetoed() {
                return Ok(child(done, *base)?.clone());
            }
            let instance = match &borrow_value(&base_result, "spec member").value {
                ValueKind::Spec(SpecValue::Fixed(instance)) => instance.clone(),
                ValueKind::Spec(SpecValue::Row(_)) => {
                    let mut operands: Vec<ExplanationNode> =
                        child(done, *base)?.as_operand.clone().into_iter().collect();
                    operands.extend(row_read_nodes(id, None, plan, ctx));
                    drop_list_data_when_rows(&mut operands);
                    let expression = explanation_display(forms, id);
                    return Ok(Narrated {
                        body: expression.clone(),
                        causes: Vec::new(),
                        as_operand: Some(ExplanationNode::Compose {
                            expression,
                            operator: None,
                            operands,
                        }),
                    });
                }
                other => panic!("BUG: spec member base was {other}, not a spec value"),
            };
            let (_, target) = targets
                .iter()
                .find(|(candidate, _)| candidate == &instance)
                .unwrap_or_else(|| {
                    panic!("BUG: spec member instance {instance} is not a candidate")
                });
            let mut narrated = child(done, *target)?.clone();
            narrated.body = format!("{instance}.{member}");
            narrated
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
        )?,
        NormalFormKind::Leaf(LeafKind::Row { .. }) => {
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
        let (base, inner) = peel_where_chain(list, plan, ctx);
        let mut filters = inner;
        filters.push(filter_info(id, list, op, args, plan, ctx));
        (base, filters)
    } else {
        peel_where_chain(list, plan, ctx)
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

    let expression = explanation_display(forms, id);
    let key_fields = match &plan.result_type(list).specifications {
        crate::planning::semantics::TypeSpecification::List {
            key: ListKey::Fields(fields),
            ..
        } => Some(fields.as_slice()),
        _ => None,
    };
    // End-of-chain `where` hides predicate row reads; reductions keep their field reads.
    let rows = if this_is_where {
        Vec::new()
    } else if row_level.is_some() {
        row_read_nodes(id, key_fields, plan, ctx)
    } else {
        Vec::new()
    };

    let mut operands = Vec::new();
    if let Some(base_node) = base_operand {
        let drop_data = matches!(base_node, ExplanationNode::Data { .. }) && !rows.is_empty();
        if !drop_data {
            operands.push(base_node);
        }
    }
    operands.extend(arg_operands);
    operands.extend(wrap_filters(filters, rows));

    Ok(Narrated {
        body: expression.clone(),
        causes: Vec::new(),
        as_operand: Some(ExplanationNode::Compose {
            expression,
            operator: None,
            operands,
        }),
    })
}

fn is_where_op(op: &NormalFormListOp) -> bool {
    matches!(op, NormalFormListOp::Where(_) | NormalFormListOp::WhereRow)
}

struct FilterInfo {
    condition: String,
    matched: usize,
    total: usize,
}

fn peel_where_chain(
    mut id: NormalFormId,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
) -> (NormalFormId, Vec<FilterInfo>) {
    let mut filters = Vec::new();
    loop {
        let cell = plan.normal_form(id);
        let NormalFormKind::ListCommand { list, op, args, .. } = &cell.kind else {
            return (id, filters);
        };
        if !is_where_op(op) {
            return (id, filters);
        }
        if slot(id, plan, ctx).vetoed() {
            return (id, filters);
        }
        filters.push(filter_info(id, *list, op, args, plan, ctx));
        id = *list;
    }
}

fn filter_info(
    where_id: NormalFormId,
    input_list: NormalFormId,
    op: &NormalFormListOp,
    args: &[NormalFormId],
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
) -> FilterInfo {
    let forms = plan.normal_forms.as_slice();
    let condition = match op {
        NormalFormListOp::WhereRow => {
            let arg = *args
                .first()
                .expect("BUG: where row command has no condition argument");
            explanation_display(forms, arg)
        }
        NormalFormListOp::Where(cmp) => {
            let arg = *args
                .first()
                .expect("BUG: where value command has no right argument");
            match cmp {
                RowCondition::Compare(op) => format!("{op} {}", explanation_display(forms, arg)),
                RowCondition::In => format!("in {}", explanation_display(forms, arg)),
            }
        }
        other => panic!("BUG: filter_info on non-where op {other:?}"),
    };
    FilterInfo {
        condition,
        matched: list_len(&slot(where_id, plan, ctx)),
        total: list_len(&slot(input_list, plan, ctx)),
    }
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
        nodes = vec![ExplanationNode::Filter {
            condition: filter.condition,
            matched: filter.matched,
            total: filter.total,
            children: nodes,
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

/// One [`ExplanationNode::Row`] per row the cell read, in read order.
fn row_read_nodes(
    id: NormalFormId,
    key: Option<&[String]>,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
) -> Vec<ExplanationNode> {
    let Some(reads) = ctx.row_reads.get(&id) else {
        return Vec::new();
    };
    let mut nodes: Vec<(Arc<RowInstance>, Vec<ExplanationNode>)> = Vec::new();
    for read in reads {
        let template = plan.row_template(&read.row.identity);
        let field = row_field_node(read, template, plan, ctx);
        match nodes.iter_mut().find(|(row, _)| *row == read.row) {
            Some((_, fields)) => fields.push(field),
            None => nodes.push((Arc::clone(&read.row), vec![field])),
        }
    }
    nodes
        .into_iter()
        .map(|(row, fields)| {
            let (number, appended) = match &row.origin {
                RowOrigin::Numbered { number, .. } => (*number, false),
                RowOrigin::Appended { number } => (*number, true),
            };
            ExplanationNode::Row {
                number,
                appended,
                key: row_key_nodes(&row, key, plan, ctx),
                fields,
            }
        })
        .collect()
}

fn row_field_node(
    read: &crate::evaluation::rows::RowRead,
    template: &ExecutionPlan,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
) -> ExplanationNode {
    let rule_path = match &read.end {
        RowEnd::Rule(rule) => Some(rule.clone()),
        RowEnd::Data(path) => match template.reference_ends.get(path) {
            Some(ReferenceEnd::Rule(rule)) => Some(rule.clone()),
            Some(ReferenceEnd::Promptable(_) | ReferenceEnd::Import) | None => None,
        },
    };
    if let Some(rule_path) = rule_path {
        let row_ctx = ctx
            .row_contexts
            .get(&read.row)
            .expect("BUG: narrated row was never read");
        return rule_ref_node(&rule_path, template, row_ctx);
    }
    let local_name = match &read.end {
        RowEnd::Data(path) => path.input_key(),
        RowEnd::Rule(rule) => rule.input_key(),
    };
    ExplanationNode::Data {
        name: DataPath::local(local_name),
        result: explanation_result_value(
            &read.result,
            read.result_type.as_ref(),
            &plan.family_units,
        ),
    }
}

fn row_key_nodes(
    row: &RowInstance,
    key: Option<&[String]>,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
) -> Vec<ExplanationNode> {
    let Some(fields) = key else {
        return Vec::new();
    };
    let template = plan.row_template(&row.identity);
    let row_ctx = ctx
        .row_contexts
        .get(row)
        .expect("BUG: narrated row was never read");
    fields
        .iter()
        .map(|field| {
            let path = match row_end(template, field) {
                Some(RowEnd::Data(path)) => path,
                other => panic!("BUG: key field `{field}` is not row data: {other:?}"),
            };
            let field_type = row_end_type(template, &RowEnd::Data(path.clone()));
            let result = match row_ctx.data_slot(template, &path) {
                Some(value) => {
                    explanation_result_value(value, field_type.as_ref(), &plan.family_units)
                }
                None => panic!("BUG: key field `{field}` of a narrated row is unbound"),
            };
            ExplanationNode::Data {
                name: DataPath::local(field.clone()),
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
    done: &Done,
) -> Need<Narrated> {
    let mut operand_nodes = Vec::with_capacity(operands.len());
    for operand in operands {
        if let Some(node) = child(done, *operand)?.as_operand.clone() {
            operand_nodes.push(node);
        }
    }
    let expression = explanation_display(plan.normal_forms.as_slice(), id);
    let operator = arithmetic_operator_of(&plan.normal_form(id).kind);
    Ok(Narrated {
        body: expression.clone(),
        causes: Vec::new(),
        as_operand: Some(ExplanationNode::Compose {
            expression,
            operator,
            operands: operand_nodes,
        }),
    })
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

fn data_node(path: &DataPath, plan: &ExecutionPlan, ctx: &EvaluationContext) -> ExplanationNode {
    let result = resolve_data_path_value(path, plan, ctx);
    let data_type = EvaluationContext::data_display_type(plan, path, &result);
    ExplanationNode::Data {
        name: path.clone(),
        result: explanation_result_value(&result, data_type.as_ref(), &plan.family_units),
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
                body: explanation_display(plan.normal_forms.as_slice(), condition),
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
            BranchOutcome::NotTaken => causes.push(cause(*condition, false, plan, ctx, done)?),
            BranchOutcome::Taken | BranchOutcome::Propagate(_) => {}
        }
    }
    let winner_body = match winner {
        Some(arm) => {
            causes.push(cause(arms[arm].0, true, plan, ctx, done)?);
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
    let PiecewiseNarration::Causes { winner_body, .. } = piecewise_causes(arms, plan, ctx, done)?
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
) -> Need<Cause> {
    let mut focus = peel_bool_leaf_origins(condition, plan);
    if !held {
        while let NormalFormKind::And(children) = &plan.normal_form(focus).kind {
            let deciding = deciding_false_and_conjunct(children, plan, ctx);
            focus = peel_bool_leaf_origins(deciding, plan);
        }
    }
    let (condition_text, value) = condition_statement(focus, held, plan);
    let children = if matches!(plan.normal_form(focus).kind, NormalFormKind::And(..)) {
        and_cause_children(focus, plan, ctx)
    } else {
        cause_children(child(done, focus)?.as_operand.clone())
    };
    Ok(Cause {
        condition: condition_text,
        value,
        children,
    })
}

/// The false conjunct that decides a false `and`: the first operand in source order
/// whose outcome is `NotTaken`. Operands before it were walked, so their outcomes are
/// known; a missing or vetoed operand never decides it.
fn deciding_false_and_conjunct(
    children: &[NormalFormId],
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
) -> NormalFormId {
    children
        .iter()
        .copied()
        .find(|&child| {
            matches!(
                try_condition_outcome(child, plan, ctx),
                Some(BranchOutcome::NotTaken)
            )
        })
        .unwrap_or_else(|| panic!("BUG: false And without a NotTaken conjunct"))
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
            | NormalFormKind::And(..) => return origin,
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
                display_comparison(NfNode { forms, id: *a }, &op, NfNode { forms, id: *b }),
                "true".to_string(),
            )
        }
        NormalFormKind::Not(_) => unreachable!("BUG: Not peeled before the condition match"),
        NormalFormKind::Leaf(LeafKind::DataPath(path)) => (
            if held {
                path.input_key()
            } else {
                format!("{} is false", path.input_key())
            },
            "true".to_string(),
        ),
        NormalFormKind::Leaf(LeafKind::Literal(literal))
            if matches!(literal.value, ValueKind::Boolean(_)) =>
        {
            (
                explanation_display(forms, condition),
                if held { "true" } else { "false" }.to_string(),
            )
        }
        NormalFormKind::And(..)
        | NormalFormKind::RangeContainment(..)
        | NormalFormKind::DateRelative(..)
        | NormalFormKind::DateCalendar(..)
        | NormalFormKind::ResultIsVeto(_)
        | NormalFormKind::UnitConversion(..)
        | NormalFormKind::Piecewise(_)
        | NormalFormKind::OrderedDispatch { .. }
        | NormalFormKind::SpecMember { .. }
        | NormalFormKind::ListCommand { .. }
        | NormalFormKind::Leaf(LeafKind::ImportRule(_)) => (
            explanation_display(forms, condition),
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
        | NormalFormKind::Now
        | NormalFormKind::Veto(_)
        | NormalFormKind::RangeLiteral(..)
        | NormalFormKind::PastFutureRange(..)
        | NormalFormKind::RangeBound(..)
        | NormalFormKind::Leaf(LeafKind::Literal(_)) => {
            unreachable!("BUG: non-boolean condition in condition_statement")
        }
        NormalFormKind::Leaf(LeafKind::Row { .. }) => {
            unreachable!("BUG: row leaf narrated outside its list command")
        }
    }
}

/// Operands of a condition worth listing under its cause line.
fn cause_children(operand: Option<ExplanationNode>) -> Vec<ExplanationNode> {
    match operand {
        // Bare data ref is fully stated in the condition text.
        Some(ExplanationNode::Data { .. } | ExplanationNode::DataUnused { .. }) => Vec::new(),
        Some(ExplanationNode::Compose { operands, .. }) => operands
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

/// `and` conditions list every data path under them, in structural order:
/// bound paths as `Data`, unbound ones as `DataUnused`.
fn and_cause_children(
    focus: NormalFormId,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
) -> Vec<ExplanationNode> {
    let mut children = Vec::new();
    let mut seen = HashSet::new();
    for path in structural_data_paths_from(focus, plan) {
        if !seen.insert(path.clone()) {
            continue;
        }
        if ctx.data_slot(plan, &path).is_some() {
            children.push(data_node(&path, plan, ctx));
        } else {
            children.push(ExplanationNode::DataUnused { name: path });
        }
    }
    children
}

/// DataPath leaves under a cell, following children and origins (structural
/// mentions, whether or not the walk looked them up).
fn structural_data_paths_from(id: NormalFormId, plan: &ExecutionPlan) -> Vec<DataPath> {
    let mut out = Vec::new();
    let mut stack = vec![id];
    let mut seen = HashSet::new();
    while let Some(current) = stack.pop() {
        if !seen.insert(current.index()) {
            continue;
        }
        let cell = plan.normal_form(current);
        if let NormalFormKind::Leaf(LeafKind::DataPath(path)) = &cell.kind {
            out.push(path.clone());
        }
        stack.extend(cell.kind.children());
        if let Some(origin) = cell.origin {
            stack.push(origin);
        }
    }
    out
}

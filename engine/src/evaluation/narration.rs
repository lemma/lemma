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
use crate::evaluation::explanations::{format_operation_result, Explanation};
use crate::evaluation::expression::resolve_data_path_value;
use crate::evaluation::tree::borrow_value;
use crate::evaluation::EvaluationContext;
use crate::planning::execution_plan::{
    resolve_nested_dispatch_body, ExecutableRule, ExecutionPlan,
};
use crate::planning::explanation::{Cause, ExplanationNode};
use crate::planning::normalize::{explanation_display, LeafKind, NormalFormId, NormalFormKind};
use crate::planning::ordered_dispatch::{is_exclusive_point_table, region_of_scrutinee};
use crate::planning::semantics::{negated_comparison, DataPath, LemmaType, ValueKind};
use std::collections::HashSet;
use std::sync::Arc;

/// Narration of one cell.
struct Narrated {
    /// Expression text shown under the rule line.
    body: String,
    causes: Vec<Cause>,
    /// How this cell appears as an operand of an enclosing Compose.
    as_operand: Option<ExplanationNode>,
}

/// Operands listed under a Rule / Compose from the cell's as_operand shape.
fn operands_of(as_operand: &Option<ExplanationNode>) -> Vec<ExplanationNode> {
    match as_operand {
        Some(ExplanationNode::Compose { operands, .. }) => operands.clone(),
        Some(other) => vec![other.clone()],
        None => Vec::new(),
    }
}

/// Narrate a rule whose value is stored and insert its Rule node into
/// `rule_explanations`. Rules narrate in plan order (topological), so every rule
/// reference under this rule already has its node.
pub(crate) fn narrate_rule(
    rule: &ExecutableRule,
    plan: &ExecutionPlan,
    ctx: &mut EvaluationContext,
) {
    let narrated = narrate(rule.normal_form, plan, ctx);
    let result_type = ctx.rule_result_type(plan, rule);
    let node = ExplanationNode::Rule {
        name: rule.path.clone(),
        result: Some(format_operation_result(
            ctx.rule_value(plan, &rule.path),
            result_type.as_ref(),
        )),
        body: narrated.body,
        causes: narrated.causes,
        children: operands_of(&narrated.as_operand),
    };
    ctx.rule_explanations.insert(rule.path.clone(), node);
}

/// Root [`Explanation`] for a requested rule, from its narrated Rule node.
pub(crate) fn explanation_for(
    rule: &ExecutableRule,
    ctx: &EvaluationContext,
    rule_value: &OperationResult,
    rule_result_type: Arc<LemmaType>,
) -> Explanation {
    let ExplanationNode::Rule {
        body,
        causes,
        children,
        ..
    } = rule_node(&rule.path, ctx)
    else {
        panic!(
            "BUG: rule_explanations entry for '{}' is not a Rule node",
            rule.path.rule
        )
    };
    Explanation {
        name: rule.path.clone(),
        result: rule_value.clone(),
        result_type: rule_result_type,
        body: body.clone(),
        causes: causes.clone(),
        children: children.clone(),
    }
}

fn rule_node<'a>(
    path: &crate::planning::semantics::RulePath,
    ctx: &'a EvaluationContext,
) -> &'a ExplanationNode {
    ctx.rule_explanations.get(path).unwrap_or_else(|| {
        panic!(
            "BUG: rule '{}' narrated after its use-site (plan order is topological)",
            path.rule
        )
    })
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

fn narrate(id: NormalFormId, plan: &ExecutionPlan, ctx: &EvaluationContext) -> Narrated {
    let cell = plan.normal_form(id);
    if let Some(path) = &cell.rule_ref {
        let node = rule_node(path, ctx).clone();
        return Narrated {
            body: path.rule.clone(),
            causes: Vec::new(),
            as_operand: Some(node),
        };
    }
    if let Some(origin) = cell.origin {
        if let NormalFormKind::OrderedDispatch {
            scrutinee, regions, ..
        } = &cell.kind
        {
            assert_dispatch_agrees_with_record(id, plan, ctx);
            let mut narrated = narrate(origin, plan, ctx);
            if is_exclusive_point_table(regions, |body| region_body_is_nested(plan, body)) {
                apply_exclusive_point_causes(&mut narrated, origin, *scrutinee, plan, ctx);
            }
            return narrated;
        }
        return narrate(origin, plan, ctx);
    }
    narrate_shape(id, plan, ctx)
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
) {
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
            let Some(scrutinee_node) = narrate(scrutinee, plan, ctx).as_operand else {
                panic!("BUG: exclusive point default needs a scrutinee operand");
            };
            narrated.as_operand = match narrated.as_operand.take() {
                Some(ExplanationNode::Veto { message }) => Some(ExplanationNode::Compose {
                    expression: narrated.body.clone(),
                    operands: vec![scrutinee_node, ExplanationNode::Veto { message }],
                }),
                Some(ExplanationNode::Compose { operands, .. }) if operands.is_empty() => {
                    Some(scrutinee_node)
                }
                Some(other) => Some(ExplanationNode::Compose {
                    expression: narrated.body.clone(),
                    operands: vec![scrutinee_node, other],
                }),
                None => Some(scrutinee_node),
            };
        }
        PiecewiseDecision::Undecided { .. } => {
            // Vetoed condition: origin narration already carries the veto.
        }
    }
}

/// Narrate the cell's own Kind (no origin redirection).
fn narrate_shape(id: NormalFormId, plan: &ExecutionPlan, ctx: &EvaluationContext) -> Narrated {
    let forms = plan.normal_forms.as_slice();
    match &plan.normal_form(id).kind {
        NormalFormKind::Leaf(LeafKind::Literal(literal)) => {
            let expression = literal.display_value_with_type(plan.result_type(id).as_ref());
            // Empty operands mark a bare literal; ASCII formatting skips these nodes.
            Narrated {
                body: expression.clone(),
                causes: Vec::new(),
                as_operand: Some(ExplanationNode::Compose {
                    expression,
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
        NormalFormKind::Sum(children) | NormalFormKind::Product(children) => {
            compose(id, children, plan, ctx)
        }
        NormalFormKind::And(left, right) => {
            let operands = match condition_outcome(&slot(*left, plan, ctx)) {
                BranchOutcome::Taken => vec![*left, *right],
                BranchOutcome::NotTaken | BranchOutcome::Propagate(_) => vec![*left],
            };
            compose(id, &operands, plan, ctx)
        }
        NormalFormKind::Subtract(a, b)
        | NormalFormKind::Divide(a, b)
        | NormalFormKind::Power(a, b)
        | NormalFormKind::Modulo(a, b)
        | NormalFormKind::Comparison(a, _, b)
        | NormalFormKind::RangeLiteral(a, b)
        | NormalFormKind::RangeContainment(a, b) => compose(id, &[*a, *b], plan, ctx),
        NormalFormKind::Negate(x)
        | NormalFormKind::Reciprocal(x)
        | NormalFormKind::Not(x)
        | NormalFormKind::MathOp(_, x)
        | NormalFormKind::DateRelative(_, x)
        | NormalFormKind::DateCalendar(_, _, x)
        | NormalFormKind::PastFutureRange(_, x)
        | NormalFormKind::ResultIsVeto(x) => compose(id, &[*x], plan, ctx),
        NormalFormKind::UnitConversion(inner, target) => {
            let expression = explanation_display(forms, id);
            let inner_narrated = narrate(*inner, plan, ctx);
            let source = slot(*inner, plan, ctx);
            if source.vetoed() {
                return compose(id, &[*inner], plan, ctx);
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
                    let data_ref = match &inner_narrated.as_operand {
                        Some(ExplanationNode::Data { name, .. }) => Some(name),
                        _ => None,
                    };
                    let steps = build_conversion_steps(
                        borrow_value(&source, "conversion source"),
                        plan.result_type(*inner),
                        target,
                        result_literal,
                        plan.result_type(id),
                        data_ref,
                    );
                    ExplanationNode::Conversion {
                        expression: expression.clone(),
                        steps,
                        operands: inner_narrated.as_operand.into_iter().collect(),
                    }
                }
            };
            Narrated {
                body: expression,
                causes: Vec::new(),
                as_operand: Some(node),
            }
        }
        NormalFormKind::Piecewise(arms) => match piecewise_causes(arms, plan, ctx) {
            PiecewiseNarration::Causes {
                causes,
                winner_body,
            } => {
                let body = narrate(winner_body, plan, ctx);
                Narrated {
                    body: body.body,
                    causes,
                    as_operand: body.as_operand,
                }
            }
            PiecewiseNarration::Veto(narrated) => narrated,
        },
        NormalFormKind::OrderedDispatch { .. } => {
            unreachable!("BUG: OrderedDispatch always carries its Piecewise pre-image as origin")
        }
    }
}

/// Compose node over the operands the walk visited: expression text from the
/// plan, operands from each child's narration (including bare literals for JSON).
fn compose(
    id: NormalFormId,
    operands: &[NormalFormId],
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
) -> Narrated {
    let expression = explanation_display(plan.normal_forms.as_slice(), id);
    let operands: Vec<ExplanationNode> = operands
        .iter()
        .filter_map(|operand| narrate(*operand, plan, ctx).as_operand)
        .collect();
    Narrated {
        body: expression.clone(),
        causes: Vec::new(),
        as_operand: Some(ExplanationNode::Compose {
            expression,
            operands,
        }),
    }
}

fn data_node(path: &DataPath, plan: &ExecutionPlan, ctx: &EvaluationContext) -> ExplanationNode {
    let result = resolve_data_path_value(path, plan, ctx);
    let data_type = ctx.data_display_type(plan, path);
    let display = match &result {
        OperationResult::Value(value) => value.display_value_with_type(data_type.as_ref()),
        OperationResult::Veto(_) => format_operation_result(&result, data_type.as_ref()),
    };
    ExplanationNode::Data {
        name: path.clone(),
        display,
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
    Veto(Narrated),
}

/// One cause builder for every Piecewise record: the live arms of a Piecewise
/// cell, or the Piecewise pre-image of a collapsed / dispatched cell. Winner from
/// [`piecewise_decision`] over the condition slots the walk filled.
fn piecewise_causes(
    arms: &[(NormalFormId, NormalFormId)],
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
) -> PiecewiseNarration {
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
            return PiecewiseNarration::Veto(Narrated {
                body: explanation_display(plan.normal_forms.as_slice(), condition),
                causes: Vec::new(),
                as_operand: Some(node),
            });
        }
    };
    let mut causes = Vec::new();
    // Arms after the winner (every unless when the default wins), source order.
    let after = winner.map(|arm| arm + 1).unwrap_or(1);
    for (condition, _) in arms.iter().skip(after) {
        match condition_outcome(&slot(*condition, plan, ctx)) {
            BranchOutcome::NotTaken => causes.push(cause(*condition, false, plan, ctx)),
            BranchOutcome::Taken | BranchOutcome::Propagate(_) => {}
        }
    }
    let winner_body = match winner {
        Some(arm) => {
            causes.push(cause(arms[arm].0, true, plan, ctx));
            arms[arm].1
        }
        None => arms[0].1,
    };
    PiecewiseNarration::Causes {
        causes,
        winner_body,
    }
}

/// Cross-check a dispatched cell with a value: the region its scrutinee slot
/// selects must resolve (through any nested dispatch) to the body the Piecewise
/// record's winner names. Two decision procedures (planning fold, source record)
/// must not disagree.
fn assert_dispatch_agrees_with_record(
    id: NormalFormId,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
) {
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
        return;
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
    let PiecewiseNarration::Causes { winner_body, .. } = piecewise_causes(arms, plan, ctx) else {
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
}

fn cause(
    condition: NormalFormId,
    held: bool,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
) -> Cause {
    let focus = peel_bool_leaf_origins(condition, plan);
    if !held {
        if let NormalFormKind::And(left, right) = &plan.normal_form(focus).kind {
            let deciding = deciding_false_and_conjunct(*left, *right, plan, ctx);
            return cause(deciding, false, plan, ctx);
        }
    }
    let (condition_text, value) = condition_statement(focus, held, plan);
    let children = if matches!(plan.normal_form(focus).kind, NormalFormKind::And(..)) {
        and_cause_children(focus, plan, ctx)
    } else {
        cause_children(narrate(focus, plan, ctx).as_operand)
    };
    Cause {
        condition: condition_text,
        value,
        children,
    }
}

/// Short-circuit deciding conjunct of a false `and`, matching [`evaluate_and`].
/// When the left was never walked (static `… and false` force), a static
/// `false` leaf conjunct decides.
fn deciding_false_and_conjunct(
    left: NormalFormId,
    right: NormalFormId,
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
) -> NormalFormId {
    match try_condition_outcome(left, plan, ctx) {
        Some(BranchOutcome::NotTaken) => left,
        Some(BranchOutcome::Taken) => match try_condition_outcome(right, plan, ctx) {
            Some(BranchOutcome::NotTaken) => right,
            other => panic!("BUG: false And with Taken left but right outcome {other:?}"),
        },
        Some(BranchOutcome::Propagate(_)) | None => {
            if is_bool_leaf(plan, right) == Some(false) {
                right
            } else if is_bool_leaf(plan, left) == Some(false) {
                left
            } else {
                panic!("BUG: false And cause without a filled deciding conjunct")
            }
        }
    }
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
    condition: NormalFormId,
    held: bool,
    plan: &ExecutionPlan,
) -> (String, String) {
    let forms = plan.normal_forms.as_slice();
    match &plan.normal_form(condition).kind {
        NormalFormKind::Comparison(a, op, b) => {
            let op = if held {
                op.clone()
            } else {
                negated_comparison(op.clone())
            };
            (
                format!(
                    "{} {op} {}",
                    explanation_display(forms, *a),
                    explanation_display(forms, *b)
                ),
                "true".to_string(),
            )
        }
        NormalFormKind::Not(inner) => condition_statement(*inner, !held, plan),
        NormalFormKind::Leaf(LeafKind::DataPath(path)) => (
            format!(
                "{} is {}",
                path.input_key(),
                if held { "true" } else { "false" }
            ),
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
        | NormalFormKind::OrderedDispatch { .. } => (
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
        | NormalFormKind::Leaf(LeafKind::Literal(_)) => {
            unreachable!("BUG: non-boolean condition in condition_statement")
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
                        | ExplanationNode::Rule { .. }
                        | ExplanationNode::Conversion { .. }
                )
            })
            .collect(),
        Some(node @ ExplanationNode::Rule { .. }) => vec![node],
        Some(node @ ExplanationNode::Conversion { .. }) => vec![node],
        Some(node @ ExplanationNode::Veto { .. }) => vec![node],
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

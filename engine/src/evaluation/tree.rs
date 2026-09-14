//! Value walk: evaluates [`NormalForm`] cells by [`NormalFormId`] into the
//! request value table. Produces values only; narration is a separate pass
//! over the filled table ([`crate::evaluation::narration`]).
//!
//! Policy: [`EvaluationContext::exhaustive`] (set for explain runs) makes the
//! walk visit every cell narration displays: the pre-image (`origin`) of every
//! rewritten cell, and arithmetic siblings after a definitive veto. It never
//! changes a value. `and` keeps its short-circuit in both policies: a left
//! conjunct that is `false` or vetoed ends the `and`. Unless arms after the
//! winner are already visited by the last-wins reverse scan.

use crate::computation::{
    arithmetic_operation, comparison_operation, convert_unit_operand, OperationResult, VetoType,
};
use crate::evaluation::branch_semantics::{condition_outcome, BranchOutcome};
use crate::evaluation::expression::{evaluate_mathematical_operator, resolve_data_path_value};
use crate::evaluation::EvaluationContext;
use crate::planning::execution_plan::{ExecutionPlan, RuleIndex};
use crate::planning::normalize::{LeafKind, NormalFormId, NormalFormKind};
use crate::planning::ordered_dispatch::{region_count, region_of_scrutinee, DispatchKey};
use crate::planning::semantics::{
    ArithmeticComputation, ComparisonComputation, LemmaType, LiteralValue, ValueKind,
};
use std::sync::Arc;

pub(crate) fn borrow_value<'a>(result: &'a OperationResult, operand: &str) -> &'a LiteralValue {
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

/// Walk one rule's NormalForm. On a visited rule reference with an empty slot,
/// returns `Err(index)` without storing this rule. The outer heap stack fills that
/// rule then retries. On success, stores the result in `rule_values`.
fn evaluate_rule(
    index: RuleIndex,
    plan: &ExecutionPlan,
    ctx: &mut EvaluationContext,
) -> Result<(), RuleIndex> {
    let result = eval(plan.rule_at(index).normal_form, plan, ctx)?;
    ctx.rule_values[index.index()] = Some(result);
    Ok(())
}

/// Fill `rule_values` for `rule` and every rule reference its walk visits.
/// Heap stack of rule indices so a long chain does not grow the Rust call
/// stack. The Kind walk must not call this.
pub(crate) fn ensure_rule_values(
    rule: RuleIndex,
    plan: &ExecutionPlan,
    ctx: &mut EvaluationContext,
) {
    let mut stack = vec![rule];
    while let Some(current) = stack.last().copied() {
        if ctx.rule_values[current.index()].is_some() {
            stack.pop();
            continue;
        }
        match evaluate_rule(current, plan, ctx) {
            Ok(()) => {
                stack.pop();
            }
            Err(dependency) => {
                if stack.contains(&dependency) {
                    panic!(
                        "BUG: cyclic rule reference while ensuring values for '{}'",
                        plan.rule_at(current).path.rule
                    );
                }
                stack.push(dependency);
            }
        }
    }
}

/// Evaluate a NormalForm cell. `Err(index)` = visited rule reference whose slot
/// is empty. Does not write incomplete cells.
fn eval(
    id: NormalFormId,
    plan: &ExecutionPlan,
    ctx: &mut EvaluationContext,
) -> Result<OperationResult, RuleIndex> {
    if let Some(cached) = ctx.values[id.index()].as_ref() {
        return Ok(cached.clone());
    }

    let cell = plan.normal_form(id);
    // Rule reference boundary: the cell's value is the referenced rule's stored
    // value. It is cached in `values[id]` like any other cell so liveness walks
    // over the value table see it.
    if let Some(path) = &cell.rule_ref {
        let index = plan.rule_index(path);
        let result = match &ctx.rule_values[index.index()] {
            None => return Err(index),
            Some(result) => result.clone(),
        };
        ctx.values[id.index()] = Some(result.clone());
        return Ok(result);
    }

    let result = eval_kind(id, plan, ctx)?;
    if ctx.exhaustive {
        if let Some(origin) = cell.origin {
            eval(origin, plan, ctx)?;
        }
    }
    // Unbound data leaves must stay None so missing_data walks still see them
    // as unbound. MissingData from a DataPath leaf is not a binding.
    let store = !matches!(
        (&cell.kind, &result),
        (
            NormalFormKind::Leaf(LeafKind::DataPath(_)),
            OperationResult::Veto(VetoType::MissingData { .. }),
        )
    );
    if store {
        ctx.values[id.index()] = Some(result.clone());
    }
    Ok(result)
}

fn eval_kind(
    id: NormalFormId,
    plan: &ExecutionPlan,
    ctx: &mut EvaluationContext,
) -> Result<OperationResult, RuleIndex> {
    match &plan.normal_form(id).kind {
        NormalFormKind::Leaf(LeafKind::Literal(literal)) => {
            Ok(OperationResult::from_literal(literal.clone()))
        }
        NormalFormKind::Leaf(LeafKind::DataPath(path)) => {
            Ok(resolve_data_path_value(path, plan, ctx))
        }
        NormalFormKind::Now => Ok(OperationResult::from_literal(ctx.now().clone())),
        NormalFormKind::Veto(veto) => Ok(OperationResult::Veto(VetoType::UserDefined {
            message: veto.message.clone().filter(|m| !m.is_empty()),
        })),
        NormalFormKind::Sum(children) => {
            fold_nary_arithmetic(id, children, ArithmeticComputation::Add, plan, ctx)
        }
        NormalFormKind::Product(children) => {
            fold_nary_arithmetic(id, children, ArithmeticComputation::Multiply, plan, ctx)
        }
        NormalFormKind::Subtract(left, right) => {
            binary_arithmetic(*left, *right, ArithmeticComputation::Subtract, plan, ctx)
        }
        NormalFormKind::Divide(left, right) => {
            binary_arithmetic(*left, *right, ArithmeticComputation::Divide, plan, ctx)
        }
        NormalFormKind::Power(left, right) => {
            binary_arithmetic(*left, *right, ArithmeticComputation::Power, plan, ctx)
        }
        NormalFormKind::Modulo(left, right) => {
            binary_arithmetic(*left, *right, ArithmeticComputation::Modulo, plan, ctx)
        }
        NormalFormKind::Negate(inner) => {
            let value = eval(*inner, plan, ctx)?;
            if value.vetoed() {
                return Ok(value);
            }
            let zero = OperationResult::from_literal(LiteralValue::number(
                crate::computation::rational::rational_zero(),
            ));
            Ok(binary_arithmetic_result(
                &zero,
                crate::planning::semantics::primitive_number_arc(),
                &value,
                plan.result_type(*inner),
                ArithmeticComputation::Subtract,
                plan,
            ))
        }
        NormalFormKind::Reciprocal(inner) => {
            let value = eval(*inner, plan, ctx)?;
            if value.vetoed() {
                return Ok(value);
            }
            let one = OperationResult::from_literal(LiteralValue::number(
                crate::computation::rational::rational_one(),
            ));
            Ok(binary_arithmetic_result(
                &one,
                crate::planning::semantics::primitive_number_arc(),
                &value,
                plan.result_type(*inner),
                ArithmeticComputation::Divide,
                plan,
            ))
        }
        NormalFormKind::Comparison(left, op, right) => {
            evaluate_binary(*left, *right, plan, ctx, |left_result, right_result| {
                comparison_operation(
                    borrow_value(left_result, "left operand"),
                    plan.result_type(*left),
                    op,
                    borrow_value(right_result, "right operand"),
                    plan.result_type(*right),
                )
            })
        }
        NormalFormKind::And(left, right) => evaluate_and(*left, *right, plan, ctx),
        NormalFormKind::Not(inner) => {
            let value = eval(*inner, plan, ctx)?;
            if value.vetoed() {
                return Ok(value);
            }
            let false_lit = OperationResult::from_literal(LiteralValue::from_bool(false));
            Ok(comparison_operation(
                borrow_value(&value, "not operand"),
                plan.result_type(*inner),
                &ComparisonComputation::Is,
                borrow_value(&false_lit, "not operand"),
                crate::planning::semantics::primitive_boolean_arc(),
            ))
        }
        NormalFormKind::MathOp(op, inner) => {
            let value = eval(*inner, plan, ctx)?;
            if value.vetoed() {
                return Ok(value);
            }
            Ok(evaluate_mathematical_operator(
                op,
                borrow_value(&value, "operand"),
                plan.result_type(id),
            ))
        }
        NormalFormKind::UnitConversion(inner, target) => {
            let value = eval(*inner, plan, ctx)?;
            if value.vetoed() {
                return Ok(value);
            }
            Ok(convert_unit_operand(
                borrow_value(&value, "conversion operand"),
                plan.result_type(*inner).as_ref(),
                target,
            ))
        }
        NormalFormKind::DateRelative(kind, inner) => {
            let value = eval(*inner, plan, ctx)?;
            if value.vetoed() {
                return Ok(value);
            }
            let date = match &borrow_value(&value, "date operand").value {
                ValueKind::Date(dt) => dt,
                other => panic!("BUG: date-relative operand expected date, got {other:?}"),
            };
            Ok(crate::computation::datetime::compute_date_relative(
                kind,
                date,
                now_date(ctx),
            ))
        }
        NormalFormKind::DateCalendar(kind, unit, inner) => {
            let value = eval(*inner, plan, ctx)?;
            if value.vetoed() {
                return Ok(value);
            }
            let date = match &borrow_value(&value, "date operand").value {
                ValueKind::Date(dt) => dt,
                other => panic!("BUG: date-calendar operand expected date, got {other:?}"),
            };
            Ok(crate::computation::datetime::compute_date_calendar(
                kind,
                unit,
                date,
                now_date(ctx),
            ))
        }
        NormalFormKind::RangeLiteral(left, right) => {
            evaluate_binary(*left, *right, plan, ctx, |left_result, right_result| {
                OperationResult::from_literal(LiteralValue::range(
                    borrow_value(left_result, "left endpoint").clone(),
                    borrow_value(right_result, "right endpoint").clone(),
                ))
            })
        }
        NormalFormKind::PastFutureRange(kind, inner) => {
            let value = eval(*inner, plan, ctx)?;
            if value.vetoed() {
                return Ok(value);
            }
            Ok(crate::computation::datetime::evaluate_past_future_range(
                kind,
                borrow_value(&value, "offset operand"),
                plan.result_type(*inner),
                now_date(ctx),
            ))
        }
        NormalFormKind::RangeContainment(value, range) => {
            evaluate_binary(*value, *range, plan, ctx, |value_result, range_result| {
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
                            borrow_value(value_result, "value operand"),
                            plan.result_type(*value),
                            range_left.as_ref(),
                            range_right.as_ref(),
                            &endpoint_type,
                        )
                    }
                    other => {
                        panic!("BUG: range containment expected range operand, got {other:?}")
                    }
                }
            })
        }
        NormalFormKind::ResultIsVeto(inner) => {
            let value = eval(*inner, plan, ctx)?;
            Ok(OperationResult::from_literal(LiteralValue::from_bool(
                value.vetoed(),
            )))
        }
        NormalFormKind::Piecewise(arms) => evaluate_piecewise(arms, plan, ctx),
        NormalFormKind::OrderedDispatch {
            scrutinee,
            boundaries,
            regions,
        } => evaluate_ordered_dispatch(*scrutinee, boundaries, regions, plan, ctx),
    }
}

/// Unless arms scanned high to low: the first `Taken` condition wins, a vetoed
/// condition is the value, otherwise the default body. Arms after the winner are
/// already visited by that reverse scan (narration states why they did not apply).
fn evaluate_piecewise(
    arms: &[(NormalFormId, NormalFormId)],
    plan: &ExecutionPlan,
    ctx: &mut EvaluationContext,
) -> Result<OperationResult, RuleIndex> {
    assert!(!arms.is_empty(), "BUG: empty piecewise");
    for arm in (1..arms.len()).rev() {
        let (condition, body) = arms[arm];
        match condition_outcome(&eval(condition, plan, ctx)?) {
            BranchOutcome::Propagate(result) => return Ok(result),
            BranchOutcome::Taken => return eval(body, plan, ctx),
            BranchOutcome::NotTaken => {}
        }
    }
    eval(arms[0].1, plan, ctx)
}

/// Value of an [`NormalFormKind::OrderedDispatch`] cell: evaluate the scrutinee
/// once, binary-search its region, evaluate that region's body. The exhaustive
/// policy reaches the Piecewise pre-image through the cell's `origin` in [`eval`].
fn evaluate_ordered_dispatch(
    scrutinee: NormalFormId,
    boundaries: &[DispatchKey],
    regions: &[NormalFormId],
    plan: &ExecutionPlan,
    ctx: &mut EvaluationContext,
) -> Result<OperationResult, RuleIndex> {
    assert_eq!(
        regions.len(),
        region_count(boundaries.len()),
        "BUG: OrderedDispatch region table does not match its boundary list"
    );
    let scrutinee_result = eval(scrutinee, plan, ctx)?;
    if scrutinee_result.vetoed() {
        return Ok(scrutinee_result);
    }
    let value = borrow_value(&scrutinee_result, "dispatch scrutinee");
    let region = match region_of_scrutinee(boundaries, &value.value) {
        Ok(region) => region,
        Err(veto) => return Ok(OperationResult::Veto(veto)),
    };
    eval(regions[region], plan, ctx)
}

/// N-ary Sum / Product. A MissingData child does not end the fold: a later
/// definitive veto is the answer instead. After a definitive veto the remaining
/// children are visited only under the exhaustive policy; the value is fixed.
/// Progressive accumulator types come from the Sum/Product cell's `fold_types`.
fn fold_nary_arithmetic(
    id: NormalFormId,
    children: &[NormalFormId],
    op: ArithmeticComputation,
    plan: &ExecutionPlan,
    ctx: &mut EvaluationContext,
) -> Result<OperationResult, RuleIndex> {
    assert!(!children.is_empty(), "BUG: empty n-ary arithmetic");
    let fold_types = &plan.normal_form(id).fold_types;
    assert_eq!(
        fold_types.len(),
        children.len(),
        "BUG: Sum/Product fold_types length must match children"
    );
    let mut veto: Option<OperationResult> = None;
    let mut acc: Option<OperationResult> = None;

    for (i, child) in children.iter().enumerate() {
        if let Some(settled) = &veto {
            if !settled.is_missing_data() && !ctx.exhaustive {
                break;
            }
        }
        let result = eval(*child, plan, ctx)?;
        if let Some(settled) = &veto {
            if settled.is_missing_data() && result.vetoed() && !result.is_missing_data() {
                veto = Some(result);
            }
            continue;
        }
        if result.vetoed() {
            veto = Some(result);
            continue;
        }
        acc = Some(match acc.take() {
            None => result,
            Some(left) => {
                // left type = fold_types[i-1]; next left (if any) = fold_types[i].
                let combined = binary_arithmetic_result(
                    &left,
                    &fold_types[i - 1],
                    &result,
                    plan.result_type(*child),
                    op.clone(),
                    plan,
                );
                if combined.vetoed() {
                    veto = Some(combined);
                    continue;
                }
                combined
            }
        });
    }

    Ok(veto
        .or(acc)
        .expect("BUG: n-ary arithmetic produced neither value nor veto after evaluating children"))
}

/// Binary operator with the sibling rule of [`fold_nary_arithmetic`]: a
/// MissingData left still visits the right operand, and a definitive veto there
/// wins. A definitive left veto is the value; the right operand is visited only
/// under the exhaustive policy.
fn evaluate_binary<F>(
    left: NormalFormId,
    right: NormalFormId,
    plan: &ExecutionPlan,
    ctx: &mut EvaluationContext,
    combine: F,
) -> Result<OperationResult, RuleIndex>
where
    F: FnOnce(&OperationResult, &OperationResult) -> OperationResult,
{
    let left_result = eval(left, plan, ctx)?;
    if left_result.vetoed() && !left_result.is_missing_data() {
        if ctx.exhaustive {
            eval(right, plan, ctx)?;
        }
        return Ok(left_result);
    }
    let right_result = eval(right, plan, ctx)?;
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

fn binary_arithmetic(
    left: NormalFormId,
    right: NormalFormId,
    op: ArithmeticComputation,
    plan: &ExecutionPlan,
    ctx: &mut EvaluationContext,
) -> Result<OperationResult, RuleIndex> {
    evaluate_binary(left, right, plan, ctx, |left_result, right_result| {
        binary_arithmetic_result(
            left_result,
            plan.result_type(left),
            right_result,
            plan.result_type(right),
            op,
            plan,
        )
    })
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
        &plan.resolved_types.unit_index,
        &plan.signature_index,
    )
}

/// Left decides: a conjunct that is `false` or vetoed is the value and the
/// right is never visited. A `true` left yields the right's value.
fn evaluate_and(
    left: NormalFormId,
    right: NormalFormId,
    plan: &ExecutionPlan,
    ctx: &mut EvaluationContext,
) -> Result<OperationResult, RuleIndex> {
    match condition_outcome(&eval(left, plan, ctx)?) {
        BranchOutcome::Propagate(result) => Ok(result),
        BranchOutcome::NotTaken => Ok(OperationResult::from_literal(LiteralValue::from_bool(
            false,
        ))),
        BranchOutcome::Taken => eval(right, plan, ctx),
    }
}

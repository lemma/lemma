//! Show rule graph: resolved branch expressions projected for [`crate::Engine::show`].
//!
//! Built once from [`crate::planning::graph::RuleNode`] branches during plan build.
//! Not NormalForm and not evaluation narration.

use crate::parsing::arena::{ExprId, ExprKind, ExprRef};
use crate::parsing::ast::{
    ArithmeticComputation, CalendarPeriodUnit, ComparisonComputation, DateCalendarKind,
    DateRelativeKind, MathematicalComputation, PrimitiveKind, RangeBound, VetoExpression,
};
use crate::planning::semantics::{
    Expression, ExpressionKind, ListOp, SemanticConversionTarget, TypedLiteral,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// One arm of a rule's flat last-match table on Show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShowBranch {
    /// Absent on the default arm; present on each `unless`.
    pub condition: Option<ShowExpression>,
    pub result: ShowExpression,
}

/// Local rule on Show: result type, authored branches, stored topo deps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShowRule {
    pub lemma_type: crate::planning::semantics::LemmaType,
    /// Import hops to this rule (`[]` = this spec). Same shape as [`ShowData::path`].
    pub path: Vec<crate::planning::semantics::PathSegment>,
    pub branches: Vec<ShowBranch>,
    /// Show.rules keys this rule depends on (planning topo). Always present.
    pub depends_on_rules: Vec<String>,
}

/// Conversion target on a Show `as` expression (no owning type — inspection only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShowConversionTarget {
    Type(PrimitiveKind),
    Unit { unit_name: String },
}

/// Resolved expression tree for Show branches (`tag = "type"` on the API mirror).
///
/// Literals stay [`TypedLiteral`] (postcard-safe ℚ). [`RuleResultValue`] is built at the
/// JSON/`lemma::api` boundary only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShowExpression {
    Literal(Box<TypedLiteral>),
    Data {
        name: String,
    },
    Rule {
        name: String,
    },
    /// `a and b and c`: one left fold, operands in source order.
    And {
        operands: Vec<ShowExpression>,
    },
    Not {
        operand: Box<ShowExpression>,
    },
    /// `a + b - c`: one left fold on one precedence level, operands in source order.
    Arithmetic {
        first: Box<ShowExpression>,
        steps: Vec<ShowArithmeticStep>,
    },
    Comparison {
        op: ComparisonComputation,
        left: Box<ShowExpression>,
        right: Box<ShowExpression>,
    },
    /// `x as a as b`: one node over the innermost operand, `targets` in application order.
    UnitConversion {
        operand: Box<ShowExpression>,
        targets: Vec<ShowConversionTarget>,
    },
    Math {
        op: MathematicalComputation,
        operand: Box<ShowExpression>,
    },
    Veto {
        message: Option<String>,
    },
    Now,
    DateRelative {
        kind: DateRelativeKind,
        operand: Box<ShowExpression>,
    },
    DateCalendar {
        kind: DateCalendarKind,
        unit: CalendarPeriodUnit,
        operand: Box<ShowExpression>,
    },
    RangeLiteral {
        from: Box<ShowExpression>,
        to: Box<ShowExpression>,
    },
    PastFutureRange {
        kind: DateRelativeKind,
        operand: Box<ShowExpression>,
    },
    RangeContainment {
        value: Box<ShowExpression>,
        range: Box<ShowExpression>,
    },
    RangeBound {
        bound: RangeBound,
        operand: Box<ShowExpression>,
    },
    IsVeto {
        operand: Box<ShowExpression>,
    },
    /// `rule.field` on a spec-valued rule. `member` is `field` or `alias.field`.
    SpecMember {
        base: String,
        member: String,
    },
    /// `(expression).field` on a spec value that is not a rule reference.
    Member {
        base: Box<ShowExpression>,
        member: String,
    },
    /// Field of the current row inside an instance-list command argument.
    RowField {
        field: String,
    },
    /// The current row's value: the implicit left operand of a value-list condition.
    RowValue,
    /// `field: value` of a row appended by `-> with`.
    RowFieldValue {
        field: String,
        value: Box<ShowExpression>,
    },
    /// `direction` is set exactly when `command` is [`ShowListCommand::Sort`].
    ListCommand {
        list: Box<ShowExpression>,
        command: ShowListCommand,
        direction: Option<SortDirection>,
        args: Vec<ShowExpression>,
    },
}

/// One operator step of a Show arithmetic chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShowArithmeticStep {
    pub op: ArithmeticComputation,
    pub operand: ShowExpression,
}

/// Postfix list command on Show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShowListCommand {
    Find,
    First,
    Last,
    Count,
    Sum,
    Avg,
    Product,
    Min,
    Max,
    Any,
    All,
    Where,
    Sort,
    Top,
    With,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortDirection {
    Asc,
    Desc,
}

impl ShowExpression {
    /// Direct sub-expressions in the order [`Self::fold`] hands their results back.
    pub(crate) fn operands(&self) -> Vec<&ShowExpression> {
        match self {
            Self::Literal(_)
            | Self::Data { .. }
            | Self::Rule { .. }
            | Self::Veto { .. }
            | Self::Now
            | Self::SpecMember { .. }
            | Self::RowField { .. }
            | Self::RowValue => Vec::new(),
            Self::And { operands } => operands.iter().collect(),
            Self::Not { operand }
            | Self::UnitConversion { operand, .. }
            | Self::Math { operand, .. }
            | Self::DateRelative { operand, .. }
            | Self::DateCalendar { operand, .. }
            | Self::PastFutureRange { operand, .. }
            | Self::RangeBound { operand, .. }
            | Self::IsVeto { operand } => vec![operand.as_ref()],
            Self::Arithmetic { first, steps } => {
                let mut out = vec![first.as_ref()];
                out.extend(steps.iter().map(|step| &step.operand));
                out
            }
            Self::Comparison { left, right, .. }
            | Self::RangeLiteral {
                from: left,
                to: right,
            }
            | Self::RangeContainment {
                value: left,
                range: right,
            } => vec![left.as_ref(), right.as_ref()],
            Self::Member { base, .. } => vec![base.as_ref()],
            Self::RowFieldValue { value, .. } => vec![value.as_ref()],
            Self::ListCommand { list, args, .. } => {
                let mut out = vec![list.as_ref()];
                out.extend(args.iter());
                out
            }
        }
    }

    /// Bottom-up fold with an explicit stack. `step` receives each node with its operands'
    /// results in [`Self::operands`] order.
    pub(crate) fn fold<U>(&self, mut step: impl FnMut(&ShowExpression, Vec<U>) -> U) -> U {
        let mut values: Vec<U> = Vec::new();
        let mut stack: Vec<(&ShowExpression, bool)> = vec![(self, false)];
        while let Some((node, expanded)) = stack.pop() {
            let operands = node.operands();
            if !expanded {
                stack.push((node, true));
                stack.extend(operands.iter().rev().map(|&operand| (operand, false)));
                continue;
            }
            let split = values
                .len()
                .checked_sub(operands.len())
                .expect("BUG: operand folded before its parent");
            let args = values.split_off(split);
            values.push(step(node, args));
        }
        values
            .pop()
            .expect("BUG: fold produced no value for its root")
    }
}

/// Next operand from a parent's operand list, in [`ShowExpression::operands`] order.
pub(crate) fn take_operand<T, I: Iterator<Item = T>>(operands: &mut I) -> T {
    operands
        .next()
        .expect("BUG: operand missing for its parent node")
}

/// Project the subtree at `root` into the Show tree.
///
/// `show_keys` are `RulePath::input_key` values for reachable Show rules. A
/// `rule` leaf whose key is missing from that set is a bug.
///
/// Bottom-up projection with an explicit stack. Each node takes its operands' projections
/// from the value stack in [`for_each_projection_operand`] order.
fn project(
    root: ExprRef<'_, ExpressionKind>,
    show_keys: &HashSet<String>,
    scratch: &mut ProjectScratch,
) -> ShowExpression {
    let ProjectScratch { values, stack } = scratch;
    stack.push((root.id, false));
    while let Some((id, expanded)) = stack.pop() {
        let node = root.child(id);
        if !expanded {
            stack.push((id, true));
            let start = stack.len();
            for_each_projection_operand(node, |operand| stack.push((operand, false)));
            stack[start..].reverse();
            continue;
        }
        let mut arity = 0;
        for_each_projection_operand(node, |_| arity += 1);
        let split = values
            .len()
            .checked_sub(arity)
            .expect("BUG: operand projected before its parent");
        let shown = project_node(node, values.drain(split..), show_keys);
        values.push(shown);
    }
    let shown = values
        .pop()
        .expect("BUG: projection produced no value for its root");
    assert!(
        values.is_empty(),
        "BUG: projection left operands without a parent"
    );
    shown
}

/// Stacks of [`project`], reused across calls. Empty between calls.
#[derive(Default)]
struct ProjectScratch {
    values: Vec<ShowExpression>,
    stack: Vec<(ExprId, bool)>,
}

/// Calls `f` on the sub-expressions a Show node is built from, in [`project_node`] consumption order.
/// An `as` run contributes only its innermost operand; a rule-path member base, none.
fn for_each_projection_operand(node: ExprRef<'_, ExpressionKind>, mut f: impl FnMut(ExprId)) {
    match node.kind() {
        ExpressionKind::Piecewise(_) => {}
        ExpressionKind::SpecMember { base, .. } => {
            if !matches!(node.child(*base).kind(), ExpressionKind::RulePath(_)) {
                f(*base);
            }
        }
        ExpressionKind::UnitConversion(_, _) => f(conversion_operand(node)),
        other => other.for_each_child(f),
    }
}

/// Innermost operand of the `as` run ending at `node`.
fn conversion_operand(node: ExprRef<'_, ExpressionKind>) -> ExprId {
    let mut current = node;
    while let ExpressionKind::UnitConversion(operand, _) = current.kind() {
        current = current.child(*operand);
    }
    current.id
}

/// Innermost operand and targets (application order) of the `as` run ending at `node`.
fn conversion_run(node: ExprRef<'_, ExpressionKind>) -> (ExprId, Vec<ShowConversionTarget>) {
    let mut current = node;
    let mut targets = Vec::new();
    while let ExpressionKind::UnitConversion(operand, target) = current.kind() {
        targets.push(conversion_target_from(target));
        current = current.child(*operand);
    }
    targets.reverse();
    (current.id, targets)
}

/// Build one Show node from its already-projected operands.
fn project_node(
    node: ExprRef<'_, ExpressionKind>,
    mut operands: std::vec::Drain<'_, ShowExpression>,
    show_keys: &HashSet<String>,
) -> ShowExpression {
    match node.kind() {
        ExpressionKind::Literal(typed) => ShowExpression::Literal(Box::new(typed.as_ref().clone())),
        ExpressionKind::DataPath(path) => ShowExpression::Data {
            name: path.input_key(),
        },
        ExpressionKind::RulePath(path) => {
            let key = path.input_key();
            if !show_keys.contains(&key) {
                panic!("BUG: rule leaf '{key}' not in Show.rules keys");
            }
            ShowExpression::Rule { name: key }
        }
        ExpressionKind::LogicalAnd { .. } => ShowExpression::And {
            operands: operands.collect(),
        },
        ExpressionKind::LogicalNegation(_, _) => ShowExpression::Not {
            operand: Box::new(take_operand(&mut operands)),
        },
        ExpressionKind::Arithmetic { steps, .. } => {
            let first = Box::new(take_operand(&mut operands));
            let steps = steps
                .iter()
                .map(|step| ShowArithmeticStep {
                    op: step.op.clone(),
                    operand: take_operand(&mut operands),
                })
                .collect();
            ShowExpression::Arithmetic { first, steps }
        }
        ExpressionKind::Comparison(_, op, _) => {
            let left = Box::new(take_operand(&mut operands));
            let right = Box::new(take_operand(&mut operands));
            ShowExpression::Comparison {
                op: op.clone(),
                left,
                right,
            }
        }
        ExpressionKind::UnitConversion(_, _) => {
            let (_, targets) = conversion_run(node);
            ShowExpression::UnitConversion {
                operand: Box::new(take_operand(&mut operands)),
                targets,
            }
        }
        ExpressionKind::MathematicalComputation(op, _) => ShowExpression::Math {
            op: op.clone(),
            operand: Box::new(take_operand(&mut operands)),
        },
        ExpressionKind::Veto(VetoExpression { message }) => ShowExpression::Veto {
            message: message.clone(),
        },
        ExpressionKind::Now => ShowExpression::Now,
        ExpressionKind::DateRelative(kind, _) => ShowExpression::DateRelative {
            kind: *kind,
            operand: Box::new(take_operand(&mut operands)),
        },
        ExpressionKind::DateCalendar(kind, unit, _) => ShowExpression::DateCalendar {
            kind: *kind,
            unit: *unit,
            operand: Box::new(take_operand(&mut operands)),
        },
        ExpressionKind::RangeLiteral(_, _) => {
            let from = Box::new(take_operand(&mut operands));
            let to = Box::new(take_operand(&mut operands));
            ShowExpression::RangeLiteral { from, to }
        }
        ExpressionKind::PastFutureRange(kind, _) => ShowExpression::PastFutureRange {
            kind: *kind,
            operand: Box::new(take_operand(&mut operands)),
        },
        ExpressionKind::RangeContainment(_, _) => {
            let value = Box::new(take_operand(&mut operands));
            let range = Box::new(take_operand(&mut operands));
            ShowExpression::RangeContainment { value, range }
        }
        ExpressionKind::RangeBound(bound, _) => ShowExpression::RangeBound {
            bound: *bound,
            operand: Box::new(take_operand(&mut operands)),
        },
        ExpressionKind::ResultIsVeto(_) => ShowExpression::IsVeto {
            operand: Box::new(take_operand(&mut operands)),
        },
        ExpressionKind::Piecewise(_) => {
            panic!("BUG: piecewise inside a rule branch")
        }
        ExpressionKind::SpecMember {
            base, path, name, ..
        } => {
            let mut member = String::new();
            for segment in path {
                member.push_str(segment);
                member.push('.');
            }
            member.push_str(name);
            match node.child(*base).kind() {
                ExpressionKind::RulePath(rule) => {
                    let key = rule.input_key();
                    if !show_keys.contains(&key) {
                        panic!("BUG: spec member base '{key}' not in Show.rules keys");
                    }
                    ShowExpression::SpecMember { base: key, member }
                }
                _ => ShowExpression::Member {
                    base: Box::new(take_operand(&mut operands)),
                    member,
                },
            }
        }
        ExpressionKind::RowField { end, .. } => ShowExpression::RowField {
            field: end.to_string(),
        },
        ExpressionKind::ListCommand { op, .. } => {
            let list = Box::new(take_operand(&mut operands));
            let (command, direction, args) = show_list_op(op, &mut operands);
            ShowExpression::ListCommand {
                list,
                command,
                direction,
                args,
            }
        }
    }
}

/// Operand expression ids of a list op, in the order [`show_list_op`] consumes them.
fn show_list_op(
    op: &ListOp,
    operands: &mut std::vec::Drain<'_, ShowExpression>,
) -> (ShowListCommand, Option<SortDirection>, Vec<ShowExpression>) {
    use crate::parsing::ast::RowCondition;
    use crate::planning::semantics::{ListPredKind, ListReduce};
    let predicate = |kind: &ListPredKind| match kind {
        ListPredKind::Any => ShowListCommand::Any,
        ListPredKind::All => ShowListCommand::All,
        ListPredKind::Where => ShowListCommand::Where,
    };
    match op {
        ListOp::Find { keys } => (
            ShowListCommand::Find,
            None,
            operands.by_ref().take(keys.len()).collect(),
        ),
        ListOp::First => (ShowListCommand::First, None, Vec::new()),
        ListOp::Last => (ShowListCommand::Last, None, Vec::new()),
        ListOp::Count => (ShowListCommand::Count, None, Vec::new()),
        ListOp::Reduce { kind, argument } => {
            let command = match kind {
                ListReduce::Sum => ShowListCommand::Sum,
                ListReduce::Avg => ShowListCommand::Avg,
                ListReduce::Product => ShowListCommand::Product,
                ListReduce::Min => ShowListCommand::Min,
                ListReduce::Max => ShowListCommand::Max,
            };
            let args = if argument.is_some() {
                vec![take_operand(operands)]
            } else {
                Vec::new()
            };
            (command, None, args)
        }
        ListOp::Predicate { kind, op, .. } => {
            let right = Box::new(take_operand(operands));
            let condition = match op {
                RowCondition::Compare(comparison) => ShowExpression::Comparison {
                    op: comparison.clone(),
                    left: Box::new(ShowExpression::RowValue),
                    right,
                },
                RowCondition::In => ShowExpression::RangeContainment {
                    value: Box::new(ShowExpression::RowValue),
                    range: right,
                },
            };
            (predicate(kind), None, vec![condition])
        }
        ListOp::InstancePredicate { kind, .. } => {
            (predicate(kind), None, vec![take_operand(operands)])
        }
        ListOp::Sort {
            descending,
            argument,
        } => {
            let direction = if *descending {
                SortDirection::Desc
            } else {
                SortDirection::Asc
            };
            let args = if argument.is_some() {
                vec![take_operand(operands)]
            } else {
                Vec::new()
            };
            (ShowListCommand::Sort, Some(direction), args)
        }
        ListOp::Top { .. } => (ShowListCommand::Top, None, vec![take_operand(operands)]),
        ListOp::With { rows } => (
            ShowListCommand::With,
            None,
            operands.by_ref().take(rows.len()).collect(),
        ),
        ListOp::WithRow { fields } => (
            ShowListCommand::With,
            None,
            fields
                .iter()
                .map(|(path, _)| ShowExpression::RowFieldValue {
                    field: path.input_key(),
                    value: Box::new(take_operand(operands)),
                })
                .collect(),
        ),
    }
}

/// Project `RuleNode.branches` into Show arms (default first, no synthetic true).
pub fn show_branches_from(
    expression: &Expression,
    branches: &[(Option<ExprId>, ExprId)],
    show_keys: &HashSet<String>,
) -> Vec<ShowBranch> {
    let mut scratch = ProjectScratch::default();
    branches
        .iter()
        .map(|(condition, result)| ShowBranch {
            condition: condition
                .map(|condition| project(expression.at(condition), show_keys, &mut scratch)),
            result: project(expression.at(*result), show_keys, &mut scratch),
        })
        .collect()
}

/// Project `RuleNode.depends_on_rules` to Show.rules keys (BTreeSet order).
pub fn project_depends_on_rules(
    deps: &std::collections::BTreeSet<crate::planning::semantics::RulePath>,
    show_keys: &HashSet<String>,
) -> Vec<String> {
    let mut out = Vec::new();
    for path in deps {
        let key = path.input_key();
        if !show_keys.contains(&key) {
            panic!("BUG: depends_on_rules entry '{key}' missing from Show.rules keys");
        }
        out.push(key);
    }
    out
}

/// Prefix Data/Rule/SpecMember names when lifting a template Show tree under a `uses` alias path.
pub(crate) fn prefix_show_expression(prefix: &str, expression: &ShowExpression) -> ShowExpression {
    expression.fold(|node, operands| prefix_show_node(prefix, node, operands))
}

fn prefix_show_node(
    prefix: &str,
    node: &ShowExpression,
    operands: Vec<ShowExpression>,
) -> ShowExpression {
    let qualify = |name: &str| format!("{prefix}.{name}");
    let mut operands = operands.into_iter();
    match node {
        ShowExpression::Literal(literal) => ShowExpression::Literal(literal.clone()),
        ShowExpression::Data { name } => ShowExpression::Data {
            name: qualify(name),
        },
        ShowExpression::Rule { name } => ShowExpression::Rule {
            name: qualify(name),
        },
        ShowExpression::And { .. } => ShowExpression::And {
            operands: operands.collect(),
        },
        ShowExpression::Not { .. } => ShowExpression::Not {
            operand: Box::new(take_operand(&mut operands)),
        },
        ShowExpression::Arithmetic { steps, .. } => {
            let first = Box::new(take_operand(&mut operands));
            let steps = steps
                .iter()
                .map(|step| ShowArithmeticStep {
                    op: step.op.clone(),
                    operand: take_operand(&mut operands),
                })
                .collect();
            ShowExpression::Arithmetic { first, steps }
        }
        ShowExpression::Comparison { op, .. } => {
            let left = Box::new(take_operand(&mut operands));
            let right = Box::new(take_operand(&mut operands));
            ShowExpression::Comparison {
                op: op.clone(),
                left,
                right,
            }
        }
        ShowExpression::UnitConversion { targets, .. } => ShowExpression::UnitConversion {
            operand: Box::new(take_operand(&mut operands)),
            targets: targets.clone(),
        },
        ShowExpression::Math { op, .. } => ShowExpression::Math {
            op: op.clone(),
            operand: Box::new(take_operand(&mut operands)),
        },
        ShowExpression::Veto { message } => ShowExpression::Veto {
            message: message.clone(),
        },
        ShowExpression::Now => ShowExpression::Now,
        ShowExpression::DateRelative { kind, .. } => ShowExpression::DateRelative {
            kind: *kind,
            operand: Box::new(take_operand(&mut operands)),
        },
        ShowExpression::DateCalendar { kind, unit, .. } => ShowExpression::DateCalendar {
            kind: *kind,
            unit: *unit,
            operand: Box::new(take_operand(&mut operands)),
        },
        ShowExpression::RangeLiteral { .. } => {
            let from = Box::new(take_operand(&mut operands));
            let to = Box::new(take_operand(&mut operands));
            ShowExpression::RangeLiteral { from, to }
        }
        ShowExpression::PastFutureRange { kind, .. } => ShowExpression::PastFutureRange {
            kind: *kind,
            operand: Box::new(take_operand(&mut operands)),
        },
        ShowExpression::RangeContainment { .. } => {
            let value = Box::new(take_operand(&mut operands));
            let range = Box::new(take_operand(&mut operands));
            ShowExpression::RangeContainment { value, range }
        }
        ShowExpression::RangeBound { bound, .. } => ShowExpression::RangeBound {
            bound: *bound,
            operand: Box::new(take_operand(&mut operands)),
        },
        ShowExpression::IsVeto { .. } => ShowExpression::IsVeto {
            operand: Box::new(take_operand(&mut operands)),
        },
        ShowExpression::SpecMember { base, member } => ShowExpression::SpecMember {
            base: qualify(base),
            member: member.clone(),
        },
        ShowExpression::Member { member, .. } => ShowExpression::Member {
            base: Box::new(take_operand(&mut operands)),
            member: member.clone(),
        },
        ShowExpression::RowField { field } => ShowExpression::RowField {
            field: field.clone(),
        },
        ShowExpression::RowValue => ShowExpression::RowValue,
        ShowExpression::RowFieldValue { field, .. } => ShowExpression::RowFieldValue {
            field: field.clone(),
            value: Box::new(take_operand(&mut operands)),
        },
        ShowExpression::ListCommand {
            command, direction, ..
        } => {
            let list = Box::new(take_operand(&mut operands));
            ShowExpression::ListCommand {
                list,
                command: *command,
                direction: *direction,
                args: operands.collect(),
            }
        }
    }
}

fn conversion_target_from(target: &SemanticConversionTarget) -> ShowConversionTarget {
    match target {
        SemanticConversionTarget::Type(kind) => ShowConversionTarget::Type(*kind),
        SemanticConversionTarget::Unit { unit_name, .. } => ShowConversionTarget::Unit {
            unit_name: unit_name.clone(),
        },
    }
}

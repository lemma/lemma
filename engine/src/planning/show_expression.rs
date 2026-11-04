//! Show rule graph: resolved branch expressions projected for [`crate::Engine::show`].
//!
//! Built once from [`crate::planning::graph::RuleNode`] branches during plan build.
//! Not NormalForm and not evaluation narration.

use crate::parsing::arena::{ExprId, ExprKind, ExprRef};
use crate::parsing::ast::{
    ArithmeticComputation, CalendarPeriodUnit, ComparisonComputation, ConversionTypeKind,
    DateCalendarKind, DateRelativeKind, LogicalConnective, MathematicalComputation, RangeBound,
    RoundingMode, VetoExpression,
};
use crate::planning::execution_plan::ExecutionPlan;
use crate::planning::instance_list::{row_end_chain, RowEndView};
use crate::planning::semantics::{
    DataDefinition, DataPath, Expression, ExpressionKind, ListOp, PathSegment, RowEnd, RulePath,
    SemanticConversionTarget, SpecIdentity, TypedLiteral,
};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

/// What projecting the rules of one plan reads: the reachable Show rules, and the
/// plan's data and row templates, which name a member read on a `uses` import.
pub(crate) struct ShowScope<'a> {
    pub(crate) show_keys: &'a HashSet<RulePath>,
    pub(crate) data: &'a IndexMap<DataPath, DataDefinition>,
    pub(crate) templates: &'a BTreeMap<SpecIdentity, Arc<ExecutionPlan>>,
}

/// What a member read on a `uses` import names: a rule or data of the instance it
/// lands on, by path composed from the import hops.
enum ImportMember {
    Rule(RulePath),
    Data(DataPath),
}

impl ShowScope<'_> {
    /// The member `path.name` names when `base` is a `uses` slot of the plan.
    fn import_member(
        &self,
        base: &ExpressionKind,
        path: &[String],
        name: &str,
    ) -> Option<ImportMember> {
        let ExpressionKind::DataPath(slot) = base else {
            return None;
        };
        let Some(DataDefinition::Import { row, .. }) = self.data.get(slot) else {
            return None;
        };
        let template = self.templates.get(&row.identity).unwrap_or_else(|| {
            panic!(
                "BUG: `uses` slot '{slot}' has no template of '{}'",
                row.identity
            )
        });
        let parts: Vec<&str> = path
            .iter()
            .map(String::as_str)
            .chain(std::iter::once(name))
            .collect();
        let chain = row_end_chain(
            &row.identity,
            RowEndView::Plan(template),
            self.templates,
            &parts,
        )
        .unwrap_or_else(|| {
            panic!(
                "BUG: member '{}' of `uses` slot '{slot}' planned but unresolved",
                parts.join(".")
            )
        });
        let mut segments = vec![PathSegment::import(&slot.data, &row.identity)];
        for (hop, next) in chain.iter().zip(chain.iter().skip(1)) {
            let RowEnd::Data(alias) = &hop.end else {
                panic!(
                    "BUG: a member chain hop is a `uses` slot, got {:?}",
                    hop.end
                );
            };
            segments.push(PathSegment::import(&alias.data, &next.identity));
        }
        let last = chain.last().expect("BUG: a member chain has a last end");
        Some(match &last.end {
            RowEnd::Rule(rule) => ImportMember::Rule(RulePath::new(segments, rule.rule.clone())),
            RowEnd::Data(data) => ImportMember::Data(DataPath {
                segments,
                data: data.data.clone(),
            }),
        })
    }
}

/// One arm of a rule's flat last-match table on Show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShowBranch {
    /// Absent on the default arm; present on each `unless`.
    pub condition: Option<ShowExpression>,
    pub result: ShowExpression,
}

/// Local rule on Show: result type, authored branches, stored topo deps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShowRule {
    pub lemma_type: crate::planning::semantics::LemmaType,
    /// Import hops to this rule (`[]` = this spec). Catalog entries are always empty.
    pub path: Vec<crate::planning::semantics::PathSegment>,
    pub branches: Vec<ShowBranch>,
    /// Other Show.rules keys this rule depends on (local catalog closure). Always present.
    /// Branch Rule leaves may name reachable imports outside this set.
    pub depends_on_rules: Vec<RulePath>,
}

/// Conversion target on a Show `as` expression (no owning type — inspection only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShowConversionTarget {
    Type(ConversionTypeKind),
    Unit { unit_name: String },
}

/// Resolved expression tree for Show branches (`tag = "type"` on the API mirror).
///
/// Literals stay [`TypedLiteral`]. [`RuleResultValue`] is built at the
/// JSON/`lemma::api` boundary only.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    Or {
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
    /// `operand -> round` or `operand -> round step`.
    StepRounding {
        mode: RoundingMode,
        operand: Box<ShowExpression>,
        step: Option<Box<ShowExpression>>,
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
        /// Written list qualifier when the source used `list.field`; `None` for bare refs.
        qualifier: Option<Vec<String>>,
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
    Key,
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
            Self::And { operands } | Self::Or { operands } => operands.iter().collect(),
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
            Self::StepRounding { operand, step, .. } => {
                let mut out = vec![operand.as_ref()];
                if let Some(step) = step {
                    out.push(step.as_ref());
                }
                out
            }
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
/// `show_keys` are reachable Show rules. A `rule` leaf whose path is missing from that set is a bug.
///
/// Bottom-up projection with an explicit stack. Each node takes its operands' projections
/// from the value stack in [`for_each_projection_operand`] order.
fn project(
    root: ExprRef<'_, ExpressionKind>,
    scope: &ShowScope<'_>,
    scratch: &mut ProjectScratch,
) -> ShowExpression {
    let ProjectScratch { values, stack } = scratch;
    stack.push((root.id, false));
    while let Some((id, expanded)) = stack.pop() {
        let node = root.child(id);
        if !expanded {
            stack.push((id, true));
            let start = stack.len();
            for_each_projection_operand(node, scope, |operand| stack.push((operand, false)));
            stack[start..].reverse();
            continue;
        }
        let mut arity = 0;
        for_each_projection_operand(node, scope, |_| arity += 1);
        let split = values
            .len()
            .checked_sub(arity)
            .expect("BUG: operand projected before its parent");
        let shown = project_node(node, values.drain(split..), scope);
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
/// An `as` run contributes only its innermost operand; a rule-path or `uses` member
/// base, none.
fn for_each_projection_operand(
    node: ExprRef<'_, ExpressionKind>,
    scope: &ShowScope<'_>,
    mut f: impl FnMut(ExprId),
) {
    match node.kind() {
        ExpressionKind::Piecewise(_) => {}
        ExpressionKind::SpecMember { base, path, name } => {
            let base_kind = node.child(*base).kind();
            if !matches!(base_kind, ExpressionKind::RulePath(_))
                && scope.import_member(base_kind, path, name).is_none()
            {
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
    scope: &ShowScope<'_>,
) -> ShowExpression {
    let show_keys = scope.show_keys;
    match node.kind() {
        ExpressionKind::Literal(typed) => ShowExpression::Literal(Box::new(typed.as_ref().clone())),
        ExpressionKind::DataPath(path) => ShowExpression::Data {
            name: path.input_key(),
        },
        ExpressionKind::RulePath(path) => {
            if !show_keys.contains(path) {
                panic!(
                    "BUG: rule leaf '{}' not in Show.rules keys",
                    path.input_key()
                );
            }
            ShowExpression::Rule {
                name: path.input_key(),
            }
        }
        ExpressionKind::Logical { connective, .. } => match connective {
            LogicalConnective::And => ShowExpression::And {
                operands: operands.collect(),
            },
            LogicalConnective::Or => ShowExpression::Or {
                operands: operands.collect(),
            },
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
        ExpressionKind::StepRounding { mode, step, .. } => {
            let operand = Box::new(take_operand(&mut operands));
            ShowExpression::StepRounding {
                mode: *mode,
                operand,
                step: step.map(|_| Box::new(take_operand(&mut operands))),
            }
        }
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
        ExpressionKind::SpecMember { base, path, name } => {
            match scope.import_member(node.child(*base).kind(), path, name) {
                Some(ImportMember::Rule(rule)) => {
                    if !show_keys.contains(&rule) {
                        panic!(
                            "BUG: `uses` member rule '{}' not in Show.rules keys",
                            rule.input_key()
                        );
                    }
                    return ShowExpression::Rule {
                        name: rule.input_key(),
                    };
                }
                Some(ImportMember::Data(data)) => {
                    return ShowExpression::Data {
                        name: data.input_key(),
                    };
                }
                None => {}
            }
            let mut member = String::new();
            for segment in path {
                member.push_str(segment);
                member.push('.');
            }
            member.push_str(name);
            match node.child(*base).kind() {
                ExpressionKind::RulePath(rule) => {
                    if !show_keys.contains(rule) {
                        panic!(
                            "BUG: spec member base '{}' not in Show.rules keys",
                            rule.input_key()
                        );
                    }
                    ShowExpression::SpecMember {
                        base: rule.input_key(),
                        member,
                    }
                }
                _ => ShowExpression::Member {
                    base: Box::new(take_operand(&mut operands)),
                    member,
                },
            }
        }
        ExpressionKind::RowField { end, qualifier, .. } => ShowExpression::RowField {
            field: end.to_string(),
            qualifier: qualifier.clone(),
        },
        ExpressionKind::RowValue { .. } => ShowExpression::RowValue,
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
        ListOp::Predicate { kind, .. } => (predicate(kind), None, vec![take_operand(operands)]),
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
        ListOp::WithRow { fields, .. } => (
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
        ListOp::Key { fields } => (
            ShowListCommand::Key,
            None,
            fields
                .iter()
                .map(|path| ShowExpression::Data {
                    name: path.input_key(),
                })
                .collect(),
        ),
    }
}

/// Project `RuleNode.branches` into Show arms (default first, no synthetic true).
pub(crate) fn show_branches_from(
    expression: &Expression,
    branches: &[(Option<ExprId>, ExprId)],
    scope: &ShowScope<'_>,
) -> Vec<ShowBranch> {
    let mut scratch = ProjectScratch::default();
    branches
        .iter()
        .map(|(condition, result)| ShowBranch {
            condition: condition
                .map(|condition| project(expression.at(condition), scope, &mut scratch)),
            result: project(expression.at(*result), scope, &mut scratch),
        })
        .collect()
}

/// Project planning edges onto the public Show.rules catalog (BTreeSet order).
///
/// Local deps must be catalog keys. Import deps (non-empty segments) are planning-internal
/// and omitted from the public vector.
pub fn project_depends_on_rules(
    deps: &std::collections::BTreeSet<RulePath>,
    catalog_keys: &HashSet<RulePath>,
) -> Vec<RulePath> {
    let mut out = Vec::new();
    for path in deps {
        if catalog_keys.contains(path) {
            out.push(path.clone());
        } else if path.segments.is_empty() {
            panic!(
                "BUG: local depends_on_rules entry '{}' missing from Show.rules keys",
                path.input_key()
            );
        }
    }
    out
}

fn conversion_target_from(target: &SemanticConversionTarget) -> ShowConversionTarget {
    match target {
        SemanticConversionTarget::Type(kind) => ShowConversionTarget::Type(*kind),
        SemanticConversionTarget::Unit { unit_name, .. } => ShowConversionTarget::Unit {
            unit_name: unit_name.clone(),
        },
    }
}

//! Show rule graph: resolved branch expressions projected for [`crate::Engine::show`].
//!
//! Built once from [`crate::planning::graph::RuleNode`] branches during plan build.
//! Not NormalForm and not evaluation narration.

use crate::parsing::ast::{
    ArithmeticComputation, CalendarPeriodUnit, ComparisonComputation, DateCalendarKind,
    DateRelativeKind, MathematicalComputation, PrimitiveKind, VetoExpression,
};
use crate::planning::semantics::{
    Expression, ExpressionKind, SemanticConversionTarget, TypedLiteral,
};
use crate::result_value::{type_scoped_result_value_from_literal, RuleResultValue};
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
    pub branches: Vec<ShowBranch>,
    /// Local rule names this rule depends on (planning topo). Always present.
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShowExpression {
    Literal(Box<RuleResultValue>),
    Data {
        name: String,
    },
    Rule {
        name: String,
    },
    And {
        left: Box<ShowExpression>,
        right: Box<ShowExpression>,
    },
    Not {
        operand: Box<ShowExpression>,
    },
    Arithmetic {
        op: ArithmeticComputation,
        left: Box<ShowExpression>,
        right: Box<ShowExpression>,
    },
    Comparison {
        op: ComparisonComputation,
        left: Box<ShowExpression>,
        right: Box<ShowExpression>,
    },
    UnitConversion {
        operand: Box<ShowExpression>,
        target: ShowConversionTarget,
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
    IsVeto {
        operand: Box<ShowExpression>,
    },
}

/// Project a resolved planning expression into the Show tree.
///
/// `local_rule_names` are local `RulePath.rule` values present on the plan. A local
/// `rule` leaf missing from that set is a bug.
pub fn show_expression_from(
    expression: &Expression,
    local_rule_names: &HashSet<String>,
) -> ShowExpression {
    match &expression.kind {
        ExpressionKind::Literal(typed) => ShowExpression::Literal(Box::new(literal_to_show(typed))),
        ExpressionKind::DataPath(path) => ShowExpression::Data {
            name: path.input_key(),
        },
        ExpressionKind::RulePath(path) => {
            if path.segments.is_empty() && !local_rule_names.contains(&path.rule) {
                panic!(
                    "BUG: local rule leaf '{}' not in plan local rules",
                    path.rule
                );
            }
            ShowExpression::Rule {
                name: path.to_string(),
            }
        }
        ExpressionKind::LogicalAnd(left, right) => ShowExpression::And {
            left: Box::new(show_expression_from(left, local_rule_names)),
            right: Box::new(show_expression_from(right, local_rule_names)),
        },
        ExpressionKind::LogicalNegation(operand, _) => ShowExpression::Not {
            operand: Box::new(show_expression_from(operand, local_rule_names)),
        },
        ExpressionKind::Arithmetic(left, op, right) => ShowExpression::Arithmetic {
            op: op.clone(),
            left: Box::new(show_expression_from(left, local_rule_names)),
            right: Box::new(show_expression_from(right, local_rule_names)),
        },
        ExpressionKind::Comparison(left, op, right) => ShowExpression::Comparison {
            op: op.clone(),
            left: Box::new(show_expression_from(left, local_rule_names)),
            right: Box::new(show_expression_from(right, local_rule_names)),
        },
        ExpressionKind::UnitConversion(operand, target) => ShowExpression::UnitConversion {
            operand: Box::new(show_expression_from(operand, local_rule_names)),
            target: conversion_target_from(target),
        },
        ExpressionKind::MathematicalComputation(op, operand) => ShowExpression::Math {
            op: op.clone(),
            operand: Box::new(show_expression_from(operand, local_rule_names)),
        },
        ExpressionKind::Veto(VetoExpression { message }) => ShowExpression::Veto {
            message: message.clone(),
        },
        ExpressionKind::Now => ShowExpression::Now,
        ExpressionKind::DateRelative(kind, operand) => ShowExpression::DateRelative {
            kind: *kind,
            operand: Box::new(show_expression_from(operand, local_rule_names)),
        },
        ExpressionKind::DateCalendar(kind, unit, operand) => ShowExpression::DateCalendar {
            kind: *kind,
            unit: *unit,
            operand: Box::new(show_expression_from(operand, local_rule_names)),
        },
        ExpressionKind::RangeLiteral(from, to) => ShowExpression::RangeLiteral {
            from: Box::new(show_expression_from(from, local_rule_names)),
            to: Box::new(show_expression_from(to, local_rule_names)),
        },
        ExpressionKind::PastFutureRange(kind, operand) => ShowExpression::PastFutureRange {
            kind: *kind,
            operand: Box::new(show_expression_from(operand, local_rule_names)),
        },
        ExpressionKind::RangeContainment(value, range) => ShowExpression::RangeContainment {
            value: Box::new(show_expression_from(value, local_rule_names)),
            range: Box::new(show_expression_from(range, local_rule_names)),
        },
        ExpressionKind::ResultIsVeto(operand) => ShowExpression::IsVeto {
            operand: Box::new(show_expression_from(operand, local_rule_names)),
        },
        ExpressionKind::Piecewise(_) => {
            panic!("BUG: piecewise inside a rule branch")
        }
    }
}

/// Project `RuleNode.branches` into Show arms (default first, no synthetic true).
pub fn show_branches_from(
    branches: &[(Option<Expression>, Expression)],
    local_rule_names: &HashSet<String>,
) -> Vec<ShowBranch> {
    branches
        .iter()
        .map(|(condition, result)| ShowBranch {
            condition: condition
                .as_ref()
                .map(|expression| show_expression_from(expression, local_rule_names)),
            result: show_expression_from(result, local_rule_names),
        })
        .collect()
}

/// Local names from `RuleNode.depends_on_rules` (BTreeSet order). Imported paths omitted.
pub fn project_depends_on_rules(
    deps: &std::collections::BTreeSet<crate::planning::semantics::RulePath>,
    local_rule_names: &HashSet<String>,
) -> Vec<String> {
    let mut out = Vec::new();
    for path in deps {
        if !path.segments.is_empty() {
            continue;
        }
        if !local_rule_names.contains(&path.rule) {
            panic!(
                "BUG: local depends_on_rules entry '{}' missing from plan local rules",
                path.rule
            );
        }
        out.push(path.rule.clone());
    }
    out
}

fn literal_to_show(typed: &TypedLiteral) -> RuleResultValue {
    type_scoped_result_value_from_literal(&typed.to_literal(), typed.lemma_type.as_ref())
        .unwrap_or_else(|failure| {
            panic!(
                "BUG: show branch literal failed type_scoped_result_value_from_literal: {}",
                crate::result_value::rule_result_value_failure_message(failure)
            )
        })
}

fn conversion_target_from(target: &SemanticConversionTarget) -> ShowConversionTarget {
    match target {
        SemanticConversionTarget::Type(kind) => ShowConversionTarget::Type(*kind),
        SemanticConversionTarget::Unit { unit_name, .. } => ShowConversionTarget::Unit {
            unit_name: unit_name.clone(),
        },
    }
}

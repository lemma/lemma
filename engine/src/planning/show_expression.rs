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
/// `show_keys` are `RulePath::input_key` values for reachable Show rules. A
/// `rule` leaf whose key is missing from that set is a bug.
pub fn show_expression_from(
    expression: &Expression,
    show_keys: &HashSet<String>,
) -> ShowExpression {
    match &expression.kind {
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
        ExpressionKind::LogicalAnd(left, right) => ShowExpression::And {
            left: Box::new(show_expression_from(left, show_keys)),
            right: Box::new(show_expression_from(right, show_keys)),
        },
        ExpressionKind::LogicalNegation(operand, _) => ShowExpression::Not {
            operand: Box::new(show_expression_from(operand, show_keys)),
        },
        ExpressionKind::Arithmetic(left, op, right) => ShowExpression::Arithmetic {
            op: op.clone(),
            left: Box::new(show_expression_from(left, show_keys)),
            right: Box::new(show_expression_from(right, show_keys)),
        },
        ExpressionKind::Comparison(left, op, right) => ShowExpression::Comparison {
            op: op.clone(),
            left: Box::new(show_expression_from(left, show_keys)),
            right: Box::new(show_expression_from(right, show_keys)),
        },
        ExpressionKind::UnitConversion(operand, target) => ShowExpression::UnitConversion {
            operand: Box::new(show_expression_from(operand, show_keys)),
            target: conversion_target_from(target),
        },
        ExpressionKind::MathematicalComputation(op, operand) => ShowExpression::Math {
            op: op.clone(),
            operand: Box::new(show_expression_from(operand, show_keys)),
        },
        ExpressionKind::Veto(VetoExpression { message }) => ShowExpression::Veto {
            message: message.clone(),
        },
        ExpressionKind::Now => ShowExpression::Now,
        ExpressionKind::DateRelative(kind, operand) => ShowExpression::DateRelative {
            kind: *kind,
            operand: Box::new(show_expression_from(operand, show_keys)),
        },
        ExpressionKind::DateCalendar(kind, unit, operand) => ShowExpression::DateCalendar {
            kind: *kind,
            unit: *unit,
            operand: Box::new(show_expression_from(operand, show_keys)),
        },
        ExpressionKind::RangeLiteral(from, to) => ShowExpression::RangeLiteral {
            from: Box::new(show_expression_from(from, show_keys)),
            to: Box::new(show_expression_from(to, show_keys)),
        },
        ExpressionKind::PastFutureRange(kind, operand) => ShowExpression::PastFutureRange {
            kind: *kind,
            operand: Box::new(show_expression_from(operand, show_keys)),
        },
        ExpressionKind::RangeContainment(value, range) => ShowExpression::RangeContainment {
            value: Box::new(show_expression_from(value, show_keys)),
            range: Box::new(show_expression_from(range, show_keys)),
        },
        ExpressionKind::ResultIsVeto(operand) => ShowExpression::IsVeto {
            operand: Box::new(show_expression_from(operand, show_keys)),
        },
        ExpressionKind::Piecewise(_) => {
            panic!("BUG: piecewise inside a rule branch")
        }
    }
}

/// Project `RuleNode.branches` into Show arms (default first, no synthetic true).
pub fn show_branches_from(
    branches: &[(Option<Expression>, Expression)],
    show_keys: &HashSet<String>,
) -> Vec<ShowBranch> {
    branches
        .iter()
        .map(|(condition, result)| ShowBranch {
            condition: condition
                .as_ref()
                .map(|expression| show_expression_from(expression, show_keys)),
            result: show_expression_from(result, show_keys),
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

fn conversion_target_from(target: &SemanticConversionTarget) -> ShowConversionTarget {
    match target {
        SemanticConversionTarget::Type(kind) => ShowConversionTarget::Type(*kind),
        SemanticConversionTarget::Unit { unit_name, .. } => ShowConversionTarget::Unit {
            unit_name: unit_name.clone(),
        },
    }
}

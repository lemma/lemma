//! Show / ShowData / ShowRule / ShowExpression JSON shapes.

use crate::api::types::LemmaType;
use crate::api::value::RuleResultValue;
use crate::literals::Value;
use crate::parsing::ast::{
    ArithmeticComputation, CalendarPeriodUnit, ComparisonComputation, DateCalendarKind,
    DateRelativeKind, DateTimeValue, MathematicalComputation, PrimitiveKind,
};
use crate::parsing::source::SourceType;
use crate::planning::execution_plan::{
    Show as DomainShow, ShowBranch as DomainShowBranch,
    ShowConversionTarget as DomainConversionTarget, ShowData as DomainShowData,
    ShowExpression as DomainShowExpression, ShowRule as DomainShowRule,
    ShowVersion as DomainShowVersion,
};
use crate::planning::semantics::PathSegment as DomainPathSegment;
use crate::result_value::type_scoped_result_value_from_literal;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// One `uses` hop on a Show data or rule path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathSegment {
    pub uses: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub repository: Option<String>,
    pub spec: String,
}

impl From<&DomainPathSegment> for PathSegment {
    fn from(segment: &DomainPathSegment) -> Self {
        Self {
            uses: segment.uses.clone(),
            repository: segment.repository.clone(),
            spec: segment.spec.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShowData {
    #[serde(rename = "type")]
    pub lemma_type: LemmaType,
    pub path: Vec<PathSegment>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub fill: Option<RuleResultValue>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub suggestion: Option<RuleResultValue>,
    pub needed_by_rules: Vec<String>,
}

impl From<&DomainShowData> for ShowData {
    fn from(data: &DomainShowData) -> Self {
        Self {
            lemma_type: LemmaType::from(&data.lemma_type),
            path: data.path.iter().map(PathSegment::from).collect(),
            fill: data.fill.as_ref().map(RuleResultValue::from),
            suggestion: data.suggestion.as_ref().map(RuleResultValue::from),
            needed_by_rules: data.needed_by_rules.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShowVersion {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effective_from: Option<DateTimeValue>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effective_to: Option<DateTimeValue>,
}

impl From<&DomainShowVersion> for ShowVersion {
    fn from(version: &DomainShowVersion) -> Self {
        Self {
            effective_from: version.effective_from.clone(),
            effective_to: version.effective_to.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShowConversionTarget {
    Type(PrimitiveKind),
    Unit { unit_name: String },
}

impl From<&DomainConversionTarget> for ShowConversionTarget {
    fn from(target: &DomainConversionTarget) -> Self {
        match target {
            DomainConversionTarget::Type(kind) => Self::Type(*kind),
            DomainConversionTarget::Unit { unit_name } => Self::Unit {
                unit_name: unit_name.clone(),
            },
        }
    }
}

/// Resolved expression on a Show rule branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ShowExpression {
    Literal {
        #[serde(flatten)]
        value: Box<RuleResultValue>,
    },
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
        #[serde(skip_serializing_if = "Option::is_none", default)]
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

impl From<&DomainShowExpression> for ShowExpression {
    fn from(expression: &DomainShowExpression) -> Self {
        match expression {
            DomainShowExpression::Literal(typed) => {
                let domain_value = type_scoped_result_value_from_literal(
                    &typed.to_literal(),
                    typed.lemma_type.as_ref(),
                )
                .unwrap_or_else(|failure| {
                    panic!(
                        "BUG: show branch literal failed type_scoped_result_value_from_literal: {}",
                        crate::result_value::rule_result_value_failure_message(failure)
                    )
                });
                Self::Literal {
                    value: Box::new(RuleResultValue::from(&domain_value)),
                }
            }
            DomainShowExpression::Data { name } => Self::Data { name: name.clone() },
            DomainShowExpression::Rule { name } => Self::Rule { name: name.clone() },
            DomainShowExpression::And { left, right } => Self::And {
                left: Box::new(ShowExpression::from(left.as_ref())),
                right: Box::new(ShowExpression::from(right.as_ref())),
            },
            DomainShowExpression::Not { operand } => Self::Not {
                operand: Box::new(ShowExpression::from(operand.as_ref())),
            },
            DomainShowExpression::Arithmetic { op, left, right } => Self::Arithmetic {
                op: op.clone(),
                left: Box::new(ShowExpression::from(left.as_ref())),
                right: Box::new(ShowExpression::from(right.as_ref())),
            },
            DomainShowExpression::Comparison { op, left, right } => Self::Comparison {
                op: op.clone(),
                left: Box::new(ShowExpression::from(left.as_ref())),
                right: Box::new(ShowExpression::from(right.as_ref())),
            },
            DomainShowExpression::UnitConversion { operand, target } => Self::UnitConversion {
                operand: Box::new(ShowExpression::from(operand.as_ref())),
                target: ShowConversionTarget::from(target),
            },
            DomainShowExpression::Math { op, operand } => Self::Math {
                op: op.clone(),
                operand: Box::new(ShowExpression::from(operand.as_ref())),
            },
            DomainShowExpression::Veto { message } => Self::Veto {
                message: message.clone(),
            },
            DomainShowExpression::Now => Self::Now,
            DomainShowExpression::DateRelative { kind, operand } => Self::DateRelative {
                kind: *kind,
                operand: Box::new(ShowExpression::from(operand.as_ref())),
            },
            DomainShowExpression::DateCalendar {
                kind,
                unit,
                operand,
            } => Self::DateCalendar {
                kind: *kind,
                unit: *unit,
                operand: Box::new(ShowExpression::from(operand.as_ref())),
            },
            DomainShowExpression::RangeLiteral { from, to } => Self::RangeLiteral {
                from: Box::new(ShowExpression::from(from.as_ref())),
                to: Box::new(ShowExpression::from(to.as_ref())),
            },
            DomainShowExpression::PastFutureRange { kind, operand } => Self::PastFutureRange {
                kind: *kind,
                operand: Box::new(ShowExpression::from(operand.as_ref())),
            },
            DomainShowExpression::RangeContainment { value, range } => Self::RangeContainment {
                value: Box::new(ShowExpression::from(value.as_ref())),
                range: Box::new(ShowExpression::from(range.as_ref())),
            },
            DomainShowExpression::IsVeto { operand } => Self::IsVeto {
                operand: Box::new(ShowExpression::from(operand.as_ref())),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShowBranch {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub condition: Option<ShowExpression>,
    pub result: ShowExpression,
}

impl From<&DomainShowBranch> for ShowBranch {
    fn from(branch: &DomainShowBranch) -> Self {
        Self {
            condition: branch.condition.as_ref().map(ShowExpression::from),
            result: ShowExpression::from(&branch.result),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShowRule {
    #[serde(rename = "type")]
    pub lemma_type: LemmaType,
    pub path: Vec<PathSegment>,
    pub branches: Vec<ShowBranch>,
    pub depends_on_rules: Vec<String>,
}

impl From<&DomainShowRule> for ShowRule {
    fn from(rule: &DomainShowRule) -> Self {
        Self {
            lemma_type: LemmaType::from(&rule.lemma_type),
            path: rule.path.iter().map(PathSegment::from).collect(),
            branches: rule.branches.iter().map(ShowBranch::from).collect(),
            depends_on_rules: rule.depends_on_rules.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Show {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub repository: Option<String>,
    pub spec: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub commentary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effective_from: Option<DateTimeValue>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effective_to: Option<DateTimeValue>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub versions: Vec<ShowVersion>,
    pub start_line: usize,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source_type: Option<SourceType>,
    pub data: IndexMap<String, ShowData>,
    pub rules: IndexMap<String, ShowRule>,
    pub meta: IndexMap<String, Value>,
}

impl From<&DomainShow> for Show {
    fn from(show: &DomainShow) -> Self {
        Self {
            repository: show.repository.clone(),
            spec: show.spec.clone(),
            commentary: show.commentary.clone(),
            effective_from: show.effective_from.clone(),
            effective_to: show.effective_to.clone(),
            versions: show.versions.iter().map(ShowVersion::from).collect(),
            start_line: show.start_line,
            source_type: show.source_type.clone(),
            data: show
                .data
                .iter()
                .map(|(name, data)| (name.clone(), ShowData::from(data)))
                .collect(),
            rules: show
                .rules
                .iter()
                .map(|(name, rule)| (name.clone(), ShowRule::from(rule)))
                .collect(),
            meta: show.meta.clone(),
        }
    }
}

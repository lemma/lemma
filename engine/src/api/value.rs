//! ValueKind / LiteralValue / RuleResultValue JSON shapes.

use crate::planning::semantics::{
    LiteralValue as DomainLiteralValue, SemanticDateTime, SemanticTime,
    ValueKind as DomainValueKind,
};
use crate::result_value::{
    CalendarResult as DomainCalendarResult, RangeResult as DomainRangeResult,
    RuleResultValue as DomainRuleResultValue,
};
use indexmap::IndexMap;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Externally tagged value payload matching today's `ValueKind` Serialize shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ValueKind {
    Number(Decimal),
    Measure {
        value: Decimal,
    },
    Ratio {
        value: Decimal,
    },
    Text(String),
    Date(SemanticDateTime),
    Time(SemanticTime),
    Boolean(bool),
    Range {
        from: Box<ValueKind>,
        to: Box<ValueKind>,
    },
}

impl ValueKind {
    fn from_non_range(value: &DomainValueKind) -> Self {
        match value {
            DomainValueKind::Number(rational) => Self::Number(
                rational
                    .try_to_decimal()
                    .expect("BUG: planned number must convert to decimal"),
            ),
            DomainValueKind::Measure(rational) => Self::Measure {
                value: rational
                    .try_to_decimal()
                    .expect("BUG: planned measure must convert to decimal"),
            },
            DomainValueKind::Ratio(rational) => Self::Ratio {
                value: rational
                    .try_to_decimal()
                    .expect("BUG: planned ratio must convert to decimal"),
            },
            DomainValueKind::Text(text) => Self::Text(text.clone()),
            DomainValueKind::Date(date) => Self::Date(date.clone()),
            DomainValueKind::Time(time) => Self::Time(time.clone()),
            DomainValueKind::Boolean(flag) => Self::Boolean(*flag),
            DomainValueKind::Range(_, _) => {
                unreachable!("BUG: range payloads are converted by ValueKind::from")
            }
            DomainValueKind::Spec(_) => {
                unreachable!("BUG: spec values are rule results, not nested ValueKind payloads")
            }
            DomainValueKind::List(_) => {
                unreachable!("BUG: list values are rule results, not nested ValueKind payloads")
            }
        }
    }
}

enum ValueKindTask<'a> {
    Visit(&'a DomainValueKind),
    BuildRange,
}

impl From<&DomainValueKind> for ValueKind {
    fn from(value: &DomainValueKind) -> Self {
        let mut tasks = vec![ValueKindTask::Visit(value)];
        let mut converted: Vec<ValueKind> = Vec::new();
        while let Some(task) = tasks.pop() {
            match task {
                ValueKindTask::Visit(DomainValueKind::Range(from, to)) => {
                    tasks.push(ValueKindTask::BuildRange);
                    tasks.push(ValueKindTask::Visit(&to.value));
                    tasks.push(ValueKindTask::Visit(&from.value));
                }
                ValueKindTask::Visit(other) => converted.push(ValueKind::from_non_range(other)),
                ValueKindTask::BuildRange => {
                    let to = converted
                        .pop()
                        .expect("BUG: range endpoint converted before build");
                    let from = converted
                        .pop()
                        .expect("BUG: range endpoint converted before build");
                    converted.push(Self::Range {
                        from: Box::new(from),
                        to: Box::new(to),
                    });
                }
            }
        }
        converted
            .pop()
            .expect("BUG: value conversion yields exactly one ValueKind")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiteralValue {
    pub value: ValueKind,
}

impl From<&DomainLiteralValue> for LiteralValue {
    fn from(literal: &DomainLiteralValue) -> Self {
        Self {
            value: ValueKind::from(&literal.value),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CalendarResult {
    pub value: Decimal,
    pub unit: String,
}

impl From<&DomainCalendarResult> for CalendarResult {
    fn from(calendar: &DomainCalendarResult) -> Self {
        Self {
            value: calendar.value,
            unit: calendar.unit.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RangeResult {
    pub from: RuleResultValue,
    pub to: RuleResultValue,
}

impl From<&DomainRangeResult> for RangeResult {
    fn from(range: &DomainRangeResult) -> Self {
        Self {
            from: RuleResultValue::from(&range.from),
            to: RuleResultValue::from(&range.to),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct RuleResultValue {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measure: Option<BTreeMap<String, Decimal>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ratio: Option<BTreeMap<String, Decimal>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boolean: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<SemanticDateTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<SemanticTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar: Option<CalendarResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<Box<RangeResult>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec: Option<Box<crate::result_value::SpecResult>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row: Option<crate::result_value::RowReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list: Option<Vec<RuleResultValue>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<IndexMap<String, RuleResultValue>>,
    /// Show only: on a source-row field whose value is a reference; the input key
    /// of the referenced data in the spec declaring the list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

/// A converted rule result value waiting for its children: range endpoints, list rows and
/// field values, in that order, are the `child_count` results on top of the converted stack.
struct RuleResultFrame {
    shell: RuleResultValue,
    has_range: bool,
    list_len: Option<usize>,
    field_names: Option<Vec<String>>,
    child_count: usize,
}

enum RuleResultTask<'a> {
    Visit(&'a DomainRuleResultValue),
    Assemble(Box<RuleResultFrame>),
}

impl From<&DomainRuleResultValue> for RuleResultValue {
    fn from(value: &DomainRuleResultValue) -> Self {
        let mut tasks = vec![RuleResultTask::Visit(value)];
        let mut converted: Vec<RuleResultValue> = Vec::new();
        while let Some(task) = tasks.pop() {
            match task {
                RuleResultTask::Visit(value) => {
                    let mut children: Vec<&DomainRuleResultValue> = Vec::new();
                    if let Some(range) = &value.range {
                        children.push(&range.from);
                        children.push(&range.to);
                    }
                    let list_len = value.list.as_ref().map(|rows| {
                        children.extend(rows.iter());
                        rows.len()
                    });
                    let field_names = value.fields.as_ref().map(|fields| {
                        children.extend(fields.values());
                        fields.keys().cloned().collect()
                    });
                    let frame = RuleResultFrame {
                        shell: RuleResultValue {
                            result: value.result.clone(),
                            measure: value.measure.clone(),
                            ratio: value.ratio.clone(),
                            number: value.number,
                            boolean: value.boolean,
                            text: value.text.clone(),
                            date: value.date.clone(),
                            time: value.time.clone(),
                            calendar: value.calendar.as_ref().map(CalendarResult::from),
                            unit: value.unit.clone(),
                            range: None,
                            spec: value.spec.clone(),
                            row: value.row.clone(),
                            list: None,
                            fields: None,
                            reference: value.reference.clone(),
                        },
                        has_range: value.range.is_some(),
                        list_len,
                        field_names,
                        child_count: children.len(),
                    };
                    tasks.push(RuleResultTask::Assemble(Box::new(frame)));
                    tasks.extend(children.into_iter().rev().map(RuleResultTask::Visit));
                }
                RuleResultTask::Assemble(frame) => {
                    let frame = *frame;
                    let mut children = converted
                        .split_off(converted.len() - frame.child_count)
                        .into_iter();
                    let mut shell = frame.shell;
                    if frame.has_range {
                        let from = children
                            .next()
                            .expect("BUG: range value has a from endpoint");
                        let to = children.next().expect("BUG: range value has a to endpoint");
                        shell.range = Some(Box::new(RangeResult { from, to }));
                    }
                    if let Some(len) = frame.list_len {
                        shell.list = Some(children.by_ref().take(len).collect());
                    }
                    if let Some(names) = frame.field_names {
                        shell.fields = Some(names.into_iter().zip(children).collect());
                    }
                    converted.push(shell);
                }
            }
        }
        converted
            .pop()
            .expect("BUG: rule result value conversion yields exactly one value")
    }
}

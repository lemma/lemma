//! ValueKind / LiteralValue / RuleResultValue JSON shapes.

use crate::planning::semantics::{
    LiteralValue as DomainLiteralValue, SemanticDateTime, SemanticTime,
    ValueKind as DomainValueKind,
};
use crate::result_value::{
    CalendarResult as DomainCalendarResult, RangeResult as DomainRangeResult,
    RuleResultValue as DomainRuleResultValue,
};
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

impl From<&DomainValueKind> for ValueKind {
    fn from(value: &DomainValueKind) -> Self {
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
            DomainValueKind::Range(from, to) => Self::Range {
                from: Box::new(ValueKind::from(&from.value)),
                to: Box::new(ValueKind::from(&to.value)),
            },
        }
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
    pub range: Option<Box<RangeResult>>,
}

impl From<&DomainRuleResultValue> for RuleResultValue {
    fn from(value: &DomainRuleResultValue) -> Self {
        Self {
            result: value.result.clone(),
            measure: value.measure.clone(),
            ratio: value.ratio.clone(),
            number: value.number,
            boolean: value.boolean,
            text: value.text.clone(),
            date: value.date.clone(),
            time: value.time.clone(),
            calendar: value.calendar.as_ref().map(CalendarResult::from),
            range: value
                .range
                .as_ref()
                .map(|range| Box::new(RangeResult::from(range.as_ref()))),
        }
    }
}

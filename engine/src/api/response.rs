//! Response / RuleResult JSON shapes.

use crate::api::value::RuleResultValue;
use crate::evaluation::evaluate_data::EvaluateDataValue;
use crate::evaluation::explanations::Explanation;
use crate::evaluation::response::{Response as DomainResponse, RuleResult as DomainRuleResult};
use crate::parsing::ast::DateTimeValue;
use crate::result_value::RowResult;
use indexmap::IndexMap;
use serde::Serialize;

fn evaluate_data_json(value: &EvaluateDataValue) -> serde_json::Value {
    enum Step<'a> {
        Descend(&'a EvaluateDataValue),
        NextChild,
        Deliver(serde_json::Value),
    }
    enum Frame<'a> {
        List {
            out: Vec<serde_json::Value>,
            children: std::slice::Iter<'a, EvaluateDataValue>,
        },
        Map {
            out: serde_json::Map<String, serde_json::Value>,
            pending_key: Option<String>,
            children: indexmap::map::Iter<'a, String, EvaluateDataValue>,
        },
    }

    let mut frames: Vec<Frame<'_>> = Vec::new();
    let mut step = Step::Descend(value);
    loop {
        step = match step {
            Step::Descend(node) => match node {
                EvaluateDataValue::Literal(text) => {
                    Step::Deliver(serde_json::Value::String(text.clone()))
                }
                EvaluateDataValue::Boolean(bit) => Step::Deliver(serde_json::Value::Bool(*bit)),
                EvaluateDataValue::List(items) => {
                    frames.push(Frame::List {
                        out: Vec::with_capacity(items.len()),
                        children: items.iter(),
                    });
                    Step::NextChild
                }
                EvaluateDataValue::Map(entries) => {
                    frames.push(Frame::Map {
                        out: serde_json::Map::with_capacity(entries.len()),
                        pending_key: None,
                        children: entries.iter(),
                    });
                    Step::NextChild
                }
            },
            Step::NextChild => {
                let child = match frames.last_mut() {
                    Some(Frame::List { children, .. }) => children.next(),
                    Some(Frame::Map {
                        pending_key,
                        children,
                        ..
                    }) => children.next().map(|(key, child)| {
                        *pending_key = Some(key.clone());
                        child
                    }),
                    None => {
                        panic!("BUG: evaluate data json frame stack empty while reading a child")
                    }
                };
                match child {
                    Some(child) => Step::Descend(child),
                    None => {
                        let finished = frames
                            .pop()
                            .expect("BUG: evaluate data json frame stack empty on completion");
                        let done = match finished {
                            Frame::List { out, .. } => serde_json::Value::Array(out),
                            Frame::Map { out, .. } => serde_json::Value::Object(out),
                        };
                        Step::Deliver(done)
                    }
                }
            }
            Step::Deliver(done) => match frames.last_mut() {
                None => return done,
                Some(Frame::List { out, .. }) => {
                    out.push(done);
                    Step::NextChild
                }
                Some(Frame::Map {
                    out, pending_key, ..
                }) => {
                    let key = pending_key
                        .take()
                        .expect("BUG: nested evaluate data json value has no field name");
                    out.insert(key, done);
                    Step::NextChild
                }
            },
        };
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Response {
    #[serde(rename = "spec")]
    pub spec_name: String,
    pub effective: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec_effective_from: Option<DateTimeValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec_effective_to: Option<DateTimeValue>,
    pub results: IndexMap<String, RuleResult>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rows: Vec<RowResult>,
}

impl From<&DomainResponse> for Response {
    fn from(response: &DomainResponse) -> Self {
        Self {
            spec_name: response.spec_name.clone(),
            effective: response.effective.clone(),
            spec_effective_from: response.spec_effective_from.clone(),
            spec_effective_to: response.spec_effective_to.clone(),
            results: response
                .results
                .iter()
                .map(|(name, result)| (name.clone(), RuleResult::from(result)))
                .collect(),
            rows: response.rows.clone(),
        }
    }
}

/// Rule result JSON: flattened value fields, no rule/veto_detail.
#[derive(Debug, Clone, Serialize)]
pub struct RuleResult {
    pub vetoed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub veto_reason: Option<String>,
    pub rule_type: String,
    #[serde(flatten)]
    pub result: Option<RuleResultValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explanation: Option<Explanation>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub missing_data: Vec<String>,
    /// Closed options for keys in [`Self::missing_data`] on this run.
    #[serde(skip_serializing_if = "IndexMap::is_empty")]
    pub missing_data_options: IndexMap<String, Vec<serde_json::Value>>,
}

impl From<&DomainRuleResult> for RuleResult {
    fn from(result: &DomainRuleResult) -> Self {
        Self {
            vetoed: result.vetoed,
            veto_reason: result.veto_reason.clone(),
            rule_type: result.rule_type.clone(),
            result: result.result.as_ref().map(RuleResultValue::from),
            explanation: result.explanation.clone(),
            missing_data: result.missing_data().to_vec(),
            missing_data_options: result
                .missing_data_options()
                .iter()
                .map(|(key, options)| {
                    (
                        key.clone(),
                        options.iter().map(evaluate_data_json).collect(),
                    )
                })
                .collect(),
        }
    }
}

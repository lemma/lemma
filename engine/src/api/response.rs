//! Response / RuleResult JSON shapes.

use crate::api::value::RuleResultValue;
use crate::evaluation::explanations::Explanation;
use crate::evaluation::response::{Response as DomainResponse, RuleResult as DomainRuleResult};
use crate::parsing::ast::DateTimeValue;
use indexmap::IndexMap;
use serde::Serialize;

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
        }
    }
}

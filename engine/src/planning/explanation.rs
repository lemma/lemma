//! Explanation API types built by narration after evaluation.
//!
//! Planning ships THE DAG (`NormalForm` nodes with optional fold `origin`).
//! Narration ([`crate::evaluation::narration`]) reads the filled value table,
//! follows origins so rewritten cells display their pre-image, and fills these
//! nodes for the response.
//!
//! Each narrated rule is stored once in an arena and referenced by
//! [`ExplanationNode::RuleRef`]. The public [`Explanation`] carries a `rules`
//! table of reachable rules with compact [`ExplainedRuleId`]s.
//!
//! Bound data narration is `Data` with a flattened [`RuleResultValue`] (result
//! plus typed fields). Structural mentions of paths that were never looked up
//! are `DataUnused` (no value fields), distinct from a Missing-data veto on a
//! visited leaf.

use crate::parsing::ast::ArithmeticComputation;
use crate::planning::semantics::{DataPath, RulePath};
use crate::result_value::RuleResultValue;
use serde::{Serialize, Serializer};

pub(crate) fn serialize_rule_path_as_name<S>(
    path: &RulePath,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&path.input_key())
}

pub(crate) fn serialize_data_path_as_name<S>(
    path: &DataPath,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&path.input_key())
}

/// Compact id of a rule in an [`crate::evaluation::explanations::Explanation`]'s
/// `rules` table. Assigned in first-reach order from the root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct ExplainedRuleId(pub u32);

/// One narrated rule body stored in an explanation's `rules` table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExplainedRule {
    pub id: ExplainedRuleId,
    #[serde(serialize_with = "serialize_rule_path_as_name")]
    pub name: RulePath,
    #[serde(flatten)]
    pub result: RuleResultValue,
    pub body: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub causes: Vec<Cause>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<ExplanationNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExplanationNode {
    /// Reference to a rule in the enclosing explanation's `rules` table.
    RuleRef {
        id: ExplainedRuleId,
        #[serde(serialize_with = "serialize_rule_path_as_name")]
        name: RulePath,
        #[serde(flatten)]
        result: RuleResultValue,
    },
    Compose {
        expression: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        operator: Option<ArithmeticComputation>,
        operands: Vec<ExplanationNode>,
    },
    /// Evaluated / bound data narration. Result fields flatten like [`crate::evaluation::response::RuleResult`].
    Data {
        #[serde(serialize_with = "serialize_data_path_as_name")]
        name: DataPath,
        #[serde(flatten)]
        result: RuleResultValue,
    },
    /// Structural mention of a data path that was not looked up for this cause
    /// (short-circuit skip or static record narration without a binding).
    DataUnused {
        #[serde(serialize_with = "serialize_data_path_as_name")]
        name: DataPath,
    },
    Conversion {
        expression: String,
        steps: Vec<SerializedConversionTraceStep>,
        operands: Vec<ExplanationNode>,
    },
    Veto {
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    /// One instance-list row the enclosing command read.
    Row {
        number: usize,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        appended: bool,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        key: Vec<ExplanationNode>,
        fields: Vec<ExplanationNode>,
    },
    /// Summary of a `where` filter: how many of the input rows matched.
    Filter {
        condition: String,
        matched: usize,
        total: usize,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        children: Vec<ExplanationNode>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Cause {
    pub condition: String,
    pub value: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<ExplanationNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversionTraceRole {
    Outcome,
    Rule,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SerializedConversionTraceStep {
    pub(crate) role: ConversionTraceRole,
    pub(crate) text: String,
}

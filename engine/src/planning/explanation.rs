//! Explanation API types built by narration after evaluation.
//!
//! Planning ships THE DAG (`NormalForm` nodes with optional fold `origin`).
//! Narration ([`crate::evaluation::narration`]) reads the filled value table,
//! follows origins so rewritten cells display their pre-image, and fills these
//! nodes for the response.
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExplanationNode {
    Rule {
        #[serde(serialize_with = "serialize_rule_path_as_name")]
        name: RulePath,
        #[serde(flatten)]
        result: RuleResultValue,
        body: String,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        causes: Vec<Cause>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        children: Vec<ExplanationNode>,
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
    Source,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SerializedConversionTraceStep {
    pub(crate) role: ConversionTraceRole,
    pub(crate) text: String,
}

//! Show document (a JSON Schema for evaluation-data inputs) and the ShowRule / ShowExpression
//! shapes under its `x-lemma-rules`.

use crate::api::types::LemmaType;
use crate::api::value::RuleResultValue;
use crate::parsing::ast::{
    ArithmeticComputation, CalendarPeriodUnit, ComparisonComputation, ConversionTypeKind,
    DateCalendarKind, DateRelativeKind, MathematicalComputation, RangeBound, RoundingMode,
};
use crate::planning::execution_plan::{
    ExecutionPlan, Show as DomainShow, ShowBranch as DomainShowBranch,
    ShowConversionTarget as DomainConversionTarget, ShowData as DomainShowData,
    ShowExpression as DomainShowExpression, ShowRule as DomainShowRule,
};
pub use crate::planning::execution_plan::{ShowListCommand, SortDirection};
use crate::planning::semantics::{
    LemmaType as DomainLemmaType, PathSegment as DomainPathSegment, SpecIdentity,
    TypeSpecification as DomainTypeSpecification,
};
use crate::planning::show_expression::take_operand;
use crate::result_value::{
    type_scoped_result_value_from_literal, RuleResultValue as DomainRuleResultValue,
};
use crate::Engine;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value as JsonValue};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// One hop on a Show data or rule path: a `uses` alias, or (with `row`) a row written
/// in the data list named `uses`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathSegment {
    pub uses: String,
    /// Row number (1-based) when this hop is a spec-written row of list `uses`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub row: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub repository: Option<String>,
    pub spec: String,
}

impl From<&DomainPathSegment> for PathSegment {
    fn from(segment: &DomainPathSegment) -> Self {
        Self {
            uses: segment.uses.to_string(),
            row: segment.row,
            repository: segment.repository.as_ref().map(|arc| arc.to_string()),
            spec: segment.spec.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShowConversionTarget {
    Type(ConversionTypeKind),
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
        operands: Vec<ShowExpression>,
    },
    Or {
        operands: Vec<ShowExpression>,
    },
    Not {
        operand: Box<ShowExpression>,
    },
    Arithmetic {
        first: Box<ShowExpression>,
        steps: Vec<ShowArithmeticStep>,
    },
    Comparison {
        op: ComparisonComputation,
        left: Box<ShowExpression>,
        right: Box<ShowExpression>,
    },
    UnitConversion {
        operand: Box<ShowExpression>,
        targets: Vec<ShowConversionTarget>,
    },
    Math {
        op: MathematicalComputation,
        operand: Box<ShowExpression>,
    },
    StepRounding {
        mode: RoundingMode,
        operand: Box<ShowExpression>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        step: Option<Box<ShowExpression>>,
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
    RangeBound {
        bound: RangeBound,
        operand: Box<ShowExpression>,
    },
    IsVeto {
        operand: Box<ShowExpression>,
    },
    SpecMember {
        base: String,
        member: String,
    },
    Member {
        base: Box<ShowExpression>,
        member: String,
    },
    RowField {
        field: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        qualifier: Option<Vec<String>>,
    },
    RowValue,
    RowFieldValue {
        field: String,
        value: Box<ShowExpression>,
    },
    ListCommand {
        list: Box<ShowExpression>,
        command: ShowListCommand,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        direction: Option<SortDirection>,
        args: Vec<ShowExpression>,
    },
}

/// One operator step of an `arithmetic` node: `op operand`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShowArithmeticStep {
    pub op: ArithmeticComputation,
    pub operand: ShowExpression,
}

impl From<&DomainShowExpression> for ShowExpression {
    fn from(expression: &DomainShowExpression) -> Self {
        expression.fold(convert_show_node)
    }
}

/// One API node from its domain node and the API operands already converted, in
/// [`DomainShowExpression::operands`] order.
fn convert_show_node(node: &DomainShowExpression, operands: Vec<ShowExpression>) -> ShowExpression {
    let mut operands = operands.into_iter();
    match node {
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
            ShowExpression::Literal {
                value: Box::new(RuleResultValue::from(&domain_value)),
            }
        }
        DomainShowExpression::Data { name } => ShowExpression::Data { name: name.clone() },
        DomainShowExpression::Rule { name } => ShowExpression::Rule { name: name.clone() },
        DomainShowExpression::And { .. } => ShowExpression::And {
            operands: operands.collect(),
        },
        DomainShowExpression::Or { .. } => ShowExpression::Or {
            operands: operands.collect(),
        },
        DomainShowExpression::Not { .. } => ShowExpression::Not {
            operand: Box::new(take_operand(&mut operands)),
        },
        DomainShowExpression::Arithmetic { steps, .. } => {
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
        DomainShowExpression::Comparison { op, .. } => {
            let left = Box::new(take_operand(&mut operands));
            let right = Box::new(take_operand(&mut operands));
            ShowExpression::Comparison {
                op: op.clone(),
                left,
                right,
            }
        }
        DomainShowExpression::UnitConversion { targets, .. } => ShowExpression::UnitConversion {
            operand: Box::new(take_operand(&mut operands)),
            targets: targets.iter().map(ShowConversionTarget::from).collect(),
        },
        DomainShowExpression::Math { op, .. } => ShowExpression::Math {
            op: op.clone(),
            operand: Box::new(take_operand(&mut operands)),
        },
        DomainShowExpression::StepRounding { mode, step, .. } => {
            let operand = Box::new(take_operand(&mut operands));
            ShowExpression::StepRounding {
                mode: *mode,
                operand,
                step: step.as_ref().map(|_| Box::new(take_operand(&mut operands))),
            }
        }
        DomainShowExpression::Veto { message } => ShowExpression::Veto {
            message: message.clone(),
        },
        DomainShowExpression::Now => ShowExpression::Now,
        DomainShowExpression::DateRelative { kind, .. } => ShowExpression::DateRelative {
            kind: *kind,
            operand: Box::new(take_operand(&mut operands)),
        },
        DomainShowExpression::DateCalendar { kind, unit, .. } => ShowExpression::DateCalendar {
            kind: *kind,
            unit: *unit,
            operand: Box::new(take_operand(&mut operands)),
        },
        DomainShowExpression::RangeLiteral { .. } => {
            let from = Box::new(take_operand(&mut operands));
            let to = Box::new(take_operand(&mut operands));
            ShowExpression::RangeLiteral { from, to }
        }
        DomainShowExpression::PastFutureRange { kind, .. } => ShowExpression::PastFutureRange {
            kind: *kind,
            operand: Box::new(take_operand(&mut operands)),
        },
        DomainShowExpression::RangeContainment { .. } => {
            let value = Box::new(take_operand(&mut operands));
            let range = Box::new(take_operand(&mut operands));
            ShowExpression::RangeContainment { value, range }
        }
        DomainShowExpression::RangeBound { bound, .. } => ShowExpression::RangeBound {
            bound: *bound,
            operand: Box::new(take_operand(&mut operands)),
        },
        DomainShowExpression::IsVeto { .. } => ShowExpression::IsVeto {
            operand: Box::new(take_operand(&mut operands)),
        },
        DomainShowExpression::SpecMember { base, member } => ShowExpression::SpecMember {
            base: base.clone(),
            member: member.clone(),
        },
        DomainShowExpression::Member { member, .. } => ShowExpression::Member {
            base: Box::new(take_operand(&mut operands)),
            member: member.clone(),
        },
        DomainShowExpression::RowField { field, qualifier } => ShowExpression::RowField {
            field: field.clone(),
            qualifier: qualifier.clone(),
        },
        DomainShowExpression::RowValue => ShowExpression::RowValue,
        DomainShowExpression::RowFieldValue { field, .. } => ShowExpression::RowFieldValue {
            field: field.clone(),
            value: Box::new(take_operand(&mut operands)),
        },
        DomainShowExpression::ListCommand {
            command, direction, ..
        } => {
            let list = Box::new(take_operand(&mut operands));
            ShowExpression::ListCommand {
                list,
                command: *command,
                direction: *direction,
                args: operands.collect(),
            }
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
            depends_on_rules: rule
                .depends_on_rules
                .iter()
                .map(|path| path.input_key())
                .collect(),
        }
    }
}

const JSON_SCHEMA_DIALECT: &str = "https://json-schema.org/draft/2020-12/schema";

/// Run-data boolean text (`BooleanValue::from_str`): case-insensitive, surrounding whitespace.
const BOOLEAN_TEXT_PATTERN: &str =
    r"^\s*(?:[Tt][Rr][Uu][Ee]|[Ff][Aa][Ll][Ss][Ee]|[Yy][Ee][Ss]|[Nn][Oo])\s*$";

/// The Show document: a Draft 2020-12 JSON Schema for the spec's evaluation-data inputs.
///
/// Standard keywords carry what JSON Schema expresses (`type`, `enum`, `minLength` /
/// `maxLength`, `format`, `default`, `examples`, `items`, ...; `pattern` only on the
/// boolean string alternative). Date inputs are `oneOf` `date` | `date-time`; time
/// uses `format: time`. Number, measure, ratio and ranges are plain strings; the
/// engine validates literals on evaluate. `x-lemma-*` keywords carry the rest,
/// including this spec's own rules under `x-lemma-rules`. `$defs` holds one row object
/// schema per instance-list row spec.
pub fn show_document(engine: &Engine, show: &DomainShow) -> JsonValue {
    let consumer = match engine.plan_matching_show(show) {
        Some(plan) => Some(plan),
        None if !show_has_instance_list(show) => None,
        None => panic!(
            "BUG: Show '{}' has instance-list inputs but no matching execution plan",
            show.spec
        ),
    };
    let mut rows = RowSchemas::new(engine, consumer);
    let mut document = Map::new();
    document.insert("$schema".to_string(), json!(JSON_SCHEMA_DIALECT));
    document.insert("title".to_string(), json!(show.spec));
    if let Some(commentary) = &show.commentary {
        document.insert("description".to_string(), json!(commentary));
    }
    document.extend(input_object_schema(show, &mut rows));
    let (defs, row_field_needed_by_rules) = rows.into_defs();
    if !defs.is_empty() {
        document.insert("$defs".to_string(), JsonValue::Object(defs));
    }
    if !row_field_needed_by_rules.is_empty() {
        // Parent-specific: which of this Show's rules need each `$defs` row field.
        // Kept off `$defs` so `LemmaRow_*` schemas are identical across Show documents
        // (OpenAPI shares them by row identity).
        document.insert(
            "x-lemma-row-field-needed-by-rules".to_string(),
            JsonValue::Object(row_field_needed_by_rules),
        );
    }
    if let Some(repository) = &show.repository {
        document.insert("x-lemma-repository".to_string(), json!(repository));
    }
    if let Some(from) = &show.effective_from {
        document.insert("x-lemma-effective-from".to_string(), to_json(from));
    }
    if let Some(to) = &show.effective_to {
        document.insert("x-lemma-effective-to".to_string(), to_json(to));
    }
    document.insert("x-lemma-start-line".to_string(), json!(show.start_line));
    if let Some(source_type) = &show.source_type {
        document.insert("x-lemma-source-type".to_string(), to_json(source_type));
    }
    document.insert("x-lemma-meta".to_string(), to_json(&show.meta));
    let types: IndexMap<&String, LemmaType> = show
        .types
        .iter()
        .map(|(name, lemma_type)| (name, LemmaType::from(lemma_type)))
        .collect();
    document.insert("x-lemma-types".to_string(), to_json(&types));
    let rules: IndexMap<&String, ShowRule> = show
        .rules
        .iter()
        .map(|(name, rule)| (name, ShowRule::from(rule)))
        .collect();
    document.insert("x-lemma-rules".to_string(), to_json(&rules));
    JsonValue::Object(document)
}

fn to_json<T: Serialize>(value: &T) -> JsonValue {
    serde_json::to_value(value).expect("BUG: Show document parts serialize to JSON")
}

fn show_has_instance_list(show: &DomainShow) -> bool {
    show.data.values().any(|data| {
        matches!(
            &data.lemma_type.specifications,
            DomainTypeSpecification::List { element, .. }
                if matches!(&element.specifications, DomainTypeSpecification::Spec { .. })
        )
    })
}

/// `type`, `additionalProperties`, `properties` and `required` for the data of `show`.
/// `required`: data a rule needs that has no committed value. Lists are never required:
/// an absent list is empty.
fn input_object_schema(show: &DomainShow, rows: &mut RowSchemas) -> Map<String, JsonValue> {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for (name, data) in &show.data {
        properties.insert(
            name.clone(),
            property_schema(data, rows, &data.needed_by_rules),
        );
        let is_list = matches!(
            data.lemma_type.specifications,
            DomainTypeSpecification::List { .. }
        );
        if data.value.is_none() && !data.needed_by_rules.is_empty() && !is_list {
            required.push(name.clone());
        }
    }
    required.sort();
    let mut schema = Map::new();
    schema.insert("type".to_string(), json!("object"));
    schema.insert("additionalProperties".to_string(), json!(true));
    schema.insert("properties".to_string(), JsonValue::Object(properties));
    if !required.is_empty() {
        schema.insert("required".to_string(), json!(required));
    }
    schema
}

/// Row `$defs` object: local types, plus any other `show.data` slots on the row
/// (imported fields, local instance lists).
///
/// `x-lemma-needed-by-rules` on these properties is always empty: parent consumers
/// differ per Show document, and `$defs` are keyed by row identity so every Show
/// must render the same `LemmaRow_*` schema. Parent field consumers live on the
/// document as `x-lemma-row-field-needed-by-rules`.
fn row_object_schema(show: &DomainShow, rows: &mut RowSchemas) -> Map<String, JsonValue> {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for (name, lemma_type) in &show.types {
        let stub;
        let entry = match show.data.get(name) {
            Some(data) => data,
            None => {
                stub = DomainShowData {
                    lemma_type: lemma_type.clone(),
                    path: Vec::new(),
                    value: None,
                    suggestion: None,
                    needed_by_rules: Vec::new(),
                };
                &stub
            }
        };
        let is_list = matches!(
            entry.lemma_type.specifications,
            DomainTypeSpecification::List { .. }
        );
        if entry.value.is_none() && !is_list {
            required.push(name.clone());
        }
        properties.insert(name.clone(), property_schema(entry, rows, &[]));
    }
    for (name, data) in &show.data {
        if properties.contains_key(name) {
            continue;
        }
        let is_list = matches!(
            data.lemma_type.specifications,
            DomainTypeSpecification::List { .. }
        );
        if data.value.is_none() && !is_list {
            required.push(name.clone());
        }
        properties.insert(name.clone(), property_schema(data, rows, &[]));
    }
    required.sort();
    let mut schema = Map::new();
    schema.insert("type".to_string(), json!("object"));
    schema.insert("additionalProperties".to_string(), json!(true));
    schema.insert("properties".to_string(), JsonValue::Object(properties));
    if !required.is_empty() {
        schema.insert("required".to_string(), json!(required));
    }
    schema
}

/// Property names of a row `$defs` schema: local types, then any other `show.data` keys.
fn row_property_names(show: &DomainShow) -> BTreeSet<String> {
    let mut names: BTreeSet<String> = show.types.keys().cloned().collect();
    names.extend(show.data.keys().cloned());
    names
}

fn property_schema(
    data: &DomainShowData,
    rows: &mut RowSchemas,
    needed_by_rules: &[String],
) -> JsonValue {
    let mut schema = type_schema(&data.lemma_type, rows);
    let help = specification_help(&data.lemma_type.specifications);
    if !help.is_empty() {
        schema.insert("description".to_string(), json!(help));
    }
    if let Some(value) = &data.value {
        // A reference field is wiring, not a run-data literal. Emitting a partial
        // default would replace source rows and drop those bindings.
        if !rule_result_value_has_reference(value) {
            schema.insert(
                "default".to_string(),
                input_literal(value, &data.lemma_type, rows),
            );
        }
    }
    if let Some(suggestion) = &data.suggestion {
        schema.insert(
            "examples".to_string(),
            json!([input_literal(suggestion, &data.lemma_type, rows)]),
        );
    }
    schema.insert(
        "x-lemma-type".to_string(),
        lemma_type_without_standard_keywords(&data.lemma_type),
    );
    let path: Vec<PathSegment> = data.path.iter().map(PathSegment::from).collect();
    schema.insert("x-lemma-path".to_string(), to_json(&path));
    schema.insert(
        "x-lemma-needed-by-rules".to_string(),
        json!(needed_by_rules),
    );
    if let Some(value) = &data.value {
        schema.insert(
            "x-lemma-value".to_string(),
            to_json(&RuleResultValue::from(value)),
        );
    }
    if let Some(suggestion) = &data.suggestion {
        schema.insert(
            "x-lemma-suggestion".to_string(),
            to_json(&RuleResultValue::from(suggestion)),
        );
    }
    JsonValue::Object(schema)
}

/// Standard keywords for one type. Lists are a JSON array: rows of an instance list are
/// objects (`$defs` row schema), items of a value list are the element's literal.
fn type_schema(lemma_type: &DomainLemmaType, rows: &mut RowSchemas) -> Map<String, JsonValue> {
    let DomainTypeSpecification::List { element, .. } = &lemma_type.specifications else {
        return scalar_data_schema(lemma_type);
    };
    let mut schema = Map::new();
    let items = match &element.specifications {
        DomainTypeSpecification::Spec { spec } => rows.reference(spec),
        DomainTypeSpecification::List { .. } => {
            unreachable!("BUG: lists do not nest; planning rejects a list element list")
        }
        _ => JsonValue::Object(scalar_data_schema(element)),
    };
    schema.insert("type".to_string(), json!("array"));
    schema.insert("items".to_string(), items);
    schema.insert(
        "maxItems".to_string(),
        json!(rows.engine.limits().max_list_rows),
    );
    schema
}

/// A scalar data slot. Measure and ratio data also accept a single-unit object (`{"eur": "84"}`).
fn scalar_data_schema(lemma_type: &DomainLemmaType) -> Map<String, JsonValue> {
    let specification = &lemma_type.specifications;
    let names: Vec<&str> = match specification {
        DomainTypeSpecification::Measure { units, .. } => {
            units.iter().map(|unit| unit.name.as_str()).collect()
        }
        DomainTypeSpecification::Ratio { units, .. } => {
            units.iter().map(|unit| unit.name.as_str()).collect()
        }
        _ => return scalar_type_schema(lemma_type),
    };
    let unit_map = json!({
        "type": "object",
        "minProperties": 1,
        "maxProperties": 1,
        "propertyNames": {"enum": names},
        "additionalProperties": {"type": "string"}
    });
    let mut schema = Map::new();
    schema.insert(
        "oneOf".to_string(),
        json!([JsonValue::Object(scalar_type_schema(lemma_type)), unit_map]),
    );
    publish_option_enum(&mut schema, lemma_type);
    schema
}

fn scalar_type_schema(lemma_type: &DomainLemmaType) -> Map<String, JsonValue> {
    let specification = &lemma_type.specifications;
    let mut schema = Map::new();
    match specification {
        DomainTypeSpecification::Boolean { .. } => {
            schema.insert(
                "oneOf".to_string(),
                json!([
                    {"type": "boolean"},
                    {"type": "string", "pattern": BOOLEAN_TEXT_PATTERN}
                ]),
            );
        }
        DomainTypeSpecification::Text { length, .. } => {
            schema.insert("type".to_string(), json!("string"));
            if let Some(length) = length {
                schema.insert("minLength".to_string(), json!(length));
                schema.insert("maxLength".to_string(), json!(length));
            }
        }
        DomainTypeSpecification::Number { .. }
        | DomainTypeSpecification::Measure { .. }
        | DomainTypeSpecification::Ratio { .. }
        | DomainTypeSpecification::NumberRange { .. }
        | DomainTypeSpecification::MeasureRange { .. }
        | DomainTypeSpecification::RatioRange { .. }
        | DomainTypeSpecification::DateRange { .. }
        | DomainTypeSpecification::TimeRange { .. } => {
            schema.insert("type".to_string(), json!("string"));
        }
        DomainTypeSpecification::Date { .. } => {
            schema.insert(
                "oneOf".to_string(),
                json!([
                    {"type": "string", "format": "date"},
                    {"type": "string", "format": "date-time"}
                ]),
            );
        }
        DomainTypeSpecification::Time { .. } => {
            schema.insert("type".to_string(), json!("string"));
            schema.insert("format".to_string(), json!("time"));
        }
        DomainTypeSpecification::List { .. } => {
            unreachable!("BUG: list schemas are built by type_schema")
        }
        DomainTypeSpecification::Spec { spec } => {
            unreachable!("BUG: spec '{spec}' is a uses alias, never a Show data entry")
        }
        DomainTypeSpecification::Veto { .. } => {
            unreachable!("BUG: veto sentinel type on a Show data entry")
        }
        DomainTypeSpecification::Undetermined => {
            unreachable!("BUG: undetermined sentinel type on a Show data entry")
        }
    }
    publish_option_enum(&mut schema, lemma_type);
    schema
}

fn publish_option_enum(schema: &mut Map<String, JsonValue>, lemma_type: &DomainLemmaType) {
    if lemma_type.options.values.is_empty() {
        return;
    }
    schema.insert(
        "enum".to_string(),
        json!(lemma_type
            .options
            .values
            .iter()
            .map(|option| lemma_type.display_bound(&option.value))
            .collect::<Vec<_>>()),
    );
}

fn specification_help(specification: &DomainTypeSpecification) -> String {
    let mut leaf = specification;
    while let DomainTypeSpecification::List { element, .. } = leaf {
        leaf = &element.specifications;
    }
    match leaf {
        DomainTypeSpecification::Boolean { help }
        | DomainTypeSpecification::Measure { help, .. }
        | DomainTypeSpecification::MeasureRange { help, .. }
        | DomainTypeSpecification::Number { help, .. }
        | DomainTypeSpecification::NumberRange { help, .. }
        | DomainTypeSpecification::Ratio { help, .. }
        | DomainTypeSpecification::RatioRange { help, .. }
        | DomainTypeSpecification::Text { help, .. }
        | DomainTypeSpecification::Date { help, .. }
        | DomainTypeSpecification::DateRange { help, .. }
        | DomainTypeSpecification::Time { help, .. }
        | DomainTypeSpecification::TimeRange { help, .. } => help.clone(),
        DomainTypeSpecification::Spec { spec } => spec.to_string(),
        DomainTypeSpecification::List { .. } => {
            unreachable!("BUG: list chain is unwound before the leaf match")
        }
        DomainTypeSpecification::Veto { .. } | DomainTypeSpecification::Undetermined => {
            unreachable!("BUG: sentinel type on a Show data entry")
        }
    }
}

/// Run-data JSON for a committed value or suggestion: a boolean is a JSON boolean, a value
/// list an array of element literals, an instance list an array of row objects keyed by
/// field, everything else the literal string.
fn rule_result_value_has_reference(value: &DomainRuleResultValue) -> bool {
    if value.reference.is_some() {
        return true;
    }
    if let Some(fields) = &value.fields {
        if fields.values().any(rule_result_value_has_reference) {
            return true;
        }
    }
    if let Some(list) = &value.list {
        if list.iter().any(rule_result_value_has_reference) {
            return true;
        }
    }
    false
}

enum InputLiteralTask<'a> {
    Visit {
        value: &'a DomainRuleResultValue,
        lemma_type: Arc<DomainLemmaType>,
    },
    CollectArray {
        len: usize,
    },
    CollectObject {
        keys: Vec<String>,
    },
}

fn input_literal(
    value: &DomainRuleResultValue,
    lemma_type: &DomainLemmaType,
    rows: &RowSchemas,
) -> JsonValue {
    let mut tasks = vec![InputLiteralTask::Visit {
        value,
        lemma_type: Arc::new(lemma_type.clone()),
    }];
    let mut converted: Vec<JsonValue> = Vec::new();
    while let Some(task) = tasks.pop() {
        match task {
            InputLiteralTask::Visit { value, lemma_type } => match &lemma_type.specifications {
                DomainTypeSpecification::Boolean { .. } => converted.push(JsonValue::Bool(
                    value
                        .boolean
                        .expect("BUG: boolean-typed Show value without boolean"),
                )),
                DomainTypeSpecification::List { element, .. } => {
                    let items = value
                        .list
                        .as_ref()
                        .expect("BUG: list-typed Show value without list");
                    tasks.push(InputLiteralTask::CollectArray { len: items.len() });
                    match &element.specifications {
                        DomainTypeSpecification::Spec { spec } => {
                            let row_show = rows.engine.show_spec(spec);
                            for row in items.iter().rev() {
                                let fields = row
                                    .fields
                                    .as_ref()
                                    .expect("BUG: instance list Show row without fields");
                                let mut keys = Vec::with_capacity(fields.len());
                                let mut children = Vec::with_capacity(fields.len());
                                for (name, field) in fields {
                                    assert!(
                                        field.reference.is_none(),
                                        "BUG: input_literal on Show value with reference field '{name}'"
                                    );
                                    let field_type = row_show
                                        .types
                                        .get(name)
                                        .or_else(|| row_show.data.get(name).map(|d| &d.lemma_type))
                                        .unwrap_or_else(|| {
                                            panic!("BUG: row field '{name}' missing from Show of '{spec}'")
                                        });
                                    keys.push(name.clone());
                                    children.push((field, Arc::new(field_type.clone())));
                                }
                                tasks.push(InputLiteralTask::CollectObject { keys });
                                for (field, field_type) in children.into_iter().rev() {
                                    tasks.push(InputLiteralTask::Visit {
                                        value: field,
                                        lemma_type: field_type,
                                    });
                                }
                            }
                        }
                        _ => {
                            for item in items.iter().rev() {
                                tasks.push(InputLiteralTask::Visit {
                                    value: item,
                                    lemma_type: Arc::clone(element),
                                });
                            }
                        }
                    }
                }
                _ => converted.push(JsonValue::String(
                    value
                        .result
                        .clone()
                        .expect("BUG: scalar Show value without result"),
                )),
            },
            InputLiteralTask::CollectArray { len } => {
                let items = converted.split_off(converted.len() - len);
                converted.push(JsonValue::Array(items));
            }
            InputLiteralTask::CollectObject { keys } => {
                let children = converted.split_off(converted.len() - keys.len());
                converted.push(JsonValue::Object(keys.into_iter().zip(children).collect()));
            }
        }
    }
    converted
        .pop()
        .expect("BUG: Show value conversion yields exactly one JSON value")
}

/// The API `LemmaType` minus what standard keywords already state: `help` (`description`)
/// and text `options`/`length` (`enum`, `minLength`/`maxLength`).
fn lemma_type_without_standard_keywords(lemma_type: &DomainLemmaType) -> JsonValue {
    let mut value = to_json(&LemmaType::from(lemma_type));
    strip_standard_keywords(&mut value);
    value
}

fn strip_standard_keywords(value: &mut JsonValue) {
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        let object = value
            .as_object_mut()
            .expect("BUG: LemmaType serializes to a JSON object");
        object.remove("help");
        if object.get("kind") == Some(&json!("text")) {
            object.remove("options");
            object.remove("length");
        }
        if let Some(element) = object.get_mut("element") {
            pending.push(element);
        }
    }
}

/// Row object schemas of instance lists, one `$defs` entry per row spec. A self-list's row
/// schema refers to itself.
struct RowSchemas<'a> {
    engine: &'a Engine,
    /// Consumer plan whose instance-list [`crate::planning::execution_plan::NeededByRules::fields`]
    /// feed `x-lemma-row-field-needed-by-rules` (not `$defs`).
    consumer: Option<&'a ExecutionPlan>,
    started: BTreeSet<SpecIdentity>,
    pending: Vec<SpecIdentity>,
    defs: BTreeMap<String, JsonValue>,
}

impl<'a> RowSchemas<'a> {
    fn new(engine: &'a Engine, consumer: Option<&'a ExecutionPlan>) -> Self {
        Self {
            engine,
            consumer,
            started: BTreeSet::new(),
            pending: Vec::new(),
            defs: BTreeMap::new(),
        }
    }

    /// Builds every row schema reached from the document, plus the parent-rule map
    /// for each def. Row schemas that reference further row specs queue them on
    /// `pending`, so the walk never nests.
    fn into_defs(mut self) -> (Map<String, JsonValue>, Map<String, JsonValue>) {
        let mut row_field_needed_by_rules = Map::new();
        while let Some(identity) = self.pending.pop() {
            let show = self.engine.show_spec(&identity);
            let mut schema = Map::new();
            schema.insert("title".to_string(), json!(format!("{identity} row")));
            schema.extend(row_object_schema(&show, &mut self));
            let def_name = row_def_name(&identity);
            let mut field_map = Map::new();
            for field in row_property_names(&show) {
                let needed = self.row_field_needed_by_rules(&identity, &field);
                if !needed.is_empty() {
                    field_map.insert(field, json!(needed));
                }
            }
            if !field_map.is_empty() {
                row_field_needed_by_rules.insert(def_name.clone(), JsonValue::Object(field_map));
            }
            self.defs.insert(def_name, JsonValue::Object(schema));
        }
        (self.defs.into_iter().collect(), row_field_needed_by_rules)
    }

    /// Parent Show.rules keys that need `field` on rows of `identity`.
    fn row_field_needed_by_rules(&self, identity: &SpecIdentity, field: &str) -> Vec<String> {
        let Some(consumer) = self.consumer else {
            return Vec::new();
        };
        consumer
            .row_field_needed_by_rules(identity, field)
            .into_iter()
            .map(|rule_position| {
                &consumer
                    .rules
                    .get_index(rule_position as usize)
                    .expect("BUG: needed_by_rules field consumer out of consumer.rules range")
                    .1
                    .path
            })
            .filter(|rule_path| consumer.show_rules.contains_key(*rule_path))
            .map(|rule_path| rule_path.input_key())
            .collect()
    }

    /// `$ref` to the row schema of `identity`; the schema is queued on first use.
    fn reference(&mut self, identity: &SpecIdentity) -> JsonValue {
        let name = row_def_name(identity);
        if self.started.insert(identity.clone()) {
            self.pending.push(identity.clone());
        }
        json!({ "$ref": format!("#/$defs/{name}") })
    }
}

/// `LemmaRow_` plus the row spec identity, injectively escaped to `[A-Za-z0-9_-]` (a valid
/// OpenAPI component name): `_` becomes `__`, any other character `_<hex code point>_`.
/// One name per identity, so documents of different specs agree on shared row specs.
pub fn row_def_name(identity: &SpecIdentity) -> String {
    let mut name = String::from("LemmaRow_");
    for c in identity.to_string().chars() {
        if c.is_ascii_alphanumeric() || c == '-' {
            name.push(c);
        } else if c == '_' {
            name.push_str("__");
        } else {
            name.push_str(&format!("_{:x}_", c as u32));
        }
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsing::ast::Span;
    use crate::parsing::source::{Source, SourceType};
    use crate::planning::semantics::parse_value_from_string;

    const INPUTS: &str = r#"
spec inputs
data money: measure -> unit eur: 1 -> unit cent: 0.01
data price: money -> suggest 10 eur
data deposit: 250 cent
data rate: ratio -> suggest 21%
data bps: ratio -> unit basis_points: 10000 -> unit percent: 100 -> suggest 500 basis_points
data fee: 3%
data quantity: number -> suggest 12.5
data offset: -3
data express: boolean -> suggest true
data code: text -> option "A" -> option "B" -> suggest "B"
data label: text -> length 3
data start: date -> suggest 2026-01-01
data at: time -> suggest 09:30:00
data window: number range -> suggest 1...5
data tags: number list
  -> with 5, 10
rule r_price: price + deposit
rule r_rate: rate
rule r_bps: bps
rule r_fee: fee
rule r_quantity: quantity + offset
rule r_express: express
rule r_code: code
rule r_label: label
rule r_start: start
rule r_at: at
rule r_window: window
rule r_tags: tags -> sum
"#;

    fn engine() -> Engine {
        let mut engine = Engine::new();
        engine
            .load([(SourceType::Volatile, INPUTS.to_string())])
            .expect("load inputs");
        engine
    }

    fn document() -> (Engine, DomainShow, JsonValue) {
        let engine = engine();
        let show = engine.show(None, "inputs", None).expect("show inputs");
        let document = show_document(&engine, &show);
        (engine, show, document)
    }

    fn source() -> Source {
        Source::new(
            SourceType::Volatile,
            Span {
                start: 0,
                end: 0,
                line: 1,
                col: 1,
            },
        )
    }

    fn decimal(text: &JsonValue) -> rust_decimal::Decimal {
        text.as_str()
            .expect("API decimal is a string")
            .parse()
            .expect("API decimal parses")
    }

    /// The parsed literal names the same magnitude, in the same unit, as the API value.
    fn assert_literal_denotes(
        name: &str,
        literal: &str,
        value: &JsonValue,
        specification: &DomainTypeSpecification,
    ) {
        let parsed = parse_value_from_string(literal, specification, &source())
            .unwrap_or_else(|error| panic!("{name}: '{literal}' must parse: {error}"));
        assert_parsed_denotes(name, literal, &parsed, value);
    }

    fn assert_parsed_denotes(
        name: &str,
        literal: &str,
        parsed: &crate::parsing::ast::Value,
        value: &JsonValue,
    ) {
        use crate::parsing::ast::Value;
        match parsed {
            Value::Number(magnitude) => {
                assert_eq!(*magnitude, decimal(&value["number"]), "{name}: '{literal}'");
            }
            Value::NumberWithUnit(magnitude, unit) => {
                let units = value
                    .get("measure")
                    .or_else(|| value.get("ratio"))
                    .unwrap_or_else(|| panic!("{name}: unit map in {value}"));
                assert_eq!(
                    *magnitude,
                    decimal(&units[unit]),
                    "{name}: '{literal}' vs {value}"
                );
            }
            Value::Range(left, right) => {
                assert_parsed_denotes(name, literal, left, &value["range"]["from"]);
                assert_parsed_denotes(name, literal, right, &value["range"]["to"]);
            }
            other => panic!("{name}: '{literal}' parsed to unexpected {other:?}"),
        }
    }

    #[test]
    fn spec_level_keywords() {
        let (_, _, document) = document();
        assert_eq!(document["$schema"], JSON_SCHEMA_DIALECT);
        assert_eq!(document["title"], "inputs");
        assert_eq!(document["type"], "object");
        assert_eq!(document["additionalProperties"], true);
        assert!(document.get("$defs").is_none());
        assert_eq!(document["x-lemma-start-line"], 2);
        assert!(document["x-lemma-rules"]["r_price"].is_object());
    }

    #[test]
    fn required_is_sorted_needed_inputs_without_committed_value() {
        let (_, _, document) = document();
        assert_eq!(
            document["required"],
            json!([
                "at", "bps", "code", "express", "label", "price", "quantity", "rate", "start",
                "window"
            ])
        );
    }

    #[test]
    fn standard_keywords_per_type() {
        let (_, _, document) = document();
        let properties = &document["properties"];
        assert_eq!(properties["express"]["oneOf"][0]["type"], "boolean");
        assert_eq!(properties["express"]["examples"], json!([true]));
        assert_eq!(properties["code"]["type"], "string");
        assert_eq!(properties["code"]["enum"], json!(["A", "B"]));
        assert_eq!(properties["label"]["minLength"], 3);
        assert_eq!(properties["label"]["maxLength"], 3);
        assert_eq!(properties["quantity"]["type"], "string");
        assert!(properties["quantity"].get("pattern").is_none());
        assert_eq!(
            properties["price"]["oneOf"][1]["propertyNames"]["enum"],
            json!(["eur", "cent"])
        );
        assert_eq!(
            properties["price"]["oneOf"][1]["additionalProperties"],
            json!({ "type": "string" })
        );
        assert!(properties["price"]["oneOf"][0].get("pattern").is_none());
        assert_eq!(
            properties["rate"]["oneOf"][1]["propertyNames"]["enum"],
            json!(["percent", "permille"])
        );
        assert_eq!(
            properties["bps"]["oneOf"][1]["propertyNames"]["enum"],
            json!(["percent", "permille", "basis_points"])
        );
        assert_eq!(
            properties["start"]["oneOf"],
            json!([
                {"type": "string", "format": "date"},
                {"type": "string", "format": "date-time"}
            ])
        );
        assert_eq!(properties["at"]["type"], "string");
        assert_eq!(properties["at"]["format"], "time");
        assert_eq!(properties["window"]["type"], "string");
        assert!(properties["window"].get("pattern").is_none());
        assert!(properties["window"].get("format").is_none());
        let tags = &properties["tags"];
        assert_eq!(tags["type"], "array");
        assert_eq!(tags["items"], json!({ "type": "string" }));
        assert_eq!(
            tags["maxItems"],
            crate::ResourceLimits::default().max_list_rows
        );
        assert_eq!(tags["default"], json!(["5", "10"]));
    }

    #[test]
    fn x_lemma_type_omits_what_standard_keywords_state() {
        let (_, _, document) = document();
        for (name, property) in document["properties"].as_object().expect("properties") {
            let lemma_type = &property["x-lemma-type"];
            assert!(lemma_type.get("help").is_none(), "{name}: {lemma_type}");
            assert!(lemma_type.get("options").is_none(), "{name}: {lemma_type}");
            assert!(lemma_type.get("length").is_none(), "{name}: {lemma_type}");
            if let Some(element) = lemma_type.get("element") {
                assert!(element.get("help").is_none(), "{name}: {element}");
            }
            assert!(property.get("x-lemma-path").is_some(), "{name}");
            assert!(property.get("x-lemma-needed-by-rules").is_some(), "{name}");
        }
        assert_eq!(
            document["properties"]["price"]["x-lemma-type"]["kind"],
            "measure"
        );
        assert!(document["properties"]["price"]["x-lemma-type"]["units"].is_array());
    }

    fn value_json_has_reference(value: &JsonValue) -> bool {
        if value.get("reference").is_some() {
            return true;
        }
        if let Some(fields) = value.get("fields").and_then(|v| v.as_object()) {
            if fields.values().any(value_json_has_reference) {
                return true;
            }
        }
        if let Some(list) = value.get("list").and_then(|v| v.as_array()) {
            if list.iter().any(value_json_has_reference) {
                return true;
            }
        }
        false
    }

    /// The evaluation-data JSON a typed value stands for, derived from the API value alone.
    fn evaluate_data_json_of(value: &JsonValue) -> JsonValue {
        if let Some(boolean) = value.get("boolean") {
            return boolean.clone();
        }
        if let Some(items) = value.get("list") {
            return JsonValue::Array(
                items
                    .as_array()
                    .expect("list")
                    .iter()
                    .map(evaluate_data_json_of)
                    .collect(),
            );
        }
        if let Some(fields) = value.get("fields") {
            return JsonValue::Object(
                fields
                    .as_object()
                    .expect("fields")
                    .iter()
                    .filter(|(_, field)| field.get("reference").is_none())
                    .map(|(name, field)| (name.clone(), evaluate_data_json_of(field)))
                    .collect(),
            );
        }
        if value.get("reference").is_some() {
            panic!("BUG: reference field is not a run-data literal");
        }
        value["result"].clone()
    }

    #[test]
    fn instance_list_with_references_has_value_but_no_default() {
        let mut engine = Engine::new();
        engine
            .load([(
                SourceType::Volatile,
                r#"
spec processing
data type: text
data weight: number
data enabled: boolean
spec line_item
data weight: number
data finish_a: boolean
uses step: processing
data processing: step list
  -> with weight: weight, type: "finish_a", enabled: finish_a
rule cost: processing -> where enabled -> count
"#
                .to_string(),
            )])
            .expect("load");
        let show = engine.show(None, "line_item", None).expect("show");
        let processing = show.data.get("processing").expect("processing");
        let value = processing.value.as_ref().expect("value");
        assert!(
            rule_result_value_has_reference(value),
            "expected reference in value: {value:?}"
        );
        let document = show_document(&engine, &show);
        let property = &document["properties"]["processing"];
        assert!(property.get("x-lemma-value").is_some(), "{property}");
        assert!(
            property.get("default").is_none(),
            "default must be omitted when fields reference data: {property}"
        );
    }

    #[test]
    fn default_and_examples_agree_with_typed_value_and_suggestion() {
        let (_, _, document) = document();
        let mut checked = 0;
        for (name, property) in document["properties"].as_object().expect("properties") {
            match (property.get("default"), property.get("x-lemma-value")) {
                (Some(default), Some(value)) => {
                    assert_eq!(default, &evaluate_data_json_of(value), "{name}");
                    checked += 1;
                }
                (None, Some(value)) => {
                    assert!(
                        value_json_has_reference(value),
                        "{name}: value without default must carry a reference field"
                    );
                }
                (None, None) => {}
                other => {
                    panic!("{name}: default and x-lemma-value disagree on presence: {other:?}")
                }
            }
            match (property.get("examples"), property.get("x-lemma-suggestion")) {
                (Some(examples), Some(suggestion)) => {
                    assert_eq!(
                        examples,
                        &json!([evaluate_data_json_of(suggestion)]),
                        "{name}"
                    );
                    checked += 1;
                }
                (None, None) => {}
                other => panic!(
                    "{name}: examples and x-lemma-suggestion disagree on presence: {other:?}"
                ),
            }
        }
        assert_eq!(checked, 13);
    }

    fn is_numeric_literal_specification(specification: &DomainTypeSpecification) -> bool {
        matches!(
            specification,
            DomainTypeSpecification::Number { .. }
                | DomainTypeSpecification::Measure { .. }
                | DomainTypeSpecification::Ratio { .. }
                | DomainTypeSpecification::NumberRange { .. }
                | DomainTypeSpecification::MeasureRange { .. }
                | DomainTypeSpecification::RatioRange { .. }
        )
    }

    #[test]
    fn emitted_string_literals_denote_their_value() {
        let (_, show, document) = document();
        let mut checked = 0;
        for (name, property) in document["properties"].as_object().expect("properties") {
            let specification = &show.data[name].lemma_type.specifications;
            if !is_numeric_literal_specification(specification) {
                continue;
            }
            let mut literals: Vec<(&str, &JsonValue)> = Vec::new();
            if let Some(literal) = property.get("default").and_then(|value| value.as_str()) {
                literals.push((literal, &property["x-lemma-value"]));
            }
            if let Some(literal) = property
                .get("examples")
                .and_then(|examples| examples[0].as_str())
            {
                literals.push((literal, &property["x-lemma-suggestion"]));
            }
            for (literal, value) in literals {
                assert_literal_denotes(name, literal, value, specification);
                checked += 1;
            }
        }
        assert_eq!(checked, 8);
    }

    fn identity(repository: Option<&str>, spec: &str) -> SpecIdentity {
        SpecIdentity {
            repository: repository.map(str::to_string),
            spec: spec.to_string(),
            effective: crate::parsing::ast::EffectiveDate::Origin,
            slice: crate::parsing::ast::EffectiveDate::Origin,
        }
    }

    #[test]
    fn row_def_name_is_injective_and_component_safe() {
        assert_eq!(row_def_name(&identity(None, "line")), "LemmaRow_line");
        assert_eq!(
            row_def_name(&identity(None, "tax_bracket")),
            "LemmaRow_tax__bracket"
        );
        let names = [
            row_def_name(&identity(Some("@org/repo"), "line")),
            row_def_name(&identity(None, "_org_repo_line")),
            row_def_name(&identity(None, "line")),
        ];
        for name in &names {
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
                "{name}"
            );
        }
        assert_ne!(names[0], names[1]);
        assert_ne!(names[0], names[2]);
    }
}

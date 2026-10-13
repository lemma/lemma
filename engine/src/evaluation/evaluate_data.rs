use crate::computation::{OperationResult, VetoType};
use crate::planning::execution_plan::{validate_value_against_type, ExecutionPlan};
use crate::planning::instance_list::{
    key_conflict, row_end, row_end_type, template_default, typed_from_bound, InstanceListSource,
    KeyedRow,
};
use crate::planning::normalize::lookup_data_definition;
use crate::planning::semantics::{
    parse_value_from_string, parser_value_to_value_kind, row_literal, DataDefinition, DataPath,
    ImportOverlay, LemmaType, ListKey, LiteralValue, ReferenceTarget, RowEnd, RowOrigin, Source,
    TypeSpecification, TypedLiteral, UnitTokenResolver, ValueKind,
};
use crate::planning::unit_index::UnitIndex;
use crate::Error;
use crate::ResourceLimits;
use indexmap::IndexMap;
use std::collections::HashSet;
use std::sync::Arc;

/// Neutral overlay tree. [`EvaluateData::resolve`] reads each value by declared type.
///
/// Host input may nest arbitrarily deep, so `Clone`, `Debug`, `PartialEq` and `Drop` are
/// iterative. Derived versions recurse on the native stack.
pub enum EvaluateDataValue {
    /// Unparsed Lemma literal text, parsed against the declared type by [`parse_value_from_string`].
    Literal(String),
    Boolean(bool),
    Map(IndexMap<String, EvaluateDataValue>),
    List(Vec<EvaluateDataValue>),
}

impl EvaluateDataValue {
    pub fn string(value: impl Into<String>) -> Self {
        Self::Literal(value.into())
    }

    pub(crate) fn is_empty(&self) -> bool {
        match self {
            Self::Literal(s) => s.trim().is_empty(),
            Self::Boolean(_) | Self::Map(_) | Self::List(_) => false,
        }
    }

    pub(crate) fn into_list(mut self) -> Option<Vec<EvaluateDataValue>> {
        match &mut self {
            Self::List(items) => Some(std::mem::take(items)),
            Self::Literal(_) | Self::Boolean(_) | Self::Map(_) => None,
        }
    }

    pub(crate) fn into_map(mut self) -> Option<IndexMap<String, EvaluateDataValue>> {
        match &mut self {
            Self::Map(map) => Some(std::mem::take(map)),
            Self::Literal(_) | Self::Boolean(_) | Self::List(_) => None,
        }
    }
}

impl Drop for EvaluateDataValue {
    fn drop(&mut self) {
        let mut pending = Vec::new();
        take_children(self, &mut pending);
        while let Some(mut node) = pending.pop() {
            take_children(&mut node, &mut pending);
        }
    }
}

fn take_children(node: &mut EvaluateDataValue, pending: &mut Vec<EvaluateDataValue>) {
    match node {
        EvaluateDataValue::Literal(_) | EvaluateDataValue::Boolean(_) => {}
        EvaluateDataValue::Map(map) => pending.extend(std::mem::take(map).into_values()),
        EvaluateDataValue::List(items) => pending.append(items),
    }
}

impl Clone for EvaluateDataValue {
    fn clone(&self) -> Self {
        let mut frames: Vec<CloneFrame<'_>> = Vec::new();
        let mut step = CloneStep::Descend(self);
        loop {
            step = match step {
                CloneStep::Descend(value) => match value {
                    EvaluateDataValue::Literal(s) => {
                        CloneStep::Deliver(EvaluateDataValue::Literal(s.clone()))
                    }
                    EvaluateDataValue::Boolean(b) => {
                        CloneStep::Deliver(EvaluateDataValue::Boolean(*b))
                    }
                    EvaluateDataValue::List(items) => {
                        frames.push(CloneFrame::List {
                            out: Vec::with_capacity(items.len()),
                            children: items.iter(),
                        });
                        CloneStep::NextChild
                    }
                    EvaluateDataValue::Map(map) => {
                        frames.push(CloneFrame::Map {
                            out: IndexMap::with_capacity(map.len()),
                            pending_key: None,
                            children: map.iter(),
                        });
                        CloneStep::NextChild
                    }
                },
                CloneStep::NextChild => {
                    let frame = frames
                        .last_mut()
                        .expect("BUG: clone frame stack empty while reading a child");
                    match frame.next_child() {
                        Some(child) => CloneStep::Descend(child),
                        None => {
                            let frame = frames
                                .pop()
                                .expect("BUG: clone frame stack empty on completion");
                            CloneStep::Deliver(frame.finish())
                        }
                    }
                }
                CloneStep::Deliver(value) => match frames.last_mut() {
                    None => return value,
                    Some(frame) => {
                        frame.push(value);
                        CloneStep::NextChild
                    }
                },
            };
        }
    }
}

enum CloneStep<'a> {
    Descend(&'a EvaluateDataValue),
    NextChild,
    Deliver(EvaluateDataValue),
}

enum CloneFrame<'a> {
    List {
        out: Vec<EvaluateDataValue>,
        children: std::slice::Iter<'a, EvaluateDataValue>,
    },
    Map {
        out: IndexMap<String, EvaluateDataValue>,
        pending_key: Option<String>,
        children: indexmap::map::Iter<'a, String, EvaluateDataValue>,
    },
}

impl<'a> CloneFrame<'a> {
    fn next_child(&mut self) -> Option<&'a EvaluateDataValue> {
        match self {
            CloneFrame::List { children, .. } => children.next(),
            CloneFrame::Map {
                pending_key,
                children,
                ..
            } => children.next().map(|(key, child)| {
                *pending_key = Some(key.clone());
                child
            }),
        }
    }

    fn push(&mut self, value: EvaluateDataValue) {
        match self {
            CloneFrame::List { out, .. } => out.push(value),
            CloneFrame::Map {
                out, pending_key, ..
            } => {
                let key = pending_key
                    .take()
                    .expect("BUG: clone map child delivered without its key");
                out.insert(key, value);
            }
        }
    }

    fn finish(self) -> EvaluateDataValue {
        match self {
            CloneFrame::List { out, .. } => EvaluateDataValue::List(out),
            CloneFrame::Map { out, .. } => EvaluateDataValue::Map(out),
        }
    }
}

impl PartialEq for EvaluateDataValue {
    fn eq(&self, other: &Self) -> bool {
        let mut pending: Vec<(&EvaluateDataValue, &EvaluateDataValue)> = vec![(self, other)];
        while let Some((left, right)) = pending.pop() {
            match (left, right) {
                (EvaluateDataValue::Literal(a), EvaluateDataValue::Literal(b)) => {
                    if a != b {
                        return false;
                    }
                }
                (EvaluateDataValue::Boolean(a), EvaluateDataValue::Boolean(b)) => {
                    if a != b {
                        return false;
                    }
                }
                (EvaluateDataValue::List(a), EvaluateDataValue::List(b)) => {
                    if a.len() != b.len() {
                        return false;
                    }
                    pending.extend(a.iter().zip(b.iter()));
                }
                (EvaluateDataValue::Map(a), EvaluateDataValue::Map(b)) => {
                    if a.len() != b.len() {
                        return false;
                    }
                    for (key, value) in a {
                        match b.get(key) {
                            Some(other_value) => pending.push((value, other_value)),
                            None => return false,
                        }
                    }
                }
                _ => return false,
            }
        }
        true
    }
}

impl Eq for EvaluateDataValue {}

enum DebugTask<'a> {
    Node(&'a EvaluateDataValue, usize),
    Text(&'static str),
    Quoted(&'a str),
    Indent(usize),
}

impl std::fmt::Debug for EvaluateDataValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let pretty = f.alternate();
        let mut tasks = vec![DebugTask::Node(self, 0)];
        while let Some(task) = tasks.pop() {
            match task {
                DebugTask::Node(value, level) => {
                    let mut seq = Vec::new();
                    debug_seq(value, level, pretty, &mut seq);
                    tasks.extend(seq.into_iter().rev());
                }
                DebugTask::Text(text) => f.write_str(text)?,
                DebugTask::Quoted(text) => write!(f, "{text:?}")?,
                DebugTask::Indent(level) => write!(f, "{:width$}", "", width = 4 * level)?,
            }
        }
        Ok(())
    }
}

/// Tokens of `value` at nesting `level`, matching the derived `Debug` layout.
fn debug_seq<'a>(
    value: &'a EvaluateDataValue,
    level: usize,
    pretty: bool,
    seq: &mut Vec<DebugTask<'a>>,
) {
    match value {
        EvaluateDataValue::Literal(text) => debug_tuple(seq, level, pretty, "Literal", |out, _| {
            out.push(DebugTask::Quoted(text))
        }),
        EvaluateDataValue::Boolean(b) => debug_tuple(seq, level, pretty, "Boolean", |out, _| {
            out.push(DebugTask::Text(if *b { "true" } else { "false" }))
        }),
        EvaluateDataValue::Map(map) => {
            debug_tuple(seq, level, pretty, "Map", |out, field_level| {
                if map.is_empty() {
                    out.push(DebugTask::Text("{}"));
                } else if pretty {
                    out.push(DebugTask::Text("{\n"));
                    for (key, child) in map {
                        out.push(DebugTask::Indent(field_level + 1));
                        out.push(DebugTask::Quoted(key));
                        out.push(DebugTask::Text(": "));
                        out.push(DebugTask::Node(child, field_level + 1));
                        out.push(DebugTask::Text(",\n"));
                    }
                    out.push(DebugTask::Indent(field_level));
                    out.push(DebugTask::Text("}"));
                } else {
                    out.push(DebugTask::Text("{"));
                    for (index, (key, child)) in map.iter().enumerate() {
                        if index > 0 {
                            out.push(DebugTask::Text(", "));
                        }
                        out.push(DebugTask::Quoted(key));
                        out.push(DebugTask::Text(": "));
                        out.push(DebugTask::Node(child, field_level));
                    }
                    out.push(DebugTask::Text("}"));
                }
            })
        }
        EvaluateDataValue::List(items) => {
            debug_tuple(seq, level, pretty, "List", |out, field_level| {
                if items.is_empty() {
                    out.push(DebugTask::Text("[]"));
                } else if pretty {
                    out.push(DebugTask::Text("[\n"));
                    for child in items {
                        out.push(DebugTask::Indent(field_level + 1));
                        out.push(DebugTask::Node(child, field_level + 1));
                        out.push(DebugTask::Text(",\n"));
                    }
                    out.push(DebugTask::Indent(field_level));
                    out.push(DebugTask::Text("]"));
                } else {
                    out.push(DebugTask::Text("["));
                    for (index, child) in items.iter().enumerate() {
                        if index > 0 {
                            out.push(DebugTask::Text(", "));
                        }
                        out.push(DebugTask::Node(child, field_level));
                    }
                    out.push(DebugTask::Text("]"));
                }
            })
        }
    }
}

/// `Name(field)` with the field's tokens at `field_level`; pretty mode puts it on its own line.
fn debug_tuple<'a>(
    seq: &mut Vec<DebugTask<'a>>,
    level: usize,
    pretty: bool,
    name: &'static str,
    field: impl FnOnce(&mut Vec<DebugTask<'a>>, usize),
) {
    seq.push(DebugTask::Text(name));
    if pretty {
        seq.push(DebugTask::Text("(\n"));
        seq.push(DebugTask::Indent(level + 1));
        field(seq, level + 1);
        seq.push(DebugTask::Text(",\n"));
        seq.push(DebugTask::Indent(level));
        seq.push(DebugTask::Text(")"));
    } else {
        seq.push(DebugTask::Text("("));
        field(seq, level);
        seq.push(DebugTask::Text(")"));
    }
}

impl From<bool> for EvaluateDataValue {
    fn from(value: bool) -> Self {
        Self::Boolean(value)
    }
}

impl From<String> for EvaluateDataValue {
    fn from(value: String) -> Self {
        Self::Literal(value)
    }
}

impl From<&str> for EvaluateDataValue {
    fn from(value: &str) -> Self {
        Self::Literal(value.to_string())
    }
}

fn reject_case_insensitive_duplicates<'a>(
    keys: impl IntoIterator<Item = &'a str>,
    describe: impl Fn(&str) -> String,
) -> Result<(), Error> {
    let mut seen = HashSet::new();
    for key in keys {
        let canonical = crate::parsing::ast::ascii_lowercase_logical_name(key.to_string());
        if !seen.insert(canonical.clone()) {
            return Err(Error::request(
                describe(&canonical),
                Some("Keys are case-insensitive; remove the duplicate"),
            ));
        }
    }
    Ok(())
}

/// Top-level run-data entries from a host walker. Keys differing only in case: request Error.
pub fn evaluate_data_from_entries(
    entries: Vec<(String, EvaluateDataValue)>,
) -> Result<IndexMap<String, EvaluateDataValue>, Error> {
    reject_case_insensitive_duplicates(entries.iter().map(|(k, _)| k.as_str()), |k| {
        format!("Duplicate data key '{k}'")
    })?;
    Ok(entries.into_iter().collect())
}

/// Nested object entries from a host walker. Keys differing only in case: request Error.
pub fn evaluate_data_map_from_entries(
    entries: Vec<(String, EvaluateDataValue)>,
) -> Result<EvaluateDataValue, Error> {
    reject_case_insensitive_duplicates(entries.iter().map(|(k, _)| k.as_str()), |k| {
        format!("Duplicate object key '{k}'")
    })?;
    Ok(EvaluateDataValue::Map(entries.into_iter().collect()))
}

/// Host list item that is null (JSON `null`, JS `null` / `undefined`, Java `null`, Elixir `nil`).
/// A list has no unsupplied row, so this is a request Error, never an omitted item.
/// `position` is 1-based within its list.
pub fn evaluate_data_null_list_item(position: usize) -> Error {
    Error::request(
        format!("list item {position} is null"),
        Some("remove the item or supply a value"),
    )
}

/// Attribute a walker error to its top-level data key unless already attributed.
pub fn attribute_evaluate_data_error(error: Error, key: &str) -> Error {
    if error.related_data().is_some() {
        error
    } else {
        error.with_related_data(key)
    }
}

pub fn parse_data_value(
    input: &EvaluateDataValue,
    lemma_type: &Arc<LemmaType>,
    source: &Source,
    unit_index: &UnitIndex,
    named_types: &IndexMap<String, Arc<LemmaType>>,
) -> Result<TypedLiteral, Error> {
    let mut frames: Vec<ListParseFrame<'_>> = Vec::new();
    let mut step = ParseStep::Parse(input, Arc::clone(lemma_type));
    loop {
        step = match step {
            ParseStep::Parse(value, ty) => {
                match parse_node(value, &ty, source, unit_index, named_types)? {
                    ParseNode::Literal(literal) => ParseStep::Deliver(literal),
                    ParseNode::List {
                        rows,
                        lemma_type,
                        element_type,
                    } => {
                        frames.push(ListParseFrame {
                            rows: rows.iter(),
                            lemma_type,
                            element_type,
                            converted: Vec::with_capacity(rows.len()),
                        });
                        ParseStep::NextChild
                    }
                }
            }
            ParseStep::NextChild => {
                let frame = frames
                    .last_mut()
                    .expect("BUG: list parse frame stack empty while reading a child");
                match frame.rows.next() {
                    Some(child) => ParseStep::Parse(child, Arc::clone(&frame.element_type)),
                    None => {
                        let frame = frames
                            .pop()
                            .expect("BUG: list parse frame stack empty on completion");
                        ParseStep::Deliver(TypedLiteral {
                            value: ValueKind::List(frame.converted),
                            lemma_type: frame.lemma_type,
                        })
                    }
                }
            }
            ParseStep::Deliver(literal) => match frames.last_mut() {
                None => return Ok(literal),
                Some(frame) => {
                    frame.converted.push(literal);
                    ParseStep::NextChild
                }
            },
        };
    }
}

enum ParseStep<'a> {
    Parse(&'a EvaluateDataValue, Arc<LemmaType>),
    NextChild,
    Deliver(TypedLiteral),
}

struct ListParseFrame<'a> {
    rows: std::slice::Iter<'a, EvaluateDataValue>,
    lemma_type: Arc<LemmaType>,
    element_type: Arc<LemmaType>,
    converted: Vec<TypedLiteral>,
}

enum ParseNode<'a> {
    Literal(TypedLiteral),
    List {
        rows: &'a [EvaluateDataValue],
        lemma_type: Arc<LemmaType>,
        element_type: Arc<LemmaType>,
    },
}

/// Shape mismatches are request Errors from `evaluate_data_shape_error`, checked before parsing.
/// The mismatch panics below are therefore unreachable.
fn parse_node<'a>(
    input: &'a EvaluateDataValue,
    lemma_type: &Arc<LemmaType>,
    source: &Source,
    unit_index: &UnitIndex,
    named_types: &IndexMap<String, Arc<LemmaType>>,
) -> Result<ParseNode<'a>, Error> {
    let type_spec = &lemma_type.specifications;

    let (kind, binding_unit) = match (input, type_spec) {
        (EvaluateDataValue::Literal(s), _) => {
            return parse_literal(s, lemma_type, source, unit_index, named_types)
                .map(ParseNode::Literal);
        }
        (EvaluateDataValue::Boolean(b), TypeSpecification::Boolean { .. }) => {
            (ValueKind::Boolean(*b), None)
        }
        (EvaluateDataValue::Boolean(_), _) => {
            panic!("BUG: boolean overlay on non-boolean type is a request shape error, not parse")
        }
        (EvaluateDataValue::Map(map), TypeSpecification::Measure { .. })
        | (EvaluateDataValue::Map(map), TypeSpecification::Ratio { .. }) => {
            let (magnitude, unit) = single_unit_literal_map(map).expect(
                "BUG: measure/ratio map overlay is a request shape error unless it is a single-unit Literal",
            );
            return parse_literal(
                &format!("{magnitude} {unit}"),
                lemma_type,
                source,
                unit_index,
                named_types,
            )
            .map(ParseNode::Literal);
        }
        (EvaluateDataValue::Map(_), _) => {
            panic!("BUG: map overlay on this type is a request shape error, not parse")
        }
        (EvaluateDataValue::List(rows), TypeSpecification::List { .. }) => {
            return Ok(ParseNode::List {
                rows: rows.as_slice(),
                lemma_type: Arc::clone(lemma_type),
                element_type: Arc::clone(lemma_type.list_element_lemma_type()),
            });
        }
        (EvaluateDataValue::List(_), _) => {
            panic!("BUG: list overlay on this type is a request shape error, not parse")
        }
    };

    Ok(ParseNode::Literal(typed_literal_with_binding_unit(
        kind,
        binding_unit,
        lemma_type,
    )))
}

fn parse_literal(
    text: &str,
    lemma_type: &Arc<LemmaType>,
    source: &Source,
    unit_index: &UnitIndex,
    named_types: &IndexMap<String, Arc<LemmaType>>,
) -> Result<TypedLiteral, Error> {
    let parsed = parse_value_from_string(text, &lemma_type.specifications, source)?;
    typed_literal_from_parsed(&parsed, lemma_type, unit_index, named_types)
        .map_err(|msg| Error::validation(msg, Some(source.clone()), None::<String>))
}

/// Read a parsed Lemma literal as a value of `lemma_type`, keeping the written unit.
pub(crate) fn typed_literal_from_parsed(
    parsed: &crate::parsing::ast::Value,
    lemma_type: &Arc<LemmaType>,
    unit_index: &UnitIndex,
    named_types: &IndexMap<String, Arc<LemmaType>>,
) -> Result<TypedLiteral, String> {
    let resolve = UnitTokenResolver::Typed {
        unit_index,
        named_types,
        lemma_type: lemma_type.as_ref(),
    };
    let kind = parser_value_to_value_kind(parsed, &lemma_type.specifications, Some(&resolve))?;
    let binding = match binding_unit_from_parser_value(parsed) {
        Some(written) => Some(resolve.resolve(&written)?),
        None => None,
    };
    Ok(typed_literal_with_binding_unit(kind, binding, lemma_type))
}

fn typed_literal_with_binding_unit(
    kind: ValueKind,
    binding_unit: Option<String>,
    lemma_type: &Arc<LemmaType>,
) -> TypedLiteral {
    let typed_type = match binding_unit {
        Some(unit)
            if matches!(
                &lemma_type.specifications,
                TypeSpecification::Measure { .. } | TypeSpecification::Ratio { .. }
            ) =>
        {
            Arc::new(lemma_type.as_ref().clone().with_unit(unit))
        }
        Some(_) | None => Arc::clone(lemma_type),
    };
    TypedLiteral {
        value: kind,
        lemma_type: typed_type,
    }
}

fn binding_unit_from_parser_value(value: &crate::parsing::ast::Value) -> Option<String> {
    use crate::parsing::ast::Value;
    match value {
        Value::NumberWithUnit(_, unit) => Some(unit.clone()),
        Value::Range(left, right) => match (left.as_ref(), right.as_ref()) {
            (Value::NumberWithUnit(_, left_unit), Value::NumberWithUnit(_, right_unit))
                if left_unit == right_unit =>
            {
                Some(left_unit.clone())
            }
            _ => None,
        },
        _ => None,
    }
}

/// Resolve a unit the caller actually wrote to a bare declared name.
///
/// Qualified paths go through [`UnitIndex::resolve_with_named_types`]. Bare names
/// accepted only when this data type (or range element) declares that exact name.
/// Never invents a unit the caller did not pass.
fn single_unit_literal_map(map: &IndexMap<String, EvaluateDataValue>) -> Option<(&str, &str)> {
    if map.len() != 1 {
        return None;
    }
    let (unit, value) = map.iter().next()?;
    match value {
        EvaluateDataValue::Literal(magnitude) => Some((magnitude.as_str(), unit.as_str())),
        _ => None,
    }
}

enum ShapeCheck<'a> {
    Accepts,
    Mismatch(String),
    Rows {
        rows: &'a [EvaluateDataValue],
        element: &'a LemmaType,
    },
}

fn shape_check<'a>(input: &'a EvaluateDataValue, lemma_type: &'a LemmaType) -> ShapeCheck<'a> {
    let type_spec = &lemma_type.specifications;
    match (input, type_spec) {
        (EvaluateDataValue::Literal(_), TypeSpecification::List { .. }) => ShapeCheck::Mismatch(
            format!("literal overlay is not valid for list data ({type_spec})"),
        ),
        (EvaluateDataValue::Literal(_), _) => ShapeCheck::Accepts,
        (EvaluateDataValue::Boolean(_), TypeSpecification::Boolean { .. }) => ShapeCheck::Accepts,
        (EvaluateDataValue::Boolean(_), _) => ShapeCheck::Mismatch(format!(
            "boolean input is only valid for boolean data, not {type_spec}"
        )),
        (
            EvaluateDataValue::Map(map),
            TypeSpecification::Measure { .. } | TypeSpecification::Ratio { .. },
        ) => {
            if single_unit_literal_map(map).is_some() {
                ShapeCheck::Accepts
            } else {
                ShapeCheck::Mismatch(
                    "measure and ratio data must be a convenience string or a single-unit map"
                        .to_string(),
                )
            }
        }
        (EvaluateDataValue::Map(_), _) => ShapeCheck::Mismatch(format!(
            "object overlay is only valid for measure or ratio data, not {type_spec}"
        )),
        (EvaluateDataValue::List(rows), TypeSpecification::List { .. }) => ShapeCheck::Rows {
            rows: rows.as_slice(),
            element: lemma_type.list_element_lemma_type().as_ref(),
        },
        (EvaluateDataValue::List(_), _) => ShapeCheck::Mismatch(format!(
            "array overlay is not valid for scalar data ({type_spec})"
        )),
    }
}

struct ShapeFrame<'a> {
    rows: std::slice::Iter<'a, EvaluateDataValue>,
    element: &'a LemmaType,
    position: usize,
}

/// First shape mismatch in `input` against `lemma_type`, at any list depth. `None` if every
/// value matches the type its parse node will use. List items are reported by 1-based position.
fn evaluate_data_shape_error(input: &EvaluateDataValue, lemma_type: &LemmaType) -> Option<String> {
    let mut frames: Vec<ShapeFrame<'_>> = Vec::new();
    let mut pending = Some((input, lemma_type));
    loop {
        if let Some((value, ty)) = pending.take() {
            match shape_check(value, ty) {
                ShapeCheck::Accepts => {}
                ShapeCheck::Mismatch(message) => return Some(item_path_message(&frames, message)),
                ShapeCheck::Rows { rows, element } => frames.push(ShapeFrame {
                    rows: rows.iter(),
                    element,
                    position: 0,
                }),
            }
        }
        let frame = frames.last_mut()?;
        match frame.rows.next() {
            Some(child) => {
                frame.position += 1;
                pending = Some((child, frame.element));
            }
            None => {
                frames.pop();
            }
        }
    }
}

fn item_path_message(frames: &[ShapeFrame<'_>], message: String) -> String {
    if frames.is_empty() {
        return message;
    }
    let positions: Vec<String> = frames.iter().map(|f| f.position.to_string()).collect();
    format!("list item {}: {message}", positions.join(", item "))
}

/// User-provided data values resolved against a plan's type declarations.
///
/// Lightweight and cheap to construct — no plan cloning required. The
/// [`ExecutionPlan`] stays immutable; callers pass `(&ExecutionPlan, &EvaluateData)`
/// to evaluation and show paths.
#[derive(Debug, Clone, Default)]
pub struct EvaluateData {
    /// Caller bindings: successful literals or Veto (bad override) per Data.
    pub bindings: IndexMap<DataPath, OperationResult>,
    /// Input keys that did not match any plan Data (including Import aliases).
    pub ignored_unknown: Vec<String>,
}

impl EvaluateData {
    /// Parse and validate caller-supplied values against the plan's data declarations.
    ///
    /// Unknown keys and Import aliases are ignored (recorded in [`Self::ignored_unknown`]).
    /// Parse, constraint, options, decimals, and input-oversize failures bind that Data
    /// as [`OperationResult::Veto`]; evaluation still runs. Duplicate canonical keys Error.
    pub fn resolve(
        plan: &ExecutionPlan,
        raw_values: IndexMap<String, EvaluateDataValue>,
        limits: &ResourceLimits,
    ) -> Result<Self, Error> {
        Self::resolve_level(plan, raw_values, limits, ListLevel::Top)
    }

    fn resolve_level(
        plan: &ExecutionPlan,
        raw_values: IndexMap<String, EvaluateDataValue>,
        limits: &ResourceLimits,
        level: ListLevel,
    ) -> Result<Self, Error> {
        match run_resolve(ResolveFrame::Level(LevelFrame::new(
            plan, raw_values, limits, level,
        )))? {
            ResolveOutput::Level(evaluate_data) => Ok(evaluate_data),
            ResolveOutput::Binding(_) => {
                panic!("BUG: root level frame returned a list binding")
            }
        }
    }
}

enum ResolveOutput {
    Level(EvaluateData),
    Binding(OperationResult),
}

enum ResolveStep<'a> {
    Call(ResolveFrame<'a>),
    Return(ResolveOutput),
}

enum ResolveFrame<'a> {
    Level(LevelFrame<'a>),
    List(ListFrame<'a>),
}

impl<'a> ResolveFrame<'a> {
    fn resume(&mut self, input: Option<ResolveOutput>) -> Result<ResolveStep<'a>, Error> {
        match self {
            ResolveFrame::Level(frame) => frame.resume(input),
            ResolveFrame::List(frame) => frame.resume(input),
        }
    }
}

fn run_resolve(root: ResolveFrame<'_>) -> Result<ResolveOutput, Error> {
    let mut frames = vec![root];
    let mut input = None;
    loop {
        let frame = frames.last_mut().expect("BUG: resolve frame stack empty");
        match frame.resume(input.take())? {
            ResolveStep::Call(child) => frames.push(child),
            ResolveStep::Return(output) => {
                frames.pop();
                if frames.is_empty() {
                    return Ok(output);
                }
                input = Some(output);
            }
        }
    }
}

/// One level of run data: its entries in caller order. A list entry suspends the level
/// until its rows are resolved.
struct LevelFrame<'a> {
    plan: &'a ExecutionPlan,
    limits: &'a ResourceLimits,
    level: ListLevel,
    entries: indexmap::map::IntoIter<String, EvaluateDataValue>,
    seen_canonical: HashSet<String>,
    evaluate_data: EvaluateData,
    awaiting: Option<&'a DataPath>,
}

impl<'a> LevelFrame<'a> {
    fn new(
        plan: &'a ExecutionPlan,
        raw_values: IndexMap<String, EvaluateDataValue>,
        limits: &'a ResourceLimits,
        level: ListLevel,
    ) -> Self {
        let capacity = raw_values.len();
        Self {
            plan,
            limits,
            level,
            entries: raw_values.into_iter(),
            seen_canonical: HashSet::with_capacity(capacity),
            evaluate_data: EvaluateData {
                bindings: IndexMap::with_capacity(capacity),
                ..EvaluateData::default()
            },
            awaiting: None,
        }
    }

    fn resume(&mut self, input: Option<ResolveOutput>) -> Result<ResolveStep<'a>, Error> {
        match (self.awaiting.take(), input) {
            (None, None) => {}
            (Some(path), Some(ResolveOutput::Binding(binding))) => {
                self.evaluate_data.bindings.insert(path.clone(), binding);
            }
            _ => panic!("BUG: level frame resumed with an unexpected output"),
        }
        self.advance()
    }

    fn advance(&mut self) -> Result<ResolveStep<'a>, Error> {
        let plan = self.plan;
        let limits = self.limits;
        let level = self.level;
        let evaluate_data = &mut self.evaluate_data;
        let seen_canonical = &mut self.seen_canonical;

        for (name, raw_value) in self.entries.by_ref() {
            let canonical = crate::parsing::ast::canonical_logical_name(&name);
            if seen_canonical.contains(canonical.as_ref()) {
                let canonical = canonical.into_owned();
                return Err(Error::request(
                    format!("Duplicate data key '{canonical}'"),
                    Some("Data keys are case-insensitive; remove the duplicate"),
                )
                .with_related_data(name));
            }
            seen_canonical.insert(canonical.to_string());

            let Some((data_path, data_definition)) = resolve_evaluate_data_target(plan, &canonical)
            else {
                if let Some(list) = instance_list_of_row_key(plan, &canonical) {
                    return Err(Error::request(
                        format!(
                            "data '{name}': rows of list '{list}' are supplied as one array under '{list}'"
                        ),
                        Some(format!("pass the whole list as '{list}'")),
                    )
                    .with_related_data(name));
                }
                evaluate_data.ignored_unknown.push(name);
                continue;
            };

            if let Some(target) = overlay_reference_target(plan, data_path) {
                return Err(Error::request(
                    format!(
                        "data '{name}' is bound to '{target}' by the spec and cannot be supplied"
                    ),
                    Some(format!("remove '{name}' from the evaluation data")),
                )
                .with_related_data(name));
            }

            let data_source = data_definition.source();
            let type_arc = match data_definition {
                DataDefinition::TypeDeclaration { resolved_type, .. }
                | DataDefinition::Value { resolved_type, .. } => Arc::clone(resolved_type),
                DataDefinition::Reference { target, .. } => {
                    let target = match target {
                        ReferenceTarget::Data(path) => path.input_key(),
                        ReferenceTarget::Rule(path) => path.input_key(),
                    };
                    return Err(Error::request(
                        format!(
                            "data '{name}' is bound to '{target}' by the spec and cannot be supplied"
                        ),
                        Some(format!("remove '{name}' from the evaluation data")),
                    )
                    .with_related_data(name));
                }
                DataDefinition::Import { .. } => {
                    if matches!(
                        raw_value,
                        EvaluateDataValue::Map(_) | EvaluateDataValue::List(_)
                    ) {
                        return Err(Error::request(
                            format!(
                                "data '{name}': '{name}' is a uses alias; objects and arrays are not a nesting level"
                            ),
                            Some(format!("bind fields with dotted keys, e.g. '{name}.<field>'")),
                        )
                        .with_related_data(name));
                    }
                    evaluate_data.ignored_unknown.push(name);
                    continue;
                }
                DataDefinition::InstanceList { .. } => {
                    self.awaiting = Some(data_path);
                    return Ok(ResolveStep::Call(ResolveFrame::List(ListFrame::new(
                        plan, data_path, name, raw_value, limits, level,
                    ))));
                }
            };

            let input_key = canonical;

            if raw_value.is_empty() && type_arc.empty_runtime_input_vetoes() {
                evaluate_data.bindings.insert(
                    data_path.clone(),
                    OperationResult::Veto(VetoType::computation(
                        type_arc.data_veto_message(&input_key, "cannot be empty."),
                    )),
                );
                continue;
            }

            if let Some(message) = evaluate_data_shape_error(&raw_value, &type_arc) {
                return Err(Error::request(
                    format!("data '{name}': {message}"),
                    Some(format!("fix the overlay for '{name}'")),
                )
                .with_related_data(name));
            }

            let typed = match parse_data_value(
                &raw_value,
                &type_arc,
                data_source,
                plan.expression_unit_index(),
                &plan.resolved_types.resolved,
            ) {
                Ok(value) => value,
                Err(error) => {
                    evaluate_data.bindings.insert(
                        data_path.clone(),
                        OperationResult::Veto(VetoType::computation(
                            type_arc.data_veto_message(&input_key, error.message()),
                        )),
                    );
                    continue;
                }
            };

            if let ValueKind::List(rows) = &typed.value {
                if rows.len() > limits.max_list_rows {
                    return Err(Error::request(
                        format!(
                            "data '{name}' has {} rows, over max_list_rows {}",
                            rows.len(),
                            limits.max_list_rows
                        ),
                        Some("reduce the list size or raise max_list_rows".to_string()),
                    )
                    .with_related_data(name));
                }
            }

            let literal_value = LiteralValue {
                value: typed.value.clone(),
            };
            let oversize = match &typed.value {
                ValueKind::List(items) => items
                    .iter()
                    .position(|item| item.byte_size() > limits.max_data_value_bytes)
                    .map(|index| format!("list item {} exceeds the size limit.", index + 1)),
                _ => (literal_value.byte_size() > limits.max_data_value_bytes)
                    .then(|| "exceeds the size limit.".to_string()),
            };
            if let Some(reason) = oversize {
                evaluate_data.bindings.insert(
                    data_path.clone(),
                    OperationResult::Veto(VetoType::computation(
                        type_arc.data_veto_message(&input_key, &reason),
                    )),
                );
                continue;
            }

            if let Err(message) = validate_value_against_type(
                typed.lemma_type.as_ref(),
                &literal_value,
                plan.expression_unit_index(),
            ) {
                evaluate_data.bindings.insert(
                    data_path.clone(),
                    OperationResult::Veto(VetoType::computation(
                        type_arc.data_veto_message(&input_key, &message),
                    )),
                );
                continue;
            }

            evaluate_data.bindings.insert(
                data_path.clone(),
                OperationResult::from_bound(crate::planning::semantics::BoundValueKind {
                    value: typed.value.clone(),
                    unit: typed.lemma_type.unit().map(std::sync::Arc::from),
                }),
            );
        }

        Ok(ResolveStep::Return(ResolveOutput::Level(std::mem::take(
            evaluate_data,
        ))))
    }
}

/// Local plan.data path, or a named-`uses` member leaf matched by `input_key`.
fn resolve_evaluate_data_target<'a>(
    plan: &'a ExecutionPlan,
    key: &str,
) -> Option<(&'a DataPath, &'a DataDefinition)> {
    let path = plan.input_key_index.get(key)?;
    let definition = lookup_data_definition(&plan.data, &plan.row_templates, path)
        .unwrap_or_else(|| panic!("BUG: indexed input '{key}' has no data definition"));
    Some((path, definition))
}

/// Site-overlay reference binding for a nested import member, if any.
fn overlay_reference_target(plan: &ExecutionPlan, path: &DataPath) -> Option<String> {
    let first = path.segments.first()?;
    let import_marker = DataPath::local(first.uses.clone());
    let DataDefinition::Import { overlay, .. } = plan.data.get(&import_marker)? else {
        return None;
    };
    let relative = DataPath {
        segments: path.segments[1..].to_vec(),
        data: path.data.clone(),
    };
    match overlay.get(&relative)? {
        ImportOverlay::Reference { target, .. } => Some(match target {
            ReferenceTarget::Data(target) => target.input_key(),
            ReferenceTarget::Rule(target) => target.input_key(),
        }),
        ImportOverlay::Literal(_) => None,
    }
}

/// Instance list whose rows `key` addresses one field of (`staff.2.salary`).
fn instance_list_of_row_key(plan: &ExecutionPlan, key: &str) -> Option<String> {
    let parts: Vec<&str> = key.split('.').collect();
    plan.instance_lists.keys().find_map(|path| {
        let list_parts: Vec<&str> = path.input_key_segments().collect();
        (parts.starts_with(&list_parts) && parts.len() > list_parts.len()).then(|| path.input_key())
    })
}

/// Whether a run-data list value is a top-level one, whose whole row tree is checked
/// against the list limits, or rows nested inside one already checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListLevel {
    Top,
    Nested,
}

/// Rows of one top-level list value across all its levels, `template` being the plan
/// its rows read. Over `max_list_nodes` rows in total, or rows nesting deeper than
/// `max_list_depth` levels (the spec holding the list is level 1), is a request Error.
/// Node count is checked first so a wide tree reports its size, not its depth.
fn check_list_tree(
    template: &ExecutionPlan,
    raw_rows: &[EvaluateDataValue],
    limits: &ResourceLimits,
) -> Result<(), String> {
    let mut stack: Vec<(&ExecutionPlan, &EvaluateDataValue, usize)> =
        raw_rows.iter().map(|row| (template, row, 2)).collect();
    let mut nodes = 0usize;
    let mut deepest = 0usize;
    while let Some((row_template, raw_row, depth)) = stack.pop() {
        nodes += 1;
        if nodes > limits.max_list_nodes {
            return Err(format!(
                "has more than max_list_nodes {} rows across all levels",
                limits.max_list_nodes
            ));
        }
        deepest = deepest.max(depth);
        let EvaluateDataValue::Map(fields) = raw_row else {
            continue;
        };
        for (field, value) in fields {
            let canonical = crate::parsing::ast::ascii_lowercase_logical_name(field.clone());
            let Some(source) = row_template
                .input_key_index
                .get(canonical.as_str())
                .and_then(|path| row_template.instance_lists.get(path))
            else {
                continue;
            };
            let EvaluateDataValue::List(rows) = value else {
                continue;
            };
            let nested = row_template.row_template(&source.identity);
            stack.extend(rows.iter().map(|row| (nested, row, depth + 1)));
        }
    }
    if deepest > limits.max_list_depth {
        return Err(format!(
            "rows nest {deepest} levels deep counting the spec itself, over max_list_depth {}",
            limits.max_list_depth
        ));
    }
    Ok(())
}

/// Rows of instance list `path` from run data: one object per row, resolved against
/// the row spec's template. Unknown fields, a missing key field, or a non-object row
/// is a request Error; a vetoed field, a duplicate key or overlapping key ranges veto
/// the whole list.
/// Frame of one instance list entry: validates the list, then resolves each row through
/// a level frame of the row template. A vetoed field, duplicate key or overlapping key
/// range resolves the list to a veto.
struct ListFrame<'a> {
    plan: &'a ExecutionPlan,
    path: &'a DataPath,
    name: String,
    limits: &'a ResourceLimits,
    level: ListLevel,
    raw_value: Option<EvaluateDataValue>,
    rows: Option<ListRows<'a>>,
}

struct ListRows<'a> {
    source: &'a InstanceListSource,
    template: &'a ExecutionPlan,
    row_type: Arc<LemmaType>,
    raw_rows: std::vec::IntoIter<EvaluateDataValue>,
    number: usize,
    literals: Vec<TypedLiteral>,
    keyed: Vec<KeyedRow>,
}

fn request_error(name: &str, message: String) -> Error {
    Error::request(format!("data '{name}': {message}"), None::<String>).with_related_data(name)
}

impl<'a> ListFrame<'a> {
    fn new(
        plan: &'a ExecutionPlan,
        path: &'a DataPath,
        name: String,
        raw_value: EvaluateDataValue,
        limits: &'a ResourceLimits,
        level: ListLevel,
    ) -> Self {
        Self {
            plan,
            path,
            name,
            limits,
            level,
            raw_value: Some(raw_value),
            rows: None,
        }
    }

    fn resume(&mut self, input: Option<ResolveOutput>) -> Result<ResolveStep<'a>, Error> {
        match self.rows.take() {
            None => {
                assert!(
                    input.is_none(),
                    "BUG: list frame started with a child output"
                );
                let raw_value = self
                    .raw_value
                    .take()
                    .expect("BUG: list frame started twice");
                let rows = self.start(raw_value)?;
                self.next_row(rows)
            }
            Some(mut rows) => {
                let Some(ResolveOutput::Level(row_data)) = input else {
                    panic!("BUG: list frame resumed without a row result");
                };
                match self.finish_row(&mut rows, row_data)? {
                    Some(veto) => Ok(ResolveStep::Return(ResolveOutput::Binding(veto))),
                    None => self.next_row(rows),
                }
            }
        }
    }

    fn start(&self, raw_value: EvaluateDataValue) -> Result<ListRows<'a>, Error> {
        let plan = self.plan;
        let source = plan
            .instance_lists
            .get(self.path)
            .expect("BUG: instance list data without a source");
        let template = plan.row_template(&source.identity);
        let Some(raw_rows) = raw_value.into_list() else {
            return Err(request_error(
                &self.name,
                format!(
                    "rows of a `{}` list are an array of objects",
                    source.identity.spec
                ),
            ));
        };
        if raw_rows.len() > self.limits.max_list_rows {
            return Err(request_error(
                &self.name,
                format!(
                    "has {} rows, over max_list_rows {}",
                    raw_rows.len(),
                    self.limits.max_list_rows
                ),
            ));
        }
        if self.level == ListLevel::Top {
            check_list_tree(template, &raw_rows, self.limits)
                .map_err(|message| request_error(&self.name, message))?;
        }
        let row_type = Arc::new(LemmaType::primitive(TypeSpecification::Spec {
            spec: source.identity.clone(),
        }));
        Ok(ListRows {
            source,
            template,
            row_type,
            literals: Vec::with_capacity(raw_rows.len()),
            keyed: Vec::new(),
            number: 0,
            raw_rows: raw_rows.into_iter(),
        })
    }

    fn next_row(&mut self, mut rows: ListRows<'a>) -> Result<ResolveStep<'a>, Error> {
        let Some(raw_row) = rows.raw_rows.next() else {
            return Ok(ResolveStep::Return(ResolveOutput::Binding(
                self.finish_list(rows),
            )));
        };
        rows.number += 1;
        let Some(fields) = raw_row.into_map() else {
            return Err(request_error(
                &self.name,
                format!("row {} must be an object", rows.number),
            ));
        };
        let template = rows.template;
        self.rows = Some(rows);
        Ok(ResolveStep::Call(ResolveFrame::Level(LevelFrame::new(
            template,
            fields,
            self.limits,
            ListLevel::Nested,
        ))))
    }

    /// Adds the resolved row `rows.number`. `Some` is a veto of the whole list.
    fn finish_row(
        &self,
        rows: &mut ListRows<'a>,
        row_data: EvaluateData,
    ) -> Result<Option<OperationResult>, Error> {
        let number = rows.number;
        let name = self.name.as_str();
        let mut bindings = IndexMap::new();
        for (field, binding) in row_data.bindings {
            if let OperationResult::Veto(veto) = &binding {
                return Ok(Some(OperationResult::Veto(VetoType::computation(format!(
                    "{name} row {number}: {veto}"
                )))));
            }
            bindings.insert(field, binding);
        }
        if let ListKey::Fields(key_fields) = &rows.source.key {
            let mut values = Vec::with_capacity(key_fields.len());
            for field in key_fields {
                let key_path = match row_end(rows.template, field) {
                    Some(RowEnd::Data(key_path)) => key_path,
                    other => panic!("BUG: key field `{field}` is not row data: {other:?}"),
                };
                let field_type = row_end_type(rows.template, &RowEnd::Data(key_path.clone()));
                let value = match bindings.get(&key_path) {
                    Some(OperationResult::Value(bound)) => typed_from_bound(bound, &field_type),
                    Some(OperationResult::Veto(_)) => {
                        unreachable!("BUG: vetoed row binding returned above")
                    }
                    None => template_default(rows.template, &key_path).ok_or_else(|| {
                        request_error(
                            name,
                            format!("row {number} has no value for key field '{field}'"),
                        )
                    })?,
                };
                values.push(value);
            }
            rows.keyed.push(KeyedRow {
                label: format!("row {number}"),
                values,
            });
        }
        rows.literals.push(row_literal(
            rows.source.identity.clone(),
            RowOrigin::Numbered {
                list: self.path.input_key(),
                number,
            },
            bindings,
            Arc::clone(&rows.row_type),
        ));
        Ok(None)
    }

    fn finish_list(&self, rows: ListRows<'a>) -> OperationResult {
        if let ListKey::Fields(key_fields) = &rows.source.key {
            if let Some(conflict) = key_conflict(key_fields, &rows.keyed) {
                return OperationResult::Veto(VetoType::computation(format!(
                    "{}: {conflict}",
                    self.name
                )));
            }
        }
        OperationResult::from_literal(LiteralValue {
            value: ValueKind::List(rows.literals),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computation::rational::{decimal_to_rational, rational_new, rational_one};
    use crate::planning::semantics::{
        primitive_number_arc, MeasureUnit, MeasureUnits, RatioUnit, RatioUnits, TypeExtends,
    };
    use rust_decimal::Decimal;

    fn dummy_source() -> Source {
        Source::new(
            crate::parsing::source::SourceType::Volatile,
            crate::parsing::ast::Span {
                start: 0,
                end: 0,
                line: 1,
                col: 1,
            },
        )
    }

    fn mass_measure_type() -> Arc<LemmaType> {
        Arc::new(LemmaType::new(
            "Mass".to_string(),
            TypeSpecification::Measure {
                minimum: None,
                maximum: None,
                decimals: None,
                units: MeasureUnits::from(vec![
                    MeasureUnit {
                        name: "kilogram".to_string(),
                        factor: rational_one(),
                        derived_measure_factors: Vec::new(),
                        decomposition: crate::literals::BaseMeasureVector::new(),
                        minimum: None,
                        maximum: None,
                        suggestion_magnitude: None,
                    },
                    MeasureUnit {
                        name: "gram".to_string(),
                        factor: decimal_to_rational(Decimal::new(1, 3)).expect("factor"),
                        derived_measure_factors: Vec::new(),
                        decomposition: crate::literals::BaseMeasureVector::new(),
                        minimum: None,
                        maximum: None,
                        suggestion_magnitude: None,
                    },
                ]),
                traits: Vec::new(),
                decomposition: None,
                unit: None,
                help: String::new(),
            },
            TypeExtends::Primitive,
        ))
    }

    fn ratio_with_percent_type() -> Arc<LemmaType> {
        Arc::new(LemmaType::new(
            "Rate".to_string(),
            TypeSpecification::Ratio {
                minimum: None,
                maximum: None,
                decimals: None,
                units: RatioUnits::from(vec![
                    RatioUnit {
                        name: "percent".to_string(),
                        value: decimal_to_rational(Decimal::new(100, 0)).expect("factor"),
                        minimum: None,
                        maximum: None,
                        suggestion_magnitude: None,
                    },
                    RatioUnit {
                        name: "fraction".to_string(),
                        value: rational_one(),
                        minimum: None,
                        maximum: None,
                        suggestion_magnitude: None,
                    },
                ]),
                unit: None,
                help: String::new(),
            },
            TypeExtends::Primitive,
        ))
    }

    #[test]
    fn string_input_parsed_against_type() {
        let ty = primitive_number_arc();
        let lit = parse_data_value(
            &EvaluateDataValue::Literal("42".to_string()),
            ty,
            &dummy_source(),
            &UnitIndex::new(),
            &IndexMap::new(),
        )
        .unwrap();
        assert!(matches!(lit.value, ValueKind::Number(_)));
    }

    fn unit_map(unit: &str, magnitude: &str) -> EvaluateDataValue {
        let mut map = IndexMap::new();
        map.insert(
            unit.to_string(),
            EvaluateDataValue::Literal(magnitude.into()),
        );
        EvaluateDataValue::Map(map)
    }

    #[test]
    fn single_unit_measure_map_parses() {
        let ty = mass_measure_type();
        let lit = parse_data_value(
            &unit_map("kilogram", "2"),
            &ty,
            &dummy_source(),
            &UnitIndex::new(),
            &IndexMap::new(),
        )
        .unwrap();
        let ValueKind::Measure(magnitude) = &lit.value else {
            panic!("expected measure");
        };
        assert_eq!(magnitude, &rational_new(2, 1));
    }

    #[test]
    fn single_unit_ratio_map_parses() {
        let ty = ratio_with_percent_type();
        let lit = parse_data_value(
            &unit_map("percent", "5"),
            &ty,
            &dummy_source(),
            &UnitIndex::new(),
            &IndexMap::new(),
        )
        .unwrap();
        let ValueKind::Ratio(canonical) = &lit.value else {
            panic!("expected ratio");
        };
        assert_eq!(
            *canonical,
            decimal_to_rational(Decimal::new(5, 2)).expect("canonical")
        );
    }

    #[test]
    fn multi_key_map_is_shape_error() {
        let mut map = IndexMap::new();
        map.insert(
            "kilogram".to_string(),
            EvaluateDataValue::Literal("2".to_string()),
        );
        map.insert(
            "gram".to_string(),
            EvaluateDataValue::Literal("2000".into()),
        );
        let message = evaluate_data_shape_error(&EvaluateDataValue::Map(map), &mass_measure_type())
            .expect("shape");
        assert!(message.contains("single-unit"), "{message}");
    }

    #[test]
    fn empty_map_is_shape_error() {
        let message = evaluate_data_shape_error(
            &EvaluateDataValue::Map(IndexMap::new()),
            &mass_measure_type(),
        )
        .expect("shape");
        assert!(message.contains("single-unit"), "{message}");
    }

    #[test]
    fn map_on_text_is_shape_error() {
        let message = evaluate_data_shape_error(
            &unit_map("eur", "84"),
            &LemmaType::primitive(TypeSpecification::Text {
                length: None,
                options: Vec::new(),
                help: String::new(),
            }),
        )
        .expect("shape");
        assert!(message.contains("text"), "{message}");
    }

    #[test]
    fn list_on_scalar_is_shape_error() {
        let message = evaluate_data_shape_error(
            &EvaluateDataValue::List(vec![EvaluateDataValue::Literal("1".to_string())]),
            &LemmaType::primitive(TypeSpecification::Number {
                minimum: None,
                maximum: None,
                decimals: None,
                help: String::new(),
            }),
        )
        .expect("shape");
        assert!(message.contains("array"), "{message}");
    }

    #[test]
    fn boolean_on_number_is_shape_error() {
        let message = evaluate_data_shape_error(
            &EvaluateDataValue::Boolean(true),
            &LemmaType::primitive(TypeSpecification::Number {
                minimum: None,
                maximum: None,
                decimals: None,
                help: String::new(),
            }),
        )
        .expect("shape");
        assert!(message.contains("boolean"), "{message}");
    }

    fn mass_list_type() -> LemmaType {
        LemmaType::list_of(mass_measure_type())
    }

    #[test]
    fn nested_boolean_item_is_shape_error() {
        let message = evaluate_data_shape_error(
            &EvaluateDataValue::List(vec![
                EvaluateDataValue::Literal("1 kilogram".to_string()),
                EvaluateDataValue::Boolean(true),
            ]),
            &mass_list_type(),
        )
        .expect("shape");
        assert!(
            message.starts_with("list item 2: boolean input is only valid for boolean data"),
            "{message}"
        );
    }

    #[test]
    fn nested_map_item_is_shape_error() {
        let text_list =
            LemmaType::list_of(Arc::new(LemmaType::primitive(TypeSpecification::Text {
                length: None,
                options: Vec::new(),
                help: String::new(),
            })));
        let message = evaluate_data_shape_error(
            &EvaluateDataValue::List(vec![unit_map("eur", "84")]),
            &text_list,
        )
        .expect("shape");
        assert!(
            message.starts_with("list item 1: object overlay"),
            "{message}"
        );
    }

    #[test]
    fn nested_list_item_is_shape_error() {
        let message = evaluate_data_shape_error(
            &EvaluateDataValue::List(vec![EvaluateDataValue::List(vec![
                EvaluateDataValue::Literal("1 eur".to_string()),
            ])]),
            &mass_list_type(),
        )
        .expect("shape");
        assert!(
            message.starts_with("list item 1: array overlay"),
            "{message}"
        );
    }

    #[test]
    fn deeply_nested_item_reports_full_item_path() {
        let nested = LemmaType::list_of(Arc::new(mass_list_type()));
        let message = evaluate_data_shape_error(
            &EvaluateDataValue::List(vec![
                EvaluateDataValue::List(vec![EvaluateDataValue::Literal("1 kilogram".to_string())]),
                EvaluateDataValue::List(vec![EvaluateDataValue::Boolean(true)]),
            ]),
            &nested,
        )
        .expect("shape");
        assert!(
            message
                .starts_with("list item 2, item 1: boolean input is only valid for boolean data"),
            "{message}"
        );
    }

    #[test]
    fn nested_literal_under_list_type_is_shape_error() {
        let nested = LemmaType::list_of(Arc::new(mass_list_type()));
        let message = evaluate_data_shape_error(
            &EvaluateDataValue::List(vec![EvaluateDataValue::Literal("1 kilogram".to_string())]),
            &nested,
        )
        .expect("shape");
        assert!(
            message.starts_with("list item 1: literal overlay"),
            "{message}"
        );
    }

    #[test]
    fn valid_nested_measure_list_has_no_shape_error() {
        let message = evaluate_data_shape_error(
            &EvaluateDataValue::List(vec![
                unit_map("kilogram", "2"),
                EvaluateDataValue::Literal("500 gram".to_string()),
            ]),
            &mass_list_type(),
        );
        assert!(message.is_none(), "{message:?}");
    }

    fn load_engine(code: &str) -> crate::Engine {
        let mut engine = crate::Engine::new();
        engine
            .load([(crate::SourceType::Volatile, code.to_string())])
            .expect("load");
        engine
    }

    fn ordered(pairs: &[(&str, EvaluateDataValue)]) -> IndexMap<String, EvaluateDataValue> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    fn sample_shallow() -> EvaluateDataValue {
        EvaluateDataValue::Map(ordered(&[
            (
                "price",
                EvaluateDataValue::Map(ordered(&[("eur", EvaluateDataValue::from("84"))])),
            ),
            (
                "fares",
                EvaluateDataValue::List(vec![
                    EvaluateDataValue::from("89 eur"),
                    EvaluateDataValue::Boolean(true),
                    EvaluateDataValue::Map(IndexMap::new()),
                    EvaluateDataValue::List(Vec::new()),
                ]),
            ),
        ]))
    }

    #[test]
    fn debug_matches_derived_layout_compact_and_pretty() {
        let value = sample_shallow();
        assert_eq!(
            format!("{value:?}"),
            r#"Map({"price": Map({"eur": Literal("84")}), "fares": List([Literal("89 eur"), Boolean(true), Map({}), List([])])})"#
        );
        assert_eq!(
            format!("{value:#?}"),
            concat!(
                "Map(\n",
                "    {\n",
                "        \"price\": Map(\n",
                "            {\n",
                "                \"eur\": Literal(\n",
                "                    \"84\",\n",
                "                ),\n",
                "            },\n",
                "        ),\n",
                "        \"fares\": List(\n",
                "            [\n",
                "                Literal(\n",
                "                    \"89 eur\",\n",
                "                ),\n",
                "                Boolean(\n",
                "                    true,\n",
                "                ),\n",
                "                Map(\n",
                "                    {},\n",
                "                ),\n",
                "                List(\n",
                "                    [],\n",
                "                ),\n",
                "            ],\n",
                "        ),\n",
                "    },\n",
                ")",
            )
        );
    }

    #[test]
    fn deep_chains_clone_eq_debug_and_drop_on_small_stack() {
        const DEPTH: usize = 1_000_000;
        std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(|| {
                let mut list = EvaluateDataValue::Literal("leaf".to_string());
                for _ in 0..DEPTH {
                    list = EvaluateDataValue::List(vec![list]);
                }
                let cloned = list.clone();
                assert!(list == cloned);
                let rendered = format!("{list:?}");
                assert_eq!(
                    rendered.len(),
                    DEPTH * "List([".len() + "Literal(\"leaf\")".len() + DEPTH * "])".len()
                );
                drop(cloned);

                let mut map = EvaluateDataValue::Boolean(false);
                for _ in 0..DEPTH {
                    let mut level = IndexMap::new();
                    level.insert("n".to_string(), map);
                    map = EvaluateDataValue::Map(level);
                }
                let cloned = map.clone();
                assert!(map == cloned);
                assert!(map != list);
                drop(cloned);
                drop(map);
                drop(list);
            })
            .expect("spawn small-stack thread")
            .join()
            .expect("deep run data must not overflow the stack");
    }

    #[test]
    fn eq_is_order_insensitive_for_maps_and_ordered_for_lists() {
        let forward = EvaluateDataValue::Map(ordered(&[
            ("a", EvaluateDataValue::from("1")),
            ("b", EvaluateDataValue::from("2")),
        ]));
        let reversed = EvaluateDataValue::Map(ordered(&[
            ("b", EvaluateDataValue::from("2")),
            ("a", EvaluateDataValue::from("1")),
        ]));
        assert!(forward == reversed);
        assert!(
            EvaluateDataValue::List(vec![
                EvaluateDataValue::from("1"),
                EvaluateDataValue::from("2")
            ]) != EvaluateDataValue::List(vec![
                EvaluateDataValue::from("2"),
                EvaluateDataValue::from("1")
            ])
        );
        assert!(EvaluateDataValue::from("1") != EvaluateDataValue::Boolean(true));
    }

    #[test]
    fn request_error_names_first_key_in_caller_order() {
        let engine = load_engine(
            r#"
spec inner
data n: 1

spec s
uses left: inner
uses right: inner
rule out: 1
"#,
        );
        let now = crate::DateTimeValue::now();
        let overlay = EvaluateDataValue::Map(ordered(&[("n", EvaluateDataValue::from("2"))]));
        let left_first = engine
            .evaluate(
                None,
                "s",
                Some(&now),
                ordered(&[("left", overlay.clone()), ("right", overlay.clone())]),
                None,
                false,
            )
            .expect_err("uses alias map is a request error");
        assert!(left_first.to_string().contains("'left'"), "{left_first}");
        assert!(!left_first.to_string().contains("'right'"), "{left_first}");

        let right_first = engine
            .evaluate(
                None,
                "s",
                Some(&now),
                ordered(&[("right", overlay.clone()), ("left", overlay)]),
                None,
                false,
            )
            .expect_err("uses alias map is a request error");
        assert!(right_first.to_string().contains("'right'"), "{right_first}");
        assert!(!right_first.to_string().contains("'left'"), "{right_first}");
    }

    fn roster_engine() -> crate::Engine {
        load_engine(
            r#"
spec person
data age: number
  -> minimum 0
data score: number
  -> minimum 0

spec roster
uses people: person list
rule n: people -> count
"#,
        )
    }

    #[test]
    fn row_veto_names_first_field_in_caller_order() {
        let engine = roster_engine();
        let now = crate::DateTimeValue::now();
        let age_first = engine
            .evaluate(
                None,
                "roster",
                Some(&now),
                ordered(&[(
                    "people",
                    EvaluateDataValue::List(vec![EvaluateDataValue::Map(ordered(&[
                        ("age", EvaluateDataValue::from("-1")),
                        ("score", EvaluateDataValue::from("-1")),
                    ]))]),
                )]),
                Some(&["n".to_string()]),
                false,
            )
            .expect("evaluate");
        let reason = age_first
            .results
            .get("n")
            .expect("n")
            .veto_reason
            .as_deref()
            .expect("veto");
        assert!(reason.contains("age"), "{reason}");
        assert!(!reason.contains("score"), "{reason}");

        let score_first = engine
            .evaluate(
                None,
                "roster",
                Some(&now),
                ordered(&[(
                    "people",
                    EvaluateDataValue::List(vec![EvaluateDataValue::Map(ordered(&[
                        ("score", EvaluateDataValue::from("-1")),
                        ("age", EvaluateDataValue::from("-1")),
                    ]))]),
                )]),
                Some(&["n".to_string()]),
                false,
            )
            .expect("evaluate");
        let reason = score_first
            .results
            .get("n")
            .expect("n")
            .veto_reason
            .as_deref()
            .expect("veto");
        assert!(reason.contains("score"), "{reason}");
        assert!(!reason.contains("age"), "{reason}");
    }

    #[test]
    fn row_unknown_fields_are_ignored() {
        let engine = roster_engine();
        let now = crate::DateTimeValue::now();
        let bogus_first = engine
            .evaluate(
                None,
                "roster",
                Some(&now),
                ordered(&[(
                    "people",
                    EvaluateDataValue::List(vec![EvaluateDataValue::Map(ordered(&[
                        ("bogus", EvaluateDataValue::from("1")),
                        ("also", EvaluateDataValue::from("2")),
                        ("age", EvaluateDataValue::from("1")),
                        ("score", EvaluateDataValue::from("1")),
                    ]))]),
                )]),
                None,
                false,
            )
            .expect("unknown row fields are ignored");
        assert!(bogus_first.results.get("n").is_some());

        let also_first = engine
            .evaluate(
                None,
                "roster",
                Some(&now),
                ordered(&[(
                    "people",
                    EvaluateDataValue::List(vec![EvaluateDataValue::Map(ordered(&[
                        ("also", EvaluateDataValue::from("2")),
                        ("bogus", EvaluateDataValue::from("1")),
                        ("age", EvaluateDataValue::from("1")),
                        ("score", EvaluateDataValue::from("1")),
                    ]))]),
                )]),
                None,
                false,
            )
            .expect("unknown row fields are ignored");
        assert!(also_first.results.get("n").is_some());
    }

    fn tags_engine(limits: crate::ResourceLimits) -> crate::Engine {
        let mut engine = crate::Engine::with_limits(limits);
        engine
            .load([(
                crate::SourceType::Volatile,
                "spec tagged\ndata tags: text list\nrule n: tags -> count".to_string(),
            )])
            .expect("load");
        engine
    }

    fn evaluate_tags(engine: &crate::Engine, tags: Vec<String>) -> crate::Response {
        let now = crate::DateTimeValue::now();
        engine
            .evaluate(
                None,
                "tagged",
                Some(&now),
                ordered(&[(
                    "tags",
                    EvaluateDataValue::List(
                        tags.into_iter().map(EvaluateDataValue::Literal).collect(),
                    ),
                )]),
                Some(&["n".to_string()]),
                false,
            )
            .expect("evaluate")
    }

    #[test]
    fn value_list_over_value_byte_limit_binds() {
        let engine = tags_engine(crate::ResourceLimits::default());
        let limit = crate::ResourceLimits::default().max_data_value_bytes;
        let tags: Vec<String> = (0..200).map(|i| format!("customer-tag-{i:04}")).collect();
        let total: usize = tags.iter().map(String::len).sum();
        assert!(
            total > limit,
            "fixture must exceed {limit} bytes, has {total}"
        );

        let response = evaluate_tags(&engine, tags);
        let result = response.results.get("n").expect("n");
        assert_eq!(result.veto_reason, None);
        assert_eq!(result.result(), Some("200"));
    }

    #[test]
    fn value_list_item_over_value_byte_limit_vetoes() {
        let engine = tags_engine(crate::ResourceLimits {
            max_data_value_bytes: 50,
            ..crate::ResourceLimits::default()
        });
        let response = evaluate_tags(
            &engine,
            vec!["vip".to_string(), "x".repeat(51), "trial".to_string()],
        );
        let reason = response
            .results
            .get("n")
            .expect("n")
            .veto_reason
            .as_deref()
            .expect("veto");
        assert!(
            reason.contains("list item 2 exceeds the size limit"),
            "{reason}"
        );
    }
}

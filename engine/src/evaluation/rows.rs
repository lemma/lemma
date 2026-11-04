//! Instance-list rows: building rows from source declarations and reading row
//! fields through the row spec's template plan.
//!
//! Every row reads in its own [`EvaluationContext`] over the template plan, kept
//! for the request so a row's rules evaluate once. Missing fields come back as
//! [`VetoType::MissingRowData`] keyed by the row's origin and are logged in the
//! consumer context for `missing_data`. Row contexts are built and read by the
//! frame machine in [`super::tree`].

use crate::computation::{OperationResult, VetoType};
use crate::evaluation::{tree, EvaluationContext};
use crate::planning::execution_plan::{validate_value_against_type, ExecutionPlan};
use crate::planning::semantics::{
    DataDefinition, DataPath, InstanceKey, LemmaType, RowEnd, RowOrigin, SpecValue, ValueKind,
};
use std::sync::Arc;

/// Why `value` violates the type of row field `field` in the template; `None` when it
/// does not.
fn row_field_violation(
    template: &ExecutionPlan,
    field: &DataPath,
    value: &OperationResult,
) -> Option<String> {
    let OperationResult::Value(bound) = value else {
        return None;
    };
    let definition = template
        .data
        .get(field)
        .expect("BUG: row field path must be data of the template");
    let field_type = definition
        .resolved_type_arc()
        .expect("BUG: row field must be typed data of the template");
    validate_value_against_type(
        field_type,
        &bound.to_literal(),
        &template.resolved_types.unit_index,
    )
    .err()
}

/// `value` for row field `field`, vetoed when it violates the field's type in the template.
pub(crate) fn checked_row_field(
    template: &ExecutionPlan,
    field: &DataPath,
    value: OperationResult,
) -> OperationResult {
    match row_field_violation(template, field, &value) {
        None => value,
        Some(message) => OperationResult::Veto(VetoType::computation(format!(
            "Field '{}' is invalid: {message}",
            field.input_key()
        ))),
    }
}

/// `value` a deferred binding of field `field` of row `key` resolved to, vetoed when it
/// violates the field's type in the template. The field is named by its path, or on a
/// row without one (an inline row) by its name.
pub(crate) fn checked_deferred_field(
    key: &InstanceKey,
    template: &ExecutionPlan,
    field: &DataPath,
    value: OperationResult,
) -> OperationResult {
    match row_field_violation(template, field, &value) {
        None => value,
        Some(message) => {
            let name = key
                .field_key(&field.input_key())
                .unwrap_or_else(|| field.input_key());
            OperationResult::Veto(VetoType::computation(format!(
                "Reference '{name}' violates its declared constraint: {message}"
            )))
        }
    }
}

/// The context `row` reads in: taken from `ctx` when already built, otherwise built
/// bottom-up over its self-list rows. Return it to
/// [`RequestCaches::row_contexts`](crate::evaluation::RequestCaches::row_contexts) once done.
pub(crate) fn take_row_context(
    key: &InstanceKey,
    plan: &ExecutionPlan,
    template: &ExecutionPlan,
    ctx: &mut EvaluationContext,
) -> EvaluationContext {
    tree::build_row_context(key, plan, template, ctx)
}

/// Rows of `key`'s own spec in its self-list fields the upcoming body reads
/// (`slots`), from the values the row's context is seeded with (`bindings`).
pub(crate) fn self_rows(
    key: &InstanceKey,
    template: &ExecutionPlan,
    slots: &[DataPath],
    bindings: &[(usize, OperationResult)],
) -> Vec<InstanceKey> {
    let mut children = Vec::new();
    for (slot, value) in bindings {
        let (path, definition) = template
            .data
            .get_index(*slot)
            .expect("BUG: row binding slot is template data");
        let is_read_self_list = matches!(definition, DataDefinition::InstanceList { .. })
            && slots.contains(path)
            && template
                .instance_lists
                .get(path)
                .is_some_and(|source| source.identity == template.identity);
        if !is_read_self_list {
            continue;
        }
        let OperationResult::Value(bound) = value else {
            continue;
        };
        let ValueKind::List(items) = &bound.value else {
            panic!("BUG: self-list field `{path}` of {key} bound to a non-list value");
        };
        for item in items.iter() {
            match &item.value {
                ValueKind::Spec(SpecValue::Row(child)) => children.push(child.clone()),
                other => panic!("BUG: self-list field `{path}` holds a non-row value {other:?}"),
            }
        }
    }
    children
}

/// The row a field was read on, and its 0-based place in the list a list command
/// iterated when the read is per row.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ReadRow {
    pub(crate) key: InstanceKey,
    pub(crate) position: Option<usize>,
}

impl ReadRow {
    /// Row `key` read at 0-based `position` of the list a list command iterates. Only a
    /// `uses` instance is labelled by its position; every other row by its origin.
    pub(crate) fn listed(key: InstanceKey, position: usize) -> Self {
        let position = matches!(key.row.origin, RowOrigin::Import { .. }).then_some(position);
        Self { key, position }
    }

    /// Row label of an explanation: 1-based number and whether a rule appended it.
    /// A spec-written row keeps its declared number; a `uses` instance in a list is
    /// labelled by its position.
    pub(crate) fn list_position(&self) -> (usize, bool) {
        match (&self.key.row.origin, self.position) {
            (RowOrigin::Import { .. }, Some(position)) => (position + 1, true),
            (origin, _) => origin.list_position(),
        }
    }
}

/// One row field read, kept for narration.
#[derive(Clone)]
pub(crate) struct RowRead {
    pub(crate) row: ReadRow,
    pub(crate) end: RowEnd,
    pub(crate) result: OperationResult,
    pub(crate) result_type: Arc<LemmaType>,
}

impl RowRead {
    pub(crate) fn same_field(&self, other: &RowRead) -> bool {
        self.row == other.row && self.end == other.end
    }
}

pub(crate) fn row_key(row: &InstanceKey, key: &str) -> Option<String> {
    row.field_key(key)
}

/// A row-context result as seen by the consumer: missing template data names the
/// row. Logs the missing key in `ctx`.
pub(crate) fn from_row(
    result: OperationResult,
    key: &InstanceKey,
    template: &ExecutionPlan,
    ctx: &mut EvaluationContext,
) -> OperationResult {
    match row_result(result, key, template) {
        OperationResult::Veto(VetoType::MissingRowData { key }) => {
            ctx.missing_row_keys.push(key.clone());
            OperationResult::Veto(VetoType::MissingRowData { key })
        }
        OperationResult::Veto(VetoType::MissingData { data, suggestion }) => {
            ctx.missing_row_keys.push(data.input_key());
            let suggestion = suggestion.or_else(|| ctx.missing_data_suggestion(&data));
            OperationResult::Veto(VetoType::MissingData { data, suggestion })
        }
        translated => translated,
    }
}

/// `result` of a deferred reference, resolved in the context `owner` of `plan` that
/// wrote it (`None` for the root), as the row binding it holds: missing data names
/// its full path, so no context it is read from re-qualifies it.
pub(crate) fn owner_result(
    result: OperationResult,
    owner: Option<&InstanceKey>,
    plan: &ExecutionPlan,
) -> OperationResult {
    match owner {
        Some(owner) => row_result(result, owner, plan),
        None => match result {
            OperationResult::Veto(VetoType::MissingData { data, .. }) => {
                OperationResult::Veto(VetoType::MissingRowData {
                    key: data.input_key(),
                })
            }
            other => other,
        },
    }
}

/// `result` of `row`'s context in consumer terms: missing template data becomes
/// [`VetoType::MissingData`] at its path from the root plan when every hop naming the
/// row is a `uses` import or a written row ([`InstanceKey::segments`]), otherwise
/// [`VetoType::MissingRowData`] under the row's path. Either is already the full path,
/// so no consumer re-qualifies it. A field an appended row does
/// not set is data of nothing (planning rejects reads of it); a rule of the row that
/// needs it, reached only when the row is shown, vetoes naming the field.
pub(crate) fn row_result(
    result: OperationResult,
    key: &InstanceKey,
    template: &ExecutionPlan,
) -> OperationResult {
    let data = match result {
        OperationResult::Veto(VetoType::MissingData { data, .. })
            if template.data_leaf.contains_key(&data) =>
        {
            data
        }
        OperationResult::Value(_) | OperationResult::Veto(_) => return result,
    };
    if let Some(mut segments) = key.segments() {
        segments.extend(data.segments);
        return OperationResult::Veto(VetoType::missing_data(
            DataPath::new(segments, data.data),
            None,
        ));
    }
    let field = data.input_key();
    match row_key(key, &field) {
        Some(key) => OperationResult::Veto(VetoType::MissingRowData { key }),
        None => OperationResult::Veto(VetoType::computation(format!(
            "Field '{field}' is not set on {key}."
        ))),
    }
}

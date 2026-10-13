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
    row_field_input_key, DataPath, LemmaType, RowEnd, RowInstance, RowOrigin, SpecValue, ValueKind,
};
use std::sync::Arc;

/// `value` for row field `field`, vetoed when it violates the field's type in the template.
pub(crate) fn checked_row_field(
    template: &ExecutionPlan,
    field: &DataPath,
    value: OperationResult,
) -> OperationResult {
    let OperationResult::Value(bound) = &value else {
        return value;
    };
    let (_, _, definition) = template
        .peel_data(field)
        .expect("BUG: row field path must peel on template");
    let field_type = definition
        .resolved_type_arc()
        .expect("BUG: row field must be typed data of the template");
    match validate_value_against_type(
        field_type,
        &bound.to_literal(),
        &template.resolved_types.unit_index,
    ) {
        Ok(()) => value,
        Err(message) => OperationResult::Veto(VetoType::computation(format!(
            "`{}`: {message}",
            field.input_key()
        ))),
    }
}

/// The context `row` reads in: taken from `ctx` when already built, otherwise built
/// bottom-up over its self-list rows. Return it with [`EvaluationContext::row_contexts`]
/// once done.
pub(crate) fn take_row_context(
    row: &Arc<RowInstance>,
    template: &ExecutionPlan,
    ctx: &mut EvaluationContext,
) -> EvaluationContext {
    tree::build_row_context(row, template, ctx)
}

/// Rows of `row`'s own spec in its self-list fields.
pub(crate) fn self_rows(row: &RowInstance, template: &ExecutionPlan) -> Vec<Arc<RowInstance>> {
    let mut children = Vec::new();
    for (path, binding) in &row.bindings {
        let is_self_list = template
            .instance_lists
            .get(path)
            .is_some_and(|source| source.identity == template.identity);
        if !is_self_list {
            continue;
        }
        let crate::computation::OperationResult::Value(bound) = binding else {
            continue;
        };
        let ValueKind::List(items) = &bound.value else {
            panic!("BUG: self-list field `{path}` bound to a non-list value");
        };
        for item in items {
            match &item.value {
                ValueKind::Spec(SpecValue::Row(child)) => children.push(Arc::clone(child)),
                other => panic!("BUG: self-list field `{path}` holds a non-row value {other:?}"),
            }
        }
    }
    children
}

/// One row field read, kept for narration.
#[derive(Clone)]
pub(crate) struct RowRead {
    pub(crate) row: Arc<RowInstance>,
    pub(crate) end: RowEnd,
    pub(crate) result: OperationResult,
    pub(crate) result_type: Arc<LemmaType>,
}

impl RowRead {
    pub(crate) fn same_field(&self, other: &RowRead) -> bool {
        self.row == other.row && self.end == other.end
    }
}

pub(crate) fn row_key(row: &RowInstance, key: &str) -> String {
    match &row.origin {
        RowOrigin::Numbered { list, number } => row_field_input_key(list, *number, key),
        RowOrigin::Appended { number } => format!("appended row {number}.{key}"),
    }
}

/// A row-context result as seen by the consumer: missing template data names the
/// row. Logs the missing key in `ctx`.
pub(crate) fn from_row(
    result: OperationResult,
    row: &RowInstance,
    template: &ExecutionPlan,
    ctx: &mut EvaluationContext,
) -> OperationResult {
    let translated = row_result(result, row, template);
    match &translated {
        OperationResult::Veto(VetoType::MissingRowData { key }) => {
            ctx.missing_row_keys.push(key.clone());
        }
        OperationResult::Veto(VetoType::MissingData { data, .. }) => {
            ctx.missing_row_keys.push(data.input_key());
        }
        OperationResult::Value(_) | OperationResult::Veto(_) => {}
    }
    translated
}

/// `result` of `row`'s context in consumer terms: missing template data becomes
/// [`VetoType::MissingRowData`] under the row's number, or a veto naming an appended row.
pub(crate) fn row_result(
    result: OperationResult,
    row: &RowInstance,
    template: &ExecutionPlan,
) -> OperationResult {
    let key = match &result {
        OperationResult::Veto(VetoType::MissingData { data, .. })
            if template.data.contains_key(data) =>
        {
            data.input_key()
        }
        OperationResult::Veto(VetoType::MissingRowData { key }) => key.clone(),
        OperationResult::Value(_) | OperationResult::Veto(_) => return result,
    };
    match &row.origin {
        RowOrigin::Numbered { .. } => OperationResult::Veto(VetoType::MissingRowData {
            key: row_key(row, &key),
        }),
        RowOrigin::Appended { .. } => OperationResult::Veto(VetoType::computation(format!(
            "{} has no value for `{key}`",
            row.origin
        ))),
    }
}

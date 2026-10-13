//! Named `uses` imports: peel into template plans with site overlay (mirror row contexts).
//!
//! Overlay [`ImportOverlay`] entries are planned against the import **owner** plan.
//! They are resolved to [`OperationResult`]s in that owner context before the body
//! template context is built — never re-interpreted against the imported body.
//! The import scopes themselves are entered and left by the frame machine in
//! [`super::tree`].

use crate::computation::{OperationResult, VetoType};
use crate::evaluation::{tree, EvaluationContext};
use crate::planning::execution_plan::{validate_value_against_type, ExecutionPlan};
use crate::planning::semantics::{
    BoundValueKind, DataDefinition, DataPath, ImportOverlay, PathSegment, RulePath, SpecInstance,
    SpecValue, ValueKind,
};
use indexmap::IndexMap;

/// Checks a resolved overlay `raw` for `path` against the template field type when the
/// entry is a reference (LHS constraints). Literal entries pass through unchanged.
pub(crate) fn check_overlay_value(
    entry: &ImportOverlay,
    raw: OperationResult,
    plan: &ExecutionPlan,
    template: &ExecutionPlan,
    import_path: &DataPath,
    path: &DataPath,
) -> OperationResult {
    match (entry, raw) {
        (ImportOverlay::Reference { .. }, OperationResult::Value(bound)) => {
            let field_type = template
                .peel_data(path)
                .map(|(_, _, definition)| definition)
                .and_then(DataDefinition::resolved_type_arc)
                .unwrap_or_else(|| {
                    panic!(
                        "BUG: overlay field '{}' must be typed on template",
                        path.input_key()
                    )
                });
            let absolute = qualify_under_import(import_path, path, plan);
            match validate_value_against_type(
                field_type.as_ref(),
                &bound.to_literal(),
                &plan.resolved_types.unit_index,
            ) {
                Ok(()) => OperationResult::from_bound(bound),
                Err(msg) => OperationResult::Veto(VetoType::computation(format!(
                    "Reference '{}' violates declared constraint: {}",
                    absolute, msg
                ))),
            }
        }
        (_, other) => other,
    }
}

fn qualify_under_import(
    import_path: &DataPath,
    relative: &DataPath,
    plan: &ExecutionPlan,
) -> DataPath {
    let DataDefinition::Import { identity, .. } = plan
        .data
        .get(import_path)
        .expect("BUG: qualify_under_import on non-import")
    else {
        panic!("BUG: qualify_under_import on non-import");
    };
    let mut segments = import_path.segments.clone();
    segments.push(PathSegment {
        uses: import_path.data.clone(),
        repository: identity.repository.clone(),
        spec: identity.spec.clone(),
    });
    segments.extend(relative.segments.iter().cloned());
    DataPath {
        segments,
        data: relative.data.clone(),
    }
}

/// Overlay values under a nested uses hop (strip that hop from each key).
pub(crate) fn values_under_hop(
    values: &IndexMap<DataPath, OperationResult>,
    hop: &str,
) -> IndexMap<DataPath, OperationResult> {
    let mut out = IndexMap::new();
    for (path, value) in values {
        let Some(first) = path.segments.first() else {
            continue;
        };
        if first.uses != hop {
            continue;
        }
        out.insert(
            DataPath {
                segments: path.segments[1..].to_vec(),
                data: path.data.clone(),
            },
            value.clone(),
        );
    }
    out
}

/// Consumer data-slot values under import alias `hop`, relative to that hop.
pub(crate) fn consumer_values_under_hop(
    plan: &ExecutionPlan,
    ctx: &EvaluationContext,
    hop: &str,
) -> IndexMap<DataPath, OperationResult> {
    let mut under_import = IndexMap::new();
    for path in plan.data_leaf.keys() {
        let Some(first) = path.segments.first() else {
            continue;
        };
        if first.uses != hop {
            continue;
        }
        let Some(slot) = ctx.data_slot(plan, path) else {
            continue;
        };
        under_import.insert(path.clone(), slot.clone());
    }
    values_under_hop(&under_import, hop)
}

/// Import scopes to build for the nested hops of an import's overlay: one per leading hop
/// that is a named import of `template` with at least one overlay value under it.
pub(crate) fn seed_queue(
    template: &ExecutionPlan,
    overlay_values: &IndexMap<DataPath, OperationResult>,
) -> Vec<(DataPath, IndexMap<DataPath, OperationResult>)> {
    let mut by_hop: IndexMap<String, IndexMap<DataPath, OperationResult>> = IndexMap::new();
    for (path, value) in overlay_values {
        let Some(first) = path.segments.first() else {
            continue;
        };
        by_hop.entry(first.uses.clone()).or_default().insert(
            DataPath {
                segments: path.segments[1..].to_vec(),
                data: path.data.clone(),
            },
            value.clone(),
        );
    }
    let mut queue = Vec::new();
    for (hop, nested_extra) in by_hop {
        let marker = DataPath::local(hop);
        if !matches!(
            template.data.get(&marker),
            Some(DataDefinition::Import { .. })
        ) {
            continue;
        }
        if nested_extra.is_empty() {
            continue;
        }
        queue.push((marker, nested_extra));
    }
    queue
}

/// Template-relative parts of an import rule result in the owner's frame: spec
/// instances gain the rule's `uses` hops, template-local missing data is qualified.
pub(crate) fn into_owner_frame(
    path: &RulePath,
    owner: &ExecutionPlan,
    result: OperationResult,
) -> OperationResult {
    match result {
        OperationResult::Value(BoundValueKind {
            value: ValueKind::Spec(SpecValue::Fixed(instance)),
            unit,
        }) => OperationResult::Value(BoundValueKind {
            value: ValueKind::Spec(SpecValue::Fixed(SpecInstance {
                prefix: path
                    .segments
                    .iter()
                    .chain(instance.prefix.iter())
                    .cloned()
                    .collect(),
            })),
            unit,
        }),
        other => remap_missing_under_import(path, owner, other),
    }
}

fn remap_missing_under_import(
    path: &RulePath,
    owner: &ExecutionPlan,
    result: OperationResult,
) -> OperationResult {
    match result {
        OperationResult::Veto(VetoType::MissingData { data, suggestion })
            if data.segments.is_empty() && owner.data.contains_key(&data) =>
        {
            // Overlay already named an owner-plan slot — do not re-qualify.
            OperationResult::Veto(VetoType::MissingData { data, suggestion })
        }
        OperationResult::Veto(VetoType::MissingData { data, suggestion })
            if data.segments.is_empty() =>
        {
            OperationResult::Veto(VetoType::missing_data(
                DataPath {
                    segments: path.segments.clone(),
                    data: data.data.clone(),
                },
                suggestion,
            ))
        }
        other => other,
    }
}

/// Missing data of a template member read through `path` is qualified under that member path.
pub(crate) fn qualify_import_member_missing(
    path: &DataPath,
    plan: &ExecutionPlan,
    result: OperationResult,
) -> OperationResult {
    match result {
        OperationResult::Veto(VetoType::MissingData { data, suggestion })
            if data.segments.is_empty() && plan.data.contains_key(&data) =>
        {
            OperationResult::Veto(VetoType::MissingData { data, suggestion })
        }
        OperationResult::Veto(VetoType::MissingData { data, suggestion })
            if data.segments.is_empty() =>
        {
            OperationResult::Veto(VetoType::missing_data(
                DataPath {
                    segments: path.segments.clone(),
                    data: data.data.clone(),
                },
                suggestion,
            ))
        }
        other => other,
    }
}

/// Value of `path` after peeling leading Import hops into template contexts.
pub(crate) fn eval_import_member(
    path: &DataPath,
    plan: &ExecutionPlan,
    ctx: &mut EvaluationContext,
) -> OperationResult {
    tree::evaluate_import_member(path, plan, ctx)
}

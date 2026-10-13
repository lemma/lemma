//! Instance lists (`uses x: spec list`): source rows, row fields, and key checks.
//!
//! Each list of rows of spec `S` reads its fields through a template plan of `S`
//! built at the consumer's slice. Rows bind template data paths; anything a row
//! leaves unbound reads the template's own default or is missing data.

use crate::computation::comparison_operation;
use crate::computation::range::ordered_endpoints;
use crate::computation::OperationResult;
use crate::planning::execution_plan::ExecutionPlan;
use crate::planning::semantics::{
    BoundValueKind, ComparisonComputation, DataDefinition, DataPath, LemmaType, ListKey,
    PathSegment, RowEnd, RulePath, SpecIdentity, TypeSpecification, TypedLiteral, ValueKind,
};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// Instance lists of one consumer slice: row-spec template plans and source rows.
#[derive(Debug)]
pub(crate) struct InstanceLists {
    /// The spec being planned. Rows of its self-lists read the plan being built,
    /// so it has no entry in [`Self::templates`].
    pub self_identity: SpecIdentity,
    pub templates: BTreeMap<SpecIdentity, Arc<ExecutionPlan>>,
    pub sources: IndexMap<DataPath, InstanceListSource>,
}

impl InstanceLists {
    pub(crate) fn new(self_identity: SpecIdentity) -> Self {
        Self {
            self_identity,
            templates: BTreeMap::new(),
            sources: IndexMap::new(),
        }
    }
}

/// Rows written under one `uses x: spec list` declaration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceListSource {
    pub identity: SpecIdentity,
    pub key: ListKey,
    /// Shared across nested `uses` of the same declaring list (see [`ListSourceInterner`]).
    #[serde(with = "crate::snapshot::shared::arc")]
    pub rows: Arc<Vec<SourceRow>>,
}

/// One source row: template data path → where its value comes from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceRow {
    pub fields: Vec<(DataPath, DataSource)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DataSource {
    Literal(BoundValueKind),
    /// Data in the declaring list owner's plan. Paths are owner-relative (no
    /// consumer `uses` prefix); materialize joins [`DataPath::segments`] of the
    /// list path in the consumer plan.
    Reference(DataPath),
}

/// Intern key for a catalog list body: declaring owner + local list name +
/// element row-spec identity + planning instant. The instant is part of the key
/// because one owner version planned at several breakpoints shares its identity
/// while its element can be planned at a different instant.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ListSourceKey {
    pub owner: SpecIdentity,
    pub local: DataPath,
    pub element: SpecIdentity,
    pub at: crate::parsing::ast::EffectiveDate,
}

/// One interned list body (element identity, key, shared rows).
#[derive(Debug, Clone)]
pub(crate) struct InternedList {
    pub identity: SpecIdentity,
    pub key: ListKey,
    pub rows: Arc<Vec<SourceRow>>,
}

/// Pass-wide share of instance-list source rows across nested `uses` consumers.
#[derive(Debug, Default)]
pub(crate) struct ListSourceInterner {
    bodies: HashMap<ListSourceKey, InternedList>,
}

impl ListSourceInterner {
    pub(crate) fn get(&self, key: &ListSourceKey) -> Option<&InternedList> {
        self.bodies.get(key)
    }

    pub(crate) fn insert(&mut self, key: ListSourceKey, list: InternedList) {
        let previous = self.bodies.insert(key.clone(), list);
        assert!(
            previous.is_none(),
            "BUG: list source {key:?} interned twice in one pass"
        );
    }
}

/// Pass-wide share of spec template plans (`SpecIdentity` → [`Arc<ExecutionPlan>`]).
///
/// Named `uses` and instance-list row templates both intern here so the same
/// catalog body is one `Arc` across every consumer planned in the pass.
#[derive(Debug, Default)]
pub(crate) struct SpecTemplateInterner {
    /// Keyed by template identity and the planning instant `at`. The same
    /// unversioned spec can embed different versioned transitive deps at
    /// different slice dates; identity alone must not share those Arcs.
    templates: HashMap<(SpecIdentity, crate::parsing::ast::EffectiveDate), Arc<ExecutionPlan>>,
}

impl SpecTemplateInterner {
    pub(crate) fn get(
        &self,
        identity: &SpecIdentity,
        at: &crate::parsing::ast::EffectiveDate,
    ) -> Option<Arc<ExecutionPlan>> {
        self.templates
            .get(&(identity.clone(), at.clone()))
            .map(Arc::clone)
    }

    /// Record a plan built in this pass. Each `(identity, instant)` is built once per pass,
    /// so an existing entry is a bug.
    pub(crate) fn insert(
        &mut self,
        identity: SpecIdentity,
        at: &crate::parsing::ast::EffectiveDate,
        template: Arc<ExecutionPlan>,
    ) {
        let key = (identity.clone(), at.clone());
        let previous = self.templates.insert(key, template);
        assert!(
            previous.is_none(),
            "BUG: template {identity} at {at:?} built twice in one pass"
        );
    }
}

/// Interners shared by every graph built in one planning pass.
#[derive(Debug, Default)]
pub(crate) struct PassInterners {
    pub list_sources: ListSourceInterner,
    pub spec_templates: SpecTemplateInterner,
}

/// Strip the declaring owner's `uses` prefix from an absolute consumer path.
pub(crate) fn owner_relative_reference(
    absolute: &DataPath,
    owner_segments: &[PathSegment],
) -> DataPath {
    assert!(
        absolute.segments.len() >= owner_segments.len()
            && absolute.segments[..owner_segments.len()] == *owner_segments,
        "BUG: reference path `{absolute}` must start with owner segments {owner_segments:?}"
    );
    DataPath {
        segments: absolute.segments[owner_segments.len()..].to_vec(),
        data: absolute.data.clone(),
    }
}

/// Join a consumer list path's alias prefix with an owner-relative reference.
pub(crate) fn absolute_reference(list_path: &DataPath, relative: &DataPath) -> DataPath {
    let mut segments = list_path.segments.clone();
    segments.extend(relative.segments.iter().cloned());
    DataPath {
        segments,
        data: relative.data.clone(),
    }
}

/// Field `name` on rows of a template plan, or of the spec being planned.
pub(crate) enum RowEndView<'a> {
    Plan(&'a ExecutionPlan),
    Root(&'a IndexMap<DataPath, DataDefinition>, &'a [&'a RulePath]),
}

pub(crate) fn row_end_of(view: RowEndView<'_>, name: &str) -> Option<RowEnd> {
    match view {
        RowEndView::Plan(plan) => row_end(plan, name),
        RowEndView::Root(data, rules) => root_row_end(data, rules.iter().copied(), name),
    }
}

/// Field `name` (dotted for nested `uses`) on rows of `template`: a data path or a rule.
pub(crate) fn row_end(template: &ExecutionPlan, name: &str) -> Option<RowEnd> {
    let parts: Vec<&str> = name.split('.').collect();
    if let Some(path) = template.qualify_data_names(&parts) {
        let (_, _, definition) = template
            .peel_data(&path)
            .expect("BUG: qualified row data path must peel on template");
        return data_row_end(&path, definition);
    }
    template.qualify_rule_names(&parts).map(RowEnd::Rule)
}

/// Row field `name` on rows of the spec being planned (a self-list), given its data
/// and rule paths so far.
pub(crate) fn root_row_end<'r>(
    data: &IndexMap<DataPath, DataDefinition>,
    mut rules: impl Iterator<Item = &'r RulePath>,
    name: &str,
) -> Option<RowEnd> {
    if let Some((path, definition)) = data.iter().find(|(path, _)| path.input_key() == name) {
        return data_row_end(path, definition);
    }
    rules
        .find(|path| path.input_key() == name)
        .map(|path| RowEnd::Rule(path.clone()))
}

/// A `uses` instance of the row spec is not a readable row field: bind or read its
/// fields (`item.price`), never the instance as a whole.
fn data_row_end(path: &DataPath, definition: &DataDefinition) -> Option<RowEnd> {
    match definition {
        DataDefinition::Import { .. } => None,
        DataDefinition::Value { .. }
        | DataDefinition::TypeDeclaration { .. }
        | DataDefinition::Reference { .. }
        | DataDefinition::InstanceList { .. } => Some(RowEnd::Data(path.clone())),
    }
}

/// Whether `name` is a `uses` instance of the row spec (not a readable field).
pub(crate) fn is_row_import(data: &IndexMap<DataPath, DataDefinition>, name: &str) -> bool {
    data.iter().any(|(path, definition)| {
        path.input_key() == name && matches!(definition, DataDefinition::Import { .. })
    })
}

/// Result type of field `end` on rows of `template`.
pub(crate) fn row_end_type(template: &ExecutionPlan, end: &RowEnd) -> Arc<LemmaType> {
    match end {
        RowEnd::Data(path) => row_data_type_peeled(template, path),
        RowEnd::Rule(path) => {
            let (plan, local) = template
                .peel_rule(path)
                .expect("BUG: row rule end must peel on template");
            Arc::clone(
                &plan
                    .rules
                    .get(local)
                    .expect("BUG: peeled row rule missing from template plan")
                    .rule_type,
            )
        }
    }
}

/// Type of row data `path` after peeling Import hops on `template`.
pub(crate) fn row_data_type_peeled(template: &ExecutionPlan, path: &DataPath) -> Arc<LemmaType> {
    let (_, _, definition) = template
        .peel_data(path)
        .expect("BUG: row field path must peel on template");
    Arc::clone(
        definition
            .resolved_type_arc()
            .expect("BUG: row data end must be typed data of the row spec"),
    )
}

/// Type of row data `path` in a data map still being planned (self-list rows), peeling
/// named-`uses` hops into `row_templates`.
pub(crate) fn row_data_type(
    data: &IndexMap<DataPath, DataDefinition>,
    row_templates: &BTreeMap<SpecIdentity, Arc<ExecutionPlan>>,
    path: &DataPath,
) -> Arc<LemmaType> {
    Arc::clone(
        crate::planning::normalize::lookup_data_definition(data, row_templates, path)
            .and_then(DataDefinition::resolved_type_arc)
            .expect("BUG: row data end must be typed data of the row spec"),
    )
}

/// Default value the row template gives `path` when a row leaves it unbound.
pub(crate) fn template_default(template: &ExecutionPlan, path: &DataPath) -> Option<TypedLiteral> {
    let (_, _, definition) = template
        .peel_data(path)
        .expect("BUG: key field must peel on template");
    let lemma_type = definition
        .resolved_type_arc()
        .expect("BUG: key field must be typed data");
    if let Some(bound) = definition.bound_value() {
        return Some(typed_from_bound(bound, lemma_type));
    }
    definition.value().map(|literal| TypedLiteral {
        value: literal.value,
        lemma_type: Arc::clone(lemma_type),
    })
}

/// Default from a local data map (self-list root while the plan is still building).
pub(crate) fn template_default_local(
    data: &IndexMap<DataPath, DataDefinition>,
    path: &DataPath,
) -> Option<TypedLiteral> {
    let definition = data
        .get(path)
        .expect("BUG: key field must be template data");
    let lemma_type = definition
        .resolved_type_arc()
        .expect("BUG: key field must be typed data");
    if let Some(bound) = definition.bound_value() {
        return Some(typed_from_bound(bound, lemma_type));
    }
    definition.value().map(|literal| TypedLiteral {
        value: literal.value,
        lemma_type: Arc::clone(lemma_type),
    })
}

/// Typed value of a bound field, keeping the written unit.
pub(crate) fn typed_from_bound(
    bound: &BoundValueKind,
    lemma_type: &Arc<LemmaType>,
) -> TypedLiteral {
    let lemma_type = match &bound.unit {
        Some(unit)
            if matches!(
                &lemma_type.specifications,
                TypeSpecification::Measure { .. } | TypeSpecification::Ratio { .. }
            ) =>
        {
            Arc::new(lemma_type.as_ref().clone().with_unit(unit.as_ref()))
        }
        Some(_) | None => Arc::clone(lemma_type),
    };
    TypedLiteral {
        value: bound.value.clone(),
        lemma_type,
    }
}

/// Key values of one row and how to name the row in an error.
pub(crate) struct KeyedRow {
    pub label: String,
    pub values: Vec<TypedLiteral>,
}

fn describe_key(fields: &[String], values: &[TypedLiteral]) -> String {
    fields
        .iter()
        .zip(values)
        .map(|(field, value)| format!("{field} {}", value.display_value()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn is_range(value: &TypedLiteral) -> bool {
    matches!(value.value, ValueKind::Range(..))
}

fn strictly_less(left: &TypedLiteral, right: &TypedLiteral) -> bool {
    match comparison_operation(
        &left.value,
        &left.lemma_type,
        &ComparisonComputation::LessThan,
        &right.value,
        &right.lemma_type,
    ) {
        OperationResult::Value(bound) => match bound.value {
            ValueKind::Boolean(less) => less,
            other => panic!("BUG: comparison returned non-boolean {other:?}"),
        },
        OperationResult::Veto(veto) => {
            panic!("BUG: key endpoints of one field must compare: {veto}")
        }
    }
}

/// Half-open `[lo, hi)` endpoints of a range key value.
fn half_open(value: &TypedLiteral) -> (&TypedLiteral, &TypedLiteral) {
    let ValueKind::Range(left, right) = &value.value else {
        panic!("BUG: half_open on non-range key value");
    };
    let endpoint_type = &left.lemma_type;
    let (lo, _) = ordered_endpoints(&left.value, &right.value, endpoint_type)
        .unwrap_or_else(|veto| panic!("BUG: range key endpoints must compare: {veto}"));
    if std::ptr::eq(lo, &left.value) {
        (left, right)
    } else {
        (right, left)
    }
}

fn ranges_overlap(a: &TypedLiteral, b: &TypedLiteral) -> bool {
    let (a_lo, a_hi) = half_open(a);
    let (b_lo, b_hi) = half_open(b);
    strictly_less(a_lo, b_hi) && strictly_less(b_lo, a_hi)
}

/// First pair of rows whose keys collide: equal exact fields and overlapping range fields.
pub(crate) fn key_conflict(fields: &[String], rows: &[KeyedRow]) -> Option<String> {
    let mut groups: HashMap<Vec<&ValueKind>, Vec<usize>> = HashMap::new();
    for (index, row) in rows.iter().enumerate() {
        let exact: Vec<&ValueKind> = row
            .values
            .iter()
            .filter(|value| !is_range(value))
            .map(|value| &value.value)
            .collect();
        groups.entry(exact).or_default().push(index);
    }
    let mut conflicts: Vec<(usize, usize)> = Vec::new();
    for members in groups.values() {
        for (position, &first) in members.iter().enumerate() {
            for &second in &members[position + 1..] {
                let ranges_collide = rows[first]
                    .values
                    .iter()
                    .zip(&rows[second].values)
                    .filter(|(value, _)| is_range(value))
                    .all(|(a, b)| ranges_overlap(a, b));
                if ranges_collide {
                    conflicts.push((first, second));
                }
            }
        }
    }
    let (first, second) = conflicts.into_iter().min()?;
    let has_range = rows[first].values.iter().any(is_range);
    let first_row = &rows[first];
    let second_row = &rows[second];
    Some(if has_range {
        format!(
            "key ranges overlap: {} ({}) and {} ({})",
            first_row.label,
            describe_key(fields, &first_row.values),
            second_row.label,
            describe_key(fields, &second_row.values),
        )
    } else {
        format!(
            "duplicate key {} in {} and {}",
            describe_key(fields, &first_row.values),
            first_row.label,
            second_row.label,
        )
    })
}

//! Instance lists (`data x: spec list`): rows written in the spec, row fields, and key checks.
//!
//! A row written in the spec is data of a [`RowStore`]: its bindings are checked once
//! against the row spec's template plan. Rows supplied by evaluation data are values
//! checked against the same template plans ([`InstanceLists::templates`]). Both
//! evaluate through the template plan as row instances.

use crate::computation::datetime::{semantic_datetime_to_chrono, semantic_time_to_chrono_datetime};
use crate::computation::range::ordered_endpoints;
use crate::computation::rational::RationalInteger;
use crate::computation::{OperationResult, VetoType};
use crate::engine::Context;
use crate::error::Error;
use crate::limits::ResourceLimits;
use crate::parsing::ast::{DateTimeValue, EffectiveDate, LemmaRepository, LemmaSpec};
use crate::planning::discovery;
use crate::planning::execution_plan::ExecutionPlan;
use crate::planning::semantics::{
    semantic_date_as_time, BoundValueKind, DataDefinition, DataPath, InstanceKey, LemmaType,
    ListKey, ListValue, ReferenceTarget, RowBinding, RowEnd, RowInstance, RulePath, SpecIdentity,
    SpecVersionKey, TypeSpecification, TypedLiteral, ValueKind,
};
use chrono::NaiveDateTime;
use indexmap::IndexMap;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

/// Instance lists of one consumer slice: row-spec template plans and spec-written rows.
#[derive(Debug)]
pub(crate) struct InstanceLists {
    /// The spec being planned. Rows of its self-lists read the plan being built,
    /// so it has no entry in [`Self::templates`].
    pub self_identity: SpecIdentity,
    /// Plans rows of each spec are checked and evaluated against: `uses` instances,
    /// rows written in a spec, rows supplied by evaluation data and option rows.
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

    /// Rows of `identity` as instance-list commands read them.
    ///
    /// `None` when the row Spec is not self and was never imported (`uses`), so no
    /// template plan exists. Callers must report a planning Error, never panic.
    pub(crate) fn row_template_of(&self, identity: &SpecIdentity) -> Option<RowTemplate> {
        if *identity == self.self_identity {
            return Some(RowTemplate::Root);
        }
        self.templates
            .get(identity)
            .map(|plan| RowTemplate::Plan(Arc::clone(plan)))
    }
}

/// Where rows of an instance list read their fields while the consumer is planned.
#[derive(Clone)]
pub(crate) enum RowTemplate {
    /// Row spec planned on its own.
    Plan(Arc<ExecutionPlan>),
    /// Self-list: rows are the spec being planned.
    Root,
}

/// Rows written under one `data x: spec list` declaration, as data. One store per
/// declaration, built once in the plan of the declaring spec. Rows evaluate through
/// the row spec's template plan, exactly like rows supplied as evaluation data.
#[derive(Debug, Clone)]
pub struct RowStore {
    /// Rows in declaration order; row `i` is number `i + 1`. Origins and reference
    /// targets are paths of the declaring plan; a reading instance owns them.
    pub rows: Vec<Arc<RowInstance>>,
    /// Key fields of a `-> key` list whose rows passed the key check; empty otherwise.
    pub key: Vec<StoreKeyField>,
    /// Index over `key`; `None` when `key` is empty.
    pub index: Option<Arc<KeyIndex>>,
    /// Position of the first row of each binding shape, in row order. Rows of one
    /// shape agree on which slots are unset and which hold a written value, and hold
    /// every other binding equal, so planning's walks over the rows' reads take one
    /// row per shape.
    shapes: Vec<u32>,
}

/// One slot of a row's binding shape (see [`RowStore::shapes`]).
#[derive(PartialEq, Eq, Hash)]
enum ShapeSlot<'r> {
    Unset,
    Written,
    Deferred(&'r ReferenceTarget, u32),
    Rows(Vec<*const RowInstance>, u32),
    Instance(*const RowInstance),
}

/// Index over `key` of `rows`; the key conflict message when two rows collide.
fn store_index(rows: &[Arc<RowInstance>], key: &[StoreKeyField]) -> Result<KeyIndex, String> {
    KeyIndex::build(key_names(key), rows.len(), |row, field| {
        store_key_value(&rows[row], &key[field])
    })
    .map_err(|(fields, conflict)| {
        conflict.message(&fields, |row| {
            (
                format!("row {}", row + 1),
                store_key_values(&rows[row], key),
            )
        })
    })
}

fn binding_shapes(rows: &[Arc<RowInstance>]) -> Vec<u32> {
    let mut seen: std::collections::HashSet<Vec<ShapeSlot<'_>>> = std::collections::HashSet::new();
    let mut shapes = Vec::new();
    for (position, row) in rows.iter().enumerate() {
        let shape: Vec<ShapeSlot<'_>> = row
            .bindings
            .iter()
            .map(|binding| match binding {
                None => ShapeSlot::Unset,
                Some(RowBinding::Bound(_)) => ShapeSlot::Written,
                Some(RowBinding::Deferred { target, hops }) => ShapeSlot::Deferred(target, *hops),
                Some(RowBinding::Rows { rows, hops, .. }) => {
                    ShapeSlot::Rows(rows.iter().map(Arc::as_ptr).collect(), *hops)
                }
                Some(RowBinding::Instance(row)) => ShapeSlot::Instance(Arc::as_ptr(row)),
            })
            .collect();
        if seen.insert(shape) {
            shapes.push(u32::try_from(position).expect("BUG: row count exceeds u32"));
        }
    }
    shapes
}

/// Where a key field's value lives in a stored row: its template slot, or the
/// template default when the row leaves the slot unset.
#[derive(Debug, Clone)]
pub struct StoreKeyField {
    pub name: String,
    pub slot: usize,
    pub field_type: Arc<LemmaType>,
    pub default: Option<TypedLiteral>,
}

impl RowStore {
    pub(crate) fn unkeyed(rows: Vec<Arc<RowInstance>>) -> Self {
        let shapes = binding_shapes(&rows);
        Self {
            rows,
            key: Vec::new(),
            index: None,
            shapes,
        }
    }

    /// Store whose `key` fields hold written values on every row. When two rows
    /// collide, the key conflict message and the store without a key.
    pub(crate) fn keyed(
        rows: Vec<Arc<RowInstance>>,
        key: Vec<StoreKeyField>,
    ) -> Result<Self, (String, Self)> {
        match store_index(&rows, &key) {
            Ok(index) => Ok(Self {
                shapes: binding_shapes(&rows),
                rows,
                key,
                index: Some(Arc::new(index)),
            }),
            Err(message) => Err((message, Self::unkeyed(rows))),
        }
    }

    /// One row of each binding shape, in row order.
    pub(crate) fn shape_rows(&self) -> impl Iterator<Item = &Arc<RowInstance>> {
        self.shapes
            .iter()
            .map(|&position| &self.rows[position as usize])
    }

    pub(crate) fn key_names(&self) -> Vec<String> {
        key_names(&self.key)
    }

    /// Key value of key field `field` of row `row` (0-based).
    pub(crate) fn key_value(&self, row: usize, field: usize) -> &ValueKind {
        store_key_value(&self.rows[row], &self.key[field])
    }

    /// Typed key values of row `row` (0-based), in key field order.
    pub(crate) fn key_values(&self, row: usize) -> Vec<TypedLiteral> {
        store_key_values(&self.rows[row], &self.key)
    }
}

fn key_names(key: &[StoreKeyField]) -> Vec<String> {
    key.iter().map(|field| field.name.clone()).collect()
}

fn store_key_value<'r>(row: &'r RowInstance, field: &'r StoreKeyField) -> &'r ValueKind {
    match &row.bindings[field.slot] {
        Some(RowBinding::Bound(OperationResult::Value(bound))) => &bound.value,
        None => {
            &field
                .default
                .as_ref()
                .expect("BUG: an unset key field of a stored row has a template default")
                .value
        }
        Some(other) => panic!("BUG: stored key field `{}` bound to {other:?}", field.name),
    }
}

fn store_key_values(row: &RowInstance, key: &[StoreKeyField]) -> Vec<TypedLiteral> {
    key.iter()
        .map(|field| match &row.bindings[field.slot] {
            Some(RowBinding::Bound(OperationResult::Value(bound))) => {
                typed_from_bound(bound, &field.field_type)
            }
            None => field
                .default
                .clone()
                .expect("BUG: an unset key field of a stored row has a template default"),
            Some(other) => panic!("BUG: stored key field `{}` bound to {other:?}", field.name),
        })
        .collect()
}

/// Rows written in the spec, in order, as template-checked data.
#[derive(Debug, Clone)]
pub struct InstanceListSource {
    pub identity: SpecIdentity,
    pub key: ListKey,
    pub store: Arc<RowStore>,
    /// Type of one row of the list.
    pub row_type: Arc<LemmaType>,
}

impl InstanceListSource {
    pub(crate) fn new(identity: SpecIdentity, key: ListKey, store: Arc<RowStore>) -> Self {
        let row_type = Arc::new(LemmaType::primitive(TypeSpecification::Spec {
            spec: identity.clone(),
        }));
        Self {
            identity,
            key,
            store,
            row_type,
        }
    }

    /// The list's rows as `owner` reads them, with the store's index.
    pub(crate) fn list(&self, owner: Option<InstanceKey>) -> ListValue {
        ListValue::stored_rows(&self.store, owner, Arc::clone(&self.row_type))
    }
}

/// Static row nesting depth of each spec-written list type against
/// `max_list_depth`: the declaring spec is level 1 and every row level adds one,
/// so `data x: t list` in `s` nests `2 + depth of t's own list fields`. A type
/// that reaches a list-field cycle (self-lists included) has unbounded depth:
/// it stays legal and its request rows are bounded at evaluation
/// (`check_list_tree`), as today. Only finitely deep chains are rejected here.
pub(crate) fn check_instance_list_depths(
    instance_lists: &InstanceLists,
    data: &IndexMap<DataPath, DataDefinition>,
    max_list_depth: usize,
) -> Result<(), Vec<Error>> {
    #[derive(Clone, Copy)]
    enum Depth {
        Finite(usize),
        /// Reaches a list-field cycle; rows can nest arbitrarily deep.
        Unbounded,
    }

    fn rows_depth<'a>(
        root: &'a SpecIdentity,
        instance_lists: &'a InstanceLists,
        visiting: &mut BTreeSet<SpecIdentity>,
        memo: &mut HashMap<SpecIdentity, Depth>,
    ) -> Depth {
        struct Visit<'a> {
            identity: &'a SpecIdentity,
            children: Vec<&'a SpecIdentity>,
            next: usize,
            deepest: usize,
        }
        let mut stack: Vec<Visit<'a>> = Vec::new();
        let mut entering = Some(root);
        let mut returned = None;
        loop {
            if let Some(identity) = entering.take() {
                if let Some(depth) = memo.get(identity) {
                    returned = Some(*depth);
                } else if visiting.contains(identity) {
                    returned = Some(Depth::Unbounded);
                } else if let Some(template) = instance_lists.templates.get(identity) {
                    visiting.insert(identity.clone());
                    stack.push(Visit {
                        identity,
                        children: template
                            .instance_lists
                            .values()
                            .map(|source| &source.identity)
                            .collect(),
                        next: 0,
                        deepest: 0,
                    });
                } else {
                    // No template plan: the list declaration is rejected elsewhere.
                    returned = Some(Depth::Finite(0));
                }
            }
            let Some(visit) = stack.last_mut() else {
                return returned.expect("BUG: rows depth of the root is computed");
            };
            let done = match returned.take() {
                Some(Depth::Unbounded) => Some(Depth::Unbounded),
                Some(Depth::Finite(nested)) => {
                    visit.deepest = visit.deepest.max(1 + nested);
                    None
                }
                None => None,
            };
            let done = match (done, visit.children.get(visit.next).copied()) {
                (Some(depth), _) => Some(depth),
                (None, Some(child)) => {
                    visit.next += 1;
                    entering = Some(child);
                    None
                }
                (None, None) => Some(Depth::Finite(visit.deepest)),
            };
            if let Some(depth) = done {
                let visit = stack.pop().expect("BUG: a finished visit is on the stack");
                visiting.remove(visit.identity);
                memo.insert(visit.identity.clone(), depth);
                returned = Some(depth);
            }
        }
    }

    let mut errors = Vec::new();
    let mut memo: HashMap<SpecIdentity, Depth> = HashMap::new();
    for (path, source) in &instance_lists.sources {
        // Self-lists are bounded at evaluation.
        if source.identity == instance_lists.self_identity {
            continue;
        }
        let mut visiting = BTreeSet::from([instance_lists.self_identity.clone()]);
        let Depth::Finite(nested) =
            rows_depth(&source.identity, instance_lists, &mut visiting, &mut memo)
        else {
            continue;
        };
        let total = 2 + nested;
        if total > max_list_depth {
            errors.push(crate::error::Error::resource_limit_exceeded(
                "max_list_depth",
                max_list_depth.to_string(),
                total.to_string(),
                format!(
                    "Rows of `{}` nest {total} levels deep counting the spec itself; reduce row nesting",
                    path.input_key()
                ),
                Some(data[path].source().clone()),
                None,
                None,
            ));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Pass-wide share of spec template plans (`SpecIdentity` → [`Arc<ExecutionPlan>`]).
///
/// Import and instance-list row templates and root slice plans intern here, so one
/// slice of one version ([`SpecIdentity`]) is one `Arc` across the pass.
#[derive(Debug, Default)]
pub(crate) struct SpecTemplateInterner {
    templates: HashMap<SpecIdentity, Arc<ExecutionPlan>>,
}

impl SpecTemplateInterner {
    pub(crate) fn get(&self, identity: &SpecIdentity) -> Option<Arc<ExecutionPlan>> {
        self.templates.get(identity).map(Arc::clone)
    }

    /// Record a plan built in this pass. Each slice is built once per pass, so an
    /// existing entry is a bug.
    pub(crate) fn insert(&mut self, identity: SpecIdentity, template: Arc<ExecutionPlan>) {
        let previous = self.templates.insert(identity.clone(), template);
        assert!(
            previous.is_none(),
            "BUG: template {identity} built twice in one pass"
        );
    }
}

/// The temporal slices of one spec version: the starts
/// ([`crate::planning::discovery::plan_breakpoints`]) and the version's exclusive end.
#[derive(Debug)]
pub(crate) struct VersionSlices {
    starts: Vec<EffectiveDate>,
    end: Option<DateTimeValue>,
}

impl VersionSlices {
    pub(crate) fn starts(&self) -> &[EffectiveDate] {
        &self.starts
    }

    /// Identity of `spec` in the slice that holds `at`. `at` lies in the version's range:
    /// the version was resolved at `at`.
    pub(crate) fn identity_at(
        &self,
        repository: &LemmaRepository,
        spec: &LemmaSpec,
        at: &EffectiveDate,
    ) -> SpecIdentity {
        if let Some(end) = &self.end {
            assert!(
                *at < EffectiveDate::DateTimeValue(end.clone()),
                "BUG: spec '{}' resolved at {at:?}, after its version ends",
                spec.name
            );
        }
        let slice = self
            .starts
            .iter()
            .rev()
            .find(|start| *start <= at)
            .expect("BUG: a spec is resolved at an instant inside its version, whose first slice starts at the version")
            .clone();
        SpecIdentity::of(repository, spec, slice)
    }
}

/// [`VersionSlices`] of every spec version a pass asked for, computed once per version.
#[derive(Debug, Default)]
pub(crate) struct SpecSlices {
    by_version: HashMap<SpecVersionKey, Arc<VersionSlices>>,
}

impl SpecSlices {
    pub(crate) fn of(
        &mut self,
        context: &Context,
        repository: &Arc<LemmaRepository>,
        spec: &LemmaSpec,
        limits: &ResourceLimits,
    ) -> Result<Arc<VersionSlices>, Vec<Error>> {
        let key = SpecVersionKey::of(repository, spec);
        if let Some(slices) = self.by_version.get(&key) {
            return Ok(Arc::clone(slices));
        }
        let spec_set = context
            .spec_set(repository, &spec.name)
            .expect("BUG: sliced spec missing from context");
        let starts = discovery::plan_breakpoints(context, spec_set, spec, limits)?;
        let (_, end) = spec_set.effective_range(spec);
        let slices = Arc::new(VersionSlices { starts, end });
        self.by_version.insert(key, Arc::clone(&slices));
        Ok(slices)
    }

    /// Identity of `spec` in the slice that holds `at`.
    pub(crate) fn identity_at(
        &mut self,
        context: &Context,
        repository: &Arc<LemmaRepository>,
        spec: &LemmaSpec,
        at: &EffectiveDate,
        limits: &ResourceLimits,
    ) -> Result<SpecIdentity, Vec<Error>> {
        Ok(self
            .of(context, repository, spec, limits)?
            .identity_at(repository, spec, at))
    }
}

/// Interners shared by every graph built in one planning pass.
#[derive(Debug, Default)]
pub(crate) struct PassInterners {
    pub spec_templates: SpecTemplateInterner,
    pub slices: SpecSlices,
}

/// Fields of rows of one spec: its template plan, or the spec being planned.
#[derive(Clone, Copy)]
pub(crate) enum RowEndView<'a> {
    Plan(&'a ExecutionPlan),
    Root {
        data: &'a IndexMap<DataPath, DataDefinition>,
        rules: &'a BTreeSet<RulePath>,
    },
}

impl<'a> RowEndView<'a> {
    fn data(&self) -> &'a IndexMap<DataPath, DataDefinition> {
        match self {
            RowEndView::Plan(plan) => &plan.data,
            RowEndView::Root { data, .. } => data,
        }
    }

    fn has_rule(&self, rule: &RulePath) -> bool {
        match self {
            RowEndView::Plan(plan) => plan.rules.contains_key(rule),
            RowEndView::Root { rules, .. } => rules.contains(rule),
        }
    }
}

/// One read of a member chain: `end` read on a row of `identity`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RowHop {
    pub identity: SpecIdentity,
    pub end: RowEnd,
}

/// Field path `parts` on rows of `identity` read through `view`, hop by hop. Every
/// part but the last names a `uses` slot, read as that instance on its spec's
/// template plan (`templates`); the last names data or a rule. `None` when a hop is
/// not a `uses` slot or the last part is neither data nor a rule.
pub(crate) fn row_end_chain(
    identity: &SpecIdentity,
    view: RowEndView<'_>,
    templates: &BTreeMap<SpecIdentity, Arc<ExecutionPlan>>,
    parts: &[&str],
) -> Option<Vec<RowHop>> {
    let (name, hops) = parts.split_last()?;
    let mut chain = Vec::with_capacity(parts.len());
    let mut identity = identity.clone();
    let mut view = view;
    for hop in hops {
        let path = DataPath::local((*hop).to_string());
        let DataDefinition::Import { row, .. } = view.data().get(&path)? else {
            return None;
        };
        let next = row.identity.as_ref().clone();
        chain.push(RowHop {
            identity: std::mem::replace(&mut identity, next),
            end: RowEnd::Data(path),
        });
        view = RowEndView::Plan(templates.get(&identity).unwrap_or_else(|| {
            panic!("BUG: `uses` slot `{hop}` of a row spec has no template of `{identity}`")
        }));
    }
    let path = DataPath::local((*name).to_string());
    let end = if view.data().contains_key(&path) {
        RowEnd::Data(path)
    } else {
        let rule = RulePath::new(Vec::new(), (*name).to_string());
        if !view.has_rule(&rule) {
            return None;
        }
        RowEnd::Rule(rule)
    };
    chain.push(RowHop { identity, end });
    Some(chain)
}

/// Result type of field `end` on rows of `template`.
pub(crate) fn row_end_type(template: &ExecutionPlan, end: &RowEnd) -> Arc<LemmaType> {
    match end {
        RowEnd::Data(path) => row_data_type(&template.data, path),
        RowEnd::Rule(path) => Arc::clone(
            &template
                .rules
                .get(path)
                .expect("BUG: row rule end missing from template plan")
                .rule_type,
        ),
    }
}

/// Type of row data `path` of a spec: a `uses` slot is an instance of its spec.
pub(crate) fn row_data_type(
    data: &IndexMap<DataPath, DataDefinition>,
    path: &DataPath,
) -> Arc<LemmaType> {
    match data.get(path) {
        Some(DataDefinition::Import { row, .. }) => {
            Arc::new(LemmaType::primitive(TypeSpecification::Spec {
                spec: row.identity.as_ref().clone(),
            }))
        }
        definition => Arc::clone(
            definition
                .and_then(DataDefinition::resolved_type_arc)
                .expect("BUG: row data end must be typed data of the row spec"),
        ),
    }
}

/// Default value `data` gives `path` when a row leaves it unbound.
pub(crate) fn default_of(
    data: &IndexMap<DataPath, DataDefinition>,
    path: &DataPath,
) -> Option<TypedLiteral> {
    let definition = data.get(path).expect("BUG: key field must be row data");
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

/// Two rows (0-based, `first < second`) whose keys collide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyConflict {
    Duplicate { first: usize, second: usize },
    Overlap { first: usize, second: usize },
}

impl KeyConflict {
    /// Error text naming both rows; `row` gives a row's label and typed key values.
    pub(crate) fn message(
        self,
        fields: &[String],
        row: impl Fn(usize) -> (String, Vec<TypedLiteral>),
    ) -> String {
        match self {
            KeyConflict::Duplicate { first, second } => {
                let (first_label, first_values) = row(first);
                let (second_label, _) = row(second);
                format!(
                    "Duplicate key {} appears in both {first_label} and {second_label}.",
                    describe_key(fields, &first_values),
                )
            }
            KeyConflict::Overlap { first, second } => {
                let (first_label, first_values) = row(first);
                let (second_label, second_values) = row(second);
                format!(
                    "Key ranges overlap between {first_label} ({}) and {second_label} ({}).",
                    describe_key(fields, &first_values),
                    describe_key(fields, &second_values),
                )
            }
        }
    }
}

/// Key fields for veto text: `band: 23 hour, and tier: gold`.
pub(crate) fn describe_key(fields: &[String], values: &[TypedLiteral]) -> String {
    fields
        .iter()
        .zip(values)
        .map(|(field, value)| format!("{field}: {}", value.display_value()))
        .collect::<Vec<_>>()
        .join(", and ")
}

/// Half-open `[lo, hi)` endpoints of a range key value.
fn half_open(value: &ValueKind) -> (&TypedLiteral, &TypedLiteral) {
    let ValueKind::Range(left, right) = value else {
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

/// Canonical hashable and orderable key value for `-> key` indexing.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum KeyValue {
    Number(RationalInteger),
    Text(Arc<str>),
    Boolean(bool),
    Date(NaiveDateTime),
    Time(NaiveDateTime),
}

impl KeyValue {
    pub fn from_value(value: &ValueKind) -> Result<Self, String> {
        match value {
            ValueKind::Number(n) | ValueKind::Measure(n) | ValueKind::Ratio(n) => {
                Ok(Self::Number(n.clone()))
            }
            ValueKind::Text(s) => Ok(Self::Text(Arc::clone(s))),
            ValueKind::Boolean(b) => Ok(Self::Boolean(*b)),
            ValueKind::Date(dt) => {
                semantic_datetime_to_chrono(dt).map(|c| Self::Date(c.naive_utc()))
            }
            ValueKind::Time(t) => {
                semantic_time_to_chrono_datetime(t).map(|c| Self::Time(c.naive_utc()))
            }
            ValueKind::Spec(..) => {
                unreachable!("BUG: key fields are data, and data are never spec-typed")
            }
            ValueKind::Range(..) | ValueKind::List(..) | ValueKind::UnscaledMeasure { .. } => {
                unreachable!("BUG: KeyValue::from_value called on non-scalar {value:?}")
            }
        }
    }

    pub fn try_cmp(&self, other: &Self) -> Result<Ordering, String> {
        match (self, other) {
            (Self::Number(a), Self::Number(b)) => a.try_cmp(b).map_err(|e| e.to_string()),
            (Self::Date(a), Self::Date(b)) => Ok(a.cmp(b)),
            (Self::Time(a), Self::Time(b)) => Ok(a.cmp(b)),
            (a, b) => unreachable!("BUG: KeyValue::try_cmp on non-ordered keys {a:?} vs {b:?}"),
        }
    }
}

/// One half-open interval `[lo, hi)` of a range key field.
#[derive(Debug, Clone)]
pub struct RangeBounds {
    pub lo: KeyValue,
    pub hi: KeyValue,
}

impl RangeBounds {
    pub fn from_range(value: &ValueKind) -> Result<Self, String> {
        let (lo_typed, hi_typed) = half_open(value);
        Ok(Self {
            lo: KeyValue::from_value(&lo_typed.value)?,
            hi: KeyValue::from_value(&hi_typed.value)?,
        })
    }

    pub fn contains(&self, probe: &KeyValue) -> Result<bool, String> {
        let lo_cmp = probe.try_cmp(&self.lo)?;
        if lo_cmp == Ordering::Less {
            return Ok(false);
        }
        let hi_cmp = probe.try_cmp(&self.hi)?;
        Ok(hi_cmp == Ordering::Less)
    }

    pub fn overlaps(&self, other: &Self) -> Result<bool, String> {
        let a_lt_b = self.lo.try_cmp(&other.hi)? == Ordering::Less;
        let b_lt_a = other.lo.try_cmp(&self.hi)? == Ordering::Less;
        Ok(a_lt_b && b_lt_a)
    }
}

/// One row's range key bounds inside an exact-key group.
#[derive(Debug, Clone)]
pub struct RangeEntry {
    pub row_index: u32,
    pub ranges: Vec<RangeBounds>,
}

/// Per-exact-group range index: entries sorted by first range's `lo`, with prefix max `hi`.
#[derive(Debug, Clone, Default)]
pub struct GroupRangeIndex {
    pub entries: Vec<RangeEntry>,
    pub max_hi: Vec<KeyValue>,
}

/// Range index across all exact-key groups.
#[derive(Debug, Clone, Default)]
pub struct RangeIndex {
    /// Positions in the key field list that hold range values.
    pub range_positions: Vec<usize>,
    /// Whether each range position is a `time range` (for `date in time range` projection).
    pub is_time_range: Vec<bool>,
    pub groups: HashMap<Vec<KeyValue>, GroupRangeIndex>,
}

/// Prebuilt index over a keyed list's rows.
#[derive(Debug, Clone)]
pub struct KeyIndex {
    /// Key field names, in key order.
    pub fields: Vec<String>,
    pub lookup: KeyLookup,
}

/// How a [`KeyIndex`] finds a row.
#[derive(Debug, Clone)]
pub enum KeyLookup {
    /// No key field is a range: each key names exactly one row.
    Exact(HashMap<Vec<KeyValue>, u32>),
    Range(RangeIndex),
}

impl KeyIndex {
    /// Build a `KeyIndex` from `rows` and detect the first pair `(first, second)` (with `first < second`
    /// in lexicographic `(first, second)` order) whose keys collide.
    /// Index over rows numbered from 1, one typed key value per field of `fields`.
    /// Errs with the conflict message.
    pub(crate) fn build_numbered(
        fields: Vec<String>,
        keys: &[Vec<TypedLiteral>],
    ) -> Result<Self, String> {
        Self::build(fields, keys.len(), |row, field| &keys[row][field].value).map_err(
            |(fields, conflict)| {
                conflict.message(&fields, |row| {
                    (format!("row {}", row + 1), keys[row].clone())
                })
            },
        )
    }

    /// Index keyed by `fields`; `key(row, field)` is the key value of field `field`
    /// of row `row`. On a conflict, the fields back with it.
    pub(crate) fn build<'v>(
        fields: Vec<String>,
        rows: usize,
        key: impl Fn(usize, usize) -> &'v ValueKind,
    ) -> Result<Self, (Vec<String>, KeyConflict)> {
        let width = fields.len();
        if rows == 0 {
            return Ok(Self {
                fields,
                lookup: KeyLookup::Exact(HashMap::new()),
            });
        }
        let range_positions: Vec<usize> = (0..width)
            .filter(|&field| matches!(key(0, field), ValueKind::Range(..)))
            .collect();
        let has_range = !range_positions.is_empty();

        if !has_range {
            let mut exact: HashMap<Vec<KeyValue>, u32> = HashMap::with_capacity(rows);
            let mut min_conflict: Option<(usize, usize)> = None;
            for index in 0..rows {
                let mut row_key = Vec::with_capacity(width);
                for field in 0..width {
                    row_key.push(
                        KeyValue::from_value(key(index, field))
                            .expect("BUG: key values were validated when bound"),
                    );
                }
                match exact.entry(row_key) {
                    std::collections::hash_map::Entry::Occupied(first) => {
                        let pair = (*first.get() as usize, index);
                        min_conflict = Some(match min_conflict {
                            Some(prev) => prev.min(pair),
                            None => pair,
                        });
                    }
                    std::collections::hash_map::Entry::Vacant(slot) => {
                        slot.insert(u32::try_from(index).expect("BUG: row index fits u32"));
                    }
                }
            }
            if let Some((first, second)) = min_conflict {
                return Err((fields, KeyConflict::Duplicate { first, second }));
            }
            return Ok(Self {
                fields,
                lookup: KeyLookup::Exact(exact),
            });
        }

        let is_time_range: Vec<bool> = range_positions
            .iter()
            .map(|&field| match key(0, field) {
                ValueKind::Range(lo, _) => matches!(lo.value, ValueKind::Time(_)),
                other => unreachable!("BUG: range key position holds {other:?}"),
            })
            .collect();
        let mut raw_groups: HashMap<Vec<KeyValue>, Vec<RangeEntry>> = HashMap::new();

        for index in 0..rows {
            let mut exact_key = Vec::with_capacity(width - range_positions.len());
            let mut ranges = Vec::with_capacity(range_positions.len());
            for field in 0..width {
                let value = key(index, field);
                if matches!(value, ValueKind::Range(..)) {
                    ranges.push(
                        RangeBounds::from_range(value)
                            .expect("BUG: range key bounds were validated when bound"),
                    );
                } else {
                    exact_key.push(
                        KeyValue::from_value(value)
                            .expect("BUG: key values were validated when bound"),
                    );
                }
            }
            let row_u32 = u32::try_from(index).expect("BUG: row index fits u32");
            raw_groups.entry(exact_key).or_default().push(RangeEntry {
                row_index: row_u32,
                ranges,
            });
        }

        let mut min_conflict: Option<(usize, usize)> = None;
        let mut groups: HashMap<Vec<KeyValue>, GroupRangeIndex> =
            HashMap::with_capacity(raw_groups.len());

        for (exact_key, mut entries) in raw_groups {
            // Sort by first range's `lo`, then by `row_index`.
            entries.sort_by(|a, b| {
                a.ranges[0]
                    .lo
                    .try_cmp(&b.ranges[0].lo)
                    .expect("BUG: range key endpoints of one field compare")
                    .then_with(|| a.row_index.cmp(&b.row_index))
            });

            // Sweep-line conflict detection: since sorted by `ranges[0].lo`, any later entry `j > i`
            // with `entries[j].ranges[0].lo >= entries[i].ranges[0].hi` cannot overlap `entries[i]` on range 0.
            for i in 0..entries.len() {
                let first_hi = &entries[i].ranges[0].hi;
                for j in (i + 1)..entries.len() {
                    let cmp = entries[j].ranges[0]
                        .lo
                        .try_cmp(first_hi)
                        .expect("BUG: range key endpoints of one field compare");
                    if cmp != Ordering::Less {
                        break;
                    }
                    let all_overlap = entries[i]
                        .ranges
                        .iter()
                        .zip(&entries[j].ranges)
                        .skip(1)
                        .all(|(ra, rb)| {
                            ra.overlaps(rb)
                                .expect("BUG: range key endpoints of one field compare")
                        });
                    if all_overlap {
                        let r1 = entries[i].row_index as usize;
                        let r2 = entries[j].row_index as usize;
                        let pair = if r1 < r2 { (r1, r2) } else { (r2, r1) };
                        min_conflict = Some(match min_conflict {
                            Some(prev) => prev.min(pair),
                            None => pair,
                        });
                    }
                }
            }

            let mut max_hi: Vec<KeyValue> = Vec::with_capacity(entries.len());
            for entry in &entries {
                let hi = entry.ranges[0].hi.clone();
                let next_max = match max_hi.last() {
                    None => hi,
                    Some(prev) => {
                        if prev
                            .try_cmp(&hi)
                            .expect("BUG: range key endpoints of one field compare")
                            == Ordering::Less
                        {
                            hi
                        } else {
                            prev.clone()
                        }
                    }
                };
                max_hi.push(next_max);
            }

            groups.insert(exact_key, GroupRangeIndex { entries, max_hi });
        }

        if let Some((first, second)) = min_conflict {
            return Err((fields, KeyConflict::Overlap { first, second }));
        }

        Ok(Self {
            fields,
            lookup: KeyLookup::Range(RangeIndex {
                range_positions,
                is_time_range,
                groups,
            }),
        })
    }

    /// Probe the index with `wanted` key operands (one per key field, in key field order).
    /// Returns the 0-based position of the first matching row, or a veto when a key
    /// operand cannot be compared.
    pub(crate) fn probe(&self, wanted: &[TypedLiteral]) -> Result<Option<usize>, OperationResult> {
        let veto = |msg: String| OperationResult::Veto(VetoType::computation(msg));
        let range_index = match &self.lookup {
            KeyLookup::Exact(exact) => {
                let key = wanted
                    .iter()
                    .map(|val| KeyValue::from_value(&val.value))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(veto)?;
                return Ok(exact.get(&key).map(|&idx| idx as usize));
            }
            KeyLookup::Range(range_index) => range_index,
        };
        let mut exact_key = Vec::with_capacity(wanted.len() - range_index.range_positions.len());
        let mut range_probes = Vec::with_capacity(range_index.range_positions.len());
        for (i, val) in wanted.iter().enumerate() {
            match range_index.range_positions.iter().position(|&pos| pos == i) {
                Some(range) => {
                    let kv = match &val.value {
                        ValueKind::Date(date) if range_index.is_time_range[range] => {
                            KeyValue::from_value(&ValueKind::Time(semantic_date_as_time(date)))
                        }
                        other => KeyValue::from_value(other),
                    };
                    range_probes.push(kv.map_err(veto)?);
                }
                None => exact_key.push(KeyValue::from_value(&val.value).map_err(veto)?),
            }
        }
        let Some(group) = range_index.groups.get(&exact_key) else {
            return Ok(None);
        };
        let probe0 = &range_probes[0];
        let mut lo_failure = None;
        let upper =
            group
                .entries
                .partition_point(|entry| match entry.ranges[0].lo.try_cmp(probe0) {
                    Ok(ord) => ord != Ordering::Greater,
                    Err(msg) => {
                        lo_failure.get_or_insert(msg);
                        false
                    }
                });
        if let Some(msg) = lo_failure {
            return Err(veto(msg));
        }
        // Entries are sorted by the first range's `lo`; `max_hi` is the running maximum of
        // its `hi`, so scanning down from `upper` stops once no earlier range reaches `probe0`.
        let mut best: Option<u32> = None;
        for idx in (0..upper).rev() {
            if probe0.try_cmp(&group.max_hi[idx]).map_err(veto)? != Ordering::Less {
                break;
            }
            let entry = &group.entries[idx];
            if best.is_some_and(|b| entry.row_index >= b) {
                continue;
            }
            let mut matches_all = true;
            for (bounds, probe) in entry.ranges.iter().zip(&range_probes) {
                if !bounds.contains(probe).map_err(veto)? {
                    matches_all = false;
                    break;
                }
            }
            if matches_all {
                best = Some(entry.row_index);
            }
        }
        Ok(best.map(|idx| idx as usize))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computation::comparison_operation;
    use crate::computation::range::check_containment;
    use crate::computation::rational::rational_new;
    use crate::parsing::ast::PrimitiveKind;

    /// Key values of rows, or of probes, one row per entry.
    type Keys = Vec<Vec<TypedLiteral>>;
    use crate::planning::semantics::{
        lemma_type_for_primitive, primitive_boolean_arc, primitive_date_arc,
        primitive_date_range_arc, primitive_number_arc, primitive_ratio_arc, primitive_text_arc,
        primitive_time_arc, SemanticDateTime, SemanticTime, SemanticTimezone,
    };
    use crate::planning::semantics::{ComparisonComputation, RowOrigin};

    fn shape_row(bindings: Vec<Option<RowBinding>>) -> Arc<RowInstance> {
        Arc::new(RowInstance {
            identity: Arc::new(SpecIdentity {
                repository: None,
                spec: "rate_cell".to_string(),
                effective: EffectiveDate::Origin,
                slice: EffectiveDate::Origin,
            }),
            origin: RowOrigin::Numbered {
                list: "rates".to_string(),
                number: 1,
                under: None,
            },
            bindings: bindings.into(),
        })
    }

    fn price(amount: i64) -> Option<RowBinding> {
        Some(RowBinding::Bound(OperationResult::number(
            rust_decimal::Decimal::from(amount),
        )))
    }

    fn deferred(rule: &str) -> Option<RowBinding> {
        Some(RowBinding::Deferred {
            target: ReferenceTarget::Rule(RulePath::new(Vec::new(), rule.to_string())),
            hops: 0,
        })
    }

    #[test]
    fn binding_shapes_take_one_row_per_shape_in_row_order() {
        let nested = shape_row(vec![price(1)]);
        let rows = vec![
            shape_row(vec![price(10), None]),
            shape_row(vec![price(20), None]),
            shape_row(vec![price(30), deferred("surcharge")]),
            shape_row(vec![price(40), deferred("surcharge")]),
            shape_row(vec![price(50), deferred("discount")]),
            shape_row(vec![None, None]),
            shape_row(vec![
                price(60),
                Some(RowBinding::Instance(Arc::clone(&nested))),
            ]),
            shape_row(vec![
                price(70),
                Some(RowBinding::Instance(Arc::clone(&nested))),
            ]),
            shape_row(vec![
                price(80),
                Some(RowBinding::Instance(shape_row(vec![price(1)]))),
            ]),
        ];
        assert_eq!(binding_shapes(&rows), vec![0, 2, 4, 5, 6, 8]);
    }

    fn key_matches(row_value: &TypedLiteral, value: &TypedLiteral) -> OperationResult {
        match &row_value.value {
            ValueKind::Range(lower, upper) => {
                let endpoint_type = row_value
                    .lemma_type
                    .specifications
                    .element_from_range()
                    .map(|element| Arc::new(LemmaType::primitive(element)))
                    .expect("BUG: range key field has a range type");
                check_containment(
                    &value.value,
                    &value.lemma_type,
                    &lower.value,
                    &upper.value,
                    &endpoint_type,
                )
            }
            _ => comparison_operation(
                &row_value.value,
                &row_value.lemma_type,
                &ComparisonComputation::Is,
                &value.value,
                &value.lemma_type,
            ),
        }
    }

    fn keys_match(key: &[TypedLiteral], wanted: &[TypedLiteral]) -> Result<bool, OperationResult> {
        for (row_value, value) in key.iter().zip(wanted) {
            match key_matches(row_value, value) {
                OperationResult::Value(flag) => match flag.value {
                    ValueKind::Boolean(true) => {}
                    ValueKind::Boolean(false) => return Ok(false),
                    other => panic!("BUG: key match returned {other:?}"),
                },
                veto @ OperationResult::Veto(_) => return Err(veto),
            }
        }
        Ok(true)
    }

    #[test]
    fn key_index_probe_agrees_with_comparison_semantics() {
        let num = |n: i64| TypedLiteral {
            value: ValueKind::Number(rational_new(n, 1)),
            lemma_type: Arc::clone(primitive_number_arc()),
        };
        let ratio = |n: i64, d: i64| TypedLiteral {
            value: ValueKind::Ratio(rational_new(n, d)),
            lemma_type: Arc::clone(primitive_ratio_arc()),
        };
        let text = |s: &str| TypedLiteral {
            value: ValueKind::Text(Arc::from(s)),
            lemma_type: Arc::clone(primitive_text_arc()),
        };
        let boolean = |b: bool| TypedLiteral {
            value: ValueKind::Boolean(b),
            lemma_type: Arc::clone(primitive_boolean_arc()),
        };
        let date = |y: i32, m: u32, d: u32, h: u32, offset_min: i16| TypedLiteral {
            value: ValueKind::Date(SemanticDateTime {
                year: y,
                month: m,
                day: d,
                hour: h,
                minute: 0,
                second: 0,
                microsecond: 0,
                timezone: Some(SemanticTimezone {
                    offset_minutes: offset_min,
                }),
            }),
            lemma_type: Arc::clone(primitive_date_arc()),
        };
        let time = |h: u32, m: u32| TypedLiteral {
            value: ValueKind::Time(SemanticTime {
                hour: h,
                minute: m,
                second: 0,
                microsecond: 0,
                timezone: None,
            }),
            lemma_type: Arc::clone(primitive_time_arc()),
        };
        let range = |lo: TypedLiteral, hi: TypedLiteral, range_ty: &Arc<LemmaType>| TypedLiteral {
            value: ValueKind::Range(Box::new(lo), Box::new(hi)),
            lemma_type: Arc::clone(range_ty),
        };

        // Exact scalar stores across every ValueKind
        let exact_stores: Vec<(Keys, Keys)> = vec![
            (
                vec![vec![num(1)], vec![num(5)], vec![num(10)]],
                vec![
                    vec![num(0)],
                    vec![num(1)],
                    vec![num(5)],
                    vec![num(7)],
                    vec![num(10)],
                ],
            ),
            (
                vec![vec![ratio(1, 4)], vec![ratio(1, 2)]],
                vec![vec![ratio(1, 4)], vec![ratio(1, 3)], vec![ratio(1, 2)]],
            ),
            (
                vec![vec![text("espresso")], vec![text("latte")]],
                vec![
                    vec![text("espresso")],
                    vec![text("latte")],
                    vec![text("mocha")],
                ],
            ),
            (
                vec![vec![boolean(false)], vec![boolean(true)]],
                vec![vec![boolean(false)], vec![boolean(true)]],
            ),
            (
                vec![vec![date(2025, 1, 1, 12, 0)], vec![date(2025, 6, 1, 12, 0)]],
                vec![
                    vec![date(2025, 1, 1, 13, 60)], // equal UTC instant (+01:00)
                    vec![date(2025, 3, 1, 12, 0)],
                    vec![date(2025, 6, 1, 12, 0)],
                ],
            ),
            (
                vec![vec![time(8, 0)], vec![time(17, 30)]],
                vec![vec![time(8, 0)], vec![time(12, 0)], vec![time(17, 30)]],
            ),
        ];

        for (rows, probes) in exact_stores {
            let index = KeyIndex::build(vec!["k".to_string()], rows.len(), |row, field| {
                &rows[row][field].value
            })
            .expect("valid index");
            for wanted in &probes {
                let expected = rows
                    .iter()
                    .position(|r| keys_match(r, wanted).expect("compare"));
                let actual = index.probe(wanted).expect("probe");
                assert_eq!(actual, expected, "mismatch for probe {wanted:?}");
            }
        }

        // Range stores (number range, ratio range, date range, time range with date probe, composite exact + 2 ranges)
        let range_stores: Vec<(Vec<String>, Keys, Keys)> = vec![
            (
                vec!["band".to_string()],
                vec![
                    vec![range(
                        num(0),
                        num(10),
                        lemma_type_for_primitive(PrimitiveKind::NumberRange),
                    )],
                    vec![range(
                        num(20),
                        num(10),
                        lemma_type_for_primitive(PrimitiveKind::NumberRange),
                    )], // reversed endpoints
                ],
                vec![
                    vec![num(-1)],
                    vec![num(0)],
                    vec![num(5)],
                    vec![num(10)],
                    vec![num(15)],
                    vec![num(20)],
                ],
            ),
            (
                vec!["band".to_string()],
                vec![
                    vec![range(
                        ratio(0, 1),
                        ratio(1, 2),
                        lemma_type_for_primitive(PrimitiveKind::RatioRange),
                    )],
                    vec![range(
                        ratio(1, 2),
                        ratio(1, 1),
                        lemma_type_for_primitive(PrimitiveKind::RatioRange),
                    )],
                ],
                vec![vec![ratio(1, 4)], vec![ratio(1, 2)], vec![ratio(1, 1)]],
            ),
            (
                vec!["window".to_string()],
                vec![vec![range(
                    date(2025, 1, 1, 0, 0),
                    date(2025, 2, 1, 0, 0),
                    primitive_date_range_arc(),
                )]],
                vec![vec![date(2025, 1, 15, 0, 0)], vec![date(2025, 2, 1, 0, 0)]],
            ),
            (
                vec!["hours".to_string()],
                vec![vec![range(
                    time(9, 0),
                    time(17, 0),
                    lemma_type_for_primitive(PrimitiveKind::TimeRange),
                )]],
                vec![
                    vec![time(10, 0)],
                    vec![time(17, 0)],
                    vec![date(2025, 5, 1, 12, 0)], // date projected as time
                    vec![date(2025, 5, 1, 20, 0)],
                ],
            ),
            (
                vec![
                    "zone".to_string(),
                    "weight".to_string(),
                    "volume".to_string(),
                ],
                vec![
                    vec![
                        text("EU"),
                        range(
                            num(0),
                            num(5),
                            lemma_type_for_primitive(PrimitiveKind::NumberRange),
                        ),
                        range(
                            num(0),
                            num(2),
                            lemma_type_for_primitive(PrimitiveKind::NumberRange),
                        ),
                    ],
                    vec![
                        text("EU"),
                        range(
                            num(0),
                            num(5),
                            lemma_type_for_primitive(PrimitiveKind::NumberRange),
                        ),
                        range(
                            num(2),
                            num(10),
                            lemma_type_for_primitive(PrimitiveKind::NumberRange),
                        ),
                    ],
                    vec![
                        text("US"),
                        range(
                            num(0),
                            num(5),
                            lemma_type_for_primitive(PrimitiveKind::NumberRange),
                        ),
                        range(
                            num(0),
                            num(10),
                            lemma_type_for_primitive(PrimitiveKind::NumberRange),
                        ),
                    ],
                ],
                vec![
                    vec![text("EU"), num(3), num(1)],
                    vec![text("EU"), num(3), num(5)],
                    vec![text("EU"), num(3), num(10)],
                    vec![text("US"), num(3), num(5)],
                    vec![text("UK"), num(3), num(1)],
                ],
            ),
        ];

        for (fields, rows, probes) in range_stores {
            let index = KeyIndex::build(fields, rows.len(), |row, field| &rows[row][field].value)
                .expect("valid range index");
            for wanted in &probes {
                let expected = rows
                    .iter()
                    .position(|r| keys_match(r, wanted).expect("compare"));
                let actual = index.probe(wanted).expect("probe");
                assert_eq!(actual, expected, "mismatch for range probe {wanted:?}");
            }
        }
    }
}

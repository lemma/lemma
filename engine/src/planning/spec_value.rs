//! Spec-valued expressions: the instances an expression can yield, and field reads on them.
//!
//! A `uses` alias used as a value is one instance of the plan. A rule that returns that
//! alias (or another spec-valued rule) yields the union of those instances. A data list
//! yields rows of its row spec; a list command passes those on, and `-> with kim` adds
//! `kim`. `rule.field` and a row field inside a list command read `field` on whichever
//! row the base can yield, through that row spec's template plan.

use crate::parsing::arena::{ExprId, ExprKind};
use crate::parsing::source::Source;
use crate::planning::execution_plan::ExecutionPlan;
use crate::planning::instance_list::{self, InstanceListSource, RowEndView, RowHop};
use crate::planning::semantics::{
    DataDefinition, DataPath, Expression, ExpressionKind, ListOp, RowEnd, RulePath, SpecIdentity,
};
use indexmap::IndexMap;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

/// Whether a rule node is in the graph, was dropped during conversion, or was never defined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RulePresence {
    Present,
    Dropped,
    Absent,
}

/// What a spec-valued expression can yield.
///
/// `instances` are `uses` instances of the plan, each named by its alias hops from the
/// plan (`kim`, `i.j`). `rows` are the row specs whose rows the expression can yield,
/// instances included. A row appended inline (`-> with field: value`) is no instance;
/// `inline_rows` records, per such row, the fields it writes and what each field's
/// expression yields, so a read of a field it does not write is rejected.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SpecCandidates {
    pub instances: Vec<Vec<String>>,
    pub inline_rows: Vec<InlineRow>,
    pub rows: Vec<SpecIdentity>,
}

/// A row of `identity` written inline (`-> with field: value`): the fields it writes,
/// row-relative, with what each yields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InlineRow {
    pub identity: SpecIdentity,
    pub fields: Vec<(DataPath, SpecCandidates)>,
}

impl InlineRow {
    fn writes(&self, path: &DataPath) -> bool {
        self.fields.iter().any(|(field, _)| field == path)
    }
}

/// A data slot of a row spec that a read of an end on its rows touches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RowFieldRead {
    pub slot: DataPath,
    /// The spec gives the slot no value: a row must supply it.
    pub unbound: bool,
}

impl SpecCandidates {
    fn push_instance(&mut self, instance: Vec<String>) {
        if !self.instances.contains(&instance) {
            self.instances.push(instance);
        }
    }

    fn push_row_spec(&mut self, identity: SpecIdentity) {
        if !self.rows.contains(&identity) {
            self.rows.push(identity);
        }
    }

    pub(crate) fn extend(&mut self, other: &SpecCandidates) {
        for instance in &other.instances {
            self.push_instance(instance.clone());
        }
        self.inline_rows.extend(other.inline_rows.iter().cloned());
        for identity in &other.rows {
            self.push_row_spec(identity.clone());
        }
    }

    /// These candidates as the plan holding instance `at` sees them: instances of the
    /// row's plan are named under `at`. Without `at` (a row with no name in the plan),
    /// only the row specs stay.
    fn under(&self, at: Option<&[String]>) -> SpecCandidates {
        let mut out = SpecCandidates::default();
        if let Some(at) = at {
            for instance in &self.instances {
                out.push_instance(at.iter().chain(instance).cloned().collect());
            }
        }
        for identity in &self.rows {
            out.push_row_spec(identity.clone());
        }
        out
    }

    /// What field `path` of the inline rows yields.
    fn inline_field(&self, path: &DataPath) -> SpecCandidates {
        let mut out = SpecCandidates::default();
        for row in &self.inline_rows {
            for (field, candidates) in &row.fields {
                if field == path {
                    out.extend(candidates);
                }
            }
        }
        out
    }
}

/// What candidate resolution reads: linked rules so far, the plan's data and rules, the
/// rows written under each data list, rule presence, the row fields a read of `end` on a
/// row of `identity` touches ([`InlineRow`] checks), and the template plan of each row
/// spec other than the spec being planned (`self_identity`).
pub(crate) struct CandidateScope<'s, P, U> {
    pub rule_candidates: &'s HashMap<RulePath, SpecCandidates>,
    pub data: &'s IndexMap<DataPath, DataDefinition>,
    pub rules: &'s BTreeSet<RulePath>,
    pub sources: &'s IndexMap<DataPath, InstanceListSource>,
    pub rule_presence: &'s P,
    pub row_end_reads: &'s U,
    pub self_identity: &'s SpecIdentity,
    pub templates: &'s BTreeMap<SpecIdentity, Arc<ExecutionPlan>>,
}

/// Bounds of [`CandidateScope`]'s closures.
pub(crate) trait Presence: Fn(&RulePath) -> RulePresence {}
impl<P: Fn(&RulePath) -> RulePresence> Presence for P {}
pub(crate) trait RowReads: Fn(&SpecIdentity, &RowEnd) -> Vec<RowFieldRead> {}
impl<U: Fn(&SpecIdentity, &RowEnd) -> Vec<RowFieldRead>> RowReads for U {}

impl<P: Presence, U: RowReads> CandidateScope<'_, P, U> {
    fn view(&self, identity: &SpecIdentity) -> RowEndView<'_> {
        if identity == self.self_identity {
            return RowEndView::Root {
                data: self.data,
                rules: self.rules,
            };
        }
        RowEndView::Plan(self.template(identity))
    }

    fn template(&self, identity: &SpecIdentity) -> &ExecutionPlan {
        self.templates.get(identity).unwrap_or_else(|| {
            panic!("BUG: rows of '{identity}' are read but it has no template plan")
        })
    }

    /// Member `parts` on rows of `identity`, hop by hop.
    fn chain(&self, identity: &SpecIdentity, parts: &[&str]) -> Option<Vec<RowHop>> {
        instance_list::row_end_chain(identity, self.view(identity), self.templates, parts)
    }

    /// Spec of the instance `instance` names.
    fn instance_identity(&self, instance: &[String]) -> SpecIdentity {
        let (alias, hops) = instance
            .split_first()
            .expect("BUG: an instance has an alias");
        let mut identity = match self.data.get(&DataPath::local(alias.clone())) {
            Some(DataDefinition::Import { row, .. }) => row.identity.as_ref().clone(),
            other => panic!("BUG: instance `{alias}` is not a `uses` of the plan: {other:?}"),
        };
        for hop in hops {
            identity = match self
                .template(&identity)
                .data
                .get(&DataPath::local(hop.clone()))
            {
                Some(DataDefinition::Import { row, .. }) => row.identity.as_ref().clone(),
                other => panic!("BUG: instance hop `{hop}` is not a `uses`: {other:?}"),
            };
        }
        identity
    }
}

/// Candidate instances of every node of one rule expression.
pub(crate) struct CandidateTable {
    /// Per node: the list operand of each in-row scope enclosing it, outermost first.
    /// [`ExpressionKind::RowField::level`] indexes it.
    levels: Vec<Vec<ExprId>>,
    candidates: Vec<SpecCandidates>,
}

impl CandidateTable {
    /// Candidates of every node, children before parents (table order).
    pub(crate) fn build<P: Presence, U: RowReads>(
        expression: &Expression,
        scope: &CandidateScope<'_, P, U>,
    ) -> Self {
        let size = expression.table.len();
        let mut levels: Vec<Vec<ExprId>> = vec![Vec::new(); size];
        let mut pending: Vec<(ExprId, Vec<ExprId>)> = vec![(expression.root().id, Vec::new())];
        while let Some((id, enclosing)) = pending.pop() {
            match expression.table.kind(id) {
                ExpressionKind::ListCommand {
                    list,
                    op,
                    row_level,
                } => {
                    pending.push((*list, enclosing.clone()));
                    let mut in_rows = enclosing.clone();
                    if row_level.is_some() {
                        in_rows.push(*list);
                    }
                    for argument in in_row_arguments(op) {
                        pending.push((argument, in_rows.clone()));
                    }
                    for argument in enclosing_arguments(op) {
                        pending.push((argument, enclosing.clone()));
                    }
                }
                kind => kind.for_each_child(|child| pending.push((child, enclosing.clone()))),
            }
            levels[id.index()] = enclosing;
        }

        let mut candidates: Vec<SpecCandidates> = vec![SpecCandidates::default(); size];
        for id in expression.table.ids() {
            let own = match expression.table.kind(id) {
                ExpressionKind::RulePath(rule) => linked_candidates(scope, rule),
                ExpressionKind::DataPath(path) => data_candidates(scope, path),
                ExpressionKind::SpecMember {
                    base, path, name, ..
                } => {
                    let parts: Vec<&str> = path
                        .iter()
                        .map(String::as_str)
                        .chain(std::iter::once(name.as_str()))
                        .collect();
                    let base = &candidates[base.index()];
                    let mut out = SpecCandidates::default();
                    for instance in &base.instances {
                        let identity = scope.instance_identity(instance);
                        let Some(chain) = scope.chain(&identity, &parts) else {
                            continue;
                        };
                        let at: Vec<String> = instance
                            .iter()
                            .cloned()
                            .chain(path.iter().cloned())
                            .collect();
                        out.extend(&hop_candidates(scope, &chain).under(Some(&at)));
                    }
                    for identity in &base.rows {
                        if let Some(chain) = scope.chain(identity, &parts) {
                            out.extend(&hop_candidates(scope, &chain).under(None));
                        }
                    }
                    for row in &base.inline_rows {
                        let Some(chain) = scope.chain(&row.identity, &parts) else {
                            continue;
                        };
                        match chain.as_slice() {
                            [RowHop {
                                end: RowEnd::Data(field),
                                ..
                            }] if row.writes(field) => out.extend(&base.inline_field(field)),
                            _ => out.extend(&hop_candidates(scope, &chain).under(None)),
                        }
                    }
                    out
                }
                ExpressionKind::RowField {
                    level,
                    end,
                    identity,
                    ..
                } => {
                    let operand = levels[id.index()].get(*level as usize).unwrap_or_else(|| {
                        panic!(
                            "BUG: row field at level {level} has {} enclosing row scopes",
                            levels[id.index()].len()
                        )
                    });
                    let rows = &candidates[operand.index()];
                    let own = row_end_candidates(scope, identity, end);
                    let mut out = own.under(None);
                    for instance in &rows.instances {
                        out.extend(&own.under(Some(instance)));
                    }
                    if let RowEnd::Data(path) = end {
                        out.extend(&rows.inline_field(path));
                    }
                    out
                }
                ExpressionKind::ListCommand { list, op, .. } => match op {
                    ListOp::Find { .. }
                    | ListOp::First
                    | ListOp::Last
                    | ListOp::Sort { .. }
                    | ListOp::Top { .. }
                    | ListOp::Key { .. } => candidates[list.index()].clone(),
                    ListOp::WithRow { identity, fields } => {
                        let mut out = candidates[list.index()].clone();
                        out.inline_rows.push(InlineRow {
                            identity: identity.clone(),
                            fields: fields
                                .iter()
                                .map(|(path, value)| {
                                    (path.clone(), candidates[value.index()].clone())
                                })
                                .collect(),
                        });
                        out
                    }
                    ListOp::Predicate { kind, .. } => match kind {
                        crate::planning::semantics::ListPredKind::Where => {
                            candidates[list.index()].clone()
                        }
                        crate::planning::semantics::ListPredKind::Any
                        | crate::planning::semantics::ListPredKind::All => {
                            SpecCandidates::default()
                        }
                    },
                    ListOp::With { rows } => {
                        let mut out = candidates[list.index()].clone();
                        for row in rows {
                            out.extend(&candidates[row.index()]);
                        }
                        out
                    }
                    ListOp::Count | ListOp::Reduce { .. } => SpecCandidates::default(),
                },
                ExpressionKind::Piecewise(arms) => {
                    let mut out = SpecCandidates::default();
                    for (_, result) in arms {
                        out.extend(&candidates[result.index()]);
                    }
                    out
                }
                _ => SpecCandidates::default(),
            };
            candidates[id.index()] = own;
        }
        Self { levels, candidates }
    }

    pub(crate) fn candidates(&self, id: ExprId) -> &SpecCandidates {
        &self.candidates[id.index()]
    }

    /// Rows in view of the row field at `id` (level `level`).
    fn row_candidates(&self, id: ExprId, level: u32) -> &SpecCandidates {
        let operand = self.levels[id.index()]
            .get(level as usize)
            .expect("BUG: row field level checked while building the table");
        &self.candidates[operand.index()]
    }
}

/// Arguments of `op` converted with the rows of the list in view.
fn in_row_arguments(op: &ListOp) -> Vec<ExprId> {
    match op {
        ListOp::Reduce {
            argument: Some(argument),
            ..
        }
        | ListOp::Sort {
            argument: Some(argument),
            ..
        } => vec![*argument],
        ListOp::Predicate { expr, .. } => vec![*expr],
        ListOp::Reduce { argument: None, .. }
        | ListOp::Sort { argument: None, .. }
        | ListOp::Find { .. }
        | ListOp::First
        | ListOp::Last
        | ListOp::Count
        | ListOp::Top { .. }
        | ListOp::With { .. }
        | ListOp::WithRow { .. }
        | ListOp::Key { .. } => Vec::new(),
    }
}

/// Arguments of `op` converted in the scopes enclosing the command.
fn enclosing_arguments(op: &ListOp) -> Vec<ExprId> {
    match op {
        ListOp::Find { keys } => keys.clone(),
        ListOp::Top { n } => vec![*n],
        ListOp::With { rows } => rows.clone(),
        ListOp::WithRow { fields, .. } => fields.iter().map(|(_, value)| *value).collect(),
        ListOp::Reduce { .. }
        | ListOp::Sort { .. }
        | ListOp::Predicate { .. }
        | ListOp::First
        | ListOp::Last
        | ListOp::Count
        | ListOp::Key { .. } => Vec::new(),
    }
}

/// Instances a linked rule yields. A present rule that is not linked yet yields none for
/// now: the caller adds the dependency edge and linking repeats in the new order.
fn linked_candidates<P: Presence, U: RowReads>(
    scope: &CandidateScope<'_, P, U>,
    rule: &RulePath,
) -> SpecCandidates {
    if let Some(candidates) = scope.rule_candidates.get(rule) {
        return candidates.clone();
    }
    match (scope.rule_presence)(rule) {
        RulePresence::Dropped | RulePresence::Present => SpecCandidates::default(),
        RulePresence::Absent => panic!("BUG: rule '{rule}' neither in graph nor dropped"),
    }
}

/// What data `path` of the plan yields: a `uses` instance, or the rows of a data list.
fn data_candidates<P: Presence, U: RowReads>(
    scope: &CandidateScope<'_, P, U>,
    path: &DataPath,
) -> SpecCandidates {
    slot_candidates(scope.data, scope.sources, path)
}

/// What data slot `path` of a plan with `data` and list `sources` yields.
fn slot_candidates(
    data: &IndexMap<DataPath, DataDefinition>,
    sources: &IndexMap<DataPath, InstanceListSource>,
    path: &DataPath,
) -> SpecCandidates {
    match data.get(path) {
        Some(DataDefinition::Import { row, .. }) => SpecCandidates {
            instances: vec![vec![path.data.clone()]],
            rows: vec![row.identity.as_ref().clone()],
            ..SpecCandidates::default()
        },
        Some(DataDefinition::InstanceList { .. }) => {
            let source = sources
                .get(path)
                .unwrap_or_else(|| panic!("BUG: instance list '{path}' has no source"));
            SpecCandidates {
                rows: vec![source.identity.clone()],
                ..SpecCandidates::default()
            }
        }
        Some(
            DataDefinition::Value { .. }
            | DataDefinition::TypeDeclaration { .. }
            | DataDefinition::Reference { .. },
        )
        | None => SpecCandidates::default(),
    }
}

/// What the last hop of `chain` yields, its instances named in that hop's row plan.
fn hop_candidates<P: Presence, U: RowReads>(
    scope: &CandidateScope<'_, P, U>,
    chain: &[RowHop],
) -> SpecCandidates {
    let last = chain.last().expect("BUG: a row end chain has a last hop");
    row_end_candidates(scope, &last.identity, &last.end)
}

/// What a read of `end` on a row of `identity` yields, its instances named in that
/// row's plan.
fn row_end_candidates<P: Presence, U: RowReads>(
    scope: &CandidateScope<'_, P, U>,
    identity: &SpecIdentity,
    end: &RowEnd,
) -> SpecCandidates {
    if identity == scope.self_identity {
        return match end {
            RowEnd::Data(path) => data_candidates(scope, path),
            RowEnd::Rule(rule) => linked_candidates(scope, rule),
        };
    }
    let template = scope.template(identity);
    match end {
        RowEnd::Rule(rule) => template
            .rule_candidates
            .get(rule)
            .cloned()
            .unwrap_or_else(|| {
                panic!("BUG: rule '{rule}' of row spec '{identity}' has no candidate list")
            }),
        RowEnd::Data(path) => slot_candidates(&template.data, &template.instance_lists, path),
    }
}

/// Reads of `end` on inline row `row` that planning rejects. A field the row writes
/// is read as the command's plan planned it; everything else runs in the row's
/// context over the row spec's plan, so the fields that run reads must all be
/// written, or given a value by the row spec.
fn inline_row_errors<P: Presence, U: RowReads>(
    scope: &CandidateScope<'_, P, U>,
    row: &InlineRow,
    end: &RowEnd,
    source: Option<&Source>,
    errors: &mut Vec<(Option<Source>, String)>,
) {
    if let RowEnd::Data(field) = end {
        if row.writes(field) {
            return;
        }
    }
    for read in (scope.row_end_reads)(&row.identity, end) {
        if read.unbound && !row.writes(&read.slot) {
            errors.push((
                source.cloned(),
                format!(
                    "`{end}` reads `{}`, which a row appended with `-> with` does not set; add `{}: ...` to that row",
                    read.slot.input_key(),
                    read.slot.input_key()
                ),
            ));
        }
    }
}

/// After linking stops adding edges, every rule an expression names has a candidate list.
/// An empty list means the rule is not spec-valued. A missing list is a broken edge.
pub(crate) fn assert_expression_candidates_linked<P: Presence, U: RowReads>(
    expression: &Expression,
    scope: &CandidateScope<'_, P, U>,
) {
    for id in expression.table.ids() {
        if let ExpressionKind::RulePath(rule) = expression.table.kind(id) {
            if scope.rule_candidates.contains_key(rule) {
                continue;
            }
            match (scope.rule_presence)(rule) {
                RulePresence::Dropped => {}
                RulePresence::Present => panic!(
                    "BUG: rule '{rule}' is referenced but has no candidate list after linking"
                ),
                RulePresence::Absent => panic!("BUG: rule '{rule}' neither in graph nor dropped"),
            }
        }
    }
}

/// Errors of the [`ExpressionKind::SpecMember`] and [`ExpressionKind::RowField`] reads of
/// `expression`: a member an instance's spec does not have, and a read on a row
/// appended inline of a field that row does not set.
pub(crate) fn spec_member_errors<P: Presence, U: RowReads>(
    expression: &Expression,
    table: &CandidateTable,
    scope: &CandidateScope<'_, P, U>,
) -> Vec<(Option<Source>, String)> {
    let mut errors = Vec::new();
    for id in expression.table.ids() {
        let node = expression.at(id);
        let source = node.source();
        match node.kind() {
            ExpressionKind::SpecMember {
                base, path, name, ..
            } => {
                let parts: Vec<&str> = path
                    .iter()
                    .map(String::as_str)
                    .chain(std::iter::once(name.as_str()))
                    .collect();
                let base = table.candidates(*base);
                for instance in &base.instances {
                    let identity = scope.instance_identity(instance);
                    if scope.chain(&identity, &parts).is_none() {
                        errors.push((
                            source.cloned(),
                            format!("no data or rule '{name}' on spec '{}'", identity.spec),
                        ));
                    }
                }
                for row in &base.inline_rows {
                    match scope.chain(&row.identity, &parts) {
                        Some(chain) => {
                            let end = &chain.first().expect("BUG: a chain has a hop").end;
                            inline_row_errors(scope, row, end, source, &mut errors);
                        }
                        None => errors.push((
                            source.cloned(),
                            format!("no data or rule '{name}' on spec '{}'", row.identity.spec),
                        )),
                    }
                }
            }
            ExpressionKind::RowField {
                level,
                end,
                identity,
                ..
            } => {
                for row in &table.row_candidates(id, *level).inline_rows {
                    assert_eq!(
                        row.identity, *identity,
                        "BUG: inline row of '{}' iterated by a command over '{identity}' rows",
                        row.identity
                    );
                    inline_row_errors(scope, row, end, source, &mut errors);
                }
            }
            _ => {}
        }
    }
    errors
}

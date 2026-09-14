//! Planning module for Lemma specs
//!
//! This module performs complete static analysis and builds execution plans:
//! - Builds Graph with data and rules (validated, with types computed)
//! - Builds ExecutionPlan from Graph (topo order for cycles, typing, and dep-before-consumer)
//! - Validates spec structure and references
//!
//! Contract model:
//! - Interface contract: data (inputs) + rules (outputs), including full type constraints.
//!   Cross-spec bindings must satisfy this contract at planning time.

pub mod discovery;
pub mod execution_plan;
pub mod explanation;
pub mod graph;
pub mod normalize;
pub mod ordered_dispatch;
pub mod semantics;
pub mod show_expression;
pub mod spec_set;
pub mod typing;
pub mod unit_family;
pub mod unit_index;
use crate::engine::Context;
use crate::parsing::ast::{DateTimeValue, EffectiveDate, LemmaRepository};
use crate::Error;
pub use execution_plan::ExecutionPlan;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
pub use spec_set::LemmaSpecSet;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::unreachable;

/// Canonical identity of one temporal spec set: `(repository, spec name)`.
///
/// `repository` is `None` for the default unnamed repository. Both components are canonicalized
/// exactly as [`PlanStore`] canonicalizes its own keys, so a key built here always
/// addresses the same entry the plans were stored under.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct SpecSetKey {
    repository: Option<String>,
    spec: String,
}

impl SpecSetKey {
    pub(crate) fn new(repository: Option<&str>, spec: &str) -> Self {
        Self {
            repository: PlanStore::repo_key(repository),
            spec: PlanStore::spec_key(spec),
        }
    }
}

impl std::fmt::Display for SpecSetKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.repository {
            Some(repository) => write!(formatter, "{repository} {}", self.spec),
            None => write!(formatter, "{}", self.spec),
        }
    }
}

/// One spec set a planning pass rebuilds, with the identity needed to address it in
/// both the [`Context`] and a [`PlanStore`].
///
/// `repository` and `spec` are the identity as the context spells it, or as the caller
/// spelled it for a spec set that this batch removed from the context.
///
/// `dirty_slices`: `None` means every temporal slice must be rebuilt; `Some` lists
/// only the `effective_from` values whose bodies changed (structural add/remove of
/// versions and reverse-edge consumers are always whole-set).
pub(crate) struct ScopeMember {
    key: SpecSetKey,
    repository: Arc<LemmaRepository>,
    spec: String,
    dirty_slices: Option<HashSet<EffectiveDate>>,
}

/// The spec sets a planning pass rebuilds.
///
/// A spec set's execution plans are a function of its own spec versions plus the
/// spec versions and temporal structure of every spec set reachable through its
/// dependency edges (`uses` imports and qualified parent types, as extracted by
/// [`discovery::dependency_edges`]). Mutating one spec set therefore invalidates
/// that set and every set that transitively depends on it, and nothing else:
/// plans outside the scope remain valid and are kept as they are.
///
/// Reverse edges are derived from the context on every pass rather than cached
/// across passes. Edge resolution depends on global context state (`uses` aliases,
/// repository qualifiers), so a cached reverse index could resolve differently than
/// the forward pass and silently under-report consumers, leaving stale plans behind.
/// Deriving the edges costs one cheap walk of the context per pass and cannot go
/// stale.
pub(crate) struct ReplanScope {
    /// Sorted by [`SpecSetKey`], so a pass visits its members in an order determined
    /// only by which spec sets are in scope, never by the order specs were loaded.
    members: Vec<ScopeMember>,
    keys: HashSet<SpecSetKey>,
}

impl ReplanScope {
    /// Every listed set is fully dirty (first load / structural identity change).
    #[cfg(test)]
    pub(crate) fn from_changed_sets(
        context: &Context,
        changed: Vec<(Arc<LemmaRepository>, String)>,
    ) -> Self {
        let whole_set: HashSet<SpecSetKey> = changed
            .iter()
            .map(|(repository, spec)| SpecSetKey::new(repository.name.as_deref(), spec))
            .collect();
        let changed = changed
            .into_iter()
            .map(|(repository, spec)| (repository, spec, EffectiveDate::Origin))
            .collect();
        Self::from_changed(context, changed, whole_set)
    }

    /// `changed` plus every spec set that transitively depends on a changed one.
    ///
    /// Keys in `whole_set` rebuild every temporal slice. Other seed keys rebuild only
    /// the `EffectiveDate`s listed in `changed`. Consumers reached via reverse edges
    /// are always whole-set.
    ///
    /// A changed spec set need not be in `context`: this batch may have removed it.
    /// Pinned edges (`uses dep 2025-06-01`) propagate dirtiness like any other, since
    /// a pin fixes the resolution instant, not the pinned source text.
    pub(crate) fn from_changed(
        context: &Context,
        changed: Vec<(Arc<LemmaRepository>, String, EffectiveDate)>,
        whole_set: HashSet<SpecSetKey>,
    ) -> Self {
        let mut identities: HashMap<SpecSetKey, (Arc<LemmaRepository>, String)> = HashMap::new();
        let mut consumers_of: HashMap<SpecSetKey, Vec<SpecSetKey>> = HashMap::new();
        let mut worklist: Vec<SpecSetKey> = Vec::with_capacity(changed.len());
        let mut seed_slices: HashMap<SpecSetKey, HashSet<EffectiveDate>> = HashMap::new();

        for (repository, by_name) in context.repositories() {
            for (spec_name, spec_set) in by_name {
                let consumer = SpecSetKey::new(repository.name.as_deref(), spec_name);
                identities.insert(
                    consumer.clone(),
                    (Arc::clone(repository), spec_name.clone()),
                );
                for spec in spec_set.iter_specs() {
                    match discovery::dependency_edges(spec, repository, context) {
                        Ok(edges) => {
                            for edge in edges {
                                consumers_of
                                    .entry(SpecSetKey::new(
                                        edge.dep_repository.name.as_deref(),
                                        &edge.dep_name,
                                    ))
                                    .or_default()
                                    .push(consumer.clone());
                            }
                        }
                        // This spec set's dependencies are unknown, so it cannot be shown
                        // to be unaffected. Replanning it surfaces the same edge errors
                        // through the normal planning path.
                        Err(_) => worklist.push(consumer.clone()),
                    }
                }
            }
        }

        for (repository, spec, effective) in changed {
            let key = SpecSetKey::new(repository.name.as_deref(), &spec);
            identities
                .entry(key.clone())
                .or_insert_with(|| (Arc::clone(&repository), spec.clone()));
            seed_slices
                .entry(key.clone())
                .or_default()
                .insert(effective);
            worklist.push(key);
        }

        let mut keys: HashSet<SpecSetKey> = HashSet::new();
        let mut consumer_keys: HashSet<SpecSetKey> = HashSet::new();
        while let Some(key) = worklist.pop() {
            if !keys.insert(key.clone()) {
                continue;
            }
            if let Some(consumers) = consumers_of.get(&key) {
                for consumer in consumers {
                    consumer_keys.insert(consumer.clone());
                    worklist.push(consumer.clone());
                }
            }
        }

        let mut members: Vec<ScopeMember> = keys
            .iter()
            .map(|key| {
                let (repository, spec) = identities.get(key).unwrap_or_else(|| {
                    panic!("BUG: spec set '{key}' entered the replan scope without an identity")
                });
                let dirty_slices = if whole_set.contains(key)
                    || consumer_keys.contains(key)
                    || !seed_slices.contains_key(key)
                {
                    None
                } else {
                    Some(seed_slices.get(key).cloned().expect(
                        "BUG: seed_slices.contains_key was true in the complementary branch",
                    ))
                };
                ScopeMember {
                    key: key.clone(),
                    repository: Arc::clone(repository),
                    spec: spec.clone(),
                    dirty_slices,
                }
            })
            .collect();
        members.sort_by(|left, right| left.key.cmp(&right.key));

        Self { members, keys }
    }

    pub(crate) fn contains(&self, repository: Option<&str>, spec: &str) -> bool {
        self.keys.contains(&SpecSetKey::new(repository, spec))
    }

    /// The spec sets this pass rebuilds, in canonical order.
    pub(crate) fn members(&self) -> impl Iterator<Item = &ScopeMember> + '_ {
        self.members.iter()
    }
}

/// Plans as they must be read while a pass is still in progress: newly built plans
/// for whole-set members inside the [`ReplanScope`], a merged view (committed slices
/// outside the dirty ranges plus replanned dirty slices) for slice-mode members, and
/// already committed plans for every spec set outside the scope.
///
/// A committed store always holds plans for every spec set of its context, because
/// [`crate::Engine`] commits a pass only when that pass reported no errors. A spec
/// set outside the scope with no committed plans is therefore a bug.
pub(crate) struct PlanView<'a> {
    replanned: &'a PlanStore,
    committed: &'a PlanStore,
    scope: &'a ReplanScope,
    /// Merged plans for slice-mode members: committed outside dirty ranges, then
    /// overlay of replanned dirty slices. Built once so [`Self::get_plans`] can
    /// return a stable reference for the duration of the pass.
    slice_merged: HashMap<SpecSetKey, BTreeMap<EffectiveDate, ExecutionPlan>>,
}

impl<'a> PlanView<'a> {
    pub(crate) fn new(
        replanned: &'a PlanStore,
        committed: &'a PlanStore,
        scope: &'a ReplanScope,
        context: &Context,
    ) -> Self {
        let mut slice_merged = HashMap::new();
        for member in scope.members() {
            let Some(dirty) = &member.dirty_slices else {
                continue;
            };
            let mut merged = committed
                .get_plans(member.key.repository.as_deref(), &member.key.spec)
                .cloned()
                .unwrap_or_default();
            if let Some(spec_set) = context.spec_set(&member.repository, &member.spec) {
                let dirty_ranges: Vec<(Option<DateTimeValue>, Option<DateTimeValue>)> = spec_set
                    .iter_with_ranges()
                    .filter(|(spec, _, _)| dirty.contains(&spec.effective_from))
                    .map(|(_, from, to)| (from, to))
                    .collect();
                merged.retain(|effective, _| {
                    !dirty_ranges.iter().any(|(from, to)| {
                        effective_in_version_range(effective, from.as_ref(), to.as_ref())
                    })
                });
            }
            if let Some(from_replan) =
                replanned.get_plans(member.key.repository.as_deref(), &member.key.spec)
            {
                for (effective, plan) in from_replan {
                    merged.insert(effective.clone(), plan.clone());
                }
            }
            slice_merged.insert(member.key.clone(), merged);
        }
        Self {
            replanned,
            committed,
            scope,
            slice_merged,
        }
    }

    /// Temporal slices for `(repository, spec)`, ordered by `effective`.
    ///
    /// `None` means a spec set inside the scope has no plans in this pass: its
    /// planning failed for every slice that still exists in context, or it is no
    /// longer in the context.
    pub(crate) fn get_plans(
        &self,
        repository: Option<&str>,
        spec: &str,
    ) -> Option<&BTreeMap<EffectiveDate, ExecutionPlan>> {
        let key = SpecSetKey::new(repository, spec);
        if let Some(merged) = self.slice_merged.get(&key) {
            if merged.is_empty() {
                return None;
            }
            return Some(merged);
        }
        if self.scope.contains(repository, spec) {
            return self.replanned.get_plans(repository, spec);
        }
        Some(
            self.committed
                .get_plans(repository, spec)
                .unwrap_or_else(|| {
                    panic!(
                        "BUG: spec set '{}' is outside the replan scope but has no committed plans",
                        key
                    )
                }),
        )
    }
}

/// Compiled plans keyed by repository name (`None` = default unnamed repository) then spec name.
///
/// Temporal slices per spec are a [`BTreeMap`] keyed by [`ExecutionPlan::effective`].
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct PlanStore {
    plans: IndexMap<Option<String>, IndexMap<String, BTreeMap<EffectiveDate, ExecutionPlan>>>,
}

impl PlanStore {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            plans: IndexMap::new(),
        }
    }

    fn repo_key(repository: Option<&str>) -> Option<String> {
        repository.map(|name| crate::parsing::ast::ascii_lowercase_logical_name(name.to_string()))
    }

    fn spec_key(spec: &str) -> String {
        crate::parsing::ast::ascii_lowercase_logical_name(spec.to_string())
    }

    /// Insert one temporal slice. Panics on duplicate [`ExecutionPlan::effective`]
    /// for the same `(repository, spec)`.
    pub(crate) fn insert_plan(
        &mut self,
        repository: Option<&str>,
        spec: &str,
        plan: ExecutionPlan,
    ) {
        let effective = plan.effective.clone();
        let occupied = self
            .plans
            .entry(Self::repo_key(repository))
            .or_default()
            .entry(Self::spec_key(spec))
            .or_default()
            .insert(effective.clone(), plan)
            .is_some();
        if occupied {
            panic!("BUG: duplicate plan effective {effective:?} for spec '{spec}'");
        }
    }

    /// Temporal slices for `(repository, spec)`, ordered by `effective`.
    #[must_use]
    pub(crate) fn get_plans(
        &self,
        repository: Option<&str>,
        spec: &str,
    ) -> Option<&BTreeMap<EffectiveDate, ExecutionPlan>> {
        self.plans
            .get(&Self::repo_key(repository))
            .and_then(|by_name| by_name.get(&Self::spec_key(spec)))
    }

    /// Covering plan at `effective_at`, or `None`.
    #[must_use]
    pub(crate) fn get_plan(
        &self,
        repository: Option<&str>,
        spec: &str,
        effective_at: &EffectiveDate,
    ) -> Option<&ExecutionPlan> {
        self.get_plans(repository, spec)
            .and_then(|plans| execution_plan::plan_at(plans, effective_at))
    }

    fn take_spec_set(
        &mut self,
        key: &SpecSetKey,
    ) -> Option<BTreeMap<EffectiveDate, ExecutionPlan>> {
        self.plans
            .get_mut(&key.repository)
            .and_then(|by_name| by_name.shift_remove(&key.spec))
    }

    fn put_spec_set(&mut self, key: &SpecSetKey, slices: BTreeMap<EffectiveDate, ExecutionPlan>) {
        self.plans
            .entry(key.repository.clone())
            .or_default()
            .insert(key.spec.clone(), slices);
    }

    fn remove_spec_set(&mut self, key: &SpecSetKey) {
        if let Some(by_name) = self.plans.get_mut(&key.repository) {
            by_name.shift_remove(&key.spec);
        }
    }

    fn holds_any_spec_set(&self) -> bool {
        self.plans.values().any(|by_name| !by_name.is_empty())
    }

    /// Install the result of one planning pass.
    ///
    /// Whole-set members (`dirty_slices == None`) take the plans `replanned` built for
    /// them, or lose plans when removed from `context`. Slice members merge dirty
    /// keys into the existing `BTreeMap`, drop keys absent from context, and leave
    /// other slices untouched. Spec sets outside `scope` keep their plans.
    ///
    /// Call only after the pass reported no errors.
    pub(crate) fn commit(&mut self, context: &Context, scope: &ReplanScope, mut replanned: Self) {
        for member in scope.members() {
            match &member.dirty_slices {
                None => match replanned.take_spec_set(&member.key) {
                    Some(slices) => self.put_spec_set(&member.key, slices),
                    None => {
                        assert!(
                            context
                                .spec_set(&member.repository, &member.spec)
                                .is_none(),
                            "BUG: spec set '{}' is in the context but the planning pass produced no plans for it",
                            member.key
                        );
                        self.remove_spec_set(&member.key);
                    }
                },
                Some(dirty) => {
                    let Some(spec_set) = context.spec_set(&member.repository, &member.spec) else {
                        self.remove_spec_set(&member.key);
                        let leftover = replanned.take_spec_set(&member.key);
                        assert!(
                            leftover.is_none(),
                            "BUG: slice-mode member '{}' left context but replanned still holds slices",
                            member.key
                        );
                        continue;
                    };

                    let dirty_ranges: Vec<(Option<DateTimeValue>, Option<DateTimeValue>)> =
                        spec_set
                            .iter_with_ranges()
                            .filter(|(spec, _, _)| dirty.contains(&spec.effective_from))
                            .map(|(_, from, to)| (from, to))
                            .collect();

                    let existing = self
                        .plans
                        .entry(member.key.repository.clone())
                        .or_default()
                        .entry(member.key.spec.clone())
                        .or_default();
                    existing.retain(|effective, _| {
                        !dirty_ranges.iter().any(|(from, to)| {
                            effective_in_version_range(effective, from.as_ref(), to.as_ref())
                        })
                    });

                    let from_replan = replanned.take_spec_set(&member.key).unwrap_or_else(|| {
                        panic!(
                            "BUG: slice-mode member '{}' produced no replanned slices",
                            member.key
                        )
                    });
                    for (effective, plan) in from_replan {
                        existing.insert(effective, plan);
                    }
                }
            }
        }
        assert!(
            !replanned.holds_any_spec_set(),
            "BUG: planning pass produced plans for spec sets outside the replan scope"
        );
    }
}

/// Whether plan key `effective` falls in version half-open range `[from, to)`.
fn effective_in_version_range(
    effective: &EffectiveDate,
    from: Option<&DateTimeValue>,
    to: Option<&DateTimeValue>,
) -> bool {
    let from_key = EffectiveDate::from_option(from.cloned());
    if effective < &from_key {
        return false;
    }
    match to {
        None => true,
        Some(end) => effective < &EffectiveDate::DateTimeValue(end.clone()),
    }
}

/// Two half-open ranges `[a_from, a_to)` and `[b_from, b_to)` overlap when
/// `a_from < b_to AND b_from < a_to` (with `None` representing +/-infinity).
pub(crate) fn ranges_overlap(
    a_from: &Option<DateTimeValue>,
    a_to: &Option<DateTimeValue>,
    b_from: &Option<DateTimeValue>,
    b_to: &Option<DateTimeValue>,
) -> bool {
    let a_before_b_end = match (a_from, b_to) {
        (_, None) => true,
        (None, Some(_)) => true,
        (Some(a), Some(b)) => a < b,
    };
    let b_before_a_end = match (b_from, a_to) {
        (_, None) => true,
        (None, Some(_)) => true,
        (Some(b), Some(a)) => b < a,
    };
    a_before_b_end && b_before_a_end
}

/// Result of one planning pass over a [`Context`].
pub(crate) struct PlanningResult {
    /// Plans for the spec sets in the pass's [`ReplanScope`], and only those.
    /// Install with [`PlanStore::commit`].
    pub plans: PlanStore,
    pub errors: Vec<Error>,
}

/// Build execution plans for every spec set of `context` that is in `scope`.
///
/// `committed` holds the plans of the previous successful pass; spec sets outside
/// `scope` are read from it during dependency interface validation instead of being
/// rebuilt. Does not mutate caller state.
pub(crate) fn plan(
    context: &Context,
    limits: &crate::limits::ResourceLimits,
    scope: &ReplanScope,
    committed: &PlanStore,
) -> PlanningResult {
    let mut plans = PlanStore::new();
    let mut errors = Vec::new();
    let mut failed_specs: HashSet<(Arc<LemmaRepository>, String, EffectiveDate)> = HashSet::new();
    let mut failed_plan_sets: HashSet<(Arc<LemmaRepository>, String)> = HashSet::new();
    let mut missing_repository_source_specs: HashSet<(
        Arc<LemmaRepository>,
        String,
        EffectiveDate,
    )> = HashSet::new();

    for member in scope.members() {
        let repository = &member.repository;
        // A member with no spec set left the context in this batch: it has no versions
        // to plan. `PlanStore::commit` asserts that this is the only reason a scope
        // member can end a pass without plans.
        if let Some(lemma_spec_set) = context.spec_set(repository, &member.spec) {
            let versions: std::sync::Arc<[execution_plan::ShowVersion]> = lemma_spec_set
                .iter_with_ranges()
                .map(|(_, from, to)| execution_plan::ShowVersion {
                    effective_from: from,
                    effective_to: to,
                })
                .collect::<Vec<_>>()
                .into();
            // One interner for every temporal slice of this set in this pass: cons
            // keys ignore spans, so identical subexpressions share cells across slices.
            let mut interner = normalize::NormalFormInterner::new();

            for spec in lemma_spec_set.iter_specs() {
                if let Some(dirty) = &member.dirty_slices {
                    if !dirty.contains(&spec.effective_from) {
                        continue;
                    }
                }
                let spec_name = &spec.name;
                let mut slice_errors = Vec::new();
                let mut slice_plans = Vec::new();

                let breakpoints =
                    match discovery::plan_breakpoints(context, lemma_spec_set, spec, limits) {
                        Ok(dates) => dates,
                        Err(errors) => {
                            slice_errors.extend(errors);
                            // Skip the slice loop: existing error handling below treats empty
                            // slice_plans + non-empty slice_errors as a failed spec.
                            Vec::new()
                        }
                    };

                for effective in breakpoints {
                    let ordered_dependencies = match discovery::discover_dependency_order(
                        context, spec, &effective, limits,
                    ) {
                        Ok(ordered_dependencies) => ordered_dependencies,
                        Err(discovery::DependencyDiscoveryError::Cycle(plan_errors)) => {
                            slice_errors.extend(plan_errors);
                            continue;
                        }
                        Err(discovery::DependencyDiscoveryError::Other(plan_errors)) => {
                            slice_errors.extend(plan_errors);
                            continue;
                        }
                    };

                    match graph::Graph::build(
                        context,
                        repository,
                        spec,
                        &ordered_dependencies,
                        &effective,
                        limits,
                    ) {
                        Ok((graph, resolved_types)) => {
                            match execution_plan::build_execution_plan(
                                &graph,
                                resolved_types,
                                &effective,
                                limits,
                                &mut interner,
                            ) {
                                Ok(mut execution_plan) => {
                                    execution_plan::attach_show_cache(
                                        &mut execution_plan,
                                        lemma_spec_set,
                                        spec,
                                        &versions,
                                    );
                                    slice_plans.push(execution_plan);
                                }
                                Err(plan_errors) => slice_errors.extend(plan_errors),
                            }
                        }
                        Err(build_errors) => {
                            slice_errors.extend(build_errors);
                        }
                    }
                }

                if slice_plans.is_empty() && slice_errors.is_empty() {
                    unreachable!(
                        "BUG: no plans or errors for spec {}, effective from {:?}",
                        spec_name, spec.effective_from
                    );
                }

                if !slice_errors.is_empty() {
                    if slice_errors
                        .iter()
                        .any(|error| error.kind() == crate::ErrorKind::MissingRepository)
                    {
                        missing_repository_source_specs.insert((
                            Arc::clone(repository),
                            spec_name.clone(),
                            spec.effective_from.clone(),
                        ));
                    }
                    errors.extend(
                        slice_errors
                            .into_iter()
                            .map(|err| err.with_spec_context(spec)),
                    );
                    if slice_plans.is_empty() {
                        failed_specs.insert((
                            Arc::clone(repository),
                            spec_name.clone(),
                            spec.effective_from.clone(),
                        ));
                    }
                }

                if !slice_plans.is_empty() {
                    for execution_plan in slice_plans {
                        plans.insert_plan(repository.name.as_deref(), spec_name, execution_plan);
                    }
                }
            }
        }
    }

    // Validate dependency interfaces for the replanned specs. A spec set outside the
    // scope keeps the dependencies it was already validated against: its own edges did
    // not change, and neither did any spec set it reaches, or it would be in the scope.
    let plan_view = PlanView::new(&plans, committed, scope, context);
    for (consumer_repository, consumer_spec_name, consumer_spec, err) in
        discovery::validate_dependency_interfaces(
            context,
            &plan_view,
            scope,
            &missing_repository_source_specs,
            &failed_specs,
        )
    {
        errors.push(err.with_spec_context(consumer_spec));
        failed_plan_sets.insert((consumer_repository, consumer_spec_name));
    }

    // Remove failed plan sets from the plan store.
    for (repository, name) in &failed_plan_sets {
        plans.remove_spec_set(&SpecSetKey::new(repository.name.as_deref(), name));
    }

    dedup_errors(&mut errors);
    PlanningResult { plans, errors }
}

/// Remove duplicate errors in-place, preserving first occurrence order.
/// Two errors are considered duplicates when they share the same kind,
/// message, source location, and owned spec-context attribution.
fn dedup_errors(errors: &mut Vec<Error>) {
    let mut seen = std::collections::HashSet::new();
    errors.retain(|error| {
        let details = error.details();
        let key = (
            error.kind(),
            error.message().to_string(),
            error.location().cloned(),
            details.spec_context_name.clone(),
            details.spec_context_effective_from.clone(),
        );
        seen.insert(key)
    });
}

#[cfg(test)]
mod tests {
    use super::dedup_errors;
    use super::{Arc, Context, EffectiveDate, LemmaRepository, ReplanScope};
    use crate::parsing::ast::{LemmaSpec, Span};
    use crate::parsing::source::{Source, SourceType};
    use crate::Error;

    /// `plan` dedups errors globally, after spec context is attached.
    /// Two errors that agree on kind, message, and location but belong to
    /// different specs are distinct diagnostics: they render as
    /// "In spec 'a': ..." and "In spec 'b': ..." and point the user at two
    /// different places to fix. Dedup must not collapse them.
    #[test]
    fn dedup_keeps_identical_errors_attributed_to_different_specs() {
        let source = Source::new(
            SourceType::Volatile,
            Span {
                start: 0,
                end: 0,
                line: 1,
                col: 0,
            },
        );
        let base = Error::validation("same message", Some(source), None::<String>);
        let spec_a = LemmaSpec::new("a".to_string());
        let spec_b = LemmaSpec::new("b".to_string());
        let mut errors = vec![
            base.clone().with_spec_context(&spec_a),
            base.with_spec_context(&spec_b),
        ];

        dedup_errors(&mut errors);

        assert_eq!(
            errors.len(),
            2,
            "errors from different specs must both survive dedup, got: {:?}",
            errors.iter().map(|e| e.to_string()).collect::<Vec<_>>()
        );
    }

    use crate::literals::DateGranularity;
    use crate::parsing::ast::DateTimeValue;

    fn date(year: i32, month: u32, day: u32) -> DateTimeValue {
        DateTimeValue {
            year,
            month,
            day,
            hour: 0,
            minute: 0,
            second: 0,
            microsecond: 0,
            timezone: None,
            granularity: DateGranularity::Full,
        }
    }

    /// Count the temporal slices that were planned for `spec_name` across all versions.
    fn slice_count(engine: &crate::Engine, spec_name: &str) -> usize {
        engine
            .plans
            .get_plans(None, spec_name)
            .map(|m| m.len())
            .unwrap_or(0)
    }

    /// Run `spec_name` at `at` with no data and return the display string for `rule_name`.
    fn run_display(
        engine: &crate::Engine,
        spec_name: &str,
        at: &DateTimeValue,
        rule_name: &str,
    ) -> String {
        engine
            .run(
                None,
                spec_name,
                Some(at),
                std::collections::HashMap::new(),
                None,
                false,
            )
            .expect("evaluation must succeed")
            .results
            .get(rule_name)
            .expect("rule must be in results")
            .display()
            .expect("rule must have a display value")
            .to_string()
    }

    /// Sum of workspace plan-set sizes for `scale_test_0` .. `scale_test_{count-1}`.
    fn slice_count_for_independent_specs(engine: &crate::Engine, count: usize) -> usize {
        (0..count)
            .map(|i| slice_count(engine, &format!("scale_test_{i}")))
            .sum()
    }

    /// Axis 5: each temporal version is one slice → count exactly.
    #[test]
    fn planning_slice_count_temporal_versions_of_one_spec_is_exact() {
        use crate::tests::scaling_generators::temporal_versions_of_one_spec;

        for count in [1_usize, 4, 16] {
            let engine = load(&temporal_versions_of_one_spec(count));
            let slices = slice_count(&engine, "scale_test");
            assert_eq!(
                slices, count,
                "count={count}: temporal versions of one spec must yield exactly count slices, got {slices}"
            );
        }
    }

    /// Axis 6 — dated independent specs: each spec must get exactly one slice
    /// (its own effective date is the only breakpoint in its closure).
    #[test]
    fn planning_slice_count_dated_independent_specs_each_get_one_slice() {
        use crate::tests::scaling_generators::specs_at_distinct_effective_dates;

        for count in [4_usize, 8] {
            let engine = load(&specs_at_distinct_effective_dates(count));
            let slices = slice_count_for_independent_specs(&engine, count);
            assert_eq!(
                slices, count,
                "count={count}: each independent dated spec must get exactly one slice, got {slices}"
            );
            for i in 0..count {
                assert_eq!(
                    slice_count(&engine, &format!("scale_test_{i}")),
                    1,
                    "scale_test_{i} must have exactly one slice"
                );
            }
        }
    }

    /// Axis 6 — undated control: all specs undated → each gets exactly one slice.
    #[test]
    fn planning_slice_count_undated_independent_specs_each_get_one_slice() {
        use crate::tests::scaling_generators::specs_all_undated;

        for count in [4_usize, 8] {
            let engine = load(&specs_all_undated(count));
            let slices = slice_count_for_independent_specs(&engine, count);
            assert_eq!(
                slices, count,
                "count={count}: undated independent specs must each get exactly one slice, got {slices}"
            );
        }
    }

    fn nf_size(source: &str, spec_name: &str) -> usize {
        let mut engine = crate::Engine::new();
        engine
            .load([(SourceType::Volatile, source.to_string())])
            .expect("spec must load");
        engine
            .plans
            .get_plans(None, spec_name)
            .expect("plans for spec")
            .values()
            .next()
            .expect("at least one slice")
            .normal_forms
            .len()
    }

    /// Axes 1–4: NF size grows with ratio below 8 for fourfold input increase.
    ///
    /// Correct before and after all parts (NF build is already linear).
    #[test]
    fn planning_nf_size_options_scales_near_linear() {
        use crate::tests::scaling_generators::options_per_data_declaration;

        let small = nf_size(&options_per_data_declaration(8), "scale_test");
        let large = nf_size(&options_per_data_declaration(32), "scale_test");
        let ratio = large as f64 / small as f64;
        assert!(
            ratio < 8.0,
            "axis 1 (options): NF size ratio must be < 8 for 4x input; \
             small={small} large={large} ratio={ratio:.2}"
        );
    }

    #[test]
    fn planning_nf_size_shared_path_arms_scales_near_linear() {
        use crate::tests::scaling_generators::unless_arms_on_shared_path;

        let small = nf_size(&unless_arms_on_shared_path(8), "scale_test");
        let large = nf_size(&unless_arms_on_shared_path(32), "scale_test");
        let ratio = large as f64 / small as f64;
        assert!(
            ratio < 8.0,
            "axis 2 (shared-path arms): NF size ratio must be < 8 for 4x input; \
             small={small} large={large} ratio={ratio:.2}"
        );
    }

    #[test]
    fn planning_nf_size_distinct_path_arms_scales_near_linear() {
        use crate::tests::scaling_generators::unless_arms_on_distinct_paths;

        let small = nf_size(&unless_arms_on_distinct_paths(8), "scale_test");
        let large = nf_size(&unless_arms_on_distinct_paths(32), "scale_test");
        let ratio = large as f64 / small as f64;
        assert!(
            ratio < 8.0,
            "axis 3 (distinct-path arms): NF size ratio must be < 8 for 4x input; \
             small={small} large={large} ratio={ratio:.2}"
        );
    }

    #[test]
    fn planning_nf_size_conjunction_chain_scales_near_linear() {
        use crate::tests::scaling_generators::conjunction_chain;

        let small = nf_size(&conjunction_chain(8), "scale_test");
        let large = nf_size(&conjunction_chain(32), "scale_test");
        let ratio = large as f64 / small as f64;
        assert!(
            ratio < 8.0,
            "axis 4 (conjunction chain): NF size ratio must be < 8 for 4x input; \
             small={small} large={large} ratio={ratio:.2}"
        );
    }

    fn load(source: &str) -> crate::Engine {
        let mut engine = crate::Engine::new();
        engine
            .load([(SourceType::Volatile, source.to_string())])
            .expect("spec must load");
        engine
    }

    // --- Part 2 corpus: breakpoint correctness ---

    /// Unpinned dependency with two versions inside the consumer's range → consumer gets
    /// exactly two slices.  Evaluating at dates within each slice yields the corresponding
    /// dependency version's value.
    #[test]
    fn breakpoints_unpinned_dep_two_versions_consumer_gets_two_slices() {
        let engine = load(
            r#"
spec dep
data v: 1
rule r: v

spec dep 2025-06-01
data v: 2
rule r: v

spec consumer 2025-01-01
uses d: dep
rule out: d.r
"#,
        );
        assert_eq!(
            slice_count(&engine, "consumer"),
            2,
            "consumer must have exactly 2 slices"
        );
        assert_eq!(
            run_display(&engine, "consumer", &date(2025, 3, 1), "out"),
            "1",
            "before dep v2 boundary: must use dep v1"
        );
        assert_eq!(
            run_display(&engine, "consumer", &date(2025, 9, 1), "out"),
            "2",
            "after dep v2 boundary: must use dep v2"
        );
    }

    /// Pinned dependency with two versions → consumer gets exactly one slice regardless of
    /// the number of dep versions in the context.
    #[test]
    fn breakpoints_pinned_dep_two_versions_consumer_gets_one_slice() {
        let engine = load(
            r#"
spec dep
data v: 1
rule r: v

spec dep 2025-06-01
data v: 2
rule r: v

spec consumer 2025-01-01
uses d: dep 2025-01-01
rule out: d.r
"#,
        );
        assert_eq!(
            slice_count(&engine, "consumer"),
            1,
            "pinned dep: consumer must have exactly 1 slice"
        );
        assert_eq!(
            run_display(&engine, "consumer", &date(2025, 3, 1), "out"),
            "1",
            "pinned dep: must always use dep v1"
        );
        assert_eq!(
            run_display(&engine, "consumer", &date(2025, 9, 1), "out"),
            "1",
            "pinned dep: must still use dep v1 after dep v2 boundary"
        );
    }

    /// Pinned dependency whose own dependency has versions → consumer still gets one slice.
    /// The pin prunes the entire subtree: nothing beneath the pinned dep can shift.
    #[test]
    fn breakpoints_pinned_dep_subtree_pruned_consumer_gets_one_slice() {
        let engine = load(
            r#"
spec base
data v: 10
rule r: v

spec base 2025-06-01
data v: 20
rule r: v

spec dep 2025-01-01
uses b: base
rule r: b.r

spec consumer 2025-01-01
uses d: dep 2025-01-01
rule out: d.r
"#,
        );
        assert_eq!(
            slice_count(&engine, "consumer"),
            1,
            "pinned dep with versioned transitive dep: consumer must still get 1 slice"
        );
    }

    /// Origin consumer with a dependency that gains a second version at a dated boundary →
    /// consumer gets two slices (at Origin using dep v1, and at dep's v2 date using dep v2).
    #[test]
    fn breakpoints_origin_consumer_dated_dep_gets_two_slices() {
        let engine = load(
            r#"
spec dep
data v: 5
rule r: v

spec dep 2025-01-01
data v: 99
rule r: v

spec consumer
uses d: dep
rule out: d.r
"#,
        );
        assert_eq!(
            slice_count(&engine, "consumer"),
            2,
            "origin consumer with dated dep: must have slices at Origin and dep's second-version date"
        );
        assert_eq!(
            run_display(&engine, "consumer", &date(2024, 6, 1), "out"),
            "5",
            "before dep v2: must use dep v1"
        );
        assert_eq!(
            run_display(&engine, "consumer", &date(2025, 3, 1), "out"),
            "99",
            "after dep v2 boundary: must use dep v2"
        );
    }

    /// Diamond closure: two paths lead to the same dependency. Dependency version dates
    /// must be counted once, not duplicated.
    #[test]
    fn breakpoints_diamond_closure_dates_counted_once() {
        let engine = load(
            r#"
spec dep
data v: 1
rule r: v

spec dep 2025-06-01
data v: 2
rule r: v

spec mid_a
uses d: dep
rule r: d.r

spec mid_b
uses d: dep
rule r: d.r

spec consumer
uses a: mid_a
uses b: mid_b
rule out: a.r
"#,
        );
        // Two paths to dep but only 2 breakpoints: Origin and 2025-06-01.
        assert_eq!(
            slice_count(&engine, "consumer"),
            2,
            "diamond closure: dep dates must be counted once, not doubled"
        );
        assert_eq!(
            run_display(&engine, "consumer", &date(2025, 3, 1), "out"),
            "1",
            "diamond: before dep v2 boundary"
        );
        assert_eq!(
            run_display(&engine, "consumer", &date(2025, 9, 1), "out"),
            "2",
            "diamond: after dep v2 boundary"
        );
    }

    /// Unrelated noise boundaries inside the consumer's range must not add slices.
    /// A consumer with no dependencies should have exactly one slice regardless of how
    /// many other dated specs are in the context.
    #[test]
    fn breakpoints_unrelated_noise_boundaries_do_not_add_slices() {
        let engine = load(
            r#"
spec consumer
data v: 1
rule out: v

spec noise_a 2025-01-01
data x: 1

spec noise_b 2025-06-01
data x: 1
"#,
        );
        assert_eq!(
            slice_count(&engine, "consumer"),
            1,
            "independent consumer must have exactly 1 slice even with unrelated dated specs"
        );
    }

    /// A dependency closure that breaches max_dag_specs must return an error, and the spec
    /// must surface no slices.
    #[test]
    fn breakpoints_closure_exceeding_max_dag_specs_errors() {
        // Build a spec with many dependencies to exceed the tiny limit.
        let mut source = String::new();
        for i in 0..6 {
            source.push_str(&format!("spec dep_{i}\ndata v: {i}\nrule r: v\n\n"));
        }
        source.push_str("spec consumer\n");
        for i in 0..6 {
            source.push_str(&format!("uses d_{i}: dep_{i}\n"));
        }
        source.push_str("rule out: d_0.r\n");

        let limits = crate::ResourceLimits {
            max_dag_specs: 3,
            ..crate::ResourceLimits::default()
        };
        let mut engine = crate::Engine::with_limits(limits);
        engine
            .load([(SourceType::Volatile, source)])
            .expect_err("must fail with too many closure members");

        // No slice must have been produced for consumer.
        assert_eq!(
            slice_count(&engine, "consumer"),
            0,
            "consumer must have no slices when closure exceeds max_dag_specs"
        );
    }

    // --- Incremental replanning: scope, reuse, transactionality ---

    use crate::planning::normalize::NormalForm;
    use std::path::PathBuf;

    fn path_source(name: &str) -> SourceType {
        SourceType::Path(Arc::new(PathBuf::from(name)))
    }

    /// Build a context from one source text and return it with its default repository.
    fn context_from(source: &str) -> (Context, Arc<LemmaRepository>) {
        let specs = crate::parse(
            source,
            SourceType::Volatile,
            &crate::ResourceLimits::default(),
        )
        .expect("parse")
        .into_flattened_specs();
        let mut context = Context::new();
        let workspace = context.workspace();
        for spec in specs {
            context
                .insert_spec(Arc::clone(&workspace), spec)
                .expect("insert spec");
        }
        (context, workspace)
    }

    /// Spec-set names the scope covers when `changed` names change, default repository only.
    fn scope_names(source: &str, changed: &[&str]) -> std::collections::BTreeSet<String> {
        let (context, workspace) = context_from(source);
        let scope = ReplanScope::from_changed_sets(
            &context,
            changed
                .iter()
                .map(|name| (Arc::clone(&workspace), (*name).to_string()))
                .collect(),
        );
        scope
            .members()
            .map(|member| member.key.spec.clone())
            .collect::<std::collections::BTreeSet<_>>()
    }

    /// Heap identity of each slice's normal-form table for a spec set.
    ///
    /// A moved (reused) plan keeps its buffer address; a rebuilt plan allocates a new
    /// one. Empty tables are rejected because every empty `Vec` shares one dangling
    /// address, which would make reuse indistinguishable from a rebuild.
    fn plan_buffers(
        engine: &crate::Engine,
        repository: Option<&str>,
        spec_name: &str,
    ) -> Vec<*const NormalForm> {
        let plans = engine
            .plans
            .get_plans(repository, spec_name)
            .unwrap_or_else(|| panic!("plans for spec '{spec_name}'"));
        assert!(!plans.is_empty(), "spec '{spec_name}' must have slices");
        plans
            .values()
            .map(|plan| {
                assert!(
                    !plan.normal_forms.is_empty(),
                    "spec '{spec_name}' must have a non-empty normal-form table for buffer \
                     identity to be meaningful; give it a rule"
                );
                plan.normal_forms.as_ptr()
            })
            .collect()
    }

    /// Structural fingerprint of the whole plan store: every spec set, its slice
    /// boundaries, and per-slice rule names and normal-form table size.
    type StoreShape = Vec<(
        Option<String>,
        String,
        Vec<(EffectiveDate, Vec<String>, usize)>,
    )>;

    fn store_shape(engine: &crate::Engine) -> StoreShape {
        let mut shape: StoreShape = engine
            .plans
            .plans
            .iter()
            .flat_map(|(repository, by_name)| {
                by_name.iter().map(move |(spec_name, slices)| {
                    let slice_shapes = slices
                        .iter()
                        .map(|(effective, plan)| {
                            (
                                effective.clone(),
                                plan.rules
                                    .values()
                                    .map(|rule| rule.name().to_string())
                                    .collect(),
                                plan.normal_forms.len(),
                            )
                        })
                        .collect();
                    (repository.clone(), spec_name.clone(), slice_shapes)
                })
            })
            .collect();
        shape.sort();
        shape
    }

    /// A change propagates to every transitive consumer and stops there.
    #[test]
    fn replan_scope_covers_transitive_consumers_only() {
        let source = r#"
spec base
data v: 1
rule r: v

spec mid
uses b: base
rule r: b.r

spec root
uses m: mid
rule out: m.r

spec unrelated
data v: 2
rule r: v
"#;
        assert_eq!(
            scope_names(source, &["base"]),
            ["base", "mid", "root"]
                .into_iter()
                .map(String::from)
                .collect::<std::collections::BTreeSet<_>>(),
            "a change to 'base' must dirty 'mid' and 'root', and must not dirty 'unrelated'"
        );
        assert_eq!(
            scope_names(source, &["unrelated"]),
            ["unrelated"]
                .into_iter()
                .map(String::from)
                .collect::<std::collections::BTreeSet<_>>(),
            "a leaf change must dirty nothing else"
        );
        assert_eq!(
            scope_names(source, &["mid"]),
            ["mid", "root"]
                .into_iter()
                .map(String::from)
                .collect::<std::collections::BTreeSet<_>>(),
            "dirtiness flows to consumers, never down into dependencies"
        );
    }

    /// A pin fixes the resolution instant, not the pinned spec's source text or its own
    /// dependencies. A pinned consumer must therefore still be replanned when the spec
    /// set it pins into changes.
    #[test]
    fn replan_scope_includes_pinned_consumers() {
        let source = r#"
spec dep 2025-01-01
data v: 1
rule r: v

spec pinned_consumer 2025-01-01
uses d: dep 2025-01-01
rule out: d.r
"#;
        assert_eq!(
            scope_names(source, &["dep"]),
            ["dep", "pinned_consumer"]
                .into_iter()
                .map(String::from)
                .collect::<std::collections::BTreeSet<_>>(),
            "pinning must not exempt a consumer from replanning"
        );
    }

    /// A diamond closure must dirty each consumer once and reach the root.
    #[test]
    fn replan_scope_handles_diamond_closure() {
        let source = r#"
spec base
data v: 1
rule r: v

spec left
uses b: base
rule r: b.r

spec right
uses b: base
rule r: b.r

spec root
uses l: left
uses r_dep: right
rule out: l.r
"#;
        assert_eq!(
            scope_names(source, &["base"]),
            ["base", "left", "right", "root"]
                .into_iter()
                .map(String::from)
                .collect::<std::collections::BTreeSet<_>>(),
            "both diamond arms and the root must be dirty"
        );
    }

    /// A dependency cycle must not make scope computation diverge.
    #[test]
    fn replan_scope_terminates_on_dependency_cycle() {
        let source = r#"
spec a
uses b_dep: b
data amount: number

spec b
uses a_dep: a
data imported: a_dep.amount
"#;
        assert_eq!(
            scope_names(source, &["a"]),
            ["a", "b"]
                .into_iter()
                .map(String::from)
                .collect::<std::collections::BTreeSet<_>>(),
            "a cyclic closure must be dirtied once and terminate"
        );
    }

    /// Editing a workspace spec must not rebuild the plans of an untouched dependency
    /// repository: those plans move into the new store instead.
    #[test]
    fn dependency_plans_are_reused_across_a_workspace_edit() {
        let mut engine = crate::Engine::new();
        engine
            .load([(
                SourceType::Dependency("@iso/countries".to_string()),
                r#"repo @iso/countries
spec alpha2
data code: 1
rule current: code

spec alpha2 2025-06-01
data code: 2
rule current: code

spec alpha2 2026-01-01
data code: 3
rule current: code
"#
                .to_string(),
            )])
            .expect("dependency loads");
        engine
            .load([(
                path_source("consumer.lemma"),
                "spec consumer\nuses iso: @iso/countries alpha2\nrule out: iso.current\n"
                    .to_string(),
            )])
            .expect("consumer loads");

        let dependency_before = plan_buffers(&engine, Some("@iso/countries"), "alpha2");
        let consumer_before = plan_buffers(&engine, None, "consumer");
        assert_eq!(
            dependency_before.len(),
            3,
            "dependency must have one slice per temporal version"
        );

        engine
            .update(
                None,
                "spec consumer\nuses iso: @iso/countries alpha2\nrule out: iso.current + 1\n"
                    .to_string(),
                path_source("consumer.lemma"),
            )
            .expect("consumer edit applies");

        assert_eq!(
            plan_buffers(&engine, Some("@iso/countries"), "alpha2"),
            dependency_before,
            "untouched dependency slices must be moved, not replanned"
        );
        assert_ne!(
            plan_buffers(&engine, None, "consumer"),
            consumer_before,
            "the edited spec must be replanned"
        );
    }

    /// Editing a dependency must replan it and every transitive consumer, and leave
    /// unrelated spec sets alone.
    #[test]
    fn editing_a_dependency_replans_transitive_consumers_only() {
        let mut engine = crate::Engine::new();
        engine
            .load([
                (
                    path_source("base.lemma"),
                    "spec base\ndata v: 1\nrule r: v\n",
                ),
                (
                    path_source("mid.lemma"),
                    "spec mid\nuses b: base\nrule r: b.r\n",
                ),
                (
                    path_source("root.lemma"),
                    "spec root\nuses m: mid\nrule out: m.r\n",
                ),
                (
                    path_source("unrelated.lemma"),
                    "spec unrelated\ndata v: 2\nrule r: v\n",
                ),
            ])
            .expect("initial load");

        let base_before = plan_buffers(&engine, None, "base");
        let mid_before = plan_buffers(&engine, None, "mid");
        let root_before = plan_buffers(&engine, None, "root");
        let unrelated_before = plan_buffers(&engine, None, "unrelated");

        engine
            .update(
                None,
                "spec base\ndata v: 7\nrule r: v\n".to_string(),
                path_source("base.lemma"),
            )
            .expect("base edit applies");

        assert_ne!(
            plan_buffers(&engine, None, "base"),
            base_before,
            "the edited dependency must be replanned"
        );
        assert_ne!(
            plan_buffers(&engine, None, "mid"),
            mid_before,
            "a direct consumer must be replanned"
        );
        assert_ne!(
            plan_buffers(&engine, None, "root"),
            root_before,
            "a transitive consumer must be replanned"
        );
        assert_eq!(
            plan_buffers(&engine, None, "unrelated"),
            unrelated_before,
            "an unrelated spec set must keep its plans"
        );
        assert_eq!(
            run_display(&engine, "root", &date(2025, 1, 1), "out"),
            "7",
            "the transitive consumer must observe the edited dependency value"
        );
    }

    /// A pinned consumer reads one fixed dependency slice, but that slice's source can
    /// change underneath it. The pinned consumer must be replanned and must observe the
    /// new value.
    #[test]
    fn editing_a_pinned_dependency_slice_replans_the_pinned_consumer() {
        let mut engine = crate::Engine::new();
        engine
            .load([
                (
                    path_source("dep.lemma"),
                    "spec dep 2025-01-01\ndata v: 1\nrule r: v\n",
                ),
                (
                    path_source("consumer.lemma"),
                    "spec consumer 2025-01-01\nuses d: dep 2025-01-01\nrule out: d.r\n",
                ),
            ])
            .expect("initial load");

        let consumer_before = plan_buffers(&engine, None, "consumer");
        assert_eq!(
            run_display(&engine, "consumer", &date(2025, 3, 1), "out"),
            "1"
        );

        engine
            .update(
                None,
                "spec dep 2025-01-01\ndata v: 42\nrule r: v\n".to_string(),
                path_source("dep.lemma"),
            )
            .expect("pinned dependency slice edit applies");

        assert_ne!(
            plan_buffers(&engine, None, "consumer"),
            consumer_before,
            "a pinned consumer must be replanned when the slice it pins changes"
        );
        assert_eq!(
            run_display(&engine, "consumer", &date(2025, 3, 1), "out"),
            "42",
            "a pinned consumer must observe the new source of the pinned slice"
        );
    }

    /// Removing one temporal version of a dependency changes the sibling slice windows
    /// and the consumer's breakpoints, so the whole dependency set and its consumers
    /// must be replanned.
    #[test]
    fn removing_a_dependency_version_replans_the_set_and_its_consumers() {
        let mut engine = crate::Engine::new();
        engine
            .load([
                (
                    path_source("dep_v1.lemma"),
                    "spec dep\ndata v: 1\nrule r: v\n",
                ),
                (
                    path_source("dep_v2.lemma"),
                    "spec dep 2025-06-01\ndata v: 2\nrule r: v\n",
                ),
                (
                    path_source("consumer.lemma"),
                    "spec consumer\nuses d: dep\nrule out: d.r\n",
                ),
            ])
            .expect("initial load");

        assert_eq!(
            slice_count(&engine, "consumer"),
            2,
            "unpinned consumer must have a slice per dependency version"
        );

        engine
            .remove(None, "dep", Some(&date(2025, 6, 1)))
            .expect("removing the second dependency version applies");

        assert_eq!(
            slice_count(&engine, "dep"),
            1,
            "the dependency set must lose its second slice"
        );
        assert_eq!(
            slice_count(&engine, "consumer"),
            1,
            "the consumer must lose the breakpoint the removed version introduced"
        );
        assert_eq!(
            run_display(&engine, "consumer", &date(2025, 9, 1), "out"),
            "1",
            "after removal the consumer must resolve to the remaining version"
        );
    }

    /// Removing the last version of a spec set drops its plans from the store.
    #[test]
    fn removing_the_last_version_drops_the_plan_set() {
        let mut engine = crate::Engine::new();
        engine
            .load([(
                path_source("solo.lemma"),
                "spec solo\ndata v: 1\nrule r: v\n".to_string(),
            )])
            .expect("initial load");
        assert_eq!(slice_count(&engine, "solo"), 1);

        engine
            .remove(None, "solo", None)
            .expect("removing the only version applies");

        assert_eq!(
            slice_count(&engine, "solo"),
            0,
            "a spec set that left the context must leave no plans behind"
        );
    }

    /// A failed batch must leave the committed store byte-for-byte as it was, including
    /// the plans of spec sets the failed batch would have replanned.
    #[test]
    fn a_failed_batch_leaves_committed_plans_untouched() {
        let mut engine = crate::Engine::new();
        engine
            .load([
                (path_source("dep.lemma"), "spec dep\ndata v: 1\nrule r: v\n"),
                (
                    path_source("consumer.lemma"),
                    "spec consumer\nuses d: dep\nrule out: d.r\n",
                ),
            ])
            .expect("initial load");

        let shape_before = store_shape(&engine);
        let dep_before = plan_buffers(&engine, None, "dep");
        let consumer_before = plan_buffers(&engine, None, "consumer");

        // Replacing the dependency with a version that no longer exposes rule `r`
        // breaks the consumer. Both spec sets are in the replan scope, so both are
        // rebuilt during the failed pass.
        engine
            .update(
                None,
                "spec dep\ndata other: 5\nrule unrelated_rule: other\n".to_string(),
                path_source("dep.lemma"),
            )
            .expect_err("the batch must fail because the consumer's dependency broke");

        assert_eq!(
            plan_buffers(&engine, None, "dep"),
            dep_before,
            "a failed batch must not replace the dependency's plans"
        );
        assert_eq!(
            plan_buffers(&engine, None, "consumer"),
            consumer_before,
            "a failed batch must not replace a replanned consumer's plans"
        );
        assert_eq!(
            store_shape(&engine),
            shape_before,
            "a failed batch must leave the whole store unchanged"
        );
        assert_eq!(
            run_display(&engine, "consumer", &date(2025, 1, 1), "out"),
            "1",
            "the pre-failure plans must still evaluate"
        );
    }

    /// Removing a spec set that another spec set still depends on must fail the batch
    /// and leave both spec sets' plans in place. The removed spec set is in the replan
    /// scope but no longer in the context, so this drives the pass over a scope member
    /// that has no spec set to plan.
    #[test]
    fn removing_a_dependency_that_has_a_consumer_fails_and_rolls_back() {
        let mut engine = crate::Engine::new();
        engine
            .load([
                (path_source("dep.lemma"), "spec dep\ndata v: 1\nrule r: v\n"),
                (
                    path_source("consumer.lemma"),
                    "spec consumer\nuses d: dep\nrule out: d.r\n",
                ),
            ])
            .expect("initial load");

        let shape_before = store_shape(&engine);
        let consumer_before = plan_buffers(&engine, None, "consumer");

        engine
            .remove(None, "dep", None)
            .expect_err("removing a spec set that still has a consumer must fail");

        assert_eq!(
            store_shape(&engine),
            shape_before,
            "the failed removal must leave the store unchanged"
        );
        assert_eq!(
            plan_buffers(&engine, None, "consumer"),
            consumer_before,
            "the consumer must keep the exact plans it had before the failed removal"
        );
        assert_eq!(
            run_display(&engine, "consumer", &date(2025, 1, 1), "out"),
            "1",
            "the consumer must still evaluate against its dependency"
        );
    }

    /// A new spec that fails to plan must not disturb existing plans.
    #[test]
    fn a_failed_new_spec_leaves_existing_plans_untouched() {
        let mut engine = crate::Engine::new();
        engine
            .load([(
                path_source("good.lemma"),
                "spec good\ndata v: 1\nrule r: v\n".to_string(),
            )])
            .expect("initial load");
        let shape_before = store_shape(&engine);

        engine
            .load([(
                path_source("bad.lemma"),
                "spec bad\nuses missing_dep: nonexistent\nrule out: missing_dep.r\n".to_string(),
            )])
            .expect_err("a spec referencing a missing dependency must fail to load");

        assert_eq!(
            store_shape(&engine),
            shape_before,
            "a failed load must leave the store unchanged"
        );
    }

    /// A dirty consumer reading an untouched dependency must find that dependency's
    /// committed plans. The embedded stdlib is the case that always exists: it is
    /// loaded by `Engine::new` and never mutated again, and it carries no rules, so
    /// its plans exercise the empty-normal-form-table path through commit.
    ///
    /// If a commit dropped the stdlib's plans, the next edit of a spec that imports
    /// it would panic inside [`PlanView::get_plans`] instead of applying.
    #[test]
    fn a_dirty_consumer_validates_against_committed_stdlib_plans() {
        let mut engine = crate::Engine::new();
        engine
            .load([(
                path_source("measures.lemma"),
                "spec measures\nuses lemma units\ndata distance: 5 kilometer\nrule doubled: distance * 2\n"
                    .to_string(),
            )])
            .expect("spec importing the embedded stdlib loads");

        let stdlib_before: StoreShape = store_shape(&engine)
            .into_iter()
            .filter(|(repository, _, _)| repository.as_deref() == Some("lemma"))
            .collect();
        assert!(
            !stdlib_before.is_empty(),
            "the embedded stdlib must be planned after Engine::new"
        );

        engine
            .update(
                None,
                "spec measures\nuses lemma units\ndata distance: 5 kilometer\nrule tripled: distance * 3\n"
                    .to_string(),
                path_source("measures.lemma"),
            )
            .expect("editing the consumer of the embedded stdlib applies");

        assert_eq!(
            store_shape(&engine)
                .into_iter()
                .filter(|(repository, _, _)| repository.as_deref() == Some("lemma"))
                .collect::<StoreShape>(),
            stdlib_before,
            "a workspace edit must leave the embedded stdlib's plans as they were"
        );
        assert_eq!(
            run_display(&engine, "measures", &date(2025, 1, 1), "tripled"),
            "15 kilometer",
            "the edited consumer must expose its new rule and still resolve stdlib units"
        );
    }

    /// Reaching a context incrementally must produce the same plans as planning that
    /// context from scratch. This is the guarantee that makes scoped replanning safe.
    #[test]
    fn incremental_and_from_scratch_planning_agree() {
        let base_v1 = "spec base\ndata v: 1\nrule r: v\n";
        let base_v2 = "spec base 2025-06-01\ndata v: 2\nrule r: v\n";
        let mid = "spec mid\nuses b: base\nrule r: b.r + 1\n";
        let root = "spec root\nuses m: mid\nrule out: m.r * 2\n";
        let pinned = "spec pinned\nuses b: base 2025-06-01\nrule out: b.r\n";

        let mut incremental = crate::Engine::new();
        incremental
            .load([(path_source("base_v1.lemma"), base_v1.to_string())])
            .expect("load base v1");
        incremental
            .load([(path_source("mid.lemma"), mid.to_string())])
            .expect("load mid");
        incremental
            .load([(path_source("root.lemma"), root.to_string())])
            .expect("load root");
        incremental
            .load([(path_source("base_v2.lemma"), base_v2.to_string())])
            .expect("load base v2");
        incremental
            .load([(path_source("pinned.lemma"), pinned.to_string())])
            .expect("load pinned");

        let mut from_scratch = crate::Engine::new();
        from_scratch
            .load([
                (path_source("base_v1.lemma"), base_v1),
                (path_source("base_v2.lemma"), base_v2),
                (path_source("mid.lemma"), mid),
                (path_source("root.lemma"), root),
                (path_source("pinned.lemma"), pinned),
            ])
            .expect("load everything at once");

        assert_eq!(
            store_shape(&incremental),
            store_shape(&from_scratch),
            "incremental planning must produce the same plan store as a single full pass"
        );

        for at in [date(2025, 3, 1), date(2025, 9, 1)] {
            assert_eq!(
                run_display(&incremental, "root", &at, "out"),
                run_display(&from_scratch, "root", &at, "out"),
                "root must evaluate identically at {at}"
            );
            assert_eq!(
                run_display(&incremental, "pinned", &at, "out"),
                run_display(&from_scratch, "pinned", &at, "out"),
                "pinned must evaluate identically at {at}"
            );
        }
    }

    /// Repeating the same edit must reach the same store: no order or iteration
    /// dependence leaks in through scoped commits.
    #[test]
    fn repeated_identical_edits_reach_the_same_store() {
        let mut engine = crate::Engine::new();
        engine
            .load([
                (path_source("dep.lemma"), "spec dep\ndata v: 1\nrule r: v\n"),
                (
                    path_source("consumer.lemma"),
                    "spec consumer\nuses d: dep\nrule out: d.r\n",
                ),
            ])
            .expect("initial load");

        let edit = "spec dep\ndata v: 9\nrule r: v\n";
        engine
            .update(None, edit.to_string(), path_source("dep.lemma"))
            .expect("first edit");
        let shape_after_first = store_shape(&engine);

        for _ in 0..3 {
            engine
                .update(None, edit.to_string(), path_source("dep.lemma"))
                .expect("repeat edit");
            assert_eq!(
                store_shape(&engine),
                shape_after_first,
                "re-applying the same edit must reach the same store shape"
            );
        }
    }

    /// Body-only edit of the last temporal version must rebuild that version's
    /// plans and keep earlier slice plan buffer identity (earlier spans unchanged).
    #[test]
    fn body_edit_one_version_preserves_other_slice_plan_identity() {
        let mut engine = crate::Engine::new();
        engine
            .load([(
                path_source("versions.lemma"),
                r#"spec item
data v: 1
rule r: v

spec item 2025-06-01
data v: 2
rule r: v

spec item 2026-01-01
data v: 3
rule r: v
"#
                .to_string(),
            )])
            .expect("load versions");

        let before = plan_buffers(&engine, None, "item");
        assert_eq!(before.len(), 3);

        engine
            .update(
                None,
                r#"spec item
data v: 1
rule r: v

spec item 2025-06-01
data v: 2
rule r: v

spec item 2026-01-01
data v: 30
rule r: v
"#
                .to_string(),
                path_source("versions.lemma"),
            )
            .expect("body-edit last version");

        let after = plan_buffers(&engine, None, "item");
        assert_eq!(after.len(), 3);
        assert_eq!(
            after[0], before[0],
            "origin slice plan buffer must be reused"
        );
        assert_eq!(
            after[1], before[1],
            "unedited mid slice plan buffer must be reused"
        );
        assert_ne!(after[2], before[2], "edited last slice must be rebuilt");
        assert_eq!(
            run_display(&engine, "item", &date(2026, 2, 1), "r"),
            "30",
            "edited version must evaluate the new body"
        );
    }

    /// Adding or removing a temporal version dirties the whole spec set.
    #[test]
    fn version_add_forces_whole_set_replan() {
        let mut engine = crate::Engine::new();
        engine
            .load([(
                path_source("versions.lemma"),
                r#"spec item
data v: 1
rule r: v

spec item 2025-06-01
data v: 2
rule r: v
"#
                .to_string(),
            )])
            .expect("load versions");

        let before = plan_buffers(&engine, None, "item");
        assert_eq!(before.len(), 2);

        engine
            .update(
                None,
                r#"spec item
data v: 1
rule r: v

spec item 2025-06-01
data v: 2
rule r: v

spec item 2026-01-01
data v: 3
rule r: v
"#
                .to_string(),
                path_source("versions.lemma"),
            )
            .expect("add third version");

        let after = plan_buffers(&engine, None, "item");
        assert_eq!(after.len(), 3);
        assert_ne!(
            after[0], before[0],
            "whole-set replan must rebuild the origin slice"
        );
        assert_ne!(
            after[1], before[1],
            "whole-set replan must rebuild the mid slice"
        );
    }
}

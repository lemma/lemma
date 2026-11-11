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
pub mod instance_list;
pub mod normalize;
pub mod ordered_dispatch;
pub mod semantics;
pub mod show_expression;
pub mod spec_set;
pub(crate) mod spec_value;
pub mod types;
pub mod typing;
pub mod unit_family;
pub mod unit_index;
use crate::engine::Context;
use crate::parsing::ast::{DateTimeValue, EffectiveDate, LemmaRepository, LemmaSpec};
use crate::Error;
pub use execution_plan::ExecutionPlan;
use indexmap::IndexMap;
pub use spec_set::LemmaSpecSet;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::unreachable;

/// Temporal slices of one spec, keyed by [`ExecutionPlan::effective`].
pub(crate) type PlanSlices = BTreeMap<EffectiveDate, Arc<ExecutionPlan>>;

/// Compiled plans keyed by repository name (`None` = default unnamed repository) then spec name.
///
/// Temporal slices per spec are a [`BTreeMap`] keyed by [`ExecutionPlan::effective`].
/// Values are [`Arc`] so a planning pass can intern the same plan as a named-`uses`
/// / row template without a second deep clone.
#[derive(Debug, Default)]
pub(crate) struct PlanStore {
    plans: IndexMap<Option<String>, IndexMap<String, PlanSlices>>,
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
        plan: Arc<ExecutionPlan>,
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
    pub(crate) fn get_plans(&self, repository: Option<&str>, spec: &str) -> Option<&PlanSlices> {
        self.plans
            .get(&Self::repo_key(repository))
            .and_then(|by_name| {
                by_name.get(crate::parsing::ast::canonical_logical_name(spec).as_ref())
            })
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
    /// Plans for every spec set of the context. Committed only when `errors` is empty.
    pub plans: PlanStore,
    pub errors: Vec<Error>,
}

/// Every spec set of `context` with its identity, ordered by repository name then spec
/// name, so a pass
/// visits sets in an order fixed by their identities and not by load order.
pub(crate) fn spec_sets_in_order(
    context: &Context,
) -> Vec<(&Arc<LemmaRepository>, &String, &LemmaSpecSet)> {
    let mut sets: Vec<_> = context
        .repositories()
        .iter()
        .flat_map(|(repository, by_name)| {
            by_name
                .iter()
                .map(move |(name, spec_set)| (repository, name, spec_set))
        })
        .collect();
    sets.sort_by(|(repo_a, name_a, _), (repo_b, name_b, _)| {
        repo_a
            .name
            .as_deref()
            .cmp(&repo_b.name.as_deref())
            .then_with(|| name_a.as_str().cmp(name_b.as_str()))
    });
    sets
}

/// One dependency edge of a slice plan: the spec version it reaches and that version's
/// slice.
#[derive(Clone)]
struct SliceEdge<'a> {
    repository: Arc<LemmaRepository>,
    spec: &'a LemmaSpec,
    identity: semantics::SpecIdentity,
}

/// The slices the dependency edges of the `slice` plan of `spec` resolve to, in edge
/// order, self-edges left out.
fn slice_edges<'a>(
    context: &'a Context,
    repository: &Arc<LemmaRepository>,
    spec: &LemmaSpec,
    slice: &EffectiveDate,
    slices: &mut instance_list::SpecSlices,
    limits: &crate::limits::ResourceLimits,
) -> Result<Vec<SliceEdge<'a>>, Vec<Error>> {
    let mut edges = Vec::new();
    let mut errors = Vec::new();
    for edge in discovery::dependency_edges(spec, repository, context)? {
        match discovery::resolve_dependency_targets(
            context, &edge, repository, spec, slice, slices, limits,
        ) {
            Ok(targets) => {
                for dependency in targets {
                    match slices.identity_at(
                        context,
                        &dependency.repository,
                        dependency.spec,
                        &dependency.at,
                        limits,
                    ) {
                        Ok(identity) => edges.push(SliceEdge {
                            repository: dependency.repository,
                            spec: dependency.spec,
                            identity,
                        }),
                        Err(slice_errors) => errors.extend(slice_errors),
                    }
                }
            }
            Err(resolve_errors) => errors.extend(resolve_errors),
        }
    }
    if errors.is_empty() {
        Ok(edges)
    } else {
        Err(errors)
    }
}

/// The committed plan of slice `identity` of `spec` with the slice's edges, when this pass
/// would build it from the same own bytes `Arc`, boundaries and edge slices. Whether the edge
/// slices are themselves reused is left to [`seed_reusable_plans`].
fn reuse_candidate<'a>(
    context: &'a Context,
    committed: &PlanStore,
    repository: &Arc<LemmaRepository>,
    spec: &LemmaSpec,
    identity: &semantics::SpecIdentity,
    slices: &mut instance_list::SpecSlices,
    limits: &crate::limits::ResourceLimits,
) -> Option<(Arc<ExecutionPlan>, Vec<SliceEdge<'a>>)> {
    let candidate = committed
        .get_plans(repository.name.as_deref(), &spec.name)?
        .get(&identity.slice)?;
    if candidate.identity != *identity {
        return None;
    }
    let spec_set = context
        .spec_set(repository, &spec.name)
        .expect("BUG: reuse checked for a spec missing from context");
    if !Arc::ptr_eq(spec_set.spec_arc(spec), &candidate.own_spec) {
        return None;
    }
    let (effective_from, effective_to) = spec_set.effective_range(spec);
    if candidate.effective_from != effective_from || candidate.effective_to != effective_to {
        return None;
    }
    // Edges that fail to resolve are reported when the slice is built.
    let edges = slice_edges(context, repository, spec, &identity.slice, slices, limits).ok()?;
    if !edges
        .iter()
        .map(|edge| &edge.identity)
        .eq(candidate.edges.iter())
    {
        return None;
    }
    Some((Arc::clone(candidate), edges))
}

/// A slice whose committed plan passed [`reuse_candidate`], waiting on its edge slices.
struct ReuseFrame<'a> {
    identity: semantics::SpecIdentity,
    candidate: Arc<ExecutionPlan>,
    edges: Vec<SliceEdge<'a>>,
    next_edge: usize,
}

/// Put every committed slice plan this pass would build identically into
/// `interners.spec_templates`, so [`get_or_build_template`] returns it as is.
///
/// A slice is reused when [`reuse_candidate`] accepts it and every slice its edges reach
/// is reused. Decisions are made once per slice, depth first on an explicit stack; an
/// edge back to a slice still on the stack is not reused.
fn seed_reusable_plans(
    context: &Context,
    committed: &PlanStore,
    limits: &crate::limits::ResourceLimits,
    interners: &mut instance_list::PassInterners,
) {
    let mut decisions: HashMap<semantics::SpecIdentity, bool> = HashMap::new();
    for (repository, _, lemma_spec_set) in spec_sets_in_order(context) {
        for spec in lemma_spec_set.iter_specs() {
            // Slice errors are reported when `plan` asks for the same slices.
            let Ok(version_slices) = interners.slices.of(context, repository, spec, limits) else {
                continue;
            };
            for start in version_slices.starts() {
                let identity = semantics::SpecIdentity::of(repository, spec, start.clone());
                if decisions.contains_key(&identity) {
                    continue;
                }
                let Some((candidate, edges)) = reuse_candidate(
                    context,
                    committed,
                    repository,
                    spec,
                    &identity,
                    &mut interners.slices,
                    limits,
                ) else {
                    decisions.insert(identity, false);
                    continue;
                };
                let mut stack = vec![ReuseFrame {
                    identity,
                    candidate,
                    edges,
                    next_edge: 0,
                }];
                while let Some(top) = stack.last() {
                    let Some(edge) = top.edges.get(top.next_edge).cloned() else {
                        let reused = stack.pop().expect("BUG: stack top was just read");
                        decisions.insert(reused.identity.clone(), true);
                        interners
                            .spec_templates
                            .insert(reused.identity, reused.candidate);
                        continue;
                    };
                    match decisions.get(&edge.identity) {
                        Some(true) => {
                            stack
                                .last_mut()
                                .expect("BUG: stack top was just read")
                                .next_edge += 1;
                            continue;
                        }
                        Some(false) => {
                            let rejected = stack.pop().expect("BUG: stack top was just read");
                            decisions.insert(rejected.identity, false);
                            continue;
                        }
                        None => {}
                    }
                    if stack.iter().any(|frame| frame.identity == edge.identity) {
                        let rejected = stack.pop().expect("BUG: stack top was just read");
                        decisions.insert(rejected.identity, false);
                        continue;
                    }
                    match reuse_candidate(
                        context,
                        committed,
                        &edge.repository,
                        edge.spec,
                        &edge.identity,
                        &mut interners.slices,
                        limits,
                    ) {
                        Some((candidate, edges)) => stack.push(ReuseFrame {
                            identity: edge.identity,
                            candidate,
                            edges,
                            next_edge: 0,
                        }),
                        None => {
                            decisions.insert(edge.identity, false);
                        }
                    }
                }
            }
        }
    }
}

/// The plan of the slice of `spec` that holds instant `at`, built at most once per pass.
///
/// Nested row templates and root temporal slices both come through here, so one slice
/// ([`SpecIdentity`]) has exactly one plan in a pass. The plan is built at the slice's
/// start: inside a slice every unpinned import resolves to the same version.
pub(crate) fn get_or_build_template(
    context: &Context,
    repository: &Arc<LemmaRepository>,
    spec: &LemmaSpec,
    at: &EffectiveDate,
    limits: &crate::limits::ResourceLimits,
    interners: &mut instance_list::PassInterners,
) -> Result<Arc<ExecutionPlan>, Vec<Error>> {
    let identity = interners
        .slices
        .identity_at(context, repository, spec, at, limits)?;
    if let Some(template) = interners.spec_templates.get(&identity) {
        return Ok(template);
    }
    let slice = identity.slice.clone();
    let lemma_spec_set = context
        .spec_set(repository, &spec.name)
        .expect("BUG: template spec missing from context");
    let dependencies = discovery::discover_dependency_order(
        context, repository, spec, &slice, limits,
    )
    .map_err(|error| match error {
        discovery::DependencyDiscoveryError::Cycle(errors)
        | discovery::DependencyDiscoveryError::Other(errors) => errors,
    })?;
    let (graph, resolved_types) = graph::Graph::build(
        context,
        repository,
        spec,
        &dependencies,
        &slice,
        limits,
        interners,
    )?;
    let edges = slice_edges(
        context,
        repository,
        spec,
        &slice,
        &mut interners.slices,
        limits,
    )
    .expect("BUG: edges of a slice whose dependencies were discovered and built resolve")
    .into_iter()
    .map(|edge| edge.identity)
    .collect();
    let mut plan = execution_plan::build_execution_plan(
        &graph,
        resolved_types,
        &slice,
        limits,
        &mut normalize::NormalFormInterner::new(),
        Arc::clone(lemma_spec_set.spec_arc(spec)),
        edges,
    )?;
    execution_plan::attach_show_cache(&mut plan, lemma_spec_set, spec);
    let template = Arc::new(plan);
    interners
        .spec_templates
        .insert(identity, Arc::clone(&template));
    Ok(template)
}

/// Build the execution plans of every spec set of `context`.
///
/// `committed` is read only to reuse plans: a slice [`seed_reusable_plans`] accepts keeps
/// its committed `Arc`. Every other slice is built. Neither the context nor `committed`
/// is mutated.
pub(crate) fn plan(
    context: &Context,
    limits: &crate::limits::ResourceLimits,
    committed: &PlanStore,
) -> PlanningResult {
    let mut plans = PlanStore::new();
    let mut errors = Vec::new();
    let mut failed_specs: HashSet<(Arc<LemmaRepository>, String, EffectiveDate)> = HashSet::new();
    let mut missing_repository_source_specs: HashSet<(
        Arc<LemmaRepository>,
        String,
        EffectiveDate,
    )> = HashSet::new();
    let mut interners = instance_list::PassInterners::default();
    seed_reusable_plans(context, committed, limits, &mut interners);

    for (repository, spec_name, lemma_spec_set) in spec_sets_in_order(context) {
        for spec in lemma_spec_set.iter_specs() {
            let mut slice_errors = Vec::new();
            let mut slice_plans = Vec::new();
            let breakpoints = match interners.slices.of(context, repository, spec, limits) {
                Ok(slices) => slices.starts().to_vec(),
                Err(plan_errors) => {
                    slice_errors.extend(plan_errors);
                    Vec::new()
                }
            };
            for effective in breakpoints {
                match get_or_build_template(
                    context,
                    repository,
                    spec,
                    &effective,
                    limits,
                    &mut interners,
                ) {
                    Ok(plan) => slice_plans.push(plan),
                    Err(build_errors) => slice_errors.extend(build_errors),
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

            for execution_plan in slice_plans {
                plans.insert_plan(repository.name.as_deref(), spec_name, execution_plan);
            }
        }
    }

    for (_, _, consumer_spec, err) in discovery::validate_dependency_interfaces(
        context,
        &plans,
        &missing_repository_source_specs,
        &failed_specs,
    ) {
        errors.push(err.with_spec_context(consumer_spec));
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
    use super::{Arc, EffectiveDate};
    use crate::parsing::ast::{LemmaSpec, Span};
    use crate::parsing::source::{Source, SourceType};
    use crate::Error;
    use indexmap::IndexMap;

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
            .evaluate(None, spec_name, Some(at), IndexMap::new(), None, false)
            .expect("evaluation must succeed")
            .results
            .get(rule_name)
            .expect("rule must be in results")
            .result()
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

    // --- Incremental replanning: reuse, transactionality ---

    use crate::planning::normalize::NormalForm;
    use crate::planning::ExecutionPlan;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn path_source(name: &str) -> SourceType {
        SourceType::Path(Arc::new(PathBuf::from(name)))
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

    /// An edit to the base of a diamond must replan both arms and the root, and the root
    /// must observe the new value.
    #[test]
    fn editing_a_diamond_dependency_replans_both_arms_and_root() {
        let mut engine = crate::Engine::new();
        engine
            .load([
                (
                    path_source("base.lemma"),
                    "spec base\ndata v: 1\nrule r: v\n",
                ),
                (
                    path_source("left.lemma"),
                    "spec left\nuses b: base\nrule r: b.r\n",
                ),
                (
                    path_source("right.lemma"),
                    "spec right\nuses b: base\nrule r: b.r\n",
                ),
                (
                    path_source("root.lemma"),
                    "spec root\nuses l: left\nuses r_dep: right\nrule out: l.r + r_dep.r\n",
                ),
            ])
            .expect("initial load");

        let left_before = plan_buffers(&engine, None, "left");
        let right_before = plan_buffers(&engine, None, "right");
        let root_before = plan_buffers(&engine, None, "root");

        engine
            .update(
                None,
                "spec base\ndata v: 5\nrule r: v\n".to_string(),
                path_source("base.lemma"),
            )
            .expect("base edit applies");

        assert_ne!(plan_buffers(&engine, None, "left"), left_before);
        assert_ne!(plan_buffers(&engine, None, "right"), right_before);
        assert_ne!(plan_buffers(&engine, None, "root"), root_before);
        assert_eq!(
            run_display(&engine, "root", &date(2025, 1, 1), "out"),
            "10",
            "the root must observe both arms of the diamond after the base edit"
        );
    }

    /// A dependency cycle must fail planning with an error and must not diverge.
    #[test]
    fn dependency_cycle_fails_planning_and_terminates() {
        let mut engine = crate::Engine::new();
        engine
            .load([(
                path_source("cycle.lemma"),
                "spec a\nuses b_dep: b\ndata amount: number\n\nspec b\nuses a_dep: a\ndata imported: a_dep.amount\n",
            )])
            .expect_err("a dependency cycle must fail to load");
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

    /// Committed slice plans of a spec set, by slice start, as `Arc` addresses.
    fn slice_arcs(
        engine: &crate::Engine,
        spec_name: &str,
    ) -> BTreeMap<EffectiveDate, *const ExecutionPlan> {
        engine
            .plans
            .get_plans(None, spec_name)
            .unwrap_or_else(|| panic!("plans for spec '{spec_name}'"))
            .iter()
            .map(|(slice, plan)| (slice.clone(), Arc::as_ptr(plan)))
            .collect()
    }

    fn slice_start(year: i32, month: u32, day: u32) -> EffectiveDate {
        EffectiveDate::DateTimeValue(date(year, month, day))
    }

    /// A second load that leaves a spec set and its closure unchanged keeps every committed
    /// plan, and every template a consumer holds is the dependency's committed slice.
    #[test]
    fn a_second_load_reuses_every_unchanged_plan() {
        let mut engine = crate::Engine::new();
        engine
            .load([
                (
                    path_source("leaf.lemma"),
                    "spec leaf\ndata v: 1\nrule r: v\n\nspec leaf 2025-01-01\ndata v: 2\nrule r: v\n",
                ),
                (
                    path_source("mid.lemma"),
                    "spec mid\nuses l: leaf\nrule r: l.r\n",
                ),
                (
                    path_source("root.lemma"),
                    "spec root\nuses m: mid\nrule out: m.r\n",
                ),
            ])
            .expect("initial load");
        let before: Vec<_> = ["leaf", "mid", "root"]
            .iter()
            .map(|name| slice_arcs(&engine, name))
            .collect();

        engine
            .load([(
                path_source("other.lemma"),
                "spec other\ndata v: 3\nrule r: v\n",
            )])
            .expect("second load");

        let after: Vec<_> = ["leaf", "mid", "root"]
            .iter()
            .map(|name| slice_arcs(&engine, name))
            .collect();
        assert_eq!(
            after, before,
            "unchanged spec sets keep their committed Arcs"
        );

        for plan in engine
            .plans
            .get_plans(None, "root")
            .expect("root plans")
            .values()
        {
            for (identity, template) in &plan.row_templates {
                let committed = engine
                    .plans
                    .get_plans(identity.repository.as_deref(), &identity.spec)
                    .and_then(|slices| slices.get(&identity.slice))
                    .unwrap_or_else(|| panic!("committed slice for template {identity}"));
                assert!(
                    Arc::ptr_eq(committed, template),
                    "template {identity} is the committed slice plan"
                );
            }
        }
    }

    /// Adding a dependency version ends the previous version earlier, so the consumer slice
    /// over that version is replanned; the slice over an older, untouched version is reused.
    #[test]
    fn a_new_dependency_version_keeps_consumer_slices_over_untouched_versions() {
        let mut engine = crate::Engine::new();
        engine
            .load([
                (
                    path_source("dep.lemma"),
                    "spec dep 2024-01-01\ndata v: 1\nrule r: v\n\nspec dep 2025-01-01\ndata v: 2\nrule r: v\n",
                ),
                (
                    path_source("consumer.lemma"),
                    "spec consumer 2024-01-01\nuses d: dep\nrule out: d.r\n",
                ),
            ])
            .expect("initial load");
        let before = slice_arcs(&engine, "consumer");
        assert_eq!(before.len(), 2, "one consumer slice per dependency version");

        engine
            .load([(
                path_source("dep_2026.lemma"),
                "spec dep 2026-01-01\ndata v: 3\nrule r: v\n",
            )])
            .expect("new dependency version loads");

        let after = slice_arcs(&engine, "consumer");
        assert_eq!(after.len(), 3, "the new version adds a consumer slice");
        assert_eq!(
            after[&slice_start(2024, 1, 1)],
            before[&slice_start(2024, 1, 1)],
            "the slice over `dep` 2024, whose window did not change, is reused"
        );
        assert_ne!(
            after[&slice_start(2025, 1, 1)],
            before[&slice_start(2025, 1, 1)],
            "the slice over `dep` 2025, which now ends at 2026, is replanned"
        );
        assert_eq!(
            run_display(&engine, "consumer", &date(2024, 6, 1), "out"),
            "1"
        );
        assert_eq!(
            run_display(&engine, "consumer", &date(2025, 6, 1), "out"),
            "2"
        );
        assert_eq!(
            run_display(&engine, "consumer", &date(2026, 6, 1), "out"),
            "3"
        );
    }

    /// A pinned import reads one dependency version; a new version after it changes nothing
    /// the consumer reads, so the consumer is reused.
    #[test]
    fn a_pinned_import_keeps_its_plan_when_a_later_dependency_version_arrives() {
        let mut engine = crate::Engine::new();
        engine
            .load([
                (
                    path_source("dep.lemma"),
                    "spec dep 2024-01-01\ndata v: 1\nrule r: v\n\nspec dep 2025-01-01\ndata v: 2\nrule r: v\n",
                ),
                (
                    path_source("consumer.lemma"),
                    "spec consumer\nuses d: dep 2024-06-01\nrule out: d.r\n",
                ),
            ])
            .expect("initial load");
        let before = slice_arcs(&engine, "consumer");

        engine
            .load([(
                path_source("dep_2026.lemma"),
                "spec dep 2026-01-01\ndata v: 3\nrule r: v\n",
            )])
            .expect("new dependency version loads");

        assert_eq!(
            slice_arcs(&engine, "consumer"),
            before,
            "the pinned consumer reads `dep` 2024 only and is reused"
        );
        assert_eq!(
            run_display(&engine, "consumer", &date(2026, 6, 1), "out"),
            "1"
        );
    }

    /// A version inserted at an old slice start of its own spec takes that slice: the
    /// committed plan there belongs to the earlier version and must not be reused.
    #[test]
    fn a_version_inserted_at_an_old_slice_start_is_planned_fresh() {
        let mut engine = crate::Engine::new();
        engine
            .load([
                (
                    path_source("dep.lemma"),
                    "spec dep\ndata v: 1\nrule r: v\n\nspec dep 2025-01-01\ndata v: 2\nrule r: v\n",
                ),
                (
                    path_source("consumer.lemma"),
                    "spec consumer\nuses d: dep\nrule out: d.r\n",
                ),
            ])
            .expect("initial load");
        assert_eq!(slice_count(&engine, "consumer"), 2);

        engine
            .load([(
                path_source("consumer_2025.lemma"),
                "spec consumer 2025-01-01\nuses d: dep\nrule out: d.r * 10\n",
            )])
            .expect("new consumer version loads");

        let slices = engine
            .plans
            .get_plans(None, "consumer")
            .expect("consumer plans");
        let at_2025 = &slices[&slice_start(2025, 1, 1)];
        assert_eq!(
            at_2025.identity.effective,
            slice_start(2025, 1, 1),
            "the 2025 slice is the new version's"
        );
        assert_eq!(
            run_display(&engine, "consumer", &date(2025, 6, 1), "out"),
            "20"
        );
        assert_eq!(
            run_display(&engine, "consumer", &date(2024, 6, 1), "out"),
            "1"
        );
    }

    /// Moving a spec in its source moves its spans, which are part of its own bytes.
    #[test]
    fn moving_a_spec_in_its_source_replans_it() {
        let mut engine = crate::Engine::new();
        engine
            .load([(
                path_source("solo.lemma"),
                "spec solo\ndata v: 1\nrule r: v\n",
            )])
            .expect("initial load");
        let before = slice_arcs(&engine, "solo");

        engine
            .update(
                None,
                "\n\nspec solo\ndata v: 1\nrule r: v\n".to_string(),
                path_source("solo.lemma"),
            )
            .expect("moved spec applies");

        assert_ne!(slice_arcs(&engine, "solo"), before);
        assert_eq!(
            engine
                .plans
                .get_plans(None, "solo")
                .expect("solo plans")
                .values()
                .next()
                .expect("one slice")
                .start_line,
            3
        );
    }

    /// Every plan the engine keeps alive: the committed slices and every template they
    /// carry, transitively, each once.
    fn held_plans(engine: &crate::Engine) -> Vec<Arc<ExecutionPlan>> {
        let mut stack: Vec<Arc<ExecutionPlan>> = engine
            .plans
            .plans
            .values()
            .flat_map(|by_spec| by_spec.values())
            .flat_map(|slices| slices.values())
            .cloned()
            .collect();
        let mut held: Vec<Arc<ExecutionPlan>> = Vec::new();
        while let Some(plan) = stack.pop() {
            if held.iter().any(|kept| Arc::ptr_eq(kept, &plan)) {
                continue;
            }
            stack.extend(plan.row_templates.values().cloned());
            held.push(plan);
        }
        held
    }

    fn committed_plan_of(
        engine: &crate::Engine,
        identity: &super::semantics::SpecIdentity,
    ) -> Option<Arc<ExecutionPlan>> {
        engine
            .plans
            .get_plans(identity.repository.as_deref(), &identity.spec)
            .and_then(|slices| slices.get(&identity.slice))
            .cloned()
    }

    /// No plan outlives the pass that replaced it: every template a committed plan holds
    /// is the committed plan of its slice.
    fn assert_every_template_is_committed(engine: &crate::Engine, step: &str) {
        for plan in held_plans(engine) {
            let committed = committed_plan_of(engine, &plan.identity).unwrap_or_else(|| {
                panic!(
                    "{step}: plan of {} is held but no longer committed",
                    plan.identity
                )
            });
            assert!(
                Arc::ptr_eq(&committed, &plan),
                "{step}: a stale plan of {} is held beside the committed one",
                plan.identity
            );
        }
    }

    /// Weak handles to every plan the engine holds now.
    fn weak_plans(engine: &crate::Engine) -> Vec<(String, std::sync::Weak<ExecutionPlan>)> {
        held_plans(engine)
            .iter()
            .map(|plan| (plan.identity.to_string(), Arc::downgrade(plan)))
            .collect()
    }

    /// Every plan held before a step is either still committed or freed.
    fn assert_replaced_plans_freed(
        engine: &crate::Engine,
        before: &[(String, std::sync::Weak<ExecutionPlan>)],
        step: &str,
    ) {
        let held = held_plans(engine);
        for (identity, weak) in before {
            if let Some(plan) = weak.upgrade() {
                assert!(
                    held.iter().any(|kept| Arc::ptr_eq(kept, &plan)),
                    "{step}: replaced plan of {identity} is still alive outside the engine's plans"
                );
            }
        }
    }

    const RETENTION_LEAF: &str =
        "spec leaf\ndata v: 1\nrule r: v\n\nspec leaf 2025-01-01\ndata v: 2\nrule r: v\n";
    const RETENTION_ITEM: &str =
        "spec item\nuses l: leaf\ndata amount: number\nrule total: amount * l.r\n";
    const RETENTION_BOX: &str =
        "spec box\nuses item\ndata items: item list\nrule item_count: items -> count\n";
    const RETENTION_ROOT: &str = "spec root\nuses b: box\nuses i: item\n  -> with amount: 3\nrule out: i.total\nrule boxed: b.item_count\n";
    const RETENTION_PINNED: &str = "spec pinned\nuses l: leaf 2024-06-01\nrule out: l.r\n";

    fn retention_engine() -> crate::Engine {
        let mut engine = crate::Engine::new();
        engine
            .load([
                (path_source("leaf.lemma"), RETENTION_LEAF),
                (path_source("item.lemma"), RETENTION_ITEM),
                (path_source("box.lemma"), RETENTION_BOX),
                (path_source("root.lemma"), RETENTION_ROOT),
                (path_source("pinned.lemma"), RETENTION_PINNED),
            ])
            .expect("retention workspace loads");
        assert_every_template_is_committed(&engine, "initial load");
        engine
    }

    /// Apply `step` and check that nothing the engine held before outlives it unless it
    /// is still committed, and that every held template is the committed slice.
    fn replan_step(engine: &mut crate::Engine, step: &str, apply: impl FnOnce(&mut crate::Engine)) {
        let before = weak_plans(engine);
        apply(engine);
        assert_replaced_plans_freed(engine, &before, step);
        assert_every_template_is_committed(engine, step);
    }

    /// A sequence of edits, additions, removals and a failed batch over a workspace
    /// with nested templates (`root` holds `box`, `box` holds `item`,
    /// `item` holds `leaf`) and a pinned import. After every step, no replaced plan may
    /// survive and no held template may differ from its committed slice.
    #[test]
    fn replanning_frees_every_replaced_plan() {
        let mut engine = retention_engine();

        replan_step(&mut engine, "edit leaf", |engine| {
            engine
                .update(
                    None,
                    RETENTION_LEAF.replace("data v: 2", "data v: 5"),
                    path_source("leaf.lemma"),
                )
                .expect("leaf edit applies");
        });
        assert_eq!(run_display(&engine, "root", &date(2025, 6, 1), "out"), "15");

        replan_step(&mut engine, "edit leaf back", |engine| {
            engine
                .update(None, RETENTION_LEAF.to_string(), path_source("leaf.lemma"))
                .expect("leaf revert applies");
        });

        replan_step(&mut engine, "re-parse unchanged item", |engine| {
            engine
                .update(None, RETENTION_ITEM.to_string(), path_source("item.lemma"))
                .expect("unchanged item applies");
        });

        replan_step(&mut engine, "add leaf version", |engine| {
            engine
                .load([(
                    path_source("leaf_2027.lemma"),
                    "spec leaf 2027-01-01\ndata v: 7\nrule r: v\n",
                )])
                .expect("leaf version loads");
        });
        assert_eq!(run_display(&engine, "root", &date(2027, 6, 1), "out"), "21");

        replan_step(&mut engine, "remove leaf version", |engine| {
            engine
                .remove(None, "leaf", Some(&date(2027, 6, 1)))
                .expect("leaf version removes");
        });

        replan_step(&mut engine, "failed batch", |engine| {
            engine
                .update(
                    None,
                    "spec item\nuses l: leaf\ndata amount: number\nrule total: amount * l.missing\n"
                        .to_string(),
                    path_source("item.lemma"),
                )
                .expect_err("an item reading a missing rule fails to plan");
        });

        replan_step(&mut engine, "remove root", |engine| {
            engine.remove(None, "root", None).expect("root removes");
        });
        assert!(engine.plans.get_plans(None, "root").is_none());

        replan_step(&mut engine, "edit leaf after removals", |engine| {
            engine
                .update(
                    None,
                    RETENTION_LEAF.replace("data v: 1", "data v: 4"),
                    path_source("leaf.lemma"),
                )
                .expect("leaf edit applies");
        });
        replan_step(&mut engine, "re-parse unchanged box again", |engine| {
            engine
                .update(None, RETENTION_BOX.to_string(), path_source("box.lemma"))
                .expect("unchanged box applies");
        });
    }

    /// Every spec version in the context with its spec `Arc`.
    fn live_spec_arcs(engine: &crate::Engine) -> Vec<(String, Arc<LemmaSpec>)> {
        let mut live = Vec::new();
        for (repository, by_name) in engine.context.repositories() {
            for spec_set in by_name.values() {
                for spec in spec_set.iter_specs() {
                    live.push((
                        format!(
                            "{:?} {} {:?}",
                            repository.name, spec.name, spec.effective_from
                        ),
                        Arc::clone(spec_set.spec_arc(spec)),
                    ));
                }
            }
        }
        live
    }

    /// Replanning the same specs a few times must not keep the old specs alive: once a
    /// step replaces a spec version, the version it replaced is freed.
    #[test]
    fn replanning_drops_the_specs_it_replaces() {
        const BROKEN_ITEM: &str =
            "spec item\nuses l: leaf\ndata amount: number\nrule total: amount * l.missing\n";
        let mut engine = retention_engine();
        type ReplanStep = (&'static str, fn(&mut crate::Engine));
        let steps: [ReplanStep; 5] = [
            ("re-parse unchanged item", |engine| {
                engine
                    .update(None, RETENTION_ITEM.to_string(), path_source("item.lemma"))
                    .expect("unchanged item applies");
            }),
            ("edit leaf", |engine| {
                engine
                    .update(
                        None,
                        RETENTION_LEAF.replace("data v: 2", "data v: 5"),
                        path_source("leaf.lemma"),
                    )
                    .expect("leaf edit applies");
            }),
            ("revert leaf", |engine| {
                engine
                    .update(None, RETENTION_LEAF.to_string(), path_source("leaf.lemma"))
                    .expect("leaf revert applies");
            }),
            ("failed item batch", |engine| {
                engine
                    .update(None, BROKEN_ITEM.to_string(), path_source("item.lemma"))
                    .expect_err("an item reading a missing rule fails to plan");
            }),
            ("re-parse unchanged item again", |engine| {
                engine
                    .update(None, RETENTION_ITEM.to_string(), path_source("item.lemma"))
                    .expect("unchanged item applies");
            }),
        ];

        let mut seen: Vec<(String, std::sync::Weak<LemmaSpec>)> = Vec::new();
        let mut failures = Vec::new();
        for (step, apply) in steps {
            for (version, spec_arc) in live_spec_arcs(&engine) {
                let weak = Arc::downgrade(&spec_arc);
                if !seen.iter().any(|(_, known)| known.ptr_eq(&weak)) {
                    seen.push((version, weak));
                }
            }
            apply(&mut engine);
            let live = live_spec_arcs(&engine);
            for (version, weak) in &seen {
                let Some(spec_arc) = weak.upgrade() else {
                    continue;
                };
                if !live
                    .iter()
                    .any(|(_, current)| Arc::ptr_eq(current, &spec_arc))
                {
                    failures.push(format!("after {step}: old {version}"));
                }
            }
        }
        assert!(
            failures.is_empty(),
            "replanning kept replaced specs alive:\n{}",
            failures.join("\n")
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
        // breaks the consumer. Both spec sets are planned again during the failed pass.
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
    /// and leave both spec sets' plans in place. The removed spec set is no longer in the
    /// context while its consumer still imports it.
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
    /// it would panic inside [`discovery::validate_dependency_interfaces`] instead of applying.
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
    /// context from scratch. This is the guarantee that makes reusing committed plans safe.
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
    /// dependence leaks in through reused plans.
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

    /// Adding a temporal version rebuilds only the slices whose planning input changed:
    /// the new version, and the neighbor whose `effective_to` moved to the new boundary.
    #[test]
    fn version_add_rebuilds_only_neighbor_with_moved_boundary() {
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
        assert_eq!(
            after[0], before[0],
            "origin slice keeps its range and body, so its plan must be reused"
        );
        assert_ne!(
            after[1], before[1],
            "mid slice's effective_to moved to the new version, so it must be rebuilt"
        );
    }
}

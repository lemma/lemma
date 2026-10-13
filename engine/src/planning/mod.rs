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
pub mod typing;
pub mod unit_family;
pub mod unit_index;
use crate::engine::Context;
use crate::parsing::ast::{DateTimeValue, EffectiveDate, LemmaRepository, LemmaSpec};
use crate::Error;
pub use execution_plan::ExecutionPlan;
use indexmap::IndexMap;
use semantics::SpecIdentity;
use serde::{Deserialize, Serialize};
pub use spec_set::LemmaSpecSet;
use std::collections::{BTreeMap, HashSet};
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

/// Temporal slices of one spec, keyed by [`ExecutionPlan::effective`].
pub(crate) type PlanSlices = BTreeMap<EffectiveDate, Arc<ExecutionPlan>>;

/// Compiled plans keyed by repository name (`None` = default unnamed repository) then spec name.
///
/// Temporal slices per spec are a [`BTreeMap`] keyed by [`ExecutionPlan::effective`].
/// Values are [`Arc`] so a planning pass can intern the same plan as a named-`uses`
/// / row template without a second deep clone.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct PlanStore {
    #[serde(with = "plan_store_plans")]
    plans: IndexMap<Option<String>, IndexMap<String, PlanSlices>>,
}

/// Snapshot form of [`PlanStore::plans`]: every plan slice keeps its `Arc` identity.
mod plan_store_plans {
    use super::PlanSlices;
    use crate::snapshot::shared::arc_map;
    use indexmap::IndexMap;
    use serde::{Deserialize, Deserializer, Serializer};

    type Plans = IndexMap<Option<String>, IndexMap<String, PlanSlices>>;

    struct SlicesRef<'a>(&'a PlanSlices);

    impl serde::Serialize for SlicesRef<'_> {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            arc_map::serialize(self.0, serializer)
        }
    }

    struct SlicesOwned(PlanSlices);

    impl<'de> Deserialize<'de> for SlicesOwned {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            arc_map::deserialize(deserializer).map(SlicesOwned)
        }
    }

    struct SpecsRef<'a>(&'a IndexMap<String, PlanSlices>);

    impl serde::Serialize for SpecsRef<'_> {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.collect_seq(
                self.0
                    .iter()
                    .map(|(spec, slices)| (spec, SlicesRef(slices))),
            )
        }
    }

    pub(super) fn serialize<S: Serializer>(
        plans: &Plans,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(
            plans
                .iter()
                .map(|(repository, specs)| (repository, SpecsRef(specs))),
        )
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Plans, D::Error> {
        let raw = Vec::<(Option<String>, Vec<(String, SlicesOwned)>)>::deserialize(deserializer)?;
        let mut plans = IndexMap::with_capacity(raw.len());
        for (repository, specs) in raw {
            let mut by_spec = IndexMap::with_capacity(specs.len());
            for (spec, slices) in specs {
                if by_spec.insert(spec, slices.0).is_some() {
                    return Err(serde::de::Error::custom(
                        "engine snapshot repeats a spec name",
                    ));
                }
            }
            if plans.insert(repository, by_spec).is_some() {
                return Err(serde::de::Error::custom(
                    "engine snapshot repeats a repository",
                ));
            }
        }
        Ok(plans)
    }
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

/// Every spec set of `context` with its identity, ordered by [`SpecSetKey`], so a pass
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
    sets.sort_by_key(|(repository, name, _)| SpecSetKey::new(repository.name.as_deref(), name));
    sets
}

/// Canonical bytes of everything the plans of one temporal version are built from: the
/// version with its effective boundaries, and every spec set reachable from it through
/// dependency edges, each with all of its versions and boundaries.
///
/// Equal bytes mean equal plans, so a committed slice with equal bytes is reused as is.
/// Bytes are compared directly and never hashed. `start_line` and spans are part of the
/// bytes, so moving a version in its source rebuilds its slice.
pub(crate) fn planning_input(
    context: &Context,
    repository: &Arc<LemmaRepository>,
    spec: &LemmaSpec,
) -> Result<Arc<Vec<u8>>, Vec<Error>> {
    #[derive(Serialize)]
    struct VersionInput<'a> {
        spec: &'a LemmaSpec,
        boundaries: (Option<DateTimeValue>, Option<DateTimeValue>),
    }

    #[derive(Serialize)]
    struct DependencySetInput<'a> {
        repository: &'a LemmaRepository,
        name: &'a str,
        versions: Option<Vec<VersionInput<'a>>>,
    }

    #[derive(Serialize)]
    struct PlanningInput<'a> {
        repository: &'a LemmaRepository,
        version: VersionInput<'a>,
        dependencies: Vec<DependencySetInput<'a>>,
    }

    let lemma_spec_set = context
        .spec_set(repository, &spec.name)
        .expect("BUG: planning input requested for a spec missing from context");

    let mut reached: BTreeMap<SpecSetKey, (Arc<LemmaRepository>, String, Option<&LemmaSpecSet>)> =
        BTreeMap::new();
    let mut frontier: Vec<(Arc<LemmaRepository>, &LemmaSpec)> =
        vec![(Arc::clone(repository), spec)];
    while let Some((owner, version)) = frontier.pop() {
        for edge in discovery::dependency_edges(version, &owner, context)? {
            let key = SpecSetKey::new(edge.dep_repository.name.as_deref(), &edge.dep_name);
            if reached.contains_key(&key) {
                continue;
            }
            let dep_set = context.spec_set(&edge.dep_repository, &edge.dep_name);
            if let Some(dep_set) = dep_set {
                for dep_version in dep_set.iter_specs() {
                    frontier.push((Arc::clone(&edge.dep_repository), dep_version));
                }
            }
            reached.insert(
                key,
                (Arc::clone(&edge.dep_repository), edge.dep_name, dep_set),
            );
        }
    }

    let dependencies = reached
        .values()
        .map(|(dep_repository, name, dep_set)| DependencySetInput {
            repository: dep_repository.as_ref(),
            name,
            versions: dep_set.map(|dep_set| {
                dep_set
                    .iter_with_ranges()
                    .map(|(spec, from, to)| VersionInput {
                        spec,
                        boundaries: (from, to),
                    })
                    .collect()
            }),
        })
        .collect();
    let input = PlanningInput {
        repository: repository.as_ref(),
        version: VersionInput {
            spec,
            boundaries: lemma_spec_set.effective_range(spec),
        },
        dependencies,
    };
    Ok(Arc::new(
        postcard::to_allocvec(&input).expect("BUG: planning input serializes"),
    ))
}

/// The template plan of `spec` at instant `at`, built at most once per pass.
///
/// Nested row templates and root temporal slices both come through here, so one
/// `(identity, instant)` has exactly one plan in a pass.
pub(crate) fn get_or_build_template(
    context: &Context,
    repository: &Arc<LemmaRepository>,
    spec: &LemmaSpec,
    at: &EffectiveDate,
    limits: &crate::limits::ResourceLimits,
    interners: &mut instance_list::PassInterners,
    planning_input: &Arc<Vec<u8>>,
) -> Result<Arc<ExecutionPlan>, Vec<Error>> {
    let identity = SpecIdentity::of(repository, spec);
    if let Some(template) = interners.spec_templates.get(&identity, at) {
        return Ok(template);
    }
    let lemma_spec_set = context
        .spec_set(repository, &spec.name)
        .expect("BUG: template spec missing from context");
    let dependencies = discovery::discover_dependency_order(context, spec, at, limits).map_err(
        |error| match error {
            discovery::DependencyDiscoveryError::Cycle(errors)
            | discovery::DependencyDiscoveryError::Other(errors) => errors,
        },
    )?;
    let (graph, resolved_types) = graph::Graph::build(
        context,
        repository,
        spec,
        &dependencies,
        at,
        limits,
        interners,
    )?;
    let mut plan = execution_plan::build_execution_plan(
        &graph,
        resolved_types,
        at,
        limits,
        &mut normalize::NormalFormInterner::new(),
        Arc::clone(planning_input),
    )?;
    execution_plan::attach_show_cache(&mut plan, lemma_spec_set, spec);
    let template = Arc::new(plan);
    interners
        .spec_templates
        .insert(identity, at, Arc::clone(&template));
    Ok(template)
}

/// Committed plans of the temporal version `spec` of `repository`, when every one of them
/// was built from `planning_input`. Empty when the version must be planned again.
fn reusable_version_plans(
    committed: &PlanStore,
    repository: &Arc<LemmaRepository>,
    spec: &LemmaSpec,
    planning_input: &Arc<Vec<u8>>,
) -> Vec<Arc<ExecutionPlan>> {
    let Some(slices) = committed.get_plans(repository.name.as_deref(), &spec.name) else {
        return Vec::new();
    };
    let version_plans: Vec<Arc<ExecutionPlan>> = slices
        .values()
        .filter(|plan| plan.identity.effective == spec.effective_from)
        .cloned()
        .collect();
    let unchanged = !version_plans.is_empty()
        && version_plans
            .iter()
            .all(|plan| plan.planning_input == *planning_input);
    if unchanged {
        version_plans
    } else {
        Vec::new()
    }
}

/// Build the execution plans of every spec set of `context`.
///
/// `committed` is read only to reuse plans: a temporal version whose planning input
/// bytes equal the committed ones keeps its committed `Arc`s. Every other version is
/// built. Neither the context nor `committed` is mutated.
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

    for (repository, spec_name, lemma_spec_set) in spec_sets_in_order(context) {
        for spec in lemma_spec_set.iter_specs() {
            let input = match planning_input(context, repository, spec) {
                Ok(input) => input,
                Err(input_errors) => {
                    errors.extend(
                        input_errors
                            .into_iter()
                            .map(|err| err.with_spec_context(spec)),
                    );
                    failed_specs.insert((
                        Arc::clone(repository),
                        spec_name.clone(),
                        spec.effective_from.clone(),
                    ));
                    continue;
                }
            };

            let reused = reusable_version_plans(committed, repository, spec, &input);
            if !reused.is_empty() {
                for plan in reused {
                    plans.insert_plan(repository.name.as_deref(), spec_name, plan);
                }
                continue;
            }

            let mut slice_errors = Vec::new();
            let mut slice_plans = Vec::new();
            let breakpoints =
                match discovery::plan_breakpoints(context, lemma_spec_set, spec, limits) {
                    Ok(dates) => dates,
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
                    &input,
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

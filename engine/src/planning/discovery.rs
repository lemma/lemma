use crate::engine::Context;
use crate::parsing::ast::{
    DataValue, DateTimeValue, EffectiveDate, Expression, ExpressionKind, LemmaRepository,
    LemmaSpec, ParentType, Reference, RepositoryQualifier, SpecRef, WithRhs,
};
use crate::parsing::source::Source;
use crate::planning::semantics::{DataDefinition, LemmaType};
use crate::Error;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::Arc;

/// A spec together with its owning repository, as produced by dependency discovery.
/// Spec is borrowed from Context storage for the duration of `plan`.
#[derive(Debug)]
pub(crate) struct DependencySpec<'a> {
    pub repository: Arc<LemmaRepository>,
    pub spec: &'a LemmaSpec,
}

/// Same Context-owned row (planning identity within one `plan` pass).
#[inline]
pub(crate) fn same_loaded_spec(a: &LemmaSpec, b: &LemmaSpec) -> bool {
    std::ptr::eq(a, b)
}

// ---------------------------------------------------------------------------
// Two-stage spec reference resolution
// ---------------------------------------------------------------------------

/// Planning-internal resolved spec reference. [`ResolvedSpecRef::repository`] is `None`
/// for same-repository references (resolved against the consumer spec's owning repository).
/// When `Some`, the arc is the [`LemmaRepository`] for an explicit repository
/// qualifier target, resolved against the context's interned repositories.
#[derive(Debug, Clone)]
pub(crate) struct ResolvedSpecRef {
    pub repository: Option<Arc<LemmaRepository>>,
    pub name: String,
    pub effective: Option<DateTimeValue>,
}

/// Expands same-spec [`DataValue::Import`] (`uses`) aliases for bare names, then resolves
/// a parsed [`SpecRef`] against the active [`Context`].
///
/// Same-repository references (no explicit repository qualifier) leave
/// [`ResolvedSpecRef::repository`] as `None`. Explicit qualifiers resolve to interned
/// repositories as `Some(...)`.
///
/// When a bare name matches a top-level [`DataValue::Import`] (`uses` alias), the returned
/// [`SpecRef`] (second tuple element) is that import's target [`SpecRef`].
pub(crate) fn resolve_spec_ref_after_expanding_uses_aliases(
    context: &Context,
    spec_ref: &SpecRef,
    ref_source: Option<&Source>,
    consumer_name: &str,
    spec_context: Option<&LemmaSpec>,
) -> Result<(ResolvedSpecRef, SpecRef), Error> {
    let effective_ref = if spec_ref.repository.is_some() {
        spec_ref.clone()
    } else if let Some(consumer) = &spec_context {
        let mut expanded: Option<SpecRef> = None;
        for d in &consumer.data {
            if !d.reference.segments.is_empty() {
                continue;
            }
            if d.reference.name != spec_ref.name {
                continue;
            }
            let DataValue::Import {
                spec_ref: inner, ..
            } = &d.value
            else {
                continue;
            };
            if spec_ref.effective.is_some()
                && (inner.name != spec_ref.name
                    || inner.repository != spec_ref.repository
                    || inner.effective != spec_ref.effective)
            {
                return Err(Error::validation_with_context(
                    format!(
                        "This reference pins an effective date on `{}`, which is only a `uses` import alias in this spec. Put the effective date on the matching `uses` line (for example `uses {}: <target_spec> <datetime>`).",
                        spec_ref.name, spec_ref.name
                    ),
                    ref_source.cloned(),
                    None::<String>,
                    spec_context,
                    None,
                ));
            }
            expanded = Some(inner.clone());
            break;
        }
        expanded.unwrap_or_else(|| spec_ref.clone())
    } else {
        spec_ref.clone()
    };

    let resolved_repository = match &effective_ref.repository {
        None => None,
        Some(qualifier) => {
            let Some(arc) = context.find_repository(&qualifier.name) else {
                return Err(unknown_repository_qualifier_error(
                    qualifier,
                    &effective_ref,
                    ref_source,
                    consumer_name,
                    spec_context,
                ));
            };
            Some(arc)
        }
    };
    Ok((
        ResolvedSpecRef {
            repository: resolved_repository,
            name: effective_ref.name.clone(),
            effective: effective_ref.effective.clone(),
        },
        effective_ref,
    ))
}

fn unknown_repository_qualifier_error(
    qualifier: &RepositoryQualifier,
    spec_ref: &SpecRef,
    ref_source: Option<&Source>,
    consumer_name: &str,
    spec_context: Option<&LemmaSpec>,
) -> Error {
    let message = if qualifier.is_registry() {
        format!(
            "'{}' references '{}' from '{}', but repository '{}' is not loaded",
            consumer_name, spec_ref.name, qualifier.name, qualifier.name
        )
    } else {
        format!(
            "'{}' references '{}' from repository '{}', but that repository is not loaded",
            consumer_name, spec_ref.name, qualifier.name
        )
    };

    let suggestion = if qualifier.is_registry() {
        format!(
            "Run `lemma install {}` to download the repository.",
            qualifier.name
        )
    } else {
        format!(
            "Ensure the repository '{}' is included in the workspace. If '{}' is a local spec, remove the repository qualifier.",
            qualifier.name, spec_ref.name
        )
    };
    Error::missing_repository(
        message,
        ref_source.cloned(),
        qualifier.name.clone(),
        Some(suggestion),
        spec_context,
    )
}

fn consumer_identity(spec: &LemmaSpec) -> String {
    match &spec.effective_from {
        EffectiveDate::Origin => spec.name.clone(),
        EffectiveDate::DateTimeValue(datetime) => format!("{} {}", spec.name, datetime),
    }
}

/// Resolve a `SpecRef` to the owning repository and loaded `&LemmaSpec` at the
/// planning `effective`. Returns a [`crate::Error::MissingRepository`] when the
/// repository qualifier is not loaded, or another planning error when the reference
/// cannot be resolved.
pub(crate) fn resolve_spec_ref<'a>(
    context: &'a Context,
    spec_ref: &SpecRef,
    consumer_repository: &Arc<LemmaRepository>,
    consumer_spec: &LemmaSpec,
    effective: &EffectiveDate,
    ref_source: Option<Source>,
) -> Result<(Arc<LemmaRepository>, &'a LemmaSpec), Error> {
    let consumer_name = consumer_spec.name.as_str();
    let (resolved, effective_ref) = resolve_spec_ref_after_expanding_uses_aliases(
        context,
        spec_ref,
        ref_source.as_ref(),
        consumer_name,
        Some(consumer_spec),
    )?;
    let repository_arc = match &resolved.repository {
        Some(explicit) => Arc::clone(explicit),
        None => Arc::clone(consumer_repository),
    };
    let instant = effective_ref.at(effective);
    let Some(spec_set) = context.spec_set(&repository_arc, effective_ref.name.as_str()) else {
        let (message, suggestion) = format_missing_spec_ref(
            consumer_name,
            &repository_arc,
            effective_ref.name.as_str(),
            effective_ref.repository.as_ref(),
            &effective_ref.effective,
            &instant,
            context,
        );
        return Err(Error::validation_with_context(
            message,
            ref_source,
            Some(suggestion),
            Some(consumer_spec),
            None,
        ));
    };

    let spec = if let Some(pin) = effective_ref.effective.as_ref() {
        if let Some(exact) = spec_set.get_exact(Some(pin)) {
            Some(exact)
        } else {
            spec_set.spec_at(&EffectiveDate::DateTimeValue(pin.clone()))
        }
    } else {
        spec_set.spec_at(&instant)
    };

    let spec = spec.ok_or_else(|| {
        let (message, suggestion) = format_missing_spec_ref(
            consumer_name,
            &repository_arc,
            effective_ref.name.as_str(),
            effective_ref.repository.as_ref(),
            &effective_ref.effective,
            &instant,
            context,
        );
        Error::validation_with_context(
            message,
            ref_source.clone(),
            Some(suggestion),
            Some(consumer_spec),
            None,
        )
    })?;

    if same_loaded_spec(consumer_spec, spec) {
        return Err(Error::validation_with_context(
            format!(
                "spec '{}' cannot reference itself via '{}'",
                consumer_identity(consumer_spec),
                spec_ref
            ),
            ref_source,
            None::<String>,
            Some(consumer_spec),
            None,
        ));
    }

    Ok((repository_arc, spec))
}

fn format_dep_qualified_name(repository: &LemmaRepository, dep_name: &str) -> String {
    match &repository.name {
        Some(name) => format!("{dep_name} from {name}"),
        None => dep_name.to_string(),
    }
}

fn format_missing_spec_ref(
    consumer_name: &str,
    dep_repository: &Arc<LemmaRepository>,
    dep_name: &str,
    explicit_repository_qualifier: Option<&RepositoryQualifier>,
    qualified_at: &Option<DateTimeValue>,
    dep_effective: &EffectiveDate,
    context: &Context,
) -> (String, String) {
    let dep_qualified = format_dep_qualified_name(dep_repository, dep_name);
    if let Some(ref dt) = qualified_at {
        let message = format!(
            "'{}' references '{}' at {}, but no '{}' is active at that instant",
            consumer_name, dep_qualified, dt, dep_qualified
        );
        let suggestion = format!(
            "Add '{}' with effective_from on or before {}, or change the reference instant.",
            dep_qualified, dt
        );
        return if explicit_repository_qualifier.is_some_and(|q| q.is_registry()) {
            (
                message,
                format!(
                    "{} Or run `lemma install {}` to install it.",
                    suggestion,
                    explicit_repository_qualifier
                        .expect("BUG: checked above")
                        .name
                ),
            )
        } else {
            (message, suggestion)
        };
    }

    let dep_ss = context.spec_set(dep_repository, dep_name);
    let dep_exists = dep_ss.is_some_and(|ss| !ss.is_empty());

    if !dep_exists {
        let message = format!(
            "'{}' depends on '{}', but '{}' does not exist",
            consumer_name, dep_qualified, dep_qualified
        );
        let suggestion = if explicit_repository_qualifier.is_some_and(|q| q.is_registry()) {
            format!(
                "Run `lemma install --all` or `lemma install {}` to install this dependency.",
                explicit_repository_qualifier
                    .expect("BUG: checked above")
                    .name
            )
        } else {
            format!("Create a spec named '{}'.", dep_qualified)
        };
        return (message, suggestion);
    }

    let message = format!(
        "'{}' depends on '{}', but no '{}' is active at {}",
        consumer_name, dep_qualified, dep_qualified, dep_effective
    );
    let suggestion = format!(
        "Add '{}' with effective_from covering {}, or adjust effective_from on '{}'.",
        dep_qualified, dep_effective, consumer_name
    );
    (message, suggestion)
}

// ---------------------------------------------------------------------------
// Dependency edge extraction
// ---------------------------------------------------------------------------

/// One outgoing edge from a spec: `(dep_repository, dep_name, optional explicit
/// effective on reference, source location)`. `dep_repository` is the resolved repository
/// `Arc` — same-repository references inherit the consumer's repository here.
pub(crate) struct DependencyEdge {
    pub dep_repository: Arc<LemmaRepository>,
    pub dep_name: String,
    pub explicit_repository_qualifier: Option<RepositoryQualifier>,
    pub explicit_effective: Option<DateTimeValue>,
    pub source: Source,
    /// Name the consumer writes before `.` to reach the dependency.
    pub alias: String,
    pub origin: DependencyEdgeOrigin,
}

/// Which row of the consumer spec produced a [`DependencyEdge`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DependencyEdgeOrigin {
    /// `uses alias: dep`
    Uses,
    /// `data x: alias.type`
    QualifiedType,
}

impl DependencyEdge {
    pub(crate) fn as_spec_ref(&self) -> SpecRef {
        let mut spec_ref = match &self.explicit_repository_qualifier {
            Some(qualifier) => SpecRef::cross_repository(self.dep_name.clone(), qualifier.clone()),
            None => SpecRef::same_repository(self.dep_name.clone()),
        };
        spec_ref.effective = self.explicit_effective.clone();
        spec_ref
    }
}

pub(crate) fn dependency_edges(
    spec: &LemmaSpec,
    consumer_repository: &Arc<LemmaRepository>,
    context: &Context,
) -> Result<Vec<DependencyEdge>, Vec<Error>> {
    let mut out = Vec::new();
    let mut errors: Vec<Error> = Vec::new();

    let mut push_edge =
        |spec_ref: &SpecRef, source: &Source, alias: &str, origin: DependencyEdgeOrigin| {
            match resolve_spec_ref_after_expanding_uses_aliases(
                context,
                spec_ref,
                Some(source),
                spec.name.as_str(),
                Some(spec),
            ) {
                Ok((resolved, effective_ref)) => {
                    let dep_repository = match &resolved.repository {
                        Some(r) => Arc::clone(r),
                        None => Arc::clone(consumer_repository),
                    };
                    out.push(DependencyEdge {
                        dep_repository,
                        dep_name: resolved.name,
                        explicit_repository_qualifier: effective_ref.repository.clone(),
                        explicit_effective: resolved.effective,
                        source: source.clone(),
                        alias: alias.to_string(),
                        origin,
                    });
                }
                Err(e) => errors.push(e),
            }
        };

    for data in &spec.data {
        match &data.value {
            DataValue::Import { spec_ref, .. } => {
                push_edge(
                    spec_ref,
                    &data.source_location,
                    &data.reference.name,
                    DependencyEdgeOrigin::Uses,
                );
            }
            DataValue::Definition {
                base: Some(ParentType::Qualified { spec_alias, .. }),
                ..
            } => {
                push_edge(
                    &SpecRef::same_repository(spec_alias.clone()),
                    &data.source_location,
                    spec_alias,
                    DependencyEdgeOrigin::QualifiedType,
                );
            }
            _ => {}
        }
    }

    if errors.is_empty() {
        Ok(out)
    } else {
        Err(errors)
    }
}

// ---------------------------------------------------------------------------
// Temporal slice breakpoints
// ---------------------------------------------------------------------------

/// Compute the temporal slice boundaries for `spec` by walking its transitive name-level
/// dependency closure and collecting every version `effective_from` of every closure member.
///
/// # Soundness precondition
///
/// Breakpoints derived from the closure are sound because type and unit resolution in
/// `Graph::build` are closure-scoped: `TypeResolver` learns types only from specs passed in
/// `ordered_dependencies`, and `unit_index` comes from the resulting `ResolvedSpecTypes`. Any
/// change that widens type or unit resolution beyond the closure would invalidate these results.
///
/// # Seeding and pin pruning
///
/// The closure is seeded from `spec`'s own `uses` lines only — not from every version of its
/// name. The plan being built is for this one version, so only this version's imports matter.
/// Pinned edges (`uses dep 2025-06-01`) prune their entire subtree: the pin instant is fixed, so
/// nothing beneath it can shift as the consumer's evaluation instant moves.
///
/// # Missing dependencies and cycles
///
/// When a dependency name is not found in the context, it is silently skipped; the error will
/// surface per slice through `discover_dependency_order`. Cycles are terminated by the visited set
/// without producing an error here; `discover_dependency_order` reports them per slice.
pub(crate) fn plan_breakpoints(
    context: &Context,
    spec_set: &crate::planning::spec_set::LemmaSpecSet,
    spec: &LemmaSpec,
    limits: &crate::limits::ResourceLimits,
) -> Result<Vec<EffectiveDate>, Vec<Error>> {
    use crate::parsing::ast::ascii_lowercase_logical_name;

    let (from, to) = spec_set.effective_range(spec);
    let from_key = EffectiveDate::from_option(from);

    let mut candidate_dates: BTreeSet<EffectiveDate> = BTreeSet::new();
    candidate_dates.insert(spec.effective_from.clone());

    // (repository, canonical_name) pairs already admitted to the closure.
    // The root spec is pre-inserted so its other versions are not treated as closure members.
    let mut visited: HashSet<(Arc<LemmaRepository>, String)> = HashSet::new();
    visited.insert((
        Arc::clone(&spec_set.repository),
        ascii_lowercase_logical_name(spec.name.clone()),
    ));

    // Worklist entries: (repository, canonical_name, depth).
    let mut worklist: VecDeque<(Arc<LemmaRepository>, String, usize)> = VecDeque::new();

    // Seed using only this spec version's dependency edges.
    match dependency_edges(spec, &spec_set.repository, context) {
        Ok(edges) => {
            for edge in edges {
                if edge.explicit_effective.is_none() {
                    let canonical = ascii_lowercase_logical_name(edge.dep_name.clone());
                    worklist.push_back((edge.dep_repository, canonical, 1));
                }
                // Pinned edges (explicit_effective.is_some()) prune their entire subtree.
            }
        }
        Err(errors) => return Err(errors),
    }

    let mut closure_errors: Vec<Error> = Vec::new();

    while let Some((repository, canonical_name, depth)) = worklist.pop_front() {
        let identity = (Arc::clone(&repository), canonical_name.clone());
        if visited.contains(&identity) {
            continue;
        }

        if depth > limits.max_spec_dependency_depth {
            closure_errors.push(Error::resource_limit_exceeded(
                "max_spec_dependency_depth",
                limits.max_spec_dependency_depth.to_string(),
                depth.to_string(),
                format!(
                    "Spec '{}' exceeds the maximum dependency nesting depth; flatten the import chain",
                    canonical_name
                ),
                None,
                None,
                None,
            ));
            continue;
        }

        // visited.len() equals the number of already-admitted members (root + previously processed).
        // Refusing to admit the current member when this count already meets the limit
        // matches dfs_discover's `nodes.len() >= limits.max_dag_specs` check.
        if visited.len() >= limits.max_dag_specs {
            closure_errors.push(Error::resource_limit_exceeded(
                "max_dag_specs",
                limits.max_dag_specs.to_string(),
                visited.len().to_string(),
                format!(
                    "Dependency graph of the root spec grew past {} specs at '{}'; \
                     reduce the number of transitive imports",
                    limits.max_dag_specs, canonical_name
                ),
                None,
                None,
                None,
            ));
            continue;
        }

        visited.insert(identity);

        let member_spec_set = match context.spec_set(&repository, &canonical_name) {
            Some(ss) => ss,
            // Not loaded; per-slice discover_dependency_order will report the missing dependency.
            None => continue,
        };

        for version in member_spec_set.iter_specs() {
            candidate_dates.insert(version.effective_from.clone());

            match dependency_edges(version, &repository, context) {
                Ok(edges) => {
                    for edge in edges {
                        if edge.explicit_effective.is_none() {
                            let next_canonical =
                                ascii_lowercase_logical_name(edge.dep_name.clone());
                            let next_identity =
                                (Arc::clone(&edge.dep_repository), next_canonical.clone());
                            if !visited.contains(&next_identity) {
                                worklist.push_back((
                                    edge.dep_repository,
                                    next_canonical,
                                    depth + 1,
                                ));
                            }
                        }
                        // Pinned edges prune their subtree.
                    }
                }
                Err(errors) => closure_errors.extend(errors),
            }
        }
    }

    if !closure_errors.is_empty() {
        return Err(closure_errors);
    }

    // Clip to [from_key, effective_to), matching the range logic of the deleted effective_dates.
    let clipped: Vec<EffectiveDate> = match to {
        Some(dt) => candidate_dates
            .range(from_key..EffectiveDate::DateTimeValue(dt))
            .cloned()
            .collect(),
        None => candidate_dates.range(from_key..).cloned().collect(),
    };

    assert!(
        !clipped.is_empty(),
        "BUG: plan_breakpoints produced no dates for spec '{}' effective from {:?}; \
         spec.effective_from is always within [from_key, effective_to)",
        spec.name,
        spec.effective_from
    );

    Ok(clipped)
}

// ---------------------------------------------------------------------------
// Unqualified dep interface validation
// ---------------------------------------------------------------------------

/// Members a consumer spec names through one alias, keyed like
/// [`crate::planning::semantics::DataPath::input_key`] relative to the dependency.
#[derive(Debug, Default, PartialEq, Eq)]
struct AliasReads {
    /// Data or rule members: `alias.x` in rule expressions, `-> with` paths, and
    /// `-> with` reference targets.
    members: HashSet<String>,
    /// Named types: `data x: alias.money` (also under `range`).
    types: HashSet<String>,
    /// The alias is used as a spec value (`rule chosen: alias`), so every data
    /// and rule of the dependency, including nested `uses`, is part of the result.
    whole: bool,
}

fn alias_reads(spec: &LemmaSpec, alias: &str) -> AliasReads {
    let mut reads = AliasReads::default();
    for rule in &spec.rules {
        collect_alias_expression_reads(&rule.expression, alias, &mut reads);
        for clause in &rule.unless_clauses {
            collect_alias_expression_reads(&clause.condition, alias, &mut reads);
            collect_alias_expression_reads(&clause.result, alias, &mut reads);
        }
    }
    for data in &spec.data {
        if !data.reference.is_local() {
            continue;
        }
        match &data.value {
            DataValue::Definition {
                base: Some(base), ..
            } => {
                if let Some(type_name) = qualified_type_read(base, alias) {
                    reads.types.insert(type_name.to_string());
                }
            }
            DataValue::Definition { base: None, .. } => {}
            DataValue::Import { bindings, .. } => {
                for binding in bindings {
                    if data.reference.name == alias {
                        reads
                            .members
                            .insert(member_key(&binding.path.segments, &binding.path.name));
                    }
                    if let WithRhs::Reference { target } = &binding.rhs {
                        collect_alias_reference_read(target, alias, &mut reads.members);
                    }
                }
            }
        }
    }
    reads
}

fn member_key(segments: &[String], name: &str) -> String {
    let mut key = String::new();
    for segment in segments {
        key.push_str(segment);
        key.push('.');
    }
    key.push_str(name);
    key
}

fn collect_alias_reference_read(reference: &Reference, alias: &str, out: &mut HashSet<String>) {
    if let Some((first, rest)) = reference.segments.split_first() {
        if first == alias {
            out.insert(member_key(rest, &reference.name));
        }
    }
}

fn collect_alias_expression_reads(expression: &Expression, alias: &str, reads: &mut AliasReads) {
    match &expression.kind {
        ExpressionKind::Reference(reference) => {
            if reference.segments.is_empty() && reference.name == alias {
                reads.whole = true;
            } else {
                collect_alias_reference_read(reference, alias, &mut reads.members);
            }
        }
        ExpressionKind::DateRelative(_, inner)
        | ExpressionKind::DateCalendar(_, _, inner)
        | ExpressionKind::PastFutureRange(_, inner)
        | ExpressionKind::UnitConversion(inner, _)
        | ExpressionKind::LogicalNegation(inner, _)
        | ExpressionKind::MathematicalComputation(_, inner)
        | ExpressionKind::RangeBound(_, inner)
        | ExpressionKind::ResultIsVeto(inner) => {
            collect_alias_expression_reads(inner, alias, reads);
        }
        ExpressionKind::RangeLiteral(left, right)
        | ExpressionKind::RangeContainment(left, right)
        | ExpressionKind::LogicalAnd(left, right)
        | ExpressionKind::Arithmetic(left, _, right)
        | ExpressionKind::Comparison(left, _, right) => {
            collect_alias_expression_reads(left, alias, reads);
            collect_alias_expression_reads(right, alias, reads);
        }
        ExpressionKind::Literal(_) | ExpressionKind::Now | ExpressionKind::Veto(_) => {}
    }
}

/// Type name read from `alias` by a data row's parent type, if any.
fn qualified_type_read<'p>(base: &'p ParentType, alias: &str) -> Option<&'p str> {
    match base {
        ParentType::Qualified { spec_alias, inner } if spec_alias == alias => {
            match inner.as_ref() {
                ParentType::Custom { name } => Some(name.as_str()),
                ParentType::Primitive { .. }
                | ParentType::Qualified { .. }
                | ParentType::Ranged { .. } => None,
            }
        }
        ParentType::Ranged { inner } => qualified_type_read(inner, alias),
        ParentType::Qualified { .. } | ParentType::Primitive { .. } | ParentType::Custom { .. } => {
            None
        }
    }
}

fn dep_row_label(dep_qualified: &str, at: &EffectiveDate) -> String {
    match at {
        EffectiveDate::Origin => dep_qualified.to_string(),
        EffectiveDate::DateTimeValue(datetime) => format!("{dep_qualified} {datetime}"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum MemberKind {
    Data,
    Rule,
    Type,
}

impl MemberKind {
    fn label(self) -> &'static str {
        match self {
            MemberKind::Data => "data",
            MemberKind::Rule => "rule",
            MemberKind::Type => "type",
        }
    }
}

/// Type of an interface member as seen in one dependency row.
struct MemberRow<'a> {
    lemma_type: &'a LemmaType,
    at: EffectiveDate,
}

fn interface_member_drift_message(
    consumer: &str,
    dep_qualified: &str,
    kind: &str,
    name: &str,
    earlier: &MemberRow<'_>,
    later: &MemberRow<'_>,
) -> String {
    let earlier_row = dep_row_label(dep_qualified, &earlier.at);
    let later_row = dep_row_label(dep_qualified, &later.at);
    let earlier_label = earlier.lemma_type.specifications.to_string();
    let later_label = later.lemma_type.specifications.to_string();
    if earlier_label == later_label {
        format!(
            "'{consumer}' depends on '{dep_qualified}' without pinning an effective date, but {kind} '{name}' changed between {earlier_row} and {later_row}"
        )
    } else {
        format!(
            "'{consumer}' depends on '{dep_qualified}' without pinning an effective date, but {kind} '{name}' has type {earlier_label} in {earlier_row} and {later_label} in {later_row}"
        )
    }
}

/// For each spec with unqualified deps, verify that the dep's interface
/// (schema) is type-compatible across all dep specs active within the
/// consumer's effective range. Qualified deps are pinned and skip this check.
///
/// Only consumers inside `scope` are checked: a consumer outside it has unchanged
/// edges into unchanged dependencies, so the previous pass already proved it.
///
/// Returns `(consumer_repository, consumer_spec_name, consumer_spec, error)` tuples.
pub fn validate_dependency_interfaces<'a>(
    context: &'a Context,
    plan_view: &super::PlanView<'_>,
    scope: &super::ReplanScope,
    missing_repository_source_specs: &HashSet<(Arc<LemmaRepository>, String, EffectiveDate)>,
    failed_source_specs: &HashSet<(Arc<LemmaRepository>, String, EffectiveDate)>,
) -> Vec<(Arc<LemmaRepository>, String, &'a LemmaSpec, Error)> {
    let mut errors: Vec<(Arc<LemmaRepository>, String, &'a LemmaSpec, Error)> = Vec::new();

    for member in scope.members() {
        let consumer_repository = &member.repository;
        let consumer_spec_name = &member.spec;
        // A member with no spec set left the context in this batch: it has no consumer
        // interfaces left to check.
        if let Some(consumer_spec_set) = context.spec_set(consumer_repository, consumer_spec_name) {
            for spec in consumer_spec_set.iter_specs() {
                if missing_repository_source_specs.contains(&(
                    Arc::clone(consumer_repository),
                    consumer_spec_name.clone(),
                    spec.effective_from.clone(),
                )) {
                    continue;
                }

                let consumer_repository = Arc::clone(consumer_repository);
                let (eff_from, eff_to) = consumer_spec_set.effective_range(spec);

                let edges = match dependency_edges(spec, &consumer_repository, context) {
                    Ok(edges) => edges,
                    Err(errs) => {
                        for e in errs {
                            errors.push((
                                Arc::clone(&consumer_repository),
                                consumer_spec_name.clone(),
                                spec,
                                e,
                            ));
                        }
                        continue;
                    }
                };

                let uses_aliases: HashSet<&str> = edges
                    .iter()
                    .filter(|edge| edge.origin == DependencyEdgeOrigin::Uses)
                    .map(|edge| edge.alias.as_str())
                    .collect();

                for edge in &edges {
                    if edge.explicit_effective.is_some() {
                        continue;
                    }
                    // The `uses` row for this alias already checks every member read through it.
                    if edge.origin == DependencyEdgeOrigin::QualifiedType
                        && uses_aliases.contains(edge.alias.as_str())
                    {
                        continue;
                    }

                    let dep_qualified =
                        format_dep_qualified_name(&edge.dep_repository, &edge.dep_name);
                    let Some(_dep_ss) = context.spec_set(&edge.dep_repository, &edge.dep_name)
                    else {
                        errors.push((
                            Arc::clone(&consumer_repository),
                            consumer_spec_name.clone(),
                            spec,
                            Error::validation_with_context(
                                format!(
                                    "'{}' depends on '{}', but '{}' does not exist",
                                    spec.name, dep_qualified, dep_qualified
                                ),
                                Some(edge.source.clone()),
                                None::<String>,
                                Some(spec),
                                None,
                            ),
                        ));
                        continue;
                    };
                    let Some(dep_plans) =
                        plan_view.get_plans(edge.dep_repository.name.as_deref(), &edge.dep_name)
                    else {
                        let dep_repo = Arc::clone(&edge.dep_repository);
                        let dep_name = edge.dep_name.clone();
                        let all_dep_rows_failed = context
                            .spec_set(&dep_repo, &dep_name)
                            .expect("BUG: dependency spec set missing after existence check")
                            .iter_specs()
                            .all(|dep_spec| {
                                failed_source_specs.contains(&(
                                    Arc::clone(&dep_repo),
                                    dep_name.clone(),
                                    dep_spec.effective_from.clone(),
                                ))
                            });
                        if all_dep_rows_failed {
                            continue;
                        }
                        panic!(
                            "BUG: dependency '{}' in context but has no planning result",
                            dep_qualified
                        );
                    };

                    let reads = alias_reads(spec, &edge.alias);
                    let mut seen: HashMap<(MemberKind, String), MemberRow<'_>> = HashMap::new();
                    let mut member_drift: Option<String> = None;
                    let mut saw_overlapping_plan = false;

                    let mut dep_iter = dep_plans.iter().peekable();
                    'dep_plans: while let Some((effective, plan)) = dep_iter.next() {
                        let window_from = plan.effective.as_ref().cloned();
                        let window_to = dep_iter
                            .peek()
                            .and_then(|(_, next)| next.effective.as_ref().cloned());

                        if !super::ranges_overlap(&eff_from, &eff_to, &window_from, &window_to) {
                            continue;
                        }
                        saw_overlapping_plan = true;

                        let mut observed: Vec<(MemberKind, String, &LemmaType)> = Vec::new();
                        for (path, data) in &plan.data {
                            let Some(lemma_type) = data.schema_type() else {
                                continue;
                            };
                            if matches!(data, DataDefinition::Reference { .. }) {
                                continue;
                            }
                            let input_key = path.input_key();
                            if reads.whole || reads.members.contains(&input_key) {
                                observed.push((MemberKind::Data, input_key, lemma_type));
                            }
                        }
                        for rule in plan.rules.values() {
                            let key = rule.path.input_key();
                            let named = rule.path.segments.is_empty()
                                && reads.members.contains(rule.name());
                            if reads.whole || named {
                                observed.push((MemberKind::Rule, key, rule.rule_type.as_ref()));
                            }
                        }
                        for (type_name, lemma_type) in &plan.resolved_types.resolved {
                            if reads.types.contains(type_name) {
                                observed.push((
                                    MemberKind::Type,
                                    type_name.clone(),
                                    lemma_type.as_ref(),
                                ));
                            }
                        }

                        for (kind, key, lemma_type) in observed {
                            let row = MemberRow {
                                lemma_type,
                                at: effective.clone(),
                            };
                            match seen.get(&(kind, key.clone())) {
                                Some(earlier) if earlier.lemma_type != lemma_type => {
                                    member_drift = Some(interface_member_drift_message(
                                        &spec.name,
                                        &dep_qualified,
                                        kind.label(),
                                        &key,
                                        earlier,
                                        &row,
                                    ));
                                    break 'dep_plans;
                                }
                                Some(_) => {}
                                None => {
                                    seen.insert((kind, key), row);
                                }
                            }
                        }
                    }

                    if let Some(message) = member_drift {
                        errors.push((
                            Arc::clone(&consumer_repository),
                            consumer_spec_name.clone(),
                            spec,
                            Error::validation_with_context(
                                message,
                                Some(edge.source.clone()),
                                Some(format!(
                                    "Pin '{dep_qualified}' to a specific effective date, or make '{dep_qualified}' interface-compatible across specs."
                                )),
                                Some(spec),
                                None,
                            ),
                        ));
                    } else if !saw_overlapping_plan {
                        errors.push((
                            Arc::clone(&consumer_repository),
                            consumer_spec_name.clone(),
                            spec,
                            Error::validation_with_context(
                                format!(
                                    "'{}' depends on '{dep_qualified}' without pinning an effective date, but no row of '{dep_qualified}' is active during {}",
                                    spec.name,
                                    consumer_identity(spec)
                                ),
                                Some(edge.source.clone()),
                                Some(format!(
                                    "Add '{dep_qualified}' with an effective date covering {}, or pin the import.",
                                    consumer_identity(spec)
                                )),
                                Some(spec),
                                None,
                            ),
                        ));
                    }
                }
            }
        }
    }

    errors
}

// ---------------------------------------------------------------------------
// Spec DAG: DFS discovery + Kahn's topological sort
// ---------------------------------------------------------------------------

/// Errors from dependency discovery, distinguishing cycles (global) from other
/// errors (per-spec).
#[derive(Debug)]
pub(crate) enum DependencyDiscoveryError {
    /// Dependency cycle detected -- global structural error.
    Cycle(Vec<Error>),
    /// Missing deps, resolution failures, etc. -- per-spec errors.
    Other(Vec<Error>),
}

/// Single-root DFS dependency discovery. Returns a topo-sorted list of
/// [`DependencySpec`]s containing `root` and its transitive deps, or a typed
/// error on cycles / missing deps.
///
/// Entries are dependency-first (leaves first after Kahn sort, root last)
/// and deduplicated by Context-owned row identity (`same_loaded_spec`).
pub(crate) fn discover_dependency_order<'a>(
    context: &'a Context,
    root: &'a LemmaSpec,
    effective: &EffectiveDate,
    limits: &crate::limits::ResourceLimits,
) -> Result<Vec<DependencySpec<'a>>, DependencyDiscoveryError> {
    let mut nodes: Vec<DependencySpec<'a>> = Vec::new();
    let mut edges: Vec<(usize, usize)> = Vec::new();
    let mut errors: Vec<Error> = Vec::new();
    // Visited: (repo name key, loaded name, loaded effective_from, resolution instant).
    let mut visited: Vec<(Option<String>, String, EffectiveDate, EffectiveDate)> = Vec::new();

    let root_repository =
        lookup_owning_repository(context, root).unwrap_or_else(|| context.workspace());
    dfs_discover(
        context,
        root,
        &root_repository,
        effective,
        &mut nodes,
        &mut edges,
        &mut errors,
        &mut visited,
        0,
        limits,
    );

    if errors.is_empty() {
        kahns_topo_sort(&nodes, &edges).map_err(|err| DependencyDiscoveryError::Cycle(vec![err]))
    } else {
        Err(DependencyDiscoveryError::Other(errors))
    }
}

/// Find the repository `Arc` that owns `spec` in `context`. Used by DFS to resolve
/// same-repository references against the consumer's repository without callers having to
/// pass it explicitly.
pub(crate) fn lookup_owning_repository(
    context: &Context,
    spec: &LemmaSpec,
) -> Option<Arc<LemmaRepository>> {
    for (repository, inner) in context.repositories().iter() {
        for (name, set) in inner.iter() {
            if name != &spec.name {
                continue;
            }
            if set.iter_specs().any(|p| same_loaded_spec(p, spec)) {
                return Some(Arc::clone(repository));
            }
        }
    }
    None
}

fn visit_key(
    repository: &LemmaRepository,
    spec: &LemmaSpec,
    effective: &EffectiveDate,
) -> (Option<String>, String, EffectiveDate, EffectiveDate) {
    (
        repository.name.clone(),
        spec.name.clone(),
        spec.effective_from.clone(),
        effective.clone(),
    )
}

#[allow(clippy::too_many_arguments)]
fn dfs_discover<'a>(
    context: &'a Context,
    spec: &'a LemmaSpec,
    consumer_repository: &Arc<LemmaRepository>,
    effective: &EffectiveDate,
    nodes: &mut Vec<DependencySpec<'a>>,
    edges: &mut Vec<(usize, usize)>,
    errors: &mut Vec<Error>,
    visited: &mut Vec<(Option<String>, String, EffectiveDate, EffectiveDate)>,
    depth: usize,
    limits: &crate::limits::ResourceLimits,
) {
    if depth > limits.max_spec_dependency_depth {
        errors.push(Error::resource_limit_exceeded(
            "max_spec_dependency_depth",
            limits.max_spec_dependency_depth.to_string(),
            depth.to_string(),
            format!(
                "Spec '{}' exceeds the maximum dependency nesting depth; flatten the import chain",
                spec.name
            ),
            None,
            Some(spec),
            None,
        ));
        return;
    }

    let key = visit_key(consumer_repository, spec, effective);
    if visited.iter().any(|v| v == &key) {
        return;
    }
    visited.push(key);

    let spec_index = match nodes
        .iter()
        .position(|node| same_loaded_spec(node.spec, spec))
    {
        Some(existing_index) => existing_index,
        None => {
            if nodes.len() >= limits.max_dag_specs {
                errors.push(Error::resource_limit_exceeded(
                    "max_dag_specs",
                    limits.max_dag_specs.to_string(),
                    (nodes.len() + 1).to_string(),
                    format!(
                        "Dependency graph of the root spec grew past {} specs at '{}'; \
                         reduce the number of transitive imports",
                        limits.max_dag_specs, spec.name
                    ),
                    None,
                    Some(spec),
                    None,
                ));
                return;
            }
            let new_index = nodes.len();
            nodes.push(DependencySpec {
                repository: Arc::clone(consumer_repository),
                spec,
            });
            new_index
        }
    };

    let edges_for_spec = match dependency_edges(spec, consumer_repository, context) {
        Ok(edges) => edges,
        Err(errs) => {
            errors.extend(errs);
            return;
        }
    };

    for edge in edges_for_spec {
        let dep_effective = edge
            .explicit_effective
            .clone()
            .map_or_else(|| effective.clone(), EffectiveDate::DateTimeValue);

        let dependency = match resolve_spec_ref(
            context,
            &edge.as_spec_ref(),
            consumer_repository,
            spec,
            effective,
            Some(edge.source.clone()),
        ) {
            Ok((_, dependency)) => dependency,
            Err(error) => {
                errors.push(error);
                continue;
            }
        };

        dfs_discover(
            context,
            dependency,
            &edge.dep_repository,
            &dep_effective,
            nodes,
            edges,
            errors,
            visited,
            depth + 1,
            limits,
        );
        let Some(dep_index) = nodes
            .iter()
            .position(|node| same_loaded_spec(node.spec, dependency))
        else {
            assert!(
                !errors.is_empty(),
                "BUG: dependency absent from nodes without a recorded error"
            );
            continue;
        };
        edges.push((dep_index, spec_index));
    }
}

/// Kahn's topological sort over an index-based edge list.
///
/// `nodes` is insertion-ordered; `edges` are `(from, to)` index pairs.
/// Returns dependency specs in dependency-first order (leaves first, root last).
fn kahns_topo_sort<'a>(
    nodes: &[DependencySpec<'a>],
    edges: &[(usize, usize)],
) -> Result<Vec<DependencySpec<'a>>, Error> {
    let n = nodes.len();
    let mut in_degree = vec![0usize; n];
    let mut adjacency: Vec<Vec<usize>> = vec![Vec::new(); n];

    for &(from, to) in edges {
        adjacency[from].push(to);
        in_degree[to] += 1;
    }

    let mut queue: VecDeque<usize> = (0..n).filter(|&i| in_degree[i] == 0).collect();
    let mut result: Vec<DependencySpec<'a>> = Vec::with_capacity(n);

    while let Some(idx) = queue.pop_front() {
        result.push(DependencySpec {
            repository: Arc::clone(&nodes[idx].repository),
            spec: nodes[idx].spec,
        });
        for &neighbor in &adjacency[idx] {
            in_degree[neighbor] -= 1;
            if in_degree[neighbor] == 0 {
                queue.push_back(neighbor);
            }
        }
    }

    if result.len() != n {
        let mut cycle_names: Vec<String> = in_degree
            .iter()
            .enumerate()
            .filter(|(_, &deg)| deg > 0)
            .map(|(i, _)| nodes[i].spec.name.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        cycle_names.sort();
        let cycle_path = if cycle_names.len() > 1 {
            let mut path = cycle_names.clone();
            path.push(cycle_names[0].clone());
            path.join(" -> ")
        } else {
            cycle_names.join(" -> ")
        };
        return Err(Error::validation(
            format!("Spec dependency cycle: {}", cycle_path),
            None,
            None::<String>,
        ));
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsing::ast::Span;
    use crate::parsing::ast::{
        DataValue as AstDataValue, LemmaData, LemmaRepository, LemmaSpec, ParentType, Reference,
        RepositoryQualifier, SpecRef,
    };
    use crate::parsing::source::Source;

    fn discovery_errors(e: DependencyDiscoveryError) -> Vec<Error> {
        match e {
            DependencyDiscoveryError::Cycle(e) | DependencyDiscoveryError::Other(e) => e,
        }
    }

    use crate::literals::DateGranularity;

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

    fn dummy_source() -> Source {
        Source::new(
            crate::parsing::source::SourceType::Volatile,
            Span {
                start: 0,
                end: 0,
                line: 1,
                col: 0,
            },
        )
    }

    fn registry_iso_repository() -> Arc<LemmaRepository> {
        Arc::new(LemmaRepository {
            name: Some("@iso/countries".to_string()),
            dependency: Some("@iso/countries".to_string()),
            start_line: 1,
            source_type: Some(crate::parsing::source::SourceType::Volatile),
        })
    }

    fn alpha2_slice_2024() -> LemmaSpec {
        let mut s = LemmaSpec::new("alpha2".to_string());
        s.effective_from = EffectiveDate::from_option(Some(date(2024, 1, 1)));
        s
    }

    fn consumer_alias_from_alpha2() -> LemmaSpec {
        let mut s = LemmaSpec::new("with_alias".to_string());
        s.data.push(LemmaData::new(
            Reference::local("iso".to_string()),
            AstDataValue::import(SpecRef {
                name: "alpha2".to_string(),
                repository: Some(RepositoryQualifier::new("@iso/countries")),
                effective: Some(date(2024, 1, 1)),
                repository_span: None,
                target_span: None,
            }),
            dummy_source(),
        ));
        s.data.push(LemmaData::new(
            Reference::local("y".to_string()),
            AstDataValue::Definition {
                base: Some(ParentType::Qualified {
                    spec_alias: "iso".into(),
                    inner: Box::new(ParentType::Custom {
                        name: "code".into(),
                    }),
                }),
                constraints: None,
                value: None,
            },
            dummy_source(),
        ));
        s
    }

    fn spec_with_dep(
        name: &str,
        eff: Option<DateTimeValue>,
        dep: &str,
        qualified_at: Option<DateTimeValue>,
        dep_repository: Option<RepositoryQualifier>,
    ) -> LemmaSpec {
        let mut s = LemmaSpec::new(name.to_string());
        s.effective_from = EffectiveDate::from_option(eff);
        s.data.push(LemmaData {
            reference: Reference::local("d".to_string()),
            value: AstDataValue::import(SpecRef {
                name: dep.to_string(),
                repository: dep_repository,
                effective: qualified_at,
                repository_span: None,
                target_span: None,
            }),
            source_location: dummy_source(),
        });
        s
    }

    /// `plan` inserts every successfully planned spec into the plan sets
    /// before interface validation runs. A dependency that exists in the
    /// context, did not fail planning, and is still absent from the plan sets
    /// means the planning pipeline skipped a spec — a bug. Skipping the
    /// interface check silently would let a consumer keep plans validated
    /// against nothing. Must crash.
    #[test]
    #[should_panic(expected = "BUG")]
    fn validate_panics_when_healthy_dep_in_context_but_unplanned() {
        let mut ctx = Context::new();
        let repository = ctx.workspace();

        let dep = LemmaSpec::new("dep".to_string());
        ctx.insert_spec(Arc::clone(&repository), dep)
            .expect("insert dep");

        let consumer = spec_with_dep("consumer", None, "dep", None, None);
        ctx.insert_spec(Arc::clone(&repository), consumer)
            .expect("insert consumer");

        // Dep is in the context, healthy, but was never planned into plans.
        let replanned = crate::planning::PlanStore::new();
        let committed = crate::planning::PlanStore::new();
        let scope = crate::planning::ReplanScope::from_changed_sets(
            &ctx,
            vec![(ctx.workspace(), "consumer".to_string())],
        );
        let plan_view = crate::planning::PlanView::new(&replanned, &committed, &scope, &ctx);
        let missing_repository_source_specs = HashSet::new();

        let _ = validate_dependency_interfaces(
            &ctx,
            &plan_view,
            &scope,
            &missing_repository_source_specs,
            &HashSet::new(),
        );
    }

    #[test]
    fn dag_error_unqualified_missing_dep_includes_parent_and_resolve_instant() {
        let mut ctx = Context::new();
        let repository = ctx.workspace();
        let consumer = spec_with_dep("consumer", Some(date(2025, 1, 1)), "dep", None, None);
        ctx.insert_spec(Arc::clone(&repository), consumer.clone())
            .unwrap();

        let effective = EffectiveDate::DateTimeValue(date(2025, 1, 1));
        let errs = discovery_errors(
            discover_dependency_order(
                &ctx,
                &consumer,
                &effective,
                &crate::ResourceLimits::default(),
            )
            .unwrap_err(),
        );

        assert_eq!(errs.len(), 1);
        let msg = errs[0].message();
        assert!(msg.contains("'consumer'"), "should name parent spec: {msg}");
        assert!(msg.contains("'dep'"), "should name missing dep: {msg}");
        assert!(
            msg.contains("does not exist"),
            "should say dep doesn't exist: {msg}"
        );

        let suggestion = errs[0].suggestion().expect("should have suggestion");
        assert!(
            suggestion.contains("dep"),
            "suggestion should name dep: {suggestion}"
        );
    }

    #[test]
    fn dag_error_qualified_missing_dep_mentions_qualifier_instant() {
        let mut ctx = Context::new();
        let repository = ctx.workspace();
        let consumer = spec_with_dep(
            "consumer",
            Some(date(2025, 1, 1)),
            "dep",
            Some(date(2025, 8, 1)),
            None,
        );
        ctx.insert_spec(Arc::clone(&repository), consumer.clone())
            .unwrap();

        let effective = EffectiveDate::DateTimeValue(date(2025, 1, 1));
        let errs = discovery_errors(
            discover_dependency_order(
                &ctx,
                &consumer,
                &effective,
                &crate::ResourceLimits::default(),
            )
            .unwrap_err(),
        );

        assert_eq!(errs.len(), 1);
        let msg = errs[0].message();
        assert!(msg.contains("'consumer'"), "should name parent: {msg}");
        assert!(msg.contains("'dep'"), "should name dep: {msg}");
        assert!(
            msg.contains("2025"),
            "should mention qualifier instant: {msg}"
        );
        assert!(
            msg.contains("at that instant"),
            "should use qualified wording: {msg}"
        );

        let suggestion = errs[0].suggestion().expect("should have suggestion");
        assert!(
            suggestion.contains("effective_from") || suggestion.contains("reference instant"),
            "suggestion should guide fix: {suggestion}"
        );
    }

    #[test]
    fn dag_error_registry_dep_suggests_lemma_install() {
        let mut ctx = Context::new();
        let repository = ctx.workspace();
        let registry_repository = Arc::new(LemmaRepository {
            name: Some("@org/pkg".to_string()),
            dependency: Some("@org/pkg".to_string()),
            start_line: 1,
            source_type: Some(crate::parsing::source::SourceType::Volatile),
        });
        // Insert a registry-bound spec so the repository qualifier resolves but
        // the *requested* spec is missing — exercising the "registry dep
        // missing" branch of the suggestion text.
        let registry_spec = LemmaSpec::new("other_spec".to_string());
        ctx.insert_spec(Arc::clone(&registry_repository), registry_spec)
            .unwrap();

        let consumer = spec_with_dep(
            "consumer",
            Some(date(2025, 1, 1)),
            "missing",
            None,
            Some(RepositoryQualifier::new("@org/pkg")),
        );
        ctx.insert_spec(Arc::clone(&repository), consumer).unwrap();

        let consumer = ctx
            .spec_set(&repository, "consumer")
            .and_then(|ss| ss.get_exact(Some(&date(2025, 1, 1))))
            .expect("consumer inserted");
        let effective = EffectiveDate::DateTimeValue(date(2025, 1, 1));
        let errs = discovery_errors(
            discover_dependency_order(
                &ctx,
                consumer,
                &effective,
                &crate::ResourceLimits::default(),
            )
            .unwrap_err(),
        );

        assert_eq!(errs.len(), 1);
        let suggestion = errs[0].suggestion().expect("should have suggestion");
        assert!(
            suggestion.contains("lemma install"),
            "registry dep suggestion should include 'lemma install': {suggestion}"
        );
    }

    #[test]
    fn dag_error_has_source_location() {
        let mut ctx = Context::new();
        let repository = ctx.workspace();
        let consumer = spec_with_dep("consumer", Some(date(2025, 1, 1)), "dep", None, None);
        ctx.insert_spec(Arc::clone(&repository), consumer.clone())
            .unwrap();

        let effective = EffectiveDate::DateTimeValue(date(2025, 1, 1));
        let errs = discovery_errors(
            discover_dependency_order(
                &ctx,
                &consumer,
                &effective,
                &crate::ResourceLimits::default(),
            )
            .unwrap_err(),
        );

        let display = format!("{}", errs[0]);
        assert!(
            display.contains("volatile") || display.contains("line"),
            "error should carry source context: {display}"
        );
    }

    #[test]
    fn resolve_spec_ref_expands_bare_from_uses_alias() {
        let mut ctx = Context::new();
        let iso_repo = registry_iso_repository();
        let workspace = ctx.workspace();
        ctx.insert_spec(Arc::clone(&iso_repo), alpha2_slice_2024())
            .unwrap();
        let consumer = consumer_alias_from_alpha2();
        ctx.insert_spec(Arc::clone(&workspace), consumer.clone())
            .unwrap();

        let effective = EffectiveDate::DateTimeValue(date(2024, 6, 1));
        let resolved = resolve_spec_ref(
            &ctx,
            &SpecRef {
                name: "iso".into(),
                repository: None,
                effective: None,
                repository_span: None,
                target_span: None,
            },
            &workspace,
            &consumer,
            &effective,
            None,
        )
        .expect("alpha2 via uses alias");

        assert_eq!(resolved.1.name, "alpha2");
    }

    #[test]
    fn uses_implicit_alias_same_name_with_effective_resolves_on_import_target() {
        let inner = SpecRef {
            name: "finance".to_string(),
            repository: None,
            effective: Some(date(2026, 1, 1)),
            repository_span: None,
            target_span: None,
        };
        let mut consumer = LemmaSpec::new("finance".to_string());
        consumer.effective_from = EffectiveDate::from_option(Some(date(2027, 1, 1)));
        consumer.data.push(LemmaData::new(
            Reference::local("finance".to_string()),
            AstDataValue::import(inner.clone()),
            dummy_source(),
        ));

        let mut ctx = Context::new();
        let workspace = ctx.workspace();
        let mut finance_2026 = LemmaSpec::new("finance".to_string());
        finance_2026.effective_from = EffectiveDate::from_option(Some(date(2026, 1, 1)));
        ctx.insert_spec(Arc::clone(&workspace), finance_2026)
            .unwrap();
        ctx.insert_spec(Arc::clone(&workspace), consumer.clone())
            .unwrap();

        let (resolved, effective_ref) = resolve_spec_ref_after_expanding_uses_aliases(
            &ctx,
            &inner,
            Some(&consumer.data[0].source_location),
            "finance",
            Some(&consumer),
        )
        .expect("uses finance 2026-01-01 must resolve when alias equals target name");

        assert_eq!(resolved.name, "finance");
        assert_eq!(
            resolved.effective.as_ref(),
            Some(&date(2026, 1, 1)),
            "effective pin on uses line must be preserved"
        );
        assert_eq!(effective_ref.effective.as_ref(), Some(&date(2026, 1, 1)),);
    }

    #[test]
    fn resolve_spec_ref_rejects_forward_pin_without_exact_slice() {
        let mut ctx = Context::new();
        let workspace = ctx.workspace();
        let mut origin = LemmaSpec::new("finance".to_string());
        origin.data.push(LemmaData::new(
            Reference::local("rate".to_string()),
            DataValue::Definition {
                base: Some(ParentType::Custom {
                    name: "number".into(),
                }),
                constraints: None,
                value: None,
            },
            dummy_source(),
        ));
        ctx.insert_spec(Arc::clone(&workspace), origin).unwrap();
        let mut consumer = LemmaSpec::new("finance".to_string());
        consumer.effective_from = EffectiveDate::from_option(Some(date(2026, 5, 20)));
        consumer.data.push(LemmaData::new(
            Reference::local("fin".to_string()),
            DataValue::import(SpecRef {
                name: "finance".into(),
                repository: None,
                effective: Some(date(2027, 1, 1)),
                repository_span: None,
                target_span: None,
            }),
            dummy_source(),
        ));
        let consumer_effective = consumer.effective_from.clone();
        ctx.insert_spec(Arc::clone(&workspace), consumer).unwrap();
        let consumer = ctx
            .spec_set(&workspace, "finance")
            .and_then(|ss| ss.get_exact(Some(&date(2026, 5, 20))))
            .expect("inserted 2026 finance slice");

        let err = resolve_spec_ref(
            &ctx,
            &SpecRef {
                name: "finance".into(),
                repository: None,
                effective: Some(date(2027, 1, 1)),
                repository_span: None,
                target_span: None,
            },
            &workspace,
            consumer,
            &consumer_effective,
            None,
        )
        .expect_err("finance 2027 row does not exist");

        let msg = err.to_string();
        assert!(
            msg.contains("active at that instant") || msg.contains("cannot reference itself"),
            "expected missing exact slice or same-body import, got: {msg}"
        );
    }

    #[test]
    fn resolve_spec_ref_rejects_effective_on_bare_from_uses_alias() {
        let consumer = consumer_alias_from_alpha2();
        let mut ctx = Context::new();
        let iso_repo = registry_iso_repository();
        let workspace = ctx.workspace();
        ctx.insert_spec(Arc::clone(&iso_repo), alpha2_slice_2024())
            .unwrap();
        ctx.insert_spec(Arc::clone(&workspace), consumer.clone())
            .unwrap();

        let effective = EffectiveDate::DateTimeValue(date(2024, 6, 1));
        let err = resolve_spec_ref(
            &ctx,
            &SpecRef {
                name: "iso".into(),
                repository: None,
                effective: Some(date(2026, 1, 1)),
                repository_span: None,
                target_span: None,
            },
            &workspace,
            &consumer,
            &effective,
            None,
        )
        .expect_err("effective pin on from + uses alias is invalid");

        let msg = err.message();
        assert!(msg.contains("uses alias") || msg.contains("uses"), "{msg}");
    }

    #[test]
    fn dependency_edges_from_uses_alias_has_registry_qualifier() {
        let mut ctx = Context::new();
        let iso_repo = registry_iso_repository();
        let workspace = ctx.workspace();
        ctx.insert_spec(Arc::clone(&iso_repo), alpha2_slice_2024())
            .unwrap();
        let consumer = consumer_alias_from_alpha2();
        ctx.insert_spec(Arc::clone(&workspace), consumer.clone())
            .unwrap();

        let edges = dependency_edges(&consumer, &workspace, &ctx).expect("dependency_edges");
        let from_y = edges
            .iter()
            .find(|e| {
                e.dep_name == "alpha2"
                    && e.explicit_repository_qualifier
                        .as_ref()
                        .is_some_and(|q| q.name == "@iso/countries")
                    && e.source == consumer.data[1].source_location
            })
            .expect("edge from data y binding");
        assert_eq!(from_y.dep_name, "alpha2");
    }

    #[test]
    fn dependency_edges_qualified_parent_resolves_uses_alias() {
        let mut ctx = Context::new();
        let workspace = ctx.workspace();
        let child = LemmaSpec::new("child".to_string());
        let mut dep = LemmaSpec::new("dep".to_string());
        dep.data.push(LemmaData::new(
            Reference::local("c".to_string()),
            DataValue::import(SpecRef {
                effective: Some(date(2025, 6, 1)),
                ..SpecRef::same_repository("child")
            }),
            dummy_source(),
        ));
        dep.data.push(LemmaData::new(
            Reference::local("money".to_string()),
            DataValue::Definition {
                base: Some(ParentType::Qualified {
                    spec_alias: "c".into(),
                    inner: Box::new(ParentType::Custom {
                        name: "money".into(),
                    }),
                }),
                constraints: None,
                value: None,
            },
            dummy_source(),
        ));
        let dep = dep;
        ctx.insert_spec(Arc::clone(&workspace), child.clone())
            .unwrap();
        ctx.insert_spec(Arc::clone(&workspace), dep.clone())
            .unwrap();

        let edges = dependency_edges(&dep, &workspace, &ctx).expect("edges");
        assert!(
            edges.iter().any(|e| e.dep_name == "child"),
            "qualified `c.money` must depend on resolved spec `child`, not alias `c`: {:?}",
            edges
                .iter()
                .map(|e| (&e.dep_name, &e.explicit_effective))
                .collect::<Vec<_>>()
        );
        assert!(
            !edges.iter().any(|e| e.dep_name == "c"),
            "must not emit unresolved alias name as dependency"
        );
    }

    #[test]
    fn build_dag_consumer_resolves_qualified_dep_parent_via_uses_alias() {
        let source = r#"
spec consumer 2025-01-01
uses d: dep 2025-01-01
rule out: d.doubled

spec dep 2025-01-01
uses c: child 2025-06-01
data money: c.money
data p: 5 usd
rule doubled: p * 2

spec child 2025-01-01
data money: measure
 -> unit eur: 1.00
 -> decimals 2

spec child 2025-06-01
data money: measure
 -> unit eur: 1.00
 -> unit usd: 0.91
 -> decimals 2
"#;
        let specs = crate::parse(
            source,
            crate::parsing::source::SourceType::Volatile,
            &crate::ResourceLimits::default(),
        )
        .expect("parse")
        .into_flattened_specs();
        let mut ctx = Context::new();
        let workspace = ctx.workspace();
        for spec in specs {
            ctx.insert_spec(Arc::clone(&workspace), spec).unwrap();
        }
        let consumer = ctx
            .spec_set(&workspace, "consumer")
            .and_then(|ss| ss.spec_at(&EffectiveDate::from_option(Some(date(2025, 3, 1)))))
            .expect("consumer");
        let ordered_dependencies = discover_dependency_order(
            &ctx,
            consumer,
            &EffectiveDate::from_option(Some(date(2025, 3, 1))),
            &crate::ResourceLimits::default(),
        )
        .expect("dependency order must not reference unresolved alias `c`");
        let names: Vec<_> = ordered_dependencies
            .iter()
            .map(|n| n.spec.name.as_str())
            .collect();
        assert!(names.contains(&"child"));
        assert!(names.contains(&"dep"));
        assert!(!names.contains(&"c"));
    }

    // --- plan_breakpoints unit tests ---

    fn breakpoints_for(
        source: &str,
        spec_name: &str,
        eff: Option<DateTimeValue>,
    ) -> Vec<EffectiveDate> {
        let specs = crate::parse(
            source,
            crate::parsing::source::SourceType::Volatile,
            &crate::ResourceLimits::default(),
        )
        .expect("parse")
        .into_flattened_specs();
        let mut ctx = Context::new();
        let workspace = ctx.workspace();
        for spec in specs {
            ctx.insert_spec(Arc::clone(&workspace), spec).unwrap();
        }
        let spec_set = ctx.spec_set(&workspace, spec_name).expect("spec set");
        let effective_key = EffectiveDate::from_option(eff.clone());
        let spec = spec_set.spec_at(&effective_key).expect("spec at effective");
        plan_breakpoints(&ctx, spec_set, spec, &crate::ResourceLimits::default())
            .expect("plan_breakpoints must succeed")
    }

    #[test]
    fn plan_breakpoints_no_deps_returns_own_effective_from() {
        let dates = breakpoints_for("spec root\ndata v: 1\nrule r: v\n", "root", None);
        assert_eq!(dates, vec![EffectiveDate::Origin]);
    }

    #[test]
    fn plan_breakpoints_unpinned_dep_includes_dep_version_dates() {
        let source = r#"
spec dep
data v: 1

spec dep 2025-06-01
data v: 2

spec root
uses d: dep
data v: 1
"#;
        let dates = breakpoints_for(source, "root", None);
        assert_eq!(
            dates,
            vec![
                EffectiveDate::Origin,
                EffectiveDate::DateTimeValue(date(2025, 6, 1)),
            ],
            "unpinned dep: must include dep's version boundaries"
        );
    }

    #[test]
    fn plan_breakpoints_pinned_dep_excludes_dep_version_dates() {
        let source = r#"
spec dep
data v: 1

spec dep 2025-06-01
data v: 2

spec root
uses d: dep 2025-01-01
data v: 1
"#;
        let dates = breakpoints_for(source, "root", None);
        assert_eq!(
            dates,
            vec![EffectiveDate::Origin],
            "pinned dep: dep's version boundaries must not appear"
        );
    }

    #[test]
    fn plan_breakpoints_transitive_dep_dates_included() {
        let source = r#"
spec base
data v: 1

spec base 2025-06-01
data v: 2

spec mid
uses b: base
data v: 1

spec root
uses m: mid
data v: 1
"#;
        let dates = breakpoints_for(source, "root", None);
        assert_eq!(
            dates,
            vec![
                EffectiveDate::Origin,
                EffectiveDate::DateTimeValue(date(2025, 6, 1)),
            ],
            "transitive dep: base's version boundary must appear in root's breakpoints"
        );
    }

    #[test]
    fn plan_breakpoints_clipped_to_validity_range() {
        let source = r#"
spec dep
data v: 1

spec dep 2025-01-01
data v: 2

spec dep 2025-12-01
data v: 3

spec root 2025-03-01
uses d: dep
data v: 1

spec root 2026-01-01
data v: 99
"#;
        // root version at 2025-03-01 is valid until 2026-01-01.
        // dep dates: Origin, 2025-01-01, 2025-12-01.
        // After clipping to [2025-03-01, 2026-01-01): only 2025-03-01 and 2025-12-01 remain.
        let dates = breakpoints_for(source, "root", Some(date(2025, 3, 1)));
        assert_eq!(
            dates,
            vec![
                EffectiveDate::DateTimeValue(date(2025, 3, 1)),
                EffectiveDate::DateTimeValue(date(2025, 12, 1)),
            ],
            "breakpoints must be clipped to this spec version's validity range"
        );
    }

    #[test]
    fn alias_reads_collects_every_member_named_through_alias() {
        let source = r#"
spec shop
uses a: amounts
  -> with rate: b.base
  -> with lines.count: 3
uses b: other
data price: a.money
data window: a.span range
data own: number
rule total: a.amount + own
 unless a.lines.flag then a.waiver
 unless b.skip then 0
rule start: lower a.period
"#;
        let specs = crate::parse(
            source,
            crate::parsing::source::SourceType::Volatile,
            &crate::ResourceLimits::default(),
        )
        .expect("parse")
        .into_flattened_specs();
        let shop = &specs[0];

        let strings = |names: &[&str]| -> HashSet<String> {
            names.iter().map(|name| (*name).to_string()).collect()
        };
        assert_eq!(
            alias_reads(shop, "a"),
            AliasReads {
                members: strings(&[
                    "rate",
                    "lines.count",
                    "amount",
                    "lines.flag",
                    "waiver",
                    "period",
                ]),
                types: strings(&["money", "span"]),
                whole: false,
            }
        );
        assert_eq!(
            alias_reads(shop, "b"),
            AliasReads {
                members: strings(&["base", "skip"]),
                types: HashSet::new(),
                whole: false,
            }
        );
    }
}

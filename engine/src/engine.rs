use crate::evaluation::Evaluator;
use crate::evaluation::{EvaluateData, EvaluateDataValue};
use crate::parsing::ast::{DateTimeValue, LemmaRepository, LemmaSpec};
use crate::parsing::source::SourceType;
use crate::parsing::{parse, EffectiveDate};
use crate::planning::execution_plan::{ExecutionPlan, Show, ShowData};
use crate::planning::semantics::{DataDefinition, SpecIdentity};
use crate::planning::{LemmaSpecSet, PlanStore};
use crate::{Error, ResourceLimits, Response};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

/// Load failure: errors plus the source texts we attempted to load.
#[derive(Debug, Clone)]
pub struct Errors {
    pub errors: Vec<Error>,
    pub sources: HashMap<SourceType, String>,
}

impl Errors {
    /// Iterate over the errors.
    pub fn iter(&self) -> std::slice::Iter<'_, Error> {
        self.errors.iter()
    }
}

/// Resolve an optional effective datetime string for planning or evaluation.
///
/// `None` or whitespace-only input resolves to [`DateTimeValue::now`].
/// Non-empty invalid strings return a request [`Error`].
pub fn resolve_effective(raw: Option<&str>) -> Result<DateTimeValue, Error> {
    match raw {
        Some(s) if !s.trim().is_empty() => s.trim().parse::<DateTimeValue>().map_err(|_| {
            Error::request(
                format!(
                    "Invalid effective value '{}'. Expected: YYYY, YYYY-MM, YYYY-MM-DD, or ISO 8601 datetime",
                    s.trim()
                ),
                None::<String>,
            )
        }),
        _ => Ok(DateTimeValue::now()),
    }
}

/// Repository name reserved for the embedded standard library (`repo lemma`, `spec units`).
/// User [`Engine::load`] must not target this name via [`SourceType::Dependency`].
pub const EMBEDDED_STDLIB_REPOSITORY: &str = "lemma";

/// One `uses` target from a spec version's source.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SpecDependency {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub repository: Option<String>,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effective: Option<DateTimeValue>,
}

impl std::fmt::Display for SpecDependency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(repository) = &self.repository {
            write!(f, "{repository} ")?;
        }
        write!(f, "{}", self.name)?;
        if let Some(effective) = &self.effective {
            write!(f, " {effective}")?;
        }
        Ok(())
    }
}

/// One temporal spec version from [`Engine::specs`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SpecVersion {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effective_from: Option<DateTimeValue>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effective_to: Option<DateTimeValue>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub dependencies: Vec<SpecDependency>,
}

/// Half-open window as `lemma specs` / `lemma show` print it, same `lo...hi`
/// notation as range values. Open start is `...to`; open end is the start date
/// alone (matches `spec name date`); both absent prints nothing.
pub(crate) fn write_effective_window(
    f: &mut impl std::fmt::Write,
    from: Option<&DateTimeValue>,
    to: Option<&DateTimeValue>,
) -> std::fmt::Result {
    match (from, to) {
        (Some(from), Some(to)) => write!(f, " {from}...{to}"),
        (Some(from), None) => write!(f, " {from}"),
        (None, Some(to)) => write!(f, " ...{to}"),
        (None, None) => Ok(()),
    }
}

impl std::fmt::Display for SpecVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)?;
        write_effective_window(f, self.effective_from.as_ref(), self.effective_to.as_ref())
    }
}

/// Repository group from [`Engine::specs`].
///
/// [`Self::repository`] is absent for the default unnamed repository.
/// [`Self::from_dependency`] is true for lemma_deps and the embedded stdlib;
/// it is omitted from JSON.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Repository {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub repository: Option<String>,
    pub specs: Vec<SpecVersion>,
    #[serde(skip)]
    pub from_dependency: bool,
}

/// Text view of [`Engine::specs`], as `lemma specs` prints it.
///
/// `all == false` (`lemma specs`): local repositories only (not lemma_deps / embedded
/// stdlib). Each temporal version is followed by its `uses` targets, indented.
/// `all == true` (`lemma specs --all`): every loaded repository; names and windows only.
///
/// Version lines are `name` plus ` from...to`, ` from`, ` ...to`, or the name alone,
/// sorted by name then `effective_from`. The unnamed repository's rows are unindented;
/// a named repository is a header with indented rows. Repositories without specs are
/// left out; groups are separated by a blank line.
#[must_use]
pub fn format_repositories(repositories: &[Repository], all: bool) -> String {
    let mut output = String::new();
    let groups = repositories
        .iter()
        .filter(|resolved| !resolved.specs.is_empty() && (all || !resolved.from_dependency));
    for (index, resolved) in groups.enumerate() {
        if index > 0 {
            output.push('\n');
        }
        let mut versions: Vec<&SpecVersion> = resolved.specs.iter().collect();
        versions.sort_by(|a, b| (&a.name, &a.effective_from).cmp(&(&b.name, &b.effective_from)));
        let indent = match resolved.repository.as_deref() {
            None => "",
            Some(repository) => {
                output.push_str(repository);
                output.push('\n');
                "  "
            }
        };
        for version in versions {
            output.push_str(indent);
            output.push_str(&version.to_string());
            output.push('\n');
            if !all {
                for dependency in &version.dependencies {
                    output.push_str(indent);
                    output.push_str("  ");
                    output.push_str(&dependency.to_string());
                    output.push('\n');
                }
            }
        }
    }
    output
}

fn uses_dependencies(spec: &LemmaSpec) -> Vec<SpecDependency> {
    spec.data
        .iter()
        .filter_map(|data| {
            data.value.uses_spec_ref().map(|spec_ref| SpecDependency {
                repository: spec_ref.repository.as_ref().map(|q| q.name.clone()),
                name: spec_ref.name.clone(),
                effective: spec_ref.effective.clone(),
            })
        })
        .collect()
}

// ─── Spec store with temporal resolution ──────────────────────────────

/// Ordered store of specs keyed by `(repository, name)` and grouped into
/// [`LemmaSpecSet`]s.
///
/// Specs with the same `(repository, name)` identity are ordered by `effective_from`.
/// A spec version's temporal end is derived from the successor spec's `effective_from`, or
/// `+∞`. The repository identity is preserved as `Arc<LemmaRepository>` — never via string
/// prefixes on the spec name. Repository names include the `@` prefix when present
/// (e.g. `"@org/repo"`). Dependency isolation is enforced at `insert_spec`: all specs
/// in a repository must share the same `dependency` provenance ID.
#[derive(Debug, Serialize, Deserialize)]
pub struct Context {
    #[serde(with = "crate::snapshot::shared::arc_key_index_map")]
    repositories: IndexMap<Arc<LemmaRepository>, IndexMap<String, LemmaSpecSet>>,
    #[serde(with = "crate::snapshot::shared::arc")]
    workspace: Arc<LemmaRepository>,
}

impl Default for Context {
    fn default() -> Self {
        Self::new()
    }
}

impl Context {
    /// Empty default repository; specs are inserted via [`Self::insert_spec`].
    pub fn new() -> Self {
        let workspace = Arc::new(LemmaRepository::new(None));
        let mut repositories = IndexMap::new();
        repositories.insert(Arc::clone(&workspace), IndexMap::new());
        Self {
            repositories,
            workspace,
        }
    }

    /// Default unnamed repository. Runtime APIs with `repository: None` use this.
    /// Stable identity; `name = None`, `dependency = None`.
    #[must_use]
    pub fn workspace(&self) -> Arc<LemmaRepository> {
        Arc::clone(&self.workspace)
    }

    /// Look up a repository by name without creating a new one.
    #[must_use]
    pub fn find_repository(&self, name: &str) -> Option<Arc<LemmaRepository>> {
        let probe = Arc::new(LemmaRepository::new(Some(name.to_string())));
        self.repositories
            .get_key_value(&probe)
            .map(|(k, _)| Arc::clone(k))
    }

    /// All spec sets, keyed by `(repository, name)`. Iteration order: repository first
    /// (insertion order), then spec name ascending.
    #[must_use]
    pub fn repositories(&self) -> &IndexMap<Arc<LemmaRepository>, IndexMap<String, LemmaSpecSet>> {
        &self.repositories
    }

    /// Flat iterator over every loaded [`LemmaSpec`] across all repositories.
    ///
    /// Used by registry resolution to discover missing `@owner/repo` qualifiers.
    pub fn iter(&self) -> impl Iterator<Item = &LemmaSpec> + '_ {
        self.repositories
            .values()
            .flat_map(|m| m.values())
            .flat_map(|ss| ss.iter_specs())
    }

    /// Look up a spec set by `(repository, name)`. Returns `None` if no such spec set
    /// is loaded.
    #[must_use]
    pub fn spec_set(&self, repository: &Arc<LemmaRepository>, name: &str) -> Option<&LemmaSpecSet> {
        let canonical_name = crate::parsing::ast::ascii_lowercase_logical_name(name.to_string());
        self.repositories
            .get(repository)
            .and_then(|m| m.get(&canonical_name))
    }

    /// Spec sets belonging to a repository. Panics if the repository is not in the map
    /// (caller must ensure it was returned by this Context).
    pub(crate) fn spec_sets_for(
        &self,
        repository: &Arc<LemmaRepository>,
    ) -> impl Iterator<Item = &LemmaSpecSet> + '_ {
        self.repositories
            .get(repository)
            .expect("BUG: repository not in context")
            .values()
    }

    fn spec_declaration_source(spec: &LemmaSpec) -> crate::parsing::source::Source {
        let source_type = spec
            .source_type
            .as_ref()
            .expect("BUG: spec must carry source_type after parse");
        crate::parsing::source::Source::new(
            source_type.clone(),
            crate::parsing::ast::Span {
                start: 0,
                end: 0,
                line: spec.start_line,
                col: 0,
            },
        )
    }

    fn duplicate_spec_path_line(spec: &LemmaSpec) -> (String, usize) {
        let source_type = spec
            .source_type
            .as_ref()
            .expect("BUG: spec must carry source_type after parse");
        (source_type.to_string(), spec.start_line)
    }

    fn duplicate_spec_errors(name: &str, incoming: &LemmaSpec, existing: &LemmaSpec) -> Vec<Error> {
        let (incoming_path, incoming_line) = Self::duplicate_spec_path_line(incoming);
        let (existing_path, existing_line) = Self::duplicate_spec_path_line(existing);
        vec![
            Error::validation(
                format!(
                    "Duplicate spec '{name}' (also declared in '{existing_path}':{existing_line})"
                ),
                Some(Self::spec_declaration_source(incoming)),
                None::<String>,
            ),
            Error::validation(
                format!(
                    "Duplicate spec '{name}' (also declared in '{incoming_path}':{incoming_line})"
                ),
                Some(Self::spec_declaration_source(existing)),
                None::<String>,
            ),
        ]
    }

    /// Insert a spec under `repository`. Enforces two invariants:
    /// 1. Dependency isolation: all specs in a repo must share the same `dependency`
    ///    provenance. A locally loaded repo cannot be merged with a dependency repo, and
    ///    two different dependencies cannot contribute to the same repo name.
    /// 2. No duplicate `(repository, name, effective_from)` triples.
    pub fn insert_spec(
        &mut self,
        repository: Arc<LemmaRepository>,
        spec: LemmaSpec,
    ) -> Result<(), Vec<Error>> {
        if let Some((existing_repo, _)) = self.repositories.get_key_value(&repository) {
            if existing_repo.dependency != repository.dependency {
                let repo_subject = match repository.name.as_deref() {
                    Some(name) => format!("Repository '{name}'"),
                    None => "The default repository".to_string(),
                };
                let existing_owner = match &existing_repo.dependency {
                    None => "a local source".to_string(),
                    Some(id) => format!("dependency '{id}'"),
                };
                let new_owner = match &repository.dependency {
                    None => "a local source".to_string(),
                    Some(id) => format!("dependency '{id}'"),
                };
                return Err(vec![Error::validation_with_context(
                    format!(
                        "{repo_subject} was introduced by {existing_owner} but {new_owner} also declares it"
                    ),
                    None,
                    Some("Each dependency's repositories must be unique across all loaded sources"),
                    Some(&spec),
                    None,
                )]);
            }
        }

        let entry = self
            .repositories
            .entry(Arc::clone(&repository))
            .or_default();
        if let Some(ss) = entry.get(&spec.name) {
            if let Some(existing) = ss.get_exact(spec.effective_from()) {
                return Err(Self::duplicate_spec_errors(&spec.name, &spec, existing));
            }
        }

        let name = spec.name.clone();
        if !entry
            .entry(name.clone())
            .or_insert_with(|| LemmaSpecSet::new(repository, name))
            .insert(spec)
        {
            unreachable!("BUG: duplicate effective_from rejected above");
        }
        Ok(())
    }

    pub fn remove_spec(&mut self, repository: &Arc<LemmaRepository>, spec: &LemmaSpec) -> bool {
        self.remove_spec_by_identity(repository, &spec.name, spec.effective_from())
    }

    /// Remove by `(repository, name, effective_from)` without needing a live `&LemmaSpec`.
    pub fn remove_spec_by_identity(
        &mut self,
        repository: &Arc<LemmaRepository>,
        name: &str,
        effective_from: Option<&DateTimeValue>,
    ) -> bool {
        let Some(inner) = self.repositories.get_mut(repository) else {
            return false;
        };
        let Some(ss) = inner.get_mut(name) else {
            return false;
        };
        if !ss.remove(effective_from) {
            return false;
        }
        if ss.is_empty() {
            inner.shift_remove(name);
        }
        true
    }
}

// ─── Engine ──────────────────────────────────────────────────────────

/// One mutation in a transactional [`Engine::apply`] batch.
///
/// Removes are applied before loads/replaces so identity swaps cannot hit the
/// duplicate-spec error or a mid-batch missing-dep replan. A batch is only
/// `Remove`s, only `Load`s, or a single `Replace` — never `Load` mixed with `Replace`.
enum Mutation {
    Remove {
        repository: Option<String>,
        spec: String,
        effective_from: EffectiveDate,
    },
    Load {
        source_type: SourceType,
        code: String,
    },
    /// Identity upsert from `code`, then prune other live specs with this
    /// `source_type` when it is [`SourceType::Path`] or [`SourceType::Dependency`].
    Replace {
        repository: Option<String>,
        source_type: SourceType,
        code: String,
    },
}

type StagedSpec = (SourceType, Arc<LemmaRepository>, LemmaSpec);

/// Engine for evaluating Lemma rules.
///
/// Pure Rust implementation that evaluates Lemma specs directly from the AST.
/// Uses pre-built execution plans that are self-contained and ready for evaluation.
///
/// The engine never performs network calls. External `@` references must be
/// pre-resolved (include dependency sources in the source map, or drive
/// [`crate::Resolve`] with a host [`crate::HttpTransport`]) before loading.
#[derive(Serialize, Deserialize)]
pub struct Engine {
    pub(crate) context: Context,
    pub(crate) plans: PlanStore,
    limits: ResourceLimits,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    pub fn new() -> Self {
        Self::with_limits(ResourceLimits::default())
    }

    pub fn with_limits(limits: ResourceLimits) -> Self {
        let mut engine = Self {
            context: Context::new(),
            plans: PlanStore::new(),
            limits,
        };
        engine
            .apply(
                vec![Mutation::Load {
                    source_type: SourceType::Dependency(EMBEDDED_STDLIB_REPOSITORY.to_string()),
                    code: crate::stdlib::UNITS_LEMMA.to_string(),
                }],
                true,
            )
            .expect("BUG: embedded stdlib must load");
        engine
    }

    /// Resource limits configured for this engine.
    pub fn limits(&self) -> &ResourceLimits {
        &self.limits
    }

    /// Instance-list source row counts and Arc pointers by path for `spec`.
    ///
    /// Each entry is `(path, row_count, Arc::as_ptr as usize)`. Diagnostic for
    /// Load-cost investigation. Not a supported consumer API.
    #[doc(hidden)]
    pub fn debug_instance_list_row_counts(
        &self,
        repository: Option<&str>,
        spec: &str,
    ) -> Vec<(String, usize, usize)> {
        let Some(plan) = self.plans.get_plan(
            repository,
            spec,
            &EffectiveDate::DateTimeValue(DateTimeValue::now()),
        ) else {
            return Vec::new();
        };
        let mut rows: Vec<(String, usize, usize)> = plan
            .instance_lists
            .iter()
            .map(|(path, source)| {
                let label = if path.segments.is_empty() {
                    path.data.clone()
                } else {
                    let mut parts: Vec<&str> = path
                        .segments
                        .iter()
                        .map(|segment| segment.uses.as_str())
                        .collect();
                    parts.push(path.data.as_str());
                    parts.join(".")
                };
                (
                    label,
                    source.rows.len(),
                    std::sync::Arc::as_ptr(&source.rows) as usize,
                )
            })
            .collect();
        rows.sort_by(|left, right| left.0.cmp(&right.0));
        rows
    }

    /// Named-`uses` / list row-template Arc pointers for `spec`.
    ///
    /// Each entry is `(holder, identity, Arc::as_ptr as usize)`. `holder` is empty
    /// for templates on `spec`'s plan; otherwise it is the template identity that
    /// owns the nested entry (one level). Diagnostic for Load-cost investigation.
    /// Not a supported consumer API.
    #[doc(hidden)]
    pub fn debug_row_template_arc_ptrs(
        &self,
        repository: Option<&str>,
        spec: &str,
    ) -> Vec<(String, String, usize)> {
        let Some(plan) = self.plans.get_plan(
            repository,
            spec,
            &EffectiveDate::DateTimeValue(DateTimeValue::now()),
        ) else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        for (identity, template) in &plan.row_templates {
            let identity_label = identity.to_string();
            rows.push((
                String::new(),
                identity_label.clone(),
                std::sync::Arc::as_ptr(template) as usize,
            ));
            for (nested_id, nested) in &template.row_templates {
                rows.push((
                    identity_label.clone(),
                    nested_id.to_string(),
                    std::sync::Arc::as_ptr(nested) as usize,
                ));
            }
        }
        rows.sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));
        rows
    }

    /// Serialize this engine (parsed specs + execution plans + limits) to bytes.
    ///
    /// Format: postcard header (`LEMS` magic + crate version) then a CRC32-protected
    /// postcard body. Same sources loaded into two engines produce identical bytes.
    ///
    /// Typical use: `std::fs::write(path, engine.snapshot()?)` then later
    /// `Engine::from_snapshot(&std::fs::read(path)?)`.
    pub fn snapshot(&self) -> Result<Vec<u8>, Error> {
        crate::snapshot::encode(self)
    }

    /// Restore an engine from [`Self::snapshot`] bytes.
    ///
    /// Rejects wrong magic, engine version mismatch, CRC failure, or corrupt body
    /// with [`Error`]. Does not re-load the embedded stdlib (it is inside the snapshot).
    /// After restore, `run` / `show` / `specs` / `update` / `remove` work as on a live engine.
    pub fn from_snapshot(bytes: &[u8]) -> Result<Self, Error> {
        crate::snapshot::decode(bytes)
    }

    /// Load Lemma sources in one planning pass. Pairs are `(source_type, source_text)`.
    ///
    /// Provenance is derived solely from [`SourceType`]: [`SourceType::Path`] and
    /// [`SourceType::Volatile`] are local; [`SourceType::Dependency`] tags
    /// repositories with that dependency id.
    pub fn load(
        &mut self,
        sources: impl IntoIterator<Item = (SourceType, impl Into<String>)>,
    ) -> Result<(), Errors> {
        let mutations = sources
            .into_iter()
            .map(|(source_type, code)| Mutation::Load {
                source_type,
                code: code.into(),
            })
            .collect();
        self.apply(mutations, false)
    }

    /// Replace identities present in `code` in a single planning pass.
    ///
    /// Parses `code` under `source_type`. Each parsed identity is inserted or
    /// replaced by exact `(repository, name, effective_from)`. For
    /// [`SourceType::Path`] and [`SourceType::Dependency`], live specs with the
    /// same `source_type` that are absent from `code` are removed in the same
    /// apply — including when `code` has zero specs (remove every live row of
    /// that source). [`SourceType::Volatile`] never prunes siblings and requires
    /// at least one spec in `code`.
    ///
    /// When `repository` is `Some(name)`, every staged spec's repository name must
    /// equal `name`.
    pub fn update(
        &mut self,
        repository: Option<&str>,
        code: String,
        source_type: SourceType,
    ) -> Result<(), Errors> {
        self.apply(
            vec![Mutation::Replace {
                repository: repository.map(str::to_string),
                source_type,
                code,
            }],
            false,
        )
    }

    /// Remove a temporal spec slice and replan remaining specs.
    pub fn remove(
        &mut self,
        repository: Option<&str>,
        spec: &str,
        effective: Option<&DateTimeValue>,
    ) -> Result<(), Error> {
        let resolved = self.get_spec(spec, repository, effective)?;
        let effective_from = resolved.effective_from.clone();
        self.apply(
            vec![Mutation::Remove {
                repository: repository.map(str::to_string),
                spec: spec.to_string(),
                effective_from,
            }],
            false,
        )
        .map_err(|errs| {
            errs.errors
                .into_iter()
                .next()
                .expect("BUG: apply Errors must contain at least one error")
        })
    }

    /// Every loaded repository in insertion order (default unnamed repository, embedded stdlib [`EMBEDDED_STDLIB_REPOSITORY`], dependencies).
    ///
    /// Returns spec version rows (metadata only, no AST, no source text).
    #[must_use]
    pub fn specs(&self) -> Vec<Repository> {
        self.context
            .repositories()
            .iter()
            .map(|(repo, inner)| {
                let specs = inner
                    .values()
                    .flat_map(|spec_set| {
                        spec_set
                            .iter_with_ranges()
                            .map(|(spec, from, to)| SpecVersion {
                                name: spec.name.clone(),
                                effective_from: from,
                                effective_to: to,
                                dependencies: uses_dependencies(spec),
                            })
                    })
                    .collect();
                Repository {
                    repository: repo.name.clone(),
                    specs,
                    from_dependency: repo.dependency.is_some(),
                }
            })
            .collect()
    }

    /// Spec interface and resolved temporal window at `effective`.
    ///
    /// `Show.data` lists every declared promptable slot. Empty
    /// [`ShowData::needed_by_rules`] means offered for reuse (`data x: alias.slot`),
    /// not needed by this spec's remaining rules after normalize.
    /// Lemma source text is [`Self::source`].
    pub fn show(
        &self,
        repository: Option<&str>,
        spec: &str,
        effective: Option<&DateTimeValue>,
    ) -> Result<Show, Error> {
        let effective_dt = self.effective_or_now(effective);
        let instant = EffectiveDate::DateTimeValue(effective_dt.clone());

        let plan = match self.plans.get_plan(repository, spec, &instant) {
            Some(plan) => plan,
            None => {
                // Preserve attributed not-found errors (repository vs spec) without
                // paying for SpecSet lookup on the common success path.
                let repository_arc = self.repository_for_lookup(repository)?;
                let canonical_name =
                    crate::parsing::ast::ascii_lowercase_logical_name(spec.to_string());
                let spec_set = self.context.spec_set(&repository_arc, &canonical_name);
                return match spec_set.and_then(|ss| ss.spec_at(&instant)) {
                    None => Err(self.spec_not_found_in_repository_error(
                        &repository_arc,
                        spec,
                        &effective_dt,
                    )),
                    Some(_) => Err(Error::request_not_found(
                        format!(
                            "No execution plan slice for spec '{spec}' at effective {effective_dt}"
                        ),
                        Some("Ensure sources loaded and planning succeeded".to_string()),
                    )),
                };
            }
        };
        Ok(self.show_plan(plan, repository))
    }

    /// Declarations of `spec` at `effective` as loaded from source, including data and
    /// rules that no other rule needs. Unlike [`Self::show`], nothing is filtered by use.
    pub fn spec_declaration(
        &self,
        repository: Option<&str>,
        spec: &str,
        effective: Option<&DateTimeValue>,
    ) -> Result<&LemmaSpec, Error> {
        let effective_dt = self.effective_or_now(effective);
        let instant = EffectiveDate::DateTimeValue(effective_dt.clone());
        let repository_arc = self.repository_for_lookup(repository)?;
        let canonical_name = crate::parsing::ast::ascii_lowercase_logical_name(spec.to_string());
        match self
            .context
            .spec_set(&repository_arc, &canonical_name)
            .and_then(|spec_set| spec_set.spec_at(&instant))
        {
            Some(found) => Ok(found),
            None => {
                Err(self.spec_not_found_in_repository_error(&repository_arc, spec, &effective_dt))
            }
        }
    }

    /// Interface of the resolved spec `identity` names, e.g. the row spec of a list type.
    /// Identities come from loaded plans, so the plan exists.
    pub fn show_spec(&self, identity: &SpecIdentity) -> Show {
        let plan = self
            .plans
            .get_plan(
                identity.repository.as_deref(),
                &identity.spec,
                &identity.effective,
            )
            .unwrap_or_else(|| panic!("BUG: no plan for resolved spec '{identity}'"));
        self.show_plan(plan, identity.repository.as_deref())
    }

    /// Plan that produced `show` (repo + spec + start line + slice effective).
    /// `None` when no plans are loaded for that spec (synthetic Show in tests).
    pub(crate) fn plan_matching_show(
        &self,
        show: &crate::planning::execution_plan::Show,
    ) -> Option<&crate::planning::execution_plan::ExecutionPlan> {
        let plans = self
            .plans
            .get_plans(show.repository.as_deref(), &show.spec)?;
        let mut matches = plans.values().filter(|plan| {
            plan.spec_name == show.spec
                && plan.start_line == show.start_line
                && plan.effective_from == show.effective_from
                && plan.effective_to == show.effective_to
                && plan.effective == show.plan_effective
        });
        let first = matches.next()?;
        if matches.next().is_some() {
            panic!(
                "BUG: multiple plans match Show '{}' (start_line {}, effective window, plan_effective)",
                show.spec, show.start_line
            );
        }
        Some(first)
    }

    fn show_plan(&self, plan: &ExecutionPlan, repository: Option<&str>) -> Show {
        let mut type_entries: Vec<(usize, String, crate::planning::semantics::LemmaType)> =
            Vec::new();
        let mut data_entries: Vec<(usize, usize, String, ShowData)> = Vec::new();

        for (path, definition) in plan.nested_input_keys() {
            let Some(lemma_type) = definition.schema_type().cloned() else {
                continue;
            };
            if matches!(definition, DataDefinition::Reference { .. }) {
                continue;
            }
            let input_key = path.input_key();
            if path.segments.is_empty()
                && matches!(
                    definition,
                    DataDefinition::Value { .. } | DataDefinition::TypeDeclaration { .. }
                )
            {
                type_entries.push((
                    definition.source().span.start,
                    input_key.clone(),
                    lemma_type.clone(),
                ));
            }
            let rule_positions = plan.data_needed_by(&path);
            if rule_positions.is_empty() {
                continue;
            }
            let used_by: Vec<String> = rule_positions
                .iter()
                .map(|&rule_position| {
                    plan.rules
                        .get_index(rule_position as usize)
                        .expect("BUG: needed_by_rules position out of plan.rules range")
                        .1
                        .path
                        .input_key()
                })
                .collect();
            let display = plan.data_display.get(&path);
            data_entries.push((
                path.segments.len(),
                definition.source().span.start,
                input_key.clone(),
                ShowData {
                    lemma_type: lemma_type.clone(),
                    path: path.segments.clone(),
                    value: match definition {
                        DataDefinition::InstanceList { .. } => {
                            crate::planning::execution_plan::show_instance_list_value(plan, &path)
                        }
                        _ => display.and_then(|d| {
                            d.value.as_ref().map(|bound| {
                                crate::planning::execution_plan::show_bound_result_value(
                                    bound,
                                    &lemma_type,
                                    &input_key,
                                    crate::planning::execution_plan::ShowBoundRole::Value,
                                )
                            })
                        }),
                    },
                    suggestion: display.and_then(|d| {
                        d.suggestion.as_ref().map(|bound| {
                            crate::planning::execution_plan::show_bound_result_value(
                                bound,
                                &lemma_type,
                                &input_key,
                                crate::planning::execution_plan::ShowBoundRole::Suggestion,
                            )
                        })
                    }),
                    needed_by_rules: used_by,
                },
            ));
        }
        type_entries.sort_by_key(|(pos, _, _)| *pos);
        data_entries.sort_by_key(|(depth, pos, _, _)| (*depth, *pos));

        let rule_entries: Vec<(String, crate::planning::show_expression::ShowRule)> = plan
            .show_rules
            .iter()
            .map(|(path, rule)| (path.input_key(), rule.clone()))
            .collect();

        let show_repository = match repository {
            None => None,
            Some(q) => {
                let repo = self
                    .context
                    .find_repository(q)
                    .expect("BUG: get_plan succeeded so that repository must be loaded");
                Some(
                    repo.name
                        .clone()
                        .expect("BUG: named repository cannot have name None"),
                )
            }
        };

        Show {
            repository: show_repository,
            spec: plan.spec_name.clone(),
            commentary: plan.commentary.clone(),
            effective_from: plan.effective_from.clone(),
            effective_to: plan.effective_to.clone(),
            plan_effective: plan.effective.clone(),
            start_line: plan.start_line,
            source_type: plan.source_type.clone(),
            types: type_entries
                .into_iter()
                .map(|(_, name, lemma_type)| (name, lemma_type))
                .collect(),
            data: data_entries
                .into_iter()
                .map(|(_, _, name, entry)| (name, entry))
                .collect(),
            rules: rule_entries.into_iter().collect(),
            meta: plan.meta.clone(),
        }
    }

    /// Formatted canonical Lemma source for a repository or one spec slice.
    ///
    /// When `spec` is `None`, returns all specs in the repository sorted by name/effective.
    /// When `spec` is `Some`, `effective` selects the temporal slice (default: now).
    pub fn source(
        &self,
        repository: Option<&str>,
        spec: Option<&str>,
        effective: Option<&DateTimeValue>,
    ) -> Result<String, Error> {
        match spec {
            None => self.format_repository_source(repository),
            Some(spec_name) => {
                let effective_dt = self.effective_or_now(effective);
                let resolved_spec = self.get_spec(spec_name, repository, Some(&effective_dt))?;
                Ok(crate::formatting::format_spec_refs(&[resolved_spec]))
            }
        }
    }

    /// Evaluate a spec.
    pub fn evaluate(
        &self,
        repository: Option<&str>,
        spec: &str,
        effective: Option<&DateTimeValue>,
        data: IndexMap<String, EvaluateDataValue>,
        rules: Option<&[String]>,
        explain: bool,
    ) -> Result<Response, Error> {
        let effective = self.effective_or_now(effective);
        let instant = EffectiveDate::DateTimeValue(effective.clone());

        let plan = self
            .plans
            .get_plan(repository, spec, &instant)
            .ok_or_else(|| {
                Error::request_not_found(
                    format!("No execution plan for spec '{spec}' at effective {effective}"),
                    Some("Ensure sources loaded and planning succeeded".to_string()),
                )
            })?;

        let response_rules = plan.validated_response_rule_paths(rules)?;
        let evaluate_data = EvaluateData::resolve(plan, data, &self.limits)?;
        let now_semantic = crate::planning::semantics::date_time_to_semantic(&effective);
        let now_literal = crate::planning::semantics::LiteralValue::date(now_semantic);
        let evaluator = Evaluator;
        let mut response =
            evaluator.evaluate(plan, &evaluate_data, now_literal, &response_rules, explain);

        response.spec_effective_from = plan.effective_from.clone();
        response.spec_effective_to = plan.effective_to.clone();

        Ok(response)
    }

    /// Deprecated alias of [`Engine::evaluate`].
    #[deprecated(note = "use evaluate")]
    pub fn run(
        &self,
        repository: Option<&str>,
        spec: &str,
        effective: Option<&DateTimeValue>,
        data: IndexMap<String, EvaluateDataValue>,
        rules: Option<&[String]>,
        explain: bool,
    ) -> Result<Response, Error> {
        self.evaluate(repository, spec, effective, data, rules, explain)
    }

    fn format_repository_source(&self, repository: Option<&str>) -> Result<String, Error> {
        let repo_arc = self.resolve_repository(repository)?;
        let mut all_specs: Vec<&LemmaSpec> = self
            .context
            .spec_sets_for(&repo_arc)
            .flat_map(|ss| ss.iter_specs())
            .collect();
        all_specs.sort_by(|a, b| {
            a.name
                .cmp(&b.name)
                .then_with(|| a.effective_from.cmp(&b.effective_from))
        });
        let body = crate::formatting::format_spec_refs(&all_specs);
        let mut source_text = String::new();
        if let Some(name) = repo_arc.name.as_deref() {
            source_text.push_str("repo ");
            source_text.push_str(name);
            source_text.push_str("\n\n");
        }
        source_text.push_str(&body);
        Ok(source_text)
    }

    fn resolve_repository(&self, repository: Option<&str>) -> Result<Arc<LemmaRepository>, Error> {
        match repository {
            None => Ok(self.context.workspace()),
            Some(qualifier) => {
                let q = qualifier.trim();
                if q.is_empty() {
                    return Err(Error::request(
                        "Repository qualifier cannot be empty",
                        None::<String>,
                    ));
                }
                self.context.find_repository(q).ok_or_else(|| {
                    Error::request_not_found(
                        format!("Repository '{qualifier}' not loaded"),
                        Some(format!(
                            "List repositories with `{}` after loading your workspace",
                            "lemma specs"
                        )),
                    )
                })
            }
        }
    }

    fn spec_not_found_in_repository_error(
        &self,
        repository: &LemmaRepository,
        spec_name: &str,
        effective: &DateTimeValue,
    ) -> Error {
        let message = match &repository.name {
            Some(n) => {
                format!("Spec '{spec_name}' not found in repository {n} at effective {effective}")
            }
            None => format!("Spec '{spec_name}' not found at effective {effective}"),
        };
        Error::request_not_found(message, Some("Try `lemma specs`"))
    }

    fn repository_for_lookup(
        &self,
        repository: Option<&str>,
    ) -> Result<Arc<LemmaRepository>, Error> {
        match repository {
            Some(q) => self.context.find_repository(q).ok_or_else(|| {
                Error::request_not_found(
                    format!("Repository '{q}' not loaded"),
                    Some("List repositories with `lemma specs` after loading your workspace"),
                )
            }),
            None => Ok(self.context.workspace()),
        }
    }

    /// Effective datetime for a request: `explicit` or now.
    #[must_use]
    fn effective_or_now(&self, effective: Option<&DateTimeValue>) -> DateTimeValue {
        effective.cloned().unwrap_or_else(DateTimeValue::now)
    }

    fn reserved_stdlib_error(source: Option<crate::parsing::source::Source>) -> Error {
        Error::validation(
            format!(
                "Repository '{EMBEDDED_STDLIB_REPOSITORY}' is reserved for the embedded standard library and cannot be loaded via load; use @owner/repo qualifiers (e.g. '@iso/countries'), not the reserved 'lemma' repository"
            ),
            source,
            Some(
                "Load registry dependencies with @owner/repo qualifiers, not the reserved 'lemma' stdlib repository"
                    .to_string(),
            ),
        )
    }

    fn resource_limit_errors(
        name: &str,
        limit: impl ToString,
        actual: impl ToString,
        hint: &str,
        sources: IndexMap<SourceType, String>,
    ) -> Errors {
        Errors {
            errors: vec![Error::resource_limit_exceeded(
                name,
                limit.to_string(),
                actual.to_string(),
                hint,
                None::<crate::parsing::source::Source>,
                None,
                None,
            )],
            sources: sources.into_iter().collect(),
        }
    }

    /// Apply removes then loads/replaces in one planning pass. Rolls back the whole batch on failure.
    ///
    /// Load sources are order-preserving so multi-source parse errors are reported in
    /// submission order rather than scrambled by hash iteration.
    fn apply(&mut self, mutations: Vec<Mutation>, embedded_stdlib: bool) -> Result<(), Errors> {
        let mut sources: IndexMap<SourceType, String> = IndexMap::new();
        let mut to_restore: Vec<(Arc<LemmaRepository>, LemmaSpec)> = Vec::new();
        let mut replace: Option<(Option<String>, SourceType, String)> = None;
        let mut saw_load = false;
        let mut saw_remove = false;

        for mutation in mutations {
            match mutation {
                Mutation::Remove {
                    repository,
                    spec,
                    effective_from,
                } => {
                    let repo_ref = repository.as_deref();
                    let repository_arc = self.resolve_repository(repo_ref).unwrap_or_else(|e| {
                        panic!(
                            "BUG: Mutation::Remove repository must resolve after public remove validated it: {e}"
                        )
                    });
                    let spec_to_remove = self
                        .context
                        .spec_set(&repository_arc, &spec)
                        .and_then(|ss| ss.get_exact(effective_from.as_ref()))
                        .unwrap_or_else(|| {
                            panic!(
                                "BUG: Mutation::Remove target '{spec}' must exist after public remove validated it"
                            )
                        });
                    saw_remove = true;
                    to_restore.push((Arc::clone(&repository_arc), spec_to_remove.clone()));
                }
                Mutation::Load { source_type, code } => {
                    saw_load = true;
                    if replace.is_some() {
                        panic!("BUG: Load and Replace in one apply");
                    }
                    if sources.insert(source_type.clone(), code).is_some() {
                        return Err(Errors {
                            errors: vec![Error::request(
                                format!("Duplicate source key: {source_type}"),
                                None::<String>,
                            )],
                            sources: sources.into_iter().collect(),
                        });
                    }
                }
                Mutation::Replace {
                    repository,
                    source_type,
                    code,
                } => {
                    if saw_load || !sources.is_empty() {
                        panic!("BUG: Load and Replace in one apply");
                    }
                    if replace.is_some() {
                        panic!("BUG: multiple Replace mutations in one apply");
                    }
                    replace = Some((repository, source_type, code));
                }
            }
        }

        if saw_remove && (saw_load || replace.is_some()) {
            panic!("BUG: Remove mixed with Load or Replace in one apply");
        }

        if let Some((repo_constraint, source_type, code)) = replace {
            return self.apply_replace(
                repo_constraint.as_deref(),
                source_type,
                code,
                embedded_stdlib,
            );
        }

        let sources_map: HashMap<SourceType, String> = sources.clone().into_iter().collect();
        for st in sources.keys() {
            if let Err(e) = Self::validate_source_type_key(st, embedded_stdlib) {
                return Err(Errors {
                    errors: vec![e],
                    sources: sources_map,
                });
            }
        }
        self.check_batch_limits(&sources, embedded_stdlib)?;

        let parse_limits = if embedded_stdlib {
            &ResourceLimits::default()
        } else {
            &self.limits
        };
        let mut staged: Vec<StagedSpec> = Vec::new();
        let mut errors: Vec<Error> = Vec::new();

        for (source_id, code) in &sources {
            match self.stage_parsed_source(source_id, code, parse_limits, embedded_stdlib) {
                Ok(chunk) => staged.extend(chunk),
                Err(es) => errors.extend(es),
            }
        }

        if !errors.is_empty() {
            return Err(Errors {
                errors,
                sources: sources.into_iter().collect(),
            });
        }

        self.commit_staged(to_restore, staged, sources.into_iter().collect())
    }

    fn validate_source_type_key(
        source_type: &SourceType,
        embedded_stdlib: bool,
    ) -> Result<(), Error> {
        match source_type {
            SourceType::Path(p) if p.as_os_str().to_string_lossy().trim().is_empty() => Err(
                Error::request("Source path must be non-empty", None::<String>),
            ),
            SourceType::Dependency(id) if id.is_empty() => Err(Error::request(
                "Dependency source identifier must be non-empty",
                None::<String>,
            )),
            SourceType::Dependency(id) if !embedded_stdlib && id == EMBEDDED_STDLIB_REPOSITORY => {
                Err(Self::reserved_stdlib_error(None))
            }
            _ => Ok(()),
        }
    }

    fn check_batch_limits(
        &self,
        sources: &IndexMap<SourceType, String>,
        embedded_stdlib: bool,
    ) -> Result<(), Errors> {
        if embedded_stdlib || sources.is_empty() {
            return Ok(());
        }
        let limits = &self.limits;
        if sources.len() > limits.max_sources {
            return Err(Self::resource_limit_errors(
                "max_sources",
                limits.max_sources,
                sources.len(),
                "Reduce the number of paths or sources in one load",
                sources.clone(),
            ));
        }
        let total_loaded_bytes: usize = sources.values().map(|s| s.len()).sum();
        if total_loaded_bytes > limits.max_loaded_bytes {
            return Err(Self::resource_limit_errors(
                "max_loaded_bytes",
                limits.max_loaded_bytes,
                total_loaded_bytes,
                "Load fewer or smaller sources",
                sources.clone(),
            ));
        }
        if let Some(code) = sources
            .values()
            .find(|code| code.len() > limits.max_source_size_bytes)
        {
            return Err(Self::resource_limit_errors(
                "max_source_size_bytes",
                limits.max_source_size_bytes,
                code.len(),
                "Use a smaller source text or increase limit",
                sources.clone(),
            ));
        }
        Ok(())
    }

    fn stage_parsed_source(
        &self,
        source_id: &SourceType,
        code: &str,
        parse_limits: &ResourceLimits,
        embedded_stdlib: bool,
    ) -> Result<Vec<StagedSpec>, Vec<Error>> {
        let dependency = match source_id {
            SourceType::Dependency(id) => Some(id.as_str()),
            _ => None,
        };
        let result = parse(code, source_id.clone(), parse_limits).map_err(|e| vec![e])?;
        if result.repositories.is_empty() {
            return Ok(Vec::new());
        }

        let mut staged = Vec::new();
        let mut errors = Vec::new();
        for (parsed_repo, specs) in result.repositories {
            let repository_arc = if let Some(dep_id) = dependency {
                let repo_name = parsed_repo
                    .name
                    .clone()
                    .or_else(|| Some(dep_id.to_string()));
                Arc::new(
                    LemmaRepository::new(repo_name)
                        .with_dependency(dep_id)
                        .with_start_line(parsed_repo.start_line),
                )
            } else {
                parsed_repo
            };
            if !embedded_stdlib
                && repository_arc.name.as_deref() == Some(EMBEDDED_STDLIB_REPOSITORY)
            {
                let source = crate::parsing::source::Source::new(
                    source_id.clone(),
                    crate::parsing::ast::Span {
                        start: 0,
                        end: 0,
                        line: repository_arc.start_line,
                        col: 0,
                    },
                );
                errors.push(Self::reserved_stdlib_error(Some(source)));
                continue;
            }
            for spec in specs {
                staged.push((source_id.clone(), Arc::clone(&repository_arc), spec));
            }
        }
        if !errors.is_empty() {
            return Err(errors);
        }
        Ok(staged)
    }

    fn apply_replace(
        &mut self,
        repository_constraint: Option<&str>,
        source_type: SourceType,
        code: String,
        embedded_stdlib: bool,
    ) -> Result<(), Errors> {
        let mut sources: IndexMap<SourceType, String> = IndexMap::new();
        sources.insert(source_type.clone(), code.clone());

        if let Err(e) = Self::validate_source_type_key(&source_type, embedded_stdlib) {
            return Err(Errors {
                errors: vec![e],
                sources: sources.into_iter().collect(),
            });
        }

        self.check_batch_limits(&sources, embedded_stdlib)?;

        let parse_limits = if embedded_stdlib {
            &ResourceLimits::default()
        } else {
            &self.limits
        };

        let staged =
            match self.stage_parsed_source(&source_type, &code, parse_limits, embedded_stdlib) {
                Ok(s) => s,
                Err(errors) => {
                    return Err(Errors {
                        errors,
                        sources: sources.into_iter().collect(),
                    });
                }
            };

        let prune = matches!(source_type, SourceType::Path(_) | SourceType::Dependency(_));
        if staged.is_empty() && !prune {
            return Err(Errors {
                errors: vec![Error::request(
                    "update requires at least one spec",
                    None::<String>,
                )],
                sources: sources.into_iter().collect(),
            });
        }

        if let Some(required) = repository_constraint {
            let required_canonical =
                crate::parsing::ast::ascii_lowercase_logical_name(required.to_string());
            for (_, repository_arc, _) in &staged {
                if repository_arc.name.as_deref() != Some(required_canonical.as_str()) {
                    return Err(Errors {
                        errors: vec![Error::request(
                            match repository_arc.name.as_deref() {
                                Some(name) => format!(
                                    "update repository '{required}' does not match staged repository '{name}'"
                                ),
                                None => format!(
                                    "update repository '{required}' does not match the default repository"
                                ),
                            },
                            None::<String>,
                        )],
                        sources: sources.into_iter().collect(),
                    });
                }
            }
        }

        let mut staged_keys: std::collections::HashSet<(Option<String>, String, EffectiveDate)> =
            std::collections::HashSet::new();
        for (_, repository_arc, spec) in &staged {
            staged_keys.insert((
                repository_arc.name.clone(),
                spec.name.clone(),
                spec.effective_from.clone(),
            ));
        }

        let mut to_restore: Vec<(Arc<LemmaRepository>, LemmaSpec)> = Vec::new();
        for (repository, by_name) in self.context.repositories() {
            for spec_set in by_name.values() {
                for spec in spec_set.iter_specs() {
                    if spec.source_type.as_ref() != Some(&source_type) {
                        continue;
                    }
                    let key = (
                        repository.name.clone(),
                        spec.name.clone(),
                        spec.effective_from.clone(),
                    );
                    if prune || staged_keys.contains(&key) {
                        to_restore.push((Arc::clone(repository), spec.clone()));
                    }
                }
            }
        }

        let staged_for_commit: Vec<StagedSpec> = staged
            .into_iter()
            .map(|(_, repo, spec)| (source_type.clone(), repo, spec))
            .collect();

        self.commit_staged(to_restore, staged_for_commit, sources.into_iter().collect())
    }

    fn commit_staged(
        &mut self,
        to_restore: Vec<(Arc<LemmaRepository>, LemmaSpec)>,
        staged: Vec<StagedSpec>,
        sources: HashMap<SourceType, String>,
    ) -> Result<(), Errors> {
        for (repo, spec) in &to_restore {
            if !self.context.remove_spec(repo, spec) {
                panic!("BUG: to_restore row must be live in context");
            }
        }

        let mut inserted: Vec<(Arc<LemmaRepository>, String, EffectiveDate)> = Vec::new();
        let mut errors: Vec<Error> = Vec::new();
        for (_, repository_arc, spec) in staged {
            let name = spec.name.clone();
            let effective_from = spec.effective_from.clone();
            match self.context.insert_spec(Arc::clone(&repository_arc), spec) {
                Ok(()) => inserted.push((repository_arc, name, effective_from)),
                Err(es) => errors.extend(es),
            }
        }
        if !errors.is_empty() {
            self.rollback_apply(&inserted, &to_restore);
            return Err(Errors { errors, sources });
        }

        let result = crate::planning::plan(&self.context, &self.limits, &self.plans);
        if !result.errors.is_empty() {
            self.rollback_apply(&inserted, &to_restore);
            return Err(Errors {
                errors: result.errors,
                sources,
            });
        }

        self.plans = result.plans;
        Ok(())
    }

    fn rollback_apply(
        &mut self,
        inserted: &[(Arc<LemmaRepository>, String, EffectiveDate)],
        removed: &[(Arc<LemmaRepository>, LemmaSpec)],
    ) {
        for (repo, inserted_name, inserted_effective) in inserted.iter().rev() {
            if !self.context.remove_spec_by_identity(
                repo,
                inserted_name,
                inserted_effective.as_ref(),
            ) {
                panic!("BUG: inserted spec must be removable on rollback");
            }
        }
        for (repo, spec) in removed.iter().rev() {
            self.context
                .insert_spec(Arc::clone(repo), spec.clone())
                .expect("BUG: restore removed spec for rollback");
        }
    }

    /// Active [`LemmaSpec`] slice for `name` at the resolved effective instant in `repository`.
    ///
    /// When `repository` is `None`, uses the default unnamed repository. When `effective` is `None`, uses now.
    pub(crate) fn get_spec(
        &self,
        name: &str,
        repository: Option<&str>,
        effective: Option<&DateTimeValue>,
    ) -> Result<&LemmaSpec, Error> {
        let effective_dt = self.effective_or_now(effective);
        let instant = EffectiveDate::DateTimeValue(effective_dt.clone());
        let repository_arc = match repository {
            Some(q) => self.context.find_repository(q).ok_or_else(|| {
                Error::request_not_found(
                    format!("Repository '{q}' not loaded"),
                    Some("List repositories with `lemma specs` after loading your workspace"),
                )
            })?,
            None => self.context.workspace(),
        };
        let spec_set = self
            .context
            .spec_set(&repository_arc, name)
            .ok_or_else(|| {
                self.spec_not_found_in_repository_error(&repository_arc, name, &effective_dt)
            })?;
        spec_set.spec_at(&instant).ok_or_else(|| {
            self.spec_not_found_in_repository_error(&repository_arc, name, &effective_dt)
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;

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
            granularity: crate::literals::DateGranularity::Full,
        }
    }

    #[test]
    fn format_repositories_prints_each_temporal_version() {
        let text = format_repositories(
            &[
                Repository {
                    repository: None,
                    from_dependency: false,
                    specs: vec![
                        SpecVersion {
                            name: "legacy".into(),
                            effective_from: None,
                            effective_to: Some(date(2026, 1, 1)),
                            dependencies: Vec::new(),
                        },
                        SpecVersion {
                            name: "taxes".into(),
                            effective_from: None,
                            effective_to: None,
                            dependencies: Vec::new(),
                        },
                        SpecVersion {
                            name: "pricing".into(),
                            effective_from: Some(date(2026, 1, 1)),
                            effective_to: None,
                            dependencies: Vec::new(),
                        },
                        SpecVersion {
                            name: "pricing".into(),
                            effective_from: Some(date(2025, 1, 1)),
                            effective_to: Some(date(2026, 1, 1)),
                            dependencies: Vec::new(),
                        },
                    ],
                },
                Repository {
                    repository: Some("@iso/countries".into()),
                    from_dependency: true,
                    specs: vec![SpecVersion {
                        name: "alpha2".into(),
                        effective_from: Some(date(2011, 8, 9)),
                        effective_to: None,
                        dependencies: Vec::new(),
                    }],
                },
            ],
            true,
        );
        assert_eq!(
            text,
            "legacy ...2026-01-01\npricing 2025-01-01...2026-01-01\npricing 2026-01-01\ntaxes\n\n@iso/countries\n  alpha2 2011-08-09\n"
        );
    }

    #[test]
    fn spec_version_display_omits_arrow_without_an_end() {
        assert_eq!(
            SpecVersion {
                name: "pricing".into(),
                effective_from: Some(date(2025, 1, 1)),
                effective_to: Some(date(2026, 1, 1)),
                dependencies: Vec::new(),
            }
            .to_string(),
            "pricing 2025-01-01...2026-01-01"
        );
        assert_eq!(
            SpecVersion {
                name: "pricing".into(),
                effective_from: Some(date(2026, 1, 1)),
                effective_to: None,
                dependencies: Vec::new(),
            }
            .to_string(),
            "pricing 2026-01-01"
        );
        assert_eq!(
            SpecVersion {
                name: "pricing".into(),
                effective_from: None,
                effective_to: Some(date(2026, 1, 1)),
                dependencies: Vec::new(),
            }
            .to_string(),
            "pricing ...2026-01-01"
        );
        assert_eq!(
            SpecVersion {
                name: "taxes".into(),
                effective_from: None,
                effective_to: None,
                dependencies: Vec::new(),
            }
            .to_string(),
            "taxes"
        );
    }

    #[test]
    fn format_local_specs_prints_uses_and_omits_dependency_repositories() {
        let text = format_repositories(
            &[
                Repository {
                    repository: None,
                    from_dependency: false,
                    specs: vec![SpecVersion {
                        name: "outer".into(),
                        effective_from: None,
                        effective_to: None,
                        dependencies: vec![
                            SpecDependency {
                                repository: None,
                                name: "inner".into(),
                                effective: None,
                            },
                            SpecDependency {
                                repository: Some("lemma".into()),
                                name: "units".into(),
                                effective: None,
                            },
                        ],
                    }],
                },
                Repository {
                    repository: Some("lemma".into()),
                    from_dependency: true,
                    specs: vec![SpecVersion {
                        name: "units".into(),
                        effective_from: None,
                        effective_to: None,
                        dependencies: Vec::new(),
                    }],
                },
            ],
            false,
        );
        assert_eq!(text, "outer\n  inner\n  lemma units\n");
    }

    #[test]
    fn specs_rows_carry_uses_targets() {
        let mut engine = Engine::new();
        engine
            .load([(
                SourceType::Path(Arc::new(std::path::PathBuf::from("main.lemma"))),
                r#"
spec inner 2024-01-01
data x: 1

spec outer
uses inner 2024-01-01
uses u: lemma units
rule r: inner.x
"#
                .to_string(),
            )])
            .expect("load");
        let workspace = engine
            .specs()
            .into_iter()
            .find(|repo| repo.repository.is_none())
            .expect("workspace");
        let outer = workspace
            .specs
            .iter()
            .find(|version| version.name == "outer")
            .expect("outer");
        assert_eq!(
            outer.dependencies,
            vec![
                SpecDependency {
                    repository: None,
                    name: "inner".into(),
                    effective: Some(date(2024, 1, 1)),
                },
                SpecDependency {
                    repository: Some("lemma".into()),
                    name: "units".into(),
                    effective: None,
                },
            ]
        );
        assert!(engine
            .specs()
            .iter()
            .any(|repo| repo.repository.as_deref() == Some("lemma") && repo.from_dependency));
    }

    fn make_spec_with_range(name: &str, effective_from: Option<DateTimeValue>) -> LemmaSpec {
        let mut spec = LemmaSpec::new(name.to_string());
        spec.effective_from = crate::parsing::ast::EffectiveDate::from_option(effective_from);
        spec
    }

    /// Spec-set temporal order is (name, effective_from) ascending.
    /// Same-name specs appear in temporal order; definition order in the source is irrelevant.
    #[test]
    fn list_order_is_name_then_effective_from_ascending() {
        let mut ctx = Context::new();
        let repository = ctx.workspace();
        let s_2026 = make_spec_with_range("mortgage", Some(date(2026, 1, 1)));
        let s_2025 = make_spec_with_range("mortgage", Some(date(2025, 1, 1)));
        ctx.insert_spec(Arc::clone(&repository), s_2026).unwrap();
        ctx.insert_spec(Arc::clone(&repository), s_2025).unwrap();
        let listed: Vec<_> = ctx
            .spec_set(&repository, "mortgage")
            .expect("mortgage set")
            .iter_specs()
            .collect();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].effective_from(), Some(&date(2025, 1, 1)));
        assert_eq!(listed[1].effective_from(), Some(&date(2026, 1, 1)));
    }

    #[test]
    fn get_spec_resolves_temporal_version_by_effective() {
        let mut engine = Engine::new();
        engine
            .load([(
                SourceType::Path(Arc::new(std::path::PathBuf::from("a.lemma"))),
                r#"
        spec pricing 2025-01-01
        data x: 1
        rule r: x
    "#
                .to_string(),
            )])
            .unwrap();
        engine
            .load([(
                SourceType::Path(Arc::new(std::path::PathBuf::from("b.lemma"))),
                r#"
        spec pricing 2025-06-01
        data x: 2
        rule r: x
    "#
                .to_string(),
            )])
            .unwrap();

        let jan = DateTimeValue {
            year: 2025,
            month: 1,
            day: 15,
            hour: 0,
            minute: 0,
            second: 0,
            microsecond: 0,
            timezone: None,
            granularity: crate::literals::DateGranularity::Full,
        };
        let jul = DateTimeValue {
            year: 2025,
            month: 7,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            microsecond: 0,
            timezone: None,
            granularity: crate::literals::DateGranularity::Full,
        };

        let v1 = DateTimeValue {
            year: 2025,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            microsecond: 0,
            timezone: None,
            granularity: crate::literals::DateGranularity::Full,
        };
        let v2 = DateTimeValue {
            year: 2025,
            month: 6,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            microsecond: 0,
            timezone: None,
            granularity: crate::literals::DateGranularity::Full,
        };

        let s_jan = engine
            .get_spec("pricing", None, Some(&jan))
            .expect("jan spec");
        let s_jul = engine
            .get_spec("pricing", None, Some(&jul))
            .expect("jul spec");
        assert_eq!(s_jan.effective_from(), Some(&v1));
        assert_eq!(s_jul.effective_from(), Some(&v2));
    }

    /// Every temporal row for a workspace spec name exposes half-open
    /// `[effective_from, effective_to)` via [`LemmaSpecSet::iter_with_ranges`]. The latest row's
    /// `effective_to` is `None` (no successor); earlier rows' `effective_to`
    /// equals the next row's `effective_from`.
    #[test]
    fn list_returns_half_open_ranges_per_temporal_version() {
        let mut engine = Engine::new();
        engine
            .load([(
                SourceType::Path(Arc::new(std::path::PathBuf::from("a.lemma"))),
                r#"
        spec pricing 2025-01-01
        data x: 1
        rule r: x
    "#
                .to_string(),
            )])
            .unwrap();
        engine
            .load([(
                SourceType::Path(Arc::new(std::path::PathBuf::from("b.lemma"))),
                r#"
        spec pricing 2025-06-01
        data x: 2
        rule r: x
    "#
                .to_string(),
            )])
            .unwrap();

        let january = date(2025, 1, 1);
        let june = date(2025, 6, 1);

        let workspace = engine
            .specs()
            .into_iter()
            .find(|r| r.repository.is_none())
            .expect("workspace");
        let mut pricing_rows: Vec<_> = workspace
            .specs
            .iter()
            .filter(|ls| ls.name == "pricing")
            .map(|ls| (ls.effective_from.clone(), ls.effective_to.clone()))
            .collect();
        pricing_rows.sort_by(|a, b| match (&a.0, &b.0) {
            (Some(x), Some(y)) => x.cmp(y),
            (None, Some(_)) => std::cmp::Ordering::Less,
            (Some(_), None) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        });
        assert_eq!(pricing_rows.len(), 2);
        assert_eq!(
            pricing_rows[0],
            (Some(january.clone()), Some(june.clone())),
            "earlier row ends at the next row's effective_from"
        );
        assert_eq!(
            pricing_rows[1],
            (Some(june.clone()), None),
            "latest row has no successor; effective_to is None"
        );

        assert!(
            !engine
                .specs()
                .into_iter()
                .find(|r| r.repository.is_none())
                .expect("workspace")
                .specs
                .iter()
                .any(|ls| ls.name == "unknown"),
            "no rows for unknown spec"
        );
    }

    /// `Engine::specs()` provides spec sets grouped by repository.
    /// Each spec version row exposes half-open `[effective_from, effective_to)` ranges.
    #[test]
    fn get_workspace_specs_with_half_open_ranges() {
        let mut engine = Engine::new();
        engine
            .load([(
                SourceType::Path(Arc::new(std::path::PathBuf::from("pricing_v1.lemma"))),
                r#"
        spec pricing 2025-01-01
        data x: 1
        rule r: x
    "#
                .to_string(),
            )])
            .unwrap();
        engine
            .load([(
                SourceType::Path(Arc::new(std::path::PathBuf::from("pricing_v2.lemma"))),
                r#"
        spec pricing 2026-01-01
        data x: 2
        rule r: x
    "#
                .to_string(),
            )])
            .unwrap();
        engine
            .load([(
                SourceType::Path(Arc::new(std::path::PathBuf::from("taxes.lemma"))),
                r#"
        spec taxes
        data rate: 0.21
        rule amount: rate
    "#
                .to_string(),
            )])
            .unwrap();

        let workspace = engine
            .specs()
            .into_iter()
            .find(|r| r.repository.is_none())
            .expect("workspace");
        let unique_names: std::collections::BTreeSet<&str> =
            workspace.specs.iter().map(|ls| ls.name.as_str()).collect();
        assert_eq!(
            unique_names.len(),
            2,
            "two unique spec names: pricing and taxes"
        );

        let pricing_rows: Vec<_> = workspace
            .specs
            .iter()
            .filter(|ls| ls.name == "pricing")
            .collect();
        assert_eq!(pricing_rows.len(), 2);
        assert_eq!(pricing_rows[0].effective_from, Some(date(2025, 1, 1)));
        assert_eq!(
            pricing_rows[0].effective_to,
            Some(date(2026, 1, 1)),
            "earlier pricing row ends at the next pricing row's effective_from"
        );
        assert_eq!(pricing_rows[1].effective_from, Some(date(2026, 1, 1)));
        assert_eq!(
            pricing_rows[1].effective_to, None,
            "latest pricing row has no successor; effective_to is None"
        );

        let tax_rows: Vec<_> = workspace
            .specs
            .iter()
            .filter(|ls| ls.name == "taxes")
            .collect();
        assert_eq!(tax_rows.len(), 1);
        assert_eq!(
            tax_rows[0].effective_from, None,
            "unversioned spec has no declared effective_from"
        );
        assert_eq!(
            tax_rows[0].effective_to, None,
            "unversioned spec has no successor; effective_to is None"
        );
    }

    #[test]
    fn test_evaluate_spec_all_rules() {
        let mut engine = Engine::new();
        engine
            .load([(
                SourceType::Path(Arc::new(std::path::PathBuf::from("test.lemma"))),
                r#"
        spec test
        data x: 10
        data y: 5
        rule added: x + y
        rule multiplied: x * y
    "#
                .to_string(),
            )])
            .unwrap();

        let now = DateTimeValue::now();
        let response = engine
            .evaluate(None, "test", Some(&now), IndexMap::new(), None, false)
            .unwrap();
        assert_eq!(response.results.len(), 2);

        let sum_result = response
            .results
            .values()
            .find(|r| r.rule.name == "added")
            .unwrap();
        assert_eq!(sum_result.result().expect("result").to_string(), "15");

        let product_result = response
            .results
            .values()
            .find(|r| r.rule.name == "multiplied")
            .unwrap();
        assert_eq!(product_result.result().expect("result").to_string(), "50");
    }

    #[test]
    fn test_evaluate_empty_data() {
        let mut engine = Engine::new();
        engine
            .load([(
                SourceType::Path(Arc::new(std::path::PathBuf::from("test.lemma"))),
                r#"
        spec test
        data price: 100
        rule total: price * 2
    "#
                .to_string(),
            )])
            .unwrap();

        let now = DateTimeValue::now();
        let response = engine
            .evaluate(None, "test", Some(&now), IndexMap::new(), None, false)
            .unwrap();
        assert_eq!(response.results.len(), 1);
        assert_eq!(
            response
                .results
                .values()
                .next()
                .unwrap()
                .result()
                .expect("result"),
            "200"
        );
    }

    #[test]
    fn test_evaluate_boolean_rule() {
        let mut engine = Engine::new();
        engine
            .load([(
                SourceType::Path(Arc::new(std::path::PathBuf::from("test.lemma"))),
                r#"
        spec test
        data age: 25
        rule is_adult: age >= 18
    "#
                .to_string(),
            )])
            .unwrap();

        let now = DateTimeValue::now();
        let response = engine
            .evaluate(None, "test", Some(&now), IndexMap::new(), None, false)
            .unwrap();
        assert_eq!(
            response
                .results
                .values()
                .next()
                .unwrap()
                .result
                .as_ref()
                .unwrap()
                .boolean,
            Some(true)
        );
    }

    #[test]
    fn test_evaluate_with_unless_clause() {
        let mut engine = Engine::new();
        engine
            .load([(
                SourceType::Path(Arc::new(std::path::PathBuf::from("test.lemma"))),
                r#"
        spec test
        data quantity: 15
        rule discount: 0
          unless quantity >= 10 then 10
    "#
                .to_string(),
            )])
            .unwrap();

        let now = DateTimeValue::now();
        let response = engine
            .evaluate(None, "test", Some(&now), IndexMap::new(), None, false)
            .unwrap();
        assert_eq!(
            response
                .results
                .values()
                .next()
                .unwrap()
                .result()
                .expect("result"),
            "10"
        );
    }

    #[test]
    fn test_spec_not_found() {
        let engine = Engine::new();
        let now = DateTimeValue::now();
        let result = engine.evaluate(
            None,
            "nonexistent",
            Some(&now),
            IndexMap::new(),
            None,
            false,
        );
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("No execution plan") && msg.contains("nonexistent"),
            "missing spec must report no plan, got: {msg}"
        );
    }

    #[test]
    fn test_multiple_specs() {
        let mut engine = Engine::new();
        engine
            .load([(
                SourceType::Path(Arc::new(std::path::PathBuf::from("spec 1.lemma"))),
                r#"
        spec spec1
        data x: 10
        rule result: x * 2
    "#
                .to_string(),
            )])
            .unwrap();

        engine
            .load([(
                SourceType::Path(Arc::new(std::path::PathBuf::from("spec 2.lemma"))),
                r#"
        spec spec2
        data y: 5
        rule result: y * 3
    "#
                .to_string(),
            )])
            .unwrap();

        let now = DateTimeValue::now();
        let response1 = engine
            .evaluate(None, "spec1", Some(&now), IndexMap::new(), None, false)
            .unwrap();
        assert_eq!(
            response1.results[0].result().expect("result").to_string(),
            "20"
        );
        let response2 = engine
            .evaluate(None, "spec2", Some(&now), IndexMap::new(), None, false)
            .unwrap();
        assert_eq!(
            response2.results[0].result().expect("result").to_string(),
            "15"
        );
    }

    #[test]
    fn test_runtime_error_mapping() {
        let mut engine = Engine::new();
        engine
            .load([(
                SourceType::Path(Arc::new(std::path::PathBuf::from("test.lemma"))),
                r#"
        spec test
        data numerator: 10
        data denominator: 0
        rule division: numerator / denominator
    "#
                .to_string(),
            )])
            .unwrap();

        let now = DateTimeValue::now();
        let result = engine.evaluate(None, "test", Some(&now), IndexMap::new(), None, false);
        // Division by zero returns a Veto (not an error)
        assert!(result.is_ok(), "Evaluation should succeed");
        let response = result.unwrap();
        let division_result = response
            .results
            .values()
            .find(|r| r.rule.name == "division");
        assert!(
            division_result.is_some(),
            "Should have division rule result"
        );
        let division = division_result.unwrap();
        assert!(division.vetoed);
        assert!(
            division
                .veto_reason
                .as_deref()
                .unwrap()
                .contains("Division by zero"),
            "Veto message should mention division by zero: {:?}",
            division.veto_reason
        );
    }

    #[test]
    fn test_rules_sorted_by_source_order() {
        let mut engine = Engine::new();
        engine
            .load([(
                SourceType::Path(Arc::new(std::path::PathBuf::from("test.lemma"))),
                r#"
        spec test
        data a: 1
        data b: 2
        rule z: a + b
        rule y: a * b
        rule x: a - b
    "#
                .to_string(),
            )])
            .unwrap();

        let now = DateTimeValue::now();
        let response = engine
            .evaluate(None, "test", Some(&now), IndexMap::new(), None, false)
            .unwrap();
        assert_eq!(response.results.len(), 3);

        // Verify source positions increase (z < y < x)
        let z_pos = response
            .results
            .values()
            .find(|r| r.rule.name == "z")
            .unwrap()
            .rule
            .source_location
            .span
            .start;
        let y_pos = response
            .results
            .values()
            .find(|r| r.rule.name == "y")
            .unwrap()
            .rule
            .source_location
            .span
            .start;
        let x_pos = response
            .results
            .values()
            .find(|r| r.rule.name == "x")
            .unwrap()
            .rule
            .source_location
            .span
            .start;

        assert!(z_pos < y_pos);
        assert!(y_pos < x_pos);
    }

    #[test]
    fn test_rule_filtering_evaluates_dependencies() {
        let mut engine = Engine::new();
        engine
            .load([(
                SourceType::Path(Arc::new(std::path::PathBuf::from("test.lemma"))),
                r#"
        spec test
        data base: 100
        rule subtotal: base * 2
        rule tax: subtotal * 10%
        rule total: subtotal + tax
    "#
                .to_string(),
            )])
            .unwrap();

        let now = DateTimeValue::now();
        let response = engine
            .evaluate(
                None,
                "test",
                Some(&now),
                IndexMap::new(),
                Some(&["total".to_string()]),
                false,
            )
            .unwrap();

        assert_eq!(response.results.len(), 1);
        assert_eq!(response.results.keys().next().unwrap(), "total");

        // But the value should be correct (dependencies were computed)
        let total = response.results.values().next().unwrap();
        assert_eq!(total.result().expect("result").to_string(), "220");
    }

    // -------------------------------------------------------------------
    // Pre-resolved dependency tests (Engine never fetches from registry)
    // -------------------------------------------------------------------

    use crate::parsing::ast::DateTimeValue;

    #[test]
    fn pre_resolved_deps_in_file_map_evaluates_external_spec() {
        let mut engine = Engine::new();

        engine
            .load([(
                SourceType::Dependency("@org/project".to_string()),
                "repo @org/project\nspec helper\ndata quantity: 42".to_string(),
            )])
            .expect("should load dependency files");

        engine
            .load([(
                SourceType::Path(Arc::new(std::path::PathBuf::from("main.lemma"))),
                r#"spec main_spec
uses external: @org/project helper
rule value: external.quantity"#
                    .to_string(),
            )])
            .expect("should succeed with pre-resolved deps");

        let now = DateTimeValue::now();
        let response = engine
            .evaluate(None, "main_spec", Some(&now), IndexMap::new(), None, false)
            .expect("evaluate should succeed");

        let value_result = response
            .results
            .get("value")
            .expect("rule 'value' should exist");
        assert_eq!(value_result.result().expect("result").to_string(), "42");
    }

    #[test]
    fn show_with_repo_resolves_registry_spec() {
        let mut engine = Engine::new();
        engine
            .load([(
                SourceType::Dependency("@org/project".to_string()),
                "repo @org/project\nspec helper\ndata quantity: 42\nrule expose: quantity"
                    .to_string(),
            )])
            .expect("registry bundle loads");

        engine
            .load([(
                SourceType::Path(Arc::new(std::path::PathBuf::from("main.lemma"))),
                r#"spec main_spec
data x: 1"#
                    .to_string(),
            )])
            .expect("main loads");

        let now = DateTimeValue::now();
        let view = engine
            .show(Some("@org/project"), "helper", Some(&now))
            .expect("show for registry spec");
        assert!(view.data.contains_key("quantity"));
    }

    #[test]
    fn load_no_external_refs_works() {
        let mut engine = Engine::new();

        engine
            .load([(
                SourceType::Path(Arc::new(std::path::PathBuf::from("local.lemma"))),
                r#"spec local_only
data price: 100
rule doubled: price * 2"#
                    .to_string(),
            )])
            .expect("should succeed when there are no @... references");

        let now = DateTimeValue::now();
        let response = engine
            .evaluate(None, "local_only", Some(&now), IndexMap::new(), None, false)
            .expect("evaluate should succeed");

        let doubled = response.results.get("doubled").expect("doubled rule");
        assert_eq!(doubled.result().expect("result").to_string(), "200");
    }

    #[test]
    fn unresolved_external_ref_without_deps_fails() {
        let mut engine = Engine::new();

        let result = engine.load([(
            SourceType::Path(Arc::new(std::path::PathBuf::from("main.lemma"))),
            r#"spec main_spec
uses external: @org/project missing
rule value: external.quantity"#
                .to_string(),
        )]);

        let errs = result.expect_err("Should fail when registry dep is not loaded");
        assert!(
            errs.iter()
                .any(|e| e.kind() == crate::ErrorKind::MissingRepository),
            "expected MissingRepository, got: {:?}",
            errs.iter().map(|e| e.kind()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn pre_resolved_deps_with_spec_and_type_refs() {
        let mut engine = Engine::new();

        engine
            .load([(
                SourceType::Dependency("@org/example".to_string()),
                "repo @org/example\nspec helper\ndata value: 42".to_string(),
            )])
            .expect("should load helper file");

        engine
        .load([(
                SourceType::Dependency("@iso/countries".to_string()),
                "repo @iso/countries\nspec alpha2\ndata code: text\n -> option \"NL\"\n -> option \"BE\"".to_string(),
            )])
            .expect("should load alpha2 file");

        engine
            .load([(
                SourceType::Path(Arc::new(std::path::PathBuf::from("main.lemma"))),
                r#"spec registry_demo
uses @iso/countries alpha2
data country: alpha2.code
data unit_count: 5
uses @org/example helper
rule helper_value: helper.value
rule line_total: unit_count * 2
rule formatted: helper_value + 0"#
                    .to_string(),
            )])
            .expect("should succeed with pre-resolved spec and type deps");

        let now = DateTimeValue::now();
        let response = engine
            .evaluate(
                None,
                "registry_demo",
                Some(&now),
                IndexMap::new(),
                None,
                false,
            )
            .expect("evaluate should succeed");

        assert_eq!(
            response
                .results
                .get("helper_value")
                .expect("helper_value")
                .result()
                .expect("result"),
            "42"
        );
        let line = response
            .results
            .get("line_total")
            .expect("line_total")
            .result()
            .expect("result");
        assert_eq!(line, "10");
        assert_eq!(
            response
                .results
                .get("formatted")
                .expect("formatted")
                .result()
                .expect("result"),
            "42"
        );
    }

    #[test]
    fn load_empty_labeled_source_is_error() {
        let mut engine = Engine::new();
        let err = engine
            .load([(
                SourceType::Path(Arc::new(std::path::PathBuf::from("  "))),
                "spec x\ndata a: 1".to_string(),
            )])
            .unwrap_err();
        assert!(err.errors.iter().any(|e| e.message().contains("non-empty")));
    }

    #[test]
    fn add_dependency_files_accepts_registry_bundle_specs() {
        let mut engine = Engine::new();
        engine
            .load([(
                SourceType::Dependency("@org/my".to_string()),
                "repo @org/my\nspec helper\ndata x: 1".to_string(),
            )])
            .expect("dependency bundle specs should be accepted");
    }

    #[test]
    fn user_load_rejects_reserved_embedded_stdlib_repository() {
        let mut engine = Engine::new();
        let batch = engine.load([(
            SourceType::Dependency(EMBEDDED_STDLIB_REPOSITORY.to_string()),
            "spec finance\ndata money: ratio -> decimals 2".to_string(),
        )]);
        assert!(
            batch.is_err(),
            "load must not write reserved lemma stdlib repo"
        );
        let msg = batch
            .unwrap_err()
            .errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            msg.contains(EMBEDDED_STDLIB_REPOSITORY) && msg.contains("reserved"),
            "expected reserved-repo error, got: {msg}"
        );

        let workspace = engine.load([(
            SourceType::Volatile,
            "repo lemma\nspec x\ndata a: 1".to_string(),
        )]);
        assert!(workspace.is_err(), "workspace repo lemma must be rejected");
        let msg = workspace
            .unwrap_err()
            .errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            msg.contains(EMBEDDED_STDLIB_REPOSITORY) && msg.contains("reserved"),
            "expected reserved-repo error, got: {msg}"
        );
    }

    #[test]
    fn load_returns_all_errors_not_just_first() {
        let mut engine = Engine::new();

        let result = engine.load([(
            SourceType::Path(Arc::new(std::path::PathBuf::from("test.lemma"))),
            r#"spec demo
uses type_src: nonexistent_type_source
  -> with amount: 10
uses helper: nonexistent_spec
data price: 10
rule total: helper.value + price"#
                .to_string(),
        )]);

        assert!(result.is_err(), "Should fail with multiple errors");
        let load_err = result.unwrap_err();
        assert!(
            load_err.errors.len() >= 2,
            "expected at least 2 errors (type + spec ref), got {}",
            load_err.errors.len()
        );
        let error_message = load_err
            .errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ");

        assert!(
            error_message.contains("nonexistent_type_source"),
            "Should mention data import source spec. Got:\n{}",
            error_message
        );
        assert!(
            error_message.contains("nonexistent_spec"),
            "Should mention spec reference error about 'nonexistent_spec'. Got:\n{}",
            error_message
        );
    }

    // ── Suggestion value type validation ────────────────────────────────
    // Planning must reject suggestion values that don't match the type.
    // These tests cover both primitives and named types (which the parser
    // can't validate because it doesn't resolve type names).

    #[test]
    fn planning_rejects_invalid_number_default() {
        let mut engine = Engine::new();
        let result = engine.load([(
            SourceType::Path(Arc::new(std::path::PathBuf::from("t.lemma"))),
            "spec t\ndata x: number -> suggest \"10 $$\"]\nrule r: x".to_string(),
        )]);
        assert!(
            result.is_err(),
            "must reject non-numeric suggestion on number type"
        );
    }

    #[test]
    fn planning_rejects_text_literal_as_number_default() {
        // `suggest "10"` produces a typed `CommandArg::Literal(Value::Text("10"))`.
        // Planning matches on the literal's variant — a `Text` literal is rejected
        // where a `Number` literal is required, even though `"10"` would parse as
        // a valid `Decimal` if coerced.
        let mut engine = Engine::new();
        let result = engine.load([(
            SourceType::Path(Arc::new(std::path::PathBuf::from("t.lemma"))),
            "spec t\ndata x: number -> suggest \"10\"]\nrule r: x".to_string(),
        )]);
        assert!(
            result.is_err(),
            "must reject text literal \"10\" as suggestion for number type"
        );
    }

    #[test]
    fn planning_rejects_invalid_boolean_default() {
        let mut engine = Engine::new();
        let result = engine.load([(
            SourceType::Path(Arc::new(std::path::PathBuf::from("t.lemma"))),
            "spec t\ndata x: [boolean -> suggest \"maybe\"]\nrule r: x".to_string(),
        )]);
        assert!(
            result.is_err(),
            "must reject non-boolean suggestion on boolean type"
        );
    }

    #[test]
    fn planning_rejects_invalid_named_type_default() {
        // Named type: the parser can't validate this, only planning can.
        let mut engine = Engine::new();
        let result = engine.load([(SourceType::Path(Arc::new(std::path::PathBuf::from("t.lemma"))), "spec t\ndata custom: number -> minimum 0\ndata x: [custom -> suggest \"abc\"]\nrule r: x".to_string())]);
        assert!(
            result.is_err(),
            "must reject non-numeric suggestion on named number type"
        );
    }

    #[test]
    fn context_merges_cross_file_repo_identities() {
        let mut engine = Engine::new();

        // Load two files with the same named repo, but different spec names.
        engine
            .load([(
                SourceType::Path(Arc::new(std::path::PathBuf::from("file1.lemma"))),
                "repo shared\nspec a\ndata x: 1".to_string(),
            )])
            .expect("first file should load");

        engine
            .load([(
                SourceType::Path(Arc::new(std::path::PathBuf::from("file2.lemma"))),
                "repo shared\nspec b\ndata y: 2".to_string(),
            )])
            .expect("second file should load");

        // Both specs should land under the same repo entry.
        // Workspace, embedded stdlib (`lemma`), plus the "shared" repo.
        assert_eq!(
            engine.context.repositories().len(),
            3,
            "should have workspace, stdlib repository, and one named user repository"
        );

        let shared_repo = engine
            .context
            .find_repository("shared")
            .expect("shared repo should exist");
        let shared_specs = engine.context.repositories().get(&shared_repo).unwrap();
        assert_eq!(
            shared_specs.len(),
            2,
            "shared repo should contain both specs"
        );
        assert!(shared_specs.contains_key("a"));
        assert!(shared_specs.contains_key("b"));

        // Loading a dependency with the same repo name should be rejected.
        let dependency_result = engine.load([(
            SourceType::Dependency("@some/dep".to_string()),
            "repo shared\nspec c\ndata z: 3".to_string(),
        )]);
        assert!(
            dependency_result.is_err(),
            "dependency with an existing repo name must be rejected"
        );

        let result = engine.load([(
            SourceType::Path(Arc::new(std::path::PathBuf::from("file2.lemma"))),
            "repo shared\nspec a\ndata y: 2".to_string(),
        )]);

        assert!(
            result.is_err(),
            "should reject duplicate spec name in same repo"
        );
        let load_err = result.unwrap_err();
        assert_eq!(
            load_err.errors.len(),
            2,
            "duplicate spec must error on both declaring sources, got: {:?}",
            load_err.errors
        );
        let joined = load_err
            .errors
            .iter()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            joined.contains("Duplicate spec 'a'"),
            "error should mention duplicate spec, got: {joined}"
        );
        let paths: Vec<String> = load_err
            .errors
            .iter()
            .map(|err| {
                err.location()
                    .expect("duplicate errors must have source")
                    .source_type
                    .to_string()
            })
            .collect();
        assert!(
            paths.iter().any(|p| p == "file1.lemma"),
            "first declaring file must get conflict diagnostic, got paths: {paths:?}"
        );
        assert!(
            paths.iter().any(|p| p == "file2.lemma"),
            "incoming file must get conflict diagnostic, got paths: {paths:?}"
        );
    }

    #[test]
    fn test_list_structure() {
        let mut engine = Engine::new();
        engine
            .load([(
                SourceType::Path(Arc::new(std::path::PathBuf::from("file1.lemma"))),
                "repo shared\nspec a\ndata x: 1\nrule r: x".to_string(),
            )])
            .expect("file should load");

        let repos = engine.specs();
        let shared_repo = repos
            .iter()
            .find(|r| r.repository.as_deref() == Some("shared"))
            .expect("shared repo in list");
        assert_eq!(shared_repo.specs.len(), 1);
        assert_eq!(shared_repo.specs[0].name, "a");
    }

    fn path_st(name: &str) -> SourceType {
        SourceType::Path(Arc::new(std::path::PathBuf::from(name)))
    }

    #[test]
    fn remove_none_removes_version_active_at_now() {
        let mut engine = Engine::new();
        engine
            .load([(
                path_st("t.lemma"),
                "spec t\ndata v: 1\nrule r: v\n\nspec t 2099-01-01\ndata v: 2\nrule r: v\n"
                    .to_string(),
            )])
            .expect("load");
        engine
            .remove(None, "t", None)
            .expect("remove active-at now (origin while before 2099)");
        let workspace = engine
            .specs()
            .into_iter()
            .find(|r| r.repository.is_none())
            .expect("workspace");
        assert_eq!(workspace.specs.iter().filter(|s| s.name == "t").count(), 1);
        assert!(
            workspace
                .specs
                .iter()
                .any(|s| s.name == "t" && s.effective_from.is_some()),
            "dated version must remain"
        );
    }

    #[test]
    fn remove_none_errors_when_no_version_active_at_now() {
        let mut engine = Engine::new();
        engine
            .load([(
                path_st("t.lemma"),
                "spec t 2099-01-01\ndata v: 1\nrule r: v\n".to_string(),
            )])
            .expect("load");
        let err = engine
            .remove(None, "t", None)
            .expect_err("no version active at now");
        assert_eq!(err.kind(), crate::ErrorKind::Request);
        assert!(
            engine.show(None, "t", Some(&date(2099, 1, 1))).is_ok(),
            "future version must remain"
        );
    }

    #[test]
    fn update_origin_code_keeps_later_version() {
        let mut engine = Engine::new();
        let st = path_st("t.lemma");
        engine
            .load([(
                st.clone(),
                "spec t\ndata v: 1\nrule r: v\n\nspec t 2099-01-01\ndata v: 2\nrule r: v\n"
                    .to_string(),
            )])
            .expect("load");
        engine
            .update(
                None,
                "spec t\ndata v: 9\nrule r: v\n\nspec t 2099-01-01\ndata v: 2\nrule r: v\n"
                    .to_string(),
                st,
            )
            .expect("update origin body while keeping later version in buffer");
        let workspace = engine
            .specs()
            .into_iter()
            .find(|r| r.repository.is_none())
            .expect("workspace");
        assert_eq!(workspace.specs.iter().filter(|s| s.name == "t").count(), 2);
        let now = DateTimeValue::now();
        let response = engine
            .evaluate(None, "t", Some(&now), IndexMap::new(), None, false)
            .expect("evaluate origin");
        assert_eq!(
            response.results.get("r").and_then(|r| r.result()),
            Some("9")
        );
    }

    #[test]
    fn update_path_prunes_dropped_version() {
        let mut engine = Engine::new();
        let st = path_st("t.lemma");
        engine
            .load([(
                st.clone(),
                "spec t\ndata v: 1\nrule r: v\n\nspec t 2025-06-01\ndata v: 2\nrule r: v\n"
                    .to_string(),
            )])
            .expect("load");
        engine
            .update(None, "spec t\ndata v: 1\nrule r: v\n".to_string(), st)
            .expect("prune second version");
        let workspace = engine
            .specs()
            .into_iter()
            .find(|r| r.repository.is_none())
            .expect("workspace");
        assert_eq!(workspace.specs.iter().filter(|s| s.name == "t").count(), 1);
        assert!(workspace
            .specs
            .iter()
            .any(|s| s.name == "t" && s.effective_from.is_none()));
    }

    #[test]
    fn update_volatile_does_not_prune_sibling() {
        let mut engine = Engine::new();
        engine
            .load([(
                SourceType::Volatile,
                "spec a\ndata v: 1\nrule r: v\n\nspec b\ndata v: 2\nrule r: v\n".to_string(),
            )])
            .expect("load");
        engine
            .update(
                None,
                "spec a\ndata v: 3\nrule r: v\n".to_string(),
                SourceType::Volatile,
            )
            .expect("update a");
        let workspace = engine
            .specs()
            .into_iter()
            .find(|r| r.repository.is_none())
            .expect("workspace");
        assert!(workspace.specs.iter().any(|s| s.name == "a"));
        assert!(workspace.specs.iter().any(|s| s.name == "b"));
    }

    #[test]
    fn update_upserts_new_identity() {
        let mut engine = Engine::new();
        let st = path_st("t.lemma");
        engine
            .load([(st.clone(), "spec a\ndata v: 1\nrule r: v\n".to_string())])
            .expect("load");
        engine
            .update(
                None,
                "spec a\ndata v: 1\nrule r: v\n\nspec b\ndata v: 2\nrule r: v\n".to_string(),
                st,
            )
            .expect("upsert b");
        let workspace = engine
            .specs()
            .into_iter()
            .find(|r| r.repository.is_none())
            .expect("workspace");
        assert!(workspace.specs.iter().any(|s| s.name == "a"));
        assert!(workspace.specs.iter().any(|s| s.name == "b"));
    }

    #[test]
    fn update_cross_path_identity_is_error() {
        let mut engine = Engine::new();
        engine
            .load([(
                path_st("a.lemma"),
                "spec conflict\ndata v: 1\nrule r: v\n".to_string(),
            )])
            .expect("load a");
        let err = engine
            .update(
                None,
                "spec conflict\ndata v: 2\nrule r: v\n".to_string(),
                path_st("b.lemma"),
            )
            .expect_err("cross-path identity");
        let joined: String = err
            .errors
            .iter()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            joined.contains("Duplicate spec") && joined.contains("also declared"),
            "expected duplicate across paths, got: {joined}"
        );
        assert!(
            engine.show(None, "conflict", None).is_ok(),
            "failed cross-path update must leave the original identity loaded"
        );
    }

    #[test]
    fn update_empty_dependency_prunes_all_specs_of_source() {
        let mut engine = Engine::new();
        let st = SourceType::Dependency("@org/dep".to_string());
        engine
            .load([(
                st.clone(),
                "repo @org/dep\n\nspec a\ndata v: 1\nrule r: v\n\nspec b\ndata v: 2\nrule r: v\n"
                    .to_string(),
            )])
            .expect("load");
        engine
            .update(None, "   \n".to_string(), st)
            .expect("empty dependency update prunes");
        let listed = engine.specs();
        assert!(
            !listed.iter().any(|r| {
                r.repository.as_deref() == Some("@org/dep")
                    && r.specs.iter().any(|s| s.name == "a" || s.name == "b")
            }),
            "empty dependency update must remove every live row of that source"
        );
    }

    #[test]
    fn failed_apply_leaves_list_source_and_run_unchanged() {
        let mut engine = Engine::new();
        engine
            .load([(
                SourceType::Volatile,
                "spec price\ndata amount: number\nrule total: amount\n".to_string(),
            )])
            .expect("load");
        let before_repositories = engine.specs();
        let before_source = engine.source(None, Some("price"), None).expect("source");
        let now = DateTimeValue::now();
        let before = engine
            .evaluate(
                None,
                "price",
                Some(&now),
                [("amount".to_string(), EvaluateDataValue::from("4"))]
                    .into_iter()
                    .collect(),
                None,
                false,
            )
            .expect("evaluate");
        let failed = engine.load([(
            SourceType::Volatile,
            "spec price\ndata amount: number\nrule total: amount +\n".to_string(),
        )]);
        assert!(failed.is_err(), "invalid spec must fail planning");
        assert_eq!(engine.specs(), before_repositories);
        assert_eq!(
            engine.source(None, Some("price"), None).expect("source"),
            before_source
        );
        let after = engine
            .evaluate(
                None,
                "price",
                Some(&now),
                [("amount".to_string(), EvaluateDataValue::from("4"))]
                    .into_iter()
                    .collect(),
                None,
                false,
            )
            .expect("evaluate after failed load");
        assert_eq!(
            after.results.get("total").expect("total").result,
            before.results.get("total").expect("total").result
        );
    }

    #[test]
    fn update_empty_path_prunes_all_specs_of_source() {
        let mut engine = Engine::new();
        let st = path_st("t.lemma");
        engine
            .load([(
                st.clone(),
                "spec a\ndata v: 1\nrule r: v\n\nspec b\ndata v: 2\nrule r: v\n".to_string(),
            )])
            .expect("load");
        engine
            .update(None, "   \n".to_string(), st)
            .expect("empty path update prunes");
        let workspace = engine
            .specs()
            .into_iter()
            .find(|r| r.repository.is_none())
            .expect("workspace");
        assert!(
            !workspace
                .specs
                .iter()
                .any(|s| s.name == "a" || s.name == "b"),
            "empty path update must remove every live row of that source"
        );
    }

    #[test]
    fn update_empty_volatile_is_error() {
        let mut engine = Engine::new();
        engine
            .load([(
                SourceType::Volatile,
                "spec a\ndata v: 1\nrule r: v\n".to_string(),
            )])
            .expect("load");
        let err = engine
            .update(None, "   \n".to_string(), SourceType::Volatile)
            .expect_err("empty volatile update");
        assert!(
            err.errors
                .iter()
                .any(|e| e.to_string().contains("at least one spec")),
            "got: {:?}",
            err.errors
        );
        assert!(
            engine.show(None, "a", None).is_ok(),
            "failed empty volatile update must leave the original identity loaded"
        );
    }

    #[test]
    fn update_repository_param_mismatch_is_error() {
        let mut engine = Engine::new();
        let st = path_st("t.lemma");
        engine
            .load([(
                st.clone(),
                "repo other\nspec a\ndata v: 1\nrule r: v\n".to_string(),
            )])
            .expect("load");
        let err = engine
            .update(
                Some("expected"),
                "repo other\nspec a\ndata v: 2\nrule r: v\n".to_string(),
                st,
            )
            .expect_err("mismatch");
        assert!(
            err.errors
                .iter()
                .any(|e| e.to_string().contains("does not match")),
            "got: {:?}",
            err.errors
        );
    }

    #[test]
    fn update_rollback_restores_pruned_and_replaced() {
        let mut engine = Engine::new();
        let dep = path_st("dep.lemma");
        let consumer = path_st("consumer.lemma");
        engine
            .load([
                (
                    dep.clone(),
                    "spec dep\ndata v: 1\nrule r: v\n\nspec dep 2025-06-01\ndata v: 2\nrule r: v\n"
                        .to_string(),
                ),
                (
                    consumer,
                    "spec consumer\nuses d: dep\nrule out: d.r\n".to_string(),
                ),
            ])
            .expect("load");
        let before = engine.specs();
        let err = engine
            .update(
                None,
                "spec dep\ndata other: 5\nrule unrelated: other\n".to_string(),
                dep,
            )
            .expect_err("consumer must break");
        assert!(!err.errors.is_empty());
        let after = engine.specs();
        assert_eq!(after.len(), before.len(), "repository count must match");
        for (before_repo, after_repo) in before.iter().zip(after.iter()) {
            assert_eq!(before_repo.repository, after_repo.repository);
            let mut before_specs = before_repo.specs.clone();
            let mut after_specs = after_repo.specs.clone();
            before_specs
                .sort_by(|a, b| (&a.name, &a.effective_from).cmp(&(&b.name, &b.effective_from)));
            after_specs
                .sort_by(|a, b| (&a.name, &a.effective_from).cmp(&(&b.name, &b.effective_from)));
            assert_eq!(
                before_specs, after_specs,
                "failed update must restore spec versions for {:?}",
                before_repo.repository
            );
        }
        let now = DateTimeValue::now();
        engine
            .evaluate(None, "consumer", Some(&now), IndexMap::new(), None, false)
            .expect("consumer still runs after rollback");
        assert!(
            engine.show(None, "dep", Some(&date(2025, 6, 1))).is_ok(),
            "pruned later dep version must be restored"
        );
    }

    fn finance_result(engine: &Engine) -> String {
        let now = DateTimeValue::now();
        engine
            .evaluate(None, "finance", Some(&now), IndexMap::new(), None, false)
            .expect("finance evaluates")
            .results
            .values()
            .next()
            .expect("one result")
            .result()
            .expect("result")
            .to_string()
    }

    #[test]
    fn update_pasted_duplicate_identity_is_error() {
        let mut engine = Engine::new();
        let st = path_st("finance.lemma");
        engine
            .load([(
                st.clone(),
                "spec finance\ndata v: 1\nrule r: 1\n".to_string(),
            )])
            .expect("load");
        let err = engine
            .update(
                None,
                "spec finance\ndata v: 1\nrule r: 1\n\nspec finance\ndata v: 2\nrule r: 2\n"
                    .to_string(),
                st,
            )
            .expect_err("pasted duplicate identity must be rejected");
        let joined: String = err
            .errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            joined.contains("Duplicate spec 'finance'"),
            "expected duplicate error, got: {joined}"
        );
        assert_eq!(
            finance_result(&engine),
            "1",
            "original spec must stay active"
        );
    }

    #[test]
    fn update_duplicate_identity_both_changed_is_error_not_panic() {
        let mut engine = Engine::new();
        let st = path_st("finance.lemma");
        engine
            .load([(
                st.clone(),
                "spec finance\ndata v: 1\nrule r: 1\n".to_string(),
            )])
            .expect("load");
        let err = engine
            .update(
                None,
                "spec finance\ndata v: 2\nrule r: 2\n\nspec finance\ndata v: 3\nrule r: 3\n"
                    .to_string(),
                st,
            )
            .expect_err("duplicate identity must be rejected");
        let joined: String = err
            .errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            joined.contains("Duplicate spec 'finance'"),
            "expected duplicate error, got: {joined}"
        );
        assert_eq!(
            finance_result(&engine),
            "1",
            "original spec must stay active"
        );
    }

    #[test]
    fn update_reports_every_duplicate_identity() {
        let mut engine = Engine::new();
        let err = engine
            .update(
                None,
                "spec finance\ndata v: 1\nrule r: 1\n\nspec finance\ndata v: 2\nrule r: 2\n\nspec policies\ndata v: 1\nrule r: 1\n\nspec policies\ndata v: 2\nrule r: 2\n"
                    .to_string(),
                path_st("policies.lemma"),
            )
            .expect_err("duplicate identities must be rejected");
        let joined: String = err
            .errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            joined.contains("Duplicate spec 'finance'"),
            "expected finance duplicate, got: {joined}"
        );
        assert!(
            joined.contains("Duplicate spec 'policies'"),
            "expected policies duplicate, got: {joined}"
        );
    }

    #[test]
    fn update_identical_bytes_succeeds() {
        let mut engine = Engine::new();
        let st = path_st("t.lemma");
        let code = "spec t\ndata v: 1\nrule r: v\n".to_string();
        engine.load([(st.clone(), code.clone())]).expect("load");
        let before = engine.show(None, "t", None).expect("show before");
        engine.update(None, code, st).expect("identical update");
        let after = engine.show(None, "t", None).expect("show after");
        assert_eq!(before.meta, after.meta);
        assert_eq!(
            before.rules.keys().collect::<Vec<_>>(),
            after.rules.keys().collect::<Vec<_>>()
        );
    }

    /// Body-only edit of one temporal version must not spuriously reject a consumer
    /// whose window overlaps only the non-dirty sibling version.
    #[test]
    fn update_slice_mode_keeps_consumer_overlapping_sibling_version() {
        let mut engine = Engine::new();
        let dep = path_st("dep.lemma");
        let consumer = path_st("consumer.lemma");
        engine
            .load([
                (
                    dep.clone(),
                    "spec dep\ndata v: 1\nrule r: v\n\nspec dep 2025-06-01\ndata v: 2\nrule r: v\n"
                        .to_string(),
                ),
                (
                    consumer,
                    "spec consumer\nuses d: dep\nrule out: d.r\n\nspec consumer 2025-06-01\nuses d: dep\nrule out: d.r\n"
                        .to_string(),
                ),
            ])
            .expect("load");
        engine
            .update(
                None,
                "spec dep\ndata v: 1\nrule r: v\n\nspec dep 2025-06-01\ndata v: 9\nrule r: v\n"
                    .to_string(),
                dep,
            )
            .expect("body-only edit of later dep version must keep consumers valid");
        let before_breakpoint = date(2025, 1, 15);
        let response = engine
            .evaluate(
                None,
                "consumer",
                Some(&before_breakpoint),
                IndexMap::new(),
                None,
                false,
            )
            .expect("origin consumer still runs against non-dirty dep version");
        assert_eq!(
            response.results.get("out").and_then(|r| r.result()),
            Some("1")
        );
        let after_breakpoint = date(2025, 7, 1);
        let response = engine
            .evaluate(
                None,
                "consumer",
                Some(&after_breakpoint),
                IndexMap::new(),
                None,
                false,
            )
            .expect("later consumer runs against dirty dep version");
        assert_eq!(
            response.results.get("out").and_then(|r| r.result()),
            Some("9")
        );
    }

    /// Body-only edit that introduces interface drift vs a sibling version must fail
    /// the same way a cold load of the resulting files would.
    #[test]
    fn update_slice_mode_rejects_interface_drift_vs_sibling_version() {
        let mut engine = Engine::new();
        let dep = path_st("dep.lemma");
        let consumer = path_st("consumer.lemma");
        engine
            .load([
                (
                    dep.clone(),
                    "spec dep\ndata v: 1\nrule r: v\n\nspec dep 2025-06-01\ndata v: 2\nrule r: v\n"
                        .to_string(),
                ),
                (
                    consumer.clone(),
                    "spec consumer\nuses d: dep\nrule out: d.r\n".to_string(),
                ),
            ])
            .expect("load");
        let err = engine
            .update(
                None,
                "spec dep\ndata v: 1\nrule r: v\n\nspec dep 2025-06-01\ndata v: \"x\"\nrule r: v\n"
                    .to_string(),
                dep.clone(),
            )
            .expect_err("drift between dep versions must fail incremental update");
        assert!(
            err.errors.iter().any(|e| {
                let msg = e.to_string();
                msg.contains("interface") || msg.contains("changed")
            }),
            "expected interface-drift error, got: {:?}",
            err.errors
        );

        let mut cold = Engine::new();
        let cold_err = cold
            .load([
                (
                    dep,
                    "spec dep\ndata v: 1\nrule r: v\n\nspec dep 2025-06-01\ndata v: \"x\"\nrule r: v\n"
                        .to_string(),
                ),
                (
                    consumer,
                    "spec consumer\nuses d: dep\nrule out: d.r\n".to_string(),
                ),
            ])
            .expect_err("cold load must also reject the drift");
        assert!(
            cold_err.errors.iter().any(|e| {
                let msg = e.to_string();
                msg.contains("interface") || msg.contains("changed")
            }),
            "expected interface-drift error on cold load, got: {:?}",
            cold_err.errors
        );
    }

    /// Body-only edit of one temporal version that fails to plan must Error, not panic,
    /// when a healthy sibling version remains and a consumer depends on the set.
    #[test]
    fn update_slice_mode_planning_error_on_dirty_version_is_error_not_panic() {
        let mut engine = Engine::new();
        let dep = path_st("dep.lemma");
        let consumer = path_st("consumer.lemma");
        engine
            .load([
                (
                    dep.clone(),
                    "spec dep\ndata v: 1\nrule r: v\n\nspec dep 2025-06-01\ndata v: 2\nrule r: v\n"
                        .to_string(),
                ),
                (
                    consumer,
                    "spec consumer\nuses d: dep\nrule out: d.r\n".to_string(),
                ),
            ])
            .expect("load");
        let err = engine
            .update(
                None,
                "spec dep\ndata v: 1\nrule r: v\n\nspec dep 2025-06-01\ndata v: 2\nrule r: v + nope\n"
                    .to_string(),
                dep,
            )
            .expect_err("planning error on dirty version must be Err, not panic");
        assert!(!err.errors.is_empty());
        let now = DateTimeValue::now();
        engine
            .evaluate(None, "consumer", Some(&now), IndexMap::new(), None, false)
            .expect("failed update must leave previous engine state intact");
    }
}

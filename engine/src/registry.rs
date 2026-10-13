//! Registry types and sans-IO resolution for external `@` repository references.
//!
//! The engine owns registry policy (URL construction, status mapping, transitive
//! resolve). Hosts own the socket: they answer [`Fetch`] with [`HttpResponse`].

use crate::engine::Context;
use crate::error::{Error, RegistryErrorKind};
use crate::limits::ResourceLimits;
use crate::parsing::ast::{DateTimeValue, LemmaRepository, RepositoryQualifier};
use crate::parsing::source::{Source, SourceType};
use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Header {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Fetch {
    pub repository: String,
    pub url: String,
    pub headers: Vec<Header>,
}

#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<Header>,
    pub body: String,
}

#[derive(Debug, Clone)]
pub struct TransportFailure {
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct RegistryBundle {
    pub repository: String,
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct RegistryError {
    pub kind: RegistryErrorKind,
    pub message: String,
}

/// Result of [`Install`]: LemmaBase repository source text and normalized id.
#[derive(Debug, Clone, Serialize)]
pub struct RepositoryInstallResult {
    pub source: String,
    pub id: String,
}

/// Synchronous driver for Rust hosts. wasm drives the steps itself with async fetch.
pub trait HttpTransport {
    fn get(&self, fetch: &Fetch) -> Result<HttpResponse, TransportFailure>;
}

/// A registry is a request builder and response interpreter. It never performs I/O.
pub trait Registry: Send + Sync {
    /// Build the fetch request for an already-validated registry qualifier.
    fn fetch_for(&self, qualifier: &RepositoryQualifier) -> Result<Fetch, Error>;
    fn bundle_from(
        &self,
        name: &str,
        response: Result<HttpResponse, TransportFailure>,
    ) -> Result<RegistryBundle, RegistryError>;
    fn navigation_url(&self, name: &str, effective: Option<&DateTimeValue>) -> Option<String>;
}

// ---------------------------------------------------------------------------
// LemmaBase
// ---------------------------------------------------------------------------

/// LemmaBase registry: bound to `https://lemmabase.com`. Not configurable.
pub struct LemmaBase;

impl LemmaBase {
    const BASE_URL: &'static str = "https://lemmabase.com";

    fn display_id(name: &str, effective: Option<&DateTimeValue>) -> String {
        match effective {
            None => name.to_string(),
            Some(d) => format!("{name} {d}"),
        }
    }

    fn kind_from_status(status: u16) -> RegistryErrorKind {
        match status {
            404 => RegistryErrorKind::NotFound,
            401 | 403 => RegistryErrorKind::Unauthorized,
            500..=599 => RegistryErrorKind::ServerError,
            _ => RegistryErrorKind::Other,
        }
    }
}

impl Registry for LemmaBase {
    fn fetch_for(&self, qualifier: &RepositoryQualifier) -> Result<Fetch, Error> {
        if !qualifier.is_registry() {
            return Err(Error::registry(
                format!(
                    "Registry identifier must start with '@' (got '{}')",
                    qualifier.name
                ),
                volatile_origin_source(),
                qualifier.name.clone(),
                RegistryErrorKind::Other,
                Some("Use a LemmaBase repository id like @owner/name".to_string()),
                None,
                None,
            ));
        }
        let id = qualifier.name.clone();
        Ok(Fetch {
            url: format!("{}/{}.lemma", Self::BASE_URL, id),
            repository: id,
            headers: Vec::new(),
        })
    }

    fn bundle_from(
        &self,
        name: &str,
        response: Result<HttpResponse, TransportFailure>,
    ) -> Result<RegistryBundle, RegistryError> {
        let display = Self::display_id(name, None);
        match response {
            Err(failure) => Err(RegistryError {
                kind: RegistryErrorKind::NetworkError,
                message: format!(
                    "Failed to reach LemmaBase for '{display}': {}",
                    failure.message
                ),
            }),
            Ok(response) => {
                if (200..300).contains(&response.status) {
                    return Ok(RegistryBundle {
                        repository: name.to_string(),
                        source: response.body,
                    });
                }
                let kind = Self::kind_from_status(response.status);
                Err(RegistryError {
                    kind,
                    message: format!(
                        "LemmaBase returned HTTP {} for '{}'",
                        response.status, display
                    ),
                })
            }
        }
    }

    fn navigation_url(&self, name: &str, effective: Option<&DateTimeValue>) -> Option<String> {
        let qualifier = crate::parsing::parse_repository_qualifier_str(name).ok()?;
        if !qualifier.is_registry() {
            return None;
        }
        let base = format!("{}/{}", Self::BASE_URL, qualifier.name);
        Some(match effective {
            None => base,
            Some(d) => format!("{base}?effective={d}"),
        })
    }
}

// ---------------------------------------------------------------------------
// Registries catalogue
// ---------------------------------------------------------------------------

/// The registries the host composes. [`Default`] is LemmaBase alone.
///
/// LemmaBase is a fixed member: adding registries never retargets it.
pub struct Registries {
    lemmabase: LemmaBase,
}

impl Default for Registries {
    fn default() -> Self {
        Self {
            lemmabase: LemmaBase,
        }
    }
}

impl Registries {
    pub fn registry_for(&self, qualifier: &RepositoryQualifier) -> &dyn Registry {
        assert!(
            qualifier.is_registry(),
            "BUG: registry_for called with non-registry qualifier '{}'",
            qualifier.name
        );
        &self.lemmabase
    }
}

// ---------------------------------------------------------------------------
// Suggestions / error mapping
// ---------------------------------------------------------------------------

fn install_failure_suggestion(kind: &RegistryErrorKind) -> Option<String> {
    match kind {
        RegistryErrorKind::NotFound => Some(
            "Check that the repository qualifier is spelled correctly and that the repository exists on LemmaBase."
                .to_string(),
        ),
        RegistryErrorKind::Unauthorized => Some(
            "Check your authentication credentials or permissions for this repository.".to_string(),
        ),
        RegistryErrorKind::NetworkError => Some("Check your network connection.".to_string()),
        RegistryErrorKind::ServerError => {
            Some("LemmaBase returned an internal error. Try again later.".to_string())
        }
        RegistryErrorKind::Other => None,
    }
}

fn resolve_failure_suggestion(kind: &RegistryErrorKind) -> Option<String> {
    match kind {
        RegistryErrorKind::NotFound => Some(
            "Check that the repository qualifier is spelled correctly and that the repository exists on the registry."
                .to_string(),
        ),
        RegistryErrorKind::Unauthorized => Some(
            "Check your authentication credentials or permissions for this registry.".to_string(),
        ),
        RegistryErrorKind::NetworkError => Some("Check your network connection.".to_string()),
        RegistryErrorKind::ServerError => {
            Some("The registry server returned an internal error. Try again later.".to_string())
        }
        RegistryErrorKind::Other => None,
    }
}

fn volatile_origin_source() -> Source {
    Source::new(
        SourceType::Volatile,
        crate::parsing::ast::Span {
            start: 0,
            end: 0,
            line: 1,
            col: 1,
        },
    )
}

fn registry_error_as_install_error(error: RegistryError, name: &str) -> Error {
    let suggestion = install_failure_suggestion(&error.kind);
    Error::registry(
        error.message,
        volatile_origin_source(),
        name.to_string(),
        error.kind,
        suggestion,
        None,
        None,
    )
}

fn parse_validate_dependency(id: &str, source: &str, limits: &ResourceLimits) -> Result<(), Error> {
    let parsed = crate::parsing::parse(source, SourceType::Dependency(id.to_string()), limits)?;
    for (parsed_repo, _) in &parsed.repositories {
        if let Some(declared) = parsed_repo.name.as_deref() {
            if declared != id {
                return Err(Error::registry(
                    format!(
                        "Registry bundle declares repo '{declared}' but '{id}' was requested"
                    ),
                    volatile_origin_source(),
                    id.to_string(),
                    RegistryErrorKind::Other,
                    Some(
                        "The `repo` declaration in the downloaded source must match the requested repository id"
                            .to_string(),
                    ),
                    None,
                    None,
                ));
            }
        }
    }
    Ok(())
}

fn respond_without_fetch_error() -> Error {
    Error::request(
        "Registry response supplied but no fetch is pending",
        Some("Respond exactly once to each Fetch step; stop after Finished"),
    )
}

// ---------------------------------------------------------------------------
// Install step machine
// ---------------------------------------------------------------------------

pub enum InstallStep {
    Fetch(Fetch),
    Finished(Result<RepositoryInstallResult, Error>),
}

enum InstallState {
    Awaiting { repository: String },
    Done,
}

pub struct Install<'r> {
    registries: &'r Registries,
    limits: ResourceLimits,
    state: InstallState,
}

impl<'r> Install<'r> {
    pub fn start(
        registries: &'r Registries,
        repository: &str,
        limits: ResourceLimits,
    ) -> (Self, InstallStep) {
        let qualifier = match crate::parsing::parse_repository_qualifier_str(repository) {
            Ok(q) => q,
            Err(e) => {
                return (
                    Self {
                        registries,
                        limits,
                        state: InstallState::Done,
                    },
                    InstallStep::Finished(Err(Error::registry(
                        e.message().to_string(),
                        volatile_origin_source(),
                        repository.trim().to_string(),
                        RegistryErrorKind::Other,
                        Some("Use a LemmaBase repository id like @owner/name".to_string()),
                        None,
                        None,
                    ))),
                );
            }
        };
        if !qualifier.is_registry() {
            let id = qualifier.name.clone();
            return (
                Self {
                    registries,
                    limits,
                    state: InstallState::Done,
                },
                InstallStep::Finished(Err(Error::registry(
                    format!("Registry identifier must start with '@' (got '{id}')"),
                    volatile_origin_source(),
                    id,
                    RegistryErrorKind::Other,
                    Some("Use a LemmaBase repository id like @owner/name".to_string()),
                    None,
                    None,
                ))),
            );
        }
        let registry = registries.registry_for(&qualifier);
        match registry.fetch_for(&qualifier) {
            Ok(fetch) => {
                let repository = fetch.repository.clone();
                (
                    Self {
                        registries,
                        limits,
                        state: InstallState::Awaiting { repository },
                    },
                    InstallStep::Fetch(fetch),
                )
            }
            Err(e) => (
                Self {
                    registries,
                    limits,
                    state: InstallState::Done,
                },
                InstallStep::Finished(Err(e)),
            ),
        }
    }

    /// A response when no fetch is pending (the install already finished) yields
    /// `Finished(Err)`: callers drive this machine from host code across SDK boundaries.
    pub fn respond(&mut self, response: Result<HttpResponse, TransportFailure>) -> InstallStep {
        let repository = match &self.state {
            InstallState::Awaiting { repository } => repository.clone(),
            InstallState::Done => return InstallStep::Finished(Err(respond_without_fetch_error())),
        };
        let qualifier = RepositoryQualifier::new(repository.clone());
        let registry = self.registries.registry_for(&qualifier);
        let step = match registry.bundle_from(&repository, response) {
            Ok(bundle) => {
                match parse_validate_dependency(&bundle.repository, &bundle.source, &self.limits) {
                    Ok(()) => InstallStep::Finished(Ok(RepositoryInstallResult {
                        source: bundle.source,
                        id: bundle.repository,
                    })),
                    Err(e) => InstallStep::Finished(Err(e)),
                }
            }
            Err(error) => {
                InstallStep::Finished(Err(registry_error_as_install_error(error, &repository)))
            }
        };
        self.state = InstallState::Done;
        step
    }

    pub fn run<T: HttpTransport>(
        registries: &Registries,
        repository: &str,
        transport: &T,
        limits: ResourceLimits,
    ) -> Result<RepositoryInstallResult, Error> {
        let (mut install, step) = Install::start(registries, repository, limits);
        match step {
            InstallStep::Finished(result) => result,
            InstallStep::Fetch(fetch) => {
                let response = transport.get(&fetch);
                match install.respond(response) {
                    InstallStep::Finished(result) => result,
                    InstallStep::Fetch(_) => {
                        panic!("BUG: Install yielded a second Fetch")
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Resolve step machine
// ---------------------------------------------------------------------------

pub enum ResolveStep {
    Fetch(Fetch),
    Finished(Result<(), Vec<Error>>),
}

struct PendingReference {
    repository: RepositoryQualifier,
    source: Source,
}

pub struct Resolve<'a> {
    registries: &'a Registries,
    ctx: &'a mut Context,
    sources: &'a mut HashMap<SourceType, String>,
    limits: &'a ResourceLimits,
    already_requested: HashSet<String>,
    pending: VecDeque<PendingReference>,
    awaiting: Option<PendingReference>,
    round_errors: Vec<Error>,
}

impl<'a> Resolve<'a> {
    pub fn start(
        registries: &'a Registries,
        ctx: &'a mut Context,
        sources: &'a mut HashMap<SourceType, String>,
        limits: &'a ResourceLimits,
    ) -> (Self, ResolveStep) {
        let mut resolve = Self {
            registries,
            ctx,
            sources,
            limits,
            already_requested: HashSet::new(),
            pending: VecDeque::new(),
            awaiting: None,
            round_errors: Vec::new(),
        };
        let step = resolve.next_fetch_or_advance();
        (resolve, step)
    }

    /// A response when no fetch is pending (resolution already finished) yields
    /// `Finished(Err)`: callers drive this machine from host code across SDK boundaries.
    pub fn respond(&mut self, response: Result<HttpResponse, TransportFailure>) -> ResolveStep {
        let Some(reference) = self.awaiting.take() else {
            return ResolveStep::Finished(Err(vec![respond_without_fetch_error()]));
        };

        let registry = self.registries.registry_for(&reference.repository);
        match registry.bundle_from(&reference.repository.name, response) {
            Ok(bundle) => {
                let source_type = SourceType::Dependency(reference.repository.name.clone());
                self.sources
                    .insert(source_type.clone(), bundle.source.clone());

                match crate::parsing::parse(&bundle.source, source_type.clone(), self.limits) {
                    Ok(parsed) => {
                        for (parsed_repo, specs) in parsed.repositories {
                            if let Some(declared) = parsed_repo.name.as_deref() {
                                if declared != reference.repository.name.as_str() {
                                    self.round_errors.push(Error::registry(
                                        format!(
                                            "Registry bundle declares repo '{declared}' but '{}' was requested",
                                            reference.repository.name
                                        ),
                                        reference.source.clone(),
                                        reference.repository.name.clone(),
                                        RegistryErrorKind::Other,
                                        Some(
                                            "The `repo` declaration in the downloaded source must match the requested repository id"
                                                .to_string(),
                                        ),
                                        None,
                                        None,
                                    ));
                                    continue;
                                }
                            }
                            let repo_name = parsed_repo
                                .name
                                .clone()
                                .unwrap_or_else(|| reference.repository.name.clone());
                            let dep_id = reference.repository.name.clone();
                            let header = LemmaRepository::new(Some(repo_name))
                                .with_dependency(dep_id)
                                .with_start_line(parsed_repo.start_line)
                                .with_source_type(source_type.clone());
                            let repository_arc = Arc::new(header);
                            for spec in specs {
                                if let Err(es) =
                                    self.ctx.insert_spec(Arc::clone(&repository_arc), spec)
                                {
                                    self.round_errors.extend(es);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        self.round_errors.push(e);
                        return ResolveStep::Finished(Err(std::mem::take(&mut self.round_errors)));
                    }
                }
            }
            Err(error) => {
                let suggestion = resolve_failure_suggestion(&error.kind);
                let spec_context = self
                    .ctx
                    .iter()
                    .find(|s| s.source_type == Some(reference.source.source_type.clone()));
                self.round_errors.push(Error::registry(
                    error.message,
                    reference.source.clone(),
                    reference.repository.name.clone(),
                    error.kind,
                    suggestion,
                    spec_context,
                    None,
                ));
            }
        }

        self.next_fetch_or_advance()
    }

    pub fn run<T: HttpTransport>(
        registries: &Registries,
        ctx: &mut Context,
        sources: &mut HashMap<SourceType, String>,
        limits: &ResourceLimits,
        transport: &T,
    ) -> Result<(), Vec<Error>> {
        let (mut resolve, mut step) = Resolve::start(registries, ctx, sources, limits);
        loop {
            match step {
                ResolveStep::Finished(result) => return result,
                ResolveStep::Fetch(fetch) => {
                    let response = transport.get(&fetch);
                    step = resolve.respond(response);
                }
            }
        }
    }

    fn next_fetch_or_advance(&mut self) -> ResolveStep {
        loop {
            while let Some(reference) = self.pending.pop_front() {
                if self.already_requested.contains(&reference.repository.name) {
                    continue;
                }
                self.already_requested
                    .insert(reference.repository.name.clone());
                let registry = self.registries.registry_for(&reference.repository);
                match registry.fetch_for(&reference.repository) {
                    Ok(fetch) => {
                        self.awaiting = Some(reference);
                        return ResolveStep::Fetch(fetch);
                    }
                    Err(e) => {
                        self.round_errors.push(e);
                    }
                }
            }

            if !self.round_errors.is_empty() {
                return ResolveStep::Finished(Err(std::mem::take(&mut self.round_errors)));
            }

            let unresolved = find_missing_repositories(self.ctx, &self.already_requested);
            if unresolved.is_empty() {
                return ResolveStep::Finished(Ok(()));
            }
            self.pending = unresolved.into();
        }
    }
}

fn collect_repository_qualifiers_from_spec_ref(
    spec_ref: &crate::parsing::ast::SpecRef,
    source: &Source,
    ctx: &Context,
    already_requested: &HashSet<String>,
    seen_in_this_round: &mut HashSet<String>,
    out: &mut Vec<PendingReference>,
) {
    let Some(qualifier) = spec_ref.repository.as_ref() else {
        return;
    };
    if !qualifier.is_registry() {
        return;
    }
    if ctx.find_repository(&qualifier.name).is_some() {
        return;
    }
    if already_requested.contains(&qualifier.name) {
        return;
    }
    if !seen_in_this_round.insert(qualifier.name.clone()) {
        return;
    }
    out.push(PendingReference {
        repository: qualifier.clone(),
        source: source.clone(),
    });
}

fn find_missing_repositories(
    ctx: &Context,
    already_requested: &HashSet<String>,
) -> Vec<PendingReference> {
    let mut unresolved: Vec<PendingReference> = Vec::new();
    let mut seen_in_this_round: HashSet<String> = HashSet::new();

    for spec in ctx.iter() {
        for data in &spec.data {
            if let crate::parsing::ast::DataValue::Import { spec_ref, .. } = &data.value {
                collect_repository_qualifiers_from_spec_ref(
                    spec_ref,
                    &data.source_location,
                    ctx,
                    already_requested,
                    &mut seen_in_this_round,
                    &mut unresolved,
                );
            }
        }
    }

    unresolved
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Context;
    use crate::literals::DateGranularity;

    struct MapTransport {
        bodies: HashMap<String, String>,
        last_url: std::cell::RefCell<Option<String>>,
        request_count: std::cell::Cell<usize>,
    }

    impl MapTransport {
        fn new(bodies: HashMap<String, String>) -> Self {
            Self {
                bodies,
                last_url: std::cell::RefCell::new(None),
                request_count: std::cell::Cell::new(0),
            }
        }
    }

    impl HttpTransport for MapTransport {
        fn get(&self, fetch: &Fetch) -> Result<HttpResponse, TransportFailure> {
            assert!(
                fetch.url.starts_with("https://lemmabase.com/"),
                "Fetch.url must target LemmaBase, got {}",
                fetch.url
            );
            self.request_count.set(self.request_count.get() + 1);
            *self.last_url.borrow_mut() = Some(fetch.url.clone());
            match self.bodies.get(&fetch.repository) {
                Some(body) => Ok(HttpResponse {
                    status: 200,
                    headers: Vec::new(),
                    body: body.clone(),
                }),
                None => Ok(HttpResponse {
                    status: 404,
                    headers: Vec::new(),
                    body: String::new(),
                }),
            }
        }
    }

    fn context_with_embedded_stdlib() -> Context {
        use crate::engine::EMBEDDED_STDLIB_REPOSITORY;
        use crate::parsing::ast::LemmaRepository;
        use crate::stdlib::UNITS_LEMMA;

        let mut ctx = Context::new();
        let source_type = SourceType::Dependency(EMBEDDED_STDLIB_REPOSITORY.to_string());
        let parsed = crate::parse(UNITS_LEMMA, source_type, &ResourceLimits::default())
            .expect("BUG: embedded stdlib must parse");
        for (parsed_repo, specs) in &parsed.repositories {
            let repository_arc = Arc::new(
                LemmaRepository::new(
                    parsed_repo
                        .name
                        .clone()
                        .or_else(|| Some(EMBEDDED_STDLIB_REPOSITORY.to_string())),
                )
                .with_dependency(EMBEDDED_STDLIB_REPOSITORY)
                .with_start_line(parsed_repo.start_line),
            );
            for spec in specs {
                ctx.insert_spec(Arc::clone(&repository_arc), spec.clone())
                    .expect("BUG: embedded stdlib must load");
            }
        }
        ctx
    }

    // -----------------------------------------------------------------------
    // LemmaBase fetch_for / navigation_url
    // -----------------------------------------------------------------------

    fn qualifier(raw: &str) -> RepositoryQualifier {
        crate::parsing::parse_repository_qualifier_str(raw)
            .unwrap_or_else(|e| panic!("BUG: test qualifier {raw:?} must parse: {}", e.message()))
    }

    #[test]
    fn fetch_for_builds_lemmabase_url() {
        let fetch = LemmaBase
            .fetch_for(&qualifier("@org/project"))
            .expect("fetch_for");
        assert_eq!(fetch.repository, "@org/project");
        assert_eq!(fetch.url, "https://lemmabase.com/@org/project.lemma");
        assert!(fetch.headers.is_empty());
    }

    #[test]
    fn fetch_for_accepts_parsed_whitespace_trimmed_id() {
        let fetch = LemmaBase
            .fetch_for(&qualifier("  @org/project  "))
            .expect("fetch_for");
        assert_eq!(fetch.repository, "@org/project");
        assert_eq!(fetch.url, "https://lemmabase.com/@org/project.lemma");
    }

    #[test]
    fn fetch_for_rejects_id_without_at() {
        let err = LemmaBase
            .fetch_for(&RepositoryQualifier::new("org/project"))
            .expect_err("no @");
        assert_eq!(err.kind(), crate::ErrorKind::Registry);
    }

    #[test]
    fn navigation_url_without_effective() {
        let url = LemmaBase.navigation_url("@org/spec", None);
        assert_eq!(url, Some("https://lemmabase.com/@org/spec".to_string()));
    }

    #[test]
    fn navigation_url_with_effective() {
        let effective = DateTimeValue {
            year: 2026,
            month: 1,
            day: 15,
            hour: 0,
            minute: 0,
            second: 0,
            microsecond: 0,
            timezone: None,
            granularity: DateGranularity::Full,
        };
        let url = LemmaBase.navigation_url("@org/spec", Some(&effective));
        assert_eq!(
            url,
            Some("https://lemmabase.com/@org/spec?effective=2026-01-15".to_string())
        );
    }

    #[test]
    fn navigation_url_rejects_non_at_id() {
        assert!(LemmaBase.navigation_url("iso/countries", None).is_none());
    }

    // -----------------------------------------------------------------------
    // bundle_from status mapping
    // -----------------------------------------------------------------------

    #[test]
    fn bundle_from_maps_404_to_not_found() {
        let err = LemmaBase
            .bundle_from(
                "@missing/repo",
                Ok(HttpResponse {
                    status: 404,
                    headers: Vec::new(),
                    body: String::new(),
                }),
            )
            .expect_err("404");
        assert_eq!(err.kind, RegistryErrorKind::NotFound);
    }

    #[test]
    fn bundle_from_maps_401_to_unauthorized() {
        let err = LemmaBase
            .bundle_from(
                "@org/private",
                Ok(HttpResponse {
                    status: 401,
                    headers: Vec::new(),
                    body: String::new(),
                }),
            )
            .expect_err("401");
        assert_eq!(err.kind, RegistryErrorKind::Unauthorized);
    }

    #[test]
    fn bundle_from_maps_403_to_unauthorized() {
        let err = LemmaBase
            .bundle_from(
                "@org/private",
                Ok(HttpResponse {
                    status: 403,
                    headers: Vec::new(),
                    body: String::new(),
                }),
            )
            .expect_err("403");
        assert_eq!(err.kind, RegistryErrorKind::Unauthorized);
    }

    #[test]
    fn bundle_from_maps_500_to_server_error() {
        let err = LemmaBase
            .bundle_from(
                "@org/broken",
                Ok(HttpResponse {
                    status: 500,
                    headers: Vec::new(),
                    body: String::new(),
                }),
            )
            .expect_err("500");
        assert_eq!(err.kind, RegistryErrorKind::ServerError);
    }

    #[test]
    fn bundle_from_maps_transport_failure_to_network_error() {
        let err = LemmaBase
            .bundle_from(
                "@org/unreachable",
                Err(TransportFailure {
                    message: "connection refused".to_string(),
                }),
            )
            .expect_err("transport");
        assert_eq!(err.kind, RegistryErrorKind::NetworkError);
    }

    #[test]
    fn bundle_from_maps_418_to_other() {
        let err = LemmaBase
            .bundle_from(
                "@org/teapot",
                Ok(HttpResponse {
                    status: 418,
                    headers: Vec::new(),
                    body: String::new(),
                }),
            )
            .expect_err("418");
        assert_eq!(err.kind, RegistryErrorKind::Other);
    }

    // -----------------------------------------------------------------------
    // Install
    // -----------------------------------------------------------------------

    #[test]
    fn install_returns_bundle() {
        let mut bodies = HashMap::new();
        bodies.insert(
            "@iso/countries".to_string(),
            "repo @iso/countries\nspec alpha2\ndata code: text\n".to_string(),
        );
        let transport = MapTransport::new(bodies);
        let registries = Registries::default();
        let result = Install::run(
            &registries,
            "  @iso/countries  ",
            &transport,
            ResourceLimits::default(),
        )
        .expect("install");
        assert_eq!(result.id, "@iso/countries");
        assert!(result.source.contains("spec alpha2"));
        assert_eq!(
            transport.last_url.borrow().as_deref(),
            Some("https://lemmabase.com/@iso/countries.lemma")
        );
    }

    fn ok_response(body: &str) -> Result<HttpResponse, TransportFailure> {
        Ok(HttpResponse {
            status: 200,
            headers: Vec::new(),
            body: body.to_string(),
        })
    }

    #[test]
    fn install_respond_after_finished_is_error() {
        let registries = Registries::default();
        let (mut install, step) =
            Install::start(&registries, "@iso/countries", ResourceLimits::default());
        assert!(matches!(step, InstallStep::Fetch(_)));
        let body = "repo @iso/countries\nspec alpha2\ndata code: text\n";
        assert!(matches!(
            install.respond(ok_response(body)),
            InstallStep::Finished(Ok(_))
        ));
        match install.respond(ok_response(body)) {
            InstallStep::Finished(Err(error)) => {
                assert_eq!(error.kind(), crate::ErrorKind::Request);
                assert!(error.message().contains("no fetch is pending"));
            }
            InstallStep::Finished(Ok(_)) | InstallStep::Fetch(_) => {
                panic!("second respond must be an error")
            }
        }
    }

    #[test]
    fn resolve_respond_without_pending_fetch_is_error() {
        let mut store = context_with_embedded_stdlib();
        let mut sources: HashMap<SourceType, String> = HashMap::new();
        let registries = Registries::default();
        let limits = ResourceLimits::default();
        let (mut resolve, step) = Resolve::start(&registries, &mut store, &mut sources, &limits);
        assert!(matches!(step, ResolveStep::Finished(Ok(()))));
        match resolve.respond(ok_response("spec s\n")) {
            ResolveStep::Finished(Err(errors)) => {
                assert_eq!(errors.len(), 1);
                assert_eq!(errors[0].kind(), crate::ErrorKind::Request);
                assert!(errors[0].message().contains("no fetch is pending"));
            }
            ResolveStep::Finished(Ok(())) | ResolveStep::Fetch(_) => {
                panic!("respond without a pending fetch must be an error")
            }
        }
    }

    #[test]
    fn install_rejects_empty_id() {
        let transport = MapTransport::new(HashMap::new());
        let registries = Registries::default();
        let err = Install::run(&registries, "   ", &transport, ResourceLimits::default())
            .expect_err("empty id");
        assert_eq!(err.kind(), crate::ErrorKind::Registry);
    }

    #[test]
    fn install_rejects_path_injection() {
        let transport = MapTransport::new(HashMap::new());
        let registries = Registries::default();
        let err = Install::run(
            &registries,
            "@org/../secret",
            &transport,
            ResourceLimits::default(),
        )
        .expect_err("path injection");
        assert_eq!(err.kind(), crate::ErrorKind::Registry);
    }

    #[test]
    fn install_rejects_repo_declaration_mismatch() {
        let mut bodies = HashMap::new();
        bodies.insert(
            "@org/requested".to_string(),
            "repo @org/other\n\nspec s\ndata v: 1\nrule r: v\n".to_string(),
        );
        let transport = MapTransport::new(bodies);
        let registries = Registries::default();
        let err = Install::run(
            &registries,
            "@org/requested",
            &transport,
            ResourceLimits::default(),
        )
        .expect_err("repo mismatch");
        assert_eq!(err.kind(), crate::ErrorKind::Registry);
        assert!(
            err.message().contains("@org/other") && err.message().contains("@org/requested"),
            "got: {}",
            err.message()
        );
    }

    #[test]
    fn install_maps_not_found() {
        let transport = MapTransport::new(HashMap::new());
        let registries = Registries::default();
        let err = Install::run(
            &registries,
            "@missing/repo",
            &transport,
            ResourceLimits::default(),
        )
        .expect_err("missing");
        assert_eq!(err.kind(), crate::ErrorKind::Registry);
        assert_eq!(err.registry_kind(), Some(RegistryErrorKind::NotFound));
    }

    // -----------------------------------------------------------------------
    // Resolve
    // -----------------------------------------------------------------------

    #[test]
    fn resolve_with_no_registry_references_returns_local_specs_unchanged() {
        let source = r#"spec example
data price: 100"#;
        let local_specs = crate::parse(source, SourceType::Volatile, &ResourceLimits::default())
            .unwrap()
            .into_flattened_specs();
        let mut store = context_with_embedded_stdlib();
        let local_repository = store.workspace();
        for spec in &local_specs {
            store
                .insert_spec(Arc::clone(&local_repository), spec.clone())
                .unwrap();
        }
        let mut sources: HashMap<SourceType, String> = HashMap::new();
        sources.insert(SourceType::Volatile, source.to_string());

        let registries = Registries::default();
        let transport = MapTransport::new(HashMap::new());
        Resolve::run(
            &registries,
            &mut store,
            &mut sources,
            &ResourceLimits::default(),
            &transport,
        )
        .unwrap();

        assert_eq!(
            store.iter().count(),
            2,
            "embedded spec units plus workspace example"
        );
        let names: Vec<String> = store.iter().map(|a| a.name.clone()).collect();
        assert!(names.iter().any(|n| n == "example"));
        assert!(names.iter().any(|n| n == "units"));
    }

    #[test]
    fn resolve_does_not_fetch_non_at_qualified_repositories() {
        let local_source = r#"spec burn_baby_burn
uses lemma units
rule x: 1 hour"#;
        let local_specs = crate::parse(
            local_source,
            SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap()
        .into_flattened_specs();
        let mut store = Context::new();
        let local_repository = store.workspace();
        for spec in local_specs {
            store
                .insert_spec(Arc::clone(&local_repository), spec)
                .unwrap();
        }
        let mut sources: HashMap<SourceType, String> = HashMap::new();
        sources.insert(SourceType::Volatile, local_source.to_string());

        let registries = Registries::default();
        let transport = MapTransport::new(HashMap::new());
        let result = Resolve::run(
            &registries,
            &mut store,
            &mut sources,
            &ResourceLimits::default(),
            &transport,
        );

        assert!(
            result.is_ok(),
            "non-@ repository qualifiers must not be sent to the registry, got: {:?}",
            result.err()
        );
        assert!(transport.last_url.borrow().is_none());
    }

    #[test]
    fn resolve_fetches_single_spec_from_registry() {
        let local_source = r#"spec main_spec
uses external: @org/project helper
rule value: external.quantity"#;
        let local_specs = crate::parse(
            local_source,
            SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap()
        .into_flattened_specs();
        let mut store = context_with_embedded_stdlib();
        let local_repository = store.workspace();
        for spec in local_specs {
            store
                .insert_spec(Arc::clone(&local_repository), spec)
                .unwrap();
        }
        let mut sources: HashMap<SourceType, String> = HashMap::new();
        sources.insert(SourceType::Volatile, local_source.to_string());

        let mut bodies = HashMap::new();
        bodies.insert(
            "@org/project".to_string(),
            "repo @org/project\nspec helper\ndata quantity: 42".to_string(),
        );
        let transport = MapTransport::new(bodies);
        let registries = Registries::default();

        Resolve::run(
            &registries,
            &mut store,
            &mut sources,
            &ResourceLimits::default(),
            &transport,
        )
        .unwrap();

        assert_eq!(store.iter().count(), 3);
        let names: Vec<String> = store.iter().map(|a| a.name.clone()).collect();
        assert!(names.iter().any(|n| n == "main_spec"));
        assert!(names.iter().any(|n| n == "helper"));
        assert!(names.iter().any(|n| n == "units"));
        assert_eq!(
            transport.last_url.borrow().as_deref(),
            Some("https://lemmabase.com/@org/project.lemma")
        );
    }

    #[test]
    fn resolve_registry_bundle_without_repo_decl_uses_reference_repository_name() {
        let local_source = r#"spec main_spec
uses external: @org/project helper
rule value: external.quantity"#;
        let local_specs = crate::parse(
            local_source,
            SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap()
        .into_flattened_specs();
        let mut store = context_with_embedded_stdlib();
        let local_repository = store.workspace();
        for spec in local_specs {
            store
                .insert_spec(Arc::clone(&local_repository), spec)
                .unwrap();
        }
        let mut sources: HashMap<SourceType, String> = HashMap::new();
        sources.insert(SourceType::Volatile, local_source.to_string());

        let mut bodies = HashMap::new();
        bodies.insert(
            "@org/project".to_string(),
            "spec helper\ndata quantity: 42".to_string(),
        );
        let transport = MapTransport::new(bodies);
        let registries = Registries::default();

        Resolve::run(
            &registries,
            &mut store,
            &mut sources,
            &ResourceLimits::default(),
            &transport,
        )
        .unwrap();

        assert!(store.find_repository("@org/project").is_some());
    }

    #[test]
    fn resolve_fetches_transitive_dependencies() {
        let local_source = r#"spec main_spec
uses a: @org/a helper_a
rule value: a.x"#;
        let local_specs = crate::parse(
            local_source,
            SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap()
        .into_flattened_specs();
        let mut store = context_with_embedded_stdlib();
        let local_repository = store.workspace();
        for spec in local_specs {
            store
                .insert_spec(Arc::clone(&local_repository), spec)
                .unwrap();
        }
        let mut sources: HashMap<SourceType, String> = HashMap::new();
        sources.insert(SourceType::Volatile, local_source.to_string());

        let mut bodies = HashMap::new();
        bodies.insert(
            "@org/a".to_string(),
            "repo @org/a\nspec helper_a\nuses b: @org/b helper_b\ndata x: b.y".to_string(),
        );
        bodies.insert(
            "@org/b".to_string(),
            "repo @org/b\nspec helper_b\ndata y: 7".to_string(),
        );
        let transport = MapTransport::new(bodies);
        let registries = Registries::default();

        Resolve::run(
            &registries,
            &mut store,
            &mut sources,
            &ResourceLimits::default(),
            &transport,
        )
        .unwrap();

        let names: Vec<String> = store.iter().map(|a| a.name.clone()).collect();
        assert!(names.iter().any(|n| n == "helper_a"));
        assert!(names.iter().any(|n| n == "helper_b"));
    }

    #[test]
    fn resolve_handles_bundle_with_multiple_specs() {
        let local_source = r#"spec main_spec
uses a: @org/multi alpha
rule value: a.x"#;
        let local_specs = crate::parse(
            local_source,
            SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap()
        .into_flattened_specs();
        let mut store = context_with_embedded_stdlib();
        let local_repository = store.workspace();
        for spec in local_specs {
            store
                .insert_spec(Arc::clone(&local_repository), spec)
                .unwrap();
        }
        let mut sources: HashMap<SourceType, String> = HashMap::new();
        sources.insert(SourceType::Volatile, local_source.to_string());

        let mut bodies = HashMap::new();
        bodies.insert(
            "@org/multi".to_string(),
            "repo @org/multi\nspec alpha\ndata x: 1\nspec second\ndata y: 2".to_string(),
        );
        let transport = MapTransport::new(bodies);
        let registries = Registries::default();

        Resolve::run(
            &registries,
            &mut store,
            &mut sources,
            &ResourceLimits::default(),
            &transport,
        )
        .unwrap();

        let names: Vec<String> = store.iter().map(|a| a.name.clone()).collect();
        assert!(names.iter().any(|n| n == "alpha"));
        assert!(names.iter().any(|n| n == "second"));
    }

    #[test]
    fn resolve_returns_registry_error_when_registry_fails() {
        let local_source = r#"spec main_spec
uses external: @org/missing helper
rule value: external.quantity"#;
        let local_specs = crate::parse(
            local_source,
            SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap()
        .into_flattened_specs();
        let mut store = context_with_embedded_stdlib();
        let local_repository = store.workspace();
        for spec in local_specs {
            store
                .insert_spec(Arc::clone(&local_repository), spec)
                .unwrap();
        }
        let mut sources: HashMap<SourceType, String> = HashMap::new();
        sources.insert(SourceType::Volatile, local_source.to_string());

        let registries = Registries::default();
        let transport = MapTransport::new(HashMap::new());
        let errs = Resolve::run(
            &registries,
            &mut store,
            &mut sources,
            &ResourceLimits::default(),
            &transport,
        )
        .expect_err("missing");
        assert!(!errs.is_empty());
        assert_eq!(errs[0].kind(), crate::ErrorKind::Registry);
        assert_eq!(errs[0].registry_kind(), Some(RegistryErrorKind::NotFound));
    }

    #[test]
    fn resolve_returns_all_registry_errors_when_multiple_repositories_fail() {
        let local_source = r#"spec main_spec
uses @org/example helper
uses @iso/countries alpha2
data country: alpha2.code"#;
        let local_specs = crate::parse(
            local_source,
            SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap()
        .into_flattened_specs();
        let mut store = context_with_embedded_stdlib();
        let local_repository = store.workspace();
        for spec in local_specs {
            store
                .insert_spec(Arc::clone(&local_repository), spec)
                .unwrap();
        }
        let mut sources: HashMap<SourceType, String> = HashMap::new();
        sources.insert(SourceType::Volatile, local_source.to_string());

        let registries = Registries::default();
        let transport = MapTransport::new(HashMap::new());
        let errs = Resolve::run(
            &registries,
            &mut store,
            &mut sources,
            &ResourceLimits::default(),
            &transport,
        )
        .expect_err("both missing");
        let identifiers: Vec<&str> = errs.iter().filter_map(|e| e.repository()).collect();
        assert!(
            identifiers.contains(&"@org/example"),
            "Should include repository error: {:?}",
            identifiers
        );
        assert!(
            identifiers.contains(&"@iso/countries"),
            "Should include data import repository error: {:?}",
            identifiers
        );
    }

    #[test]
    fn resolve_does_not_request_same_repository_twice() {
        let local_source = r#"spec spec_one
uses a: @org/shared shared

spec spec_two
uses b: @org/shared shared"#;
        let local_specs = crate::parse(
            local_source,
            SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap()
        .into_flattened_specs();
        let mut store = context_with_embedded_stdlib();
        let local_repository = store.workspace();
        for spec in local_specs {
            store
                .insert_spec(Arc::clone(&local_repository), spec)
                .unwrap();
        }
        let mut sources: HashMap<SourceType, String> = HashMap::new();
        sources.insert(SourceType::Volatile, local_source.to_string());

        let mut bodies = HashMap::new();
        bodies.insert(
            "@org/shared".to_string(),
            "repo @org/shared\nspec shared\ndata value: 1".to_string(),
        );
        let transport = MapTransport::new(bodies);
        let registries = Registries::default();
        Resolve::run(
            &registries,
            &mut store,
            &mut sources,
            &ResourceLimits::default(),
            &transport,
        )
        .unwrap();

        assert_eq!(transport.request_count.get(), 1);
        assert_eq!(store.iter().count(), 4);
        let names: Vec<String> = store.iter().map(|a| a.name.clone()).collect();
        assert!(names.iter().any(|n| n == "shared"));
        assert!(names.iter().any(|n| n == "units"));
    }

    #[test]
    fn resolve_handles_data_import_from_registry() {
        let local_source = r#"spec main_spec
uses @iso/countries alpha2
data country: alpha2.code
data home: country"#;
        let local_specs = crate::parse(
            local_source,
            SourceType::Volatile,
            &ResourceLimits::default(),
        )
        .unwrap()
        .into_flattened_specs();
        let mut store = context_with_embedded_stdlib();
        let local_repository = store.workspace();
        for spec in local_specs {
            store
                .insert_spec(Arc::clone(&local_repository), spec)
                .unwrap();
        }
        let mut sources: HashMap<SourceType, String> = HashMap::new();
        sources.insert(SourceType::Volatile, local_source.to_string());

        let mut bodies = HashMap::new();
        bodies.insert(
            "@iso/countries".to_string(),
            "repo @iso/countries\nspec alpha2\ndata code: text\n -> option \"NL\"".to_string(),
        );
        let transport = MapTransport::new(bodies);
        let registries = Registries::default();
        Resolve::run(
            &registries,
            &mut store,
            &mut sources,
            &ResourceLimits::default(),
            &transport,
        )
        .unwrap();

        assert_eq!(store.iter().count(), 3);
        let names: Vec<String> = store.iter().map(|a| a.name.clone()).collect();
        assert!(names.iter().any(|n| n == "main_spec"));
        assert!(names.iter().any(|n| n == "alpha2"));
        assert!(names.iter().any(|n| n == "units"));
    }
}

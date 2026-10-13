//! Shared LemmaBase install for CLI `install` and MCP `install`.
//!
//! Install = download + persist into `lemma_deps/`.

use crate::deps::{lemma_deps_dir, relative_dependency_cache_path};
use crate::workspace::{self, WorkspaceDiskError};
use lemma::Engine;
use lemma::{Fetch, Header, HttpResponse, HttpTransport, Install, Registries, TransportFailure};
use std::collections::HashSet;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use walkdir::WalkDir;

fn registry_runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Runtime::new().expect("BUG: failed to create registry install runtime")
    })
}

/// Drive an async future on the process-wide install runtime.
pub fn block_on_registry<F: std::future::Future>(future: F) -> F::Output {
    registry_runtime().block_on(future)
}

// ---------------------------------------------------------------------------
// ReqwestTransport — host-owned HTTP for LemmaBase
// ---------------------------------------------------------------------------

pub struct ReqwestTransport {
    http: reqwest::Client,
}

impl ReqwestTransport {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("BUG: reqwest client builder must succeed with default TLS");
        Self { http }
    }
}

impl Default for ReqwestTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpTransport for ReqwestTransport {
    fn get(&self, fetch: &Fetch) -> Result<HttpResponse, TransportFailure> {
        block_on_registry(async {
            let mut request = self.http.get(&fetch.url);
            for header in &fetch.headers {
                request = request.header(&header.name, &header.value);
            }
            let response = request.send().await.map_err(|e| TransportFailure {
                message: e.to_string(),
            })?;
            let status = response.status().as_u16();
            let mut headers = Vec::new();
            for (name, value) in response.headers() {
                match value.to_str() {
                    Ok(value) => headers.push(Header {
                        name: name.as_str().to_string(),
                        value: value.to_string(),
                    }),
                    Err(_) => {
                        return Err(TransportFailure {
                            message: format!(
                                "response header '{}' is not valid UTF-8",
                                name.as_str()
                            ),
                        });
                    }
                }
            }
            let body = response.text().await.map_err(|e| TransportFailure {
                message: e.to_string(),
            })?;
            Ok(HttpResponse {
                status,
                headers,
                body,
            })
        })
    }
}

// ---------------------------------------------------------------------------
// InstallOutcome / InstallError
// ---------------------------------------------------------------------------

/// Result of a successful single-id registry install.
#[derive(Debug)]
pub enum InstallOutcome {
    Written {
        relative_path: PathBuf,
        source: String,
    },
    AlreadyUpToDate {
        relative_path: PathBuf,
        source: String,
    },
}

/// Failure installing a LemmaBase repository.
pub enum InstallError {
    Registry(lemma::Error),
    UnparseableRegistry(lemma::Error),
    Conflict {
        spec_names: Vec<String>,
        path: PathBuf,
    },
    Io(io::Error),
    Workspace(WorkspaceDiskError),
    Plan(lemma::Errors),
}

impl std::fmt::Debug for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self}")
    }
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Registry(error) => write!(f, "LemmaBase error: {error}"),
            Self::UnparseableRegistry(error) => {
                write!(f, "LemmaBase returned unparseable repository: {error}")
            }
            Self::Conflict { spec_names, path } => write!(
                f,
                "Repository containing spec(s) {} already exists in {}.\n\
                 Content has changed on LemmaBase. Re-run with --force to overwrite.",
                spec_names.join(", "),
                path.display()
            ),
            Self::Io(error) => write!(f, "{error}"),
            Self::Workspace(error) => write!(f, "{error}"),
            Self::Plan(errors) => write!(
                f,
                "Planning installed repository failed ({} error(s))",
                errors.errors.len()
            ),
        }
    }
}

impl From<io::Error> for InstallError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<WorkspaceDiskError> for InstallError {
    fn from(error: WorkspaceDiskError) -> Self {
        match error {
            WorkspaceDiskError::EngineLoad(errors) => Self::Plan(errors),
            other => Self::Workspace(other),
        }
    }
}

/// Download, conflict-scan, plan-probe, and persist one LemmaBase repository.
pub fn install_registry_dependency<T: HttpTransport>(
    workdir: &Path,
    id: &str,
    force: bool,
    registries: &Registries,
    transport: &T,
) -> Result<InstallOutcome, InstallError> {
    let result = Install::run(registries, id, transport, lemma::ResourceLimits::default())
        .map_err(|error| {
            if error.kind() == lemma::ErrorKind::Parsing {
                InstallError::UnparseableRegistry(error)
            } else {
                InstallError::Registry(error)
            }
        })?;
    persist_registry_dependency(workdir, &result.id, &result.source, force)
}

fn spec_names_from_source(repository: &str, source: &str) -> Result<HashSet<String>, InstallError> {
    let limits = lemma::ResourceLimits::default();
    let specs = lemma::parse(
        source,
        lemma::SourceType::Dependency(repository.to_string()),
        &limits,
    )
    .map_err(InstallError::UnparseableRegistry)?
    .into_flattened_specs();
    Ok(specs.into_iter().map(|spec| spec.name).collect())
}

/// Conflict-scan, plan-probe, and persist one LemmaBase repository source.
///
/// Used by single-id install after download, and by `install --all` after
/// `Resolve::run` (no second download).
///
/// Order: write tmp → plan probe → on fail remove tmp; on ok remove
/// `conflict_paths` (when force) then rename tmp → `destination_absolute`.
pub fn persist_registry_dependency(
    workdir: &Path,
    id: &str,
    source: &str,
    force: bool,
) -> Result<InstallOutcome, InstallError> {
    let spec_names = spec_names_from_source(id, source)?;
    let scan = scan_lemma_deps(workdir, id, &spec_names)?;

    if scan.destination_absolute.is_file() {
        let existing = fs::read_to_string(&scan.destination_absolute)?;
        if existing == source {
            return Ok(InstallOutcome::AlreadyUpToDate {
                relative_path: scan.destination_relative,
                source: source.to_string(),
            });
        }
    }

    if !scan.conflict_paths.is_empty() && !force {
        let path = scan.conflict_paths[0].clone();
        let existing_content = fs::read_to_string(&path)?;
        let limits = lemma::ResourceLimits::default();
        let existing_specs = lemma::parse(
            &existing_content,
            lemma::SourceType::Path(Arc::new(path.clone())),
            &limits,
        )
        .map_err(|error| {
            InstallError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unparseable lemma_deps file {}: {error}", path.display()),
            ))
        })?
        .into_flattened_specs();
        let conflict: Vec<String> = existing_specs
            .iter()
            .filter(|spec| spec_names.contains(&spec.name))
            .map(|spec| spec.name.clone())
            .collect();
        return Err(InstallError::Conflict {
            spec_names: conflict,
            path,
        });
    }

    let tmp = write_tmp(&scan.destination_absolute, source)?;

    let mut exclude: Vec<&Path> = vec![scan.destination_absolute.as_path()];
    if force {
        for path in &scan.conflict_paths {
            exclude.push(path.as_path());
        }
    }

    let probe_result = (|| -> Result<(), InstallError> {
        let mut probe = Engine::new();
        workspace::load_workspace_excluding(&mut probe, workdir, &exclude)?;
        probe
            .load([(
                lemma::SourceType::Dependency(id.to_string()),
                source.to_string(),
            )])
            .map_err(InstallError::Plan)?;
        Ok(())
    })();

    if let Err(error) = probe_result {
        remove_tmp_after_failure(&tmp);
        return Err(error);
    }

    if force {
        for path in &scan.conflict_paths {
            if path != &scan.destination_absolute {
                fs::remove_file(path)?;
            }
        }
    }

    rename_tmp(&tmp, &scan.destination_absolute)?;

    Ok(InstallOutcome::Written {
        relative_path: scan.destination_relative,
        source: source.to_string(),
    })
}

/// Result of walking `lemma_deps/` against a candidate repository source.
pub struct LemmaDepsScan {
    pub destination_absolute: PathBuf,
    pub destination_relative: PathBuf,
    /// Paths that define overlapping spec names with the candidate.
    pub conflict_paths: Vec<PathBuf>,
}

/// Walk `lemma_deps/` for spec-name conflicts.
/// Unparseable or unreadable `.lemma` files are errors.
pub fn scan_lemma_deps(
    workdir: &Path,
    repository: &str,
    new_spec_names: &HashSet<String>,
) -> Result<LemmaDepsScan, InstallError> {
    let deps_dir = lemma_deps_dir(workdir);
    let destination_relative = relative_dependency_cache_path(repository);
    let destination_absolute = deps_dir.join(&destination_relative);
    let limits = lemma::ResourceLimits::default();

    let mut conflict_paths = Vec::new();

    if deps_dir.exists() {
        for entry in WalkDir::new(&deps_dir) {
            let entry = entry.map_err(|error| InstallError::Io(io::Error::other(error)))?;
            if entry.path().extension().and_then(|s| s.to_str()) != Some("lemma") {
                continue;
            }
            let path = entry.path();
            let existing_content = fs::read_to_string(path)?;
            let existing_specs = lemma::parse(
                &existing_content,
                lemma::SourceType::Path(Arc::new(path.to_path_buf())),
                &limits,
            )
            .map_err(|error| {
                InstallError::Io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("unparseable lemma_deps file {}: {error}", path.display()),
                ))
            })?
            .into_flattened_specs();
            let has_conflict = existing_specs
                .iter()
                .any(|spec| new_spec_names.contains(&spec.name));
            if has_conflict {
                conflict_paths.push(path.to_path_buf());
            }
        }
    }

    Ok(LemmaDepsScan {
        destination_absolute,
        destination_relative,
        conflict_paths,
    })
}

/// Temp path beside `path`: `.{file_name}.tmp` in the same directory.
fn tmp_path_for(path: &Path) -> io::Result<PathBuf> {
    let file_name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "persist path must include a file name",
        )
    })?;
    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    fs::create_dir_all(dir)?;
    Ok(dir.join(format!(".{}.tmp", file_name.to_string_lossy())))
}

/// Write `contents` to `.{file}.tmp` beside `path`, fsync. Does not rename.
pub fn write_tmp(path: &Path, contents: &str) -> io::Result<PathBuf> {
    let tmp = tmp_path_for(path)?;
    match (|| -> io::Result<()> {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(contents.as_bytes())?;
        f.sync_all()?;
        Ok(())
    })() {
        Ok(()) => Ok(tmp),
        Err(e) => {
            remove_tmp_after_failure(&tmp);
            Err(e)
        }
    }
}

fn remove_tmp_after_failure(tmp: &Path) {
    if let Err(error) = fs::remove_file(tmp) {
        if error.kind() != io::ErrorKind::NotFound {
            tracing::warn!("failed to remove temp file {}: {error}", tmp.display());
        }
    }
}

/// Rename a temp file produced by [`write_tmp`] onto `path`.
pub fn rename_tmp(tmp: &Path, path: &Path) -> io::Result<()> {
    fs::rename(tmp, path)
}

/// Atomic write: [`write_tmp`] then [`rename_tmp`].
pub fn atomic_write(path: &Path, contents: &str) -> io::Result<()> {
    let tmp = write_tmp(path, contents)?;
    match rename_tmp(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            remove_tmp_after_failure(&tmp);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn test_transport() -> (Registries, lemma::__test_support::FixtureTransport) {
        (
            Registries::default(),
            lemma::__test_support::FixtureTransport::bundled(),
        )
    }

    #[test]
    fn write_tmp_then_rename_tmp_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.lemma");
        let tmp = write_tmp(&path, "spec x\ndata a: 1\n").unwrap();
        assert!(tmp.exists(), "tmp must exist before rename");
        assert!(!path.exists(), "final path must not exist before rename");
        rename_tmp(&tmp, &path).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "spec x\ndata a: 1\n");
        assert!(!tmp.exists(), "tmp must be gone after rename");
    }

    #[test]
    fn atomic_write_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.lemma");
        atomic_write(&path, "spec x\ndata a: 1\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "spec x\ndata a: 1\n");
        assert!(
            !dir.path().join(".out.lemma.tmp").exists(),
            "temp file must be cleaned up"
        );
    }

    #[test]
    fn scan_lemma_deps_rejects_unparseable_cache_file() {
        let dir = tempfile::tempdir().unwrap();
        let junk = dir.path().join("lemma_deps").join("@junk");
        fs::create_dir_all(&junk).unwrap();
        fs::write(junk.join("corrupt.lemma"), "{{{not valid lemma").unwrap();

        let mut names = HashSet::new();
        names.insert("alpha2".to_string());

        let result = scan_lemma_deps(dir.path(), "@iso/countries", &names);
        assert!(
            result.is_err(),
            "unparseable lemma_deps .lemma must hard-error"
        );
    }

    #[test]
    fn install_writes_fixture_dependency() {
        let dir = tempfile::tempdir().unwrap();
        let (registries, transport) = test_transport();
        let outcome = install_registry_dependency(
            dir.path(),
            "@iso/countries",
            false,
            &registries,
            &transport,
        )
        .unwrap();
        match outcome {
            InstallOutcome::Written { relative_path, .. } => {
                assert_eq!(relative_path, PathBuf::from("@iso").join("countries.lemma"));
            }
            InstallOutcome::AlreadyUpToDate { .. } => panic!("expected Written"),
        }
        assert!(dir
            .path()
            .join("lemma_deps")
            .join("@iso")
            .join("countries.lemma")
            .exists());
        assert!(
            !dir.path()
                .join("lemma_deps")
                .join("@iso")
                .join(".countries.lemma.tmp")
                .exists(),
            "tmp must not remain after success"
        );
    }

    #[test]
    fn install_destination_match_is_up_to_date() {
        let dir = tempfile::tempdir().unwrap();
        let (registries, transport) = test_transport();
        install_registry_dependency(dir.path(), "@iso/countries", false, &registries, &transport)
            .unwrap();
        let second = install_registry_dependency(
            dir.path(),
            "@iso/countries",
            false,
            &registries,
            &transport,
        )
        .unwrap();
        assert!(
            matches!(second, InstallOutcome::AlreadyUpToDate { .. }),
            "canonical destination match must be up to date"
        );
    }

    #[test]
    fn install_identical_content_elsewhere_conflicts_without_force() {
        let dir = tempfile::tempdir().unwrap();
        let (registries, transport) = test_transport();
        install_registry_dependency(dir.path(), "@iso/countries", false, &registries, &transport)
            .unwrap();
        let other = dir
            .path()
            .join("lemma_deps")
            .join("@other")
            .join("copy.lemma");
        fs::create_dir_all(other.parent().unwrap()).unwrap();
        let dest = dir
            .path()
            .join("lemma_deps")
            .join("@iso")
            .join("countries.lemma");
        fs::rename(&dest, &other).unwrap();

        let err = install_registry_dependency(
            dir.path(),
            "@iso/countries",
            false,
            &registries,
            &transport,
        )
        .expect_err("foreign copy with overlapping specs must conflict");
        assert!(
            matches!(err, InstallError::Conflict { .. }),
            "expected Conflict, got: {err}"
        );
        assert!(!dest.exists(), "conflict must not write destination");
        assert!(other.exists(), "foreign copy must remain without force");
    }

    #[test]
    fn install_identical_content_elsewhere_force_writes_canonical() {
        let dir = tempfile::tempdir().unwrap();
        let (registries, transport) = test_transport();
        let first = install_registry_dependency(
            dir.path(),
            "@iso/countries",
            false,
            &registries,
            &transport,
        )
        .unwrap();
        let source = match first {
            InstallOutcome::Written { source, .. } => source,
            InstallOutcome::AlreadyUpToDate { source, .. } => source,
        };
        let other = dir
            .path()
            .join("lemma_deps")
            .join("@other")
            .join("copy.lemma");
        fs::create_dir_all(other.parent().unwrap()).unwrap();
        let dest = dir
            .path()
            .join("lemma_deps")
            .join("@iso")
            .join("countries.lemma");
        fs::rename(&dest, &other).unwrap();

        let second = install_registry_dependency(
            dir.path(),
            "@iso/countries",
            true,
            &registries,
            &transport,
        )
        .unwrap();
        match second {
            InstallOutcome::Written { relative_path, .. } => {
                assert_eq!(relative_path, PathBuf::from("@iso").join("countries.lemma"));
            }
            InstallOutcome::AlreadyUpToDate { .. } => {
                panic!("force install must write canonical destination")
            }
        }
        assert!(dest.exists(), "canonical destination must be written");
        assert_eq!(fs::read_to_string(&dest).unwrap(), source);
        assert!(
            !other.exists(),
            "force must remove conflicting foreign copy"
        );
        assert!(
            !dest
                .parent()
                .expect("parent")
                .join(".countries.lemma.tmp")
                .exists(),
            "tmp must not remain after force success"
        );
    }

    #[test]
    fn install_workspace_spec_name_collision_is_plan_error() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("local.lemma"),
            "repo @iso/countries\nspec alpha2\ndata code: text\n",
        )
        .unwrap();
        let (registries, transport) = test_transport();
        let err = install_registry_dependency(
            dir.path(),
            "@iso/countries",
            false,
            &registries,
            &transport,
        )
        .expect_err("workspace name collision must fail plan probe");
        assert!(
            matches!(err, InstallError::Plan(_)),
            "expected Plan, got: {err}"
        );
        assert!(
            !dir.path()
                .join("lemma_deps")
                .join("@iso")
                .join("countries.lemma")
                .exists(),
            "plan failure must not write destination"
        );
        assert!(
            !dir.path()
                .join("lemma_deps")
                .join("@iso")
                .join(".countries.lemma.tmp")
                .exists(),
            "plan failure must not leave tmp"
        );
    }

    #[test]
    fn install_force_plan_failure_leaves_conflict_paths_and_destination() {
        let dir = tempfile::tempdir().unwrap();
        let (registries, transport) = test_transport();
        let first = install_registry_dependency(
            dir.path(),
            "@iso/countries",
            false,
            &registries,
            &transport,
        )
        .unwrap();
        let source = match first {
            InstallOutcome::Written { source, .. } => source,
            InstallOutcome::AlreadyUpToDate { source, .. } => source,
        };
        let other = dir
            .path()
            .join("lemma_deps")
            .join("@other")
            .join("copy.lemma");
        fs::create_dir_all(other.parent().unwrap()).unwrap();
        let dest = dir
            .path()
            .join("lemma_deps")
            .join("@iso")
            .join("countries.lemma");
        fs::rename(&dest, &other).unwrap();

        fs::write(
            dir.path().join("local.lemma"),
            "repo @iso/countries\nspec alpha2\ndata code: text\n",
        )
        .unwrap();

        let err = install_registry_dependency(
            dir.path(),
            "@iso/countries",
            true,
            &registries,
            &transport,
        )
        .expect_err("workspace collision must fail even with force");
        assert!(
            matches!(err, InstallError::Plan(_)),
            "expected Plan, got: {err}"
        );
        assert!(
            other.exists(),
            "force plan failure must leave foreign conflict file"
        );
        assert_eq!(fs::read_to_string(&other).unwrap(), source);
        assert!(!dest.exists(), "plan failure must not create destination");
        assert!(
            !dest
                .parent()
                .expect("parent")
                .join(".countries.lemma.tmp")
                .exists(),
            "plan failure must not leave tmp"
        );
    }

    #[test]
    fn install_non_at_id_reaches_registry() {
        let dir = tempfile::tempdir().unwrap();
        let (registries, transport) = test_transport();
        let err = install_registry_dependency(
            dir.path(),
            "not-a-registry-id",
            false,
            &registries,
            &transport,
        )
        .expect_err("non-@ id must fail via registry");
        assert!(
            matches!(err, InstallError::Registry(_)),
            "error must be Registry, got: {err}"
        );
    }
}

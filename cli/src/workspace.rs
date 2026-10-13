//! Workspace disk policy: discover, load, and watch `.lemma` files.
//!
//! Discovery respects project `.gitignore` via the `ignore` crate, and always
//! includes `<workdir>/lemma_deps/**/*.lemma` even when that directory is gitignored.

use crate::deps::{dependency_identifier_from_dependency_path, lemma_deps_dir};
use lemma::Engine;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use walkdir::WalkDir;

/// Paths discovered under a workspace root, partitioned by provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredPaths {
    /// Workspace `.lemma` files (not under `lemma_deps/`).
    pub workspace_paths: Vec<PathBuf>,
    /// Dependency bundles under `lemma_deps/`.
    pub dependency_paths: Vec<PathBuf>,
}

impl DiscoveredPaths {
    /// Every discovered path (dependencies first, then workspace), for snapshots.
    pub fn all_paths(&self) -> impl Iterator<Item = &PathBuf> {
        self.dependency_paths
            .iter()
            .chain(self.workspace_paths.iter())
    }
}

/// Failure discovering or reading workspace sources from disk.
#[derive(Debug)]
pub enum WorkspaceDiskError {
    Io(std::io::Error),
    Ignore(ignore::Error),
    Walkdir(walkdir::Error),
    EngineLoad(lemma::Errors),
}

impl std::fmt::Display for WorkspaceDiskError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::Ignore(error) => write!(formatter, "{error}"),
            Self::Walkdir(error) => write!(formatter, "{error}"),
            Self::EngineLoad(errors) => {
                write!(
                    formatter,
                    "Workspace load failed ({} error(s))",
                    errors.errors.len()
                )
            }
        }
    }
}

impl std::error::Error for WorkspaceDiskError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Ignore(error) => Some(error),
            Self::Walkdir(error) => Some(error),
            Self::EngineLoad(_) => None,
        }
    }
}

impl From<std::io::Error> for WorkspaceDiskError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<ignore::Error> for WorkspaceDiskError {
    fn from(error: ignore::Error) -> Self {
        Self::Ignore(error)
    }
}

impl From<walkdir::Error> for WorkspaceDiskError {
    fn from(error: walkdir::Error) -> Self {
        Self::Walkdir(error)
    }
}

/// Discover `.lemma` paths under `workdir` (or a single `.lemma` file).
///
/// Directory roots: gitignore-aware walk excluding `lemma_deps/`, plus an
/// unconditional walk of `lemma_deps/` for dependency bundles.
pub fn discover_lemma_paths(workdir: &Path) -> Result<DiscoveredPaths, WorkspaceDiskError> {
    if workdir.is_file() {
        return Ok(DiscoveredPaths {
            workspace_paths: vec![workdir.to_path_buf()],
            dependency_paths: Vec::new(),
        });
    }

    let deps_dir = lemma_deps_dir(workdir);
    let mut workspace_paths = Vec::new();
    let mut dependency_paths = Vec::new();

    for entry in ignore::WalkBuilder::new(workdir)
        .hidden(true)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(false)
        .require_git(false)
        .build()
    {
        let entry = entry?;
        let path = entry.path();
        if path.starts_with(&deps_dir) {
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("lemma") {
            continue;
        }
        if path.is_file() {
            workspace_paths.push(path.to_path_buf());
        }
    }

    if deps_dir.is_dir() {
        for entry in WalkDir::new(&deps_dir) {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("lemma") {
                continue;
            }
            if path.is_file() {
                dependency_paths.push(path.to_path_buf());
            }
        }
    }

    workspace_paths.sort();
    dependency_paths.sort();

    Ok(DiscoveredPaths {
        workspace_paths,
        dependency_paths,
    })
}

/// Engine [`SourceType::Path`] label for a discovered workspace file.
///
/// Absolute disk paths are used only for I/O. Engine identity matches MCP/SDK
/// attributes: path relative to `workdir`, or the file name when `workdir` is
/// the file itself.
pub fn workspace_path_label(workdir: &Path, absolute_path: &Path) -> std::sync::Arc<PathBuf> {
    if workdir.is_file() {
        assert_eq!(
            absolute_path, workdir,
            "BUG: single-file workspace discover must yield the workdir path"
        );
        let name = workdir
            .file_name()
            .expect("BUG: workspace file path must have a file name");
        return std::sync::Arc::new(PathBuf::from(name));
    }
    let relative = absolute_path.strip_prefix(workdir).unwrap_or_else(|_| {
        panic!(
            "BUG: discovered path {} is not under workdir {}",
            absolute_path.display(),
            workdir.display()
        )
    });
    std::sync::Arc::new(relative.to_path_buf())
}

/// Load discovered sources into `engine` (same provenance rules as the CLI always used).
pub fn load_workspace(engine: &mut Engine, workdir: &Path) -> Result<(), WorkspaceDiskError> {
    load_workspace_excluding(engine, workdir, &[])
}

/// Like [`load_workspace`], but skips paths in `exclude` (compared as [`Path`]).
///
/// Used by install plan probe so `destination_absolute` and pending force
/// `conflict_paths` are not loaded; the candidate is loaded separately.
pub fn load_workspace_excluding(
    engine: &mut Engine,
    workdir: &Path,
    exclude: &[&Path],
) -> Result<(), WorkspaceDiskError> {
    let discovered = discover_lemma_paths(workdir)?;
    let mut sources = Vec::with_capacity(
        discovered
            .dependency_paths
            .len()
            .saturating_add(discovered.workspace_paths.len()),
    );

    for dep_path in &discovered.dependency_paths {
        if exclude.contains(&dep_path.as_path()) {
            continue;
        }
        let dependency_id = dependency_identifier_from_dependency_path(workdir, dep_path);
        let content = fs::read_to_string(dep_path)?;
        sources.push((
            lemma::SourceType::Dependency(dependency_id.to_string()),
            content,
        ));
    }
    for path in &discovered.workspace_paths {
        if exclude.contains(&path.as_path()) {
            continue;
        }
        let content = fs::read_to_string(path)?;
        sources.push((
            lemma::SourceType::Path(workspace_path_label(workdir, path)),
            content,
        ));
    }

    if sources.is_empty() {
        return Ok(());
    }

    match engine.load(sources) {
        Ok(()) => Ok(()),
        Err(errors) => Err(WorkspaceDiskError::EngineLoad(errors)),
    }
}

/// Refresh an existing engine from disk.
///
/// Discovers current `.lemma` paths and batch-[`Engine::load`]s them into a
/// scratch engine (order-independent), then swaps into `engine` only on success.
/// Failure leaves `engine` untouched. Callers that previously relied on
/// per-file `update` for scoped replanning still get a correct workspace; the
/// HTTP `--watch` path prioritizes last-good over incremental mutation.
pub fn refresh_workspace_via_update(
    engine: &mut Engine,
    workdir: &Path,
) -> Result<(), WorkspaceDiskError> {
    let discovered = discover_lemma_paths(workdir)?;
    let mut sources: Vec<(lemma::SourceType, String)> = Vec::new();

    for dep_path in &discovered.dependency_paths {
        let dependency_id = dependency_identifier_from_dependency_path(workdir, dep_path);
        let source_type = lemma::SourceType::Dependency(dependency_id.to_string());
        let content = fs::read_to_string(dep_path)?;
        sources.push((source_type, content));
    }
    for path in &discovered.workspace_paths {
        let source_type = lemma::SourceType::Path(workspace_path_label(workdir, path));
        let content = fs::read_to_string(path)?;
        sources.push((source_type, content));
    }

    let mut scratch = Engine::new();
    scratch
        .load(sources)
        .map_err(WorkspaceDiskError::EngineLoad)?;
    *engine = scratch;
    Ok(())
}

type ModifiedSnapshot = BTreeMap<PathBuf, SystemTime>;

fn collect_modified_times(workdir: &Path) -> Result<ModifiedSnapshot, WorkspaceDiskError> {
    let discovered = discover_lemma_paths(workdir)?;
    let mut snapshot = BTreeMap::new();
    for path in discovered.all_paths() {
        let metadata = fs::metadata(path)?;
        let modified = metadata.modified()?;
        snapshot.insert(path.clone(), modified);
    }
    Ok(snapshot)
}

/// Watch target: path plus whether the watch is recursive.
type WatchTarget = (PathBuf, bool);

fn push_ancestor_dirs(targets: &mut Vec<WatchTarget>, workdir: &Path, file_path: &Path) {
    let mut current = match file_path.parent() {
        Some(parent) => parent,
        None => return,
    };
    loop {
        targets.push((current.to_path_buf(), false));
        if current == workdir {
            break;
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent,
            _ => break,
        }
    }
}

fn desired_watch_targets(workdir: &Path) -> Result<Vec<WatchTarget>, WorkspaceDiskError> {
    let mut targets = Vec::new();

    // Non-recursive watch on the workspace root sees `lemma_deps/` appear without
    // descending into `target/` or other large trees.
    if workdir.is_dir() {
        targets.push((workdir.to_path_buf(), false));
    }

    let deps_dir = lemma_deps_dir(workdir);
    if deps_dir.is_dir() {
        targets.push((deps_dir.clone(), true));
    }

    let discovered = discover_lemma_paths(workdir)?;
    for path in &discovered.workspace_paths {
        push_ancestor_dirs(&mut targets, workdir, path);
    }

    // Dynamic expand: non-recursive watch on every non-ignored directory under
    // workdir (except lemma_deps, covered recursively). New child dirs become
    // visible on the next plant after the parent wakes.
    if workdir.is_dir() {
        for entry in ignore::WalkBuilder::new(workdir)
            .hidden(true)
            .git_ignore(true)
            .git_global(false)
            .git_exclude(false)
            .require_git(false)
            .build()
        {
            let entry = entry?;
            let path = entry.path();
            if path.starts_with(&deps_dir) {
                continue;
            }
            if path.is_dir() {
                targets.push((path.to_path_buf(), false));
            }
        }
    }

    targets.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    targets.dedup();
    Ok(targets)
}

fn plant_watches(
    watcher: &mut dyn notify::Watcher,
    workdir: &Path,
    currently_watched: &Mutex<std::collections::BTreeSet<WatchTarget>>,
) -> Result<(), WorkspaceDiskError> {
    let desired = desired_watch_targets(workdir)?;
    let desired_set: std::collections::BTreeSet<WatchTarget> = desired.into_iter().collect();

    let mut current = match currently_watched.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };

    for target in current.difference(&desired_set) {
        watcher
            .unwatch(&target.0)
            .map_err(|error| WorkspaceDiskError::Io(std::io::Error::other(error)))?;
    }
    for target in desired_set.difference(&current) {
        let mode = if target.1 {
            notify::RecursiveMode::Recursive
        } else {
            notify::RecursiveMode::NonRecursive
        };
        watcher
            .watch(&target.0, mode)
            .map_err(|error| WorkspaceDiskError::Io(std::io::Error::other(error)))?;
    }
    *current = desired_set;
    Ok(())
}

/// Keeps the filesystem watcher alive until dropped.
///
/// The notify callback holds a clone of the same `Arc` as this guard, and the
/// debouncer owns the callback. Drop takes the debouncer out of the mutex so
/// that cycle breaks and the notify thread stops.
pub struct WatchGuard {
    debouncer: Arc<Mutex<Option<notify_debouncer_mini::Debouncer<notify::RecommendedWatcher>>>>,
}

impl Drop for WatchGuard {
    fn drop(&mut self) {
        let mut slot = match self.debouncer.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *slot = None;
    }
}

/// Sends one watch event. A dropped receiver means the watch is being torn down.
fn deliver<T>(sink: &Sender<T>, event: T) {
    if sink.send(event).is_err() {
        // receiver gone: the owning guard is being dropped
    }
}

/// Watch discovered `.lemma` paths under `workdir` (not the whole tree recursively).
///
/// Plants non-recursive watches on `workdir`, on every non-ignored directory under
/// it, and on ancestors of discovered workspace `.lemma` files; plus `lemma_deps/`
/// recursively when that directory exists.
///
/// Every event is sent to `on_change` from the notify debouncer thread: `Ok(())` when
/// the discover snapshot changed; `Err` when discover/watch maintenance failed. The
/// receiver owns any async or blocking work.
pub fn watch_lemma_workspace(
    workdir: PathBuf,
    on_change: Sender<Result<(), WorkspaceDiskError>>,
) -> Result<WatchGuard, WorkspaceDiskError> {
    let initial_snapshot = collect_modified_times(&workdir)?;
    let last_snapshot: Arc<Mutex<ModifiedSnapshot>> = Arc::new(Mutex::new(initial_snapshot));
    let currently_watched: Arc<Mutex<std::collections::BTreeSet<WatchTarget>>> =
        Arc::new(Mutex::new(std::collections::BTreeSet::new()));
    let debouncer_slot: Arc<
        Mutex<Option<notify_debouncer_mini::Debouncer<notify::RecommendedWatcher>>>,
    > = Arc::new(Mutex::new(None));

    let workdir_for_callback = workdir.clone();
    let snapshot_for_callback = Arc::clone(&last_snapshot);
    let watched_for_callback = Arc::clone(&currently_watched);
    let slot_for_callback = Arc::clone(&debouncer_slot);
    let on_change_for_callback = on_change;

    let mut debouncer = notify_debouncer_mini::new_debouncer(
        Duration::from_millis(500),
        move |result: Result<Vec<notify_debouncer_mini::DebouncedEvent>, notify::Error>| {
            if let Err(error) = result {
                deliver(
                    &on_change_for_callback,
                    Err(WorkspaceDiskError::Io(std::io::Error::other(error))),
                );
                return;
            }

            let current_snapshot = match collect_modified_times(&workdir_for_callback) {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    deliver(&on_change_for_callback, Err(error));
                    return;
                }
            };

            let files_changed = {
                let previous = match snapshot_for_callback.lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => poisoned.into_inner(),
                };
                current_snapshot != *previous
            };
            if !files_changed {
                // Still refresh watches when `lemma_deps/` or nested dirs appear.
                let mut slot = match slot_for_callback.lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => poisoned.into_inner(),
                };
                if let Some(debouncer) = slot.as_mut() {
                    if let Err(error) = plant_watches(
                        debouncer.watcher(),
                        &workdir_for_callback,
                        &watched_for_callback,
                    ) {
                        deliver(&on_change_for_callback, Err(error));
                    }
                }
                return;
            }

            {
                let mut previous = match snapshot_for_callback.lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => poisoned.into_inner(),
                };
                *previous = current_snapshot;
            }

            deliver(&on_change_for_callback, Ok(()));

            let mut slot = match slot_for_callback.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            if let Some(debouncer) = slot.as_mut() {
                if let Err(error) = plant_watches(
                    debouncer.watcher(),
                    &workdir_for_callback,
                    &watched_for_callback,
                ) {
                    deliver(&on_change_for_callback, Err(error));
                }
            }
        },
    )
    .map_err(|error| WorkspaceDiskError::Io(std::io::Error::other(error)))?;

    plant_watches(debouncer.watcher(), &workdir, &currently_watched)?;

    {
        let mut slot = match debouncer_slot.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *slot = Some(debouncer);
    }

    Ok(WatchGuard {
        debouncer: debouncer_slot,
    })
}

/// Read all discovered `.lemma` files for LSP / tooling injection.
pub fn read_disk_lemma_files(
    workdir: &Path,
) -> Result<Vec<lemma_lsp::workspace_files::DiskLemmaFile>, WorkspaceDiskError> {
    let discovered = discover_lemma_paths(workdir)?;
    let mut files = Vec::with_capacity(
        discovered
            .dependency_paths
            .len()
            .saturating_add(discovered.workspace_paths.len()),
    );

    for path in &discovered.dependency_paths {
        let text = fs::read_to_string(path)?;
        files.push(lemma_lsp::workspace_files::DiskLemmaFile {
            path: path.clone(),
            text,
        });
    }
    for path in &discovered.workspace_paths {
        let text = fs::read_to_string(path)?;
        files.push(lemma_lsp::workspace_files::DiskLemmaFile {
            path: path.clone(),
            text,
        });
    }
    Ok(files)
}

/// CLI implementation of [`lemma_lsp::workspace_files::WorkspaceFiles`].
pub struct CliWorkspaceFiles;

impl lemma_lsp::workspace_files::WorkspaceFiles for CliWorkspaceFiles {
    fn load(
        &self,
        root: &Path,
    ) -> Result<
        Vec<lemma_lsp::workspace_files::DiskLemmaFile>,
        lemma_lsp::workspace_files::WorkspaceFilesError,
    > {
        read_disk_lemma_files(root).map_err(|error| {
            lemma_lsp::workspace_files::WorkspaceFilesError::new(error.to_string())
        })
    }

    fn watch(
        &self,
        root: PathBuf,
        on_change: Sender<
            Result<
                Vec<lemma_lsp::workspace_files::DiskLemmaFile>,
                lemma_lsp::workspace_files::WorkspaceFilesError,
            >,
        >,
    ) -> Result<
        lemma_lsp::workspace_files::WatchGuard,
        lemma_lsp::workspace_files::WorkspaceFilesError,
    > {
        let (watch_events, changes) = std::sync::mpsc::channel();
        let guard = watch_lemma_workspace(root.clone(), watch_events).map_err(|error| {
            lemma_lsp::workspace_files::WorkspaceFilesError::new(error.to_string())
        })?;
        std::thread::spawn(move || {
            for watch_result in changes {
                let event = match watch_result {
                    Ok(()) => read_disk_lemma_files(&root).map_err(|error| {
                        lemma_lsp::workspace_files::WorkspaceFilesError::new(error.to_string())
                    }),
                    Err(error) => Err(lemma_lsp::workspace_files::WorkspaceFilesError::new(
                        error.to_string(),
                    )),
                };
                deliver(&on_change, event);
            }
        });

        Ok(lemma_lsp::workspace_files::WatchGuard::from_keep_alive(
            Box::new(guard),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    fn flag_on_ok(
        fired: Arc<AtomicBool>,
    ) -> std::sync::mpsc::Sender<Result<(), WorkspaceDiskError>> {
        let (sink, events) = std::sync::mpsc::channel::<Result<(), WorkspaceDiskError>>();
        std::thread::spawn(move || {
            for result in events {
                if result.is_ok() {
                    fired.store(true, Ordering::SeqCst);
                }
            }
        });
        sink
    }

    fn write_file(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(path, contents).expect("write file");
    }

    fn wait_until_fired(fired: &AtomicBool, message: &str) {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            if fired.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("{message}");
    }

    #[test]
    fn discover_excludes_gitignored_target_includes_lemma_deps() {
        let root = tempfile::tempdir().expect("tempdir");
        write_file(&root.path().join(".gitignore"), "/target\nlemma_deps/\n");
        write_file(&root.path().join("src/app.lemma"), "spec app\ndata x: 1\n");
        write_file(
            &root.path().join("target/junk.lemma"),
            "spec junk\ndata x: 1\n",
        );
        write_file(
            &root.path().join("lemma_deps/@org/dep.lemma"),
            "spec dep\ndata x: 1\n",
        );

        let discovered = discover_lemma_paths(root.path()).expect("discover");
        assert_eq!(discovered.workspace_paths.len(), 1);
        assert!(discovered.workspace_paths[0].ends_with("src/app.lemma"));
        assert_eq!(discovered.dependency_paths.len(), 1);
        assert!(discovered.dependency_paths[0].ends_with("lemma_deps/@org/dep.lemma"));
    }

    #[test]
    fn load_workspace_stores_workdir_relative_path_labels() {
        let root = tempfile::tempdir().expect("tempdir");
        write_file(
            &root.path().join("src/app.lemma"),
            "spec app\ndata x: 1\nrule r: x\n",
        );

        let mut engine = Engine::new();
        load_workspace(&mut engine, root.path()).expect("load");
        let shown = engine
            .show(None, "app", None)
            .expect("app must show after load");
        assert_eq!(
            shown.source_type,
            Some(lemma::SourceType::Path(std::sync::Arc::new(PathBuf::from(
                "src/app.lemma"
            )))),
            "Engine Path identity must be workdir-relative, not absolute"
        );
    }

    #[test]
    fn refresh_resolves_consumer_that_sorts_before_provider() {
        let root = tempfile::tempdir().expect("tempdir");
        write_file(
            &root.path().join("z_provider.lemma"),
            "spec person\ndata name: \"Alice\"\n",
        );
        write_file(
            &root.path().join("a_consumer.lemma"),
            "spec company\nuses employee: person\n  -> with name: \"Bob\"\n",
        );

        let mut engine = Engine::new();
        refresh_workspace_via_update(&mut engine, root.path()).expect("two-pass refresh");
        assert!(engine.show(None, "person", None).is_ok());
        assert!(engine.show(None, "company", None).is_ok());
    }

    #[test]
    fn watch_fires_when_lemma_deps_file_appears() {
        let root = tempfile::tempdir().expect("tempdir");
        write_file(&root.path().join(".gitignore"), "lemma_deps/\n");
        write_file(&root.path().join("src/app.lemma"), "spec app\ndata x: 1\n");
        fs::create_dir_all(root.path().join("lemma_deps")).expect("create lemma_deps");

        let fired = Arc::new(AtomicBool::new(false));
        let fired_flag = Arc::clone(&fired);
        let guard = watch_lemma_workspace(
            root.path().to_path_buf(),
            flag_on_ok(Arc::clone(&fired_flag)),
        )
        .expect("start watch");

        write_file(
            &root.path().join("lemma_deps/@org/dep.lemma"),
            "spec dep\ndata x: 1\n",
        );

        wait_until_fired(&fired, "watch did not fire after creating lemma_deps file");
        drop(guard);
    }

    #[test]
    fn watch_does_not_fire_for_gitignored_target_lemma() {
        let root = tempfile::tempdir().expect("tempdir");
        write_file(&root.path().join(".gitignore"), "/target\n");
        write_file(&root.path().join("src/app.lemma"), "spec app\ndata x: 1\n");
        fs::create_dir_all(root.path().join("target")).expect("create target");

        let fired = Arc::new(AtomicBool::new(false));
        let fired_flag = Arc::clone(&fired);
        let guard = watch_lemma_workspace(
            root.path().to_path_buf(),
            flag_on_ok(Arc::clone(&fired_flag)),
        )
        .expect("start watch");

        write_file(
            &root.path().join("target/junk.lemma"),
            "spec junk\ndata x: 1\n",
        );

        std::thread::sleep(Duration::from_millis(800));
        assert!(
            !fired.load(Ordering::SeqCst),
            "gitignored target/*.lemma must not change discover snapshot"
        );
        drop(guard);
    }

    #[test]
    fn watch_fires_when_discovered_workspace_file_changes() {
        let root = tempfile::tempdir().expect("tempdir");
        let app = root.path().join("src/app.lemma");
        write_file(&app, "spec app\ndata x: 1\n");

        let fired = Arc::new(AtomicBool::new(false));
        let fired_flag = Arc::clone(&fired);
        let guard = watch_lemma_workspace(
            root.path().to_path_buf(),
            flag_on_ok(Arc::clone(&fired_flag)),
        )
        .expect("start watch");

        write_file(&app, "spec app\ndata x: 2\n");

        wait_until_fired(
            &fired,
            "watch did not fire after modifying discovered workspace file",
        );
        drop(guard);
    }

    #[test]
    fn watch_fires_when_sibling_workspace_file_appears() {
        let root = tempfile::tempdir().expect("tempdir");
        write_file(&root.path().join("src/app.lemma"), "spec app\ndata x: 1\n");

        let fired = Arc::new(AtomicBool::new(false));
        let fired_flag = Arc::clone(&fired);
        let guard = watch_lemma_workspace(
            root.path().to_path_buf(),
            flag_on_ok(Arc::clone(&fired_flag)),
        )
        .expect("start watch");

        write_file(
            &root.path().join("src/other.lemma"),
            "spec other\ndata y: 2\n",
        );

        wait_until_fired(
            &fired,
            "watch did not fire after creating sibling workspace file",
        );
        drop(guard);
    }

    #[test]
    fn watch_fires_when_nested_workspace_file_appears() {
        let root = tempfile::tempdir().expect("tempdir");
        write_file(&root.path().join("src/app.lemma"), "spec app\ndata x: 1\n");

        let fired = Arc::new(AtomicBool::new(false));
        let fired_flag = Arc::clone(&fired);
        let guard = watch_lemma_workspace(
            root.path().to_path_buf(),
            flag_on_ok(Arc::clone(&fired_flag)),
        )
        .expect("start watch");

        fs::create_dir_all(root.path().join("src/nested")).expect("create nested");
        std::thread::sleep(Duration::from_millis(600));
        write_file(
            &root.path().join("src/nested/foo.lemma"),
            "spec foo\ndata z: 3\n",
        );

        wait_until_fired(
            &fired,
            "watch did not fire after creating nested workspace file",
        );
        drop(guard);
    }

    #[test]
    fn watch_plants_lemma_deps_after_directory_is_created() {
        let root = tempfile::tempdir().expect("tempdir");
        write_file(&root.path().join(".gitignore"), "lemma_deps/\n");
        write_file(&root.path().join("src/app.lemma"), "spec app\ndata x: 1\n");

        let fired = Arc::new(AtomicBool::new(false));
        let fired_flag = Arc::clone(&fired);
        let guard = watch_lemma_workspace(
            root.path().to_path_buf(),
            flag_on_ok(Arc::clone(&fired_flag)),
        )
        .expect("start watch");

        fs::create_dir_all(root.path().join("lemma_deps")).expect("create lemma_deps");
        std::thread::sleep(Duration::from_millis(600));
        write_file(
            &root.path().join("lemma_deps/@org/dep.lemma"),
            "spec dep\ndata x: 1\n",
        );

        wait_until_fired(
            &fired,
            "watch did not fire after creating lemma_deps then a dep file",
        );
        drop(guard);
    }

    #[test]
    fn watch_guard_drop_stops_further_callbacks() {
        let root = tempfile::tempdir().expect("tempdir");
        let app = root.path().join("src/app.lemma");
        write_file(&app, "spec app\ndata x: 1\n");

        let fired = Arc::new(AtomicBool::new(false));
        let fired_flag = Arc::clone(&fired);
        let guard = watch_lemma_workspace(
            root.path().to_path_buf(),
            flag_on_ok(Arc::clone(&fired_flag)),
        )
        .expect("start watch");

        drop(guard);

        write_file(&app, "spec app\ndata x: 2\n");
        std::thread::sleep(Duration::from_millis(800));
        assert!(
            !fired.load(Ordering::SeqCst),
            "dropped WatchGuard must stop notify callbacks"
        );
    }
}

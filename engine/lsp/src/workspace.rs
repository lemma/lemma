use lemma::{parse, Engine, Error, ParseResult, ResourceLimits, SourceType};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use tower_lsp_server::ls_types::Uri;

/// Web virtual dependency docs use `file:///lemma/repo/<hex-utf8>.lemma` (`hex` lowercase per client).
/// Attributed like [`lemma::SourceType::Dependency`] when loading dependency bundles.
const VIRTUAL_LEMMA_REPO_PREFIX: &str = "/lemma/repo/";

/// Filesystem path of a `file:` URI. `None` for every other scheme, so `untitled:` and similar
/// URIs are never mistaken for paths (`Uri::to_file_path` does not check the scheme).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn file_path_of(uri: &Uri) -> Option<std::borrow::Cow<'_, std::path::Path>> {
    if uri.scheme().as_str().eq_ignore_ascii_case("file") {
        uri.to_file_path()
    } else {
        None
    }
}

/// Result of parsing a single file's content.
enum ParseOutcome {
    /// Parsing succeeded; repositories group specs as in source order.
    Success(ParseResult),
    /// Parsing failed with errors.
    Failed(Vec<Error>),
}

/// A single file tracked by the workspace.
struct TrackedFile {
    /// The latest URL for this file (used for publishing diagnostics).
    url: Uri,
    /// The latest text content of the file (from the editor buffer or disk).
    text: String,
    /// The parsed outcome: either successfully parsed specs or parse errors.
    parse_outcome: ParseOutcome,
}

/// Per-file diagnostic result after a full workspace validation pass.
pub struct FileDiagnostics {
    /// The URL of the file.
    pub url: Uri,
    /// The latest text content (for byte-offset to LSP Range conversion).
    pub text: String,
    /// The source attribute used during parsing (maps to Error source locations).
    pub attribute: String,
    /// All errors for this file (parse errors + planning errors).
    pub errors: Vec<Error>,
}

/// In-memory workspace model.
///
/// Tracks all `.lemma` files in the workspace, their parsed ASTs,
/// and supports re-parsing and re-planning when files change.
///
/// Keyed by **attribute** (file path string or URL string) so that the same
/// physical file is tracked exactly once, regardless of how the URL is constructed.
pub struct WorkspaceModel {
    /// Workspace root directory (host); `None` on WASM or single-file mode.
    workspace_root: Option<std::path::PathBuf>,
    /// Map from source attribute to tracked file state.
    files: HashMap<String, TrackedFile>,
    /// Attributes last supplied by disk injection (CLI discover/watch).
    disk_attributes: HashSet<String>,
    /// Last disk text per attribute (for restoring after editor close).
    disk_texts: HashMap<String, String>,
    /// Resource limits used during parsing.
    limits: ResourceLimits,
    /// Long-lived engine; validated via per-file [`Engine::update`] or batch load.
    engine: Engine,
    /// Attributes whose on-disk/buffer text has not yet been synced into `engine`.
    dirty_attributes: HashSet<String>,
    /// Source types whose files left the model and still need an empty
    /// [`Engine::update`] prune on the next [`Self::validate_workspace`].
    pending_removals: Vec<(String, SourceType)>,
}

impl Default for WorkspaceModel {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkspaceModel {
    pub fn new() -> Self {
        Self {
            workspace_root: None,
            files: HashMap::new(),
            disk_attributes: HashSet::new(),
            disk_texts: HashMap::new(),
            limits: ResourceLimits::default(),
            engine: Engine::new(),
            dirty_attributes: HashSet::new(),
            pending_removals: Vec::new(),
        }
    }

    /// Set the workspace root used to locate `lemma_deps/` and attribute disk paths.
    pub fn set_workspace_root(&mut self, root: std::path::PathBuf) {
        self.workspace_root = Some(root);
    }

    /// Root directory when the host workspace folder is known.
    #[must_use]
    pub fn workspace_root(&self) -> Option<&std::path::PathBuf> {
        self.workspace_root.as_ref()
    }

    /// Decode hex UTF-8 path segment emitted by Lemma Base virtual bundle URIs.
    fn dependency_id_from_hex_segment(seg: &str) -> Option<String> {
        if seg.is_empty() || seg == "_" {
            return None;
        }
        if !seg.len().is_multiple_of(2) {
            return None;
        }
        let mut bytes = Vec::with_capacity(seg.len() / 2);
        for chunk in seg.as_bytes().chunks_exact(2) {
            let h = std::str::from_utf8(chunk).ok()?;
            let b = u8::from_str_radix(h, 16).ok()?;
            bytes.push(b);
        }
        let s = String::from_utf8(bytes).ok()?;
        (!s.trim().is_empty()).then_some(s)
    }

    /// Canonical dependency id extracted from `/lemma/repo/<hex>.lemma` (RFC `file:` path).
    fn virtual_bundle_dependency_id(url: &Uri) -> Option<String> {
        let path = url.path().as_str();
        let rest = path.strip_prefix(VIRTUAL_LEMMA_REPO_PREFIX)?;
        let seg = rest.strip_suffix(".lemma")?;
        Self::dependency_id_from_hex_segment(seg)
    }

    /// Provenance for [`lemma::Engine::load`]: workspace paths use [`SourceType::Path`];
    /// `lemma_deps/` and virtual bundle URIs use [`SourceType::Dependency`].
    fn source_type_for_url(&self, url: &Uri) -> SourceType {
        if let Some(bundle_id) = Self::virtual_bundle_dependency_id(url) {
            return SourceType::Dependency(bundle_id);
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let (Some(root), Some(path)) = (self.workspace_root.as_deref(), file_path_of(url)) {
            let deps_dir = lemma::deps::lemma_deps_dir(root);
            if path.starts_with(&deps_dir) {
                let dep_id = lemma::deps::dependency_identifier_from_dependency_path(root, &path);
                return SourceType::Dependency(dep_id);
            }
        }
        SourceType::Path(Arc::new(PathBuf::from(Self::attribute_for_url(url))))
    }

    /// Derive a stable source attribute from a URL (path or URL string).
    fn attribute_for_url(url: &Uri) -> String {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(path) = file_path_of(url) {
            return path.to_string_lossy().to_string();
        }
        url.as_str().to_owned()
    }

    /// Add or update a file in the workspace. Parses immediately.
    /// If a different URL maps to the same attribute (path), the old entry is replaced.
    pub fn update_file(&mut self, url: Uri, text: String) {
        let attribute = Self::attribute_for_url(&url);
        let parse_outcome = match parse(
            &text,
            SourceType::Path(Arc::new(PathBuf::from(&attribute))),
            &self.limits,
        ) {
            Ok(result) => ParseOutcome::Success(result),
            Err(error) => ParseOutcome::Failed(vec![error]),
        };
        self.files.insert(
            attribute.clone(),
            TrackedFile {
                url,
                text,
                parse_outcome,
            },
        );
        self.dirty_attributes.insert(attribute);
    }

    /// Remove a file from the workspace; engine prune runs on the next validate.
    pub fn remove_file(&mut self, url: &Uri) {
        let attribute = Self::attribute_for_url(url);
        let source_type = self.source_type_for_url(url);
        self.files.remove(&attribute);
        self.disk_attributes.remove(&attribute);
        self.disk_texts.remove(&attribute);
        self.dirty_attributes.remove(&attribute);
        self.queue_removal(attribute, source_type);
    }

    fn queue_removal(&mut self, attribute: String, source_type: SourceType) {
        match &source_type {
            SourceType::Path(_) | SourceType::Dependency(_) => {
                self.pending_removals.retain(|(a, _)| a != &attribute);
                self.pending_removals.push((attribute, source_type));
            }
            SourceType::Volatile => {
                unreachable!("BUG: LSP never attributes Volatile sources");
            }
        }
    }

    fn apply_pending_removals(
        &mut self,
        load_errors_by_attribute: &mut HashMap<String, Vec<Error>>,
    ) -> bool {
        let removals = std::mem::take(&mut self.pending_removals);
        if removals.is_empty() {
            return true;
        }
        let mut all_ok = true;
        for (attribute, source_type) in removals {
            match self.engine.update(None, String::new(), source_type.clone()) {
                Ok(()) => {}
                Err(errs) => {
                    all_ok = false;
                    for err in errs.errors {
                        let attr = err
                            .location()
                            .map(|s| s.source_type.to_string())
                            .unwrap_or_else(|| attribute.clone());
                        load_errors_by_attribute.entry(attr).or_default().push(err);
                    }
                    // Keep trying until the prune succeeds (e.g. consumer fixed).
                    self.pending_removals.push((attribute.clone(), source_type));
                    self.dirty_attributes.insert(attribute);
                }
            }
        }
        all_ok
    }

    /// Whether this URL was present in the last successful disk injection.
    #[must_use]
    pub fn is_disk_backed(&self, url: &Uri) -> bool {
        let attribute = Self::attribute_for_url(url);
        self.disk_attributes.contains(&attribute)
    }

    /// Record the current buffer text as the disk baseline for close/restore.
    ///
    /// Called on `textDocument/didSave` so close restores the saved content even when
    /// the filesystem watch does not cover that path.
    pub fn record_saved_baseline(&mut self, url: &Uri) {
        let attribute = Self::attribute_for_url(url);
        let Some(tracked) = self.files.get(&attribute) else {
            panic!(
                "BUG: didSave for untracked URI {}; open/change must precede save",
                url.as_str()
            );
        };
        let text = tracked.text.clone();
        self.disk_texts.insert(attribute.clone(), text);
        self.disk_attributes.insert(attribute);
    }

    /// Restore tracked text from the last disk injection for this URL.
    ///
    /// Returns false when the URL is not disk-backed.
    pub fn restore_disk_text(&mut self, url: &Uri) -> bool {
        let attribute = Self::attribute_for_url(url);
        let Some(text) = self.disk_texts.get(&attribute).cloned() else {
            return false;
        };
        self.update_file(url.clone(), text);
        true
    }

    /// Replace disk-backed membership from a full disk payload.
    ///
    /// Updates disk metadata for every file in `files`. Applies `update_file` only
    /// for attributes not listed in `open_attributes` (open buffers win). Removes
    /// previously disk-backed entries that are absent from the payload and not open.
    pub fn apply_disk_files(&mut self, files: &[(Uri, String)], open_attributes: &HashSet<String>) {
        let mut next_disk_attributes = HashSet::new();
        let mut next_disk_texts = HashMap::new();

        for (url, text) in files {
            let attribute = Self::attribute_for_url(url);
            next_disk_attributes.insert(attribute.clone());
            next_disk_texts.insert(attribute.clone(), text.clone());
            if !open_attributes.contains(&attribute) {
                self.update_file(url.clone(), text.clone());
            }
        }

        let removed: Vec<String> = self
            .disk_attributes
            .difference(&next_disk_attributes)
            .cloned()
            .collect();
        for attribute in removed {
            if open_attributes.contains(&attribute) {
                continue;
            }
            if let Some(tracked) = self.files.remove(&attribute) {
                let source_type = self.source_type_for_url(&tracked.url);
                self.queue_removal(attribute.clone(), source_type);
            }
            self.dirty_attributes.remove(&attribute);
        }

        self.disk_attributes = next_disk_attributes;
        self.disk_texts = next_disk_texts;
    }

    /// Stable attribute string for a URL (path or URL string).
    #[must_use]
    pub fn attribute_for_url_public(url: &Uri) -> String {
        Self::attribute_for_url(url)
    }

    /// Successful parse tree for a tracked file, if any.
    pub fn parse_success_for_url(&self, url: &Uri) -> Option<&ParseResult> {
        let attribute = Self::attribute_for_url(url);
        self.files
            .get(&attribute)
            .and_then(|t| match &t.parse_outcome {
                ParseOutcome::Success(pr) => Some(pr),
                ParseOutcome::Failed(_) => None,
            })
    }

    /// Long-lived engine for hover, links, and evaluation.
    #[must_use]
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Run workspace validation: parse errors + planning errors.
    ///
    /// Single dirty attribute → per-file [`Engine::update`]. Multiple dirty
    /// attributes (or first population) → batch [`Engine::load`] into a fresh
    /// engine (order-independent), swapped only on success. Pending disk/close
    /// removals apply as empty Path/Dependency updates in the same pass.
    pub fn validate_workspace(&mut self) -> Vec<FileDiagnostics> {
        let dirty: HashSet<String> = std::mem::take(&mut self.dirty_attributes);
        let mut load_errors_by_attribute: HashMap<String, Vec<Error>> = HashMap::new();

        let single_dirty = if dirty.len() == 1 {
            dirty.iter().next().cloned()
        } else {
            None
        };
        let incremental = match &single_dirty {
            Some(attr) => self.files.contains_key(attr) && self.pending_removals.is_empty(),
            None => false,
        };

        if incremental {
            let attribute = single_dirty.expect("BUG: incremental requires single dirty attribute");
            let (source_type, text_or_failed) = {
                let tracked = self
                    .files
                    .get(&attribute)
                    .expect("BUG: dirty attribute must exist in files");
                let source_type = self.source_type_for_url(&tracked.url);
                match &tracked.parse_outcome {
                    ParseOutcome::Failed(_) => (source_type, None),
                    ParseOutcome::Success(_) => (source_type, Some(tracked.text.clone())),
                }
            };
            match text_or_failed {
                None => match self.engine.update(None, String::new(), source_type) {
                    Ok(()) => {}
                    Err(errs) => {
                        for err in errs.errors {
                            let attr = err
                                .location()
                                .map(|s| s.source_type.to_string())
                                .unwrap_or_else(|| attribute.clone());
                            load_errors_by_attribute.entry(attr).or_default().push(err);
                        }
                        self.dirty_attributes.insert(attribute);
                    }
                },
                Some(text) => {
                    if let Err(errs) = self.engine.update(None, text, source_type) {
                        for err in errs.errors {
                            let attr = err
                                .location()
                                .map(|s| s.source_type.to_string())
                                .unwrap_or_else(|| attribute.clone());
                            load_errors_by_attribute.entry(attr).or_default().push(err);
                        }
                        self.dirty_attributes.insert(attribute);
                    }
                }
            }
        } else if !dirty.is_empty() || !self.pending_removals.is_empty() {
            if !dirty.is_empty() {
                self.rebuild_engine_from_files(&mut load_errors_by_attribute);
                // Removals are reflected by the scratch rebuild (absent files).
                self.pending_removals.clear();
                if !load_errors_by_attribute.is_empty() {
                    // Keep dirty so the next pass retries until the workspace loads cleanly.
                    self.dirty_attributes = dirty;
                }
            } else if !self.apply_pending_removals(&mut load_errors_by_attribute) {
                // pending_removals + dirty already requeued inside apply_pending_removals
            }
        }

        let mut results = Vec::new();
        let mut attributes: Vec<_> = self.files.keys().cloned().collect();
        attributes.sort();
        for attribute in attributes {
            let tracked = self
                .files
                .get(&attribute)
                .expect("BUG: attribute from keys must exist");
            let mut file_errors = Vec::new();
            if let ParseOutcome::Failed(parse_errors) = &tracked.parse_outcome {
                file_errors.extend(parse_errors.iter().cloned());
            }
            if let Some(load_errors) = load_errors_by_attribute.get(&attribute) {
                file_errors.extend(load_errors.iter().cloned());
            }
            results.push(FileDiagnostics {
                url: tracked.url.clone(),
                text: tracked.text.clone(),
                attribute: attribute.clone(),
                errors: file_errors,
            });
        }
        results
    }

    fn rebuild_engine_from_files(
        &mut self,
        load_errors_by_attribute: &mut HashMap<String, Vec<Error>>,
    ) {
        let mut attributes: Vec<_> = self.files.keys().cloned().collect();
        attributes.sort();
        let mut sources: Vec<(SourceType, String)> = Vec::new();
        for attribute in attributes {
            let tracked = self
                .files
                .get(&attribute)
                .expect("BUG: attribute from keys must exist");
            if matches!(tracked.parse_outcome, ParseOutcome::Failed(_)) {
                continue;
            }
            let source_type = self.source_type_for_url(&tracked.url);
            sources.push((source_type, tracked.text.clone()));
        }

        let mut scratch = Engine::new();
        match scratch.load(sources) {
            Ok(()) => {
                self.engine = scratch;
            }
            Err(errs) => {
                for err in errs.errors {
                    let attr = err
                        .location()
                        .map(|s| s.source_type.to_string())
                        .unwrap_or_else(|| "unknown".to_string());
                    load_errors_by_attribute.entry(attr).or_default().push(err);
                }
            }
        }
    }

    /// Get the current text content for a file, if tracked.
    pub fn get_file_text(&self, url: &Uri) -> Option<&str> {
        let attribute = Self::attribute_for_url(url);
        self.files
            .get(&attribute)
            .map(|tracked| tracked.text.as_str())
    }

    /// Get the current text content and its source attribute for a file, if tracked.
    pub fn get_file_text_and_attribute(&self, url: &Uri) -> Option<(&str, &str)> {
        let attribute = Self::attribute_for_url(url);
        self.files
            .get_key_value(&attribute)
            .map(|(key, tracked)| (tracked.text.as_str(), key.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl WorkspaceModel {
        fn contains_file(&self, url: &Uri) -> bool {
            let attribute = Self::attribute_for_url(url);
            self.files.contains_key(&attribute)
        }
    }

    #[test]
    fn non_file_uri_attribute_is_uri_string() {
        let url: Uri = "untitled:Untitled-1".parse().expect("untitled uri");
        assert_eq!(
            WorkspaceModel::attribute_for_url_public(&url),
            "untitled:Untitled-1"
        );
    }

    fn url_from_path(path: &str) -> Uri {
        Uri::from_file_path(path).expect("valid file path for test URL")
    }

    #[test]
    fn update_file_and_validate_single_valid_spec() {
        let mut workspace = WorkspaceModel::new();
        let url = url_from_path("/tmp/test.lemma");
        workspace.update_file(
            url.clone(),
            "spec test\ndata x: 10\nrule y: x + 1".to_string(),
        );

        let results = workspace.validate_workspace();
        assert_eq!(results.len(), 1);
        assert!(
            results[0].errors.is_empty(),
            "Expected no errors, got: {:?}",
            results[0].errors
        );
    }

    #[test]
    fn update_file_with_parse_error_produces_diagnostics() {
        let mut workspace = WorkspaceModel::new();
        let url = url_from_path("/tmp/broken.lemma");
        workspace.update_file(url.clone(), "this is not valid lemma syntax".to_string());

        let results = workspace.validate_workspace();
        assert_eq!(results.len(), 1);
        assert!(
            !results[0].errors.is_empty(),
            "Expected parse errors for invalid input"
        );
    }

    #[test]
    fn cross_spec_reference_resolves_when_consumer_sorts_before_provider() {
        let mut workspace = WorkspaceModel::new();
        let url_provider = url_from_path("/tmp/z_provider.lemma");
        let url_consumer = url_from_path("/tmp/a_consumer.lemma");

        workspace.update_file(
            url_consumer.clone(),
            "spec company\nuses employee: person\n  -> with name: \"Bob\"".to_string(),
        );
        workspace.update_file(
            url_provider.clone(),
            "spec person\ndata name: \"Alice\"\ndata age: 30".to_string(),
        );

        let results = workspace.validate_workspace();
        for result in &results {
            assert!(
                result.errors.is_empty(),
                "batch load must be order-independent; file {} got: {:?}",
                result.url.as_str(),
                result.errors
            );
        }
    }

    #[test]
    fn disk_rename_does_not_leave_duplicate_spec_diagnostic() {
        let mut workspace = WorkspaceModel::new();
        let url_foo = url_from_path("/tmp/foo.lemma");
        let url_bar = url_from_path("/tmp/bar.lemma");
        let open = HashSet::new();

        workspace.apply_disk_files(
            &[(
                url_foo.clone(),
                "spec item\ndata v: 1\nrule r: v\n".to_string(),
            )],
            &open,
        );
        let results = workspace.validate_workspace();
        assert!(results.iter().all(|r| r.errors.is_empty()));
        assert!(workspace.engine().show(None, "item", None).is_ok());

        workspace.apply_disk_files(
            &[(
                url_bar.clone(),
                "spec item\ndata v: 2\nrule r: v\n".to_string(),
            )],
            &open,
        );
        let results = workspace.validate_workspace();
        for result in &results {
            assert!(
                result.errors.is_empty(),
                "rename must prune old path before loading new; {} got: {:?}",
                result.url.as_str(),
                result.errors
            );
        }
        assert!(
            !workspace.contains_file(&url_foo),
            "old path must leave the model"
        );
        assert!(workspace.engine().show(None, "item", None).is_ok());
    }

    #[test]
    fn cross_spec_reference_resolves_when_both_files_present() {
        let mut workspace = WorkspaceModel::new();
        let url_a = url_from_path("/tmp/a.lemma");
        let url_b = url_from_path("/tmp/b.lemma");

        workspace.update_file(
            url_a.clone(),
            "spec person\ndata name: \"Alice\"\ndata age: 30".to_string(),
        );
        workspace.update_file(
            url_b.clone(),
            "spec company\nuses employee: person\n  -> with name: \"Bob\"".to_string(),
        );

        let results = workspace.validate_workspace();
        for result in &results {
            assert!(
                result.errors.is_empty(),
                "Expected no errors for file {}, got: {:?}",
                result.url.as_str(),
                result.errors
            );
        }
    }

    #[test]
    fn missing_cross_spec_reference_produces_planning_error() {
        let mut workspace = WorkspaceModel::new();
        let url = url_from_path("/tmp/orphan.lemma");
        workspace.update_file(
            url.clone(),
            "spec orphan\nuses other: nonexistent".to_string(),
        );

        let results = workspace.validate_workspace();
        assert_eq!(results.len(), 1);
        assert!(
            !results[0].errors.is_empty(),
            "Expected planning error for missing spec reference"
        );
    }

    #[test]
    fn remove_file_clears_it_from_workspace() {
        let mut workspace = WorkspaceModel::new();
        let url = url_from_path("/tmp/remove_me.lemma");
        workspace.update_file(url.clone(), "spec test\ndata x: 10".to_string());
        assert!(workspace.contains_file(&url));

        workspace.remove_file(&url);
        assert!(!workspace.contains_file(&url));

        let results = workspace.validate_workspace();
        assert!(results.is_empty());
    }

    #[test]
    fn apply_disk_files_tracks_membership_and_skips_open_overlays() {
        let mut workspace = WorkspaceModel::new();
        let url_a = url_from_path("/tmp/disk_a.lemma");
        let url_b = url_from_path("/tmp/disk_b.lemma");
        let open =
            std::collections::HashSet::from([WorkspaceModel::attribute_for_url_public(&url_a)]);

        workspace.update_file(url_a.clone(), "spec a\ndata x: 1".to_string());
        workspace.apply_disk_files(
            &[
                (url_a.clone(), "spec a\ndata x: 99".to_string()),
                (url_b.clone(), "spec b\ndata y: 2".to_string()),
            ],
            &open,
        );

        assert!(workspace.is_disk_backed(&url_a));
        assert!(workspace.is_disk_backed(&url_b));
        assert_eq!(
            workspace.get_file_text(&url_a),
            Some("spec a\ndata x: 1"),
            "open buffer must win over disk payload"
        );
        assert_eq!(workspace.get_file_text(&url_b), Some("spec b\ndata y: 2"));

        workspace.apply_disk_files(&[(url_a.clone(), "spec a\ndata x: 3".to_string())], &open);
        assert!(workspace.is_disk_backed(&url_a));
        assert!(!workspace.is_disk_backed(&url_b));
        assert!(
            !workspace.contains_file(&url_b),
            "removed disk file absent from open set must leave the model"
        );
    }

    #[test]
    fn restore_disk_text_after_overlay() {
        let mut workspace = WorkspaceModel::new();
        let url = url_from_path("/tmp/disk_restore.lemma");
        let open = std::collections::HashSet::new();
        workspace.apply_disk_files(&[(url.clone(), "spec a\ndata x: 1".to_string())], &open);
        workspace.update_file(url.clone(), "spec a\ndata x: 2".to_string());
        assert!(workspace.restore_disk_text(&url));
        assert_eq!(workspace.get_file_text(&url), Some("spec a\ndata x: 1"));
    }

    #[test]
    fn record_saved_baseline_updates_restore_text() {
        let mut workspace = WorkspaceModel::new();
        let url = url_from_path("/tmp/disk_save.lemma");
        workspace.update_file(url.clone(), "spec a\ndata x: 1".to_string());
        workspace.update_file(url.clone(), "spec a\ndata x: 2".to_string());
        workspace.record_saved_baseline(&url);
        workspace.update_file(url.clone(), "spec a\ndata x: 3".to_string());
        assert!(workspace.restore_disk_text(&url));
        assert_eq!(workspace.get_file_text(&url), Some("spec a\ndata x: 2"));
    }

    #[test]
    fn same_file_different_urls_produces_single_entry() {
        let mut workspace = WorkspaceModel::new();
        let url1 = url_from_path("/tmp/test.lemma");
        let url2 = url_from_path("/tmp/test.lemma");
        workspace.update_file(url1, "spec test\ndata x: 10".to_string());
        workspace.update_file(url2, "spec test\ndata x: 20".to_string());

        let results = workspace.validate_workspace();
        assert_eq!(
            results.len(),
            1,
            "Same file should produce exactly one entry"
        );
    }

    #[test]
    fn planning_error_stays_on_owning_file_only() {
        let mut workspace = WorkspaceModel::new();
        let url_bad = url_from_path("/tmp/lsp_bad_import.lemma");
        let url_ok = url_from_path("/tmp/lsp_clean.lemma");
        workspace.update_file(
            url_bad.clone(),
            "spec consumer\nuses dep: no_such_dep\n  -> with money: 10\ndata x: 1".to_string(),
        );
        workspace.update_file(url_ok.clone(), "spec other\ndata y: 2".to_string());

        let results = workspace.validate_workspace();
        let diag_bad = results
            .iter()
            .find(|d| d.url == url_bad)
            .expect("bad file diagnostics");
        let diag_ok = results
            .iter()
            .find(|d| d.url == url_ok)
            .expect("ok file diagnostics");
        assert!(
            !diag_bad.errors.is_empty(),
            "bad file should have errors: {:?}",
            diag_bad.errors
        );
        assert!(
            diag_ok.errors.is_empty(),
            "clean file should have no errors, got {:?}",
            diag_ok.errors
        );
    }

    #[test]
    fn deps_lemma_files_use_registry_identity_like_dependency_load() {
        let root = std::env::temp_dir().join("lemma_lsp_deps_workspace_test");
        if root.exists() {
            std::fs::remove_dir_all(&root).expect("BUG: remove test workspace");
        }
        let dep_path = lemma::deps::dependency_cache_file(&root, "@iso/countries");
        std::fs::create_dir_all(dep_path.parent().expect("dep parent")).expect("create dep dir");
        std::fs::write(
            &dep_path,
            "spec alpha2 2024\ndata code: text\n -> option \"NL\"\n",
        )
        .expect("write dep");
        let main_path = root.join("main.lemma");
        std::fs::write(
            &main_path,
            "spec demo\nuses iso: @iso/countries alpha2 2026\n  -> with code: \"NL\"\n",
        )
        .expect("write main");

        let mut workspace = WorkspaceModel::new();
        workspace.set_workspace_root(root.clone());
        let url_main = Uri::from_file_path(&main_path).expect("main url");
        let url_dep = Uri::from_file_path(&dep_path).expect("dep url");
        workspace.update_file(
            url_main,
            std::fs::read_to_string(&main_path).expect("read main"),
        );
        workspace.update_file(
            url_dep,
            std::fs::read_to_string(&dep_path).expect("read dep"),
        );

        let results = workspace.validate_workspace();
        for diag in &results {
            for err in &diag.errors {
                let msg = format!("{err}");
                assert!(
                    !msg.contains("Missing repository"),
                    "unexpected missing repository: {msg}"
                );
                assert!(!msg.contains("not loaded"), "unexpected not loaded: {msg}");
            }
        }
        if root.exists() {
            std::fs::remove_dir_all(&root).expect("BUG: remove test workspace");
        }
    }

    #[test]
    fn inline_registry_repo_spec_loads_with_dependency_provenance() {
        let root = std::env::temp_dir().join("lemma_lsp_inline_registry_repo_test");
        if root.exists() {
            std::fs::remove_dir_all(&root).expect("BUG: remove test workspace");
        }
        let dep_path = lemma::deps::dependency_cache_file(&root, "@user/somedep");
        std::fs::create_dir_all(dep_path.parent().expect("dep parent")).expect("create dep dir");
        let src = "spec consumer\nuses @user/somedep some_spec\ndata x: 1\n\nrepo @user/somedep\nspec some_spec\ndata y: 2\n";
        std::fs::write(&dep_path, src).expect("write dep");

        let mut workspace = WorkspaceModel::new();
        workspace.set_workspace_root(root.clone());
        let url_dep = Uri::from_file_path(&dep_path).expect("dep url");
        workspace.update_file(url_dep, src.to_string());

        workspace.validate_workspace();
        let engine = workspace.engine();
        let shown = engine
            .show(Some("@user/somedep"), "some_spec", None)
            .expect("some_spec must show");
        assert_eq!(
            shown.source_type.as_ref().map(|st| st.to_string()),
            Some("@user/somedep".to_string()),
            "expected Dependency source_type"
        );

        if root.exists() {
            std::fs::remove_dir_all(&root).expect("BUG: remove test workspace");
        }
    }

    fn hex_utf8_path_segment(s: &str) -> String {
        s.bytes().map(|b| format!("{:02x}", b)).collect()
    }

    #[test]
    fn virtual_bundle_hex_path_scopes_unnamed_repo_like_dependency_load() {
        let dep_id = "@scope/pkg";
        let hex = hex_utf8_path_segment(dep_id);
        let dep_url = format!("file:///lemma/repo/{hex}.lemma")
            .parse::<Uri>()
            .expect("dep url");
        let main_url = url_from_path("/tmp/main.lemma");

        let mut workspace = WorkspaceModel::new();
        workspace.update_file(dep_url, "spec constants\ndata x: 1".to_string());
        workspace.update_file(
            main_url,
            "spec root\nuses @scope/pkg constants\nrule ok: constants.x".to_string(),
        );

        let results = workspace.validate_workspace();
        for result in &results {
            assert!(
                result.errors.is_empty(),
                "file {}: {:?}",
                result.url.as_str(),
                result.errors
            );
        }
    }

    #[test]
    fn validate_workspace_uses_lemma_duration_compound_units() {
        let mut workspace = WorkspaceModel::new();
        let url = url_from_path("/tmp/contractor.lemma");
        workspace.update_file(
            url,
            r#"spec contractor
uses lemma units

data money: measure
  -> unit eur: 1.00

data wage_rate: measure
  -> unit eur_per_second: eur/second
  -> unit eur_per_hour: eur/hour

rule smoke: true
"#
            .to_string(),
        );

        let results = workspace.validate_workspace();
        assert_eq!(results.len(), 1);
        assert!(
            results[0].errors.is_empty(),
            "uses lemma units must resolve stdlib duration units: {:?}",
            results[0].errors
        );
    }

    #[test]
    fn validate_workspace_rejects_stdlib_units_missing_with_target() {
        let mut workspace = WorkspaceModel::new();
        let url = url_from_path("/tmp/coffee_order.lemma");
        workspace.update_file(
            url.clone(),
            r#"spec coffee_order
uses lemma units
  -> with sdfsdf: 34
rule r: 1
"#
            .to_string(),
        );

        let results = workspace.validate_workspace();
        let file = results
            .iter()
            .find(|r| r.url == url)
            .expect("coffee_order file diagnostics");
        let joined: String = file
            .errors
            .iter()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            joined.contains("`units` has no data field `sdfsdf`"),
            "expected planning error on invalid uses binding, got:\n{joined}"
        );
    }

    #[test]
    fn validate_workspace_reports_duplicate_spec_on_both_files() {
        let mut workspace = WorkspaceModel::new();
        let url_a = url_from_path("/tmp/conflict_a.lemma");
        let url_b = url_from_path("/tmp/conflict_b.lemma");

        workspace.update_file(url_a.clone(), "spec conflict_test\ndata x: 1".to_string());
        workspace.update_file(url_b.clone(), "spec conflict_test\ndata x: 2".to_string());

        let results = workspace.validate_workspace();
        let file_a = results
            .iter()
            .find(|r| r.url == url_a)
            .expect("file_a diagnostics");
        let file_b = results
            .iter()
            .find(|r| r.url == url_b)
            .expect("file_b diagnostics");

        assert!(
            !file_a.errors.is_empty(),
            "first declaring file must report duplicate spec conflict: {:?}",
            file_a.errors
        );
        assert!(
            !file_b.errors.is_empty(),
            "second declaring file must report duplicate spec conflict: {:?}",
            file_b.errors
        );

        let joined_a: String = file_a
            .errors
            .iter()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let joined_b: String = file_b
            .errors
            .iter()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            joined_a.contains("conflict_test") && joined_a.contains("also declared"),
            "expected conflict message on file_a, got:\n{joined_a}"
        );
        assert!(
            joined_b.contains("conflict_test") && joined_b.contains("also declared"),
            "expected conflict message on file_b, got:\n{joined_b}"
        );
    }

    #[test]
    fn validate_workspace_rejects_forward_pin_same_name_import() {
        let mut workspace = WorkspaceModel::new();
        let url = url_from_path("/tmp/finance_consumer.lemma");
        workspace.update_file(
            url.clone(),
            r#"spec finance
data rate: number -> suggest 0

spec finance 2026-05-20
uses fin: finance 2027
"#
            .to_string(),
        );

        let results = workspace.validate_workspace();
        let consumer = results
            .iter()
            .find(|r| r.url == url)
            .expect("consumer file diagnostics");
        assert!(
            !consumer.errors.is_empty(),
            "forward pin without exact slice must produce diagnostics: {:?}",
            consumer.errors
        );
        let joined: String = consumer
            .errors
            .iter()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            joined.contains("active at that instant") || joined.contains("cannot reference itself"),
            "expected planning import error, got: {joined}"
        );
    }
}

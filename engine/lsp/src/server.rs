#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::Notify;
use tokio::sync::RwLock;
use tower_lsp_server::jsonrpc::{Error, Result};
use tower_lsp_server::ls_types::*;
use tower_lsp_server::{Client, LanguageServer};

use crate::completion;
use crate::diagnostics;
use crate::semantic_tokens;
#[cfg(not(target_arch = "wasm32"))]
use crate::workspace::file_path_of;
use crate::workspace::WorkspaceModel;
#[cfg(not(target_arch = "wasm32"))]
use crate::workspace_files::{DiskLemmaFile, WatchGuard, WorkspaceFiles, WorkspaceFilesError};
use lemma::{DataValue, SpecRef};

async fn publish_workspace_diagnostics(client: &Client, workspace: &mut WorkspaceModel) {
    let file_diagnostics = workspace.validate_workspace();
    for file_diag in file_diagnostics {
        let lsp_diagnostics = diagnostics::errors_to_diagnostics(
            &file_diag.errors,
            &file_diag.text,
            &file_diag.attribute,
        );
        client
            .publish_diagnostics(file_diag.url, lsp_diagnostics, None)
            .await;
    }
}

/// Shared mutable state accessed by both the LSP handlers and the debounce background task.
struct SharedState {
    workspace: RwLock<WorkspaceModel>,
    #[cfg(not(target_arch = "wasm32"))]
    debounce_notify: Notify,
    /// The workspace root URI, set during `initialize`.
    root_uri: RwLock<Option<Uri>>,
    /// Attributes of documents currently open in the editor.
    open_attributes: RwLock<std::collections::HashSet<String>>,
    /// Keeps the CLI filesystem watch alive for the process lifetime.
    #[cfg(not(target_arch = "wasm32"))]
    watch_guard: RwLock<Option<WatchGuard>>,
}

/// The Lemma Language Server.
///
/// Implements the LSP protocol for Lemma files:
/// - Diagnostics (parse errors + planning errors) published on file open/change
/// - LemmaBase navigation URLs and local `lemma_deps/` document links from parsed [`SpecRef`] spans
pub struct LemmaLanguageServer {
    client: Client,
    state: Arc<SharedState>,
    registries: lemma::Registries,
    /// Host-injected disk access (CLI). `None` on WASM / buffer-only mode.
    #[cfg(not(target_arch = "wasm32"))]
    workspace_files: Option<Arc<dyn WorkspaceFiles>>,
}

impl LemmaLanguageServer {
    pub fn new(
        client: Client,
        #[cfg(not(target_arch = "wasm32"))] workspace_files: Option<Arc<dyn WorkspaceFiles>>,
    ) -> Self {
        Self {
            client,
            state: Arc::new(SharedState {
                workspace: RwLock::new(WorkspaceModel::new()),
                #[cfg(not(target_arch = "wasm32"))]
                debounce_notify: Notify::new(),
                root_uri: RwLock::new(None),
                open_attributes: RwLock::new(std::collections::HashSet::new()),
                #[cfg(not(target_arch = "wasm32"))]
                watch_guard: RwLock::new(None),
            }),
            registries: lemma::Registries::default(),
            #[cfg(not(target_arch = "wasm32"))]
            workspace_files,
        }
    }

    /// Signal the debounce task that a workspace re-validation is needed.
    #[cfg(not(target_arch = "wasm32"))]
    fn request_workspace_validation(&self) {
        self.state.debounce_notify.notify_one();
    }

    /// Apply a full disk payload from the host into the workspace model.
    #[cfg(not(target_arch = "wasm32"))]
    async fn apply_disk_files_from_host(&self, files: Vec<DiskLemmaFile>) {
        let mut disk_entries = Vec::with_capacity(files.len());
        for file in files {
            let Some(url) = Uri::from_file_path(&file.path) else {
                panic!(
                    "BUG: disk lemma path must be absolute for URL conversion: {}",
                    file.path.display()
                );
            };
            disk_entries.push((url, file.text));
        }

        let open_attributes = self.state.open_attributes.read().await.clone();
        {
            let mut workspace = self.state.workspace.write().await;
            workspace.apply_disk_files(&disk_entries, &open_attributes);
        }
        self.request_workspace_validation();
    }

    /// Load disk files for `root_path` via the injected host, then start watching.
    ///
    /// Without an injected host (buffer-only mode) there is no disk workspace to load.
    #[cfg(not(target_arch = "wasm32"))]
    async fn load_and_watch_workspace_disk(&self, root_path: &Path) {
        let Some(workspace_files) = self.workspace_files.as_ref() else {
            return;
        };

        {
            let mut workspace = self.state.workspace.write().await;
            workspace.set_workspace_root(root_path.to_path_buf());
        }

        match workspace_files.load(root_path) {
            Ok(files) => self.apply_disk_files_from_host(files).await,
            Err(error) => {
                self.client
                    .log_message(
                        MessageType::ERROR,
                        format!("Lemma workspace disk load failed: {error}"),
                    )
                    .await;
                return;
            }
        }

        let state = Arc::clone(&self.state);
        let client = self.client.clone();
        let workspace_files = Arc::clone(workspace_files);
        let root_for_watch = root_path.to_path_buf();
        let runtime = tokio::runtime::Handle::current();
        let (on_change, changes) = std::sync::mpsc::channel::<
            std::result::Result<Vec<DiskLemmaFile>, WorkspaceFilesError>,
        >();
        let worker_runtime = runtime.clone();
        drop(runtime.spawn_blocking(move || {
            for result in changes {
                worker_runtime.block_on(async {
                    match result {
                        Ok(files) => {
                            let open_attributes = state.open_attributes.read().await.clone();
                            let mut disk_entries = Vec::with_capacity(files.len());
                            for file in files {
                                let Some(url) = Uri::from_file_path(&file.path) else {
                                    panic!(
                                        "BUG: disk lemma path must be absolute for URL conversion: {}",
                                        file.path.display()
                                    );
                                };
                                disk_entries.push((url, file.text));
                            }
                            {
                                let mut workspace = state.workspace.write().await;
                                workspace.apply_disk_files(&disk_entries, &open_attributes);
                            }
                            state.debounce_notify.notify_one();
                        }
                        Err(error) => {
                            client
                                .log_message(
                                    MessageType::ERROR,
                                    format!("Lemma workspace disk watch reload failed: {error}"),
                                )
                                .await;
                        }
                    }
                });
            }
        }));

        match workspace_files.watch(root_for_watch, on_change) {
            Ok(guard) => {
                let mut watch_guard = self.state.watch_guard.write().await;
                *watch_guard = Some(guard);
            }
            Err(error) => {
                self.client
                    .log_message(
                        MessageType::ERROR,
                        format!("Lemma workspace disk watch failed to start: {error}"),
                    )
                    .await;
            }
        }
    }

    /// Run full workspace validation inline and publish all diagnostics.
    ///
    /// On WASM there is no background debounce task (it requires `Send` futures),
    /// so we validate synchronously inside `did_open`/`did_change` instead.
    /// This is fine for the playground: a single file, no registry, fast validation.
    #[cfg(target_arch = "wasm32")]
    async fn publish_full_diagnostics(&self) {
        let mut workspace = self.state.workspace.write().await;
        publish_workspace_diagnostics(&self.client, &mut workspace).await;
    }

    /// Spawn the background debounce task.
    ///
    /// Waits for a quiet period after edits, then runs full workspace validation
    /// (parse + planning). Fetched registry bundles under `<workspace>/lemma_deps/` are loaded
    /// like the CLI; unresolved `@` references surface as planning errors.
    ///
    /// Not available on WASM — `tokio::spawn` requires `Send` and the WASM LSP
    /// path uses a different runtime model (single-threaded, no native tokio spawn).
    #[cfg(not(target_arch = "wasm32"))]
    fn spawn_debounce_task(&self) {
        let state = Arc::clone(&self.state);
        let client = self.client.clone();

        let debounce = tokio::spawn(async move {
            loop {
                state.debounce_notify.notified().await;

                loop {
                    let timeout_result = tokio::time::timeout(
                        Duration::from_millis(250),
                        state.debounce_notify.notified(),
                    )
                    .await;
                    match timeout_result {
                        Ok(()) => continue,
                        Err(_) => break,
                    }
                }

                let mut workspace = state.workspace.write().await;
                publish_workspace_diagnostics(&client, &mut workspace).await;
            }
        });

        tokio::spawn(async move {
            match debounce.await {
                Ok(()) => unreachable!("BUG: debounce loop never returns"),
                Err(join_error) => match join_error.try_into_panic() {
                    Ok(_) => std::process::abort(),
                    Err(cancelled) => {
                        panic!("BUG: debounce task cancelled without shutdown: {cancelled}")
                    }
                },
            }
        });
    }
}

impl LanguageServer for LemmaLanguageServer {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        let workspace_root = match params.workspace_folders.as_deref() {
            None | Some([]) => None,
            Some([folder]) => {
                #[cfg(not(target_arch = "wasm32"))]
                if file_path_of(&folder.uri).is_none() {
                    return Err(Error::invalid_params(format!(
                        "workspace folder must be a file URI: {}",
                        folder.uri.as_str()
                    )));
                }
                Some(folder.uri.clone())
            }
            Some(folders) => {
                return Err(Error::invalid_params(format!(
                    "multi-root workspaces are not supported, got {} folders",
                    folders.len()
                )));
            }
        };
        *self.state.root_uri.write().await = workspace_root;

        Ok(InitializeResult {
            offset_encoding: None,
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::FULL),
                        save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                            include_text: Some(true),
                        })),
                        will_save: None,
                        will_save_wait_until: None,
                    },
                )),
                document_link_provider: Some(DocumentLinkOptions {
                    resolve_provider: Some(false),
                    work_done_progress_options: WorkDoneProgressOptions {
                        work_done_progress: Some(false),
                    },
                }),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                completion_provider: Some(CompletionOptions {
                    resolve_provider: Some(false),
                    trigger_characters: Some(vec![".".into(), ">".into(), ":".into(), " ".into()]),
                    ..Default::default()
                }),
                document_formatting_provider: Some(OneOf::Left(true)),
                semantic_tokens_provider: Some(
                    SemanticTokensServerCapabilities::SemanticTokensOptions(
                        SemanticTokensOptions {
                            legend: SemanticTokensLegend {
                                token_types: semantic_tokens::TOKEN_TYPES.to_vec(),
                                token_modifiers: semantic_tokens::TOKEN_MODIFIERS.to_vec(),
                            },
                            full: Some(SemanticTokensFullOptions::Bool(true)),
                            range: None,
                            ..SemanticTokensOptions::default()
                        },
                    ),
                ),
                ..ServerCapabilities::default()
            },
            server_info: Some(ServerInfo {
                name: "lsp".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
        })
    }

    async fn initialized(&self, InitializedParams {}: InitializedParams) {
        // Spawn the debounce background task (native only; requires Send futures).
        #[cfg(not(target_arch = "wasm32"))]
        self.spawn_debounce_task();

        // Load workspace `.lemma` files via injected CLI disk access (native only).
        #[cfg(not(target_arch = "wasm32"))]
        {
            let root_uri = {
                let root = self.state.root_uri.read().await;
                root.clone()
            };
            if let Some(root_uri) = root_uri {
                let root_path = file_path_of(&root_uri)
                    .expect("BUG: workspace root validated as file URI in initialize");
                self.load_and_watch_workspace_disk(&root_path).await;
                let mut workspace = self.state.workspace.write().await;
                publish_workspace_diagnostics(&self.client, &mut workspace).await;
            }
        }

        self.client
            .log_message(MessageType::INFO, "Lemma LSP server initialized")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let text = params.text_document.text;
        let attribute = WorkspaceModel::attribute_for_url_public(&uri);

        {
            let mut open_attributes = self.state.open_attributes.write().await;
            open_attributes.insert(attribute);
        }
        {
            let mut workspace = self.state.workspace.write().await;
            workspace.update_file(uri.clone(), text);
        }

        #[cfg(not(target_arch = "wasm32"))]
        self.request_workspace_validation();
        #[cfg(target_arch = "wasm32")]
        self.publish_full_diagnostics().await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;

        // With FULL sync, the last content change contains the entire spec.
        if let Some(change) = params.content_changes.into_iter().last() {
            {
                let mut workspace = self.state.workspace.write().await;
                workspace.update_file(uri.clone(), change.text);
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        self.request_workspace_validation();
        #[cfg(target_arch = "wasm32")]
        self.publish_full_diagnostics().await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let uri = params.text_document.uri;
        {
            let mut workspace = self.state.workspace.write().await;
            if let Some(text) = params.text {
                workspace.update_file(uri.clone(), text);
            }
            workspace.record_saved_baseline(&uri);
        }
        #[cfg(not(target_arch = "wasm32"))]
        self.request_workspace_validation();
        #[cfg(target_arch = "wasm32")]
        self.publish_full_diagnostics().await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        let attribute = WorkspaceModel::attribute_for_url_public(&uri);

        {
            let mut open_attributes = self.state.open_attributes.write().await;
            open_attributes.remove(&attribute);
        }

        let cleared_open_only = {
            let mut workspace = self.state.workspace.write().await;
            if workspace.is_disk_backed(&uri) {
                let restored = workspace.restore_disk_text(&uri);
                if !restored {
                    panic!(
                        "BUG: disk-backed URI missing disk text after close: {}",
                        uri.as_str()
                    );
                }
                false
            } else {
                workspace.remove_file(&uri);
                true
            }
        };

        if cleared_open_only {
            self.client.publish_diagnostics(uri, Vec::new(), None).await;
        }

        #[cfg(not(target_arch = "wasm32"))]
        self.request_workspace_validation();
        #[cfg(target_arch = "wasm32")]
        self.publish_full_diagnostics().await;
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let uri = params.text_document.uri;

        let (text, attribute) = {
            let workspace = self.state.workspace.read().await;
            match workspace.get_file_text_and_attribute(&uri) {
                Some((text, attribute)) => (text.to_string(), attribute.to_string()),
                None => return Ok(None),
            }
        };

        // Only format if the file parses successfully — don't mangle broken code.
        match lemma::format_source(
            &text,
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(&attribute))),
        ) {
            Ok(formatted) if formatted == text => Ok(None), // No changes needed
            Ok(formatted) => {
                let line_count = text.lines().count() as u32;
                // Replace the entire spec with the formatted text.
                let edit = TextEdit {
                    range: Range {
                        start: Position::new(0, 0),
                        end: Position::new(line_count, 0),
                    },
                    new_text: formatted,
                };
                Ok(Some(vec![edit]))
            }
            Err(_) => Ok(None), // Parse error — don't format
        }
    }

    /// Resolve `uses` targets to document links against the long-lived engine.
    ///
    /// Links reflect the last `validate_workspace` pass: between a `didChange` and
    /// the debounced validation, targets may still resolve against pre-edit state.
    async fn document_link(&self, params: DocumentLinkParams) -> Result<Option<Vec<DocumentLink>>> {
        #[cfg(target_arch = "wasm32")]
        {
            drop(params);
            Ok(None)
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            let uri = params.text_document.uri;
            let workspace = self.state.workspace.read().await;
            let Some(text) = workspace.get_file_text(&uri).map(|t| t.to_string()) else {
                return Ok(None);
            };
            let Some(parse_result) = workspace.parse_success_for_url(&uri) else {
                return Ok(None);
            };
            let Some(root) = workspace.workspace_root().cloned() else {
                return Ok(None);
            };
            let engine = workspace.engine();
            let text = text.as_str();
            let root = root.as_path();

            let mut links: Vec<DocumentLink> = Vec::new();
            for consumer in parse_result.repositories.values().flatten() {
                for data in &consumer.data {
                    let DataValue::Import { spec_ref, .. } = &data.value else {
                        continue;
                    };
                    if let Some(link) = build_uses_document_link(
                        spec_ref,
                        consumer.effective_from(),
                        text,
                        root,
                        engine,
                    ) {
                        links.push(link);
                    }
                }
            }

            Ok((!links.is_empty()).then_some(links))
        }
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let workspace = self.state.workspace.read().await;
        let Some(text) = workspace.get_file_text(&uri).map(|t| t.to_string()) else {
            return Ok(None);
        };
        let Some(parse_result) = workspace.parse_success_for_url(&uri) else {
            return Ok(None);
        };

        for specs in parse_result.repositories.values() {
            for consumer in specs {
                for data in &consumer.data {
                    let DataValue::Import { spec_ref, .. } = &data.value else {
                        continue;
                    };
                    let Some(repo_qual) = spec_ref.repository.as_ref() else {
                        continue;
                    };
                    if !repo_qual.is_registry() {
                        continue;
                    }
                    let qualifier_name = repo_qual.name.as_str();
                    let Some(hit_range) = spec_ref_hit_range(spec_ref, &text, position) else {
                        continue;
                    };
                    let Some(repo_url) = self
                        .registries
                        .registry_for(repo_qual)
                        .navigation_url(qualifier_name, None)
                    else {
                        return Ok(None);
                    };
                    let markdown = format!("[Open `{qualifier_name}` in LemmaBase]({repo_url})");
                    return Ok(Some(Hover {
                        contents: HoverContents::Markup(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: markdown,
                        }),
                        range: Some(hit_range),
                    }));
                }
            }
        }

        Ok(None)
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let workspace = self.state.workspace.read().await;
        let Some(text) = workspace.get_file_text(&uri) else {
            return Ok(None);
        };
        let parse = workspace.parse_success_for_url(&uri);
        let engine = workspace.engine();
        let items = completion::complete(text, position, parse, Some(engine));
        Ok(Some(CompletionResponse::List(CompletionList {
            is_incomplete: false,
            items,
        })))
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        let uri = params.text_document.uri;

        let text = {
            let workspace = self.state.workspace.read().await;
            workspace.get_file_text(&uri).map(|t| t.to_string())
        };

        match text {
            Some(text) => {
                let tokens = semantic_tokens::tokenize(&text);
                Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
                    result_id: None,
                    data: tokens,
                })))
            }
            None => Ok(None),
        }
    }
}

/// Return the LSP `Range` of the [`SpecRef`] span that `position` is currently
/// inside (qualifier span or target span), or `None` if `position` is not on
/// either span. Used by the hover handler to scope the popup to the hovered
/// portion of the reference.
fn spec_ref_hit_range(spec_ref: &SpecRef, text: &str, position: Position) -> Option<Range> {
    let qualifier_range = spec_ref
        .repository_span
        .as_ref()
        .map(|s| diagnostics::span_to_range(text, s.start, s.end));
    let target_range = spec_ref
        .target_span
        .as_ref()
        .map(|s| diagnostics::span_to_range(text, s.start, s.end));
    qualifier_range
        .filter(|range| diagnostics::position_within_range(position, range))
        .or_else(|| {
            target_range.filter(|range| diagnostics::position_within_range(position, range))
        })
}

/// Return one LSP `Range` covering the full `uses` reference from the qualifier
/// (e.g. `@iso/countries` or `lemma`) through the target span (`alpha2 2026-01-01`,
/// `units`). Used by `document_link` so the entire reference is a single clickable
/// region, instead of two separate hot spots on `repository_span` and
/// `target_span`. Falls back to whichever span is present if one is missing.
#[cfg(not(target_arch = "wasm32"))]
fn full_ref_range(spec_ref: &SpecRef, text: &str) -> Option<Range> {
    let start = spec_ref
        .repository_span
        .as_ref()
        .or(spec_ref.target_span.as_ref())
        .map(|s| s.start)?;
    let end = spec_ref
        .target_span
        .as_ref()
        .or(spec_ref.repository_span.as_ref())
        .map(|s| s.end)?;
    Some(diagnostics::span_to_range(text, start, end))
}

/// Build a single `DocumentLink` covering the entire `uses` reference (qualifier +
/// target span) and pointing at the local on-disk file backing the reference:
/// - Registry refs (`@user/repo`): `<workspace>/lemma_deps/<qualifier>.lemma#L<line>` or the
///   actual `SourceType::Path` of the resolved spec if the workspace already has it.
/// - Embedded stdlib (`uses lemma ...`): `<workspace>/lemma_deps/lemma.std#L<line>`, lazily
///   materialised on first call.
///
/// Returns `None` when the reference is not a registry/embedded-stdlib qualifier, when
/// no span information is available, when the qualifier isn't loaded into `ctx`, when
/// the spec name can't be resolved at the consumer's effective slice, or when the
/// destination path can't be expressed as a `file://` URL. The single combined range
/// guarantees that VS Code renders one clickable region for the full `uses` token group
/// instead of separate hot spots on the qualifier and the spec name.
#[cfg(not(target_arch = "wasm32"))]
fn build_uses_document_link(
    spec_ref: &SpecRef,
    consumer_effective_from: Option<&lemma::DateTimeValue>,
    text: &str,
    workspace_root: &Path,
    engine: &lemma::Engine,
) -> Option<DocumentLink> {
    let repo_qual = spec_ref.repository.as_ref()?;
    let qualifier_name = repo_qual.name.as_str();
    let is_embedded_stdlib = qualifier_name == lemma::EMBEDDED_STDLIB_REPOSITORY;
    if !repo_qual.is_registry() && !is_embedded_stdlib {
        return None;
    }
    let full_range = full_ref_range(spec_ref, text)?;
    let instant_dt = spec_ref.resolved_instant(consumer_effective_from)?;
    let shown = engine
        .show(
            Some(qualifier_name),
            spec_ref.name.as_str(),
            Some(&instant_dt),
        )
        .ok()?;

    let dep_path = if is_embedded_stdlib {
        ensure_embedded_stdlib_view(workspace_root)?
    } else {
        match &shown.source_type {
            Some(lemma::SourceType::Path(path)) => path.as_ref().clone(),
            _ => lemma::deps::dependency_cache_file(workspace_root, qualifier_name),
        }
    };
    let file_url = format!(
        "{}#L{}",
        Uri::from_file_path(&dep_path)?.as_str(),
        shown.start_line
    )
    .parse::<Uri>()
    .expect("BUG: file URI with line fragment must parse");
    Some(DocumentLink {
        range: full_range,
        target: Some(file_url),
        tooltip: Some(format!(
            "Open {} (line {})",
            dep_path.display(),
            shown.start_line
        )),
        data: None,
    })
}

/// Path under `<workspace>/lemma_deps/` where the LSP writes a view-only copy of the embedded
/// units standard library for editor navigation. The `.std` extension keeps the file out of
/// every `.lemma` discovery pass (CLI loaders and watchers).
#[cfg(not(target_arch = "wasm32"))]
fn embedded_stdlib_view_path(workspace_root: &Path) -> std::path::PathBuf {
    lemma::deps::lemma_deps_dir(workspace_root).join("lemma.std")
}

/// Lazily write the embedded units standard library to `<workspace>/lemma_deps/lemma.std`.
///
/// Called from `document_link` only when a `uses lemma ...` reference is present in the
/// active file, so the file appears in `lemma_deps/` the first time a user wants to navigate
/// into the standard library and never otherwise. Returns the destination path on success
/// (or when the file already exists with the current content); returns `None` if the
/// filesystem write failed -- callers then skip emitting the link rather than producing
/// a link to a non-existent target.
#[cfg(not(target_arch = "wasm32"))]
fn ensure_embedded_stdlib_view(workspace_root: &Path) -> Option<std::path::PathBuf> {
    let destination = embedded_stdlib_view_path(workspace_root);
    let expected = lemma::UNITS_LEMMA;
    match std::fs::read_to_string(&destination) {
        Ok(current) if current == expected => return Some(destination),
        Ok(_) | Err(_) => {}
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }
    std::fs::write(&destination, expected).ok()?;
    Some(destination)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use lemma::DataValue;
    use lemma::EMBEDDED_STDLIB_REPOSITORY;

    #[test]
    fn position_within_range_treats_end_as_exclusive() {
        let range = Range {
            start: Position {
                line: 1,
                character: 5,
            },
            end: Position {
                line: 1,
                character: 10,
            },
        };
        assert!(diagnostics::position_within_range(
            Position {
                line: 1,
                character: 5
            },
            &range
        ));
        assert!(diagnostics::position_within_range(
            Position {
                line: 1,
                character: 9
            },
            &range
        ));
        assert!(!diagnostics::position_within_range(
            Position {
                line: 1,
                character: 10
            },
            &range
        ));
        assert!(!diagnostics::position_within_range(
            Position {
                line: 1,
                character: 4
            },
            &range
        ));
        assert!(!diagnostics::position_within_range(
            Position {
                line: 0,
                character: 7
            },
            &range
        ));
        assert!(!diagnostics::position_within_range(
            Position {
                line: 2,
                character: 0
            },
            &range
        ));
    }

    #[test]
    fn position_within_range_spans_multiple_lines() {
        let range = Range {
            start: Position {
                line: 1,
                character: 5,
            },
            end: Position {
                line: 3,
                character: 2,
            },
        };
        assert!(diagnostics::position_within_range(
            Position {
                line: 1,
                character: 5
            },
            &range
        ));
        assert!(diagnostics::position_within_range(
            Position {
                line: 2,
                character: 100
            },
            &range
        ));
        assert!(diagnostics::position_within_range(
            Position {
                line: 3,
                character: 0
            },
            &range
        ));
        assert!(!diagnostics::position_within_range(
            Position {
                line: 3,
                character: 2
            },
            &range
        ));
        assert!(!diagnostics::position_within_range(
            Position {
                line: 1,
                character: 4
            },
            &range
        ));
    }

    /// Fresh empty temp directory unique to the current test process (nextest runs
    /// each test in its own process, so pid alone is unique).
    fn test_workspace() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("lemma_lsp_test_{}", std::process::id()));
        if path.exists() {
            std::fs::remove_dir_all(&path).expect("BUG: clear stale test workspace");
        }
        std::fs::create_dir_all(&path).expect("create test workspace");
        path
    }

    #[test]
    fn embedded_stdlib_view_path_lives_under_lemma_deps() {
        let workspace = test_workspace();
        let path = embedded_stdlib_view_path(&workspace);
        assert_eq!(path.file_name().and_then(|n| n.to_str()), Some("lemma.std"));
        assert_eq!(
            path.parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str()),
            Some(lemma::deps::LEMMA_DEPS_DIR_NAME),
        );
        assert!(
            path.extension().and_then(|e| e.to_str()) != Some("lemma"),
            "view path must not use the .lemma extension or it would be picked up by workspace discovery",
        );
    }

    #[test]
    fn ensure_embedded_stdlib_writes_units_lemma_when_missing() {
        let workspace = test_workspace();
        let view_path = ensure_embedded_stdlib_view(&workspace)
            .expect("first ensure_embedded_stdlib_view must succeed");
        assert_eq!(view_path, embedded_stdlib_view_path(&workspace));
        let written = std::fs::read_to_string(&view_path).expect("written file readable");
        assert_eq!(written, lemma::UNITS_LEMMA);
    }

    #[test]
    fn ensure_embedded_stdlib_is_idempotent_when_content_already_matches() {
        let workspace = test_workspace();
        let first = ensure_embedded_stdlib_view(&workspace)
            .expect("first ensure_embedded_stdlib_view must succeed");
        let metadata_before = std::fs::metadata(&first).expect("metadata before");
        std::thread::sleep(std::time::Duration::from_millis(20));
        let second = ensure_embedded_stdlib_view(&workspace)
            .expect("second ensure_embedded_stdlib_view must succeed");
        assert_eq!(first, second);
        let metadata_after = std::fs::metadata(&second).expect("metadata after");
        assert_eq!(
            metadata_before.modified().expect("mtime before"),
            metadata_after.modified().expect("mtime after"),
            "matching content must not be re-written",
        );
    }

    #[test]
    fn ensure_embedded_stdlib_overwrites_stale_content() {
        let workspace = test_workspace();
        let destination = embedded_stdlib_view_path(&workspace);
        std::fs::create_dir_all(destination.parent().expect("deps dir parent"))
            .expect("create lemma_deps dir");
        std::fs::write(&destination, "outdated stdlib snapshot").expect("seed stale content");
        let view_path = ensure_embedded_stdlib_view(&workspace)
            .expect("ensure_embedded_stdlib_view must succeed");
        let written = std::fs::read_to_string(&view_path).expect("written file readable");
        assert_eq!(written, lemma::UNITS_LEMMA);
    }

    /// Mirrors the link-resolution body of `document_link` for the embedded stdlib path,
    /// so we can exercise it without constructing a `tower_lsp_server::Client`. Asserts both the
    /// lazy write and that we can derive a valid `file://` URL at the resolved `spec units`
    /// line number.
    #[test]
    fn uses_lemma_units_produces_link_to_lemma_deps_lemma_std() {
        let workspace = test_workspace();
        let workspace_root = workspace.as_path();
        let source = "spec consumer\nuses lemma units\n";

        let parse_result = lemma::parse(
            source,
            lemma::SourceType::Path(Arc::new(workspace_root.join("consumer.lemma"))),
            &lemma::ResourceLimits::default(),
        )
        .expect("parse consumer");

        let mut engine = lemma::Engine::new();
        engine
            .load([(
                lemma::SourceType::Path(Arc::new(workspace_root.join("consumer.lemma"))),
                source.to_string(),
            )])
            .expect("load consumer");

        let consumer_spec = parse_result
            .repositories
            .values()
            .flatten()
            .next()
            .expect("at least one parsed spec");
        let spec_ref = consumer_spec
            .data
            .iter()
            .find_map(|d| match &d.value {
                DataValue::Import { spec_ref: sr, .. } => Some(sr),
                _ => None,
            })
            .expect("consumer must contain a uses import");

        let repo_qual = spec_ref.repository.as_ref().expect("qualified import");
        assert_eq!(repo_qual.name, EMBEDDED_STDLIB_REPOSITORY);
        assert!(
            !repo_qual.is_registry(),
            "lemma is the reserved embedded stdlib repository, not a registry id",
        );

        let view_path = ensure_embedded_stdlib_view(workspace_root)
            .expect("lazy ensure_embedded_stdlib_view must succeed");
        assert_eq!(view_path, embedded_stdlib_view_path(workspace_root));
        assert!(
            view_path.exists(),
            "embedded stdlib view file must exist on disk",
        );

        let repository_link_target =
            Uri::from_file_path(&view_path).expect("file URL for repository span");
        assert_eq!(
            repository_link_target.scheme().as_str(),
            "file",
            "repository link must be a file:// URL",
        );

        let shown = engine
            .show(
                Some(EMBEDDED_STDLIB_REPOSITORY),
                spec_ref.name.as_str(),
                None,
            )
            .expect("embedded stdlib spec must show");
        assert!(
            shown.start_line >= 1,
            "show start_line must reflect UNITS_LEMMA layout, got {}",
            shown.start_line,
        );

        let target_link = format!(
            "{}#L{}",
            Uri::from_file_path(&view_path)
                .expect("file URL for target span")
                .as_str(),
            shown.start_line
        )
        .parse::<Uri>()
        .expect("file URL with line fragment");
        assert_eq!(
            target_link.fragment().map(|fragment| fragment.as_str()),
            Some(format!("L{}", shown.start_line).as_str()),
        );

        let qualifier_span = spec_ref
            .repository_span
            .as_ref()
            .expect("embedded stdlib uses must carry a repository span");
        let target_span = spec_ref
            .target_span
            .as_ref()
            .expect("embedded stdlib uses must carry a target span");
        let full = full_ref_range(spec_ref, source).expect("full_ref_range must succeed");
        let expected_full =
            diagnostics::span_to_range(source, qualifier_span.start, target_span.end);
        assert_eq!(
            full, expected_full,
            "DocumentLink range must cover `lemma units` as one region (qualifier start to target end)",
        );
    }

    /// Mirrors the markdown-building body of `hover` for a registry `uses` reference
    /// so we can assert the popup shape without spinning up a `tower_lsp_server::Client`.
    /// Verifies exactly one repository-level LemmaBase link and the absence of any
    /// spec-level LemmaBase link or local `lemma_deps/` link.
    #[test]
    fn hover_on_registry_ref_emits_only_repository_lemmabase_link() {
        let workspace = test_workspace();
        let workspace_root = workspace.as_path();
        let source = "spec consumer\nuses @iso/countries alpha2\n";

        let parse_result = lemma::parse(
            source,
            lemma::SourceType::Path(Arc::new(workspace_root.join("consumer.lemma"))),
            &lemma::ResourceLimits::default(),
        )
        .expect("parse consumer");

        let consumer_spec = parse_result
            .repositories
            .values()
            .flatten()
            .next()
            .expect("at least one parsed spec");
        let spec_ref = consumer_spec
            .data
            .iter()
            .find_map(|d| match &d.value {
                DataValue::Import { spec_ref: sr, .. } => Some(sr),
                _ => None,
            })
            .expect("consumer must contain a uses import");
        let repo_qual = spec_ref.repository.as_ref().expect("qualified import");
        assert!(
            repo_qual.is_registry(),
            "@iso/countries must be a registry id"
        );
        let qualifier_name = repo_qual.name.as_str();
        assert_eq!(qualifier_name, "@iso/countries");

        let qualifier_span = spec_ref
            .repository_span
            .as_ref()
            .expect("registry uses must carry a repository span");
        let qualifier_range =
            diagnostics::span_to_range(source, qualifier_span.start, qualifier_span.end);
        let inside_qualifier = qualifier_range.start;
        let hit = spec_ref_hit_range(spec_ref, source, inside_qualifier)
            .expect("position inside qualifier must hit the SpecRef");
        assert_eq!(hit, qualifier_range);

        let target_span = spec_ref
            .target_span
            .as_ref()
            .expect("registry uses must carry a target span");
        let target_range = diagnostics::span_to_range(source, target_span.start, target_span.end);
        let inside_target = target_range.start;
        let hit_target = spec_ref_hit_range(spec_ref, source, inside_target)
            .expect("position inside target span must hit the SpecRef");
        assert_eq!(hit_target, target_range);

        let outside = Position {
            line: 0,
            character: 0,
        };
        assert!(
            spec_ref_hit_range(spec_ref, source, outside).is_none(),
            "positions outside both spans must not produce a hit",
        );

        let registries = lemma::Registries::default();
        let qualifier = lemma::RepositoryQualifier::new(qualifier_name);
        let repo_url = lemma::Registry::navigation_url(
            registries.registry_for(&qualifier),
            qualifier_name,
            None,
        )
        .expect("LemmaBase navigation_url must yield a repository URL");
        let markdown = format!("[Open `{qualifier_name}` in LemmaBase]({repo_url})");

        assert_eq!(
            markdown,
            "[Open `@iso/countries` in LemmaBase](https://lemmabase.com/@iso/countries)",
        );
        assert_eq!(
            markdown.matches("](").count(),
            1,
            "hover popup must contain exactly one markdown link, got: {markdown}",
        );
        let spec_identifier = format!("{} {}", qualifier_name, spec_ref.name);
        assert!(
            !markdown.contains(&spec_identifier),
            "spec-level LemmaBase link must be gone, but markdown contains `{spec_identifier}`: {markdown}",
        );
        assert!(
            !markdown.contains("Open local"),
            "local file hover link must be gone: {markdown}",
        );
        assert!(
            !markdown.contains("file://"),
            "hover popup must not embed a file:// URL: {markdown}",
        );

        let full = full_ref_range(spec_ref, source).expect("full_ref_range must succeed");
        let expected_full =
            diagnostics::span_to_range(source, qualifier_span.start, target_span.end);
        assert_eq!(
            full, expected_full,
            "DocumentLink range must cover `@iso/countries alpha2` as one region",
        );
        assert!(
            full.start.line < full.end.line
                || (full.start.line == full.end.line && full.start.character < full.end.character),
            "full ref range must be non-empty: {full:?}",
        );
        assert_eq!(
            full.start, qualifier_range.start,
            "full ref range must start where the qualifier starts",
        );
    }

    /// End-to-end exercise of the `document_link` body for the canonical user scenario:
    /// a consumer file with `uses @iso/countries alpha2 2026-01-01` plus a matching
    /// `lemma_deps/@iso/countries.lemma` dependency file. Asserts `build_uses_document_link`
    /// emits exactly one `DocumentLink` whose range spans from the start of `@iso/countries`
    /// to the end of `alpha2 2026-01-01` (one clickable region) and whose target is
    /// the on-disk path of the dependency file with a `#L<line>` fragment.
    #[test]
    fn registry_uses_emits_single_unified_document_link() {
        let root = test_workspace();
        let dep_path = lemma::deps::dependency_cache_file(&root, "@iso/countries");
        std::fs::create_dir_all(dep_path.parent().expect("dep parent")).expect("create dep dir");
        std::fs::write(
            &dep_path,
            "spec alpha2\ndata code: text\n -> option \"NL\"\n",
        )
        .expect("write dep");
        let consumer_path = root.join("consumer.lemma");
        let consumer_source = "spec demo\nuses @iso/countries alpha2 2026-01-01\n".to_string();
        std::fs::write(&consumer_path, &consumer_source).expect("write consumer");

        let mut workspace_model = WorkspaceModel::new();
        workspace_model.set_workspace_root(root.clone());
        let consumer_url = Uri::from_file_path(&consumer_path).expect("consumer url");
        let dep_url = Uri::from_file_path(&dep_path).expect("dep url");
        workspace_model.update_file(consumer_url.clone(), consumer_source.clone());
        workspace_model.update_file(
            dep_url,
            std::fs::read_to_string(&dep_path).expect("read dep"),
        );

        workspace_model.validate_workspace();
        let engine = workspace_model.engine();
        assert!(
            engine
                .specs()
                .iter()
                .any(|r| r.repository.as_deref() == Some("@iso/countries")),
            "engine must contain @iso/countries after loading lemma_deps/@iso/countries.lemma",
        );

        let parse_result = workspace_model
            .parse_success_for_url(&consumer_url)
            .expect("consumer must parse cleanly");

        let mut links: Vec<DocumentLink> = Vec::new();
        for specs in parse_result.repositories.values() {
            for consumer in specs {
                for data in &consumer.data {
                    let DataValue::Import { spec_ref, .. } = &data.value else {
                        continue;
                    };
                    if let Some(link) = build_uses_document_link(
                        spec_ref,
                        consumer.effective_from(),
                        &consumer_source,
                        &root,
                        engine,
                    ) {
                        links.push(link);
                    }
                }
            }
        }

        assert_eq!(
            links.len(),
            1,
            "expected exactly one unified DocumentLink, got {}: {links:?}",
            links.len(),
        );
        let link = &links[0];

        let qualifier_start = consumer_source
            .find("@iso/countries")
            .expect("qualifier present");
        let target_end =
            consumer_source.find("2026-01-01").expect("date present") + "2026-01-01".len();
        let expected_range =
            diagnostics::span_to_range(&consumer_source, qualifier_start, target_end);
        assert_eq!(
            link.range, expected_range,
            "DocumentLink range must cover `@iso/countries alpha2 2026-01-01` as one region",
        );

        let target = link.target.as_ref().expect("link must have a target");
        assert_eq!(
            target.scheme().as_str(),
            "file",
            "target must be a file:// URL"
        );
        let target_path = target.to_file_path().expect("target must be a file path");
        assert_eq!(
            target_path, dep_path,
            "DocumentLink must point at the on-disk lemma_deps/@iso/countries.lemma",
        );
        assert_eq!(
            target.fragment().map(|fragment| fragment.as_str()),
            Some("L1"),
            "target must carry a #L<line> fragment for the resolved spec, got: {:?}",
            target.fragment().map(|fragment| fragment.as_str()),
        );
    }
}

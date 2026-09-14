pub mod http {
    use crate::formatter::Formatter;
    use axum::{
        body::Bytes,
        extract::{Path, Query, State},
        http::{header::CONTENT_TYPE, HeaderMap, HeaderValue, StatusCode},
        response::{Html, IntoResponse, Json},
        routing::get,
        Router,
    };
    use lemma::DateTimeValue;
    use lemma::Engine;
    use serde::Deserialize;
    use std::panic::{catch_unwind, AssertUnwindSafe};

    use std::net::SocketAddr;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::RwLock;
    use tower_http::cors::CorsLayer;
    use tracing::{error, info, warn};

    /// Requests take a read lock for the duration of list/show/run. The file
    /// watcher takes a write lock to refresh the same long-lived `Engine` via
    /// per-file `update` (PartialEq skip on unchanged files). On incremental
    /// failure it cold-loads into a scratch engine and swaps only on success;
    /// scratch failure keeps the previous engine (never emptied). Eval holds
    /// the read lock for its full duration, so a runaway evaluation that
    /// outlives the response timeout still blocks the watcher's write lock.
    type SharedEngine = Arc<RwLock<Engine>>;

    fn parse_spec_path(path: &str) -> String {
        path.trim_matches('/').to_string()
    }

    /// Read Accept-Datetime (RFC 7089) from headers; fallback to now.
    fn accept_datetime_from_headers(
        headers: &HeaderMap,
    ) -> Result<DateTimeValue, (StatusCode, Json<ErrorResponse>)> {
        let raw = headers
            .get("Accept-Datetime")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.trim());
        resolve_effective(raw)
    }

    #[derive(Deserialize, Default)]
    struct EffectiveQuery {
        effective: Option<String>,
    }

    #[derive(Deserialize, Default)]
    struct SpecQuery {
        rules: Option<String>,
    }

    fn resolve_effective(
        raw: Option<&str>,
    ) -> Result<DateTimeValue, (StatusCode, Json<ErrorResponse>)> {
        lemma::resolve_effective(raw).map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: e.message().to_string(),
                }),
            )
        })
    }

    #[derive(Clone)]
    struct AppState {
        engine: SharedEngine,
        explain: bool,
        eval_timeout: Duration,
    }

    /// Read guard for request handlers; held for the duration of the call.
    async fn engine_read(state: &AppState) -> tokio::sync::RwLockReadGuard<'_, Engine> {
        state.engine.read().await
    }

    #[derive(Debug, serde::Serialize)]
    struct ErrorResponse {
        error: String,
    }

    fn catch_engine_panic<F, T>(f: F) -> Result<T, (StatusCode, Json<ErrorResponse>)>
    where
        F: FnOnce() -> T + std::panic::UnwindSafe,
    {
        catch_unwind(f).map_err(|panic_payload| {
            let msg = panic_payload
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| panic_payload.downcast_ref::<String>().map(|s| s.as_str()))
                .unwrap_or("unknown internal error");
            error!("engine panic caught: {}", msg);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "internal engine error".to_string(),
                }),
            )
        })
    }

    /// Build Memento-Datetime, Vary for the resolved spec.
    fn spec_response_headers(
        effective_from: Option<&DateTimeValue>,
    ) -> Vec<(axum::http::header::HeaderName, HeaderValue)> {
        let mut h = Vec::new();
        if let Some(af) = effective_from {
            if let Ok(v) = HeaderValue::from_str(&af.to_string()) {
                h.push((
                    axum::http::header::HeaderName::from_static("memento-datetime"),
                    v,
                ));
            }
        }
        h.push((
            axum::http::header::VARY,
            HeaderValue::from_static("Accept-Datetime"),
        ));
        h
    }

    /// Start the Lemma HTTP server.
    ///
    /// The server auto-generates typed REST endpoints for each loaded spec:
    /// - `GET /{spec}` — show interface (data, rules, versions); no evaluate, no data query params
    /// - `POST /{spec}` — evaluate (`?rules=` scopes which rules to run); data as JSON or form body
    ///
    /// Meta routes:
    /// - `GET /` — list all specs
    /// - `GET /health` — health check
    /// - `GET /openapi.json` — OpenAPI 3.1 specification
    /// - `GET /docs` — Scalar interactive documentation
    ///
    /// Deployment posture: the server has no built-in authentication or TLS.
    /// It binds to localhost by default; for any non-localhost deployment, put
    /// it behind a reverse proxy that terminates TLS and enforces access
    /// control. Cross-origin browser access is denied unless `cors` is set.
    #[allow(clippy::too_many_arguments)]
    pub async fn start_server(
        engine: Engine,
        host: &str,
        port: u16,
        watch: bool,
        explain: bool,
        workdir: PathBuf,
        eval_timeout_secs: u64,
        cors: bool,
    ) -> anyhow::Result<()> {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "lemma=info,tower_http=info".into()),
            )
            .init();

        let shared_engine: SharedEngine = Arc::new(RwLock::new(engine));

        let watch_guard = if watch {
            Some(start_file_watcher(shared_engine.clone(), workdir)?)
        } else {
            None
        };

        let state = AppState {
            engine: shared_engine,
            explain,
            eval_timeout: Duration::from_secs(eval_timeout_secs),
        };

        let router = Router::new()
            .route("/", get(list))
            .route("/health", get(health_check))
            .route("/openapi.json", get(openapi_spec))
            .route("/docs", get(scalar_docs))
            .route("/scalar.js", get(scalar_js))
            .route("/{*path}", get(spec_get_show).post(spec_post_evaluate))
            .fallback(fallback_404);
        let router = if cors {
            info!("Permissive CORS enabled (--cors): cross-origin browser requests allowed");
            router.layer(CorsLayer::permissive())
        } else {
            router
        };
        let app = router.with_state(state);

        if !matches!(host, "127.0.0.1" | "localhost" | "::1" | "[::1]") {
            warn!(
                "Binding to non-localhost address {host}: the Lemma HTTP server has no \
                 built-in authentication or TLS. Deploy behind a reverse proxy that \
                 terminates TLS and enforces access control."
            );
        }

        let addr: SocketAddr = format!("{host}:{port}").parse()?;
        info!("Lemma server listening on http://{}", addr);
        info!("Interactive docs at http://{}/docs", addr);

        let listener = tokio::net::TcpListener::bind(addr).await?;
        let os_shutdown_signal = shutdown_signal()?;
        let (drain_timeout_started, drain_timeout_wait) = tokio::sync::oneshot::channel();
        let shutdown = async move {
            os_shutdown_signal.await;
            if drain_timeout_started.send(()).is_err() {
                // serve already finished; drain timer not needed
            }
        };
        let serve_result = tokio::select! {
            result = axum::serve(listener, app).with_graceful_shutdown(shutdown) => {
                if result.is_ok() {
                    info!("all connections drained");
                }
                result
            }
            _ = async {
                match drain_timeout_wait.await {
                    Ok(()) => {
                        tokio::time::sleep(Duration::from_secs(eval_timeout_secs)).await;
                        warn!(
                            "drain timeout after {}s, dropping remaining connections",
                            eval_timeout_secs
                        );
                    }
                    Err(_) => std::future::pending::<()>().await,
                }
            } => Ok(())
        };

        drop(watch_guard);
        serve_result?;
        Ok(())
    }

    /// Wait for SIGINT or SIGTERM (Unix) / Ctrl-C (Windows).
    ///
    /// Installs handlers eagerly; install failure is an environment error.
    fn shutdown_signal() -> anyhow::Result<impl std::future::Future<Output = ()>> {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let mut interrupt = signal(SignalKind::interrupt())?;
            let mut terminate = signal(SignalKind::terminate())?;
            Ok(async move {
                tokio::select! {
                    _ = interrupt.recv() => {
                        info!("SIGINT received, draining in-flight requests");
                    }
                    _ = terminate.recv() => {
                        info!("SIGTERM received, draining in-flight requests");
                    }
                }
            })
        }
        #[cfg(windows)]
        {
            let mut ctrl_c = tokio::signal::windows::ctrl_c()?;
            Ok(async move {
                ctrl_c.recv().await;
                info!("Ctrl-C received, draining in-flight requests");
            })
        }
        #[cfg(not(any(unix, windows)))]
        {
            anyhow::bail!("shutdown signals are not supported on this platform")
        }
    }

    // -----------------------------------------------------------------------
    // Meta routes
    // -----------------------------------------------------------------------

    async fn list(
        State(state): State<AppState>,
    ) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
        let engine = engine_read(&state).await;
        Ok(Json(engine.list()))
    }

    async fn health_check() -> impl IntoResponse {
        Json(serde_json::json!({
            "status": "ok",
            "service": "lemma",
            "version": env!("CARGO_PKG_VERSION")
        }))
    }

    /// Fallback when no route matches — return 404 with JSON body (never empty).
    async fn fallback_404() -> (StatusCode, Json<ErrorResponse>) {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Not found. Use GET / for spec list, GET /docs for API docs.".to_string(),
            }),
        )
    }

    async fn openapi_spec(
        State(state): State<AppState>,
        Query(q): Query<EffectiveQuery>,
    ) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
        let effective = resolve_effective(q.effective.as_deref())?;
        let engine = engine_read(&state).await;
        let spec = lemma_openapi::generate_openapi_effective(&engine, state.explain, &effective);
        Ok(Json(spec))
    }

    async fn scalar_docs(State(state): State<AppState>) -> impl IntoResponse {
        let engine = engine_read(&state).await;
        let sources = lemma_openapi::temporal_api_sources(&engine);

        let shared_opts = r#"layout: 'modern',
      theme: 'solarized',
      agent: { disabled: true },
      hideClientButton: true,
      hideTestRequestButton: false,
      showSidebar: true,
      showDeveloperTools: 'never',
      operationTitleSource: 'summary',
      persistAuth: false,
      telemetry: false,
      hideModels: true,
      documentDownloadType: 'both', // Scalar UI option, not Lemma
      hideSearch: false,
      showOperationId: false,
      hideDarkModeToggle: false,
      withDefaultFonts: false,
      defaultOpenAllTags: false,
      expandAllModelSections: true,
      expandAllResponses: true,
      orderSchemaPropertiesBy: 'alpha',
      orderRequiredPropertiesFirst: true,
      customCss: `
        a[href="https://www.scalar.com"] {
          font-size: 0 !important;
        }
        a[href="https://www.scalar.com"]::after {
          content: 'Powered by Lemma';
          font-size: var(--scalar-mini, 10px);
        }
        /*
         * Evaluate client Response → Body (not .response-body-virtual).
         * Scalar hardcodes overflow-y-hidden + max-h-fit on the Body panel, so
         * CodeMirror grows to full JSON height and the pane clips with no scroll.
         */
        .scalar-app .response-section-content-body.overflow-y-hidden,
        .scalar-app .response-section-content-body {
          overflow-y: auto !important;
          max-height: 70vh !important;
          min-height: 0 !important;
        }
        .scalar-app .response-section-content-body.diclosure-panel,
        .scalar-app .response-section-content-body .diclosure-panel {
          max-height: 70vh !important;
          overflow-y: auto !important;
          min-height: 0 !important;
        }
        .scalar-app .response-section-content-body .body-raw,
        .scalar-app .response-section-content-body .body-raw-scroller {
          max-height: 70vh !important;
          min-height: 0 !important;
          overflow-y: auto !important;
        }
        .scalar-app .response-section-content-body .cm-editor {
          height: auto !important;
          max-height: 70vh !important;
        }
        .scalar-app .response-section-content-body .cm-scroller {
          max-height: 70vh !important;
          overflow-y: auto !important;
        }
      `"#;

        let config_js = if sources.len() == 1 {
            format!("{{ url: '{}', {} }}", sources[0].url, shared_opts)
        } else {
            let sources_js: Vec<String> = sources
                .iter()
                .map(|s| {
                    format!(
                        "{{ title: '{}', slug: '{}', url: '{}' }}",
                        s.title, s.slug, s.url
                    )
                })
                .collect();
            format!(
                "{{ sources: [{}], {} }}",
                sources_js.join(", "),
                shared_opts
            )
        };

        let html = format!(
            r#"<!doctype html>
<html>
<head>
  <title>Lemma API</title>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
</head>
<body>
  <div id="app"></div>
  <script src="/scalar.js"></script>
  <script>
    Scalar.createApiReference('#app', {config_js})
  </script>
</body>
</html>"#
        );

        Html(html)
    }

    /// Serve the vendored Scalar API reference JavaScript bundle.
    /// Embedded at compile time so the server has zero external dependencies.
    async fn scalar_js() -> impl IntoResponse {
        static SCALAR_JS: &str = include_str!("../vendor/scalar-api-reference.js");

        (
            [(
                axum::http::header::CONTENT_TYPE,
                "application/javascript; charset=utf-8",
            )],
            SCALAR_JS,
        )
    }

    // -----------------------------------------------------------------------
    // Doc path (wildcard): GET = show, POST = evaluate
    // -----------------------------------------------------------------------

    fn data_values_for_run(
        data: std::collections::HashMap<String, lemma::RunDataValue>,
    ) -> Result<std::collections::HashMap<String, String>, (StatusCode, Json<ErrorResponse>)> {
        data.into_iter()
            .map(|(key, value)| {
                let string_value = match value {
                    lemma::RunDataValue::String(s) => s,
                    lemma::RunDataValue::Boolean(b) => b.to_string(),
                    lemma::RunDataValue::MeasureMap(map) | lemma::RunDataValue::RatioMap(map) => {
                        if map.len() == 1 {
                            let (unit, magnitude) = map.into_iter().next().expect("BUG: map len checked");
                            format!("{magnitude} {unit}")
                        } else {
                            return Err((
                                StatusCode::BAD_REQUEST,
                                Json(ErrorResponse {
                                    error: format!(
                                        "data field '{key}' uses a multi-key unit map; pass a convenience string instead"
                                    ),
                                }),
                            ));
                        }
                    }
                };
                Ok((key, string_value))
            })
            .collect()
    }

    async fn spec_get_show(
        State(state): State<AppState>,
        Path(path): Path<String>,
        headers: HeaderMap,
    ) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
        let raw_path = parse_spec_path(&path);
        let effective = accept_datetime_from_headers(&headers)?;
        let engine = engine_read(&state).await;

        let spec_name = lemma::parse_spec_set_id(&raw_path).map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;

        catch_engine_panic(AssertUnwindSafe(
            || -> Result<_, (StatusCode, Json<ErrorResponse>)> {
                let show = engine
                    .show(None, &spec_name, Some(&effective))
                    .map_err(|e| {
                        (
                            lemma_error_to_status(&e),
                            Json(ErrorResponse {
                                error: e.to_string(),
                            }),
                        )
                    })?;

                let effective_from = show.effective_from.clone();

                let mut response = Json(lemma::api::Show::from(&show)).into_response();
                let headers_mut = response.headers_mut();
                for (k, v) in spec_response_headers(effective_from.as_ref()) {
                    headers_mut.insert(k, v);
                }
                Ok(response)
            },
        ))?
    }

    fn parse_post_evaluate_body(
        headers: &HeaderMap,
        body: &[u8],
    ) -> Result<
        std::collections::HashMap<String, lemma::RunDataValue>,
        (StatusCode, Json<ErrorResponse>),
    > {
        if body.is_empty() {
            return Ok(std::collections::HashMap::new());
        }

        let content_type = headers
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.split(';').next().unwrap_or(s).trim().to_ascii_lowercase())
            .unwrap_or_default();

        let data_values = match content_type.as_str() {
            "application/json" => {
                let map: std::collections::HashMap<String, serde_json::Value> =
                    serde_json::from_slice(body).map_err(|e| {
                        (
                            StatusCode::BAD_REQUEST,
                            Json(ErrorResponse {
                                error: format!("invalid JSON body: {e}"),
                            }),
                        )
                    })?;
                map.into_iter()
                    .filter(|(_, v)| !v.is_null())
                    .map(|(k, v)| {
                        crate::data_json::json_value_to_run_data_value(v).map(|input| (k, input))
                    })
                    .collect::<Result<_, _>>()
                    .map_err(|e| (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: e })))?
            }
            "application/x-www-form-urlencoded" => {
                crate::data_json::form_urlencoded_to_data_values(body)
                    .map_err(|e| (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: e })))?
            }
            "" => {
                return Err((
                    StatusCode::UNSUPPORTED_MEDIA_TYPE,
                    Json(ErrorResponse {
                        error: "Expected request with Content-Type: application/json or application/x-www-form-urlencoded".to_string(),
                    }),
                ));
            }
            other => {
                return Err((
                    StatusCode::UNSUPPORTED_MEDIA_TYPE,
                    Json(ErrorResponse {
                        error: format!(
                            "Unsupported Content-Type '{other}'; expected application/json or application/x-www-form-urlencoded"
                        ),
                    }),
                ));
            }
        };

        Ok(data_values)
    }

    /// `POST /{*path}` — evaluate; path = spec name. `Accept-Datetime` for temporal, `?rules=` to limit. Body = JSON or form data.
    async fn spec_post_evaluate(
        State(state): State<AppState>,
        Path(path): Path<String>,
        Query(q): Query<SpecQuery>,
        headers: HeaderMap,
        body: Bytes,
    ) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
        let raw_path = parse_spec_path(&path);
        let effective = accept_datetime_from_headers(&headers)?;

        let spec_name = lemma::parse_spec_set_id(&raw_path).map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;

        let data_values = parse_post_evaluate_body(&headers, &body)?;

        let data_strings = data_values_for_run(data_values)?;

        let parsed_rules: Option<Vec<String>> = match q.rules.as_deref() {
            None => None,
            Some(rules_query) => {
                let parsed = parse_rule_names(rules_query);
                if parsed.is_empty() {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        Json(ErrorResponse {
                            error: "at least one rule required".to_string(),
                        }),
                    ));
                }
                Some(parsed)
            }
        };

        let include_explanations = want_explain(&state, &headers);
        let engine = Arc::clone(&state.engine);

        // Evaluate on a blocking thread under a read lock so refresh waits.
        // Post-planning evaluation is loop-free/terminating by design; the
        // wall-clock timeout is a boundary safeguard, not an engine mechanism.
        let eval_task = tokio::task::spawn_blocking(move || {
            let engine = engine.blocking_read();
            let response = engine
                .run(
                    None,
                    &spec_name,
                    Some(&effective),
                    data_strings,
                    parsed_rules.as_deref(),
                    include_explanations,
                )
                .map_err(|err| {
                    (
                        lemma_error_to_status(&err),
                        Json(ErrorResponse {
                            error: err.to_string(),
                        }),
                    )
                })?;

            let effective_from = response.spec_effective_from.clone();
            let payload = Formatter.response_json_value(&response, include_explanations);
            Ok((payload, effective_from))
        });

        let (payload, effective_from) =
            match tokio::time::timeout(state.eval_timeout, eval_task).await {
                Err(_elapsed) => {
                    error!(
                        "evaluation timed out after {}s",
                        state.eval_timeout.as_secs()
                    );
                    return Err((
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(ErrorResponse {
                            error: format!(
                                "evaluation timed out after {}s",
                                state.eval_timeout.as_secs()
                            ),
                        }),
                    ));
                }
                // The blocking task panicked (JoinError); same mapping as catch_engine_panic.
                Ok(Err(join_error)) => {
                    error!("engine panic caught: {}", join_error);
                    return Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ErrorResponse {
                            error: "internal engine error".to_string(),
                        }),
                    ));
                }
                Ok(Ok(result)) => result?,
            };

        let mut axum_response = Json(payload).into_response();
        let headers_mut = axum_response.headers_mut();
        for (k, v) in spec_response_headers(effective_from.as_ref()) {
            headers_mut.insert(k, v);
        }
        Ok(axum_response)
    }

    fn want_explain(state: &AppState, headers: &HeaderMap) -> bool {
        state.explain
            && headers
                .get("x-explain")
                .and_then(|v: &axum::http::HeaderValue| v.to_str().ok())
                .map(|s: &str| !s.trim().is_empty())
                .unwrap_or(false)
    }

    /// Map a `Error` to an HTTP status code.
    ///
    /// SpecNotFound → 404; InvalidRequest → 400.
    fn lemma_error_to_status(err: &lemma::Error) -> StatusCode {
        use lemma::RequestErrorKind;
        match err {
            lemma::Error::Request {
                kind: RequestErrorKind::SpecNotFound,
                ..
            } => StatusCode::NOT_FOUND,
            _ => StatusCode::BAD_REQUEST,
        }
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    /// Parse comma-separated rule names from a URL path segment.
    /// Filters out empty strings and the literal `{rules}` placeholder that
    /// Scalar sends when the path parameter is left blank.
    fn parse_rule_names(rules_segment: &str) -> Vec<String> {
        rules_segment
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && s != "{rules}")
            .collect()
    }

    // -----------------------------------------------------------------------
    // File watcher (--watch mode)
    // -----------------------------------------------------------------------

    fn start_file_watcher(
        shared_engine: SharedEngine,
        workdir: PathBuf,
    ) -> anyhow::Result<lemma_cli::workspace::WatchGuard> {
        let watch_dir = workdir.clone();
        let on_change = Arc::new(
            move |watch_result: Result<(), lemma_cli::workspace::WorkspaceDiskError>| {
                match watch_result {
                    Ok(()) => {}
                    Err(error) => {
                        error!("Workspace watch failed: {}", error);
                        return;
                    }
                }
                info!("Detected .lemma file changes, refreshing...");
                let engine_clone = shared_engine.clone();
                let workdir_clone = workdir.clone();

                // Spawn a dedicated OS thread for reloading. The notify
                // callback is synchronous, so we create a fresh tokio
                // runtime on a new thread to run the async reload.
                std::thread::spawn(move || {
                    let runtime = match tokio::runtime::Runtime::new() {
                        Ok(rt) => rt,
                        Err(err) => {
                            error!("Failed to create tokio runtime for reload: {}", err);
                            return;
                        }
                    };

                    runtime.block_on(async {
                        // Build scratch outside the write lock so HTTP readers
                        // are not blocked for the full workspace load.
                        let mut scratch = Engine::new();
                        match lemma_cli::workspace::load_workspace(&mut scratch, &workdir_clone) {
                            Ok(()) => {
                                let mut engine = engine_clone.write().await;
                                *engine = scratch;
                                let workspace_specs = engine
                                    .list()
                                    .into_iter()
                                    .find(|repository_group| repository_group.repository.is_none())
                                    .expect("BUG: default repository must exist after Engine::new")
                                    .specs;
                                let unique_specs: std::collections::BTreeSet<&str> =
                                    workspace_specs.iter().map(|ls| ls.name.as_str()).collect();
                                let spec_count = unique_specs.len();
                                info!("Refreshed engine with {} spec(s)", spec_count);
                            }
                            Err(lemma_cli::workspace::WorkspaceDiskError::EngineLoad(load_err)) => {
                                for err in load_err.iter() {
                                    tracing::error!(
                                        "{}",
                                        crate::error_formatter::format_error(
                                            err,
                                            &load_err.sources
                                        )
                                    );
                                }
                                warn!(
                                    "Refresh failed ({} error(s)); keeping previous engine",
                                    load_err.errors.len()
                                );
                            }
                            Err(err) => {
                                warn!("Refresh failed (keeping previous engine): {}", err);
                            }
                        }
                    });
                });
            },
        );

        let guard = lemma_cli::workspace::watch_lemma_workspace(watch_dir.clone(), on_change)
            .map_err(|error| anyhow::Error::msg(error.to_string()))?;

        info!("Watching {:?} for .lemma file changes", watch_dir);

        Ok(guard)
    }

    /// Cold-load into a scratch engine; swap into `engine` only on success.
    #[cfg(test)]
    fn cold_load_into_live_or_keep(
        engine: &mut Engine,
        workdir: &std::path::Path,
    ) -> anyhow::Result<()> {
        match lemma_cli::workspace::refresh_workspace_via_update(engine, workdir) {
            Ok(()) => Ok(()),
            Err(lemma_cli::workspace::WorkspaceDiskError::EngineLoad(load_err)) => {
                for err in load_err.iter() {
                    tracing::error!(
                        "{}",
                        crate::error_formatter::format_error(err, &load_err.sources)
                    );
                }
                warn!(
                    "Cold load failed ({} error(s)); keeping previous engine",
                    load_err.errors.len()
                );
                anyhow::bail!(
                    "Workspace load failed ({} error(s)); previous engine kept",
                    load_err.errors.len()
                )
            }
            Err(error) => {
                warn!("Cold load failed ({}); keeping previous engine", error);
                Err(anyhow::Error::msg(format!("{error}; previous engine kept")))
            }
        }
    }

    #[cfg(test)]
    mod refresh_tests {
        use super::*;
        use std::fs;

        fn write_lemma(dir: &std::path::Path, name: &str, body: &str) {
            fs::write(dir.join(name), body).expect("write lemma");
        }

        #[test]
        fn broken_file_refresh_keeps_previously_loaded_specs() {
            let root = tempfile::tempdir().expect("tempdir");
            write_lemma(
                root.path(),
                "good.lemma",
                "spec good\ndata v: 1\nrule r: v\n",
            );
            let mut engine = Engine::new();
            lemma_cli::workspace::load_workspace(&mut engine, root.path()).expect("initial load");
            assert!(engine.show(None, "good", None).is_ok());

            write_lemma(root.path(), "bad.lemma", "this is not valid lemma!!!\n");

            let err = cold_load_into_live_or_keep(&mut engine, root.path())
                .expect_err("cold load with bad file must fail");
            assert!(
                err.to_string().contains("previous engine kept"),
                "got: {err}"
            );
            assert!(
                engine.show(None, "good", None).is_ok(),
                "previous good spec must still be served after failed refresh"
            );
        }

        #[test]
        fn failed_refresh_keeps_engine_when_disk_has_planning_errors() {
            let root = tempfile::tempdir().expect("tempdir");
            write_lemma(
                root.path(),
                "good.lemma",
                "spec good\ndata v: 1\nrule r: v\n",
            );
            let mut engine = Engine::new();
            lemma_cli::workspace::load_workspace(&mut engine, root.path()).expect("initial load");

            write_lemma(
                root.path(),
                "broken.lemma",
                "spec broken\nuses missing: nonexistent\nrule r: 1\n",
            );
            let result =
                lemma_cli::workspace::refresh_workspace_via_update(&mut engine, root.path());
            assert!(result.is_err(), "broken dependency must fail refresh");
            assert!(engine.show(None, "good", None).is_ok());

            let err = cold_load_into_live_or_keep(&mut engine, root.path())
                .expect_err("scratch cold load must also fail");
            assert!(err.to_string().contains("previous engine kept"));
            assert!(
                engine.show(None, "good", None).is_ok(),
                "must not wipe engine on scratch failure"
            );
        }

        #[test]
        fn successful_scratch_swap_replaces_engine() {
            let root = tempfile::tempdir().expect("tempdir");
            write_lemma(root.path(), "a.lemma", "spec a\ndata v: 1\nrule r: v\n");
            let mut engine = Engine::new();
            lemma_cli::workspace::load_workspace(&mut engine, root.path()).expect("initial load");

            fs::remove_file(root.path().join("a.lemma")).expect("remove a");
            write_lemma(root.path(), "b.lemma", "spec b\ndata v: 2\nrule r: v\n");
            cold_load_into_live_or_keep(&mut engine, root.path()).expect("scratch swap");
            assert!(engine.show(None, "a", None).is_err());
            assert!(engine.show(None, "b", None).is_ok());
        }
    }
}

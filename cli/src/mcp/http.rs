//! Streamable HTTP transport for Lemma MCP (protocol 2026-07-28).
//!
//! Single endpoint `POST /mcp`. No SSE, no sessions, no GET listen stream.

use super::server::{
    error_response, requested_protocol_version, McpDispatcher, McpError, McpRequest, McpResponse,
    HEADER_MISMATCH, LEGACY_PROTOCOL_VERSION, MAX_REQUEST_BYTES, PROTOCOL_VERSION, SERVER_VERSION,
    UNSUPPORTED_PROTOCOL_VERSION,
};
use axum::{
    body::Bytes,
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use lemma::Engine;
use std::net::SocketAddr;
use std::path::Path;
use tower_http::cors::CorsLayer;
use tracing::{error, info, warn};

/// Listen settings for `lemma mcp --http`.
pub struct McpHttpBind {
    pub host: String,
    pub port: u16,
    pub cors: bool,
}

#[derive(Clone)]
struct HttpState {
    dispatcher: McpDispatcher,
    /// Allowed Origin values when `cors` is false (loopback URLs for this bind).
    allowed_origins: Vec<String>,
    /// When true, any Origin is accepted (and CorsLayer is permissive).
    cors: bool,
}

/// Start Streamable HTTP MCP server. Blocks until SIGINT/SIGTERM (or Ctrl-C).
pub async fn start_http_server(
    engine: Engine,
    config: super::McpConfig,
    workdir: &Path,
    bind: McpHttpBind,
) -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "lemma_mcp=info,tower_http=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    info!(
        "Starting Lemma MCP server v{} (Streamable HTTP)",
        SERVER_VERSION
    );
    info!("Protocol version: {}", PROTOCOL_VERSION);
    if config.write {
        info!("Write mode enabled (--write)");
    } else {
        info!("Read-only mode (default)");
    }

    let request_timeout = config.request_timeout;
    let dispatcher = McpDispatcher::spawn(engine, config, workdir.to_path_buf());

    let allowed_origins = loopback_origins(&bind.host, bind.port);
    let state = HttpState {
        dispatcher,
        allowed_origins,
        cors: bind.cors,
    };

    let router = Router::new()
        .route(
            "/mcp",
            post(mcp_post)
                .get(mcp_method_not_allowed)
                .delete(mcp_method_not_allowed),
        )
        .route("/health", get(health_check))
        .fallback(fallback_404);
    let router = if bind.cors {
        info!("Permissive CORS enabled (--cors): cross-origin browser requests allowed");
        router.layer(CorsLayer::permissive())
    } else {
        router
    };
    let app = router.with_state(state);

    if !matches!(
        bind.host.as_str(),
        "127.0.0.1" | "localhost" | "::1" | "[::1]"
    ) {
        warn!(
            "Binding to non-localhost address {}: the Lemma MCP HTTP server has no \
             built-in authentication or TLS. Deploy behind a reverse proxy that \
             terminates TLS and enforces access control.",
            bind.host
        );
    }

    let addr: SocketAddr = format!("{}:{}", bind.host, bind.port).parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    // Always print on stderr (tracing filter may hide info!); users need the URL.
    eprintln!("MCP endpoint: http://{}/mcp", addr);
    info!("Lemma MCP listening on http://{}/mcp", addr);
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
                    tokio::time::sleep(request_timeout).await;
                    warn!(
                        "drain timeout after {}s, dropping remaining connections",
                        request_timeout.as_secs()
                    );
                }
                Err(_) => std::future::pending::<()>().await,
            }
        } => Ok(())
    };

    serve_result?;
    info!("MCP HTTP server shutting down");
    Ok(())
}

fn loopback_origins(host: &str, port: u16) -> Vec<String> {
    let mut origins = vec![
        format!("http://127.0.0.1:{port}"),
        format!("http://localhost:{port}"),
        format!("http://[::1]:{port}"),
    ];
    // Also allow the exact bind host form when it is a loopback alias.
    if matches!(host, "127.0.0.1" | "localhost" | "::1" | "[::1]") {
        origins.push(format!("http://{host}:{port}"));
    }
    origins.sort();
    origins.dedup();
    origins
}

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

async fn health_check() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "service": "lemma-mcp",
        "version": SERVER_VERSION
    }))
}

async fn mcp_method_not_allowed() -> impl IntoResponse {
    StatusCode::METHOD_NOT_ALLOWED
}

async fn fallback_404() -> impl IntoResponse {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({
            "error": "Not found. MCP endpoint is POST /mcp; health is GET /health."
        })),
    )
}

async fn mcp_post(State(state): State<HttpState>, headers: HeaderMap, body: Bytes) -> Response {
    if let Some(forbidden) = check_origin(&state, &headers) {
        return forbidden;
    }
    if let Some(not_acceptable) = check_accept(&headers) {
        return not_acceptable;
    }
    if body.len() > MAX_REQUEST_BYTES {
        error!("HTTP body exceeds {} bytes, rejected", MAX_REQUEST_BYTES);
        return json_rpc_http(
            StatusCode::BAD_REQUEST,
            error_response(
                None,
                McpError::parse_error(format!("Request body exceeds {MAX_REQUEST_BYTES} bytes")),
            ),
        );
    }

    let request: McpRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            error!("Parse error: {}", e);
            return json_rpc_http(
                StatusCode::BAD_REQUEST,
                error_response(None, McpError::parse_error(format!("Parse error: {e}"))),
            );
        }
    };

    if request.method != "initialize" {
        if let Err(err) = validate_streamable_headers(&headers, &request) {
            return json_rpc_http(
                StatusCode::BAD_REQUEST,
                error_response(request.id.clone(), err),
            );
        }
    }

    let is_notification = request.id.is_none();
    let response = match state.dispatcher.dispatch(request) {
        Ok(r) => r,
        Err(e) => {
            error!("dispatcher error: {e}");
            return json_rpc_http(
                StatusCode::INTERNAL_SERVER_ERROR,
                error_response(
                    None,
                    McpError::internal_error("internal server error".to_string()),
                ),
            );
        }
    };

    match response {
        None => {
            if is_notification {
                StatusCode::ACCEPTED.into_response()
            } else {
                // handle_request only returns None for notifications.
                StatusCode::ACCEPTED.into_response()
            }
        }
        Some(resp) => {
            let status = http_status_for_response(&resp);
            json_rpc_http(status, resp)
        }
    }
}

fn check_origin(state: &HttpState, headers: &HeaderMap) -> Option<Response> {
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok())?;
    if state.cors || state.allowed_origins.iter().any(|o| o == origin) {
        return None;
    }
    Some(
        (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "jsonrpc": "2.0",
                "error": {
                    "code": -32600,
                    "message": format!("Origin not allowed: {origin}")
                }
            })),
        )
            .into_response(),
    )
}

fn check_accept(headers: &HeaderMap) -> Option<Response> {
    match headers.get(header::ACCEPT).and_then(|v| v.to_str().ok()) {
        None => Some(StatusCode::NOT_ACCEPTABLE.into_response()),
        Some(accept) => {
            let accepts_json = accept.split(',').any(|part| {
                let media = part.split(';').next().unwrap_or("").trim();
                media == "application/json" || media == "*/*"
            });
            if accepts_json {
                None
            } else {
                Some(StatusCode::NOT_ACCEPTABLE.into_response())
            }
        }
    }
}

/// Validate MCP-Protocol-Version / Mcp-Method / Mcp-Name against the body.
fn validate_streamable_headers(headers: &HeaderMap, request: &McpRequest) -> Result<(), McpError> {
    let header_version = header_str(headers, "mcp-protocol-version").ok_or_else(|| {
        McpError::header_mismatch("Missing required header MCP-Protocol-Version".to_string())
    })?;

    match requested_protocol_version(&request.params) {
        Some(body_version) if body_version != header_version => {
            return Err(McpError::header_mismatch(format!(
                "Header mismatch: MCP-Protocol-Version header value '{header_version}' does not match body value '{body_version}'"
            )));
        }
        Some(body_version)
            if body_version != PROTOCOL_VERSION && body_version != LEGACY_PROTOCOL_VERSION =>
        {
            return Err(McpError::unsupported_protocol_version(body_version));
        }
        None if header_version != LEGACY_PROTOCOL_VERSION && header_version != PROTOCOL_VERSION => {
            return Err(McpError::unsupported_protocol_version(header_version));
        }
        None if header_version == PROTOCOL_VERSION => {
            // Modern header without body _meta: reject as mismatch / invalid.
            return Err(McpError::header_mismatch(
                "MCP-Protocol-Version is 2026-07-28 but body is missing _meta.io.modelcontextprotocol/protocolVersion"
                    .to_string(),
            ));
        }
        _ => {}
    }

    if header_version != PROTOCOL_VERSION && header_version != LEGACY_PROTOCOL_VERSION {
        return Err(McpError::unsupported_protocol_version(header_version));
    }

    let header_method = header_str(headers, "mcp-method").ok_or_else(|| {
        McpError::header_mismatch("Missing required header Mcp-Method".to_string())
    })?;
    if header_method != request.method {
        return Err(McpError::header_mismatch(format!(
            "Header mismatch: Mcp-Method header value '{header_method}' does not match body value '{}'",
            request.method
        )));
    }

    match request.method.as_str() {
        "tools/call" => {
            let body_name = request
                .params
                .as_ref()
                .and_then(|p| p.get("name"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    McpError::header_mismatch(
                        "tools/call requires params.name for Mcp-Name header validation"
                            .to_string(),
                    )
                })?;
            let header_name = header_str(headers, "mcp-name").ok_or_else(|| {
                McpError::header_mismatch("Missing required header Mcp-Name".to_string())
            })?;
            let decoded = decode_mcp_name(header_name)?;
            if decoded != body_name {
                return Err(McpError::header_mismatch(format!(
                    "Header mismatch: Mcp-Name header value '{header_name}' does not match body value '{body_name}'"
                )));
            }
        }
        "resources/read" => {
            let body_uri = request
                .params
                .as_ref()
                .and_then(|p| p.get("uri"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    McpError::header_mismatch(
                        "resources/read requires params.uri for Mcp-Name header validation"
                            .to_string(),
                    )
                })?;
            let header_name = header_str(headers, "mcp-name").ok_or_else(|| {
                McpError::header_mismatch("Missing required header Mcp-Name".to_string())
            })?;
            let decoded = decode_mcp_name(header_name)?;
            if decoded != body_uri {
                return Err(McpError::header_mismatch(format!(
                    "Header mismatch: Mcp-Name header value '{header_name}' does not match body value '{body_uri}'"
                )));
            }
        }
        _ => {}
    }

    Ok(())
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// Decode Mcp-Name: plain ASCII, or Base64 sentinel `b64:<base64url>`.
fn decode_mcp_name(raw: &str) -> Result<String, McpError> {
    if let Some(encoded) = raw.strip_prefix("b64:") {
        use base64::Engine as _;
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|e| {
                McpError::header_mismatch(format!("Invalid Mcp-Name Base64 sentinel: {e}"))
            })?;
        String::from_utf8(bytes).map_err(|e| {
            McpError::header_mismatch(format!("Mcp-Name Base64 decoded to non-UTF-8: {e}"))
        })
    } else {
        Ok(raw.to_string())
    }
}

fn http_status_for_response(resp: &McpResponse) -> StatusCode {
    match resp.error.as_ref().map(|e| e.code) {
        Some(-32601) => StatusCode::NOT_FOUND,
        Some(HEADER_MISMATCH) | Some(UNSUPPORTED_PROTOCOL_VERSION) => StatusCode::BAD_REQUEST,
        Some(-32700) | Some(-32600) | Some(-32602) => StatusCode::BAD_REQUEST,
        Some(-32603) => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::OK,
    }
}

fn json_rpc_http(status: StatusCode, response: McpResponse) -> Response {
    (status, Json(response)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_origins_include_common_forms() {
        let o = loopback_origins("127.0.0.1", 8013);
        assert!(o.contains(&"http://127.0.0.1:8013".to_string()));
        assert!(o.contains(&"http://localhost:8013".to_string()));
        assert!(o.contains(&"http://[::1]:8013".to_string()));
    }

    #[test]
    fn decode_plain_mcp_name() {
        assert_eq!(decode_mcp_name("list").unwrap(), "list");
    }
}

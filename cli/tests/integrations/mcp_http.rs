//! Streamable HTTP MCP integration tests (`lemma mcp --http`).

use serde_json::json;
use std::net::TcpStream;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

const MCP_HTTP_BASE_PORT: u16 = 19880;
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);

fn wait_for_port(port: u16, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

fn spawn_mcp_http(prefix: &std::path::Path, port: u16, write: bool, cors: bool) -> Child {
    let bin = env!("CARGO_BIN_EXE_lemma");
    let mut cmd = Command::new(bin);
    cmd.arg("mcp")
        .arg("--http")
        .arg("--prefix")
        .arg(prefix)
        .arg("--port")
        .arg(port.to_string())
        .arg("--host")
        .arg("127.0.0.1");
    if write {
        cmd.arg("--write");
    }
    if cors {
        cmd.arg("--cors");
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("Failed to start MCP HTTP server")
}

fn stop_child(mut child: Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .expect("http client")
}

fn mcp_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/mcp")
}

fn response_json(resp: reqwest::blocking::Response) -> (reqwest::StatusCode, serde_json::Value) {
    let status = resp.status();
    let text = resp.text().expect("response body");
    let json: serde_json::Value =
        serde_json::from_str(&text).unwrap_or_else(|_| json!({ "raw": text }));
    (status, json)
}

fn modern_meta() -> serde_json::Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": {
            "name": "lemma-mcp-http-test",
            "version": "0"
        },
        "io.modelcontextprotocol/clientCapabilities": {}
    })
}

fn modern_request(id: u64, method: &str, mut params: serde_json::Value) -> serde_json::Value {
    if let Some(object) = params.as_object_mut() {
        object.entry("_meta").or_insert_with(modern_meta);
    }
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params
    })
}

fn post_mcp(
    port: u16,
    body: &serde_json::Value,
    extra_headers: &[(&str, &str)],
) -> reqwest::blocking::Response {
    let method = body["method"].as_str().unwrap_or("");
    let mut req = client()
        .post(mcp_url(port))
        .header("Accept", "application/json, text/event-stream")
        .header("Content-Type", "application/json")
        .body(serde_json::to_string(body).expect("serialize body"));

    if method != "initialize" {
        req = req
            .header("MCP-Protocol-Version", "2026-07-28")
            .header("Mcp-Method", method);
        if method == "tools/call" {
            let name = body["params"]["name"].as_str().unwrap_or("");
            req = req.header("Mcp-Name", name);
        } else if method == "resources/read" {
            let uri = body["params"]["uri"].as_str().unwrap_or("");
            req = req.header("Mcp-Name", uri);
        }
    }

    for (k, v) in extra_headers {
        req = req.header(*k, *v);
    }
    req.send().expect("POST /mcp")
}

fn write_pricing(dir: &std::path::Path) {
    std::fs::write(
        dir.join("pricing.lemma"),
        "spec pricing\ndata quantity: number\ndata base_price: 10\nrule total: quantity * base_price\n",
    )
    .unwrap();
}

#[test]
fn test_mcp_help_shows_http_flags() {
    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("lemma");
    cmd.args(["mcp", "--help"]);
    cmd.assert()
        .success()
        .stdout(predicates::str::contains("--http"))
        .stdout(predicates::str::contains("--host"))
        .stdout(predicates::str::contains("--port"))
        .stdout(predicates::str::contains("--cors"));
}

#[test]
fn test_mcp_http_stderr_prints_endpoint() {
    let temp = tempfile::tempdir().unwrap();
    write_pricing(temp.path());
    let port = MCP_HTTP_BASE_PORT + 12;
    let bin = env!("CARGO_BIN_EXE_lemma");
    let mut child = Command::new(bin)
        .arg("mcp")
        .arg("--http")
        .arg("--prefix")
        .arg(temp.path())
        .arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(port.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("Failed to start MCP HTTP server");
    assert!(
        wait_for_port(port, STARTUP_TIMEOUT),
        "MCP HTTP did not start"
    );
    let _ = child.kill();
    let output = child.wait_with_output().expect("wait");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&format!("MCP endpoint: http://127.0.0.1:{port}/mcp")),
        "stderr must print MCP endpoint, got:\n{stderr}"
    );
    assert!(
        stderr.contains("Starting MCP HTTP server with"),
        "stderr must say HTTP server started, got:\n{stderr}"
    );
}

#[test]
fn test_mcp_http_health() {
    let temp = tempfile::tempdir().unwrap();
    write_pricing(temp.path());
    let port = MCP_HTTP_BASE_PORT;
    let child = spawn_mcp_http(temp.path(), port, false, false);
    assert!(
        wait_for_port(port, STARTUP_TIMEOUT),
        "MCP HTTP did not start"
    );

    let resp = client()
        .get(format!("http://127.0.0.1:{port}/health"))
        .send()
        .expect("GET /health");
    let (status, body) = response_json(resp);
    stop_child(child);

    assert!(status.is_success(), "health status {status}");
    assert_eq!(body["status"], "ok");
    assert_eq!(body["service"], "lemma-mcp");
    assert!(body["version"]
        .as_str()
        .is_some_and(|v: &str| !v.is_empty()));
}

#[test]
fn test_mcp_http_get_delete_method_not_allowed() {
    let temp = tempfile::tempdir().unwrap();
    write_pricing(temp.path());
    let port = MCP_HTTP_BASE_PORT + 1;
    let child = spawn_mcp_http(temp.path(), port, false, false);
    assert!(
        wait_for_port(port, STARTUP_TIMEOUT),
        "MCP HTTP did not start"
    );

    let get = client().get(mcp_url(port)).send().unwrap();
    let delete = client().delete(mcp_url(port)).send().unwrap();
    stop_child(child);

    assert_eq!(get.status(), reqwest::StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(delete.status(), reqwest::StatusCode::METHOD_NOT_ALLOWED);
}

#[test]
fn test_mcp_http_tools_list_and_call() {
    let temp = tempfile::tempdir().unwrap();
    write_pricing(temp.path());
    let port = MCP_HTTP_BASE_PORT + 2;
    let child = spawn_mcp_http(temp.path(), port, false, false);
    assert!(
        wait_for_port(port, STARTUP_TIMEOUT),
        "MCP HTTP did not start"
    );

    let list_body = modern_request(1, "tools/list", json!({}));
    let (list_status, list_json) = response_json(post_mcp(port, &list_body, &[]));
    assert_eq!(list_status, reqwest::StatusCode::OK);
    assert!(list_json["error"].is_null() || list_json.get("error").is_none());
    let tools = list_json["result"]["tools"]
        .as_array()
        .expect("tools array");
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(names.contains(&"run"), "tools: {names:?}");
    assert!(names.contains(&"list"), "tools: {names:?}");

    let call_body = modern_request(
        2,
        "tools/call",
        json!({
            "name": "list",
            "arguments": {}
        }),
    );
    let (call_status, call_json) = response_json(post_mcp(port, &call_body, &[]));
    assert_eq!(call_status, reqwest::StatusCode::OK);
    assert!(call_json["error"].is_null() || call_json.get("error").is_none());
    let text = call_json["result"]["content"][0]["text"]
        .as_str()
        .expect("tool text");
    assert!(text.contains("pricing"), "list text: {text}");

    stop_child(child);
}

#[test]
fn test_mcp_http_notification_returns_202() {
    let temp = tempfile::tempdir().unwrap();
    write_pricing(temp.path());
    let port = MCP_HTTP_BASE_PORT + 3;
    let child = spawn_mcp_http(temp.path(), port, false, false);
    assert!(
        wait_for_port(port, STARTUP_TIMEOUT),
        "MCP HTTP did not start"
    );

    let init = json!({
        "jsonrpc": "2.0",
        "id": 0,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": { "name": "lemma-mcp-http-test", "version": "0" }
        }
    });
    let (init_status, _) = response_json(post_mcp(port, &init, &[]));
    assert_eq!(init_status, reqwest::StatusCode::OK);

    let notification = json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized",
        "params": {}
    });
    let resp = client()
        .post(mcp_url(port))
        .header("Accept", "application/json, text/event-stream")
        .header("Content-Type", "application/json")
        .header("MCP-Protocol-Version", "2025-11-25")
        .header("Mcp-Method", "notifications/initialized")
        .body(serde_json::to_string(&notification).unwrap())
        .send()
        .unwrap();
    let status = resp.status();
    let body = resp.text().unwrap();
    stop_child(child);

    assert_eq!(status, reqwest::StatusCode::ACCEPTED);
    assert!(body.is_empty(), "202 Accepted must have empty body");
}

#[test]
fn test_mcp_http_bad_origin_forbidden() {
    let temp = tempfile::tempdir().unwrap();
    write_pricing(temp.path());
    let port = MCP_HTTP_BASE_PORT + 4;
    let child = spawn_mcp_http(temp.path(), port, false, false);
    assert!(
        wait_for_port(port, STARTUP_TIMEOUT),
        "MCP HTTP did not start"
    );

    let body = modern_request(1, "tools/list", json!({}));
    let resp = post_mcp(port, &body, &[("Origin", "https://evil.example")]);
    stop_child(child);

    assert_eq!(resp.status(), reqwest::StatusCode::FORBIDDEN);
}

#[test]
fn test_mcp_http_no_origin_ok() {
    let temp = tempfile::tempdir().unwrap();
    write_pricing(temp.path());
    let port = MCP_HTTP_BASE_PORT + 5;
    let child = spawn_mcp_http(temp.path(), port, false, false);
    assert!(
        wait_for_port(port, STARTUP_TIMEOUT),
        "MCP HTTP did not start"
    );

    let body = modern_request(1, "tools/list", json!({}));
    let (status, _) = response_json(post_mcp(port, &body, &[]));
    stop_child(child);

    assert_eq!(status, reqwest::StatusCode::OK);
}

#[test]
fn test_mcp_http_header_mismatch() {
    let temp = tempfile::tempdir().unwrap();
    write_pricing(temp.path());
    let port = MCP_HTTP_BASE_PORT + 6;
    let child = spawn_mcp_http(temp.path(), port, false, false);
    assert!(
        wait_for_port(port, STARTUP_TIMEOUT),
        "MCP HTTP did not start"
    );

    let body = modern_request(1, "tools/list", json!({}));
    let resp = client()
        .post(mcp_url(port))
        .header("Accept", "application/json, text/event-stream")
        .header("Content-Type", "application/json")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "tools/call")
        .body(serde_json::to_string(&body).unwrap())
        .send()
        .unwrap();
    let (status, json) = response_json(resp);
    stop_child(child);

    assert_eq!(status, reqwest::StatusCode::BAD_REQUEST);
    assert_eq!(json["error"]["code"], -32020);
}

#[test]
fn test_mcp_http_missing_protocol_version_header() {
    let temp = tempfile::tempdir().unwrap();
    write_pricing(temp.path());
    let port = MCP_HTTP_BASE_PORT + 7;
    let child = spawn_mcp_http(temp.path(), port, false, false);
    assert!(
        wait_for_port(port, STARTUP_TIMEOUT),
        "MCP HTTP did not start"
    );

    let body = modern_request(1, "tools/list", json!({}));
    let resp = client()
        .post(mcp_url(port))
        .header("Accept", "application/json, text/event-stream")
        .header("Content-Type", "application/json")
        .header("Mcp-Method", "tools/list")
        .body(serde_json::to_string(&body).unwrap())
        .send()
        .unwrap();
    let (status, json) = response_json(resp);
    stop_child(child);

    assert_eq!(status, reqwest::StatusCode::BAD_REQUEST);
    assert_eq!(json["error"]["code"], -32020);
}

#[test]
fn test_mcp_http_legacy_initialize_then_tools_list() {
    let temp = tempfile::tempdir().unwrap();
    write_pricing(temp.path());
    let port = MCP_HTTP_BASE_PORT + 8;
    let child = spawn_mcp_http(temp.path(), port, false, false);
    assert!(
        wait_for_port(port, STARTUP_TIMEOUT),
        "MCP HTTP did not start"
    );

    let init = json!({
        "jsonrpc": "2.0",
        "id": 0,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": { "name": "lemma-mcp-http-test", "version": "0" }
        }
    });
    let (init_status, init_json) = response_json(post_mcp(port, &init, &[]));
    assert_eq!(init_status, reqwest::StatusCode::OK);
    assert_eq!(init_json["result"]["protocolVersion"], "2025-11-25");

    let list = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {}
    });
    let list_resp = client()
        .post(mcp_url(port))
        .header("Accept", "application/json, text/event-stream")
        .header("Content-Type", "application/json")
        .header("MCP-Protocol-Version", "2025-11-25")
        .header("Mcp-Method", "tools/list")
        .body(serde_json::to_string(&list).unwrap())
        .send()
        .unwrap();
    let (status, list_json) = response_json(list_resp);
    stop_child(child);

    assert_eq!(status, reqwest::StatusCode::OK);
    assert!(
        list_json["result"]["tools"].is_array(),
        "legacy tools/list: {list_json}"
    );
}

#[test]
fn test_mcp_http_write_required_for_add_spec() {
    let temp = tempfile::tempdir().unwrap();
    write_pricing(temp.path());
    let port = MCP_HTTP_BASE_PORT + 9;
    let child = spawn_mcp_http(temp.path(), port, false, false);
    assert!(
        wait_for_port(port, STARTUP_TIMEOUT),
        "MCP HTTP did not start"
    );

    let call = modern_request(
        1,
        "tools/call",
        json!({
            "name": "add_spec",
            "arguments": {
                "code": "spec other\ndata x: 1\n",
                "attribute": "other.lemma"
            }
        }),
    );
    let (status, json) = response_json(post_mcp(port, &call, &[]));
    stop_child(child);

    assert_eq!(status, reqwest::StatusCode::BAD_REQUEST);
    let error = &json["error"];
    assert!(
        error.is_object(),
        "add_spec without --write must error: {json}"
    );
    assert!(
        error["message"]
            .as_str()
            .unwrap_or("")
            .contains("Write tools are disabled"),
        "got: {}",
        error["message"]
    );
}

#[test]
fn test_mcp_http_write_enables_add_spec() {
    let temp = tempfile::tempdir().unwrap();
    write_pricing(temp.path());
    let port = MCP_HTTP_BASE_PORT + 10;
    let child = spawn_mcp_http(temp.path(), port, true, false);
    assert!(
        wait_for_port(port, STARTUP_TIMEOUT),
        "MCP HTTP did not start"
    );

    let call = modern_request(
        1,
        "tools/call",
        json!({
            "name": "add_spec",
            "arguments": {
                "code": "spec other\ndata x: 1\n",
                "attribute": "other.lemma"
            }
        }),
    );
    let (status, json) = response_json(post_mcp(port, &call, &[]));
    stop_child(child);

    assert_eq!(status, reqwest::StatusCode::OK, "body: {json}");
    assert!(
        json["error"].is_null() || json.get("error").is_none(),
        "add_spec error: {json}"
    );
    let text = json["result"]["content"][0]["text"].as_str().unwrap_or("");
    assert_eq!(text, "Spec added successfully.");
    assert!(
        temp.path().join("other.lemma").exists(),
        "other.lemma must be written"
    );
}

#[test]
fn test_mcp_http_unknown_method_404() {
    let temp = tempfile::tempdir().unwrap();
    write_pricing(temp.path());
    let port = MCP_HTTP_BASE_PORT + 11;
    let child = spawn_mcp_http(temp.path(), port, false, false);
    assert!(
        wait_for_port(port, STARTUP_TIMEOUT),
        "MCP HTTP did not start"
    );

    let body = modern_request(1, "no/such/method", json!({}));
    let (status, json) = response_json(post_mcp(port, &body, &[]));
    stop_child(child);

    assert_eq!(status, reqwest::StatusCode::NOT_FOUND);
    assert_eq!(json["error"]["code"], -32601);
}

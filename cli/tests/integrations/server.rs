use super::stop_child;
use std::net::TcpStream;
use std::time::{Duration, Instant};

const SERVER_TEST_PORT: u16 = 19998;
const SERVER_STARTUP_TIMEOUT: Duration = Duration::from_secs(30);

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

#[test]
fn test_get_spec_route_returns_200() {
    let temp_dir = tempfile::tempdir().unwrap();
    let lemma_file = temp_dir.path().join("single.lemma");
    std::fs::write(
        &lemma_file,
        r#"spec single_spec
data x: number
rule result: x
"#,
    )
    .unwrap();

    let bin = env!("CARGO_BIN_EXE_lemma");
    let mut child = std::process::Command::new(bin)
        .arg("server")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("--port")
        .arg(SERVER_TEST_PORT.to_string())
        .spawn()
        .unwrap();

    let ok = wait_for_port(SERVER_TEST_PORT, SERVER_STARTUP_TIMEOUT);
    if !ok {
        stop_child(&mut child);
        panic!("server did not start within timeout");
    }

    let url = format!("http://127.0.0.1:{}/single_spec", SERVER_TEST_PORT);
    let resp = reqwest::blocking::get(&url).expect("GET request");
    let status = resp.status();
    stop_child(&mut child);

    assert!(
        status.is_success(),
        "GET /single_spec should return 2xx, got {}",
        status
    );
}

#[test]
fn test_post_with_x_explain_header_returns_explanation_when_explain_enabled() {
    let temp_dir = tempfile::tempdir().unwrap();
    let lemma_file = temp_dir.path().join("single.lemma");
    std::fs::write(
        &lemma_file,
        r#"spec single_spec
data x: number
rule result: x
"#,
    )
    .unwrap();

    let port = SERVER_TEST_PORT + 1;
    let bin = env!("CARGO_BIN_EXE_lemma");
    let mut child = std::process::Command::new(bin)
        .arg("server")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("--port")
        .arg(port.to_string())
        .arg("--explain")
        .spawn()
        .unwrap();

    let ok = wait_for_port(port, SERVER_STARTUP_TIMEOUT);
    if !ok {
        stop_child(&mut child);
        panic!("server did not start within timeout");
    }

    let client = reqwest::blocking::Client::new();
    let url = format!("http://127.0.0.1:{}/single_spec", port);
    let resp = client
        .post(&url)
        .header("x-explain", "true")
        .header("Content-Type", "application/json")
        .body(r#"{"x":"42"}"#)
        .send()
        .expect("POST request");
    let status = resp.status();
    let body: serde_json::Value =
        serde_json::from_str(&resp.text().expect("response body")).expect("JSON body");
    stop_child(&mut child);

    assert!(
        status.is_success(),
        "POST with x-explain should return 2xx, got {}",
        status
    );
    let results = body
        .get("results")
        .expect("response should have envelope 'results' key");
    let rule_result = results
        .get("result")
        .expect("results should have 'result' rule");
    assert!(
        rule_result.get("explanation").is_some(),
        "response should include explanation when x-explain header sent: {:?}",
        body
    );
    assert_eq!(rule_result["number"].as_str(), Some("42"));
    assert!(body.get("spec").is_some(), "envelope should include spec");
}

#[test]
fn post_evaluate_accept_datetime_selects_temporal_version() {
    let temp_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        temp_dir.path().join("temporal.lemma"),
        r#"spec pricing 2025-01-01
data base: 10
rule total: base

spec pricing 2026-01-01
data base: 99
rule total: base
"#,
    )
    .unwrap();

    let port = SERVER_TEST_PORT + 2;
    let bin = env!("CARGO_BIN_EXE_lemma");
    let mut child = std::process::Command::new(bin)
        .arg("server")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("--port")
        .arg(port.to_string())
        .spawn()
        .unwrap();

    let ok = wait_for_port(port, SERVER_STARTUP_TIMEOUT);
    if !ok {
        stop_child(&mut child);
        panic!("server did not start within timeout");
    }

    let client = reqwest::blocking::Client::new();
    let url = format!("http://127.0.0.1:{}/pricing", port);

    let post = |accept_dt: &str| -> serde_json::Value {
        let resp = client
            .post(&url)
            .header("Accept-Datetime", accept_dt)
            .header("Content-Type", "application/json")
            .body("{}")
            .send()
            .expect("POST");
        let text = resp.text().expect("body");
        serde_json::from_str(&text).unwrap_or_else(|e| {
            panic!("invalid JSON: {e}; body: {text}");
        })
    };

    let j2025 = post("2025-06-01");
    let j2026 = post("2026-06-01");
    stop_child(&mut child);

    assert_eq!(
        j2025["results"]["total"]["number"].as_str(),
        Some("10"),
        "Accept-Datetime 2025 should resolve pricing v1: {j2025:?}"
    );
    assert_eq!(
        j2026["results"]["total"]["number"].as_str(),
        Some("99"),
        "Accept-Datetime 2026 should resolve pricing v2: {j2026:?}"
    );
}

#[test]
fn post_evaluate_form_urlencoded_body() {
    let temp_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        temp_dir.path().join("single.lemma"),
        r#"spec single_spec
data x: number
rule result: x
"#,
    )
    .unwrap();

    let port = SERVER_TEST_PORT + 4;
    let bin = env!("CARGO_BIN_EXE_lemma");
    let mut child = std::process::Command::new(bin)
        .arg("server")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("--port")
        .arg(port.to_string())
        .spawn()
        .unwrap();

    let ok = wait_for_port(port, SERVER_STARTUP_TIMEOUT);
    if !ok {
        stop_child(&mut child);
        panic!("server did not start within timeout");
    }

    let client = reqwest::blocking::Client::new();
    let url = format!("http://127.0.0.1:{}/single_spec", port);
    let resp = client
        .post(&url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body("x=42")
        .send()
        .expect("POST request");
    let status = resp.status();
    let body: serde_json::Value =
        serde_json::from_str(&resp.text().expect("response body")).expect("JSON body");
    stop_child(&mut child);

    assert!(
        status.is_success(),
        "POST form body should return 2xx, got {status}: {body:?}"
    );
    assert_eq!(body["results"]["result"]["number"].as_str(), Some("42"));
}

/// GET `/` returns the same JSON as [`Engine::specs`]: workspace and dependency
/// repositories with spec version rows (name, optional effective_from/effective_to).
/// Temporal show failures belong on GET `/{spec}`, not on the specs route.
#[test]
fn index_returns_engine_specs_api() {
    let temp_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        temp_dir.path().join("specs.lemma"),
        r#"spec always_available
data x: number
rule result: x

spec future_only 2030-01-01
data y: number
rule result: y
"#,
    )
    .unwrap();

    let port = SERVER_TEST_PORT + 5;
    let bin = env!("CARGO_BIN_EXE_lemma");
    let mut child = std::process::Command::new(bin)
        .arg("server")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("--port")
        .arg(port.to_string())
        .spawn()
        .unwrap();

    let ok = wait_for_port(port, SERVER_STARTUP_TIMEOUT);
    if !ok {
        stop_child(&mut child);
        panic!("server did not start within timeout");
    }

    let url = format!("http://127.0.0.1:{}/", port);
    let resp = reqwest::blocking::get(&url).expect("GET request");
    let status = resp.status();
    let body_text = resp.text().expect("response body");
    stop_child(&mut child);

    assert!(
        status.is_success(),
        "GET / should return 2xx, got {status}: {body_text}"
    );
    let body: serde_json::Value = serde_json::from_str(&body_text)
        .unwrap_or_else(|e| panic!("invalid JSON: {e}; {body_text}"));
    let repositories = body
        .as_array()
        .unwrap_or_else(|| panic!("specs response must be Repository[]: {body}"));

    let workspace = repositories
        .iter()
        .find(|r| r.get("repository").is_none_or(|v| v.is_null()))
        .unwrap_or_else(|| panic!("workspace repository group missing: {body}"));
    let spec_names: Vec<&str> = workspace["specs"]
        .as_array()
        .expect("workspace specs array")
        .iter()
        .filter_map(|row| row["name"].as_str())
        .collect();
    assert!(
        spec_names.contains(&"always_available"),
        "always_available must appear in specs: {body}"
    );
    assert!(
        spec_names.contains(&"future_only"),
        "future_only must appear in specs regardless of effective instant: {body}"
    );
}

/// GET `/{spec}` before a spec's effective_from must fail (temporal show error),
/// not disappear from GET `/`.
#[test]
fn get_show_before_effective_from_returns_error() {
    let temp_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        temp_dir.path().join("specs.lemma"),
        r#"spec future_only 2030-01-01
data y: number
rule result: y
"#,
    )
    .unwrap();

    let port = SERVER_TEST_PORT + 9;
    let bin = env!("CARGO_BIN_EXE_lemma");
    let mut child = std::process::Command::new(bin)
        .arg("server")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("--port")
        .arg(port.to_string())
        .spawn()
        .unwrap();

    let ok = wait_for_port(port, SERVER_STARTUP_TIMEOUT);
    if !ok {
        stop_child(&mut child);
        panic!("server did not start within timeout");
    }

    let client = reqwest::blocking::Client::new();
    let url = format!("http://127.0.0.1:{}/future_only", port);
    let resp = client
        .get(&url)
        .header("Accept-Datetime", "2025-06-01")
        .send()
        .expect("GET request");
    let status = resp.status();
    let body_text = resp.text().expect("response body");
    stop_child(&mut child);

    assert!(
        !status.is_success(),
        "GET /future_only before effective_from must not return 2xx, got {status}: {body_text}"
    );
    let body: serde_json::Value = serde_json::from_str(&body_text)
        .unwrap_or_else(|e| panic!("invalid JSON: {e}; {body_text}"));
    assert!(
        body.get("error").is_some(),
        "show failure must carry error field: {body}"
    );
}

/// By default the server sends no CORS headers: cross-origin browser
/// requests are denied. `--cors` opts in to permissive CORS.
#[test]
fn cors_denied_by_default_and_enabled_with_flag() {
    let temp_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        temp_dir.path().join("single.lemma"),
        r#"spec single_spec
data x: number
rule result: x
"#,
    )
    .unwrap();

    let bin = env!("CARGO_BIN_EXE_lemma");
    let evaluate_case = |port: u16, cors_flag: bool| -> Option<String> {
        let mut cmd = std::process::Command::new(bin);
        cmd.arg("server")
            .arg("--path")
            .arg(temp_dir.path())
            .arg("--port")
            .arg(port.to_string());
        if cors_flag {
            cmd.arg("--cors");
        }
        let mut child = cmd.spawn().unwrap();

        let ok = wait_for_port(port, SERVER_STARTUP_TIMEOUT);
        if !ok {
            stop_child(&mut child);
            panic!("server did not start within timeout");
        }

        let client = reqwest::blocking::Client::new();
        let resp = client
            .get(format!("http://127.0.0.1:{port}/health"))
            .header("Origin", "https://evil.example")
            .send()
            .expect("GET request");
        let allow_origin = resp
            .headers()
            .get("access-control-allow-origin")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        stop_child(&mut child);
        allow_origin
    };

    let default_origin = evaluate_case(SERVER_TEST_PORT + 6, false);
    assert!(
        default_origin.is_none(),
        "default must not send Access-Control-Allow-Origin, got {default_origin:?}"
    );

    let opt_in_origin = evaluate_case(SERVER_TEST_PORT + 7, true);
    assert_eq!(
        opt_in_origin.as_deref(),
        Some("*"),
        "--cors must send permissive Access-Control-Allow-Origin"
    );
}

/// `--eval-timeout 0` makes every evaluation exceed the wall-clock budget;
/// the server must answer 503 with a JSON error instead of hanging.
/// The spec carries a long rule chain so plan+eval take real work and the
/// zero-length budget always elapses before the blocking task finishes.
#[test]
fn evaluation_timeout_returns_503() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut spec = String::from("spec slow_spec\ndata x: number\nrule r0: x + 1\n");
    for i in 1..100 {
        spec.push_str(&format!("rule r{i}: r{} * 2 + {i}\n", i - 1));
    }
    std::fs::write(temp_dir.path().join("slow.lemma"), spec).unwrap();

    let port = SERVER_TEST_PORT + 8;
    let bin = env!("CARGO_BIN_EXE_lemma");
    let mut child = std::process::Command::new(bin)
        .arg("server")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("--port")
        .arg(port.to_string())
        .arg("--eval-timeout")
        .arg("0")
        .spawn()
        .unwrap();

    let ok = wait_for_port(port, SERVER_STARTUP_TIMEOUT);
    if !ok {
        stop_child(&mut child);
        panic!("server did not start within timeout");
    }

    let client = reqwest::blocking::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{port}/slow_spec"))
        .header("Content-Type", "application/json")
        .body(r#"{"x":"42"}"#)
        .send()
        .expect("POST request");
    let status = resp.status();
    let body: serde_json::Value =
        serde_json::from_str(&resp.text().expect("response body")).expect("JSON body");
    stop_child(&mut child);

    assert_eq!(
        status.as_u16(),
        503,
        "zero timeout must yield 503, got {status}: {body:?}"
    );
    assert!(
        body["error"]
            .as_str()
            .expect("error message present")
            .contains("timed out"),
        "error must mention timeout: {body:?}"
    );
}

/// Parse the `YYYY-MM-DD` prefix of an ISO-8601 date string API field.
/// Temporal fields in the public API are always ISO strings, never objects
/// (see `engine/tests/temporal_api_shape.rs`).
fn json_date_ymd(v: &serde_json::Value) -> Option<(i64, u64, u64)> {
    let s = v.as_str()?;
    let (year, rest) = s.split_once('-')?;
    let (month, day) = rest.split_once('-')?;
    Some((year.parse().ok()?, month.parse().ok()?, day.parse().ok()?))
}

/// GET `/{spec}` with `Accept-Datetime` must expose only the shown version's half-open
/// `[effective_from, effective_to)` window; the version list belongs to `specs`.
#[test]
fn get_show_exposes_only_the_shown_version_window() {
    let temp_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        temp_dir.path().join("temporal.lemma"),
        r#"spec pricing 2025-01-01
data base: 10
rule total: base

spec pricing 2026-01-01
data base: 99
rule total: base
"#,
    )
    .unwrap();

    let port = SERVER_TEST_PORT + 3;
    let bin = env!("CARGO_BIN_EXE_lemma");
    let mut child = std::process::Command::new(bin)
        .arg("server")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("--port")
        .arg(port.to_string())
        .spawn()
        .unwrap();

    let ok = wait_for_port(port, SERVER_STARTUP_TIMEOUT);
    if !ok {
        stop_child(&mut child);
        panic!("server did not start within timeout");
    }

    let url = format!("http://127.0.0.1:{}/pricing", port);
    let resp = reqwest::blocking::Client::new()
        .get(&url)
        .header("Accept-Datetime", "2025-06-01")
        .send()
        .expect("GET request");
    let status = resp.status();
    let body_text = resp.text().expect("response body");
    stop_child(&mut child);

    assert!(
        status.is_success(),
        "GET /pricing should return 2xx, got {status}: {body_text}"
    );
    let body: serde_json::Value = serde_json::from_str(&body_text)
        .unwrap_or_else(|e| panic!("invalid JSON: {e}; {body_text}"));

    assert_eq!(
        body["title"].as_str(),
        Some("pricing"),
        "GET show body must be Show: {body}"
    );
    assert!(
        body.get("spec_set_id").is_none(),
        "GET show body must not include spec_set_id: {body}"
    );

    assert_eq!(
        json_date_ymd(&body["x-lemma-effective-from"]),
        Some((2025, 1, 1)),
        "shown version effective_from: {body}"
    );
    assert_eq!(
        json_date_ymd(&body["x-lemma-effective-to"]),
        Some((2026, 1, 1)),
        "shown version effective_to equals next version's effective_from: {body}"
    );
    assert!(
        body.get("x-lemma-versions").is_none(),
        "versions belong to specs, not show: {body}"
    );
}

/// POST evaluate must report the resolved spec version's declared temporal window,
/// both in the JSON body (`spec_effective_from`/`spec_effective_to`) and as the
/// Memento `Memento-Datetime` response header (RFC 7089), scoped to whichever
/// version `?effective=` (or now) resolved to — not the request's own instant.
#[test]
fn post_evaluate_reports_resolved_spec_version_window_in_body_and_memento_header() {
    let temp_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        temp_dir.path().join("temporal.lemma"),
        r#"spec pricing 2025-01-01
data base: 10
rule total: base

spec pricing 2026-01-01
data base: 99
rule total: base
"#,
    )
    .unwrap();

    let port = SERVER_TEST_PORT + 12;
    let bin = env!("CARGO_BIN_EXE_lemma");
    let mut child = std::process::Command::new(bin)
        .arg("server")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("--port")
        .arg(port.to_string())
        .spawn()
        .unwrap();

    let ok = wait_for_port(port, SERVER_STARTUP_TIMEOUT);
    if !ok {
        stop_child(&mut child);
        panic!("server did not start within timeout");
    }

    let client = reqwest::blocking::Client::new();
    let url = format!("http://127.0.0.1:{}/pricing", port);
    let resp = client
        .post(&url)
        .header("Accept-Datetime", "2025-06-01")
        .send()
        .expect("POST request");
    let status = resp.status();
    let memento_header = resp
        .headers()
        .get("memento-datetime")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let body_text = resp.text().expect("response body");
    stop_child(&mut child);

    assert!(
        status.is_success(),
        "POST /pricing should return 2xx, got {status}: {body_text}"
    );
    let body: serde_json::Value = serde_json::from_str(&body_text)
        .unwrap_or_else(|e| panic!("invalid JSON: {e}; {body_text}"));

    assert_eq!(
        json_date_ymd(&body["spec_effective_from"]),
        Some((2025, 1, 1)),
        "resolved spec version's effective_from (2025 slice, not the 2026 successor): {body}"
    );
    assert_eq!(
        json_date_ymd(&body["spec_effective_to"]),
        Some((2026, 1, 1)),
        "resolved spec version's effective_to equals the next version's effective_from: {body}"
    );

    let memento = memento_header.expect("Memento-Datetime header must be present");
    assert!(
        memento.starts_with("2025-01-01"),
        "Memento-Datetime must reflect the resolved version's effective_from, got '{memento}'"
    );
}

#[test]
fn post_evaluate_without_explanations_exposes_rule_missing_data() {
    let temp_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        temp_dir.path().join("suggest.lemma"),
        r#"spec suggest_demo
data n: number -> suggest 42
rule r: n
"#,
    )
    .unwrap();

    let port = SERVER_TEST_PORT + 10;
    let bin = env!("CARGO_BIN_EXE_lemma");
    let mut child = std::process::Command::new(bin)
        .arg("server")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("--port")
        .arg(port.to_string())
        .spawn()
        .unwrap();

    let ok = wait_for_port(port, SERVER_STARTUP_TIMEOUT);
    if !ok {
        stop_child(&mut child);
        panic!("server did not start within timeout");
    }

    let client = reqwest::blocking::Client::new();
    let url = format!("http://127.0.0.1:{}/suggest_demo", port);
    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .body("{}")
        .send()
        .expect("POST request");
    let status = resp.status();
    let body: serde_json::Value =
        serde_json::from_str(&resp.text().expect("response body")).expect("JSON body");
    stop_child(&mut child);

    assert!(
        status.is_success(),
        "POST without explanations should return 2xx, got {status}: {body}"
    );
    assert!(
        body.get("data").is_none(),
        "evaluate body must not include top-level data: {body}"
    );
    let rule = body["results"]["r"].as_object().expect("rule r in results");
    assert!(
        rule.get("explanation").is_none(),
        "explanation must stay omitted without x-explain: {rule:?}"
    );
    let missing = rule["missing_data"]
        .as_array()
        .expect("results.r.missing_data");
    assert!(
        missing.iter().any(|v| v.as_str() == Some("n")),
        "unbound n must appear in missing_data: {missing:?}"
    );
}

#[test]
fn post_evaluate_incomplete_rule_exposes_results_missing_data() {
    let temp_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        temp_dir.path().join("incomplete.lemma"),
        r#"spec incomplete
data a: number
data b: number
rule main: a + b
"#,
    )
    .unwrap();

    let port = SERVER_TEST_PORT + 11;
    let bin = env!("CARGO_BIN_EXE_lemma");
    let mut child = std::process::Command::new(bin)
        .arg("server")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("--port")
        .arg(port.to_string())
        .spawn()
        .unwrap();

    if !wait_for_port(port, SERVER_STARTUP_TIMEOUT) {
        stop_child(&mut child);
        panic!("server did not start within timeout");
    }

    let client = reqwest::blocking::Client::new();
    let url = format!("http://127.0.0.1:{}/incomplete", port);
    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .body("{}")
        .send()
        .expect("POST request");
    let status = resp.status();
    let body: serde_json::Value =
        serde_json::from_str(&resp.text().expect("response body")).expect("JSON body");
    stop_child(&mut child);

    assert!(
        status.is_success(),
        "POST should return 2xx, got {status}: {body}"
    );
    let missing = body["results"]["main"]["missing_data"]
        .as_array()
        .expect("results.main.missing_data must be an array");
    let keys: Vec<&str> = missing.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(
        keys,
        ["a", "b"],
        "smoke: missing_data must list unbound inputs: {body}"
    );
}

#[cfg(unix)]
#[test]
fn sigterm_exits_zero() {
    let temp_dir = tempfile::tempdir().unwrap();
    let lemma_file = temp_dir.path().join("single.lemma");
    std::fs::write(
        &lemma_file,
        r#"spec single_spec
data x: number
rule result: x
"#,
    )
    .unwrap();

    let port = SERVER_TEST_PORT + 13;
    let bin = env!("CARGO_BIN_EXE_lemma");
    let mut child = std::process::Command::new(bin)
        .arg("server")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("--port")
        .arg(port.to_string())
        .spawn()
        .unwrap();

    let ok = wait_for_port(port, SERVER_STARTUP_TIMEOUT);
    if !ok {
        stop_child(&mut child);
        panic!("server did not start within timeout");
    }

    let pid = child.id().to_string();
    let kill_status = std::process::Command::new("kill")
        .args(["-TERM", &pid])
        .status()
        .expect("kill -TERM");
    assert!(
        kill_status.success(),
        "kill -TERM must succeed, got {kill_status}"
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                assert!(status.success(), "SIGTERM must exit 0, got {status}");
                return;
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    stop_child(&mut child);
                    panic!("server did not exit within deadline after SIGTERM");
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(err) => {
                stop_child(&mut child);
                panic!("try_wait failed: {err}");
            }
        }
    }
}

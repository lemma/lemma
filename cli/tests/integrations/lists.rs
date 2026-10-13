//! CLI JSON evaluation-data boundary for lists. Engine tests never use JSON.

use super::stop_child;
use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use serde_json::json;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tempfile::TempDir;

const SERVER_PORT: u16 = 20110;

fn payroll_spec() -> &'static str {
    r#"
spec employee
data name: text
data salary: measure
  -> unit eur: 1.00

spec payroll
uses staff: employee list
  -> with name: "Ada", salary: 9000 eur

rule n:
  staff -> count

rule first_name:
  (staff -> first).name

rule lead:
  staff -> first

rule everyone:
  staff
"#
}

fn fares_spec() -> &'static str {
    r#"
spec fares_card
data money: measure
  -> unit eur: 1.00
data fares: money list
  -> with 89 eur
rule total:
  fares -> sum
data express: boolean
rule fast:
  express
"#
}

fn part_spec() -> &'static str {
    r#"
spec part
data money: measure
  -> unit eur: 1.00
uses components: part list
data quantity: number
data own_cost: money
rule unit_cost:
  own_cost + (components -> sum(components.quantity * components.unit_cost))
"#
}

fn write_spec(dir: &std::path::Path, name: &str, body: &str) {
    std::fs::write(dir.join(name), body).unwrap();
}

fn evaluate_with_stdin(
    path: &std::path::Path,
    spec: &str,
    json: &str,
) -> assert_cmd::assert::Assert {
    cargo_bin_cmd!("lemma")
        .arg("evaluate")
        .arg("--path")
        .arg(path)
        .arg(spec)
        .arg("-")
        .write_stdin(json.to_string())
        .assert()
}

fn make_request(id: u64, method: &str, mut params: serde_json::Value) -> serde_json::Value {
    if let Some(object) = params.as_object_mut() {
        object.entry("_meta").or_insert_with(|| {
            json!({
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientInfo": {
                    "name": "lemma-mcp-test",
                    "version": "0"
                },
                "io.modelcontextprotocol/clientCapabilities": {}
            })
        });
    }
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params
    })
}

fn mcp_session(path: &std::path::Path, messages: &[serde_json::Value]) -> Vec<serde_json::Value> {
    let bin = env!("CARGO_BIN_EXE_lemma");
    let mut cmd = Command::new(bin);
    cmd.arg("mcp")
        .arg("--path")
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = cmd.spawn().expect("mcp");
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let reader = BufReader::new(stdout);
    let mut input = String::new();
    for msg in messages {
        input.push_str(&serde_json::to_string(msg).unwrap());
        input.push('\n');
    }
    stdin.write_all(input.as_bytes()).unwrap();
    drop(stdin);
    let mut responses = Vec::new();
    for line in reader.lines() {
        let line = line.unwrap();
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&line) {
            responses.push(val);
        }
    }
    child.wait().unwrap();
    responses
}

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
fn cli_stdin_replaces_value_list() {
    let temp = TempDir::new().unwrap();
    write_spec(temp.path(), "fares.lemma", fares_spec());
    evaluate_with_stdin(
        temp.path(),
        "fares_card",
        r#"{"fares": ["120 eur", {"eur": "64"}]}"#,
    )
    .success()
    .stdout(predicate::str::contains("184 eur"));
}

#[test]
fn cli_stdin_replaces_instance_list() {
    let temp = TempDir::new().unwrap();
    write_spec(temp.path(), "payroll.lemma", payroll_spec());
    evaluate_with_stdin(
        temp.path(),
        "payroll",
        r#"{"staff": [{"name": "Grace", "salary": "9500 eur"}]}"#,
    )
    .success()
    .stdout(predicate::str::contains("Grace"));
}

#[test]
fn cli_stdin_nested_self_list() {
    let temp = TempDir::new().unwrap();
    write_spec(temp.path(), "part.lemma", part_spec());
    evaluate_with_stdin(
        temp.path(),
        "part",
        r#"{
          "quantity": "1",
          "own_cost": "40 eur",
          "components": [{"quantity": "2", "own_cost": "5 eur"}]
        }"#,
    )
    .success()
    .stdout(predicate::str::contains("50 eur"));
}

#[test]
fn json_number_is_request_error() {
    let temp = TempDir::new().unwrap();
    write_spec(temp.path(), "fares.lemma", fares_spec());
    evaluate_with_stdin(temp.path(), "fares_card", r#"{"fares": [84]}"#)
        .failure()
        .stderr(predicate::str::contains("pass as string"));
}

#[test]
fn json_number_unit_map_magnitude_is_request_error() {
    let temp = TempDir::new().unwrap();
    write_spec(temp.path(), "fares.lemma", fares_spec());
    evaluate_with_stdin(temp.path(), "fares_card", r#"{"fares": [{"eur": 84}]}"#)
        .failure()
        .stderr(predicate::str::contains("pass as string"));
}

#[test]
fn json_boolean_is_accepted() {
    let temp = TempDir::new().unwrap();
    write_spec(temp.path(), "fares.lemma", fares_spec());
    evaluate_with_stdin(temp.path(), "fares_card", r#"{"express": true}"#)
        .success()
        .stdout(predicate::str::contains("true"));
}

#[test]
fn json_null_is_omitted() {
    let temp = TempDir::new().unwrap();
    write_spec(temp.path(), "fares.lemma", fares_spec());
    evaluate_with_stdin(
        temp.path(),
        "fares_card",
        r#"{"fares": null, "express": true}"#,
    )
    .success()
    .stdout(predicate::str::contains("89 eur"));
}

#[test]
fn json_null_list_item_is_request_error() {
    let temp = TempDir::new().unwrap();
    write_spec(temp.path(), "fares.lemma", fares_spec());
    evaluate_with_stdin(
        temp.path(),
        "fares_card",
        r#"{"fares": ["89 eur", null, "120 eur"]}"#,
    )
    .failure()
    .stderr(predicate::str::contains("list item 2 is null"));
}

#[test]
fn json_empty_array_is_explicit_empty_list() {
    let temp = TempDir::new().unwrap();
    write_spec(temp.path(), "fares.lemma", fares_spec());
    evaluate_with_stdin(temp.path(), "fares_card", r#"{"fares": []}"#)
        .success()
        .stdout(predicate::str::contains("0 eur"));
}

#[test]
fn json_keys_differing_only_in_case_are_request_error() {
    let temp = TempDir::new().unwrap();
    write_spec(temp.path(), "fares.lemma", fares_spec());
    evaluate_with_stdin(
        temp.path(),
        "fares_card",
        r#"{"express": true, "Express": false}"#,
    )
    .failure()
    .stderr(predicate::str::contains("express"));
}

#[test]
fn nested_object_as_uses_level_is_request_error() {
    let temp = TempDir::new().unwrap();
    write_spec(
        temp.path(),
        "nested.lemma",
        r#"
spec employee
data name: text
spec payroll
uses ada: employee
rule r: ada.name
"#,
    );
    evaluate_with_stdin(temp.path(), "payroll", r#"{"ada": {"name": "Ada"}}"#)
        .failure()
        .stderr(predicate::str::contains("ada"));
}

#[test]
fn cli_name_equals_value_on_list_key_is_request_error() {
    let temp = TempDir::new().unwrap();
    write_spec(temp.path(), "fares.lemma", fares_spec());

    cargo_bin_cmd!("lemma")
        .arg("evaluate")
        .arg("--path")
        .arg(temp.path())
        .arg("fares_card")
        .arg("fares=89 eur")
        .assert()
        .failure()
        .stderr(predicate::str::contains("fares"));
}

/// POST `body` with `content_type` to `/{spec}` on a fresh server; returns status and JSON body.
fn http_post(
    path: &std::path::Path,
    port: u16,
    spec: &str,
    content_type: &str,
    body: &str,
) -> (reqwest::StatusCode, serde_json::Value) {
    let bin = env!("CARGO_BIN_EXE_lemma");
    let mut child = Command::new(bin)
        .arg("server")
        .arg("--path")
        .arg(path)
        .arg("--port")
        .arg(port.to_string())
        .spawn()
        .unwrap();
    if !wait_for_port(port, Duration::from_secs(30)) {
        stop_child(&mut child);
        panic!("server did not start");
    }
    let resp = reqwest::blocking::Client::new()
        .post(format!("http://127.0.0.1:{port}/{spec}"))
        .header("Content-Type", content_type)
        .body(body.to_string())
        .send()
        .expect("POST");
    let status = resp.status();
    let text = resp.text().unwrap();
    stop_child(&mut child);
    let json = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}: {text}"));
    (status, json)
}

#[test]
fn http_post_list_body() {
    let temp = TempDir::new().unwrap();
    write_spec(temp.path(), "fares.lemma", fares_spec());
    let (status, body) = http_post(
        temp.path(),
        SERVER_PORT,
        "fares_card",
        "application/json",
        r#"{"fares": ["120 eur"]}"#,
    );
    assert!(status.is_success(), "POST list: {status} {body}");
    assert_eq!(body["results"]["total"]["result"], "120 eur", "{body}");
}

#[test]
fn http_post_null_list_item_is_request_error() {
    let temp = TempDir::new().unwrap();
    write_spec(temp.path(), "fares.lemma", fares_spec());
    let (status, body) = http_post(
        temp.path(),
        SERVER_PORT + 4,
        "fares_card",
        "application/json",
        r#"{"fares": ["120 eur", null]}"#,
    );
    assert_eq!(status, reqwest::StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|e| e.contains("list item 2 is null")),
        "{body}"
    );
}

const BOUND_SLOT_SPEC: &str = r#"
spec inner
data slot: number
spec outer
data base: number
uses i: inner
  -> with slot: base
rule r: i.slot
"#;

#[test]
fn json_evaluate_data_for_bound_slot_is_request_error() {
    let temp = TempDir::new().unwrap();
    write_spec(temp.path(), "bound.lemma", BOUND_SLOT_SPEC);
    evaluate_with_stdin(temp.path(), "outer", r#"{"i.slot": "5"}"#)
        .failure()
        .stderr(predicate::str::contains(
            "data 'i.slot' is bound to 'base' by the spec and cannot be supplied",
        ));
}

#[test]
fn http_post_evaluate_data_for_bound_slot_is_request_error() {
    let temp = TempDir::new().unwrap();
    write_spec(temp.path(), "bound.lemma", BOUND_SLOT_SPEC);
    let (status, body) = http_post(
        temp.path(),
        SERVER_PORT + 5,
        "outer",
        "application/json",
        r#"{"i.slot": "5"}"#,
    );
    assert_eq!(status, reqwest::StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|e| e.contains("data 'i.slot' is bound to 'base'")),
        "{body}"
    );
}

#[test]
fn http_post_instance_list_result_references_rows_table() {
    let temp = TempDir::new().unwrap();
    write_spec(temp.path(), "payroll.lemma", payroll_spec());
    let (status, body) = http_post(
        temp.path(),
        SERVER_PORT + 2,
        "payroll",
        "application/json",
        r#"{"staff": [{"name": "Grace", "salary": "9500 eur"}, {"name": "Linus", "salary": "8000 eur"}]}"#,
    );
    assert!(status.is_success(), "POST rows: {status} {body}");
    let lead = &body["results"]["lead"];
    assert_eq!(lead["row"]["instance"], "staff.1", "{body}");
    assert_eq!(lead["row"]["spec"], "employee", "{body}");
    let index = lead["row"]["index"].as_u64().expect("row index") as usize;
    let row = &body["rows"][index];
    assert_eq!(row["data"]["name"]["result"]["text"], "Grace", "{body}");
    assert_eq!(
        body["results"]["everyone"]["list"]
            .as_array()
            .expect("list items")
            .len(),
        2,
        "{body}"
    );
    assert_eq!(body["rows"].as_array().expect("rows").len(), 2, "{body}");
}

#[test]
fn http_form_body_on_list_key_is_request_error() {
    let temp = TempDir::new().unwrap();
    write_spec(temp.path(), "fares.lemma", fares_spec());
    let (status, body) = http_post(
        temp.path(),
        SERVER_PORT + 3,
        "fares_card",
        "application/x-www-form-urlencoded",
        "fares=89+eur",
    );
    assert_eq!(status, reqwest::StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(
        body["error"],
        "Request error: data 'fares': literal overlay is not valid for list data (measure list) (suggestion: fix the overlay for 'fares')"
    );
}

#[test]
fn http_get_query_on_list_key_is_request_error() {
    let temp = TempDir::new().unwrap();
    write_spec(temp.path(), "fares.lemma", fares_spec());
    let port = SERVER_PORT + 1;
    let bin = env!("CARGO_BIN_EXE_lemma");
    let mut child = Command::new(bin)
        .arg("server")
        .arg("--path")
        .arg(temp.path())
        .arg("--port")
        .arg(port.to_string())
        .spawn()
        .unwrap();
    if !wait_for_port(port, Duration::from_secs(30)) {
        stop_child(&mut child);
        panic!("server did not start");
    }
    let url = format!("http://127.0.0.1:{}/fares_card?fares=89%20eur", port);
    let resp = reqwest::blocking::get(&url).expect("GET");
    let status = resp.status();
    let body = resp.text().unwrap();
    stop_child(&mut child);
    assert!(
        !status.is_success(),
        "GET list key must fail, got {status} {body}"
    );
    assert!(body.contains("fares"), "{body}");
}

#[test]
fn mcp_run_with_list_data() {
    let temp = TempDir::new().unwrap();
    write_spec(temp.path(), "payroll.lemma", payroll_spec());
    let responses = mcp_session(
        temp.path(),
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "evaluate",
                    "arguments": {
                        "spec": "payroll",
                        "rules": "first_name",
                        "data": {
                            "staff": [{"name": "Grace", "salary": "9500 eur"}]
                        }
                    }
                }),
            ),
        ],
    );
    assert!(responses.len() >= 2);
    let text = responses[1]["result"]["content"][0]["text"]
        .as_str()
        .expect("evaluate text");
    assert!(text.contains("Grace"), "{text}");
}

use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::json;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

#[test]
fn test_mcp_help_shows_write_flag() {
    let mut cmd = cargo_bin_cmd!("lemma");
    cmd.args(["mcp", "--help"]);
    cmd.assert()
        .success()
        .stdout(predicates::str::contains("--write"));
}

/// Send JSON-RPC messages to the MCP server and collect responses.
/// `prefix: None` runs `lemma mcp` without `--prefix` (defaults to process cwd).
fn mcp_session(
    prefix: Option<&std::path::Path>,
    write: bool,
    messages: &[serde_json::Value],
) -> Vec<serde_json::Value> {
    mcp_session_in_dir(prefix, None, write, messages)
}

fn mcp_session_in_dir(
    prefix: Option<&std::path::Path>,
    current_dir: Option<&std::path::Path>,
    write: bool,
    messages: &[serde_json::Value],
) -> Vec<serde_json::Value> {
    mcp_session_with_env(prefix, current_dir, write, &[], messages)
}

fn mcp_session_with_env(
    prefix: Option<&std::path::Path>,
    current_dir: Option<&std::path::Path>,
    write: bool,
    env: &[(&str, &str)],
    messages: &[serde_json::Value],
) -> Vec<serde_json::Value> {
    let bin = env!("CARGO_BIN_EXE_lemma");
    let mut cmd = Command::new(bin);
    cmd.arg("mcp");
    if let Some(p) = prefix {
        cmd.arg("--prefix").arg(p);
    }
    if let Some(dir) = current_dir {
        cmd.current_dir(dir);
    }
    if write {
        cmd.arg("--write");
    }
    for (key, value) in env {
        cmd.env(key, value);
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let mut child = cmd.spawn().expect("Failed to start MCP server");
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

fn pricing_spec() -> &'static str {
    "spec pricing\ndata quantity: number\ndata base_price: 10\nrule total: quantity * base_price\n"
}

fn write_spec(dir: &std::path::Path, filename: &str, content: &str) {
    std::fs::write(dir.join(filename), content).unwrap();
}

/// Legacy `initialize` without `_meta` (MCP `2025-11-25` and earlier handshake).
fn legacy_initialize(
    id: serde_json::Value,
    protocol_version: &str,
    include_capabilities: bool,
) -> serde_json::Value {
    let mut params = json!({
        "protocolVersion": protocol_version,
        "clientInfo": {
            "name": "lemma-mcp-test",
            "version": "0"
        }
    });
    if include_capabilities {
        params
            .as_object_mut()
            .expect("params object")
            .insert("capabilities".to_string(), json!({}));
    }
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "initialize",
        "params": params
    })
}

fn assert_initialize_result(response: &serde_json::Value, expected_id: &serde_json::Value) {
    assert!(
        response["error"].is_null(),
        "initialize must succeed, got error: {}",
        response["error"]
    );
    assert!(
        response.get("error").is_none() || response["error"].is_null(),
        "initialize must not carry error field with data.requested"
    );
    assert_eq!(&response["id"], expected_id);

    let result = &response["result"];
    assert!(result.is_object(), "initialize must return result object");

    let negotiated = result["protocolVersion"]
        .as_str()
        .expect("InitializeResult.protocolVersion");
    assert_eq!(
        negotiated, "2025-11-25",
        "initialize must negotiate legacy 2025-11-25, got: {negotiated}"
    );

    assert!(
        result["capabilities"]["tools"].is_object(),
        "initialize must advertise tools capability"
    );
    assert!(
        result["capabilities"]["resources"].is_object(),
        "initialize must advertise resources capability"
    );
    assert!(
        result["capabilities"].get("prompts").is_none(),
        "Lemma must not advertise prompts"
    );
    assert!(
        result["capabilities"].get("logging").is_none(),
        "Lemma must not advertise logging"
    );
    assert!(
        result["capabilities"].get("tasks").is_none(),
        "Lemma must not advertise tasks"
    );

    assert_eq!(result["serverInfo"]["name"], "lemma-mcp-server");
    assert!(
        result["serverInfo"]["version"]
            .as_str()
            .is_some_and(|v| !v.is_empty()),
        "serverInfo.version must be non-empty"
    );
}

fn tool_names(tools_list_response: &serde_json::Value) -> Vec<String> {
    tools_list_response["result"]["tools"]
        .as_array()
        .expect("tools/list result.tools")
        .iter()
        .map(|t| t["name"].as_str().expect("tool name").to_string())
        .collect()
}

#[test]
fn test_mcp_list_returns_list() {
    let temp_dir = tempfile::tempdir().unwrap();
    write_spec(temp_dir.path(), "pricing.lemma", pricing_spec());

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(2, "tools/list", json!({})),
            make_request(
                3,
                "tools/call",
                json!({
                    "name": "list",
                    "arguments": {}
                }),
            ),
        ],
    );

    assert!(responses.len() >= 3, "Expected at least 3 responses");

    let list_result = &responses[2]["result"]["content"][0]["text"];
    let text = list_result.as_str().expect("list should return text");
    let list: serde_json::Value = serde_json::from_str(text).expect("list should return list JSON");

    let workspace = list
        .as_array()
        .and_then(|groups| {
            groups
                .iter()
                .find(|g| g["repository"].is_null())
                .map(|g| g["specs"].as_array())
        })
        .flatten()
        .expect("workspace group with specs");
    assert!(
        workspace.iter().any(|row| row["name"] == "pricing"),
        "list should include pricing, got: {text}"
    );
}

#[test]
fn test_mcp_run_with_repository_lemma_units() {
    let temp_dir = tempfile::tempdir().unwrap();

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "show",
                    "arguments": {
                        "repository": "lemma",
                        "spec": "units"
                    }
                }),
            ),
        ],
    );

    assert!(responses.len() >= 2);
    let text = responses[1]["result"]["content"][0]["text"]
        .as_str()
        .expect("show text");
    let show: serde_json::Value = serde_json::from_str(text).expect("Show JSON");
    assert_eq!(show["spec"], "units");
}

#[test]
fn test_mcp_evaluate_includes_reasoning() {
    let temp_dir = tempfile::tempdir().unwrap();
    write_spec(
        temp_dir.path(),
        "discount.lemma",
        "spec discount\ndata quantity: number\nrule rate: 0 percent\n unless quantity >= 10 then 10 percent\n unless quantity >= 50 then 20 percent\n",
    );

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "run",
                    "arguments": {
                        "spec": "discount",
                        "rules": "rate",
                        "data": { "quantity": 25 }
                    }
                }),
            ),
        ],
    );

    assert!(responses.len() >= 2, "Expected at least 2 responses");

    let text = responses[1]["result"]["content"][0]["text"]
        .as_str()
        .expect("run should return text");
    assert!(
        text.contains("rate: 10%") || text.contains("rate: 10 percent"),
        "last-wins unless for quantity=25 must be 10 percent, got: {text}"
    );
    assert!(
        text.contains("quantity >= 10") || text.contains("quantity"),
        "explanation should reference quantity condition, got: {text}"
    );
    assert!(
        text.contains("└─") || text.contains("├─"),
        "run must return ASCII tree, got: {text}"
    );
}

#[test]
fn test_mcp_evaluate_alias_matches_run() {
    let temp_dir = tempfile::tempdir().unwrap();
    write_spec(temp_dir.path(), "pricing.lemma", pricing_spec());

    let run_responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "run",
                    "arguments": {
                        "spec": "pricing",
                        "rules": "total",
                        "data": { "quantity": 3 }
                    }
                }),
            ),
        ],
    );
    let eval_responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "evaluate",
                    "arguments": {
                        "spec": "pricing",
                        "rules": "total",
                        "data": { "quantity": 3 }
                    }
                }),
            ),
        ],
    );

    let run_text = run_responses[1]["result"]["content"][0]["text"]
        .as_str()
        .expect("run text");
    let eval_text = eval_responses[1]["result"]["content"][0]["text"]
        .as_str()
        .expect("evaluate text");
    assert_eq!(run_text, eval_text);
}

#[test]
fn test_mcp_evaluate_reports_missing_data_when_partial() {
    let temp_dir = tempfile::tempdir().unwrap();
    write_spec(
        temp_dir.path(),
        "pricing.lemma",
        "spec pricing\ndata quantity: number\ndata price: number\nrule total: quantity * price\n",
    );

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "run",
                    "arguments": {
                        "spec": "pricing",
                        "rules": "total",
                        "data": { "quantity": 2 }
                    }
                }),
            ),
            make_request(
                3,
                "tools/call",
                json!({
                    "name": "run",
                    "arguments": {
                        "spec": "pricing",
                        "rules": "total",
                        "data": { "quantity": 2, "price": 10 }
                    }
                }),
            ),
        ],
    );

    assert!(responses.len() >= 3);
    let partial = responses[1]["result"]["content"][0]["text"]
        .as_str()
        .expect("partial run text");
    assert!(
        partial.contains("Missing data") && partial.contains("price"),
        "partial run must list price under Missing data, got: {partial}"
    );

    let complete = responses[2]["result"]["content"][0]["text"]
        .as_str()
        .expect("complete run text");
    assert!(
        !complete.contains("Missing data"),
        "complete run must omit Missing data, got: {complete}"
    );
    assert!(
        complete.contains("total: 20"),
        "complete run must show total: 20, got: {complete}"
    );
}

#[test]
fn test_mcp_evaluate_omits_missing_data_when_rule_already_answered_with_veto() {
    let temp_dir = tempfile::tempdir().unwrap();
    write_spec(
        temp_dir.path(),
        "settle.lemma",
        r#"spec settle
data denom: number
data is_smoker: boolean
data is_former_smoker: boolean
data years_since_quit: number
rule loading: 1
  unless is_former_smoker then years_since_quit + 1
  unless is_smoker then 2
rule premium: (1 / denom) * loading
"#,
    );

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "run",
                    "arguments": {
                        "spec": "settle",
                        "rules": "premium",
                        "data": { "denom": 0 }
                    }
                }),
            ),
        ],
    );

    assert!(responses.len() >= 2);
    let text = responses[1]["result"]["content"][0]["text"]
        .as_str()
        .expect("run text");
    assert!(
        text.contains("premium:"),
        "settled run must show rule answer, got: {text}"
    );
    // The `Missing data` section (header line, then keys) must be absent. The
    // reasoning tree still narrates the `loading` sibling, whose own value is a
    // `Missing data: is_smoker` veto: the explain walk visits every operand.
    assert!(
        !text.contains("Missing data\n"),
        "settled Computation must not list Missing data for leftover live keys, got: {text}"
    );
    assert!(
        text.contains("loading: Missing data: is_smoker"),
        "reasoning must narrate the sibling operand after the definitive veto, got: {text}"
    );
}

#[test]
fn test_mcp_run_missing_data_keys_only_help_on_show() {
    let temp_dir = tempfile::tempdir().unwrap();
    write_spec(
        temp_dir.path(),
        "pricing.lemma",
        r#"spec pricing
data quantity: number
data price: number
  -> help "Unit price of the item."
rule total: quantity * price
"#,
    );

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "show",
                    "arguments": { "spec": "pricing" }
                }),
            ),
            make_request(
                3,
                "tools/call",
                json!({
                    "name": "run",
                    "arguments": {
                        "spec": "pricing",
                        "rules": "total",
                        "data": { "quantity": 2 }
                    }
                }),
            ),
        ],
    );

    assert!(responses.len() >= 3);
    let show: serde_json::Value = serde_json::from_str(
        responses[1]["result"]["content"][0]["text"]
            .as_str()
            .expect("show text"),
    )
    .expect("Show JSON");
    assert_eq!(
        show["data"]["price"]["type"]["help"],
        "Unit price of the item."
    );

    let run_text = responses[2]["result"]["content"][0]["text"]
        .as_str()
        .expect("run text");
    assert!(
        run_text.contains("Missing data") && run_text.contains("price"),
        "Missing data keys must include price, got: {run_text}"
    );
}

#[test]
fn test_mcp_read_only_by_default() {
    let temp_dir = tempfile::tempdir().unwrap();

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(2, "tools/list", json!({})),
            make_request(
                3,
                "tools/call",
                json!({
                    "name": "add_spec",
                    "arguments": {
                        "code": "spec test\ndata x: 5\nrule y: x"
                    }
                }),
            ),
        ],
    );

    assert!(responses.len() >= 3, "Expected at least 3 responses");

    // tools/list should NOT include write tools
    let tools = &responses[1]["result"]["tools"];
    let tool_names: Vec<&str> = tools
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(
        !tool_names.contains(&"add_spec"),
        "add_spec should not be listed in read-only mode, got: {:?}",
        tool_names
    );
    assert!(
        !tool_names.contains(&"update_spec"),
        "update_spec should not be listed in read-only mode, got: {:?}",
        tool_names
    );
    assert!(
        !tool_names.contains(&"remove_spec"),
        "remove_spec should not be listed in read-only mode, got: {:?}",
        tool_names
    );
    assert!(
        !tool_names.contains(&"clear"),
        "clear should not be listed in read-only mode, got: {:?}",
        tool_names
    );
    assert!(
        !tool_names.contains(&"install"),
        "install should not be listed in read-only mode, got: {:?}",
        tool_names
    );
    assert!(
        tool_names.contains(&"source"),
        "source should be listed in read-only mode, got: {:?}",
        tool_names
    );

    // Calling add_spec should return an error
    let error = &responses[2]["error"];
    assert!(
        error.is_object(),
        "add_spec should return an error in read-only mode"
    );
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("Write tools are disabled"),
        "Error should mention write tools are disabled, got: {}",
        error["message"]
    );
}

#[test]
fn test_mcp_write_enables_add_spec() {
    let temp_dir = tempfile::tempdir().unwrap();

    let responses = mcp_session(
        Some(temp_dir.path()),
        true,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(2, "tools/list", json!({})),
            make_request(
                3,
                "tools/call",
                json!({
                    "name": "add_spec",
                    "arguments": {
                        "code": "spec test_spec\ndata x: 5\nrule y: x * 2",
                        "attribute": "test_spec.lemma"
                    }
                }),
            ),
        ],
    );

    assert!(responses.len() >= 3, "Expected at least 3 responses");

    // tools/list should include write tools
    let tools = &responses[1]["result"]["tools"];
    let tool_names: Vec<&str> = tools
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(
        tool_names.contains(&"add_spec"),
        "add_spec should be listed with --write, got: {:?}",
        tool_names
    );
    assert!(
        tool_names.contains(&"update_spec"),
        "update_spec should be listed with --write, got: {:?}",
        tool_names
    );
    assert!(
        tool_names.contains(&"remove_spec"),
        "remove_spec should be listed with --write, got: {:?}",
        tool_names
    );
    assert!(
        tool_names.contains(&"clear"),
        "clear should be listed with --write, got: {:?}",
        tool_names
    );
    assert!(
        tool_names.contains(&"install"),
        "install should be listed with --write, got: {:?}",
        tool_names
    );
    assert!(
        tool_names.contains(&"source"),
        "source should be listed (default tool), got: {:?}",
        tool_names
    );

    // add_spec should succeed
    let add_result = &responses[2]["result"]["content"][0]["text"];
    let text = add_result.as_str().expect("add_spec should return text");
    assert_eq!(text, "Spec added successfully.");
}

#[test]
fn test_mcp_source() {
    let temp_dir = tempfile::tempdir().unwrap();
    write_spec(temp_dir.path(), "pricing.lemma", pricing_spec());

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "source",
                    "arguments": {
                        "spec": "pricing"
                    }
                }),
            ),
        ],
    );

    assert!(responses.len() >= 2, "Expected at least 2 responses");

    let source_result = &responses[1]["result"]["content"][0]["text"];
    let text = source_result.as_str().expect("source should return text");

    assert!(
        text.contains("spec pricing"),
        "Should contain spec declaration, got: {text}"
    );
    assert!(
        text.contains("data quantity"),
        "Should contain data declarations, got: {text}"
    );
    assert!(
        text.contains("rule total"),
        "Should contain rule declarations, got: {text}"
    );
}

#[test]
fn test_mcp_source_embedded_lemma_repository() {
    let temp_dir = tempfile::tempdir().unwrap();

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "source",
                    "arguments": {
                        "repository": "lemma"
                    }
                }),
            ),
        ],
    );

    assert!(responses.len() >= 2);
    let text = responses[1]["result"]["content"][0]["text"]
        .as_str()
        .expect("source should return text");
    assert!(
        text.contains("repo lemma")
            && text.contains("spec units")
            && text.contains("trait duration"),
        "Should return formatted embedded stdlib, got: {text}"
    );
}

#[test]
fn test_mcp_source_without_write() {
    let temp_dir = tempfile::tempdir().unwrap();
    write_spec(
        temp_dir.path(),
        "pricing.lemma",
        "spec pricing\ndata x: 5\nrule y: x\n",
    );

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "source",
                    "arguments": {
                        "spec": "pricing"
                    }
                }),
            ),
        ],
    );

    assert!(responses.len() >= 2, "Expected at least 2 responses");

    let text = responses[1]["result"]["content"][0]["text"]
        .as_str()
        .expect("source should return text without --write");
    assert!(
        text.contains("spec pricing"),
        "source without --write should return formatted source, got: {text}"
    );
}

// ── server/discover ─────────────────────────────────────────────────────

#[test]
fn test_mcp_discover_response() {
    let temp_dir = tempfile::tempdir().unwrap();

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[make_request(1, "server/discover", json!({}))],
    );

    assert_eq!(responses.len(), 1);
    let result = &responses[0]["result"];
    assert_eq!(result["supportedVersions"][0], "2026-07-28");
    assert_eq!(
        result["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "lemma-mcp-server"
    );
    assert!(
        result["_meta"]["io.modelcontextprotocol/serverInfo"]["version"]
            .as_str()
            .is_some(),
        "Should include server version"
    );
    assert!(
        result["capabilities"]["tools"].is_object(),
        "Should advertise tools capability"
    );
    assert!(
        result["capabilities"]["resources"].is_object(),
        "Should advertise resources capability"
    );
}

// ── show ──────────────────────────────────────────────────────────

#[test]
fn test_mcp_show_full_spec() {
    let temp_dir = tempfile::tempdir().unwrap();
    write_spec(temp_dir.path(), "pricing.lemma", pricing_spec());

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "show",
                    "arguments": { "spec": "pricing" }
                }),
            ),
        ],
    );

    assert!(responses.len() >= 2);
    let text = responses[1]["result"]["content"][0]["text"]
        .as_str()
        .expect("show should return text");
    let show: serde_json::Value = serde_json::from_str(text).expect("show should return JSON Show");

    assert_eq!(show["spec"], "pricing");
    assert!(
        show["data"]["quantity"].is_object(),
        "Should list quantity data, got: {text}"
    );
    assert!(
        show["data"]["base_price"].is_object(),
        "Should list base_price data, got: {text}"
    );
    assert!(
        show["rules"]["total"].is_object(),
        "Should list total rule, got: {text}"
    );
}

#[test]
fn test_mcp_show_full_spec_rules() {
    let temp_dir = tempfile::tempdir().unwrap();
    write_spec(
        temp_dir.path(),
        "multi.lemma",
        "spec multi\ndata a: number\ndata b: number\nrule sum: a + b\nrule product: a * b\n",
    );

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "show",
                    "arguments": { "spec": "multi" }
                }),
            ),
        ],
    );

    assert!(responses.len() >= 2);
    let text = responses[1]["result"]["content"][0]["text"]
        .as_str()
        .expect("show should return text");
    let show: serde_json::Value = serde_json::from_str(text).expect("show should return JSON Show");

    assert!(
        show["rules"]["sum"].is_object() && show["rules"]["product"].is_object(),
        "Should list all rules, got: {text}"
    );
}

#[test]
fn test_mcp_show_missing_spec() {
    let temp_dir = tempfile::tempdir().unwrap();

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "show",
                    "arguments": { "spec": "nonexistent" }
                }),
            ),
        ],
    );

    assert!(responses.len() >= 2);
    let result = &responses[1]["result"];
    assert_eq!(result["isError"], true, "missing spec must be isError");
    let text = result["content"][0]["text"].as_str().expect("diagnostics");
    let diagnostics: serde_json::Value = serde_json::from_str(text).expect("EngineError JSON");
    assert!(diagnostics.is_array() && !diagnostics.as_array().unwrap().is_empty());
}

#[test]
fn test_mcp_show_empty_spec_name() {
    let temp_dir = tempfile::tempdir().unwrap();

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "show",
                    "arguments": { "spec": "" }
                }),
            ),
        ],
    );

    assert!(responses.len() >= 2);
    let error = &responses[1]["error"];
    assert!(
        error.is_object(),
        "Should return an error for empty spec name"
    );
}

// ── evaluate edge cases ─────────────────────────────────────────────────

#[test]
fn test_mcp_evaluate_all_rules() {
    let temp_dir = tempfile::tempdir().unwrap();
    write_spec(
        temp_dir.path(),
        "multi.lemma",
        "spec multi\ndata x: 3\nrule double: x * 2\nrule triple: x * 3\n",
    );

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "run",
                    "arguments": { "spec": "multi" }
                }),
            ),
        ],
    );

    assert!(responses.len() >= 2);
    let text = responses[1]["result"]["content"][0]["text"]
        .as_str()
        .expect("run should return text");
    assert!(
        text.contains("double: 6"),
        "expected double: 6, got: {text}"
    );
    assert!(
        text.contains("triple: 9"),
        "expected triple: 9, got: {text}"
    );
}

#[test]
fn test_mcp_evaluate_missing_spec() {
    let temp_dir = tempfile::tempdir().unwrap();

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "run",
                    "arguments": { "spec": "nonexistent" }
                }),
            ),
        ],
    );

    assert!(responses.len() >= 2);
    let result = &responses[1]["result"];
    assert_eq!(result["isError"], true, "missing spec must be isError");
    let text = result["content"][0]["text"]
        .as_str()
        .expect("diagnostics text");
    let diagnostics: serde_json::Value = serde_json::from_str(text).expect("EngineError JSON");
    assert!(diagnostics.is_array() && !diagnostics.as_array().unwrap().is_empty());
}

#[test]
fn test_mcp_evaluate_empty_spec_name() {
    let temp_dir = tempfile::tempdir().unwrap();

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "run",
                    "arguments": { "spec": "" }
                }),
            ),
        ],
    );

    assert!(responses.len() >= 2);
    let error = &responses[1]["error"];
    assert!(
        error.is_object(),
        "Should return an error for empty spec name"
    );
    assert!(
        error["message"].as_str().unwrap().contains("empty"),
        "Error should mention empty, got: {}",
        error["message"]
    );
}

#[test]
fn test_mcp_evaluate_veto_result() {
    let temp_dir = tempfile::tempdir().unwrap();
    write_spec(
        temp_dir.path(),
        "vetoed.lemma",
        "spec vetoed\ndata price: -5\nrule validated: price\n unless price < 0 then veto \"Price cannot be negative\"\n",
    );

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "run",
                    "arguments": { "spec": "vetoed" }
                }),
            ),
        ],
    );

    assert!(responses.len() >= 2);
    let text = responses[1]["result"]["content"][0]["text"]
        .as_str()
        .expect("run should return text");
    assert!(
        text.contains("validated:") && text.contains("Price cannot be negative"),
        "veto must appear in formatted tree, got: {text}"
    );
}

#[test]
fn test_mcp_evaluate_with_effective_datetime() {
    let temp_dir = tempfile::tempdir().unwrap();
    write_spec(
        temp_dir.path(),
        "simple.lemma",
        "spec simple\ndata x: 42\nrule y: x\n",
    );

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "run",
                    "arguments": {
                        "spec": "simple",
                        "effective": "2026-01-01"
                    }
                }),
            ),
        ],
    );

    assert!(responses.len() >= 2);
    let text = responses[1]["result"]["content"][0]["text"]
        .as_str()
        .expect("run should return text");
    assert!(
        text.contains("y: 42"),
        "expected y: 42 for effective datetime run, got: {text}"
    );
}

// ── list edge cases ─────────────────────────────────────────────────────

#[test]
fn test_mcp_list_empty_workspace() {
    let temp_dir = tempfile::tempdir().unwrap();

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "list",
                    "arguments": {}
                }),
            ),
        ],
    );

    assert!(responses.len() >= 2);
    let text = responses[1]["result"]["content"][0]["text"]
        .as_str()
        .expect("list should return text");
    let list: serde_json::Value = serde_json::from_str(text).expect("list should return JSON");
    let embedded = list
        .as_array()
        .and_then(|groups| groups.iter().find(|g| g["repository"] == "lemma"))
        .expect("embedded lemma repository group");
    assert!(
        embedded["specs"]
            .as_array()
            .and_then(|specs| specs.first())
            .and_then(|row| row["name"].as_str())
            == Some("units"),
        "embedded stdlib must appear in list, got: {text}"
    );
}

#[test]
fn test_mcp_list_empty_workspace_is_valid_json() {
    let temp_dir = tempfile::tempdir().unwrap();

    let responses = mcp_session(
        Some(temp_dir.path()),
        true,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "list",
                    "arguments": {}
                }),
            ),
        ],
    );

    assert!(responses.len() >= 2);
    let text = responses[1]["result"]["content"][0]["text"]
        .as_str()
        .expect("list should return text");
    let list: serde_json::Value = serde_json::from_str(text).expect("list must be valid JSON");
    assert!(list.is_array(), "list must be JSON array, got: {text}");
    assert!(
        !text.contains("add_spec"),
        "list must not append prose hints, got: {text}"
    );
}

#[test]
fn test_mcp_defaults_prefix_to_cwd() {
    let temp_dir = tempfile::tempdir().unwrap();
    write_spec(temp_dir.path(), "pricing.lemma", pricing_spec());

    let responses = mcp_session_in_dir(
        None,
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "list",
                    "arguments": {}
                }),
            ),
        ],
    );

    assert!(responses.len() >= 2);
    let text = responses[1]["result"]["content"][0]["text"]
        .as_str()
        .expect("list should return text");
    let list: serde_json::Value = serde_json::from_str(text).expect("list should return JSON");
    let workspace = list
        .as_array()
        .and_then(|groups| {
            groups
                .iter()
                .find(|g| g["repository"].is_null())
                .map(|g| g["specs"].as_array())
        })
        .flatten()
        .expect("workspace group");
    assert!(
        workspace.iter().any(|row| row["name"] == "pricing"),
        "workspace specs must load when --prefix is omitted, got: {text}"
    );
}

// ── handshake / protocol ────────────────────────────────────────────────

#[test]
fn test_mcp_legacy_initialize_then_tools_list_read_only() {
    let temp_dir = tempfile::tempdir().unwrap();

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            legacy_initialize(json!(0), "2025-11-25", true),
            json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized"
            }),
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/list",
                "params": {}
            }),
        ],
    );

    assert_eq!(
        responses.len(),
        2,
        "initialize + tools/list only; initialized is silent"
    );
    assert_initialize_result(&responses[0], &json!(0));

    let names = tool_names(&responses[1]);
    for required in ["run", "list", "show"] {
        assert!(
            names.iter().any(|n| n == required),
            "tools/list after initialize must include {required}, got: {names:?}"
        );
    }
    assert!(
        !names.iter().any(|n| n == "add_spec"),
        "read-only must not list add_spec, got: {names:?}"
    );
}

#[test]
fn test_mcp_legacy_initialize_then_tools_list_write() {
    let temp_dir = tempfile::tempdir().unwrap();

    let responses = mcp_session(
        Some(temp_dir.path()),
        true,
        &[
            legacy_initialize(json!(0), "2025-11-25", true),
            json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized"
            }),
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/list",
                "params": {}
            }),
        ],
    );

    assert_eq!(responses.len(), 2);
    assert_initialize_result(&responses[0], &json!(0));

    let names = tool_names(&responses[1]);
    for required in ["run", "list", "show", "add_spec"] {
        assert!(
            names.iter().any(|n| n == required),
            "write tools/list after initialize must include {required}, got: {names:?}"
        );
    }
}

#[test]
fn test_mcp_initialize_protocol_version_2024_11_05() {
    let temp_dir = tempfile::tempdir().unwrap();
    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[legacy_initialize(json!(1), "2024-11-05", true)],
    );
    assert_eq!(responses.len(), 1);
    assert_initialize_result(&responses[0], &json!(1));
}

#[test]
fn test_mcp_initialize_protocol_version_2025_03_26() {
    let temp_dir = tempfile::tempdir().unwrap();
    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[legacy_initialize(json!(1), "2025-03-26", true)],
    );
    assert_eq!(responses.len(), 1);
    assert_initialize_result(&responses[0], &json!(1));
}

#[test]
fn test_mcp_initialize_protocol_version_2025_11_25_echoes() {
    let temp_dir = tempfile::tempdir().unwrap();
    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[legacy_initialize(json!(1), "2025-11-25", true)],
    );
    assert_eq!(responses.len(), 1);
    assert_initialize_result(&responses[0], &json!(1));
    assert_eq!(
        responses[0]["result"]["protocolVersion"], "2025-11-25",
        "when client asks 2025-11-25 and server speaks it, result must echo"
    );
}

#[test]
fn test_mcp_initialize_without_capabilities_field() {
    let temp_dir = tempfile::tempdir().unwrap();
    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[legacy_initialize(json!(1), "2025-11-25", false)],
    );
    assert_eq!(responses.len(), 1);
    assert_initialize_result(&responses[0], &json!(1));
}

#[test]
fn test_mcp_initialize_string_id() {
    let temp_dir = tempfile::tempdir().unwrap();
    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[legacy_initialize(json!("init-1"), "2025-11-25", true)],
    );
    assert_eq!(responses.len(), 1);
    assert_initialize_result(&responses[0], &json!("init-1"));
}

#[test]
fn test_mcp_initialize_with_write_flag_independent() {
    let temp_dir = tempfile::tempdir().unwrap();
    let responses = mcp_session(
        Some(temp_dir.path()),
        true,
        &[legacy_initialize(json!(0), "2025-11-25", true)],
    );
    assert_eq!(responses.len(), 1);
    assert_initialize_result(&responses[0], &json!(0));
}

#[test]
fn test_mcp_initialize_requesting_modern_version_still_legacy_result() {
    let temp_dir = tempfile::tempdir().unwrap();
    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[legacy_initialize(json!(1), "2026-07-28", true)],
    );
    assert_eq!(responses.len(), 1);
    assert_initialize_result(&responses[0], &json!(1));
}

#[test]
fn test_mcp_tools_call_list_without_meta_after_initialize() {
    let temp_dir = tempfile::tempdir().unwrap();
    write_spec(temp_dir.path(), "pricing.lemma", pricing_spec());

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            legacy_initialize(json!(0), "2025-11-25", true),
            json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized"
            }),
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "list",
                    "arguments": {}
                }
            }),
        ],
    );

    assert_eq!(responses.len(), 2);
    assert_initialize_result(&responses[0], &json!(0));
    assert!(
        responses[1]["error"].is_null() || responses[1].get("error").is_none(),
        "tools/call without _meta after initialize must not error: {}",
        responses[1]["error"]
    );
    let text = responses[1]["result"]["content"][0]["text"]
        .as_str()
        .expect("list tool text");
    assert!(
        text.contains("pricing"),
        "list after legacy handshake must return workspace specs, got: {text}"
    );
}

#[test]
fn test_mcp_tools_list_without_initialize_or_meta_errors() {
    let temp_dir = tempfile::tempdir().unwrap();

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {}
        })],
    );

    assert_eq!(responses.len(), 1);
    let error = &responses[0]["error"];
    assert!(
        error.is_object(),
        "tools/list with neither initialize nor _meta must error"
    );
    assert!(
        responses[0].get("result").is_none() || responses[0]["result"].is_null(),
        "must not return a tool catalog before era is chosen"
    );
    if let Some(requested) = error["data"]["requested"].as_str() {
        assert!(
            !requested.is_empty(),
            "must not use empty-string requested as the diagnostic for missing era"
        );
    }
}

#[test]
fn test_mcp_tools_list_with_modern_meta_no_initialize() {
    let temp_dir = tempfile::tempdir().unwrap();

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[make_request(1, "tools/list", json!({}))],
    );

    assert_eq!(responses.len(), 1);
    assert!(
        responses[0]["error"].is_null() || responses[0].get("error").is_none(),
        "modern tools/list with _meta and no initialize must succeed: {}",
        responses[0]["error"]
    );
    let names = tool_names(&responses[0]);
    assert!(
        names.iter().any(|n| n == "run"),
        "modern tools/list must include run, got: {names:?}"
    );
}

// ── error handling ──────────────────────────────────────────────────────

#[test]
fn test_mcp_invalid_jsonrpc_version() {
    let temp_dir = tempfile::tempdir().unwrap();

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[json!({
            "jsonrpc": "1.0",
            "id": 1,
            "method": "server/discover",
            "params": {}
        })],
    );

    assert_eq!(responses.len(), 1);
    let error = &responses[0]["error"];
    assert!(
        error.is_object(),
        "Should return an error for bad JSON-RPC version"
    );
    assert_eq!(error["code"], -32600, "Should be invalid request code");
}

#[test]
fn test_mcp_unknown_method() {
    let temp_dir = tempfile::tempdir().unwrap();

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[make_request(1, "nonexistent/method", json!({}))],
    );

    assert_eq!(responses.len(), 1);
    let error = &responses[0]["error"];
    assert!(
        error.is_object(),
        "Should return an error for unknown method"
    );
    assert_eq!(error["code"], -32601, "Should be method not found code");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("nonexistent/method"),
        "Error should name the unknown method, got: {}",
        error["message"]
    );
}

#[test]
fn test_mcp_malformed_json() {
    let temp_dir = tempfile::tempdir().unwrap();
    let bin = env!("CARGO_BIN_EXE_lemma");
    let mut cmd = Command::new(bin);
    cmd.arg("mcp").current_dir(temp_dir.path());
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let mut child = cmd.spawn().expect("Failed to start MCP server");
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let reader = BufReader::new(stdout);

    stdin.write_all(b"this is not json\n").unwrap();
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

    assert_eq!(responses.len(), 1);
    let error = &responses[0]["error"];
    assert!(error.is_object(), "Should return a parse error");
    assert_eq!(error["code"], -32700, "Should be parse error code");
}

#[test]
fn test_mcp_tools_call_missing_params() {
    let temp_dir = tempfile::tempdir().unwrap();

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call"
            }),
        ],
    );

    assert!(responses.len() >= 2);
    let error = &responses[1]["error"];
    assert!(
        error.is_object(),
        "tools/call without params must be a JSON-RPC error, not a tool result"
    );
    assert!(
        responses[1].get("result").is_none() || responses[1]["result"].is_null(),
        "must not return a tools/call result when params are absent"
    );
}

#[test]
fn test_mcp_tools_call_missing_tool_name() {
    let temp_dir = tempfile::tempdir().unwrap();

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(2, "tools/call", json!({ "arguments": {} })),
        ],
    );

    assert!(responses.len() >= 2);
    let error = &responses[1]["error"];
    assert!(
        error.is_object(),
        "Should return an error for missing tool name"
    );
    assert_eq!(error["code"], -32602, "Should be invalid params code");
}

#[test]
fn test_mcp_tools_call_unknown_tool() {
    let temp_dir = tempfile::tempdir().unwrap();

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "nonexistent_tool",
                    "arguments": {}
                }),
            ),
        ],
    );

    assert!(responses.len() >= 2);
    let error = &responses[1]["error"];
    assert!(
        error.is_object(),
        "unknown tool is a host bug (caught panic)"
    );
    assert_eq!(error["code"], -32603, "caught panic is internal error");
    assert_eq!(
        error["message"].as_str().unwrap(),
        "internal engine error",
        "got: {}",
        error["message"]
    );
}

// ── stdin hardening ─────────────────────────────────────────────────────

/// A stdin line over the 10 MiB cap must be rejected with a JSON-RPC parse
/// error, and the server must stay in sync to serve subsequent requests.
#[test]
fn test_mcp_oversized_line_rejected_then_recovers() {
    let temp_dir = tempfile::tempdir().unwrap();
    let bin = env!("CARGO_BIN_EXE_lemma");
    let mut cmd = Command::new(bin);
    cmd.arg("mcp").current_dir(temp_dir.path());
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let mut child = cmd.spawn().expect("Failed to start MCP server");
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let reader = BufReader::new(stdout);

    let mut input = Vec::with_capacity(11 * 1024 * 1024);
    input.resize(10 * 1024 * 1024 + 1, b'x');
    input.push(b'\n');
    input.extend_from_slice(
        serde_json::to_string(&make_request(1, "server/discover", json!({})))
            .unwrap()
            .as_bytes(),
    );
    input.push(b'\n');
    stdin.write_all(&input).unwrap();
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

    assert_eq!(responses.len(), 2, "expected error + discover response");
    assert_eq!(
        responses[0]["error"]["code"], -32700,
        "oversized line must yield parse error: {}",
        responses[0]
    );
    assert!(
        responses[0]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("exceeds"),
        "error should mention the byte cap: {}",
        responses[0]
    );
    assert!(
        responses[1]["result"].is_object(),
        "server must recover and answer the next request: {}",
        responses[1]
    );
}

/// `--request-timeout 0` makes every request exceed its wall-clock budget;
/// the server must answer with a JSON-RPC internal error instead of hanging.
/// The spec carries a rule chain so handling takes real work and the
/// zero-length budget always elapses before the worker finishes.
#[test]
fn test_mcp_request_timeout_returns_error() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut spec = String::from("spec slow_spec\ndata x: number\nrule r0: x + 1\n");
    for i in 1..100 {
        spec.push_str(&format!("rule r{i}: r{} * 2 + {i}\n", i - 1));
    }
    write_spec(temp_dir.path(), "slow.lemma", &spec);

    let bin = env!("CARGO_BIN_EXE_lemma");
    let mut cmd = Command::new(bin);
    cmd.arg("mcp")
        .arg("--prefix")
        .arg(temp_dir.path())
        .arg("--request-timeout")
        .arg("0");
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let mut child = cmd.spawn().expect("Failed to start MCP server");
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let reader = BufReader::new(stdout);

    let request = make_request(
        1,
        "tools/call",
        json!({
            "name": "run",
            "arguments": { "spec": "slow_spec", "data": { "x": 1 } }
        }),
    );
    let mut input = serde_json::to_string(&request).unwrap();
    input.push('\n');
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

    assert_eq!(responses.len(), 1, "expected exactly one timeout response");
    assert_eq!(responses[0]["id"], 1, "response id must match request");
    assert!(
        responses[0]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("timed out"),
        "error should mention timeout: {}",
        responses[0]
    );
}

// ── add_spec error cases ────────────────────────────────────────────────

#[test]
fn test_mcp_add_spec_empty_code() {
    let temp_dir = tempfile::tempdir().unwrap();

    let responses = mcp_session(
        Some(temp_dir.path()),
        true,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "add_spec",
                    "arguments": { "code": "" }
                }),
            ),
        ],
    );

    assert!(responses.len() >= 2);
    let error = &responses[1]["error"];
    assert!(error.is_object(), "Should return an error for empty code");
    assert!(
        error["message"].as_str().unwrap().contains("empty"),
        "Error should mention empty, got: {}",
        error["message"]
    );
}

#[test]
fn test_mcp_add_spec_invalid_code() {
    let temp_dir = tempfile::tempdir().unwrap();

    let responses = mcp_session(
        Some(temp_dir.path()),
        true,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "add_spec",
                    "arguments": {
                        "code": "this is not valid lemma code !!!",
                        "attribute": "invalid.lemma"
                    }
                }),
            ),
        ],
    );

    assert!(responses.len() >= 2);
    let result = &responses[1]["result"];
    assert_eq!(
        result["isError"], true,
        "Invalid Lemma should return isError tool result, got: {result}"
    );
    let text = result["content"][0]["text"]
        .as_str()
        .expect("diagnostics text");
    let diagnostics: serde_json::Value =
        serde_json::from_str(text).expect("diagnostics should be JSON");
    assert!(
        diagnostics.as_array().is_some_and(|a| !a.is_empty()),
        "Should return at least one diagnostic, got: {text}"
    );
    assert!(
        diagnostics[0]["message"].as_str().is_some(),
        "Diagnostic should include message, got: {text}"
    );
}

// ── tools/list structure ────────────────────────────────────────────────

#[test]
fn test_mcp_tools_list_read_only_tools() {
    let temp_dir = tempfile::tempdir().unwrap();

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(2, "tools/list", json!({})),
        ],
    );

    assert!(responses.len() >= 2);
    let tools = responses[1]["result"]["tools"]
        .as_array()
        .expect("tools should be an array");
    let tool_names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();

    assert!(tool_names.contains(&"run"), "Should list run tool");
    assert!(tool_names.contains(&"list"), "Should list list tool");
    assert!(tool_names.contains(&"show"), "Should list show tool");
    assert!(tool_names.contains(&"source"), "Should list source tool");
    assert!(tool_names.contains(&"check"), "Should list check tool");
    assert!(tool_names.contains(&"guide"), "Should list guide tool");
    assert!(
        tool_names.contains(&"evaluate"),
        "Should list deprecated evaluate alias"
    );
    assert_eq!(
        tool_names.len(),
        7,
        "Read-only mode should have exactly 7 tools, got: {:?}",
        tool_names
    );
}

#[test]
fn test_mcp_remove_spec_and_clear() {
    let temp_dir = tempfile::tempdir().unwrap();

    let responses = mcp_session(
        Some(temp_dir.path()),
        true,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "add_spec",
                    "arguments": {
                        "code": "spec draft\ndata x: number\nrule y: x\n",
                        "attribute": "draft.lemma"
                    }
                }),
            ),
            make_request(3, "tools/call", json!({ "name": "list", "arguments": {} })),
            make_request(
                4,
                "tools/call",
                json!({
                    "name": "remove_spec",
                    "arguments": { "spec": "draft" }
                }),
            ),
            make_request(5, "tools/call", json!({ "name": "list", "arguments": {} })),
            make_request(
                6,
                "tools/call",
                json!({
                    "name": "add_spec",
                    "arguments": {
                        "code": "spec again\ndata z: 1\nrule r: z\n",
                        "attribute": "again.lemma"
                    }
                }),
            ),
            make_request(7, "tools/call", json!({ "name": "clear", "arguments": {} })),
            make_request(8, "tools/call", json!({ "name": "list", "arguments": {} })),
        ],
    );

    assert!(responses.len() >= 8);
    assert_eq!(
        responses[1]["result"]["content"][0]["text"],
        "Spec added successfully."
    );
    let list_after_add = responses[2]["result"]["content"][0]["text"]
        .as_str()
        .expect("list text");
    assert!(
        list_after_add.contains("draft"),
        "draft must appear after add, got: {list_after_add}"
    );

    let remove_text = responses[3]["result"]["content"][0]["text"]
        .as_str()
        .expect("remove text");
    assert!(
        remove_text.contains("removed"),
        "remove must confirm, got: {remove_text}"
    );
    assert!(
        responses[3]["result"].get("isError").is_none()
            || responses[3]["result"]["isError"] != true,
        "remove must succeed"
    );
    // Full session already ran remove_spec; draft.lemma must be gone on disk.
    assert!(
        !temp_dir.path().join("draft.lemma").exists(),
        "remove_spec must delete draft.lemma from disk"
    );

    let list_after_remove = responses[4]["result"]["content"][0]["text"]
        .as_str()
        .expect("list text");
    assert!(
        !list_after_remove.contains("draft"),
        "draft must be gone after remove, got: {list_after_remove}"
    );

    let clear_text = responses[6]["result"]["content"][0]["text"]
        .as_str()
        .expect("clear text");
    assert_eq!(
        clear_text, "Removed all specs.",
        "clear must confirm remove-all without stdlib chatter, got: {clear_text}"
    );
    assert!(
        !temp_dir.path().join("again.lemma").exists(),
        "clear must delete again.lemma from disk"
    );

    let list_after_clear = responses[7]["result"]["content"][0]["text"]
        .as_str()
        .expect("list text");
    assert!(
        !list_after_clear.contains("again"),
        "workspace specs must be gone after clear, got: {list_after_clear}"
    );
}

#[test]
fn test_mcp_remove_spec_rewrites_shared_path_file() {
    let temp_dir = tempfile::tempdir().unwrap();

    let responses = mcp_session(
        Some(temp_dir.path()),
        true,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "add_spec",
                    "arguments": {
                        "code": "spec first\ndata a: 1\nrule x: a\n\nspec second\ndata b: 2\nrule y: b\n",
                        "attribute": "pair.lemma"
                    }
                }),
            ),
            make_request(
                3,
                "tools/call",
                json!({
                    "name": "remove_spec",
                    "arguments": { "spec": "first" }
                }),
            ),
            make_request(4, "tools/call", json!({ "name": "list", "arguments": {} })),
        ],
    );

    assert!(responses.len() >= 4);
    assert!(
        temp_dir.path().join("pair.lemma").exists(),
        "shared Path file must remain after removing one of two specs"
    );
    let on_disk = std::fs::read_to_string(temp_dir.path().join("pair.lemma")).unwrap();
    assert!(
        on_disk.contains("spec second") && !on_disk.contains("spec first"),
        "file must be rewritten without first, got: {on_disk}"
    );
    let list_text = responses[3]["result"]["content"][0]["text"]
        .as_str()
        .expect("list text");
    assert!(
        list_text.contains("second") && !list_text.contains("first"),
        "engine must keep only second, got: {list_text}"
    );
}

#[test]
fn test_mcp_remove_spec_deletes_startup_loaded_file() {
    let temp_dir = tempfile::tempdir().unwrap();
    let policy = temp_dir.path().join("workspace_policy.lemma");
    std::fs::write(
        &policy,
        "spec workspace_policy\ndata x: number\nrule y: x\n",
    )
    .unwrap();

    let responses = mcp_session(
        Some(temp_dir.path()),
        true,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "remove_spec",
                    "arguments": { "spec": "workspace_policy" }
                }),
            ),
            make_request(3, "tools/call", json!({ "name": "list", "arguments": {} })),
        ],
    );

    assert!(responses.len() >= 3);
    assert_eq!(
        responses[1]["result"]["content"][0]["text"],
        "Spec 'workspace_policy' removed."
    );
    assert!(
        !policy.exists(),
        "remove_spec must delete startup-loaded .lemma from disk"
    );
    let list_after = responses[2]["result"]["content"][0]["text"]
        .as_str()
        .expect("list text");
    assert!(
        !list_after.contains("workspace_policy"),
        "spec must be gone after remove, got: {list_after}"
    );
}

#[test]
fn test_mcp_clear_deletes_startup_loaded_workspace_files() {
    let temp_dir = tempfile::tempdir().unwrap();
    let policy = temp_dir.path().join("workspace_policy.lemma");
    std::fs::write(
        &policy,
        "spec workspace_policy\ndata x: number\nrule y: x\n",
    )
    .unwrap();

    let responses = mcp_session(
        Some(temp_dir.path()),
        true,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(2, "tools/call", json!({ "name": "list", "arguments": {} })),
            make_request(3, "tools/call", json!({ "name": "clear", "arguments": {} })),
            make_request(4, "tools/call", json!({ "name": "list", "arguments": {} })),
        ],
    );

    assert!(responses.len() >= 4);
    let list_before = responses[1]["result"]["content"][0]["text"]
        .as_str()
        .expect("list text");
    assert!(
        list_before.contains("workspace_policy"),
        "startup load must see workspace file, got: {list_before}"
    );

    assert_eq!(
        responses[2]["result"]["content"][0]["text"],
        "Removed all specs."
    );
    assert!(
        !policy.exists(),
        "clear must delete startup-loaded .lemma from disk"
    );

    let list_after = responses[3]["result"]["content"][0]["text"]
        .as_str()
        .expect("list text");
    assert!(
        !list_after.contains("workspace_policy"),
        "spec must be gone after clear, got: {list_after}"
    );
}

#[test]
fn test_mcp_clear_description_leads_with_remove_all() {
    let temp_dir = tempfile::tempdir().unwrap();
    let responses = mcp_session(
        Some(temp_dir.path()),
        true,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(2, "tools/list", json!({})),
        ],
    );
    let tools = responses[1]["result"]["tools"].as_array().expect("tools");
    let clear = tools
        .iter()
        .find(|t| t["name"] == "clear")
        .expect("clear tool");
    let description = clear["description"].as_str().expect("description");
    assert_eq!(
        description, "Remove all specs.",
        "clear description must be remove-all only, got: {description}"
    );
}

#[test]
fn test_mcp_remove_spec_blocked_without_write() {
    let temp_dir = tempfile::tempdir().unwrap();

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "remove_spec",
                    "arguments": { "spec": "anything" }
                }),
            ),
            make_request(
                3,
                "tools/call",
                json!({
                    "name": "clear",
                    "arguments": {}
                }),
            ),
        ],
    );

    assert!(responses.len() >= 3);
    for i in [1, 2] {
        let error = &responses[i]["error"];
        assert!(
            error.is_object(),
            "write tool call {i} must error without --write"
        );
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("Write tools are disabled"),
            "got: {}",
            error["message"]
        );
    }
}

#[test]
fn test_mcp_install_blocked_without_write() {
    let temp_dir = tempfile::tempdir().unwrap();

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "install",
                    "arguments": { "repository": "@iso/countries" }
                }),
            ),
        ],
    );

    assert!(responses.len() >= 2);
    let error = &responses[1]["error"];
    assert!(error.is_object(), "install without write must error");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("Write tools are disabled"),
        "got: {}",
        error["message"]
    );
    assert!(
        !temp_dir.path().join("lemma_deps").exists(),
        "install without write must not write lemma_deps"
    );
}

#[test]
fn test_mcp_tools_list_write_tools() {
    let temp_dir = tempfile::tempdir().unwrap();

    let responses = mcp_session(
        Some(temp_dir.path()),
        true,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(2, "tools/list", json!({})),
        ],
    );

    assert!(responses.len() >= 2);
    let tools = responses[1]["result"]["tools"]
        .as_array()
        .expect("tools should be an array");
    let tool_names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();

    assert!(tool_names.contains(&"run"), "Should list run tool");
    assert!(tool_names.contains(&"list"), "Should list list tool");
    assert!(tool_names.contains(&"show"), "Should list show tool");
    assert!(tool_names.contains(&"check"), "Should list check tool");
    assert!(tool_names.contains(&"guide"), "Should list guide tool");
    assert!(
        tool_names.contains(&"add_spec"),
        "Should list add_spec tool in write mode"
    );
    assert!(
        tool_names.contains(&"update_spec"),
        "Should list update_spec tool in write mode"
    );
    assert!(
        tool_names.contains(&"remove_spec"),
        "Should list remove_spec tool in write mode"
    );
    assert!(
        tool_names.contains(&"clear"),
        "Should list clear tool in write mode"
    );
    assert!(
        tool_names.contains(&"install"),
        "Should list install tool in write mode"
    );
    assert!(tool_names.contains(&"source"), "Should list source tool");
    assert!(
        tool_names.contains(&"evaluate"),
        "Should list deprecated evaluate alias"
    );
    assert_eq!(
        tool_names.len(),
        12,
        "Write mode should have exactly 12 tools, got: {:?}",
        tool_names
    );
}

#[test]
fn test_mcp_tools_have_input_schemas() {
    let temp_dir = tempfile::tempdir().unwrap();

    let responses = mcp_session(
        Some(temp_dir.path()),
        true,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(2, "tools/list", json!({})),
        ],
    );

    assert!(responses.len() >= 2);
    let tools = responses[1]["result"]["tools"]
        .as_array()
        .expect("tools should be an array");

    for tool in tools {
        let name = tool["name"].as_str().unwrap();
        assert!(
            tool["description"].as_str().is_some_and(|d| !d.is_empty()),
            "Tool '{}' should have a non-empty description",
            name
        );
        assert!(
            tool["inputSchema"].is_object(),
            "Tool '{}' should have an inputSchema",
            name
        );
        assert_eq!(
            tool["inputSchema"]["type"], "object",
            "Tool '{}' inputSchema type should be 'object'",
            name
        );
    }
}

// ── evaluate with data overrides ────────────────────────────────────────

#[test]
fn test_mcp_evaluate_with_data_overrides() {
    let temp_dir = tempfile::tempdir().unwrap();
    write_spec(temp_dir.path(), "pricing.lemma", pricing_spec());

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "run",
                    "arguments": {
                        "spec": "pricing",
                        "data": { "quantity": 5 }
                    }
                }),
            ),
        ],
    );

    assert!(responses.len() >= 2);
    let text = responses[1]["result"]["content"][0]["text"]
        .as_str()
        .expect("run should return text");
    assert!(
        text.contains("total: 50"),
        "expected total: 50, got: {text}"
    );
}

// ── add_spec then evaluate ──────────────────────────────────────────────

#[test]
fn test_mcp_add_spec_then_evaluate() {
    let temp_dir = tempfile::tempdir().unwrap();

    let responses = mcp_session(
        Some(temp_dir.path()),
        true,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "add_spec",
                    "arguments": {
                        "code": "spec dynamic\ndata n: 7\nrule doubled: n * 2\n",
                        "attribute": "dynamic.lemma"
                    }
                }),
            ),
            make_request(
                3,
                "tools/call",
                json!({
                    "name": "run",
                    "arguments": { "spec": "dynamic" }
                }),
            ),
        ],
    );

    assert!(responses.len() >= 3);

    let add_text = responses[1]["result"]["content"][0]["text"]
        .as_str()
        .expect("add_spec should return text");
    assert_eq!(add_text, "Spec added successfully.");

    let eval_text = responses[2]["result"]["content"][0]["text"]
        .as_str()
        .expect("run should return text");
    assert!(
        eval_text.contains("doubled: 14"),
        "expected doubled: 14, got: {eval_text}"
    );
}

#[test]
fn test_mcp_update_spec_with_dependents() {
    let temp_dir = tempfile::tempdir().unwrap();
    write_spec(
        temp_dir.path(),
        "dep.lemma",
        "spec dep\ndata value: 10\nrule out: value\n",
    );
    write_spec(
        temp_dir.path(),
        "consumer.lemma",
        "spec consumer\nuses d: dep\nrule total: d.value\n",
    );

    let responses = mcp_session(
        Some(temp_dir.path()),
        true,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "update_spec",
                    "arguments": {
                        "code": "spec dep\ndata value: 20\nrule out: value\n",
                        "attribute": "dep.lemma"
                    }
                }),
            ),
            make_request(
                3,
                "tools/call",
                json!({
                    "name": "run",
                    "arguments": { "spec": "consumer" }
                }),
            ),
        ],
    );

    assert!(responses.len() >= 3);

    let update_text = responses[1]["result"]["content"][0]["text"]
        .as_str()
        .expect("update_spec should return text");
    assert_eq!(update_text, "Spec updated successfully.");

    let eval_text = responses[2]["result"]["content"][0]["text"]
        .as_str()
        .expect("run should return text");
    assert!(
        eval_text.contains("total: 20"),
        "expected total: 20 after update, got: {eval_text}"
    );
}

#[test]
fn test_add_spec_persists_to_disk() {
    let temp_dir = tempfile::tempdir().unwrap();
    let code = "spec dynamic\ndata n: 7\nrule doubled: n * 2\n";

    let responses = mcp_session(
        Some(temp_dir.path()),
        true,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "add_spec",
                    "arguments": {
                        "code": code,
                        "attribute": "dynamic.lemma"
                    }
                }),
            ),
        ],
    );

    assert!(responses.len() >= 2);
    assert_eq!(
        responses[1]["result"]["content"][0]["text"],
        "Spec added successfully."
    );

    let path = temp_dir.path().join("dynamic.lemma");
    let on_disk = std::fs::read_to_string(&path).expect("dynamic.lemma must exist after add_spec");
    let expected = lemma::format_source(
        code,
        lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
            "dynamic.lemma",
        ))),
    )
    .expect("fixture must format");
    assert_eq!(on_disk, expected);
}

#[test]
fn test_update_spec_persists_to_disk() {
    let temp_dir = tempfile::tempdir().unwrap();
    write_spec(
        temp_dir.path(),
        "dep.lemma",
        "spec dep\ndata value: 10\nrule out: value\n",
    );

    let new_code = "spec dep\ndata value: 20\nrule out: value\n";
    let responses = mcp_session(
        Some(temp_dir.path()),
        true,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "update_spec",
                    "arguments": {
                        "code": new_code,
                        "attribute": "dep.lemma"
                    }
                }),
            ),
        ],
    );

    assert!(responses.len() >= 2);
    assert_eq!(
        responses[1]["result"]["content"][0]["text"],
        "Spec updated successfully."
    );

    let on_disk =
        std::fs::read_to_string(temp_dir.path().join("dep.lemma")).expect("dep.lemma must exist");
    let expected = lemma::format_source(
        new_code,
        lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("dep.lemma"))),
    )
    .expect("fixture must format");
    assert_eq!(on_disk, expected);
    assert!(on_disk.contains("20"));
}

#[test]
fn test_update_spec_creates_new_file() {
    let temp_dir = tempfile::tempdir().unwrap();
    let code = "spec brand_new\ndata v: 1\nrule r: v\n";
    let responses = mcp_session(
        Some(temp_dir.path()),
        true,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "update_spec",
                    "arguments": {
                        "code": code,
                        "attribute": "brand_new.lemma"
                    }
                }),
            ),
            make_request(
                3,
                "tools/call",
                json!({
                    "name": "list",
                    "arguments": {}
                }),
            ),
        ],
    );
    assert_eq!(
        responses[1]["result"]["content"][0]["text"],
        "Spec updated successfully."
    );
    assert!(
        temp_dir.path().join("brand_new.lemma").exists(),
        "update_spec must create a missing file"
    );
    let list_text = responses[2]["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("");
    assert!(
        list_text.contains("brand_new"),
        "new identity must be listed: {list_text}"
    );
}

#[test]
fn test_update_spec_prunes_dropped_sibling_in_same_file() {
    let temp_dir = tempfile::tempdir().unwrap();
    write_spec(
        temp_dir.path(),
        "bundle.lemma",
        "spec keep\ndata v: 1\nrule r: v\n\nspec drop_me\ndata v: 2\nrule r: v\n",
    );
    let responses = mcp_session(
        Some(temp_dir.path()),
        true,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "update_spec",
                    "arguments": {
                        "code": "spec keep\ndata v: 9\nrule r: v\n",
                        "attribute": "bundle.lemma"
                    }
                }),
            ),
            make_request(
                3,
                "tools/call",
                json!({
                    "name": "list",
                    "arguments": {}
                }),
            ),
        ],
    );
    assert_eq!(
        responses[1]["result"]["content"][0]["text"],
        "Spec updated successfully."
    );
    let list_text = responses[2]["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("");
    assert!(list_text.contains("keep"), "keep must remain: {list_text}");
    assert!(
        !list_text.contains("drop_me"),
        "dropped sibling must be pruned: {list_text}"
    );
}

#[test]
fn test_update_spec_does_not_prune_other_files() {
    let temp_dir = tempfile::tempdir().unwrap();
    write_spec(temp_dir.path(), "a.lemma", "spec a\ndata v: 1\nrule r: v\n");
    write_spec(temp_dir.path(), "b.lemma", "spec b\ndata v: 2\nrule r: v\n");
    let responses = mcp_session(
        Some(temp_dir.path()),
        true,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "update_spec",
                    "arguments": {
                        "code": "spec a\ndata v: 9\nrule r: v\n",
                        "attribute": "a.lemma"
                    }
                }),
            ),
            make_request(
                3,
                "tools/call",
                json!({
                    "name": "list",
                    "arguments": {}
                }),
            ),
        ],
    );
    assert_eq!(
        responses[1]["result"]["content"][0]["text"],
        "Spec updated successfully."
    );
    let list_text = responses[2]["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("");
    assert!(
        list_text.contains("\"name\": \"a\"") || list_text.contains("spec a"),
        "workspace spec a must remain: {list_text}"
    );
    assert!(
        list_text.contains("\"name\": \"b\"") || list_text.contains("spec b"),
        "other file must not be pruned: {list_text}"
    );
}

#[test]
fn test_add_spec_path_traversal_rejected() {
    let temp_dir = tempfile::tempdir().unwrap();

    let responses = mcp_session(
        Some(temp_dir.path()),
        true,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "add_spec",
                    "arguments": {
                        "code": "spec escape\ndata x: 1\nrule y: x\n",
                        "attribute": "../escape.lemma"
                    }
                }),
            ),
        ],
    );

    assert!(responses.len() >= 2);
    let error = &responses[1]["error"];
    assert!(
        error.is_object(),
        "path traversal must return JSON-RPC error, got: {}",
        responses[1]
    );
    let message = error["message"].as_str().unwrap_or("");
    assert!(
        message.contains(".."),
        "error should mention '..', got: {message}"
    );
    assert!(
        !temp_dir.path().join("../escape.lemma").exists()
            || !std::fs::read_to_string(temp_dir.path().join("../escape.lemma"))
                .unwrap_or_default()
                .contains("spec escape"),
        "must not write escaped path"
    );
}

#[test]
fn test_add_spec_write_failure_rolls_back_engine() {
    let temp_dir = tempfile::tempdir().unwrap();
    // Parent path component is a file, so create_dir_all / write must fail.
    std::fs::write(temp_dir.path().join("blocked"), "not a directory").unwrap();

    let responses = mcp_session(
        Some(temp_dir.path()),
        true,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "add_spec",
                    "arguments": {
                        "code": "spec trapped\ndata x: 1\nrule y: x\n",
                        "attribute": "blocked/trapped.lemma"
                    }
                }),
            ),
            make_request(
                3,
                "tools/call",
                json!({
                    "name": "list",
                    "arguments": {}
                }),
            ),
        ],
    );

    assert!(responses.len() >= 3);
    let error = &responses[1]["error"];
    assert!(
        error.is_object(),
        "write failure must return JSON-RPC error, got: {}",
        responses[1]
    );
    assert!(
        error["message"]
            .as_str()
            .unwrap_or("")
            .contains("Failed to persist"),
        "error should mention persist failure, got: {}",
        error["message"]
    );

    let list_text = responses[2]["result"]["content"][0]["text"]
        .as_str()
        .expect("list should return text");
    assert!(
        !list_text.contains("trapped"),
        "engine must roll back failed persist; list was: {list_text}"
    );
    assert!(
        !temp_dir.path().join("blocked/trapped.lemma").exists(),
        "failed write must leave no target file"
    );
}

// ── source for missing spec ─────────────────────────────────────────────

#[test]
fn test_mcp_source_missing_spec() {
    let temp_dir = tempfile::tempdir().unwrap();

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "source",
                    "arguments": { "spec": "nonexistent" }
                }),
            ),
        ],
    );

    assert!(responses.len() >= 2);
    let result = &responses[1]["result"];
    assert_eq!(result["isError"], true, "missing spec must be isError");
    let text = result["content"][0]["text"].as_str().expect("diagnostics");
    let diagnostics: serde_json::Value = serde_json::from_str(text).expect("EngineError JSON");
    assert!(diagnostics.is_array() && !diagnostics.as_array().unwrap().is_empty());
}

// ── evaluate with invalid effective ─────────────────────────────────────

#[test]
fn test_mcp_evaluate_invalid_effective() {
    let temp_dir = tempfile::tempdir().unwrap();
    write_spec(
        temp_dir.path(),
        "simple.lemma",
        "spec simple\ndata x: 1\nrule y: x\n",
    );

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "run",
                    "arguments": {
                        "spec": "simple",
                        "effective": "not-a-date"
                    }
                }),
            ),
        ],
    );

    assert!(responses.len() >= 2);
    let result = &responses[1]["result"];
    assert_eq!(
        result["isError"], true,
        "Should return isError for invalid effective datetime"
    );
    let text = result["content"][0]["text"].as_str().expect("diagnostics");
    assert!(
        text.contains("Invalid effective"),
        "Error should mention invalid effective, got: {text}"
    );
}

#[test]
fn test_mcp_evaluate_respects_effective_for_versioned_spec() {
    let temp_dir = tempfile::tempdir().unwrap();
    write_spec(
        temp_dir.path(),
        "temporal.lemma",
        r#"spec pricing 2025-01-01
data base: 10
rule total: base

spec pricing 2026-01-01
data base: 99
rule total: base
"#,
    );

    let run_eval = |effective: &str| -> String {
        let responses = mcp_session(
            Some(temp_dir.path()),
            false,
            &[
                make_request(1, "server/discover", json!({})),
                make_request(
                    2,
                    "tools/call",
                    json!({
                        "name": "run",
                        "arguments": {
                            "spec": "pricing",
                            "effective": effective,
                            "rules": "total"
                        }
                    }),
                ),
            ],
        );
        assert!(responses.len() >= 2, "expected evaluate response");
        responses[1]["result"]["content"][0]["text"]
            .as_str()
            .expect("evaluate text")
            .to_string()
    };

    let out_2025 = run_eval("2025-06-01");
    let out_2026 = run_eval("2026-06-01");
    assert!(
        out_2025.contains("total: 10"),
        "2025 version must show total: 10, got: {out_2025}"
    );
    assert!(
        out_2026.contains("total: 99"),
        "2026 version must show total: 99, got: {out_2026}"
    );
}

// ── response IDs match request IDs ──────────────────────────────────────

#[test]
fn test_mcp_response_ids_match_request_ids() {
    let temp_dir = tempfile::tempdir().unwrap();
    write_spec(
        temp_dir.path(),
        "simple.lemma",
        "spec simple\ndata x: 1\nrule y: x\n",
    );

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(10, "server/discover", json!({})),
            make_request(20, "tools/list", json!({})),
            make_request(
                30,
                "tools/call",
                json!({
                    "name": "list",
                    "arguments": {}
                }),
            ),
        ],
    );

    assert_eq!(responses.len(), 3);
    assert_eq!(responses[0]["id"], 10, "First response should have id 10");
    assert_eq!(responses[1]["id"], 20, "Second response should have id 20");
    assert_eq!(responses[2]["id"], 30, "Third response should have id 30");
}

#[test]
fn mcp_add_spec_without_attribute_must_require_attribute() {
    let temp_dir = tempfile::tempdir().unwrap();

    let responses = mcp_session(
        Some(temp_dir.path()),
        true,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "add_spec",
                    "arguments": {
                        "code": "spec test_spec\ndata x: 5\nrule y: x * 2"
                    }
                }),
            ),
        ],
    );

    assert!(responses.len() >= 2);
    assert!(
        responses[1]["error"].is_object(),
        "add_spec without attribute must return error, got: {}",
        responses[1]
    );
}

#[test]
fn mcp_evaluate_veto_must_not_invent_vetoed_placeholder() {
    let temp_dir = tempfile::tempdir().unwrap();
    write_spec(
        temp_dir.path(),
        "veto_no_message.lemma",
        "spec veto_no_message\ndata value: -5\nrule r: value > 0\n    unless value < 0 then veto\n",
    );

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "run",
                    "arguments": {
                        "spec": "veto_no_message"
                    }
                }),
            ),
        ],
    );

    assert!(responses.len() >= 2);
    let text = responses[1]["result"]["content"][0]["text"]
        .as_str()
        .expect("run should return text");
    assert!(
        text.contains("r:"),
        "bare veto must still name the rule, got: {text}"
    );
    assert!(
        !text.contains("Vetoed"),
        "MCP must not invent 'Vetoed' placeholder when veto_reason missing, got: {text}"
    );
}

// ── check / guide / resources / unit maps ───────────────────────────────

#[test]
fn test_mcp_check_invalid_returns_diagnostics() {
    let temp_dir = tempfile::tempdir().unwrap();

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "check",
                    "arguments": {
                        "sources": [["new_spec", "this is not valid lemma code !!!"]]
                    }
                }),
            ),
        ],
    );

    assert!(responses.len() >= 2);
    let result = &responses[1]["result"];
    assert_eq!(result["isError"], true);
    let text = result["content"][0]["text"].as_str().expect("text");
    let diagnostics: serde_json::Value = serde_json::from_str(text).expect("JSON diagnostics");
    let first = &diagnostics[0];
    assert!(first["message"].as_str().is_some());
    assert!(
        first["source"]["line"].as_u64().is_some(),
        "diagnostic must include line, got: {text}"
    );
    assert!(
        first["source"]["column"].as_u64().is_some(),
        "diagnostic must include column, got: {text}"
    );
}

#[test]
fn test_mcp_check_does_not_mutate_list() {
    let temp_dir = tempfile::tempdir().unwrap();
    write_spec(temp_dir.path(), "pricing.lemma", pricing_spec());

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(2, "tools/call", json!({ "name": "list", "arguments": {} })),
            make_request(
                3,
                "tools/call",
                json!({
                    "name": "check",
                    "arguments": {
                        "sources": [["draft", "spec draft\ndata x: number\nrule y: x"]]
                    }
                }),
            ),
            make_request(4, "tools/call", json!({ "name": "list", "arguments": {} })),
        ],
    );

    assert!(responses.len() >= 4);
    let list_before = responses[1]["result"]["content"][0]["text"]
        .as_str()
        .expect("list text");
    let check_text = responses[2]["result"]["content"][0]["text"]
        .as_str()
        .expect("check text");
    assert!(
        responses[2]["result"].get("isError").is_none()
            || responses[2]["result"]["isError"] != true,
        "valid check must succeed"
    );
    let check_json: serde_json::Value =
        serde_json::from_str(check_text).expect("check success is quality JSON");
    assert!(check_json.is_array(), "check success must be JSON array");
    let list_after = responses[3]["result"]["content"][0]["text"]
        .as_str()
        .expect("list text");
    assert_eq!(
        list_before, list_after,
        "check must not mutate loaded specs"
    );
    assert!(
        !list_after.contains("draft"),
        "draft must not appear in list after check"
    );
}

#[test]
fn test_mcp_check_resolves_workspace_and_units() {
    let temp_dir = tempfile::tempdir().unwrap();
    write_spec(
        temp_dir.path(),
        "base.lemma",
        "spec base\ndata flag: boolean\nrule ok: flag\n",
    );

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "check",
                    "arguments": {
                        "sources": [
                            ["base", "spec base\ndata flag: boolean\nrule ok: flag\n"],
                            ["ship", "spec ship\nuses lemma units\nuses base\ndata package_weight: units.mass\nrule heavy: package_weight > 0 units.mass.kilogram\n"]
                        ]
                    }
                }),
            ),
        ],
    );

    assert!(responses.len() >= 2);
    let result = &responses[1]["result"];
    assert!(
        result.get("isError").is_none() || result["isError"] != true,
        "check with uses lemma units + cross-spec uses must succeed, got: {result}"
    );
    let text = result["content"][0]["text"].as_str().expect("text");
    let check_json: serde_json::Value =
        serde_json::from_str(text).expect("check success is quality JSON");
    assert!(check_json.is_array(), "check success must be JSON array");
}

#[test]
fn test_mcp_check_resolves_registry_dependency() {
    let temp_dir = tempfile::tempdir().unwrap();

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "check",
                    "arguments": {
                        "sources": [
                            ["@test/base", "repo @test/base\n\nspec base\n\ndata flag: boolean\n\nrule is_set: flag\n"],
                            ["ship", "spec ship\n\nuses base: @test/base base\n\nrule ok: base.is_set\n"]
                        ]
                    }
                }),
            ),
        ],
    );

    assert!(responses.len() >= 2);
    let result = &responses[1]["result"];
    assert!(
        result.get("isError").is_none() || result["isError"] != true,
        "check with @owner/repo dependency must succeed, got: {result}"
    );
}

#[test]
fn test_mcp_check_reports_veto_cascade_recommendation() {
    let temp_dir = tempfile::tempdir().unwrap();
    let code = r#"spec eligibility 2026-01-01
"""
Age gate.
"""

data age: number
  -> help "Customer age."
  -> suggest 30

rule is_eligible: true
  unless age < 18 then veto "Must be 18+"
"#;

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "check",
                    "arguments": {
                        "sources": [["eligibility.lemma", code]]
                    }
                }),
            ),
        ],
    );

    assert!(responses.len() >= 2);
    let result = &responses[1]["result"];
    assert!(
        result.get("isError").is_none() || result["isError"] != true,
        "check must succeed with recommendations, got: {result}"
    );
    let text = result["content"][0]["text"].as_str().expect("text");
    let recs: serde_json::Value =
        serde_json::from_str(text).expect("check success is quality JSON");
    let arr = recs.as_array().expect("quality array");
    assert!(!arr.is_empty(), "must list recommendations, got: {text}");
    let joined = serde_json::to_string(&recs).expect("serialize");
    assert!(
        joined.contains("is_eligible") && joined.contains("veto"),
        "must report veto-as-rejection cascade, got: {text}"
    );
}

#[test]
fn test_mcp_check_clean_spec_has_no_recommendations() {
    let temp_dir = tempfile::tempdir().unwrap();
    let code = r#"spec pricing 2026-01-01
"""
Bulk pricing.
"""

data qty: number
  -> minimum 0
  -> maximum 1000000
  -> help "Order quantity."
  -> suggest 10

rule total: qty
"#;

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "check",
                    "arguments": {
                        "sources": [["pricing.lemma", code]]
                    }
                }),
            ),
        ],
    );

    assert!(responses.len() >= 2);
    let result = &responses[1]["result"];
    assert!(
        result.get("isError").is_none() || result["isError"] != true,
        "clean check must succeed, got: {result}"
    );
    let text = result["content"][0]["text"].as_str().expect("text");
    let recs: serde_json::Value =
        serde_json::from_str(text).expect("check success is quality JSON");
    assert_eq!(
        recs.as_array().map(|a| a.len()).unwrap_or(1),
        0,
        "clean spec must not emit recommendations, got: {text}"
    );
}

#[test]
fn test_mcp_check_invalid_skips_recommendations() {
    let temp_dir = tempfile::tempdir().unwrap();

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "check",
                    "arguments": {
                        "sources": [["bad.lemma", "this is not valid lemma code !!!"]]
                    }
                }),
            ),
        ],
    );

    assert!(responses.len() >= 2);
    let result = &responses[1]["result"];
    assert_eq!(result["isError"], true);
    let text = result["content"][0]["text"].as_str().expect("text");
    assert!(
        !text.contains("Recommendations:"),
        "failed plan must not include recommendations, got: {text}"
    );
}

#[test]
fn test_mcp_check_rejects_duplicate_source_label() {
    let temp_dir = tempfile::tempdir().unwrap();

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "check",
                    "arguments": {
                        "sources": [
                            ["base", "spec base\ndata x: 5"],
                            ["base", "spec other\ndata y: 10"]
                        ]
                    }
                }),
            ),
        ],
    );

    assert!(responses.len() >= 2);
    let result = &responses[1]["result"];
    assert_eq!(result["isError"], true, "duplicate source label must fail");
    let text = result["content"][0]["text"].as_str().expect("text");
    assert!(
        text.to_lowercase().contains("duplicate") || text.to_lowercase().contains("repeated"),
        "diagnostic must mention duplicate source, got: {text}"
    );
}

#[test]
fn test_mcp_show_json_includes_rule_units() {
    let temp_dir = tempfile::tempdir().unwrap();
    write_spec(
        temp_dir.path(),
        "money.lemma",
        "spec money\ndata amount: measure\n  -> unit eur: 1\n  -> unit cent: 0.01\nrule total: amount\n",
    );

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "show",
                    "arguments": { "spec": "money" }
                }),
            ),
        ],
    );

    let text = responses[1]["result"]["content"][0]["text"]
        .as_str()
        .expect("show text");
    let show: serde_json::Value = serde_json::from_str(text).expect("JSON Show");
    let units = &show["rules"]["total"]["type"]["units"];
    assert!(
        units.is_array() && units.as_array().unwrap().len() >= 2,
        "rule total must expose unit map in JSON Show, got: {text}"
    );
}

#[test]
fn test_mcp_evaluate_renders_unit_map() {
    let temp_dir = tempfile::tempdir().unwrap();
    write_spec(
        temp_dir.path(),
        "money.lemma",
        "spec money\ndata amount: measure\n  -> unit eur: 1\n  -> unit cent: 0.01\nrule total: amount\n",
    );

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "run",
                    "arguments": {
                        "spec": "money",
                        "rules": "total",
                        "data": { "amount": "84 eur" }
                    }
                }),
            ),
        ],
    );

    assert!(
        responses.len() >= 2,
        "expected discover + evaluate responses, got: {responses:?}"
    );
    let text = responses[1]["result"]["content"][0]["text"]
        .as_str()
        .expect("run text");
    assert!(
        text.contains("total: 84 eur") || text.contains("84 eur"),
        "run must show measure result in formatted tree, got: {text}"
    );
}

#[test]
fn test_mcp_guide_topics() {
    let temp_dir = tempfile::tempdir().unwrap();
    let topics = [
        "method",
        "syntax",
        "data",
        "rules",
        "units",
        "veto",
        "composition",
        "anti_patterns",
        "evaluate",
        "full",
    ];
    let mut messages = vec![make_request(1, "server/discover", json!({}))];
    for (i, topic) in topics.iter().enumerate() {
        messages.push(make_request(
            (i + 2) as u64,
            "tools/call",
            json!({
                "name": "guide",
                "arguments": { "topic": topic }
            }),
        ));
    }

    let responses = mcp_session(Some(temp_dir.path()), false, &messages);
    assert!(responses.len() > topics.len());
    for (i, topic) in topics.iter().enumerate() {
        let text = responses[i + 1]["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or("");
        assert!(!text.is_empty(), "guide topic {topic} must not be empty");
        let guide_topic =
            lemma::documentation::GuideTopic::parse(topic).expect("known guide topic");
        assert_eq!(text, guide_topic.section_text(), "guide topic {topic}");
    }
}

#[test]
fn test_mcp_guide_default_is_evaluate() {
    let temp_dir = tempfile::tempdir().unwrap();
    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "guide",
                    "arguments": {}
                }),
            ),
        ],
    );

    assert!(responses.len() >= 2);
    let text = responses[1]["result"]["content"][0]["text"]
        .as_str()
        .expect("default guide text");
    assert_eq!(text, lemma::documentation::EVALUATE_GUIDE);
}

#[test]
fn test_mcp_guide_full_topic_is_authoring() {
    let temp_dir = tempfile::tempdir().unwrap();
    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(
                2,
                "tools/call",
                json!({
                    "name": "guide",
                    "arguments": { "topic": "full" }
                }),
            ),
        ],
    );

    assert!(responses.len() >= 2);
    let text = responses[1]["result"]["content"][0]["text"]
        .as_str()
        .expect("full guide text");
    assert_eq!(text, lemma::documentation::GuideTopic::Full.section_text());
}

#[test]
fn test_mcp_resources_list_and_read() {
    let temp_dir = tempfile::tempdir().unwrap();

    let responses = mcp_session(
        Some(temp_dir.path()),
        false,
        &[
            make_request(1, "server/discover", json!({})),
            make_request(2, "resources/list", json!({})),
            make_request(
                3,
                "resources/read",
                json!({ "uri": "lemma://guide/syntax" }),
            ),
            make_request(
                4,
                "resources/read",
                json!({ "uri": "lemma://examples/01_coffee_order.lemma" }),
            ),
            make_request(
                5,
                "resources/read",
                json!({ "uri": "lemma://examples/does_not_exist.lemma" }),
            ),
        ],
    );

    assert!(responses.len() >= 5);
    let resources = responses[1]["result"]["resources"]
        .as_array()
        .expect("resources list");
    assert!(
        resources.iter().any(|r| r["uri"] == "lemma://guide"),
        "must list lemma://guide"
    );
    assert!(
        resources
            .iter()
            .any(|r| r["uri"] == "lemma://examples/nl/tax/net_salary.lemma"),
        "must list nested example"
    );

    let syntax = responses[2]["result"]["contents"][0]["text"]
        .as_str()
        .expect("syntax resource");
    assert!(syntax.contains("Recommended spec opening order"));

    let coffee = responses[3]["result"]["contents"][0]["text"]
        .as_str()
        .expect("example resource");
    assert!(coffee.contains("spec coffee_order"));

    assert!(
        responses[4]["error"].is_object(),
        "unknown resource URI must error"
    );
}

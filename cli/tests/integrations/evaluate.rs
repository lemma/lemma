use assert_cmd::cargo::cargo_bin_cmd;

use predicates::prelude::*;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

fn explain_fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("engine")
        .join("tests")
        .join("fixtures")
        .join("explain")
}

/// Strip CSI/SGR sequences (`Source:` is gray in explain tables).
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(ch);
    }
    out
}

/// Inner text of the explain cell (before `Source:`), matching `format_explanation`.
fn extract_explain_ascii(stdout: &str) -> String {
    let plain = strip_ansi(stdout);
    let mut body = Vec::new();
    for line in plain.lines() {
        if !line.starts_with('│') {
            if !body.is_empty() {
                break;
            }
            continue;
        }
        let rest = line.strip_prefix('│').expect("starts with │");
        let inner = rest.strip_suffix('│').unwrap_or(rest).trim_end();
        let content = inner.strip_prefix(' ').unwrap_or(inner);
        if content.starts_with("Source:") {
            break;
        }
        body.push(content.to_string());
    }
    body.join("\n")
}

#[test]
fn test_cli_run_alias_still_evaluates() {
    let temp_dir = TempDir::new().unwrap();
    fs::write(
        temp_dir.path().join("test.lemma"),
        r#"
spec simple_test
data x: 10
data y: 5
rule added: x + y
rule multiplied: x * y
"#,
    )
    .unwrap();

    let mut cmd = cargo_bin_cmd!("lemma");
    cmd.arg("run")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("simple_test");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("added"))
        .stdout(predicate::str::contains("15"))
        .stdout(predicate::str::contains("multiplied"))
        .stdout(predicate::str::contains("50"));
}

#[test]
fn test_cli_evaluate_data_values() {
    let temp_dir = TempDir::new().unwrap();
    fs::write(
        temp_dir.path().join("test.lemma"),
        r#"
spec override_test
data base: number
rule doubled: base * 2
"#,
    )
    .unwrap();

    let mut cmd = cargo_bin_cmd!("lemma");
    cmd.arg("evaluate")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("override_test")
        .arg("base=7");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("14"));
}

#[test]
fn test_cli_run_nonexistent_spec() {
    let temp_dir = TempDir::new().unwrap();

    let mut cmd = cargo_bin_cmd!("lemma");
    cmd.arg("evaluate")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("nonexistent");

    cmd.assert().failure().stderr(
        predicate::str::contains("No execution plan").and(predicate::str::contains("nonexistent")),
    );
}

#[test]
fn test_cli_run_with_unless_clause() {
    let temp_dir = TempDir::new().unwrap();
    fs::write(
        temp_dir.path().join("test.lemma"),
        r#"
spec discount_test
data quantity: 15
rule discount: 0
  unless quantity >= 10 then 10
  unless quantity >= 20 then 20
"#,
    )
    .unwrap();

    let mut cmd = cargo_bin_cmd!("lemma");
    cmd.arg("evaluate")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("discount_test");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("10"));
}

#[test]
fn test_cli_show_spec() {
    let temp_dir = TempDir::new().unwrap();
    fs::write(
        temp_dir.path().join("test.lemma"),
        r#"
spec inspect_test
data name: text
  -> option "Test"
data value: number
  -> minimum 0
rule doubled: value * 2
"#,
    )
    .unwrap();

    let mut cmd = cargo_bin_cmd!("lemma");
    cmd.arg("show")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("inspect_test");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("inspect_test"))
        .stdout(predicate::str::contains("Data"))
        .stdout(predicate::str::contains("Rules"))
        .stdout(predicate::str::contains("minimum"))
        .stdout(predicate::str::contains("number"))
        .stdout(predicate::str::contains("doubled"));
}

#[test]
fn test_cli_show_json_is_input_json_schema() {
    let temp_dir = TempDir::new().unwrap();
    fs::write(
        temp_dir.path().join("test.lemma"),
        r#"
spec inspect_test
data name: text
  -> option "Test"
data value: number
  -> minimum 0
rule doubled: value * 2
rule label: name
"#,
    )
    .unwrap();

    let mut cmd = cargo_bin_cmd!("lemma");
    let output = cmd
        .arg("show")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("inspect_test")
        .arg("--json")
        .output()
        .expect("run lemma show --json");
    assert!(output.status.success(), "{output:?}");
    let document: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("show --json prints JSON");
    assert_eq!(
        document["$schema"],
        "https://json-schema.org/draft/2020-12/schema"
    );
    assert_eq!(document["title"], "inspect_test");
    assert_eq!(
        document["properties"]["name"]["enum"],
        serde_json::json!(["Test"])
    );
    assert_eq!(document["properties"]["value"]["type"], "string");
    assert_eq!(document["required"], serde_json::json!(["name", "value"]));
    assert!(document["x-lemma-rules"]["doubled"].is_object());
}

#[test]
fn test_cli_show_json_serializes_measure_types() {
    let temp_dir = TempDir::new().unwrap();
    fs::write(
        temp_dir.path().join("money.lemma"),
        r#"
spec purse
data money: measure
  -> unit eur: 1
data amount: money
rule out: amount
"#,
    )
    .unwrap();

    let mut cmd = cargo_bin_cmd!("lemma");
    let output = cmd
        .arg("show")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("purse")
        .arg("--json")
        .output()
        .expect("run lemma show --json");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("show --json prints JSON");
    assert_eq!(
        document["properties"]["amount"]["x-lemma-type"]["decomposition"],
        serde_json::json!({ "purse.money": 1 })
    );
}

#[test]
fn test_cli_specs_summary() {
    let temp_dir = TempDir::new().unwrap();

    fs::write(
        temp_dir.path().join("spec1.lemma"),
        r#"
spec spec1
data x: 1
"#,
    )
    .unwrap();

    fs::write(
        temp_dir.path().join("spec2.lemma"),
        r#"
spec spec2
data y: 2
"#,
    )
    .unwrap();

    let mut cmd = cargo_bin_cmd!("lemma");
    cmd.arg("specs").arg("--path").arg(temp_dir.path());

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("spec1"))
        .stdout(predicate::str::contains("spec2"))
        .stdout(predicate::str::contains("Found").not())
        .stdout(predicate::str::contains("data").not());
}

#[test]
fn test_cli_specs_scopes_main_repository() {
    let temp_dir = TempDir::new().unwrap();

    fs::write(
        temp_dir.path().join("named.lemma"),
        r#"repo deps_repo

spec dep_only
data d: 1
"#,
    )
    .unwrap();

    fs::write(
        temp_dir.path().join("workspace.lemma"),
        r#"spec entry
data x: 1"#,
    )
    .unwrap();

    let mut cmd = cargo_bin_cmd!("lemma");
    cmd.arg("specs").arg("--path").arg(temp_dir.path());

    cmd.assert().success().stdout(
        predicate::str::contains("entry")
            .and(predicate::str::contains("deps_repo"))
            .and(predicate::str::contains("dep_only"))
            .and(predicate::str::contains("Found").not())
            .and(predicate::str::contains("Repositories:").not()),
    );
}

#[test]
fn test_cli_specs_text_groups_unique_names_per_repository() {
    let temp_dir = TempDir::new().unwrap();
    fs::write(
        temp_dir.path().join("named.lemma"),
        "repo deps_repo\n\nspec dep_only\ndata d: 1\n",
    )
    .unwrap();
    fs::write(
        temp_dir.path().join("workspace.lemma"),
        "spec entry\ndata x: 1\n",
    )
    .unwrap();
    fs::write(
        temp_dir.path().join("workspace_2026.lemma"),
        "spec entry 2026-01-01\ndata x: 2\n",
    )
    .unwrap();

    let mut cmd = cargo_bin_cmd!("lemma");
    cmd.arg("specs").arg("--path").arg(temp_dir.path());

    cmd.assert()
        .success()
        .stdout("entry ...2026-01-01\nentry 2026-01-01\n\ndeps_repo\n  dep_only\n");
}

#[test]
fn test_cli_run_with_arithmetic() {
    let temp_dir = TempDir::new().unwrap();
    fs::write(
        temp_dir.path().join("test.lemma"),
        r#"
spec arithmetic_test
data price: 100
rule with_tax: price * 1.21
"#,
    )
    .unwrap();

    let mut cmd = cargo_bin_cmd!("lemma");
    cmd.arg("evaluate")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("arithmetic_test");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("121"));
}

#[test]
fn test_cli_parse_error_handling() {
    let temp_dir = TempDir::new().unwrap();
    fs::write(
        temp_dir.path().join("test.lemma"),
        r#"
spec invalid
this is not valid lemma syntax
"#,
    )
    .unwrap();

    let mut cmd = cargo_bin_cmd!("lemma");
    cmd.arg("evaluate")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("invalid");

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("error").or(predicate::str::contains("Error")));
}

#[test]
fn test_cli_reports_errors_from_all_files() {
    let temp_dir = TempDir::new().unwrap();

    fs::write(
        temp_dir.path().join("valid.lemma"),
        r#"
spec valid_spec
data price: 100
rule doubled: price * 2
"#,
    )
    .unwrap();

    fs::write(
        temp_dir.path().join("broken_a.lemma"),
        r#"
spec broken_a
this is not valid lemma
"#,
    )
    .unwrap();

    fs::write(
        temp_dir.path().join("broken_b.lemma"),
        r#"
spec broken_b
also invalid lemma syntax
"#,
    )
    .unwrap();

    let mut cmd = cargo_bin_cmd!("lemma");
    cmd.arg("evaluate")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("valid_spec");

    let output = cmd.output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "Should fail when workspace has broken files"
    );
    assert!(
        stderr.contains("broken_a") && stderr.contains("broken_b"),
        "Should report errors from both broken files, got:\n{}",
        stderr
    );
}

#[test]
fn test_cli_explain_renders_rule_explanation_not_data_catalog() {
    let temp_dir = TempDir::new().unwrap();
    fs::write(
        temp_dir.path().join("test.lemma"),
        r#"
spec data_section
data base_value: 100
rule doubled: base_value * 2
"#,
    )
    .unwrap();

    let mut cmd = cargo_bin_cmd!("lemma");
    cmd.arg("evaluate")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("data_section")
        .arg("--rules=doubled")
        .arg("--explain");

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "evaluate --explain should succeed: {}",
        stdout
    );

    assert!(
        !stdout.lines().any(|line| line == "Rules"),
        "explain must not print Rules heading, got:\n{}",
        stdout
    );
    assert!(
        !stdout.lines().any(|line| line == "Missing data"),
        "prefilled input must not appear as Missing data section, got:\n{}",
        stdout
    );
    assert!(
        stdout.contains("base_value") && stdout.contains("100"),
        "explanation must still narrate base_value, got:\n{}",
        stdout
    );
}

#[test]
fn test_cli_explain_omits_missing_data_when_rule_already_answered_with_veto() {
    let temp_dir = TempDir::new().unwrap();
    fs::write(
        temp_dir.path().join("test.lemma"),
        r#"
spec settle
data denom: number
data is_smoker: boolean
data is_former_smoker: boolean
data years_since_quit: number
rule loading: 1
  unless is_former_smoker then years_since_quit + 1
  unless is_smoker then 2
rule premium: (1 / denom) * loading
"#,
    )
    .unwrap();

    let mut cmd = cargo_bin_cmd!("lemma");
    cmd.arg("evaluate")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("settle")
        .arg("--rules=premium")
        .arg("denom=0")
        .arg("--explain");

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "evaluate --explain should succeed: {}",
        stdout
    );
    assert!(
        !stdout.lines().any(|line| line == "Rules"),
        "explain must not print Rules heading, got:\n{}",
        stdout
    );
    assert!(
        !stdout.lines().any(|line| line == "Missing data"),
        "settled Computation must not print Missing data section for leftover live keys, got:\n{}",
        stdout
    );
}

#[test]
fn test_cli_explain_shows_unbound_inputs_in_box_without_section() {
    let temp_dir = TempDir::new().unwrap();
    fs::write(
        temp_dir.path().join("test.lemma"),
        r#"
spec open
data a: number
data b: number
rule main: a + b
"#,
    )
    .unwrap();

    let mut cmd = cargo_bin_cmd!("lemma");
    cmd.arg("evaluate")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("open")
        .arg("--rules=main")
        .arg("--explain");

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "evaluate --explain should succeed: {}",
        stdout
    );
    assert!(
        !stdout.lines().any(|line| line == "Missing data"),
        "explain must not print Missing data section, got:\n{}",
        stdout
    );
    assert!(
        !stdout.lines().any(|line| line == "Rules"),
        "explain must not print Rules heading, got:\n{}",
        stdout
    );
    assert!(
        stdout.contains("a") && stdout.contains("b"),
        "unbound operands must appear in the main explanation box, got:\n{}",
        stdout
    );
}

#[test]
fn test_cli_explain_shows_all_operands_in_nested_arithmetic() {
    let temp_dir = TempDir::new().unwrap();
    fs::write(
        temp_dir.path().join("test.lemma"),
        r#"
spec nested_explanation
data x: 10
data y: 20
data z: 30
rule a: x + 1
rule b: y + 2
rule c: z + 3
rule total: a + b + c
"#,
    )
    .unwrap();

    let mut cmd = cargo_bin_cmd!("lemma");
    cmd.arg("evaluate")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("nested_explanation")
        .arg("--rules=total")
        .arg("--explain");

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "evaluate --explain should succeed: {}",
        stdout
    );

    assert!(
        stdout.contains("a:") && stdout.contains("b:") && stdout.contains("c:"),
        "explain should expand all three rule operands (a, b, c), got:\n{}",
        stdout
    );

    let lines: Vec<&str> = stdout.lines().collect();
    let a_line = lines.iter().find(|l| l.contains("a:")).unwrap();
    let b_line = lines.iter().find(|l| l.contains("b:")).unwrap();
    let c_line = lines.iter().find(|l| l.contains("c:")).unwrap();
    let indent_of = |line: &str| line.len() - line.trim_start().len();
    assert_eq!(
        indent_of(a_line),
        indent_of(b_line),
        "a and b should be at the same depth (flattened), got:\n{}",
        stdout
    );
    assert_eq!(
        indent_of(b_line),
        indent_of(c_line),
        "b and c should be at the same depth (flattened), got:\n{}",
        stdout
    );
}

#[test]
fn test_cli_explain_shows_negated_comparison_not_false() {
    let temp_dir = TempDir::new().unwrap();
    fs::write(
        temp_dir.path().join("test.lemma"),
        r#"
spec explain_test
rule out: true
 unless 5 < 3 then false
"#,
    )
    .unwrap();

    let mut cmd = cargo_bin_cmd!("lemma");
    cmd.arg("evaluate")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("explain_test")
        .arg("--explain")
        .arg("--json");

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "evaluate --explain --json should succeed: {}",
        stdout
    );

    let json: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is valid JSON");
    assert_eq!(
        json["results"]["out"]["explanation"]["causes"][0]["condition"],
        "5 >= 3"
    );
    assert_eq!(
        json["results"]["out"]["explanation"]["causes"][0]["value"],
        "true"
    );
}

#[test]
fn test_cli_explain_scalar_conversion_format() {
    let temp_dir = TempDir::new().unwrap();
    fs::write(
        temp_dir.path().join("test.lemma"),
        r#"
spec test_cli_conversion_explain
data mass: measure
    -> unit kilogram: 1.0
    -> unit gram: 0.001
    -> suggest 2 kilogram
rule result: mass as gram
"#,
    )
    .unwrap();

    let mut cmd = cargo_bin_cmd!("lemma");
    cmd.arg("evaluate")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("test_cli_conversion_explain")
        .arg("mass=2 kilogram")
        .arg("--rules=result")
        .arg("--explain");

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "evaluate --explain should succeed: {}",
        stdout
    );
    assert!(stdout.contains("2 kilogram"), "stdout:\n{stdout}");
    assert!(
        stdout.contains("1 gram is 0.001 kilogram"),
        "stdout:\n{stdout}"
    );
    assert!(stdout.contains("mass"), "stdout:\n{stdout}");
    assert!(
        !stdout.contains("The measure of mass is"),
        "stdout:\n{stdout}"
    );
    assert!(!stdout.contains('×'), "stdout:\n{stdout}");
    assert!(!stdout.contains("mass as gram is"), "stdout:\n{stdout}");
}

#[test]
fn test_cli_explain_date_range_conversion_format() {
    let temp_dir = TempDir::new().unwrap();
    fs::write(
        temp_dir.path().join("test.lemma"),
        r#"
spec test_cli_date_conversion_explain
uses lemma units
data age: date range -> suggest 2024-06-01...2024-06-15
rule result: age as day
"#,
    )
    .unwrap();

    let mut cmd = cargo_bin_cmd!("lemma");
    cmd.arg("evaluate")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("test_cli_date_conversion_explain")
        .arg("age=2024-06-01...2024-06-15")
        .arg("--rules=result")
        .arg("--explain");

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "evaluate --explain should succeed: {}",
        stdout
    );
    assert!(
        stdout.contains("2 week") || (stdout.contains("14") && stdout.contains("day")),
        "stdout:\n{stdout}"
    );
    assert!(stdout.contains('−'), "stdout:\n{stdout}");
    assert!(
        stdout.contains("2024-06-01") && stdout.contains("2024-06-15"),
        "stdout:\n{stdout}"
    );
    assert!(stdout.contains("age"), "stdout:\n{stdout}");
    assert!(
        !stdout.contains("The date range of age is"),
        "stdout:\n{stdout}"
    );
    assert!(!stdout.contains("age as day is"), "stdout:\n{stdout}");
}

#[test]
fn test_cli_explain_conversion_nested_operand() {
    let temp_dir = TempDir::new().unwrap();
    fs::write(
        temp_dir.path().join("test.lemma"),
        r#"
spec test_cli_nested_conversion_explain
data mass: measure
    -> unit kilogram: 1.0
    -> unit gram: 0.001
    -> suggest 2 kilogram
rule result: (mass * 2) as gram
"#,
    )
    .unwrap();

    let mut cmd = cargo_bin_cmd!("lemma");
    cmd.arg("evaluate")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("test_cli_nested_conversion_explain")
        .arg("mass=2 kilogram")
        .arg("--rules=result")
        .arg("--explain");

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "evaluate --explain should succeed: {}",
        stdout
    );
    assert!(
        stdout.contains("1 gram is 0.001 kilogram"),
        "stdout:\n{stdout}"
    );
    assert!(!stdout.contains("The measure is"), "stdout:\n{stdout}");
    assert!(!stdout.contains("4 kilogram"), "stdout:\n{stdout}");
    assert!(stdout.contains("mass"), "stdout:\n{stdout}");
}

#[test]
fn test_cli_run_measure_rule_result_includes_all_units_json() {
    let temp_dir = TempDir::new().unwrap();
    fs::write(
        temp_dir.path().join("money.lemma"),
        r#"
spec money
data price: measure
    -> unit eur: 1
    -> unit usd: 0.91
    -> suggest 100 eur
rule total: price
"#,
    )
    .unwrap();

    let mut cmd = cargo_bin_cmd!("lemma");
    cmd.arg("evaluate")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("money")
        .arg("price=100 eur")
        .arg("--rules=total")
        .arg("--json");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("\"measure\""))
        .stdout(predicate::str::contains("\"eur\""))
        .stdout(predicate::str::contains("\"usd\""));
}

#[test]
fn test_cli_explain_shows_multiply_trace_for_measure_product() {
    let temp_dir = TempDir::new().unwrap();
    fs::write(
        temp_dir.path().join("test.lemma"),
        r#"
spec product_explanation
data price: measure
    -> unit eur: 1
    -> suggest 10 eur
data quantity: number -> suggest 3
rule multiplied: price * quantity
"#,
    )
    .unwrap();

    let mut cmd = cargo_bin_cmd!("lemma");
    cmd.arg("evaluate")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("product_explanation")
        .arg("price=10 eur")
        .arg("quantity=3")
        .arg("--rules=multiplied")
        .arg("--explain");

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "evaluate --explain should succeed: {}",
        stdout
    );
    assert!(
        stdout.contains("price:") && stdout.contains("quantity:"),
        "explain should show both data operands, got:\n{}",
        stdout
    );
}

#[test]
fn test_cli_explain_with_measure_product_preserves_multiply_trace() {
    let temp_dir = TempDir::new().unwrap();
    fs::write(
        temp_dir.path().join("test.lemma"),
        r#"
spec product_as_explanation
data price: measure
    -> unit eur: 1
    -> unit usd: 0.91
    -> suggest 10 eur
data quantity: number -> suggest 3
rule multiplied: price * quantity
"#,
    )
    .unwrap();

    let mut cmd = cargo_bin_cmd!("lemma");
    cmd.arg("evaluate")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("product_as_explanation")
        .arg("price=10 eur")
        .arg("quantity=3")
        .arg("--rules=multiplied")
        .arg("--explain");

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "evaluate --explain should succeed: {}",
        stdout
    );
    assert!(
        stdout.contains("price:") && stdout.contains("quantity:"),
        "explain with --as should still show multiply data operands, got:\n{}",
        stdout
    );
}

#[test]
fn test_cli_run_calc_explain_json_includes_total_explanation() {
    let temp_dir = TempDir::new().unwrap();
    fs::write(
        temp_dir.path().join("calc.lemma"),
        r#"
spec calc

data money: measure
  -> decimals 2
  -> unit eur: 1

data hourly_rate: 85.00 eur
data hours_worked: 37.5
data is_rush: boolean
data is_super_rush: boolean

rule labor: hourly_rate * hours_worked
rule rush_surcharge: 0 eur
  unless is_rush then labor * 25%
  unless is_super_rush then labor * 50%
rule subtotal: labor + rush_surcharge
rule vat: subtotal * 21%
rule total: subtotal + vat
"#,
    )
    .unwrap();

    let mut cmd = cargo_bin_cmd!("lemma");
    cmd.arg("evaluate")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("calc")
        .arg("is_rush=true")
        .arg("is_super_rush=false")
        .arg("--rules=total")
        .arg("--explain")
        .arg("--json");

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "run calc --explain --json should succeed: {}",
        stdout
    );

    let json: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is valid JSON");
    assert_eq!(json["results"]["total"]["explanation"]["type"], "rule");
    assert_eq!(json["results"]["total"]["explanation"]["name"], "total");
    assert_eq!(
        json["results"]["total"]["explanation"]["result"],
        "4821.09 eur"
    );
}

#[test]
fn evaluate_json_without_explain_exposes_rule_missing_data() {
    let temp_dir = TempDir::new().unwrap();
    fs::write(
        temp_dir.path().join("suggest.lemma"),
        r#"
spec suggest_demo
data n: number -> suggest 42
rule r: n
"#,
    )
    .unwrap();

    let mut cmd = cargo_bin_cmd!("lemma");
    cmd.arg("evaluate")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("suggest_demo")
        .arg("--json");

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "evaluate --json without --explain should succeed: {stdout}"
    );

    let json: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is valid JSON");
    assert!(
        json.get("data").is_none(),
        "evaluate JSON must not include top-level data: {json}"
    );
    let results = json["results"]["r"].as_object().expect("rule r in results");
    assert!(
        results.get("explanation").is_none(),
        "explanation must stay stripped without --explain: {results:?}"
    );
    let missing = results["missing_data"]
        .as_array()
        .expect("results.r.missing_data");
    assert!(
        missing.iter().any(|v| v.as_str() == Some("n")),
        "unbound n must appear in missing_data: {missing:?}"
    );
}

#[test]
fn specs_json_emits_spec_version_temporal_rows() {
    let temp_dir = TempDir::new().unwrap();
    fs::write(
        temp_dir.path().join("pricing.lemma"),
        r#"
spec pricing 2025-01-01
data x: 1
rule r: x

spec pricing 2026-01-01
data x: 2
rule r: x
"#,
    )
    .unwrap();

    let mut cmd = cargo_bin_cmd!("lemma");
    cmd.arg("specs")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("--json");

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "specs --json should succeed: {stdout}"
    );

    let json: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is valid JSON");
    let workspace = json
        .as_array()
        .expect("specs is array")
        .iter()
        .find(|g| g["repository"].is_null())
        .expect("workspace group");
    let specs = workspace["specs"].as_array().expect("specs array");
    let pricing: Vec<_> = specs.iter().filter(|s| s["name"] == "pricing").collect();
    assert_eq!(
        pricing.len(),
        2,
        "dual-version pricing must appear as two SpecVersion rows: {workspace}"
    );
    assert!(
        pricing.iter().any(|s| s.get("effective_from").is_some()),
        "SpecVersion rows must carry effective_from: {pricing:?}"
    );
    assert!(
        !json
            .as_array()
            .expect("specs is array")
            .iter()
            .any(|g| g["repository"] == "lemma"),
        "default specs --json omits lemma: {json}"
    );
}

#[test]
fn specs_text_lists_uses_and_all_lists_loaded_repositories() {
    let temp_dir = TempDir::new().unwrap();
    fs::write(
        temp_dir.path().join("compose.lemma"),
        r#"
spec inner
data x: 1

spec outer
uses inner
uses u: lemma units
rule r: inner.x
"#,
    )
    .unwrap();

    let mut default_cmd = cargo_bin_cmd!("lemma");
    default_cmd.arg("specs").arg("--path").arg(temp_dir.path());
    let default_out = default_cmd.output().unwrap();
    let default_text = String::from_utf8_lossy(&default_out.stdout);
    assert!(
        default_out.status.success(),
        "lemma specs should succeed: {default_text}"
    );
    assert_eq!(default_text, "inner\nouter\n  inner\n  lemma units\n");

    let mut all_cmd = cargo_bin_cmd!("lemma");
    all_cmd
        .arg("specs")
        .arg("--all")
        .arg("--path")
        .arg(temp_dir.path());
    let all_out = all_cmd.output().unwrap();
    let all_text = String::from_utf8_lossy(&all_out.stdout);
    assert!(
        all_out.status.success(),
        "lemma specs --all should succeed: {all_text}"
    );
    assert!(
        all_text.starts_with("inner\nouter\n\nlemma\n  units\n"),
        "{all_text}"
    );
    assert!(
        !all_text.contains("  lemma units\n"),
        "--all must not nest uses: {all_text}"
    );

    let mut json_cmd = cargo_bin_cmd!("lemma");
    json_cmd
        .arg("specs")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("--json");
    let json_out = json_cmd.output().unwrap();
    let json_stdout = String::from_utf8_lossy(&json_out.stdout);
    assert!(
        json_out.status.success(),
        "lemma specs --json should succeed: {json_stdout}"
    );
    let json: serde_json::Value = serde_json::from_str(&json_stdout).expect("JSON");
    let workspace = json
        .as_array()
        .expect("array")
        .iter()
        .find(|g| g["repository"].is_null())
        .expect("workspace");
    let outer = workspace["specs"]
        .as_array()
        .expect("specs")
        .iter()
        .find(|s| s["name"] == "outer")
        .expect("outer");
    assert_eq!(
        outer["dependencies"],
        serde_json::json!([
            { "name": "inner" },
            { "repository": "lemma", "name": "units" }
        ])
    );
}

#[test]
fn evaluate_json_incomplete_rule_exposes_results_missing_data() {
    let temp_dir = TempDir::new().unwrap();
    fs::write(
        temp_dir.path().join("incomplete.lemma"),
        r#"
spec incomplete
data a: number
data b: number
rule main: a + b
"#,
    )
    .unwrap();

    let mut cmd = cargo_bin_cmd!("lemma");
    cmd.arg("evaluate")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("incomplete")
        .arg("--json");

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "evaluate --json should succeed: {stdout}"
    );

    let json: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is valid JSON");
    let missing = json["results"]["main"]["missing_data"]
        .as_array()
        .expect("results.main.missing_data must be an array");
    let keys: Vec<&str> = missing.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(
        keys,
        ["a", "b"],
        "smoke: missing_data must list unbound inputs: {json}"
    );
}

const SHIPPING_SPEC: &str = r#"
spec shipping
data weight: measure
  -> unit kilogram: 1
data express: boolean
data note: text
rule heavy: weight > 10 kilogram
rule fast: express
"#;

fn evaluate_with_stdin_json(json: &str, extra: &[&str]) -> assert_cmd::assert::Assert {
    let temp_dir = TempDir::new().unwrap();
    fs::write(temp_dir.path().join("shipping.lemma"), SHIPPING_SPEC).unwrap();
    let mut cmd = cargo_bin_cmd!("lemma");
    cmd.arg("evaluate")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("shipping")
        .arg("-");
    for arg in extra {
        cmd.arg(arg);
    }
    cmd.write_stdin(json.to_string());
    cmd.assert()
}

#[test]
fn evaluate_stdin_data_binds_single_unit_map_and_boolean() {
    evaluate_with_stdin_json(r#"{"weight": {"kilogram": "12"}, "express": true}"#, &[])
        .success()
        .stdout(predicate::str::contains("heavy"))
        .stdout(predicate::str::contains("fast"))
        .stdout(predicate::str::contains("true"));
}

#[test]
fn evaluate_stdin_data_null_is_omitted() {
    let assert =
        evaluate_with_stdin_json(r#"{"weight": null, "express": true}"#, &["--json"]).success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        json["results"]["heavy"]["missing_data"][0], "weight",
        "{json}"
    );
}

#[test]
fn evaluate_stdin_data_number_is_request_error() {
    evaluate_with_stdin_json(r#"{"weight": {"kilogram": 12}}"#, &[])
        .failure()
        .stderr(predicate::str::contains("pass as string: \"12\""));
}

#[test]
fn evaluate_stdin_data_multi_key_unit_map_is_request_error() {
    evaluate_with_stdin_json(r#"{"weight": {"kilogram": "12", "gram": "1"}}"#, &[])
        .failure()
        .stderr(predicate::str::contains("single-unit"));
}

#[test]
fn evaluate_stdin_data_boolean_on_text_is_request_error() {
    evaluate_with_stdin_json(r#"{"note": true}"#, &[])
        .failure()
        .stderr(predicate::str::contains("note"));
}

#[test]
fn evaluate_stdin_data_array_on_scalar_is_request_error() {
    evaluate_with_stdin_json(r#"{"note": ["a"]}"#, &[])
        .failure()
        .stderr(predicate::str::contains("array overlay"));
}

#[test]
fn evaluate_stdin_data_non_object_is_error() {
    evaluate_with_stdin_json(r#"["weight"]"#, &[])
        .failure()
        .stderr(predicate::str::contains("must contain a JSON object"));
}

#[test]
fn evaluate_stdin_data_and_name_value_same_key_is_error() {
    evaluate_with_stdin_json(r#"{"express": true}"#, &["Express=false"])
        .failure()
        .stderr(predicate::str::contains("Duplicate data key 'express'"));
}

#[test]
fn evaluate_stdin_data_combines_with_name_value() {
    evaluate_with_stdin_json(r#"{"express": true}"#, &["weight=12 kilogram"])
        .success()
        .stdout(predicate::str::contains("heavy"));
}

#[test]
fn evaluate_stdin_data_seeds_interactive() {
    evaluate_with_stdin_json(
        r#"{"weight": {"kilogram": "12"}, "express": true}"#,
        &["--interactive", "--rules=heavy,fast"],
    )
    .success()
    .stdout(predicate::str::contains("heavy"))
    .stdout(predicate::str::contains("true"));
}

#[test]
fn evaluate_stdin_data_dash_twice_is_error() {
    let temp_dir = TempDir::new().unwrap();
    fs::write(temp_dir.path().join("shipping.lemma"), SHIPPING_SPEC).unwrap();
    cargo_bin_cmd!("lemma")
        .arg("evaluate")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("shipping")
        .arg("-")
        .arg("-")
        .write_stdin(r#"{"express": true}"#)
        .assert()
        .failure()
        .stderr(predicate::str::contains("`-` given more than once"));
}

#[test]
fn evaluate_stdin_data_empty_is_error() {
    evaluate_with_stdin_json("", &[])
        .failure()
        .stderr(predicate::str::contains("stdin is empty"));
}

#[test]
fn evaluate_without_dash_ignores_piped_stdin() {
    let temp_dir = TempDir::new().unwrap();
    fs::write(temp_dir.path().join("shipping.lemma"), SHIPPING_SPEC).unwrap();
    cargo_bin_cmd!("lemma")
        .arg("evaluate")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("shipping")
        .arg("weight=12 kilogram")
        .arg("express=true")
        .write_stdin("not json")
        .assert()
        .success()
        .stdout(predicate::str::contains("heavy"));
}

#[test]
fn format_rewrites_lemma_fences_in_markdown() {
    let temp_dir = TempDir::new().unwrap();
    let md = temp_dir.path().join("guide.md");
    fs::write(
        &md,
        "# Demo\n\n```lemma\nspec demo\ndata X: 1\nrule Y: X\n```\n",
    )
    .unwrap();

    cargo_bin_cmd!("lemma")
        .arg("format")
        .arg(&md)
        .assert()
        .success()
        .stderr(predicate::str::contains("Formatted:"));

    let rewritten = fs::read_to_string(&md).unwrap();
    assert!(rewritten.contains("data x: 1"));
    assert!(rewritten.contains("rule y:"));
    assert!(rewritten.contains("# Demo"));

    cargo_bin_cmd!("lemma")
        .arg("format")
        .arg("--check")
        .arg(&md)
        .assert()
        .success();
}

#[test]
fn format_check_fails_unformatted_markdown_fence() {
    let temp_dir = TempDir::new().unwrap();
    let md = temp_dir.path().join("guide.md");
    fs::write(&md, "```lemma\nspec demo\ndata X: 1\n```\n").unwrap();

    cargo_bin_cmd!("lemma")
        .arg("format")
        .arg("--check")
        .arg(&md)
        .assert()
        .failure()
        .stderr(predicate::str::contains("Would reformat:"));
}

fn copy_explain_fixture(name: &str) -> TempDir {
    let fixtures = explain_fixtures_dir();
    let temp_dir = TempDir::new().unwrap();
    fs::copy(fixtures.join(name), temp_dir.path().join(name)).unwrap();
    temp_dir
}

fn assert_cli_explain_ascii(stdout: &str, golden: &str) {
    assert_eq!(extract_explain_ascii(stdout), golden);
}

#[test]
fn test_cli_explain_stress_rush_and_cap_ok_ascii() {
    let temp_dir = copy_explain_fixture("stress_explain.lemma");
    let output = cargo_bin_cmd!("lemma")
        .arg("evaluate")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("stress_test_extended")
        .arg("--rules=rush_and_cap_ok")
        .arg("--explain")
        .arg("order.base_price=100")
        .arg("order.quantity=10")
        .arg("order.delivery_distance=50")
        .arg("order.loyalty_points=0")
        .arg("order.package_weight=5")
        .arg("order.customer_tier=vip")
        .arg("order.payment_method=credit")
        .arg("order.is_fragile=false")
        .arg("order.is_express=true")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "evaluate --explain must succeed: {stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_cli_explain_ascii(
        &stdout,
        include_str!("../../../engine/tests/fixtures/explain/stress_rush_and_cap_ok.ascii.txt"),
    );
}

#[test]
fn test_cli_explain_stress_payment_express_noncredit_ascii() {
    let temp_dir = copy_explain_fixture("stress_explain.lemma");
    let output = cargo_bin_cmd!("lemma")
        .arg("evaluate")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("stress_test")
        .arg("--rules=payment_allowed")
        .arg("--explain")
        .arg("base_price=100")
        .arg("quantity=10")
        .arg("delivery_distance=50")
        .arg("loyalty_points=0")
        .arg("package_weight=5")
        .arg("customer_tier=vip")
        .arg("payment_method=debit")
        .arg("is_fragile=false")
        .arg("is_express=true")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "evaluate --explain must succeed: {stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_cli_explain_ascii(
        &stdout,
        include_str!(
            "../../../engine/tests/fixtures/explain/stress_payment_express_noncredit.ascii.txt"
        ),
    );
}

#[test]
fn test_cli_explain_shipping_hazardous_is_not_nl_ascii() {
    let temp_dir = copy_explain_fixture("shipping_explain.lemma");
    let output = cargo_bin_cmd!("lemma")
        .arg("evaluate")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("shipping_policy")
        .arg("--rules=hazardous_fee")
        .arg("--explain")
        .arg("destination_country=BE")
        .arg("customer_tier=standard")
        .arg("item_weight=2")
        .arg("order_total=50")
        .arg("is_expedited=false")
        .arg("is_hazardous=true")
        .arg("is_po_box=false")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "evaluate --explain must succeed: {stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_cli_explain_ascii(
        &stdout,
        include_str!("../../../engine/tests/fixtures/explain/shipping_is_not_nl.ascii.txt"),
    );
}

#[test]
fn test_cli_explain_dates_membership_active_ascii() {
    let temp_dir = copy_explain_fixture("dates_explain.lemma");
    let output = cargo_bin_cmd!("lemma")
        .arg("evaluate")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("date_handling")
        .arg("--rules=membership_active")
        .arg("--explain")
        .arg("--effective=2024-06-15")
        .arg("current_date=2024-06-15")
        .arg("membership_start=2024-01-01")
        .arg("membership_length=1 year")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "evaluate --explain must succeed: {stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_cli_explain_ascii(
        &stdout,
        include_str!("../../../engine/tests/fixtures/explain/dates_membership_active.ascii.txt"),
    );
}

#[test]
fn test_cli_explain_lists_total_ascii() {
    let temp_dir = copy_explain_fixture("lists_explain.lemma");
    let output = cargo_bin_cmd!("lemma")
        .arg("evaluate")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("coffee_order")
        .arg("--rules=total")
        .arg("--explain")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "evaluate --explain must succeed: {stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_cli_explain_ascii(
        &stdout,
        include_str!("../../../engine/tests/fixtures/explain/lists_total.ascii.txt"),
    );
}

#[test]
fn test_cli_explain_ranges_grade_b_ascii() {
    let temp_dir = copy_explain_fixture("ranges_explain.lemma");
    let output = cargo_bin_cmd!("lemma")
        .arg("evaluate")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("grading")
        .arg("--rules=grade")
        .arg("--explain")
        .arg("score=85")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "evaluate --explain must succeed: {stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_cli_explain_ascii(
        &stdout,
        include_str!("../../../engine/tests/fixtures/explain/ranges_grade_b.ascii.txt"),
    );
}

#[test]
fn test_cli_explain_compound_total_payment_ascii() {
    let temp_dir = copy_explain_fixture("compound_units_explain.lemma");
    let output = cargo_bin_cmd!("lemma")
        .arg("evaluate")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("contractor")
        .arg("--rules=total_payment")
        .arg("--explain")
        .arg("hourly_rate=85 eur_per_hour")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "evaluate --explain must succeed: {stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_cli_explain_ascii(
        &stdout,
        include_str!("../../../engine/tests/fixtures/explain/compound_total_payment.ascii.txt"),
    );
}

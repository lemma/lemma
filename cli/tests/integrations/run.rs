use assert_cmd::cargo::cargo_bin_cmd;

use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_cli_run_simple_spec() {
    let temp_dir = TempDir::new().unwrap();
    fs::write(
        temp_dir.path().join("test.lemma"),
        r#"
spec simple_test
data x: 10
data y: 5
rule sum: x + y
rule product: x * y
"#,
    )
    .unwrap();

    let mut cmd = cargo_bin_cmd!("lemma");
    cmd.arg("run")
        .arg("--prefix")
        .arg(temp_dir.path())
        .arg("simple_test");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("sum"))
        .stdout(predicate::str::contains("15"))
        .stdout(predicate::str::contains("product"))
        .stdout(predicate::str::contains("50"));
}

#[test]
fn test_cli_run_data_values() {
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
    cmd.arg("run")
        .arg("--prefix")
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
    cmd.arg("run")
        .arg("--prefix")
        .arg(temp_dir.path())
        .arg("nonexistent");

    cmd.assert().failure().stderr(
        predicate::str::contains("No execution plan").and(predicate::str::contains("nonexistent")),
    );
}

#[test]
fn test_cli_run_rejects_dash_source() {
    let mut cmd = cargo_bin_cmd!("lemma");
    cmd.arg("run").arg("-").arg("any_spec");

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("--prefix"));
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
    cmd.arg("run")
        .arg("--prefix")
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
        .arg("--prefix")
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
fn test_cli_list_summary() {
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
    cmd.arg("list").arg("--prefix").arg(temp_dir.path());

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("spec1"))
        .stdout(predicate::str::contains("spec2"))
        .stdout(predicate::str::contains("Found").not())
        .stdout(predicate::str::contains("data").not());
}

#[test]
fn test_cli_list_scopes_main_repository() {
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
    cmd.arg("list").arg("--prefix").arg(temp_dir.path());

    cmd.assert().success().stdout(
        predicate::str::contains("entry")
            .and(predicate::str::contains("deps_repo"))
            .and(predicate::str::contains("dep_only"))
            .and(predicate::str::contains("Found").not())
            .and(predicate::str::contains("Repositories:").not()),
    );
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
    cmd.arg("run")
        .arg("--prefix")
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
    cmd.arg("run")
        .arg("--prefix")
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
    cmd.arg("run")
        .arg("--prefix")
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
    cmd.arg("run")
        .arg("--prefix")
        .arg(temp_dir.path())
        .arg("data_section")
        .arg("--rules=doubled")
        .arg("--explain");

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "run --explain should succeed: {}",
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
    cmd.arg("run")
        .arg("--prefix")
        .arg(temp_dir.path())
        .arg("settle")
        .arg("--rules=premium")
        .arg("denom=0")
        .arg("--explain");

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "run --explain should succeed: {}",
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
    cmd.arg("run")
        .arg("--prefix")
        .arg(temp_dir.path())
        .arg("open")
        .arg("--rules=main")
        .arg("--explain");

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "run --explain should succeed: {}",
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
    cmd.arg("run")
        .arg("--prefix")
        .arg(temp_dir.path())
        .arg("nested_explanation")
        .arg("--rules=total")
        .arg("--explain");

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "run --explain should succeed: {}",
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
    cmd.arg("run")
        .arg("--prefix")
        .arg(temp_dir.path())
        .arg("explain_test")
        .arg("--explain")
        .arg("--json");

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "run --explain --json should succeed: {}",
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
    cmd.arg("run")
        .arg("--prefix")
        .arg(temp_dir.path())
        .arg("test_cli_conversion_explain")
        .arg("mass=2 kilogram")
        .arg("--rules=result")
        .arg("--explain");

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "run --explain should succeed: {}",
        stdout
    );
    assert!(stdout.contains("2 kilogram"), "stdout:\n{stdout}");
    assert!(
        stdout.contains("1 kilogram is 1000 gram"),
        "stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("The measure of mass is 2 kilogram"),
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
    cmd.arg("run")
        .arg("--prefix")
        .arg(temp_dir.path())
        .arg("test_cli_date_conversion_explain")
        .arg("age=2024-06-01...2024-06-15")
        .arg("--rules=result")
        .arg("--explain");

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "run --explain should succeed: {}",
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
    assert!(
        stdout.contains("The date range of age is"),
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
    cmd.arg("run")
        .arg("--prefix")
        .arg(temp_dir.path())
        .arg("test_cli_nested_conversion_explain")
        .arg("mass=2 kilogram")
        .arg("--rules=result")
        .arg("--explain");

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "run --explain should succeed: {}",
        stdout
    );
    assert!(stdout.contains("4 kilogram"), "stdout:\n{stdout}");
    assert!(
        stdout.contains("1 kilogram is 1000 gram"),
        "stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("The measure is 4 kilogram"),
        "stdout:\n{stdout}"
    );
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
    cmd.arg("run")
        .arg("--prefix")
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
rule product: price * quantity
"#,
    )
    .unwrap();

    let mut cmd = cargo_bin_cmd!("lemma");
    cmd.arg("run")
        .arg("--prefix")
        .arg(temp_dir.path())
        .arg("product_explanation")
        .arg("price=10 eur")
        .arg("quantity=3")
        .arg("--rules=product")
        .arg("--explain");

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "run --explain should succeed: {}",
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
rule product: price * quantity
"#,
    )
    .unwrap();

    let mut cmd = cargo_bin_cmd!("lemma");
    cmd.arg("run")
        .arg("--prefix")
        .arg(temp_dir.path())
        .arg("product_as_explanation")
        .arg("price=10 eur")
        .arg("quantity=3")
        .arg("--rules=product")
        .arg("--explain");

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "run --explain should succeed: {}",
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
    cmd.arg("run")
        .arg("--prefix")
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
fn run_json_without_explain_exposes_rule_missing_data() {
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
    cmd.arg("run")
        .arg("--prefix")
        .arg(temp_dir.path())
        .arg("suggest_demo")
        .arg("--json");

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "run --json without --explain should succeed: {stdout}"
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
fn list_json_emits_listed_spec_temporal_rows() {
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
    cmd.arg("list")
        .arg("--prefix")
        .arg(temp_dir.path())
        .arg("--json");

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "list --json should succeed: {stdout}"
    );

    let json: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is valid JSON");
    let workspace = json
        .as_array()
        .expect("list is array")
        .iter()
        .find(|g| g["repository"].is_null())
        .expect("workspace group");
    let specs = workspace["specs"].as_array().expect("specs array");
    let pricing: Vec<_> = specs.iter().filter(|s| s["name"] == "pricing").collect();
    assert_eq!(
        pricing.len(),
        2,
        "dual-version pricing must appear as two ListedSpec rows: {workspace}"
    );
    assert!(
        pricing.iter().any(|s| s.get("effective_from").is_some()),
        "ListedSpec rows must carry effective_from: {pricing:?}"
    );
}

#[test]
fn run_json_incomplete_rule_exposes_results_missing_data() {
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
    cmd.arg("run")
        .arg("--prefix")
        .arg(temp_dir.path())
        .arg("incomplete")
        .arg("--json");

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "run --json should succeed: {stdout}"
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

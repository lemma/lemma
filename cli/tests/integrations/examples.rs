//! Tests for all CLI integration example files
//!
//! Ensures all example files in cli/tests/integrations/examples/ are valid and can be evaluated
//! through the CLI command interface.

use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

fn examples_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("integrations")
        .join("examples")
}

#[test]
fn test_example_07_shipping_policy() {
    let temp_dir = TempDir::new().unwrap();
    let example_file = examples_dir().join("07_shipping_policy.lemma");

    fs::copy(
        &example_file,
        temp_dir.path().join("07_shipping_policy.lemma"),
    )
    .unwrap();

    let mut cmd = cargo_bin_cmd!("lemma");
    cmd.arg("evaluate")
        .arg("--path")
        .arg(temp_dir.path())
        .arg("shipping_policy")
        .arg("order_total=75.00")
        .arg("item_weight=8")
        .arg("destination_country=NL")
        .arg("destination_region=North Holland")
        .arg("is_po_box=false")
        .arg("is_expedited=false")
        .arg("is_hazardous=false")
        .arg("customer_tier=gold");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("final_shipping"))
        .stdout(predicate::str::contains("23.6")) // NL base 22.00 + weight 7.50 = 29.50, gold discount 20% = 5.90, final = 23.60
        .stdout(predicate::str::contains("estimated_delivery_days"))
        .stdout(predicate::str::contains("2")); // NL delivery is 2 day
}

#[test]
fn test_example_13_temporal_versioning() {
    let temp_dir = TempDir::new().unwrap();
    let example_file = examples_dir().join("13_temporal_versioning.lemma");

    fs::copy(
        &example_file,
        temp_dir.path().join("13_temporal_versioning.lemma"),
    )
    .unwrap();

    let dir = temp_dir.path().to_str().unwrap();

    // 2024: applicant age 28, salary 4000 EUR/month
    // Under-30 threshold in 2024 is 3672 → qualifies
    let output_2024 = std::process::Command::new(env!("CARGO_BIN_EXE_lemma"))
        .args([
            "evaluate",
            "--path",
            dir,
            "kennismigrant_aanvraag",
            "--effective",
            "2024-06",
            "applicant_age=28",
            "gross_monthly_salary=4000 eur",
        ])
        .output()
        .unwrap();

    let stdout_2024 = String::from_utf8_lossy(&output_2024.stdout);
    assert!(
        output_2024.status.success(),
        "2024 eval failed: {}",
        String::from_utf8_lossy(&output_2024.stderr)
    );
    assert!(
        stdout_2024.contains("meets_salary_requirement") && stdout_2024.contains("true"),
        "Expected meets_salary_requirement = true in 2024, got: {}",
        stdout_2024
    );

    // 2025: same salary, but applicant is now 31
    // Age 30+ threshold in 2025 is 5331 → does not qualify
    let output_2025 = std::process::Command::new(env!("CARGO_BIN_EXE_lemma"))
        .args([
            "evaluate",
            "--path",
            dir,
            "kennismigrant_aanvraag",
            "--effective",
            "2025-06",
            "applicant_age=31",
            "gross_monthly_salary=4000 eur",
        ])
        .output()
        .unwrap();

    let stdout_2025 = String::from_utf8_lossy(&output_2025.stdout);
    assert!(
        output_2025.status.success(),
        "2025 eval failed: {}",
        String::from_utf8_lossy(&output_2025.stderr)
    );
    assert!(
        stdout_2025.contains("meets_salary_requirement") && stdout_2025.contains("false"),
        "Expected meets_salary_requirement = false in 2025, got: {}",
        stdout_2025
    );
}

#[test]
fn test_all_examples_parse_via_cli() {
    let temp_dir = TempDir::new().unwrap();
    let examples_path = examples_dir();

    let registry_examples: &[&str] = &["12_registry_references.lemma"];

    for entry in fs::read_dir(&examples_path).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().map(|e| e == "lemma").unwrap_or(false) {
            let filename = path.file_name().unwrap().to_str().unwrap();
            if registry_examples.contains(&filename) {
                continue;
            }
            fs::copy(&path, temp_dir.path().join(filename)).unwrap();
        }
    }

    let mut cmd = cargo_bin_cmd!("lemma");
    cmd.arg("specs").arg("--path").arg(temp_dir.path());

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("simple_data"));
}

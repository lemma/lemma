use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const CRITERION_WARMUP_SECS: &str = "3";
pub const CRITERION_MEASUREMENT_SECS: &str = "5";
pub const ENGINE_CRITERION_MEASUREMENT_SECS: &str = "30";

fn workspace_target_dir(root: &Path) -> PathBuf {
    root.join("target")
}

fn criterion_estimates_path(root: &Path, group: &str, function: &str) -> PathBuf {
    workspace_target_dir(root)
        .join("criterion")
        .join(group)
        .join(function)
        .join("new")
        .join("estimates.json")
}

#[derive(Debug, Clone, Copy)]
pub struct LatencyRow {
    pub median_ns: f64,
    pub std_dev_ns: f64,
}

pub struct EnvironmentInfo {
    pub rustc_version: String,
    pub uname: String,
    pub git_sha: String,
    pub generated_at: String,
}

pub fn capture_environment(root: &Path) -> Result<EnvironmentInfo, String> {
    Ok(EnvironmentInfo {
        rustc_version: capture_stdout("rustc", &["-Vv"], None)?,
        uname: capture_stdout("uname", &["-srm"], None)?,
        git_sha: capture_stdout("git", &["rev-parse", "HEAD"], Some(root))?,
        generated_at: capture_stdout("date", &["-u", "+%Y-%m-%dT%H:%M:%SZ"], None)?,
    })
}

pub fn capture_stdout(program: &str, args: &[&str], cwd: Option<&Path>) -> Result<String, String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    let output = command
        .output()
        .map_err(|error| format!("failed to spawn {program}: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "{program} {args:?} exited {:?}: {}",
            output.status.code(),
            stderr.trim()
        ));
    }
    String::from_utf8(output.stdout)
        .map(|s| s.trim().to_string())
        .map_err(|error| format!("{program} stdout not UTF-8: {error}"))
}

pub fn read_latency_estimate(
    root: &Path,
    group: &str,
    function: &str,
) -> Result<LatencyRow, String> {
    let path = criterion_estimates_path(root, group, function);
    let raw = fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let parsed: Value = serde_json::from_str(&raw)
        .map_err(|error| format!("{}: invalid JSON: {error}", path.display()))?;
    let median_ns = parsed
        .get("median")
        .and_then(|v| v.get("point_estimate"))
        .and_then(Value::as_f64)
        .ok_or_else(|| format!("{}: missing median.point_estimate", path.display()))?;
    let std_dev_ns = parsed
        .get("std_dev")
        .and_then(|v| v.get("point_estimate"))
        .and_then(Value::as_f64)
        .ok_or_else(|| format!("{}: missing std_dev.point_estimate", path.display()))?;
    Ok(LatencyRow {
        median_ns,
        std_dev_ns,
    })
}

pub fn run_criterion_bench(root: &Path, package: &str, bench: &str) -> Result<(), String> {
    run_criterion_bench_with_measurement(root, package, bench, CRITERION_MEASUREMENT_SECS)
}

pub fn run_engine_criterion_bench(root: &Path, package: &str, bench: &str) -> Result<(), String> {
    run_criterion_bench_with_measurement(root, package, bench, ENGINE_CRITERION_MEASUREMENT_SECS)
}

fn run_criterion_bench_with_measurement(
    root: &Path,
    package: &str,
    bench: &str,
    measurement_secs: &str,
) -> Result<(), String> {
    let target_dir = workspace_target_dir(root);
    let status = Command::new("cargo")
        .current_dir(root)
        .env("CARGO_TARGET_DIR", &target_dir)
        .args([
            "bench",
            "-p",
            package,
            "--bench",
            bench,
            "--",
            "--warm-up-time",
            CRITERION_WARMUP_SECS,
            "--measurement-time",
            measurement_secs,
        ])
        .status()
        .map_err(|error| format!("failed to spawn cargo bench {bench}: {error}"))?;
    if !status.success() {
        return Err(format!(
            "cargo bench -p {package} --bench {bench} exited with code {:?}",
            status.code()
        ));
    }
    Ok(())
}

pub fn format_latency_ns(nanoseconds: f64) -> String {
    if nanoseconds >= 1_000_000.0 {
        format!("{:.3} ms", nanoseconds / 1_000_000.0)
    } else if nanoseconds >= 1_000.0 {
        format!("{:.2} us", nanoseconds / 1_000.0)
    } else {
        format!("{nanoseconds:.0} ns")
    }
}

pub fn format_ratio(numerator: f64, denominator: f64) -> String {
    if denominator <= 0.0 || numerator <= 0.0 {
        return "—".to_string();
    }
    let ratio = numerator / denominator;
    if ratio >= 1000.0 {
        format!("{ratio:.0}")
    } else if ratio >= 100.0 {
        format!("{ratio:.1}")
    } else if ratio >= 10.0 {
        format!("{ratio:.2}")
    } else if ratio >= 1.0 {
        format!("{ratio:.3}")
    } else if ratio >= 0.01 {
        format!("{ratio:.4}")
    } else if ratio >= 0.0001 {
        format!("{ratio:.5}")
    } else {
        format!("{ratio:.2e}")
    }
}

pub fn push_environment_block(
    out: &mut String,
    env: &EnvironmentInfo,
    python_version: Option<&str>,
) {
    out.push_str("## Environment\n\n");
    out.push_str(&format!("- Host: `{}`\n", env.uname));
    out.push_str(&format!("- Lemma git SHA: `{}`\n", env.git_sha));
    out.push_str(&format!("- Generated (UTC): `{}`\n", env.generated_at));
    if let Some(version) = python_version {
        out.push_str(&format!("- Python: `{version}`\n"));
    }
    out.push_str("- Rustc:\n\n```\n");
    out.push_str(&env.rustc_version);
    out.push_str("\n```\n\n");
}

/// GitHub blob/tree URL for a workspace-relative path at `git_sha`.
///
/// Documentation is published on lemma.run without the repo tree, so benchmark
/// reports must link to github.com/lemma/lemma at the measured commit rather
/// than using relative paths into this checkout.
pub fn github_source_url(git_sha: &str, path: &str, is_directory: bool) -> String {
    let trimmed = path.trim_start_matches("./").trim_end_matches('/');
    let kind = if is_directory { "tree" } else { "blob" };
    format!("https://github.com/lemma/lemma/{kind}/{git_sha}/{trimmed}")
}

/// Markdown link whose label is the workspace-relative path and whose target is
/// the GitHub URL for that path at `git_sha`.
pub fn github_source_link(git_sha: &str, path: &str, is_directory: bool) -> String {
    let label = path.trim_start_matches("./").trim_end_matches('/');
    let url = github_source_url(git_sha, label, is_directory);
    format!("[`{label}`]({url})")
}

pub fn write_report(root: &Path, relative: &str, report: &str) -> Result<(), String> {
    let out = root.join(relative);
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    fs::write(&out, report).map_err(|error| format!("{}: {error}", out.display()))?;
    eprintln!("benchmarks: wrote {}", out.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn read_latency_estimate_reads_criterion_layout() {
        let base = std::env::temp_dir().join(format!("lemma_bench_test_{}", std::process::id()));
        let estimates = base.join("target/criterion/bench_shipping/evaluate/new");
        fs::create_dir_all(&estimates).expect("mkdir");
        let mut file = fs::File::create(estimates.join("estimates.json")).expect("create");
        writeln!(
            file,
            r#"{{"median":{{"point_estimate":20853.0}},"std_dev":{{"point_estimate":3834.0}}}}"#
        )
        .expect("write");

        let row = read_latency_estimate(&base, "bench_shipping", "evaluate").expect("read");
        assert_eq!(row.median_ns, 20853.0);
        assert_eq!(row.std_dev_ns, 3834.0);
        fs::remove_dir_all(&base).expect("BUG: remove bench test dir");
    }

    #[test]
    fn push_environment_block_lists_generation_time() {
        let env = EnvironmentInfo {
            rustc_version: "rustc test".to_string(),
            uname: "Linux test x86_64".to_string(),
            git_sha: "abc123".to_string(),
            generated_at: "2026-01-01T00:00:00Z".to_string(),
        };
        let mut out = String::new();
        push_environment_block(&mut out, &env, None);
        assert!(
            out.contains("- Generated (UTC): `2026-01-01T00:00:00Z`\n"),
            "{out}"
        );
    }

    #[test]
    fn format_latency_ns_uses_microseconds() {
        assert_eq!(format_latency_ns(22_580.0), "22.58 us");
    }

    #[test]
    fn github_source_link_pins_blob_and_tree_to_sha() {
        assert_eq!(
            github_source_link("abc123", "engine/benches/specs/shipping.lemma", false),
            "[`engine/benches/specs/shipping.lemma`](https://github.com/lemma/lemma/blob/abc123/engine/benches/specs/shipping.lemma)"
        );
        assert_eq!(
            github_source_link("abc123", "engine/benches/python/business_rules/", true),
            "[`engine/benches/python/business_rules`](https://github.com/lemma/lemma/tree/abc123/engine/benches/python/business_rules)"
        );
    }
}

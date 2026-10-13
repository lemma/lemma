//! Workspace test-coverage report orchestration.
//!
//! ```text
//! cargo coverage engine        # lemma-engine lib + integration tests
//! cargo coverage cli           # lemma CLI lib + integration + binary
//! cargo coverage all           # both, sequentially
//! cargo coverage all --check   # verify committed reports match inputs
//! ```
//!
//! Writes reports under `cli/documentation/reference/coverage/`. Any subprocess failure
//! aborts the run; no partial reports.

mod cli;
mod engine;

use crate::benchmarks::common::{capture_environment, EnvironmentInfo};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const REPORT_FORMAT_VERSION: u32 = 1;
const DIGEST_MARKER: &str = "<!-- coverage-input-digest: ";
const COVERAGE_TOOLING_PATHS: &[&str] = &[
    "xtask/src/coverage/mod.rs",
    "xtask/src/coverage/engine.rs",
    "xtask/src/coverage/cli.rs",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Suite {
    Engine,
    Cli,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoverageArgs {
    pub suite: Suite,
    pub check: bool,
}

pub fn parse_suite(args: &[String]) -> Result<Suite, String> {
    match args {
        [] => Err(
            "missing suite: expected one of engine, cli, all (e.g. cargo coverage engine)".into(),
        ),
        [suite] => match suite.as_str() {
            "engine" => Ok(Suite::Engine),
            "cli" => Ok(Suite::Cli),
            "all" => Ok(Suite::All),
            other => Err(format!(
                "unknown suite '{other}': expected one of engine, cli, all"
            )),
        },
        _ => Err("too many arguments: expected exactly one suite (engine, cli, or all)".into()),
    }
}

pub fn parse_args(args: &[String]) -> Result<CoverageArgs, String> {
    let mut check = false;
    let mut positional = Vec::new();
    for arg in args {
        if arg == "--check" {
            check = true;
        } else {
            positional.push(arg.clone());
        }
    }
    Ok(CoverageArgs {
        suite: parse_suite(&positional)?,
        check,
    })
}

pub fn run(root: &Path, args: &[String]) -> Result<(), String> {
    let CoverageArgs { suite, check } = parse_args(args)?;
    if check {
        return check_reports(root, suite);
    }
    require_llvm_cov()?;
    match suite {
        Suite::Engine => engine::run(root).map_err(|e| format!("engine: {e}")),
        Suite::Cli => cli::run(root).map_err(|e| format!("cli: {e}")),
        Suite::All => {
            engine::run(root).map_err(|e| format!("engine: {e}"))?;
            cli::run(root).map_err(|e| format!("cli: {e}"))?;
            Ok(())
        }
    }
}

fn check_reports(root: &Path, suite: Suite) -> Result<(), String> {
    match suite {
        Suite::Engine => engine::check(root),
        Suite::Cli => cli::check(root),
        Suite::All => {
            engine::check(root)?;
            cli::check(root)
        }
    }
}

pub fn check_report(
    root: &Path,
    results_relative: &str,
    input_paths: &[&str],
) -> Result<(), String> {
    let path = root.join(results_relative);
    let content =
        fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let stored = parse_stored_digest(&content)?;
    let expected = compute_input_digest(root, input_paths)?;
    if stored == expected {
        eprintln!("coverage: {results_relative} is up to date");
        Ok(())
    } else {
        Err(format!(
            "{results_relative} is stale; run: cargo coverage {}",
            suite_label_for_report(results_relative)
        ))
    }
}

fn suite_label_for_report(results_relative: &str) -> &'static str {
    if results_relative.ends_with("engine.md") {
        "engine"
    } else {
        "cli"
    }
}

fn require_llvm_cov() -> Result<(), String> {
    let status = Command::new("cargo")
        .args(["llvm-cov", "--version"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("failed to run cargo llvm-cov --version: {error}"))?;
    if !status.success() {
        return Err(
            "cargo-llvm-cov not found; install: cargo install cargo-llvm-cov --locked".into(),
        );
    }
    Ok(())
}

pub struct ReportConfig<'a> {
    pub command_label: &'a str,
    pub nav_title: &'a str,
    pub nav_order: u32,
    pub title: &'a str,
    pub results_relative: &'a str,
    pub src_prefix: &'a str,
    pub input_paths: &'a [&'a str],
    pub methodology: &'a str,
    pub out_of_scope: &'a str,
    pub related_docs: &'a str,
}

pub fn run_coverage_and_write(
    root: &Path,
    config: &ReportConfig<'_>,
    package: &str,
    llvm_args: &[&str],
) -> Result<(), String> {
    clean_llvm_cov(root, package)?;
    let output = run_llvm_cov(root, llvm_args)?;
    let combined = combine_output(&output);
    let test_run = parse_nextest_summary(&combined)?;
    let report = parse_llvm_cov_json(&combined)?;
    let env = capture_environment(root)?;
    let markdown = compose_report(config, &env, &report, &test_run)?;
    let digest = compute_input_digest(root, config.input_paths)?;
    let markdown = embed_input_digest(&markdown, &digest);
    let out = root.join(config.results_relative);
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    fs::write(&out, markdown).map_err(|error| format!("{}: {error}", out.display()))?;
    eprintln!("coverage: wrote {}", out.display());
    Ok(())
}

pub fn compute_input_digest(root: &Path, input_paths: &[&str]) -> Result<String, String> {
    let mut paths = Vec::new();
    for path in input_paths {
        collect_input_paths(root, Path::new(path), &mut paths)?;
    }
    for path in COVERAGE_TOOLING_PATHS {
        let full = root.join(path);
        if full.is_file() {
            paths.push(full);
        }
    }
    paths.sort();
    paths.dedup();

    let mut hash = FNV1A64::new();
    hash.write_u32(REPORT_FORMAT_VERSION);
    for path in paths {
        let relative = path
            .strip_prefix(root)
            .map_err(|error| format!("{}: {error}", path.display()))?
            .to_string_lossy()
            .replace('\\', "/");
        hash.write(relative.as_bytes());
        hash.write([0_u8]);
        let bytes = fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        hash.write(&bytes);
        hash.write([0_u8]);
    }
    Ok(hash.finish_hex())
}

fn collect_input_paths(root: &Path, path: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let full = root.join(path);
    if !full.exists() {
        return Err(format!("coverage input path missing: {}", full.display()));
    }
    if full.is_file() {
        out.push(full);
        return Ok(());
    }
    let entries = fs::read_dir(&full).map_err(|error| format!("{}: {error}", full.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("{}: {error}", full.display()))?;
        let entry_path = entry.path();
        if entry_path.is_dir() {
            let relative = entry_path
                .strip_prefix(root)
                .map_err(|error| format!("{}: {error}", entry_path.display()))?;
            collect_input_paths(root, relative, out)?;
        } else if entry_path.is_file() {
            out.push(entry_path);
        }
    }
    Ok(())
}

fn embed_input_digest(markdown: &str, digest: &str) -> String {
    format!("{markdown}{DIGEST_MARKER}{digest} -->\n")
}

fn parse_stored_digest(content: &str) -> Result<String, String> {
    for line in content.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix(DIGEST_MARKER) {
            let digest = rest
                .strip_suffix("-->")
                .ok_or_else(|| format!("malformed digest marker in {line}"))?
                .trim();
            if digest.is_empty() {
                return Err("empty coverage-input-digest".into());
            }
            return Ok(digest.to_string());
        }
    }
    Err("missing coverage-input-digest marker; run cargo coverage".into())
}

struct FNV1A64 {
    state: u64,
}

impl FNV1A64 {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x00000100000001b3;

    fn new() -> Self {
        Self {
            state: Self::OFFSET,
        }
    }

    fn write(&mut self, bytes: impl AsRef<[u8]>) {
        for byte in bytes.as_ref() {
            self.state ^= u64::from(*byte);
            self.state = self.state.wrapping_mul(Self::PRIME);
        }
    }

    fn write_u32(&mut self, value: u32) {
        self.write(value.to_le_bytes());
    }

    fn finish_hex(self) -> String {
        format!("{:016x}", self.state)
    }
}

fn clean_llvm_cov(root: &Path, package: &str) -> Result<(), String> {
    let status = Command::new("cargo")
        .current_dir(root)
        .args(["llvm-cov", "clean", "-p", package])
        .status()
        .map_err(|error| format!("failed to spawn cargo llvm-cov clean -p {package}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "cargo llvm-cov clean -p {package} exited with code {:?}",
            status.code()
        ))
    }
}

fn run_llvm_cov(root: &Path, args: &[&str]) -> Result<Output, String> {
    let mut command = Command::new("cargo");
    command
        .current_dir(root)
        .arg("llvm-cov")
        .arg("nextest")
        .args(args)
        .arg("--json")
        .arg("--summary-only")
        .env("NEXTEST_TEST_THREADS", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
        .output()
        .map_err(|error| format!("failed to spawn cargo llvm-cov: {error}"))
        .and_then(|output| {
            if output.status.success() {
                Ok(output)
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                Err(format!(
                    "cargo llvm-cov exited {:?}: {}",
                    output.status.code(),
                    stderr.trim()
                ))
            }
        })
}

fn combine_output(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.is_empty() {
        stdout.into_owned()
    } else if stdout.is_empty() {
        stderr.into_owned()
    } else {
        format!("{stderr}{stdout}")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestRunSummary {
    pub total: u32,
    pub passed: u32,
    pub skipped: u32,
    pub failed: u32,
}

pub fn parse_nextest_summary(output: &str) -> Result<TestRunSummary, String> {
    for line in output.lines().rev() {
        let Some(rest) = line.trim().strip_prefix("Summary [") else {
            continue;
        };
        let Some(summary) = rest.split(']').nth(1) else {
            continue;
        };
        let summary = summary.trim();
        let (total, passed, skipped, failed) = parse_test_counts(summary)?;
        return Ok(TestRunSummary {
            total,
            passed,
            skipped,
            failed,
        });
    }
    Err("nextest summary line not found in llvm-cov output".into())
}

fn parse_test_counts(summary: &str) -> Result<(u32, u32, u32, u32), String> {
    let run_part = summary
        .split(':')
        .next()
        .ok_or_else(|| format!("invalid nextest summary: {summary}"))?
        .trim();
    let total = run_part
        .split_whitespace()
        .next()
        .ok_or_else(|| format!("invalid nextest summary run count: {summary}"))?
        .parse::<u32>()
        .map_err(|error| format!("invalid nextest total: {error}"))?;

    let mut passed = 0_u32;
    let mut skipped = 0_u32;
    let mut failed = 0_u32;
    for token in summary.split([',', ':']).map(str::trim) {
        if let Some(n) = token.strip_suffix(" passed") {
            passed = n
                .parse()
                .map_err(|e| format!("invalid passed count: {e}"))?;
        } else if let Some(n) = token.strip_suffix(" skipped") {
            skipped = n
                .parse()
                .map_err(|e| format!("invalid skipped count: {e}"))?;
        } else if let Some(n) = token.strip_suffix(" failed") {
            failed = n
                .parse()
                .map_err(|e| format!("invalid failed count: {e}"))?;
        }
    }
    if passed == 0 && failed == 0 {
        return Err(format!("could not parse pass/skip/fail from: {summary}"));
    }
    Ok((total, passed, skipped, failed))
}

#[derive(Debug, Deserialize)]
struct LlcovExport {
    data: Vec<LlcovData>,
}

#[derive(Debug, Deserialize)]
struct LlcovData {
    files: Vec<LlcovFile>,
    totals: MetricSummary,
}

#[derive(Debug, Deserialize)]
struct LlcovFile {
    filename: String,
    summary: MetricSummary,
}

#[derive(Debug, Deserialize, Clone)]
struct MetricSummary {
    lines: MetricCount,
    functions: MetricCount,
    regions: MetricCount,
}

#[derive(Debug, Deserialize, Clone, Copy)]
struct MetricCount {
    count: u64,
    covered: u64,
    percent: f64,
}

#[derive(Debug, Clone)]
pub struct ModuleCoverage {
    module: String,
    lines: MetricCount,
    functions: MetricCount,
    regions: MetricCount,
}

#[derive(Debug, Clone)]
pub struct CoverageReport {
    totals: MetricSummary,
    modules: Vec<ModuleCoverage>,
}

pub fn parse_llvm_cov_json(output: &str) -> Result<CoverageReport, String> {
    let json = extract_json_blob(output)?;
    let export: LlcovExport =
        serde_json::from_str(&json).map_err(|error| format!("invalid llvm-cov JSON: {error}"))?;
    let data = export
        .data
        .into_iter()
        .next()
        .ok_or_else(|| "llvm-cov JSON missing data section".to_string())?;
    Ok(CoverageReport {
        totals: data.totals,
        modules: data
            .files
            .into_iter()
            .map(|file| ModuleCoverage {
                module: file.filename,
                lines: file.summary.lines,
                functions: file.summary.functions,
                regions: file.summary.regions,
            })
            .collect(),
    })
}

fn extract_json_blob(output: &str) -> Result<String, String> {
    let start = output
        .find("{\"data\":")
        .ok_or_else(|| "llvm-cov JSON blob not found in command output".to_string())?;
    Ok(output[start..].trim().to_string())
}

pub fn filter_modules(report: &CoverageReport, src_prefix: &str) -> CoverageReport {
    let prefix = normalize_src_prefix(src_prefix);
    let modules = report
        .modules
        .iter()
        .filter_map(|module| {
            relative_module_path(&module.module, &prefix).map(|path| ModuleCoverage {
                module: path,
                lines: module.lines,
                functions: module.functions,
                regions: module.regions,
            })
        })
        .collect();
    CoverageReport {
        totals: report.totals.clone(),
        modules,
    }
}

fn normalize_src_prefix(prefix: &str) -> String {
    prefix.trim_end_matches('/').replace('\\', "/")
}

fn relative_module_path(filename: &str, src_prefix: &str) -> Option<String> {
    let path = filename.replace('\\', "/");
    let marker = format!("{src_prefix}/");
    path.split(&marker).nth(1).map(str::to_string)
}

pub fn compose_report(
    config: &ReportConfig<'_>,
    env: &EnvironmentInfo,
    report: &CoverageReport,
    test_run: &TestRunSummary,
) -> Result<String, String> {
    let filtered = filter_modules(report, config.src_prefix);
    if filtered.modules.is_empty() {
        return Err(format!(
            "no source files matched prefix {} in llvm-cov output",
            config.src_prefix
        ));
    }

    let mut modules = filtered.modules;
    modules.sort_by(|a, b| {
        a.lines
            .percent
            .partial_cmp(&b.lines.percent)
            .expect("BUG: coverage line percent must not be NaN")
            .then_with(|| a.module.cmp(&b.module))
    });

    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!("nav_title: {}\n", config.nav_title));
    out.push_str("parent: Reference\n");
    out.push_str(&format!("nav_order: {}\n", config.nav_order));
    out.push_str("---\n\n");
    out.push_str(&format!("# {}\n\n", config.title));
    out.push_str(&format!(
        "Numbers are produced by `cargo coverage {}`.\n\n",
        config.command_label
    ));

    out.push_str("## Methodology\n\n");
    out.push_str(config.methodology);
    out.push_str("\n\n");
    out.push_str(config.out_of_scope);
    out.push_str("\n\n");

    push_coverage_environment_block(&mut out, env);

    out.push_str("## Summary\n\n");
    out.push_str("| Metric | Covered | Total | Percent |\n");
    out.push_str("|--------|--------:|------:|--------:|\n");
    let totals = aggregate_totals(&modules);
    push_metric_row(&mut out, "Lines", totals.lines);
    push_metric_row(&mut out, "Functions", totals.functions);
    push_metric_row(&mut out, "Regions", totals.regions);
    out.push('\n');

    out.push_str("## Test run\n\n");
    out.push_str(&format!(
        "- Total: {}\n- Passed: {}\n- Skipped: {}\n- Failed: {}\n\n",
        test_run.total, test_run.passed, test_run.skipped, test_run.failed
    ));

    out.push_str("## Per-module coverage\n\n");
    out.push_str(
        "Sorted by line coverage ascending (weakest first). Only files under \
         `src/` for this crate are listed.\n\n",
    );
    out.push_str("| Module | Line % | Function % | Region % | Lines covered/total |\n");
    out.push_str("|--------|-------:|-----------:|---------:|--------------------:|\n");
    for module in &modules {
        out.push_str(&format!(
            "| `{}` | {:.2} | {:.2} | {:.2} | {}/{} |\n",
            module.module,
            module.lines.percent,
            module.functions.percent,
            module.regions.percent,
            module.lines.covered,
            module.lines.count,
        ));
    }
    out.push('\n');

    out.push_str("## Related docs\n\n");
    out.push_str(config.related_docs);
    out.push('\n');

    Ok(out)
}

fn push_coverage_environment_block(out: &mut String, env: &EnvironmentInfo) {
    out.push_str("## Environment\n\n");
    out.push_str(&format!("- Host: `{}`\n", env.uname));
    out.push_str("- Rustc:\n\n```\n");
    out.push_str(&env.rustc_version);
    out.push_str("\n```\n\n");
}

fn push_metric_row(out: &mut String, label: &str, metric: MetricCount) {
    out.push_str(&format!(
        "| {label} | {} | {} | {:.2}% |\n",
        metric.covered, metric.count, metric.percent
    ));
}

fn aggregate_totals(modules: &[ModuleCoverage]) -> MetricSummary {
    let mut lines = MetricCount {
        count: 0,
        covered: 0,
        percent: 0.0,
    };
    let mut functions = MetricCount {
        count: 0,
        covered: 0,
        percent: 0.0,
    };
    let mut regions = MetricCount {
        count: 0,
        covered: 0,
        percent: 0.0,
    };
    for module in modules {
        lines.count += module.lines.count;
        lines.covered += module.lines.covered;
        functions.count += module.functions.count;
        functions.covered += module.functions.covered;
        regions.count += module.regions.count;
        regions.covered += module.regions.covered;
    }
    lines.percent = percent(lines.covered, lines.count);
    functions.percent = percent(functions.covered, functions.count);
    regions.percent = percent(regions.covered, regions.count);
    MetricSummary {
        lines,
        functions,
        regions,
    }
}

fn percent(covered: u64, count: u64) -> f64 {
    if count == 0 {
        panic!("BUG: coverage aggregate has zero items to cover; percent is undefined");
    }
    (covered as f64) * 100.0 / (count as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_JSON: &str = r#"{"data":[{"files":[{"filename":"/repo/engine/src/planning/graph.rs","summary":{"lines":{"count":100,"covered":80,"percent":80.0},"functions":{"count":10,"covered":8,"percent":80.0},"regions":{"count":120,"covered":96,"percent":80.0}}},{"filename":"/repo/engine/src/evaluation/operations.rs","summary":{"lines":{"count":50,"covered":25,"percent":50.0},"functions":{"count":5,"covered":2,"percent":40.0},"regions":{"count":60,"covered":30,"percent":50.0}}}],"totals":{"lines":{"count":150,"covered":105,"percent":70.0},"functions":{"count":15,"covered":10,"percent":66.67},"regions":{"count":180,"covered":126,"percent":70.0}}}],"type":"llvm.coverage.json.export","version":"3.0.1"}"#;

    #[test]
    fn parse_nextest_summary_reads_counts() {
        let output = "noise\n     Summary [  10.325s] 2018 tests run: 2018 passed, 2 skipped\n";
        let summary = parse_nextest_summary(output).expect("parse");
        assert_eq!(summary.total, 2018);
        assert_eq!(summary.passed, 2018);
        assert_eq!(summary.skipped, 2);
        assert_eq!(summary.failed, 0);
    }

    #[test]
    fn parse_llvm_cov_json_reads_files_and_totals() {
        let report = parse_llvm_cov_json(SAMPLE_JSON).expect("parse");
        assert_eq!(report.modules.len(), 2);
        assert!((report.totals.lines.percent - 70.0).abs() < f64::EPSILON);
    }

    #[test]
    fn filter_modules_keeps_src_prefix_only() {
        let report = parse_llvm_cov_json(SAMPLE_JSON).expect("parse");
        let filtered = filter_modules(&report, "engine/src");
        assert_eq!(filtered.modules.len(), 2);
        assert_eq!(filtered.modules[0].module, "planning/graph.rs");
    }

    #[test]
    fn compose_report_includes_digest_marker() {
        let report = parse_llvm_cov_json(SAMPLE_JSON).expect("parse");
        let test_run = TestRunSummary {
            total: 10,
            passed: 10,
            skipped: 0,
            failed: 0,
        };
        let env = EnvironmentInfo {
            rustc_version: "rustc 1.92.0".into(),
            uname: "Linux x86_64".into(),
            git_sha: String::new(),
            generated_at: String::new(),
        };
        let config = ReportConfig {
            command_label: "engine",
            nav_title: "Engine test coverage",
            nav_order: 55,
            title: "Engine test coverage",
            results_relative: "cli/documentation/reference/coverage/engine.md",
            src_prefix: "engine/src",
            input_paths: engine::INPUT_PATHS,
            methodology: "- Tooling\n",
            out_of_scope: "- wasm32\n",
            related_docs: "- [engine/tests/README.md](engine/tests/README.md)\n",
        };
        let markdown = compose_report(&config, &env, &report, &test_run).expect("compose");
        assert!(markdown.contains("cargo coverage engine"));
        assert!(markdown.contains("evaluation/operations.rs"));
        assert!(!markdown.contains("## Gaps"));
    }

    #[test]
    fn parse_args_accepts_check_flag() {
        let args = parse_args(&["all".into(), "--check".into()]).expect("parse");
        assert_eq!(args.suite, Suite::All);
        assert!(args.check);
    }

    #[test]
    fn embed_and_parse_digest_roundtrip() {
        let digest = "abc123";
        let markdown = embed_input_digest("# Title\n\n", digest);
        assert_eq!(parse_stored_digest(&markdown).expect("parse"), digest);
    }

    #[test]
    fn parse_suite_accepts_known_suites() {
        assert_eq!(parse_suite(&["engine".into()]).unwrap(), Suite::Engine);
        assert_eq!(parse_suite(&["cli".into()]).unwrap(), Suite::Cli);
        assert_eq!(parse_suite(&["all".into()]).unwrap(), Suite::All);
    }
}

//! Engine evaluation benchmark report.
//!
//! Run via `cargo benchmarks engine`. Keep [`FIXTURES`] in sync with
//! `engine/benches/common/mod.rs`.

use super::common::{
    capture_environment, capture_stdout, format_latency_ns, format_ratio, github_source_link,
    push_environment_block, read_latency_estimate, run_engine_criterion_bench, write_report,
    EnvironmentInfo, LatencyRow,
};
use rust_decimal::Decimal;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Stdio};
use std::str::FromStr;

/// Spec metadata for the engine benchmark report.
/// Order here is the order rendered in the report.
pub const FIXTURES: &[Fixture] = &[
    Fixture {
        spec_name: "bench_shipping",
        lemma_path: "engine/benches/specs/shipping.lemma",
        python_module: "business_rules.shipping",
        inputs: &[
            ("weight", "3"),
            ("destination", "domestic"),
            ("is_member", "false"),
        ],
    },
    Fixture {
        spec_name: "bench_pricing",
        lemma_path: "engine/benches/specs/pricing.lemma",
        python_module: "business_rules.pricing",
        inputs: &[
            ("product_type", "premium"),
            ("quantity", "25"),
            ("unit_price", "100"),
            ("coupon_percent", "5"),
            ("loyalty_years", "2"),
            ("is_member", "true"),
            ("is_loyalty", "true"),
            ("is_tax_exempt", "false"),
        ],
    },
    Fixture {
        spec_name: "bench_order_pipeline",
        lemma_path: "engine/benches/specs/order_pipeline.lemma",
        python_module: "business_rules.order_pipeline",
        inputs: &[
            ("customer_tier", "gold"),
            ("payment_method", "credit"),
            ("shipping_zone", "national"),
            ("quantity", "12"),
            ("unit_price", "85"),
            ("package_weight", "3.5"),
            ("delivery_distance", "180"),
            ("loyalty_points", "6500"),
            ("coupon_percent", "10"),
            ("is_fragile", "true"),
            ("is_express", "true"),
            ("is_hazardous", "false"),
            ("is_gift", "false"),
            ("is_first_time", "false"),
        ],
    },
];

pub struct Fixture {
    pub spec_name: &'static str,
    pub lemma_path: &'static str,
    pub python_module: &'static str,
    pub inputs: &'static [(&'static str, &'static str)],
}

const BENCH_EFFECTIVE_ISO: &str = "2026-01-01T00:00:00Z";
pub const RESULTS_RELATIVE: &str = "cli/documentation/reference/benchmarks/engine.md";
const PYTHON_BENCH_RELATIVE: &str = "engine/benches/python/benchmark.py";

#[derive(Debug, Deserialize)]
struct PythonReport {
    fixtures: Vec<PythonFixture>,
}

#[derive(Debug, Deserialize)]
struct PythonFixture {
    spec_name: String,
    iterations_latency: u64,
    latency_median_ns: f64,
    latency_std_dev_ns: f64,
    outputs: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct OutputsReport {
    fixtures: Vec<OutputsFixture>,
}

#[derive(Debug, Deserialize)]
struct OutputsFixture {
    spec_name: String,
    outputs: BTreeMap<String, LemmaOutput>,
}

#[derive(Debug, Deserialize)]
struct LemmaOutput {
    kind: String,
    value: String,
    unit: Option<String>,
}

pub fn run(root: &Path) -> Result<(), String> {
    require_python3()?;

    run_engine_criterion_bench(root, "lemma-engine", "evaluate")?;
    let memory_stdout = run_memory_bench(root)?;
    let logistics_stdout = run_logistics_bench(root)?;
    let outputs_stdout = run_outputs_bench(root)?;
    let outputs_report: OutputsReport = serde_json::from_str(outputs_stdout.trim())
        .map_err(|error| format!("outputs bench stdout was not valid JSON: {error}"))?;
    let python_stdout = run_python_benchmark(root)?;
    let python_report: PythonReport = serde_json::from_str(python_stdout.trim())
        .map_err(|error| format!("python benchmark stdout was not valid JSON: {error}"))?;

    let mut latency_rows: BTreeMap<&'static str, LatencyRow> = BTreeMap::new();
    let mut explain_latency_rows: BTreeMap<&'static str, LatencyRow> = BTreeMap::new();
    let mut compile_rows: BTreeMap<&'static str, LatencyRow> = BTreeMap::new();
    for fixture in FIXTURES {
        let untraced = read_latency_estimate(root, fixture.spec_name, "evaluate")?;
        latency_rows.insert(fixture.spec_name, untraced);
        let explain = read_latency_estimate(root, fixture.spec_name, "evaluate_explain")?;
        explain_latency_rows.insert(fixture.spec_name, explain);
        let compile = read_latency_estimate(root, fixture.spec_name, "plan")?;
        compile_rows.insert(fixture.spec_name, compile);
    }

    let python_by_spec: BTreeMap<String, &PythonFixture> = python_report
        .fixtures
        .iter()
        .map(|f| (f.spec_name.clone(), f))
        .collect();
    let outputs_by_spec: BTreeMap<String, &OutputsFixture> = outputs_report
        .fixtures
        .iter()
        .map(|f| (f.spec_name.clone(), f))
        .collect();
    for fixture in FIXTURES {
        if !python_by_spec.contains_key(fixture.spec_name) {
            return Err(format!(
                "python benchmark JSON missing spec '{}'",
                fixture.spec_name
            ));
        }
        if !outputs_by_spec.contains_key(fixture.spec_name) {
            return Err(format!(
                "outputs bench JSON missing spec '{}'",
                fixture.spec_name
            ));
        }
    }

    let accuracy = compute_accuracy(&outputs_by_spec, &python_by_spec)?;
    let (memory_rows, plan_memory_rows) = parse_memory_bench_stdout(&memory_stdout)?;
    let snapshot_rows = parse_snapshot_bench_stdout(&logistics_stdout)?;
    let external_rows = parse_logistics_external_bench_stdout(&logistics_stdout)?;
    let env = capture_environment(root)?;
    let python_version = capture_stdout("python3", &["--version"], None)?;

    let report = compose_report(ComposeReportContext {
        env: &env,
        python_version: &python_version,
        latency_rows: &latency_rows,
        explain_latency_rows: &explain_latency_rows,
        compile_rows: &compile_rows,
        python_by_spec: &python_by_spec,
        accuracy: &accuracy,
        memory_rows: &memory_rows,
        plan_memory_rows: &plan_memory_rows,
        snapshot_rows: &snapshot_rows,
        external_rows: &external_rows,
    })?;

    write_report(root, RESULTS_RELATIVE, &report)
}

fn require_python3() -> Result<(), String> {
    let ok = Command::new("python3")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if !crate::probe_succeeded(ok, "python3") {
        return Err(
            "python3 not found on PATH; install Python 3.11+ before running benchmarks engine"
                .into(),
        );
    }
    Ok(())
}

fn run_memory_bench(root: &Path) -> Result<String, String> {
    let output = Command::new("cargo")
        .current_dir(root)
        .env("CARGO_TARGET_DIR", root.join("target"))
        .args(["bench", "-p", "lemma-engine", "--bench", "memory"])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|error| format!("failed to spawn cargo bench memory: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo bench memory exited with code {:?}",
            output.status.code()
        ));
    }
    String::from_utf8(output.stdout)
        .map_err(|error| format!("memory bench stdout was not UTF-8: {error}"))
}

/// Eval rows, then plan rows. The memory bench prints the eval table before the plan table;
/// each table's header names its unit (`Allocations/eval`, `Allocations/plan`).
fn parse_memory_bench_stdout(stdout: &str) -> Result<(Vec<MemoryRow>, Vec<MemoryRow>), String> {
    let mut eval_rows = Vec::new();
    let mut plan_rows = Vec::new();
    let mut current: Option<&mut Vec<MemoryRow>> = None;
    for line in stdout.lines() {
        let line = line.trim();
        if line.starts_with("| Spec |") {
            current = match line.split('|').nth(3).map(str::trim) {
                Some("Allocations/eval") => Some(&mut eval_rows),
                Some("Allocations/plan") => Some(&mut plan_rows),
                other => return Err(format!("BUG: unknown memory bench table header {other:?}")),
            };
            continue;
        }
        if !line.starts_with("| bench_") {
            continue;
        }
        let rows = current
            .as_deref_mut()
            .ok_or_else(|| format!("BUG: memory bench row before any table header: {line}"))?;
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        if cells.len() < 6 {
            return Err(format!("BUG: malformed memory bench row: {line}"));
        }
        let spec_name = cells[1];
        let iterations = cells[2]
            .parse::<usize>()
            .map_err(|error| format!("BUG: memory iterations parse failed: {error}"))?;
        let allocations_per_iteration = cells[3]
            .parse::<f64>()
            .map_err(|error| format!("BUG: allocations parse failed: {error}"))?;
        let bytes_allocated_per_iteration = cells[4]
            .parse::<f64>()
            .map_err(|error| format!("BUG: bytes allocated parse failed: {error}"))?;
        let reallocations_per_iteration = cells[5]
            .parse::<f64>()
            .map_err(|error| format!("BUG: reallocations parse failed: {error}"))?;
        let fixture = FIXTURES
            .iter()
            .find(|fixture| fixture.spec_name == spec_name)
            .ok_or_else(|| format!("memory bench reported unknown spec '{spec_name}'"))?;
        rows.push(MemoryRow {
            spec_name: fixture.spec_name,
            iterations,
            allocations_per_iteration,
            bytes_allocated_per_iteration,
            reallocations_per_iteration,
        });
    }
    for (unit, rows) in [("eval", &eval_rows), ("plan", &plan_rows)] {
        if rows.len() != FIXTURES.len() {
            return Err(format!(
                "memory bench reported {} {unit} rows, expected {}",
                rows.len(),
                FIXTURES.len()
            ));
        }
    }
    Ok((eval_rows, plan_rows))
}

/// Rungs of the logistics ladder the logistics bench must report, in order.
const SNAPSHOT_LADDER: [usize; 4] = [1050, 6300, 18900, 126000];

fn run_logistics_bench(root: &Path) -> Result<String, String> {
    let output = Command::new("cargo")
        .current_dir(root)
        .env("CARGO_TARGET_DIR", root.join("target"))
        .args(["bench", "-p", "lemma-engine", "--bench", "logistics"])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|error| format!("failed to spawn cargo bench logistics: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo bench logistics exited with code {:?}",
            output.status.code()
        ));
    }
    String::from_utf8(output.stdout)
        .map_err(|error| format!("logistics bench stdout was not UTF-8: {error}"))
}

fn parse_milliseconds(cell: &str, what: &str) -> Result<f64, String> {
    cell.strip_suffix(" ms")
        .ok_or_else(|| format!("BUG: {what} cell '{cell}' lacks ' ms' suffix"))?
        .parse::<f64>()
        .map_err(|error| format!("BUG: {what} parse failed: {error}"))
}

fn parse_snapshot_bench_stdout(stdout: &str) -> Result<Vec<SnapshotRow>, String> {
    let mut rows = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        if !line.starts_with("| logistics_") || line.starts_with("| logistics_ext_") {
            continue;
        }
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        if cells.len() < 13 {
            return Err(format!("BUG: malformed snapshot bench row: {line}"));
        }
        rows.push(SnapshotRow {
            profile: cells[1].to_string(),
            rate_cells: cells[2]
                .parse::<usize>()
                .map_err(|error| format!("BUG: rate cells parse failed: {error}"))?,
            source_bytes: cells[3]
                .parse::<usize>()
                .map_err(|error| format!("BUG: source bytes parse failed: {error}"))?,
            load_ms: parse_milliseconds(cells[4], "load")?,
            loaded_heap_bytes: cells[5]
                .parse::<i128>()
                .map_err(|error| format!("BUG: loaded heap parse failed: {error}"))?,
            snapshot_bytes: cells[6]
                .parse::<usize>()
                .map_err(|error| format!("BUG: snapshot bytes parse failed: {error}"))?,
            encode_ms: parse_milliseconds(cells[7], "encode")?,
            restore_ms: parse_milliseconds(cells[8], "restore")?,
            eval_ms: parse_milliseconds(cells[9], "eval")?,
            restore_allocations: cells[10]
                .parse::<usize>()
                .map_err(|error| format!("BUG: allocations/restore parse failed: {error}"))?,
            restored_heap_bytes: cells[11]
                .parse::<i128>()
                .map_err(|error| format!("BUG: restored heap parse failed: {error}"))?,
        });
    }
    let reported: Vec<usize> = rows.iter().map(|row| row.rate_cells).collect();
    if reported != SNAPSHOT_LADDER {
        return Err(format!(
            "snapshot bench reported rungs {reported:?}, expected {SNAPSHOT_LADDER:?}"
        ));
    }
    Ok(rows)
}

fn parse_logistics_external_bench_stdout(stdout: &str) -> Result<Vec<ExternalCatalogRow>, String> {
    let mut rows = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        if !line.starts_with("| logistics_ext_") {
            continue;
        }
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        if cells.len() < 12 {
            return Err(format!(
                "BUG: malformed logistics_external bench row: {line}"
            ));
        }
        rows.push(ExternalCatalogRow {
            profile: cells[1].to_string(),
            rate_cells: cells[2]
                .parse::<usize>()
                .map_err(|error| format!("BUG: rate cells parse failed: {error}"))?,
            source_bytes: cells[3]
                .parse::<usize>()
                .map_err(|error| format!("BUG: source bytes parse failed: {error}"))?,
            load_ms: parse_milliseconds(cells[4], "load")?,
            loaded_heap_bytes: cells[5]
                .parse::<i128>()
                .map_err(|error| format!("BUG: loaded heap parse failed: {error}"))?,
            snapshot_bytes: cells[6]
                .parse::<usize>()
                .map_err(|error| format!("BUG: snapshot bytes parse failed: {error}"))?,
            encode_ms: parse_milliseconds(cells[7], "encode")?,
            restore_ms: parse_milliseconds(cells[8], "restore")?,
            eval_ms: parse_milliseconds(cells[9], "eval")?,
            restored_heap_bytes: cells[10]
                .parse::<i128>()
                .map_err(|error| format!("BUG: restored heap parse failed: {error}"))?,
        });
    }
    let reported: Vec<usize> = rows.iter().map(|row| row.rate_cells).collect();
    if reported != SNAPSHOT_LADDER {
        return Err(format!(
            "logistics_external bench reported rungs {reported:?}, expected {SNAPSHOT_LADDER:?}"
        ));
    }
    Ok(rows)
}

fn run_outputs_bench(root: &Path) -> Result<String, String> {
    let output = Command::new("cargo")
        .current_dir(root)
        .env("CARGO_TARGET_DIR", root.join("target"))
        .args(["bench", "-p", "lemma-engine", "--bench", "outputs"])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|error| format!("failed to spawn cargo bench outputs: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo bench outputs exited with code {:?}",
            output.status.code()
        ));
    }
    String::from_utf8(output.stdout)
        .map_err(|error| format!("outputs bench stdout was not UTF-8: {error}"))
}

fn run_python_benchmark(root: &Path) -> Result<String, String> {
    let output = Command::new("python3")
        .current_dir(root)
        .arg(PYTHON_BENCH_RELATIVE)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|error| format!("failed to spawn python3 {PYTHON_BENCH_RELATIVE}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "python3 {PYTHON_BENCH_RELATIVE} exited with code {:?}",
            output.status.code()
        ));
    }
    String::from_utf8(output.stdout)
        .map_err(|error| format!("python benchmark stdout was not UTF-8: {error}"))
}

fn latency_terminal_rule(spec_name: &str) -> &'static str {
    match spec_name {
        "bench_shipping" | "bench_pricing" => "total",
        "bench_order_pipeline" => "grand_total",
        other => panic!("BUG: unknown bench spec '{other}'"),
    }
}

#[derive(Debug, Clone)]
struct AccuracyDeviation {
    spec_name: &'static str,
    rule_name: String,
    lemma_repr: String,
    python_repr: String,
    abs_delta: Option<String>,
    rel_delta_percent: Option<String>,
}

#[derive(Debug, Clone, Copy, Default)]
struct AccuracyStats {
    compared: usize,
    deviations: usize,
}

fn compute_accuracy(
    outputs_by_spec: &BTreeMap<String, &OutputsFixture>,
    python_by_spec: &BTreeMap<String, &PythonFixture>,
) -> Result<(AccuracyStats, Vec<AccuracyDeviation>), String> {
    let mut stats = AccuracyStats::default();
    let mut deviations: Vec<AccuracyDeviation> = Vec::new();

    for fixture in FIXTURES {
        let lemma_outputs = &outputs_by_spec[fixture.spec_name].outputs;
        let python_outputs = &python_by_spec[fixture.spec_name].outputs;

        let mut all_rules: std::collections::BTreeSet<&String> = lemma_outputs.keys().collect();
        for k in python_outputs.keys() {
            all_rules.insert(k);
        }

        for rule_name in all_rules {
            stats.compared += 1;
            let lemma = lemma_outputs.get(rule_name);
            let python = python_outputs.get(rule_name);

            match (lemma, python) {
                (Some(l), Some(p)) => {
                    if let Some(dev) = diff_pair(fixture.spec_name, rule_name, l, p)? {
                        stats.deviations += 1;
                        deviations.push(dev);
                    }
                }
                (Some(l), None) => {
                    stats.deviations += 1;
                    deviations.push(AccuracyDeviation {
                        spec_name: fixture.spec_name,
                        rule_name: rule_name.clone(),
                        lemma_repr: format!("{} = {}", l.kind, l.value),
                        python_repr: "<missing>".to_string(),
                        abs_delta: None,
                        rel_delta_percent: None,
                    });
                }
                (None, Some(p)) => {
                    stats.deviations += 1;
                    deviations.push(AccuracyDeviation {
                        spec_name: fixture.spec_name,
                        rule_name: rule_name.clone(),
                        lemma_repr: "<missing>".to_string(),
                        python_repr: p.clone(),
                        abs_delta: None,
                        rel_delta_percent: None,
                    });
                }
                (None, None) => unreachable!("BUG: rule name came from union of both maps"),
            }
        }
    }
    Ok((stats, deviations))
}

fn diff_pair(
    spec_name: &'static str,
    rule_name: &str,
    lemma: &LemmaOutput,
    python_value: &String,
) -> Result<Option<AccuracyDeviation>, String> {
    match lemma.kind.as_str() {
        "number" | "ratio" | "measure" | "calendar" => {
            let lemma_dec = Decimal::from_str(&lemma.value).map_err(|error| {
                format!(
                    "BUG: spec '{spec_name}' rule '{rule_name}' Lemma value '{}' exceeds rust_decimal precision: {error}",
                    lemma.value
                )
            })?;
            let python_dec = Decimal::from_str(python_value).map_err(|error| {
                format!(
                    "BUG: spec '{spec_name}' rule '{rule_name}' Python value '{python_value}' exceeds rust_decimal precision: {error}"
                )
            })?;
            if lemma_dec == python_dec {
                return Ok(None);
            }
            let abs_delta = (lemma_dec - python_dec).abs();
            let rel_delta_percent = if lemma_dec.is_zero() {
                if python_dec.is_zero() {
                    Some("0".to_string())
                } else {
                    Some("undefined".to_string())
                }
            } else {
                let raw = (abs_delta / lemma_dec.abs()) * Decimal::from(100);
                Some(raw.to_string())
            };
            Ok(Some(AccuracyDeviation {
                spec_name,
                rule_name: rule_name.to_string(),
                lemma_repr: lemma_display_repr(lemma),
                python_repr: python_value.clone(),
                abs_delta: Some(abs_delta.to_string()),
                rel_delta_percent,
            }))
        }
        "boolean" | "text" | "date" | "time" | "veto" | "range" | "list" => {
            if lemma.value == *python_value {
                Ok(None)
            } else {
                Ok(Some(AccuracyDeviation {
                    spec_name,
                    rule_name: rule_name.to_string(),
                    lemma_repr: format!("{} = {}", lemma.kind, lemma.value),
                    python_repr: python_value.clone(),
                    abs_delta: None,
                    rel_delta_percent: None,
                }))
            }
        }
        other => Err(format!(
            "BUG: spec '{spec_name}' rule '{rule_name}' has unknown Lemma kind '{other}'"
        )),
    }
}

struct ComposeReportContext<'a> {
    env: &'a EnvironmentInfo,
    python_version: &'a str,
    latency_rows: &'a BTreeMap<&'static str, LatencyRow>,
    explain_latency_rows: &'a BTreeMap<&'static str, LatencyRow>,
    compile_rows: &'a BTreeMap<&'static str, LatencyRow>,
    python_by_spec: &'a BTreeMap<String, &'a PythonFixture>,
    accuracy: &'a (AccuracyStats, Vec<AccuracyDeviation>),
    memory_rows: &'a [MemoryRow],
    plan_memory_rows: &'a [MemoryRow],
    snapshot_rows: &'a [SnapshotRow],
    external_rows: &'a [ExternalCatalogRow],
}

fn push_memory_table(out: &mut String, rows: &[MemoryRow], unit: &str) {
    out.push_str(&format!(
        "| Spec | Iterations | Allocations/{unit} | Bytes allocated/{unit} | Reallocations/{unit} |\n",
    ));
    out.push_str(
        "|------|-----------:|-----------------:|---------------------:|-------------------:|\n",
    );
    for row in rows {
        out.push_str(&format!(
            "| `{}` | {} | {:.2} | {:.0} | {:.2} |\n",
            row.spec_name,
            row.iterations,
            row.allocations_per_iteration,
            row.bytes_allocated_per_iteration,
            row.reallocations_per_iteration,
        ));
    }
    out.push('\n');
}

struct MemoryRow {
    spec_name: &'static str,
    iterations: usize,
    allocations_per_iteration: f64,
    bytes_allocated_per_iteration: f64,
    reallocations_per_iteration: f64,
}

#[derive(Debug)]
struct SnapshotRow {
    profile: String,
    rate_cells: usize,
    source_bytes: usize,
    load_ms: f64,
    loaded_heap_bytes: i128,
    snapshot_bytes: usize,
    encode_ms: f64,
    restore_ms: f64,
    eval_ms: f64,
    restore_allocations: usize,
    restored_heap_bytes: i128,
}

#[derive(Debug)]
struct ExternalCatalogRow {
    profile: String,
    rate_cells: usize,
    source_bytes: usize,
    load_ms: f64,
    loaded_heap_bytes: i128,
    snapshot_bytes: usize,
    encode_ms: f64,
    restore_ms: f64,
    eval_ms: f64,
    restored_heap_bytes: i128,
}

fn format_mebibytes(bytes: i128) -> String {
    format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
}

fn lemma_display_repr(lemma: &LemmaOutput) -> String {
    match lemma.unit.as_deref() {
        Some(unit) => format!("{} {}", lemma.value, unit),
        None => lemma.value.clone(),
    }
}

fn compose_report(ctx: ComposeReportContext<'_>) -> Result<String, String> {
    let ComposeReportContext {
        env,
        python_version,
        latency_rows,
        explain_latency_rows,
        compile_rows,
        python_by_spec,
        accuracy,
        memory_rows,
        plan_memory_rows,
        snapshot_rows,
        external_rows,
    } = ctx;
    let (stats, deviations) = accuracy;
    let mut out = String::new();
    out.push_str("---\nnav_title: Engine benchmarks\nparent: Reference\nnav_order: 60\n---\n\n");
    out.push_str("# Engine evaluation benchmarks\n\n");
    out.push_str(
        "Numbers are produced by `cargo benchmarks engine`. \
         Hand-written Lemma specs and hand-written Python ports of the same business rules \
         are measured on identical inline inputs.\n\n",
    );

    out.push_str("## Methodology\n\n");
    out.push_str(
        "- Hand-written Lemma specs vs hand-written Python ports of the same rules, \
         on identical inline inputs.\n",
    );
    out.push_str(
        "- Cross-language latency compares per-request evaluation only: like comparing \
         optimized C execution to Python, not C compile time to Python runtime.\n",
    );
    out.push_str(
        "- Lemma: compile (`Engine::new()` + `load(in_memory_source)`, parse + plan) once \
         before measurement; timed loop = inline input literals + `Engine::evaluate` → terminal rule. \
         Terminal rule is `total` (shipping, pricing) or `grand_total` (order_pipeline).\n",
    );
    out.push_str(
        "- Python: import module once before measurement; timed loop = inline input literals + \
         `build_inputs(raw)` + `compute_terminal(inputs)`. `compute_terminal` evaluates only the \
         terminal rule's dependency closure, matching Lemma's requested-rule walk.\n",
    );
    out.push_str(
        "- Literal constants are parsed once: Lemma at plan time, Python at import time \
         (module-level `Fraction` constants). Rounding number types (`Decimal`, `float`) are \
         out of scope; the comparison is exact arithmetic vs exact arithmetic.\n",
    );
    out.push_str(
        "- No disk I/O, no JSON input sidecars, no pre-built input maps outside the timed loop.\n",
    );
    out.push_str(&format!(
        "- Effective pinned to `{BENCH_EFFECTIVE_ISO}` (no timezone) on the Lemma side; Python rules carry no temporal logic.\n",
    ));
    out.push_str(
        "- Latency: Criterion (3s warmup, 30s measurement) for Lemma; \
         100 warmup + 10_000 measured `time.perf_counter_ns()` samples with \
         `gc.disable()` bracketing for Python. Median and standard deviation reported.\n",
    );
    out.push_str(
        "- Numerical precision: a separate untimed pass compares all rule outputs. Lemma's \
         `outputs` bench evaluates every local rule with explanations; Python's \
         `compute(inputs)` returns a full `Outputs` dataclass. Both sides use exact \
         rational arithmetic (`RationalInteger` / `fractions.Fraction`) and commit to \
         decimal strings at the output boundary. The accuracy table compares both sides via \
         `rust_decimal::Decimal` (28-digit precision).\n",
    );
    out.push_str(
        "- Memory: `stats_alloc` over 100 warmup + 1_000 measured eval-only `evaluate` calls per fixture \
         (`cargo bench -p lemma-engine --bench memory`). Engine loaded once per fixture; each iteration \
         wraps inline inputs + `Engine::evaluate` in a fresh region. Plan memory: same bench, 5 warmup + \
         50 measured compiles (`Engine::new()` + `load`, parse + plan) per fixture, each in a fresh \
         region.\n",
    );
    out.push_str(
        "- Snapshot: `cargo bench -p lemma-engine --bench logistics` (embedded table) on the \
         generated logistics ladder (1050 / 6300 / 18900 / 126000 rate cells; byte-identical to the \
         Java `SpecGenerator.logistics` fixtures). One `load`, then 1 warmup + 10 measured \
         `Engine::snapshot`, `Engine::from_snapshot`, and eval (`shipment inputs` + `Engine::evaluate` → \
         `cheapest`; catalog already in the plan). Heap columns are `stats_alloc` net bytes \
         retained by the loaded and by the restored engine. The restored engine must evaluate \
         `rate_shop.cheapest` to the same value as the loaded engine.\n",
    );
    out.push_str(
        "- External catalog: same `cargo bench -p lemma-engine --bench logistics` (external table) \
         on the same ladder with empty keyed lists in source and catalog rows held as JSON. Timed \
         eval = `parse_evaluate_data_object` + `Engine::evaluate` (1 warmup + 10 measured); that \
         includes JSON parse, unlike embedded eval. Load / snapshot / restore measure the skeleton \
         only. Each rung must match embedded `rate_shop.cheapest`.\n\n",
    );

    push_environment_block(&mut out, env, Some(python_version));

    out.push_str("## Compile (Lemma, parse + plan)\n\n");
    out.push_str(
        "One-time cost per spec load. Not included in the Python/Lemma latency ratio; \
         amortized across requests in production.\n\n",
    );
    out.push_str("| Spec | Median | Std dev |\n");
    out.push_str("|------|-------:|--------:|\n");
    for fixture in FIXTURES {
        let compile = compile_rows
            .get(fixture.spec_name)
            .copied()
            .ok_or_else(|| format!("missing compile row for {}", fixture.spec_name))?;
        out.push_str(&format!(
            "| `{}` | {} | {} |\n",
            fixture.spec_name,
            format_latency_ns(compile.median_ns),
            format_latency_ns(compile.std_dev_ns),
        ));
    }
    out.push('\n');

    out.push_str("## Latency\n\n");
    out.push_str(
        "| Spec | Terminal rule | Lemma median | Lemma std dev | Python median | Python iter | Python std dev | Python / Lemma |\n",
    );
    out.push_str(
        "|------|---------------|-------------:|--------------:|--------------:|------------:|---------------:|---------------:|\n",
    );
    for fixture in FIXTURES {
        let terminal_rule = latency_terminal_rule(fixture.spec_name);
        let lemma = latency_rows
            .get(fixture.spec_name)
            .copied()
            .ok_or_else(|| format!("missing latency row for {}", fixture.spec_name))?;
        let python = python_by_spec
            .get(fixture.spec_name)
            .ok_or_else(|| format!("missing python fixture for {}", fixture.spec_name))?;
        out.push_str(&latency_table_row(
            fixture.spec_name,
            terminal_rule,
            lemma,
            python,
        ));
    }
    out.push('\n');

    out.push_str("## Explain latency (`evaluate_explain`)\n\n");
    out.push_str(
        "Same fixtures and terminal rules as the latency table, with `explain: true`. \
         Ratio is explain median divided by `evaluate` median on the same machine run.\n\n",
    );
    out.push_str(
        "| Spec | Terminal rule | `evaluate` median | `evaluate_explain` median | Explain / `evaluate` |\n",
    );
    out.push_str(
        "|------|---------------|------------------:|--------------------------:|---------------------:|\n",
    );
    for fixture in FIXTURES {
        let terminal_rule = latency_terminal_rule(fixture.spec_name);
        let base = latency_rows
            .get(fixture.spec_name)
            .copied()
            .ok_or_else(|| format!("missing latency row for {}", fixture.spec_name))?;
        let explain = explain_latency_rows
            .get(fixture.spec_name)
            .copied()
            .ok_or_else(|| format!("missing explain latency row for {}", fixture.spec_name))?;
        out.push_str(&format!(
            "| `{}` | `{}` | {} | {} | {} |\n",
            fixture.spec_name,
            terminal_rule,
            format_latency_ns(base.median_ns),
            format_latency_ns(explain.median_ns),
            format_ratio(explain.median_ns, base.median_ns),
        ));
    }
    out.push('\n');

    out.push_str("## Memory (per `evaluate` call)\n\n");
    push_memory_table(&mut out, memory_rows, "eval");
    out.push_str("## Memory (per compile, parse + plan)\n\n");
    push_memory_table(&mut out, plan_memory_rows, "plan");

    out.push_str("## Snapshot (logistics ladder)\n\n");
    out.push_str(
        "Multi-spec rating workspace: `rates_*` cards with 1050 list rows each (`-> key` / `find`), \
         `zones_*` with 900 ZIP3 rows per service, fuel brackets as range-keyed lists, one quote \
         pipeline per carrier and service, and a `rate_shop` that `uses` every quote. Eval median \
         is shipment inputs + `run` to `cheapest` (catalog already planned). Restore is the number \
         to watch for snapshot cost; snapshot bytes and restored heap are dominated by the per-cell \
         type payload the plan store ships today.\n\n",
    );
    out.push_str(
        "| Profile | Rate cells | Source | Load | Loaded heap | Snapshot | Encode median | Restore median | Eval median | Allocations/restore | Restored heap | Restored / loaded heap |\n",
    );
    out.push_str(
        "|---------|-----------:|-------:|-----:|------------:|---------:|--------------:|---------------:|------------:|--------------------:|--------------:|-----------------------:|\n",
    );
    for row in snapshot_rows {
        out.push_str(&format!(
            "| `{}` | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            row.profile,
            row.rate_cells,
            format_mebibytes(row.source_bytes as i128),
            format_latency_ns(row.load_ms * 1_000_000.0),
            format_mebibytes(row.loaded_heap_bytes),
            format_mebibytes(row.snapshot_bytes as i128),
            format_latency_ns(row.encode_ms * 1_000_000.0),
            format_latency_ns(row.restore_ms * 1_000_000.0),
            format_latency_ns(row.eval_ms * 1_000_000.0),
            row.restore_allocations,
            format_mebibytes(row.restored_heap_bytes),
            format_ratio(row.restored_heap_bytes as f64, row.loaded_heap_bytes as f64),
        ));
    }
    out.push('\n');

    out.push_str("## Snapshot / eval (logistics ladder, external catalog)\n\n");
    out.push_str(
        "Same rate-shop workspace as the embedded ladder, but keyed catalog lists (`cells`, \
         ZIP3 zones, fuel brackets) stay empty in source and arrive as JSON evaluation data on \
         `rate_shop` (`qN.r.cells`, `qN.z.{service}_zones`, `qN.f.brackets`). Eval median is \
         JSON parse + `run` to `cheapest`.\n\n",
    );
    out.push_str(
        "| Profile | Rate cells | Source | Load | Loaded heap | Snapshot | Encode median | Restore median | Eval median | Restored heap | Restored / loaded heap |\n",
    );
    out.push_str(
        "|---------|-----------:|-------:|-----:|------------:|---------:|--------------:|---------------:|------------:|--------------:|-----------------------:|\n",
    );
    for row in external_rows {
        out.push_str(&format!(
            "| `{}` | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            row.profile,
            row.rate_cells,
            format_mebibytes(row.source_bytes as i128),
            format_latency_ns(row.load_ms * 1_000_000.0),
            format_mebibytes(row.loaded_heap_bytes),
            format_mebibytes(row.snapshot_bytes as i128),
            format_latency_ns(row.encode_ms * 1_000_000.0),
            format_latency_ns(row.restore_ms * 1_000_000.0),
            format_latency_ns(row.eval_ms * 1_000_000.0),
            format_mebibytes(row.restored_heap_bytes),
            format_ratio(row.restored_heap_bytes as f64, row.loaded_heap_bytes as f64),
        ));
    }
    out.push('\n');

    out.push_str("## Numerical accuracy\n\n");
    out.push_str(&format!(
        "{} rule outputs compared across the three fixtures; {} deviations.\n\n",
        stats.compared, stats.deviations,
    ));
    if !deviations.is_empty() {
        out.push_str("| Spec | Rule | Lemma | Python | Abs delta | Rel delta % |\n");
        out.push_str("|------|------|------:|-------:|----------:|------------:|\n");
        for dev in deviations {
            out.push_str(&format!(
                "| `{}` | `{}` | `{}` | `{}` | {} | {} |\n",
                dev.spec_name,
                dev.rule_name,
                dev.lemma_repr,
                dev.python_repr,
                dev.abs_delta.as_deref().unwrap_or("—"),
                dev.rel_delta_percent.as_deref().unwrap_or("—"),
            ));
        }
        out.push('\n');
    }

    out.push_str("## Python implementation\n\n");
    out.push_str(&format!(
        "Hand-written ports of the three Lemma specs live in {}. \
         Each module exports `Inputs`, `Outputs`, `TERMINAL_RULE`, `build_inputs(raw)`, \
         `compute_terminal(inputs)`, and `compute(inputs)`. \
         Standard library only (`fractions`, `dataclasses`, `importlib`, `time`, \
         `gc`, `pathlib`, `statistics`). \
         The Python benchmark harness is {}.\n\n",
        github_source_link(&env.git_sha, "engine/benches/python/business_rules", true,),
        github_source_link(&env.git_sha, "engine/benches/python/benchmark.py", false),
    ));

    out.push_str("## Inputs\n\n");
    out.push_str(&format!(
        "All fixtures share `effective = {BENCH_EFFECTIVE_ISO}` (no timezone). \
         Input values are inline string literals built inside every timed iteration on both sides.\n\n",
    ));
    for fixture in FIXTURES {
        out.push_str(&format!("### `{}`\n\n", fixture.spec_name));
        out.push_str(&format!(
            "Lemma source: {}. Python module: `{}`.\n\n",
            github_source_link(&env.git_sha, fixture.lemma_path, false),
            fixture.python_module,
        ));
        out.push_str("| Field | Value |\n|-------|-------|\n");
        for (field, value) in fixture.inputs {
            out.push_str(&format!("| `{field}` | `{value}` |\n"));
        }
        out.push('\n');
    }

    Ok(out)
}

fn latency_table_row(
    spec_name: &str,
    terminal_rule: &str,
    lemma: LatencyRow,
    python: &PythonFixture,
) -> String {
    format!(
        "| `{}` | `{}` | {} | {} | {} | {} | {} | {} |\n",
        spec_name,
        terminal_rule,
        format_latency_ns(lemma.median_ns),
        format_latency_ns(lemma.std_dev_ns),
        format_latency_ns(python.latency_median_ns),
        python.iterations_latency,
        format_latency_ns(python.latency_std_dev_ns),
        format_ratio(python.latency_median_ns, lemma.median_ns),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_python() -> PythonFixture {
        PythonFixture {
            spec_name: "bench_shipping".to_string(),
            iterations_latency: 10_000,
            latency_median_ns: 6_990.0,
            latency_std_dev_ns: 4_170.0,
            outputs: BTreeMap::new(),
        }
    }

    #[test]
    fn latency_table_row_has_eight_columns_including_ratio() {
        let row = latency_table_row(
            "bench_shipping",
            "total",
            LatencyRow {
                median_ns: 22_580.0,
                std_dev_ns: 4_940.0,
            },
            &sample_python(),
        );
        let cells: Vec<&str> = row
            .trim()
            .split('|')
            .map(str::trim)
            .filter(|cell| !cell.is_empty())
            .collect();
        assert_eq!(cells.len(), 8, "row: {row}");
        assert_eq!(cells[0], "`bench_shipping`");
        assert_eq!(cells[4], "6.99 us");
        assert_eq!(cells[5], "10000");
        assert_eq!(
            cells[7],
            format_ratio(sample_python().latency_median_ns, 22_580.0)
        );
    }

    #[test]
    fn compose_report_mentions_engine_suite_command() {
        let env = EnvironmentInfo {
            rustc_version: "rustc test".to_string(),
            uname: "Linux test x86_64".to_string(),
            git_sha: "abc123".to_string(),
            generated_at: "2026-01-01T00:00:00Z".to_string(),
        };
        let row = LatencyRow {
            median_ns: 1.0,
            std_dev_ns: 1.0,
        };
        let mut latency_rows = BTreeMap::new();
        let mut compile_rows = BTreeMap::new();
        let python_owned: Vec<PythonFixture> = FIXTURES
            .iter()
            .map(|fixture| PythonFixture {
                spec_name: fixture.spec_name.to_string(),
                iterations_latency: 10_000,
                latency_median_ns: 1.0,
                latency_std_dev_ns: 1.0,
                outputs: BTreeMap::new(),
            })
            .collect();
        let mut python_by_spec = BTreeMap::new();
        let mut explain_latency_rows = BTreeMap::new();
        for (fixture, python) in FIXTURES.iter().zip(python_owned.iter()) {
            latency_rows.insert(fixture.spec_name, row);
            compile_rows.insert(fixture.spec_name, row);
            explain_latency_rows.insert(fixture.spec_name, row);
            python_by_spec.insert(fixture.spec_name.to_string(), python);
        }
        let accuracy = (AccuracyStats::default(), Vec::new());

        let report = compose_report(ComposeReportContext {
            env: &env,
            python_version: "Python 3.12.3",
            latency_rows: &latency_rows,
            explain_latency_rows: &explain_latency_rows,
            compile_rows: &compile_rows,
            python_by_spec: &python_by_spec,
            accuracy: &accuracy,
            memory_rows: &[],
            plan_memory_rows: &[],
            snapshot_rows: &[],
            external_rows: &[],
        })
        .expect("compose");

        assert!(report.contains("cargo benchmarks engine"));
        assert!(report.contains("Compile (Lemma, parse + plan)"));
        assert!(report.contains("## Snapshot (logistics ladder)"));
        assert!(report.contains("## Snapshot / eval (logistics ladder, external catalog)"));
        assert!(report.contains(
            "[`engine/benches/specs/shipping.lemma`](https://github.com/lemma/lemma/blob/abc123/engine/benches/specs/shipping.lemma)"
        ));
        assert!(report.contains(
            "[`engine/benches/python/business_rules`](https://github.com/lemma/lemma/tree/abc123/engine/benches/python/business_rules)"
        ));
        assert!(!report.contains("../../engine/benches/"));
    }

    const SNAPSHOT_STDOUT: &str = "\
| Profile | Rate cells | Source bytes | Load | Loaded heap | Snapshot bytes | Encode median | Restore median | Eval median | Allocations/restore | Restored heap | Restored / loaded heap |
|---------|-----------:|-------------:|-----:|------------:|---------------:|--------------:|---------------:|------------:|--------------------:|--------------:|-----------------------:|
| logistics_ground | 1050 | 87898 | 46.431 ms | 10231892 | 616603 | 1.851 ms | 7.623 ms | 2.100 ms | 77834 | 13124722 | 1.283 |
| logistics_carrier | 6300 | 509122 | 404.459 ms | 74235095 | 4617773 | 18.835 ms | 45.534 ms | 12.500 ms | 586500 | 75607559 | 1.018 |
| logistics_d2c | 18900 | 1520554 | 1270.649 ms | 220702690 | 13643905 | 55.490 ms | 137.404 ms | 40.000 ms | 1720094 | 218473564 | 0.990 |
| logistics_enterprise | 126000 | 10121625 | 21065.850 ms | 1467607295 | 92171050 | 379.889 ms | 931.431 ms | 250.000 ms | 11355645 | 1436413854 | 0.979 |
";

    #[test]
    fn parse_snapshot_bench_stdout_reads_every_rung() {
        let rows = parse_snapshot_bench_stdout(SNAPSHOT_STDOUT).expect("parse");
        assert_eq!(rows.len(), 4);
        let enterprise = &rows[3];
        assert_eq!(enterprise.profile, "logistics_enterprise");
        assert_eq!(enterprise.rate_cells, 126000);
        assert_eq!(enterprise.source_bytes, 10_121_625);
        assert_eq!(enterprise.load_ms, 21065.850);
        assert_eq!(enterprise.loaded_heap_bytes, 1_467_607_295);
        assert_eq!(enterprise.snapshot_bytes, 92_171_050);
        assert_eq!(enterprise.encode_ms, 379.889);
        assert_eq!(enterprise.restore_ms, 931.431);
        assert_eq!(enterprise.eval_ms, 250.000);
        assert_eq!(enterprise.restore_allocations, 11_355_645);
        assert_eq!(enterprise.restored_heap_bytes, 1_436_413_854);
    }

    #[test]
    fn parse_snapshot_bench_stdout_rejects_missing_rung() {
        let without_enterprise: String = SNAPSHOT_STDOUT
            .lines()
            .filter(|line| !line.starts_with("| logistics_enterprise"))
            .map(|line| format!("{line}\n"))
            .collect();
        let error = parse_snapshot_bench_stdout(&without_enterprise).expect_err("must reject");
        assert!(
            error.contains("expected [1050, 6300, 18900, 126000]"),
            "{error}"
        );
    }

    #[test]
    fn snapshot_section_formats_units() {
        let rows = parse_snapshot_bench_stdout(SNAPSHOT_STDOUT).expect("parse");
        let env = EnvironmentInfo {
            rustc_version: "rustc test".to_string(),
            uname: "Linux test x86_64".to_string(),
            git_sha: "abc123".to_string(),
            generated_at: "2026-01-01T00:00:00Z".to_string(),
        };
        let row = LatencyRow {
            median_ns: 1.0,
            std_dev_ns: 1.0,
        };
        let python_owned: Vec<PythonFixture> = FIXTURES
            .iter()
            .map(|fixture| PythonFixture {
                spec_name: fixture.spec_name.to_string(),
                iterations_latency: 10_000,
                latency_median_ns: 1.0,
                latency_std_dev_ns: 1.0,
                outputs: BTreeMap::new(),
            })
            .collect();
        let mut latency_rows = BTreeMap::new();
        let mut compile_rows = BTreeMap::new();
        let mut explain_latency_rows = BTreeMap::new();
        let mut python_by_spec = BTreeMap::new();
        for (fixture, python) in FIXTURES.iter().zip(python_owned.iter()) {
            latency_rows.insert(fixture.spec_name, row);
            compile_rows.insert(fixture.spec_name, row);
            explain_latency_rows.insert(fixture.spec_name, row);
            python_by_spec.insert(fixture.spec_name.to_string(), python);
        }
        let accuracy = (AccuracyStats::default(), Vec::new());
        let report = compose_report(ComposeReportContext {
            env: &env,
            python_version: "Python 3.12.3",
            latency_rows: &latency_rows,
            explain_latency_rows: &explain_latency_rows,
            compile_rows: &compile_rows,
            python_by_spec: &python_by_spec,
            accuracy: &accuracy,
            memory_rows: &[],
            plan_memory_rows: &[],
            snapshot_rows: &rows,
            external_rows: &[],
        })
        .expect("compose");
        assert!(
            report.contains(
                "| `logistics_enterprise` | 126000 | 9.7 MiB | 21065.850 ms | 1399.6 MiB | 87.9 MiB | 379.889 ms | 931.431 ms | 250.000 ms | 11355645 | 1369.9 MiB | 0.9787 |"
            ),
            "{report}"
        );
    }

    const EXTERNAL_STDOUT: &str = "\
| Profile | Rate cells | Source bytes | Load | Loaded heap | Snapshot bytes | Encode median | Restore median | Eval median | Restored heap | Restored / loaded heap |
|---------|-----------:|-------------:|-----:|------------:|---------------:|--------------:|---------------:|------------:|--------------:|-----------------------:|
| logistics_ext_ground | 1050 | 12000 | 5.100 ms | 2000000 | 300000 | 0.500 ms | 1.200 ms | 8.400 ms | 2100000 | 1.050 |
| logistics_ext_carrier | 6300 | 25000 | 12.000 ms | 5000000 | 800000 | 1.100 ms | 3.000 ms | 40.000 ms | 5200000 | 1.040 |
| logistics_ext_d2c | 18900 | 40000 | 30.000 ms | 12000000 | 2000000 | 3.000 ms | 8.000 ms | 120.000 ms | 12500000 | 1.042 |
| logistics_ext_enterprise | 126000 | 90000 | 200.000 ms | 80000000 | 15000000 | 20.000 ms | 50.000 ms | 900.000 ms | 82000000 | 1.025 |
";

    #[test]
    fn parse_logistics_external_bench_stdout_reads_every_rung() {
        let rows = parse_logistics_external_bench_stdout(EXTERNAL_STDOUT).expect("parse");
        assert_eq!(rows.len(), 4);
        let enterprise = &rows[3];
        assert_eq!(enterprise.profile, "logistics_ext_enterprise");
        assert_eq!(enterprise.rate_cells, 126000);
        assert_eq!(enterprise.eval_ms, 900.000);
        assert_eq!(enterprise.source_bytes, 90_000);
    }

    #[test]
    fn external_section_formats_eval_median() {
        let rows = parse_logistics_external_bench_stdout(EXTERNAL_STDOUT).expect("parse");
        let env = EnvironmentInfo {
            rustc_version: "rustc test".to_string(),
            uname: "Linux test x86_64".to_string(),
            git_sha: "abc123".to_string(),
            generated_at: "2026-01-01T00:00:00Z".to_string(),
        };
        let row = LatencyRow {
            median_ns: 1.0,
            std_dev_ns: 1.0,
        };
        let python_owned: Vec<PythonFixture> = FIXTURES
            .iter()
            .map(|fixture| PythonFixture {
                spec_name: fixture.spec_name.to_string(),
                iterations_latency: 10_000,
                latency_median_ns: 1.0,
                latency_std_dev_ns: 1.0,
                outputs: BTreeMap::new(),
            })
            .collect();
        let mut latency_rows = BTreeMap::new();
        let mut compile_rows = BTreeMap::new();
        let mut explain_latency_rows = BTreeMap::new();
        let mut python_by_spec = BTreeMap::new();
        for (fixture, python) in FIXTURES.iter().zip(python_owned.iter()) {
            latency_rows.insert(fixture.spec_name, row);
            compile_rows.insert(fixture.spec_name, row);
            explain_latency_rows.insert(fixture.spec_name, row);
            python_by_spec.insert(fixture.spec_name.to_string(), python);
        }
        let accuracy = (AccuracyStats::default(), Vec::new());
        let report = compose_report(ComposeReportContext {
            env: &env,
            python_version: "Python 3.12.3",
            latency_rows: &latency_rows,
            explain_latency_rows: &explain_latency_rows,
            compile_rows: &compile_rows,
            python_by_spec: &python_by_spec,
            accuracy: &accuracy,
            memory_rows: &[],
            plan_memory_rows: &[],
            snapshot_rows: &[],
            external_rows: &rows,
        })
        .expect("compose");
        assert!(
            report.contains(
                "| `logistics_ext_enterprise` | 126000 | 0.1 MiB | 200.000 ms | 76.3 MiB | 14.3 MiB | 20.000 ms | 50.000 ms | 900.000 ms | 78.2 MiB | 1.025 |"
            ),
            "{report}"
        );
    }

    #[test]
    fn python_benchmark_emits_json_for_all_fixtures() {
        use std::process::{Command, Stdio};

        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let output = Command::new("python3")
            .current_dir(&root)
            .arg(PYTHON_BENCH_RELATIVE)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("spawn python3 benchmark");
        assert!(
            output.status.success(),
            "python benchmark failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: PythonReport = serde_json::from_str(
            String::from_utf8(output.stdout)
                .expect("stdout utf-8")
                .trim(),
        )
        .expect("python benchmark stdout must be valid JSON");
        let spec_names: std::collections::BTreeSet<&str> = report
            .fixtures
            .iter()
            .map(|f| f.spec_name.as_str())
            .collect();
        for fixture in FIXTURES {
            assert!(
                spec_names.contains(fixture.spec_name),
                "python benchmark missing spec '{}'",
                fixture.spec_name
            );
        }
    }

    fn output(kind: &str, value: &str) -> LemmaOutput {
        LemmaOutput {
            kind: kind.to_string(),
            value: value.to_string(),
            unit: None,
        }
    }

    #[test]
    fn diff_pair_range_matches_identical_serialization() {
        let lemma = output(
            "range",
            r#"[{"kind":"number","value":"1"},{"kind":"number","value":"2"}]"#,
        );
        let python = lemma.value.clone();
        let result =
            diff_pair("bench_range", "span", &lemma, &python).expect("range diff must succeed");
        assert!(result.is_none());
    }

    #[test]
    fn diff_pair_range_reports_differing_serialization() {
        let lemma = output(
            "range",
            r#"[{"kind":"number","value":"1"},{"kind":"number","value":"2"}]"#,
        );
        let python = r#"[{"kind":"number","value":"1"},{"kind":"number","value":"3"}]"#.to_string();
        let deviation = diff_pair("bench_range", "span", &lemma, &python)
            .expect("range diff must succeed")
            .expect("differing range must be a deviation");
        assert_eq!(deviation.rule_name, "span");
        assert_eq!(deviation.python_repr, python);
        assert!(deviation.abs_delta.is_none());
        assert!(deviation.rel_delta_percent.is_none());
    }

    #[test]
    fn diff_pair_list_matches_identical_serialization() {
        let lemma = output(
            "list",
            r#"[{"kind":"number","value":"1"},{"kind":"number","value":"2"}]"#,
        );
        let python = lemma.value.clone();
        let result =
            diff_pair("bench_list", "rows", &lemma, &python).expect("list diff must succeed");
        assert!(result.is_none());
    }

    #[test]
    fn diff_pair_list_reports_differing_serialization() {
        let lemma = output("list", r#"[{"kind":"number","value":"1"}]"#);
        let python = r#"[{"kind":"number","value":"9"}]"#.to_string();
        let deviation = diff_pair("bench_list", "rows", &lemma, &python)
            .expect("list diff must succeed")
            .expect("differing list must be a deviation");
        assert_eq!(deviation.rule_name, "rows");
        assert_eq!(
            deviation.lemma_repr,
            "list = [{\"kind\":\"number\",\"value\":\"1\"}]"
        );
    }
}

//! Logistics ladder benchmark report (embedded + external catalog).
//!
//! Run via `cargo benchmarks logistics`.

use super::common::{
    capture_environment, format_latency_ns, push_environment_block, write_report, EnvironmentInfo,
};
use std::path::Path;
use std::process::{Command, Stdio};

pub const RESULTS_RELATIVE: &str = "cli/documentation/reference/benchmarks/logistics.md";

/// Rungs of the logistics ladder the logistics bench must report, in order.
const LOGISTICS_LADDER: [usize; 4] = [1050, 6300, 18900, 126000];

#[derive(Debug)]
struct LogisticsRow {
    profile: String,
    rate_cells: usize,
    source_bytes: usize,
    load_ms: f64,
    loaded_heap_bytes: i128,
    eval_ms: f64,
}

pub fn run(root: &Path) -> Result<(), String> {
    let stdout = run_logistics_bench(root)?;
    let embedded_rows = parse_logistics_embedded_bench_stdout(&stdout)?;
    let external_rows = parse_logistics_external_bench_stdout(&stdout)?;
    let env = capture_environment(root)?;
    let report = compose_report(&env, &embedded_rows, &external_rows);
    write_report(root, RESULTS_RELATIVE, &report)
}

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

fn parse_logistics_row(line: &str, what: &str) -> Result<LogisticsRow, String> {
    let cells: Vec<&str> = line.split('|').map(str::trim).collect();
    if cells.len() < 8 {
        return Err(format!("BUG: malformed {what} bench row: {line}"));
    }
    Ok(LogisticsRow {
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
        eval_ms: parse_milliseconds(cells[6], "eval")?,
    })
}

fn parse_logistics_embedded_bench_stdout(stdout: &str) -> Result<Vec<LogisticsRow>, String> {
    let mut rows = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        if !line.starts_with("| logistics_") || line.starts_with("| logistics_ext_") {
            continue;
        }
        rows.push(parse_logistics_row(line, "logistics")?);
    }
    let reported: Vec<usize> = rows.iter().map(|row| row.rate_cells).collect();
    if reported != LOGISTICS_LADDER {
        return Err(format!(
            "logistics bench reported rungs {reported:?}, expected {LOGISTICS_LADDER:?}"
        ));
    }
    Ok(rows)
}

fn parse_logistics_external_bench_stdout(stdout: &str) -> Result<Vec<LogisticsRow>, String> {
    let mut rows = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        if !line.starts_with("| logistics_ext_") {
            continue;
        }
        rows.push(parse_logistics_row(line, "logistics_external")?);
    }
    let reported: Vec<usize> = rows.iter().map(|row| row.rate_cells).collect();
    if reported != LOGISTICS_LADDER {
        return Err(format!(
            "logistics_external bench reported rungs {reported:?}, expected {LOGISTICS_LADDER:?}"
        ));
    }
    Ok(rows)
}

fn format_mebibytes(bytes: i128) -> String {
    format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
}

fn push_ladder_table(out: &mut String, rows: &[LogisticsRow]) {
    out.push_str("| Profile | Rate cells | Source | Load | Loaded heap | Eval median |\n");
    out.push_str("|---------|-----------:|-------:|-----:|------------:|------------:|\n");
    for row in rows {
        out.push_str(&format!(
            "| `{}` | {} | {} | {} | {} | {} |\n",
            row.profile,
            row.rate_cells,
            format_mebibytes(row.source_bytes as i128),
            format_latency_ns(row.load_ms * 1_000_000.0),
            format_mebibytes(row.loaded_heap_bytes),
            format_latency_ns(row.eval_ms * 1_000_000.0),
        ));
    }
    out.push('\n');
}

fn compose_report(
    env: &EnvironmentInfo,
    embedded_rows: &[LogisticsRow],
    external_rows: &[LogisticsRow],
) -> String {
    let mut out = String::new();
    out.push_str("---\nnav_title: Logistics benchmarks\nparent: Reference\nnav_order: 55\n---\n\n");
    out.push_str("# Logistics benchmarks\n\n");
    out.push_str(
        "Numbers are produced by `cargo benchmarks logistics`. \
         Large multi-spec rate-shop workspaces on the research ladder \
         (1050 / 6300 / 18900 / 126000 rate cells; byte-identical to the Java \
         `SpecGenerator.logistics` fixtures).\n\n",
    );

    out.push_str("## Methodology\n\n");
    out.push_str(
        "- Embedded ladder: `cargo bench -p lemma-engine --bench logistics` (embedded table). \
         One `load`, then 1 warmup + 10 measured eval (`shipment inputs` + `Engine::evaluate` → \
         `cheapest`; catalog already in the plan). Loaded heap is `stats_alloc` net bytes \
         retained by the loaded engine.\n",
    );
    out.push_str(
        "- External catalog: same bench (external table) with empty keyed lists in source and \
         catalog rows held as JSON. Timed eval = `parse_evaluate_data_object` + `Engine::evaluate` \
         (1 warmup + 10 measured); that includes JSON parse, unlike embedded eval. Load measures \
         the skeleton only. Each rung must match embedded `rate_shop.cheapest`.\n\n",
    );

    push_environment_block(&mut out, env, None);

    out.push_str("## Load and eval (embedded catalog)\n\n");
    out.push_str(
        "Multi-spec rating workspace: `rates_*` cards with 1050 list rows each (`-> key` / `find`), \
         `zones_*` with 900 ZIP3 rows per service, fuel brackets as range-keyed lists, one quote \
         pipeline per carrier and service, and a `rate_shop` that `uses` every quote. Eval median \
         is shipment inputs + `run` to `cheapest` (catalog already planned). Spec-written rows \
         are checked once against the row spec's \
         shared template plan, and every nested consumer Arc-shares the one interned row store, \
         so load and loaded heap scale with the unique catalog bodies, not with rows x \
         consumers. Path strings are deduped by content across all path copies.\n\n",
    );
    push_ladder_table(&mut out, embedded_rows);

    out.push_str("## Load and eval (external catalog)\n\n");
    out.push_str(
        "Same rate-shop workspace as the embedded ladder, but keyed catalog lists (`cells`, \
         ZIP3 zones, fuel brackets) stay empty in source and arrive as JSON evaluation data on \
         `rate_shop` (`qN.r.cells`, `qN.z.{service}_zones`, `qN.f.brackets`). Eval median is \
         JSON parse + `run` to `cheapest`.\n\n",
    );
    push_ladder_table(&mut out, external_rows);

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOGISTICS_STDOUT: &str = "\
| Profile | Rate cells | Source bytes | Load | Loaded heap | Eval median |
|---------|-----------:|-------------:|-----:|------------:|------------:|
| logistics_ground | 1050 | 87898 | 46.431 ms | 10231892 | 2.100 ms |
| logistics_carrier | 6300 | 509122 | 404.459 ms | 74235095 | 12.500 ms |
| logistics_d2c | 18900 | 1520554 | 1270.649 ms | 220702690 | 40.000 ms |
| logistics_enterprise | 126000 | 10121625 | 21065.850 ms | 1467607295 | 250.000 ms |
";

    const EXTERNAL_STDOUT: &str = "\
| Profile | Rate cells | Source bytes | Load | Loaded heap | Eval median |
|---------|-----------:|-------------:|-----:|------------:|------------:|
| logistics_ext_ground | 1050 | 12000 | 5.100 ms | 2000000 | 8.400 ms |
| logistics_ext_carrier | 6300 | 25000 | 12.000 ms | 5000000 | 40.000 ms |
| logistics_ext_d2c | 18900 | 40000 | 30.000 ms | 12000000 | 120.000 ms |
| logistics_ext_enterprise | 126000 | 90000 | 200.000 ms | 80000000 | 900.000 ms |
";

    #[test]
    fn parse_logistics_embedded_bench_stdout_reads_every_rung() {
        let rows = parse_logistics_embedded_bench_stdout(LOGISTICS_STDOUT).expect("parse");
        assert_eq!(rows.len(), 4);
        let enterprise = &rows[3];
        assert_eq!(enterprise.profile, "logistics_enterprise");
        assert_eq!(enterprise.rate_cells, 126000);
        assert_eq!(enterprise.source_bytes, 10_121_625);
        assert_eq!(enterprise.load_ms, 21065.850);
        assert_eq!(enterprise.loaded_heap_bytes, 1_467_607_295);
        assert_eq!(enterprise.eval_ms, 250.000);
    }

    #[test]
    fn parse_logistics_embedded_bench_stdout_rejects_missing_rung() {
        let without_enterprise: String = LOGISTICS_STDOUT
            .lines()
            .filter(|line| !line.starts_with("| logistics_enterprise"))
            .map(|line| format!("{line}\n"))
            .collect();
        let error =
            parse_logistics_embedded_bench_stdout(&without_enterprise).expect_err("must reject");
        assert!(
            error.contains("expected [1050, 6300, 18900, 126000]"),
            "{error}"
        );
    }

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
    fn compose_report_mentions_logistics_suite_command() {
        let env = EnvironmentInfo {
            rustc_version: "rustc test".to_string(),
            uname: "Linux test x86_64".to_string(),
            git_sha: "abc123".to_string(),
            generated_at: "2026-01-01T00:00:00Z".to_string(),
        };
        let embedded = parse_logistics_embedded_bench_stdout(LOGISTICS_STDOUT).expect("parse");
        let external = parse_logistics_external_bench_stdout(EXTERNAL_STDOUT).expect("parse");
        let report = compose_report(&env, &embedded, &external);
        assert!(report.contains("cargo benchmarks logistics"));
        assert!(report.contains("## Load and eval (embedded catalog)"));
        assert!(report.contains("## Load and eval (external catalog)"));
        assert!(report.contains(
            "| `logistics_enterprise` | 126000 | 9.7 MiB | 21065.850 ms | 1399.6 MiB | 250.000 ms |"
        ));
        assert!(report.contains(
            "| `logistics_ext_enterprise` | 126000 | 0.1 MiB | 200.000 ms | 76.3 MiB | 900.000 ms |"
        ));
    }
}

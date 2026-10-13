//! CLI benchmark report (`http_evaluate`, `engine_profile`).
//!
//! Run via `cargo benchmarks cli`. Keep [`HTTP_BENCH_CASES`] and
//! [`PROFILE_BENCH_CASES`] in sync with `cli/benches/http_evaluate.rs` and
//! `cli/benches/engine_profile.rs`.

use super::common::{
    capture_environment, format_latency_ns, github_source_link, push_environment_block,
    read_latency_estimate, run_criterion_bench, write_report, EnvironmentInfo, LatencyRow,
};
use std::path::Path;

pub const RESULTS_RELATIVE: &str = "cli/documentation/reference/benchmarks/cli.md";

pub struct BenchCase {
    pub group: &'static str,
    pub function: &'static str,
    pub label: &'static str,
}

/// Sync with `cli/benches/http_evaluate.rs`.
pub const HTTP_BENCH_CASES: &[BenchCase] = &[
    BenchCase {
        group: "evaluate",
        function: "coffee_order",
        label: "POST `/coffee_order`",
    },
    BenchCase {
        group: "evaluate",
        function: "library_fees",
        label: "POST `/library_fees`",
    },
    BenchCase {
        group: "evaluate",
        function: "dutch_salary",
        label: "POST `/net_salary`",
    },
    BenchCase {
        group: "show",
        function: "dutch_salary",
        label: "GET `/net_salary` (show only)",
    },
];

/// Sync with `cli/benches/engine_profile.rs`.
pub const PROFILE_BENCH_CASES: &[BenchCase] = &[
    BenchCase {
        group: "dutch_salary",
        function: "engine_evaluate",
        label: "Full `Engine::evaluate`",
    },
    BenchCase {
        group: "dutch_salary",
        function: "single_rule",
        label: "Single-rule evaluate (`periods_per_year`)",
    },
    BenchCase {
        group: "dutch_salary",
        function: "json_envelope",
        label: "Envelope JSON serialize",
    },
    BenchCase {
        group: "dutch_salary",
        function: "json_raw_response",
        label: "Raw response JSON serialize",
    },
];

pub fn run(root: &Path) -> Result<(), String> {
    run_criterion_bench(root, "lemma", "http_evaluate")?;
    run_criterion_bench(root, "lemma", "engine_profile")?;

    let env = capture_environment(root)?;

    let mut http_rows = Vec::new();
    for case in HTTP_BENCH_CASES {
        http_rows.push((
            case,
            read_latency_estimate(root, case.group, case.function)?,
        ));
    }

    let mut profile_rows = Vec::new();
    for case in PROFILE_BENCH_CASES {
        profile_rows.push((
            case,
            read_latency_estimate(root, case.group, case.function)?,
        ));
    }

    let report = compose_report(&env, &http_rows, &profile_rows)?;
    write_report(root, RESULTS_RELATIVE, &report)
}

fn compose_report(
    env: &EnvironmentInfo,
    http_rows: &[(&BenchCase, LatencyRow)],
    profile_rows: &[(&BenchCase, LatencyRow)],
) -> Result<String, String> {
    let mut out = String::new();
    out.push_str("---\nnav_title: CLI benchmarks\nparent: Reference\nnav_order: 50\n---\n\n");
    out.push_str("# CLI benchmarks\n\n");
    out.push_str(
        "Numbers are produced by `cargo benchmarks cli`. \
         Measures the `lemma` binary and in-process engine wrappers used by the CLI.\n\n",
    );

    out.push_str("## Methodology\n\n");
    out.push_str("### HTTP evaluate (`http_evaluate`)\n\n");
    out.push_str(
        "- Spawns `lemma server --path engine/documentation/examples` on `127.0.0.1:19877` once per Criterion group.\n",
    );
    out.push_str(
        "- Each iteration: blocking `reqwest` POST with `application/x-www-form-urlencoded` body \
         (coffee order, library fees, Dutch net salary) or GET for show-only retrieval.\n",
    );
    out.push_str(&format!(
        "- Examples loaded from {}.\n",
        github_source_link(&env.git_sha, "engine/documentation/examples", true),
    ));
    out.push_str(
        "- Latency: Criterion (3s warmup, 10s measurement for evaluate group, 5s for show). Median and standard deviation reported.\n\n",
    );

    out.push_str("### Engine profile (`engine_profile`)\n\n");
    out.push_str(
        "- In-process: loads all `.lemma` files from `engine/documentation/examples` into one `Engine`.\n",
    );
    out.push_str(
        "- Fixture: Dutch net salary (`net_salary`) with `gross_salary=5000 eur`, \
         `pay_period=month`, `income_source=employment`, `pension_contribution=150 eur`, \
         `payroll_tax_credit=true`; effective is `DateTimeValue::now()` per iteration setup.\n",
    );
    out.push_str(
        "- Breakdown benches isolate evaluate, overlay resolve, single-rule run, \
         and JSON serialization paths.\n",
    );
    out.push_str(
        "- Latency: Criterion (3s warmup, 5s measurement). Median and standard deviation reported.\n\n",
    );

    push_environment_block(&mut out, env, None);

    out.push_str("## HTTP evaluate latency\n\n");
    push_latency_table(&mut out, http_rows);

    out.push_str("## Engine profile latency (Dutch net salary)\n\n");
    push_latency_table(&mut out, profile_rows);

    Ok(out)
}

fn push_latency_table(out: &mut String, rows: &[(&BenchCase, LatencyRow)]) {
    out.push_str("| Case | Median | Std dev |\n");
    out.push_str("|------|-------:|--------:|\n");
    for (case, row) in rows {
        out.push_str(&format!(
            "| {} | {} | {} |\n",
            case.label,
            format_latency_ns(row.median_ns),
            format_latency_ns(row.std_dev_ns),
        ));
    }
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_report_lists_http_and_profile_sections() {
        let env = EnvironmentInfo {
            rustc_version: "rustc test".to_string(),
            uname: "Linux test x86_64".to_string(),
            git_sha: "abc123".to_string(),
            generated_at: "2026-01-01T00:00:00Z".to_string(),
        };
        let row = LatencyRow {
            median_ns: 1_000_000.0,
            std_dev_ns: 50_000.0,
        };
        let http = HTTP_BENCH_CASES
            .iter()
            .map(|case| (case, row))
            .collect::<Vec<_>>();
        let profile = PROFILE_BENCH_CASES
            .iter()
            .map(|case| (case, row))
            .collect::<Vec<_>>();

        let report = compose_report(&env, &http, &profile).expect("compose");
        assert!(report.contains("cargo benchmarks cli"));
        assert!(report.contains("POST `/coffee_order`"));
        assert!(report.contains("Envelope JSON serialize"));
        assert!(report.contains("| 1.000 ms |"));
        assert!(report.contains(
            "[`engine/documentation/examples`](https://github.com/lemma/lemma/tree/abc123/engine/documentation/examples)"
        ));
        assert!(!report.contains("](../../"));
    }
}

//! Load-time scaling: deep/wide rule DAGs after plan-time bitset needed_by index.
//!
//! Measured post-fix debug wall times on this machine (2026-09-05):
//!   deep N=5000:    594 ms
//!   wide N=5000:   2008 ms
//!   unless N=5000:  317 ms
//! Nested catalog (2026-11-02, spec-written rows planned once and Arc-shared
//! across nested consumers): rows=2000 consumers=40: 1043 ms
//! Bounds are 10× those measurements.

use lemma::{Engine, ResourceLimits, SourceType};
use std::fmt::Write;
use std::time::{Duration, Instant};

/// 10× measured deep N=5000 debug load (594 ms).
const DEEP_5000_MAX: Duration = Duration::from_secs(6);
/// 10× measured wide N=5000 debug load (2008 ms).
const WIDE_5000_MAX: Duration = Duration::from_secs(21);
/// 10× measured unless N=5000 debug load (317 ms).
const UNLESS_5000_MAX: Duration = Duration::from_secs(4);

fn deep_source(n: usize) -> String {
    let mut code = String::with_capacity(n * 32);
    write!(code, "spec bench_deep\ndata x0: number\nrule r1: x0 + 1\n").unwrap();
    for i in 2..=n {
        writeln!(code, "rule r{i}: r{} + 1", i - 1).unwrap();
    }
    code
}

fn wide_source(n: usize) -> String {
    let mut code = String::with_capacity(n * 80);
    writeln!(code, "spec bench_wide").unwrap();
    for i in 0..n {
        writeln!(code, "data d{i}: number").unwrap();
    }
    for i in 0..n {
        writeln!(code, "rule p{i}: d{i} * 2").unwrap();
    }
    writeln!(code, "rule s0: p0").unwrap();
    for i in 1..n {
        writeln!(code, "rule s{i}: s{} + p{i}", i - 1).unwrap();
    }
    code
}

fn unless_source(n: usize) -> String {
    let mut code = String::with_capacity(n * 40);
    write!(code, "spec bench_unless\ndata x: number\nrule r: 0\n").unwrap();
    for i in 0..n {
        writeln!(code, "  unless x is {i} then {i}").unwrap();
    }
    code
}

fn load_limits() -> ResourceLimits {
    ResourceLimits {
        max_source_size_bytes: 100 * 1024 * 1024,
        max_expression_count: 500_000,
        max_normalized_expression_nodes: 500_000,
        ..ResourceLimits::default()
    }
}

fn load_elapsed(code: String) -> Duration {
    let mut engine = Engine::with_limits(load_limits());
    let start = Instant::now();
    engine
        .load([(SourceType::Volatile, code)])
        .unwrap_or_else(|errs| panic!("load must succeed: {errs:?}"));
    start.elapsed()
}

#[test]
fn load_deep_5000_under_bound() {
    let elapsed = load_elapsed(deep_source(5000));
    assert!(
        elapsed < DEEP_5000_MAX,
        "deep N=5000 load {elapsed:?} exceeded bound {DEEP_5000_MAX:?} (10× measured 594ms)"
    );
}

#[test]
fn load_wide_5000_under_bound() {
    let elapsed = load_elapsed(wide_source(5000));
    assert!(
        elapsed < WIDE_5000_MAX,
        "wide N=5000 load {elapsed:?} exceeded bound {WIDE_5000_MAX:?} (10× measured 2008ms)"
    );
}

#[test]
fn load_unless_5000_under_bound() {
    let elapsed = load_elapsed(unless_source(5000));
    assert!(
        elapsed < UNLESS_5000_MAX,
        "unless N=5000 load {elapsed:?} exceeded bound {UNLESS_5000_MAX:?} (10× measured 317ms)"
    );
}

/// One catalog of `rows` spec-written list rows; `consumers` specs each `uses`
/// it, and one shop `uses` every consumer. Rows are checked once against the
/// shared template plan and Arc-shared by every consumer, so load scales with
/// `rows`, not `rows × consumers`.
fn nested_catalog_source(rows: usize, consumers: usize) -> String {
    let mut code = String::with_capacity(rows * 40 + consumers * 120);
    writeln!(code, "spec cell\ndata zone: number\ndata price: number\n").unwrap();
    writeln!(
        code,
        "spec catalog\nuses cell\ndata cells: cell list\n  -> key zone"
    )
    .unwrap();
    for i in 0..rows {
        writeln!(code, "  -> with zone: {i}, price: {i}").unwrap();
    }
    for c in 0..consumers {
        writeln!(
            code,
            "\nspec consumer_{c}\nuses cat: catalog\nrule total_{c}: cat.cells -> sum price"
        )
        .unwrap();
    }
    writeln!(code, "\nspec shop").unwrap();
    for c in 0..consumers {
        writeln!(code, "uses c{c}: consumer_{c}").unwrap();
    }
    writeln!(code, "rule cheapest: c0.total_0").unwrap();
    code
}

/// Pins full alphabetical transitive `needed_by_rules` for the wide shape.
#[test]
fn show_wide_200_needed_by_rules_alphabetical_transitive() {
    const N: usize = 200;
    let mut engine = Engine::with_limits(load_limits());
    engine
        .load([(SourceType::Volatile, wide_source(N))])
        .expect("wide N=200 must load");
    let show = engine
        .show(None, "bench_wide", None)
        .expect("show must succeed");

    let mut expected_d0: Vec<String> = vec!["p0".to_string()];
    expected_d0.extend((0..N).map(|i| format!("s{i}")));
    expected_d0.sort();
    assert_eq!(
        show.data.get("d0").expect("d0").needed_by_rules,
        expected_d0,
        "d0 must list p0 and every s* in byte order"
    );

    let mut expected_d199 = vec!["p199".to_string(), "s199".to_string()];
    expected_d199.sort();
    assert_eq!(
        show.data.get("d199").expect("d199").needed_by_rules,
        expected_d199,
        "d199 must list only p199 and s199"
    );
}

/// 10× measured nested-catalog rows=2000 consumers=40 debug load (1043 ms).
const NESTED_CATALOG_MAX: Duration = Duration::from_secs(11);

#[test]
fn load_nested_catalog_scales_with_unique_rows() {
    let elapsed = load_elapsed(nested_catalog_source(2000, 40));
    assert!(
        elapsed < NESTED_CATALOG_MAX,
        "nested catalog rows=2000 consumers=40 load {elapsed:?} exceeded bound {NESTED_CATALOG_MAX:?} (10× measured 1043ms)"
    );
}

//! Logistics ladder benches: embedded-catalog and external-catalog JSON.
//!
//! Prints two markdown tables (embedded first, then external). xtask parses both
//! from this single bench stdout.

use stats_alloc::{Region, Stats, StatsAlloc, INSTRUMENTED_SYSTEM};
use std::alloc::System;
use std::fmt::Write;
use std::time::{Duration, Instant};

#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

#[path = "../../tests/logistics/fixture.rs"]
mod fixture;

use fixture::LogisticsFixture;
use lemma::{parse_evaluate_data_object, Engine, SourceType};
use serde_json::Value;

const WARMUP_ITERATIONS: usize = 1;
const MEASURED_ITERATIONS: usize = 10;

fn live_bytes(stats: &Stats) -> i128 {
    stats.bytes_allocated as i128 - stats.bytes_deallocated as i128
}

fn median_ms(samples: &mut [Duration]) -> f64 {
    samples.sort();
    let middle = samples.len() / 2;
    let median = if samples.len().is_multiple_of(2) {
        (samples[middle - 1] + samples[middle]) / 2
    } else {
        samples[middle]
    };
    median.as_secs_f64() * 1_000.0
}

fn embedded_terminal_display(engine: &Engine) -> String {
    let rules = [LogisticsFixture::TERMINAL_RULE.to_string()];
    let response = engine
        .evaluate(
            None,
            LogisticsFixture::SPEC_NAME,
            None,
            LogisticsFixture::inputs(),
            Some(rules.as_slice()),
            false,
        )
        .expect("BUG: logistics fixture must evaluate");
    response
        .results
        .get(LogisticsFixture::TERMINAL_RULE)
        .expect("BUG: requested terminal rule missing from response")
        .result()
        .expect("BUG: logistics terminal rule must produce a value on the shipment inputs")
        .to_string()
}

fn external_cheapest_display(engine: &Engine, catalog_json: &Value) -> String {
    let data = parse_evaluate_data_object(Some(catalog_json.clone()))
        .expect("BUG: external catalog_json must parse");
    let rules = [LogisticsFixture::TERMINAL_RULE.to_string()];
    let response = engine
        .evaluate(
            None,
            LogisticsFixture::SPEC_NAME,
            None,
            data,
            Some(rules.as_slice()),
            false,
        )
        .expect("BUG: external logistics must evaluate");
    response
        .results
        .get(LogisticsFixture::TERMINAL_RULE)
        .expect("BUG: requested terminal rule missing from response")
        .result()
        .expect("BUG: logistics terminal rule must produce a value on the catalog JSON")
        .to_string()
}

fn embedded_cheapest(rate_cells: usize) -> String {
    let embedded = fixture::logistics(rate_cells);
    let mut engine = Engine::with_limits(embedded.limits.clone());
    engine
        .load([(SourceType::Volatile, embedded.source)])
        .expect("BUG: embedded logistics must load");
    embedded_terminal_display(&engine)
}

struct LogisticsRow {
    profile: &'static str,
    rate_cells: usize,
    source_bytes: usize,
    load_ms: f64,
    loaded_heap_bytes: i128,
    eval_ms: f64,
}

fn measure_embedded(rate_cells: usize) -> LogisticsRow {
    let embedded = fixture::logistics(rate_cells);
    assert!(
        embedded.catalog_json.is_none(),
        "BUG: embedded logistics must not carry catalog_json"
    );
    let source_bytes = embedded.source.len();

    let load_region = Region::new(GLOBAL);
    let load_started = Instant::now();
    let mut engine = Engine::with_limits(embedded.limits.clone());
    engine
        .load([(SourceType::Volatile, embedded.source)])
        .expect("BUG: logistics fixture must load");
    let load_ms = load_started.elapsed().as_secs_f64() * 1_000.0;
    let loaded_heap_bytes = live_bytes(&load_region.change());

    let expected_display = embedded_terminal_display(&engine);

    let mut eval_samples = Vec::with_capacity(MEASURED_ITERATIONS);
    for iteration in 0..WARMUP_ITERATIONS + MEASURED_ITERATIONS {
        let started = Instant::now();
        let display = embedded_terminal_display(&engine);
        assert_eq!(display, expected_display);
        let elapsed = started.elapsed();
        if iteration >= WARMUP_ITERATIONS {
            eval_samples.push(elapsed);
        }
    }

    LogisticsRow {
        profile: embedded.profile,
        rate_cells: embedded.rate_cells,
        source_bytes,
        load_ms,
        loaded_heap_bytes,
        eval_ms: median_ms(&mut eval_samples),
    }
}

fn measure_external(rate_cells: usize) -> LogisticsRow {
    let expected_display = embedded_cheapest(rate_cells);
    let skeleton = fixture::logistics_external_catalog(rate_cells);
    let catalog_json = skeleton
        .catalog_json
        .clone()
        .expect("BUG: external fixture must carry catalog_json");
    let source_bytes = skeleton.source.len();

    let load_region = Region::new(GLOBAL);
    let load_started = Instant::now();
    let mut engine = Engine::with_limits(skeleton.limits.clone());
    engine
        .load([(SourceType::Volatile, skeleton.source)])
        .expect("BUG: external logistics skeleton must load");
    let load_ms = load_started.elapsed().as_secs_f64() * 1_000.0;
    let loaded_heap_bytes = live_bytes(&load_region.change());

    let loaded_display = external_cheapest_display(&engine, &catalog_json);
    assert_eq!(
        loaded_display,
        expected_display,
        "BUG: external catalog evaluates {} differently from embedded",
        LogisticsFixture::TERMINAL_RULE
    );

    let mut eval_samples = Vec::with_capacity(MEASURED_ITERATIONS);
    for iteration in 0..WARMUP_ITERATIONS + MEASURED_ITERATIONS {
        let started = Instant::now();
        let display = external_cheapest_display(&engine, &catalog_json);
        assert_eq!(display, expected_display);
        let elapsed = started.elapsed();
        if iteration >= WARMUP_ITERATIONS {
            eval_samples.push(elapsed);
        }
    }

    LogisticsRow {
        profile: skeleton.profile,
        rate_cells: skeleton.rate_cells,
        source_bytes,
        load_ms,
        loaded_heap_bytes,
        eval_ms: median_ms(&mut eval_samples),
    }
}

fn main() {
    let mut table = String::new();
    writeln!(
        &mut table,
        "| Profile | Rate cells | Source bytes | Load | Loaded heap | Eval median |"
    )
    .expect("BUG: writing to String never fails");
    writeln!(
        &mut table,
        "|---------|-----------:|-------------:|-----:|------------:|------------:|"
    )
    .expect("BUG: writing to String never fails");

    for rung in &fixture::LADDER {
        let row = measure_embedded(rung.rate_cells);
        writeln!(
            &mut table,
            "| logistics_{} | {} | {} | {:.3} ms | {} | {:.3} ms |",
            row.profile,
            row.rate_cells,
            row.source_bytes,
            row.load_ms,
            row.loaded_heap_bytes,
            row.eval_ms,
        )
        .expect("BUG: writing to String never fails");
    }

    writeln!(
        &mut table,
        "| Profile | Rate cells | Source bytes | Load | Loaded heap | Eval median |"
    )
    .expect("BUG: writing to String never fails");
    writeln!(
        &mut table,
        "|---------|-----------:|-------------:|-----:|------------:|------------:|"
    )
    .expect("BUG: writing to String never fails");

    for rung in &fixture::LADDER {
        let row = measure_external(rung.rate_cells);
        writeln!(
            &mut table,
            "| logistics_ext_{} | {} | {} | {:.3} ms | {} | {:.3} ms |",
            row.profile,
            row.rate_cells,
            row.source_bytes,
            row.load_ms,
            row.loaded_heap_bytes,
            row.eval_ms,
        )
        .expect("BUG: writing to String never fails");
    }

    print!("{table}");
}

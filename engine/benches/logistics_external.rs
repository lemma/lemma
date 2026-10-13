//! Logistics ladder with catalog lists outside Lemma (JSON run data).
//!
//! Skeleton source keeps empty keyed lists. Timed eval = JSON parse + `Engine::evaluate`.
//! Also reports load / snapshot / restore of the skeleton. Each rung must match the
//! embedded-catalog fixture's `rate_shop.cheapest` display.

use stats_alloc::{Region, Stats, StatsAlloc, INSTRUMENTED_SYSTEM};
use std::alloc::System;
use std::fmt::Write;
use std::time::{Duration, Instant};

#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

use lemma_fixtures::logistics;

use lemma::{parse_evaluate_data_object, Engine, SourceType};
use logistics::LogisticsFixture;
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

fn cheapest_display(engine: &Engine, catalog_json: &Value) -> String {
    let data = parse_evaluate_data_object(&Some(catalog_json.clone()))
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
    let fixture = logistics::logistics(rate_cells);
    let mut engine = Engine::with_limits(fixture.limits.clone());
    engine
        .load([(SourceType::Volatile, fixture.source)])
        .expect("BUG: embedded logistics must load");
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
        .expect("BUG: embedded logistics must evaluate");
    response
        .results
        .get(LogisticsFixture::TERMINAL_RULE)
        .expect("BUG: requested terminal rule missing from response")
        .result()
        .expect("BUG: embedded terminal must produce a value")
        .to_string()
}

struct Row {
    profile: &'static str,
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

fn measure(rate_cells: usize) -> Row {
    let expected_display = embedded_cheapest(rate_cells);
    let fixture = logistics::logistics_external_catalog(rate_cells);
    let catalog_json = fixture
        .catalog_json
        .clone()
        .expect("BUG: external fixture must carry catalog_json");
    let source_bytes = fixture.source.len();

    let load_region = Region::new(GLOBAL);
    let load_started = Instant::now();
    let mut engine = Engine::with_limits(fixture.limits.clone());
    engine
        .load([(SourceType::Volatile, fixture.source)])
        .expect("BUG: external logistics skeleton must load");
    let load_ms = load_started.elapsed().as_secs_f64() * 1_000.0;
    let loaded_heap_bytes = live_bytes(&load_region.change());

    let loaded_display = cheapest_display(&engine, &catalog_json);
    assert_eq!(
        loaded_display,
        expected_display,
        "BUG: external catalog evaluates {} differently from embedded",
        LogisticsFixture::TERMINAL_RULE
    );

    let mut encode_samples = Vec::with_capacity(MEASURED_ITERATIONS);
    let mut snapshot = Vec::new();
    for iteration in 0..WARMUP_ITERATIONS + MEASURED_ITERATIONS {
        let started = Instant::now();
        snapshot = engine.snapshot().expect("BUG: loaded engine must snapshot");
        let elapsed = started.elapsed();
        if iteration >= WARMUP_ITERATIONS {
            encode_samples.push(elapsed);
        }
    }
    let snapshot_bytes = snapshot.len();

    let mut eval_samples = Vec::with_capacity(MEASURED_ITERATIONS);
    for iteration in 0..WARMUP_ITERATIONS + MEASURED_ITERATIONS {
        let started = Instant::now();
        let data = parse_evaluate_data_object(&Some(catalog_json.clone()))
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
            .expect("BUG: timed external eval must succeed");
        let display = response
            .results
            .get(LogisticsFixture::TERMINAL_RULE)
            .expect("BUG: terminal missing")
            .result()
            .expect("BUG: terminal must produce a value")
            .to_string();
        assert_eq!(display, expected_display);
        let elapsed = started.elapsed();
        if iteration >= WARMUP_ITERATIONS {
            eval_samples.push(elapsed);
        }
    }
    drop(engine);

    let mut restore_samples = Vec::with_capacity(MEASURED_ITERATIONS);
    let mut restored_heap_bytes = 0i128;
    let mut restored = None;
    for iteration in 0..WARMUP_ITERATIONS + MEASURED_ITERATIONS {
        drop(restored.take());
        let region = Region::new(GLOBAL);
        let started = Instant::now();
        let engine = Engine::from_snapshot(&snapshot).expect("BUG: snapshot must restore");
        let elapsed = started.elapsed();
        let stats = region.change();
        if iteration >= WARMUP_ITERATIONS {
            restore_samples.push(elapsed);
            restored_heap_bytes = live_bytes(&stats);
        }
        restored = Some(engine);
    }
    let restored = restored.expect("BUG: at least one restore iteration ran");
    let restored_display = cheapest_display(&restored, &catalog_json);
    assert_eq!(
        restored_display,
        expected_display,
        "BUG: restored external engine evaluates {} differently from embedded",
        LogisticsFixture::TERMINAL_RULE
    );
    drop(restored);

    Row {
        profile: fixture.profile,
        rate_cells: fixture.rate_cells,
        source_bytes,
        load_ms,
        loaded_heap_bytes,
        snapshot_bytes,
        encode_ms: median_ms(&mut encode_samples),
        restore_ms: median_ms(&mut restore_samples),
        eval_ms: median_ms(&mut eval_samples),
        restored_heap_bytes,
    }
}

fn main() {
    let mut table = String::new();
    writeln!(
        &mut table,
        "| Profile | Rate cells | Source bytes | Load | Loaded heap | Snapshot bytes | Encode median | Restore median | Eval median | Restored heap | Restored / loaded heap |"
    )
    .expect("BUG: writing to String never fails");
    writeln!(
        &mut table,
        "|---------|-----------:|-------------:|-----:|------------:|---------------:|--------------:|---------------:|------------:|--------------:|-----------------------:|"
    )
    .expect("BUG: writing to String never fails");

    for rung in &logistics::LADDER {
        let row = measure(rung.rate_cells);
        writeln!(
            &mut table,
            "| logistics_ext_{} | {} | {} | {:.3} ms | {} | {} | {:.3} ms | {:.3} ms | {:.3} ms | {} | {:.3} |",
            row.profile,
            row.rate_cells,
            row.source_bytes,
            row.load_ms,
            row.loaded_heap_bytes,
            row.snapshot_bytes,
            row.encode_ms,
            row.restore_ms,
            row.eval_ms,
            row.restored_heap_bytes,
            row.restored_heap_bytes as f64 / row.loaded_heap_bytes as f64,
        )
        .expect("BUG: writing to String never fails");
    }

    print!("{table}");
}

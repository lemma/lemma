//! Snapshot encode / restore on the logistics ladder (1050 to 126000 rate cells).
//!
//! Prints one markdown row per rung: load time and live heap of the loaded
//! engine, snapshot bytes, encode and restore medians, allocations per restore,
//! live heap of the restored engine and its ratio to the loaded engine. The
//! restored engine must evaluate the terminal rule to the same display string as
//! the loaded engine; a mismatch is a bug and aborts the bench.

use stats_alloc::{Region, Stats, StatsAlloc, INSTRUMENTED_SYSTEM};
use std::alloc::System;
use std::fmt::Write;
use std::time::{Duration, Instant};

#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

#[path = "common/logistics.rs"]
mod logistics;

use lemma::{Engine, SourceType};
use logistics::LogisticsFixture;

const WARMUP_ITERATIONS: usize = 1;
const MEASURED_ITERATIONS: usize = 10;

/// `stats_alloc` already folds reallocation growth and shrinkage into
/// `bytes_allocated` / `bytes_deallocated`.
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

fn terminal_display(engine: &Engine) -> String {
    let rules = [LogisticsFixture::TERMINAL_RULE.to_string()];
    let response = engine
        .run(
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

struct Row {
    profile: &'static str,
    rate_cells: usize,
    source_bytes: usize,
    load_ms: f64,
    loaded_heap_bytes: i128,
    snapshot_bytes: usize,
    encode_ms: f64,
    restore_ms: f64,
    restore_allocations: usize,
    restored_heap_bytes: i128,
}

fn measure(rate_cells: usize) -> Row {
    let fixture = logistics::logistics(rate_cells);
    let source_bytes = fixture.source.len();

    let load_region = Region::new(GLOBAL);
    let load_started = Instant::now();
    let mut engine = Engine::with_limits(fixture.limits.clone());
    engine
        .load([(SourceType::Volatile, fixture.source)])
        .expect("BUG: logistics fixture must load");
    let load_ms = load_started.elapsed().as_secs_f64() * 1_000.0;
    let loaded_heap_bytes = live_bytes(&load_region.change());

    let expected_display = terminal_display(&engine);

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
    drop(engine);

    let mut restore_samples = Vec::with_capacity(MEASURED_ITERATIONS);
    let mut restore_allocations = 0usize;
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
            restore_allocations += stats.allocations;
            restored_heap_bytes = live_bytes(&stats);
        }
        restored = Some(engine);
    }
    let restored = restored.expect("BUG: at least one restore iteration ran");
    let restored_display = terminal_display(&restored);
    assert_eq!(
        restored_display,
        expected_display,
        "BUG: restored engine evaluates {} differently from the loaded engine",
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
        restore_allocations: restore_allocations / MEASURED_ITERATIONS,
        restored_heap_bytes,
    }
}

fn main() {
    let mut table = String::new();
    writeln!(
        &mut table,
        "| Profile | Rate cells | Source bytes | Load | Loaded heap | Snapshot bytes | Encode median | Restore median | Allocations/restore | Restored heap | Restored / loaded heap |"
    )
    .expect("BUG: writing to String never fails");
    writeln!(
        &mut table,
        "|---------|-----------:|-------------:|-----:|------------:|---------------:|--------------:|---------------:|--------------------:|--------------:|-----------------------:|"
    )
    .expect("BUG: writing to String never fails");

    for rung in &logistics::LADDER {
        let row = measure(rung.rate_cells);
        writeln!(
            &mut table,
            "| logistics_{} | {} | {} | {:.3} ms | {} | {} | {:.3} ms | {:.3} ms | {} | {} | {:.3} |",
            row.profile,
            row.rate_cells,
            row.source_bytes,
            row.load_ms,
            row.loaded_heap_bytes,
            row.snapshot_bytes,
            row.encode_ms,
            row.restore_ms,
            row.restore_allocations,
            row.restored_heap_bytes,
            row.restored_heap_bytes as f64 / row.loaded_heap_bytes as f64,
        )
        .expect("BUG: writing to String never fails");
    }

    print!("{table}");
}

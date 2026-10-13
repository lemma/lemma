use stats_alloc::{Region, StatsAlloc, INSTRUMENTED_SYSTEM};
use std::alloc::System;
use std::fmt::Write;

#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

mod common;
#[path = "common/eval.rs"]
mod eval;

const EVAL_WARMUP_ITERATIONS: usize = 100;
const EVAL_MEASURED_ITERATIONS: usize = 1_000;
const PLAN_WARMUP_ITERATIONS: usize = 5;
const PLAN_MEASURED_ITERATIONS: usize = 50;

struct IterationStats {
    allocations: usize,
    bytes_allocated: usize,
    reallocations: usize,
}

/// Mean allocation counts of `iteration` over `measured` runs, after `warmup` unmeasured runs.
fn measure(warmup: usize, measured: usize, mut iteration: impl FnMut()) -> IterationStats {
    for _ in 0..warmup {
        iteration();
    }
    let mut totals = IterationStats {
        allocations: 0,
        bytes_allocated: 0,
        reallocations: 0,
    };
    for _ in 0..measured {
        let region = Region::new(GLOBAL);
        iteration();
        let stats = region.change();
        totals.allocations += stats.allocations;
        totals.bytes_allocated += stats.bytes_allocated;
        totals.reallocations += stats.reallocations;
    }
    totals
}

fn push_header(table: &mut String, unit: &str) {
    writeln!(
        table,
        "| Spec | Iterations | Allocations/{unit} | Bytes allocated/{unit} | Reallocations/{unit} |"
    )
    .expect("BUG: writing to String never fails");
    writeln!(
        table,
        "|------|-----------:|-----------------:|---------------------:|-------------------:|"
    )
    .expect("BUG: writing to String never fails");
}

fn push_row(table: &mut String, spec_name: &str, measured: usize, totals: &IterationStats) {
    let n = measured as f64;
    writeln!(
        table,
        "| {} | {} | {:.2} | {:.0} | {:.2} |",
        spec_name,
        measured,
        totals.allocations as f64 / n,
        totals.bytes_allocated as f64 / n,
        totals.reallocations as f64 / n,
    )
    .expect("BUG: writing to String never fails");
}

fn main() {
    let fixtures = common::fixtures();

    let mut eval_table = String::new();
    push_header(&mut eval_table, "eval");
    for fixture in &fixtures {
        let engine = eval::load_engine(fixture);
        let rules = [eval::terminal_rule(fixture.spec_name).to_string()];
        let totals = measure(EVAL_WARMUP_ITERATIONS, EVAL_MEASURED_ITERATIONS, || {
            let data = fixture.inputs();
            let response = eval::evaluate_loaded(&engine, fixture.spec_name, data, &rules, false);
            std::hint::black_box(response);
        });
        push_row(
            &mut eval_table,
            fixture.spec_name,
            EVAL_MEASURED_ITERATIONS,
            &totals,
        );
    }

    let mut plan_table = String::new();
    push_header(&mut plan_table, "plan");
    for fixture in &fixtures {
        let totals = measure(PLAN_WARMUP_ITERATIONS, PLAN_MEASURED_ITERATIONS, || {
            std::hint::black_box(eval::load_engine(fixture));
        });
        push_row(
            &mut plan_table,
            fixture.spec_name,
            PLAN_MEASURED_ITERATIONS,
            &totals,
        );
    }

    print!("{eval_table}\n{plan_table}");
}

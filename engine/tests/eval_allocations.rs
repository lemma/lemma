//! Pin per-evaluate allocation count for the shipping fixture.
//!
//! Evaluation state is one value table sized from the plan. Any regression that
//! reintroduces per-request HashMaps / Arc-per-result / measure signature Vecs
//! will raise this count.

use lemma::{DateGranularity, DateTimeValue, Engine, SourceType};
use stats_alloc::{Region, StatsAlloc, INSTRUMENTED_SYSTEM};
use std::alloc::System;
use std::collections::HashMap;
use std::sync::Arc;

#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

const SHIPPING: &str = include_str!("../benches/specs/shipping.lemma");

/// Exact allocations for one `Engine::run` of `bench_shipping` / `total`
/// after warmup, on this machine's allocator accounting. Update deliberately
/// when the eval path's allocation shape changes.
///
/// Measured 52: public `Engine::run` takes `HashMap<String, String>` and maps
/// each overlay to `RunDataValue::string` before resolve; display binding on
/// values is still `Option<Arc<str>>`.
const SHIPPING_EVAL_ALLOCATIONS: usize = 52;

fn shipping_effective() -> DateTimeValue {
    DateTimeValue {
        year: 2026,
        month: 1,
        day: 1,
        hour: 0,
        minute: 0,
        second: 0,
        microsecond: 0,
        timezone: None,
        granularity: DateGranularity::Full,
    }
}

fn shipping_inputs() -> HashMap<String, String> {
    HashMap::from([
        ("weight".into(), "3".into()),
        ("destination".into(), "domestic".into()),
        ("is_member".into(), "false".into()),
    ])
}

fn run_once(engine: &Engine) -> lemma::Response {
    engine
        .run(
            None,
            "bench_shipping",
            Some(&shipping_effective()),
            shipping_inputs(),
            Some(&["total".to_string()]),
            false,
        )
        .expect("shipping fixture must evaluate")
}

#[test]
fn shipping_evaluate_allocation_count() {
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Path(Arc::new(std::path::PathBuf::from("shipping.lemma"))),
            SHIPPING.to_string(),
        )])
        .expect("shipping fixture must load");

    for _ in 0..100 {
        std::hint::black_box(run_once(&engine));
    }

    let mut total = 0usize;
    const N: usize = 200;
    for _ in 0..N {
        let region = Region::new(GLOBAL);
        std::hint::black_box(run_once(&engine));
        total += region.change().allocations;
    }
    let avg = total / N;
    assert_eq!(
        avg, SHIPPING_EVAL_ALLOCATIONS,
        "shipping evaluate allocations changed: got {avg} (total {total} over {N}), \
         expected {SHIPPING_EVAL_ALLOCATIONS}. Update SHIPPING_EVAL_ALLOCATIONS deliberately \
         if the eval allocation shape changed."
    );
}

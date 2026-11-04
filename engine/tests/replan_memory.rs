//! Replanning must not grow memory. Each cycle edits, re-parses, adds and removes
//! versions and specs, and fails one batch, then returns the sources to the state the
//! cycle started from. Heap the engine holds must not grow across cycles. That replaced
//! specs and plans are freed is pinned by `Weak` handles in the planning tests.

use lemma::{DateGranularity, DateTimeValue, Engine, SourceType};
use stats_alloc::{Region, StatsAlloc, INSTRUMENTED_SYSTEM};
use std::alloc::System;
use std::sync::Arc;

#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

const LEAF: &str =
    "spec leaf\ndata v: 1\nrule r: v\n\nspec leaf 2025-01-01\ndata v: 2\nrule r: v\n";
const LEAF_EDITED: &str =
    "spec leaf\ndata v: 1\nrule r: v\n\nspec leaf 2025-01-01\ndata v: 5\nrule r: v\n";
const LEAF_2027: &str = "spec leaf 2027-01-01\ndata v: 7\nrule r: v\n";
const ITEM: &str = "spec item\nuses l: leaf\ndata amount: number\nrule total: amount * l.r\n";
const ITEM_BROKEN: &str =
    "spec item\nuses l: leaf\ndata amount: number\nrule total: amount * l.missing\n";
const BOX: &str = "spec box\nuses item\ndata items: item list\nrule item_count: items -> count\n";
const ROOT: &str = "spec root\nuses b: box\nuses i: item\n  -> with amount: 3\nrule out: i.total\nrule boxed: b.item_count\n";
const PINNED: &str = "spec pinned\nuses l: leaf 2024-06-01\nrule out: l.r\n";
const EXTRA: &str = "spec extra\nuses i: item\n  -> with amount: 2\nrule out: i.total\n";

fn path(name: &str) -> SourceType {
    SourceType::Path(Arc::new(std::path::PathBuf::from(name)))
}

fn day(year: i32, month: u32, day: u32) -> DateTimeValue {
    DateTimeValue {
        year,
        month,
        day,
        hour: 0,
        minute: 0,
        second: 0,
        microsecond: 0,
        timezone: None,
        granularity: DateGranularity::Full,
    }
}

fn workspace_sources() -> Vec<(SourceType, &'static str)> {
    vec![
        (path("leaf.lemma"), LEAF),
        (path("item.lemma"), ITEM),
        (path("box.lemma"), BOX),
        (path("root.lemma"), ROOT),
        (path("pinned.lemma"), PINNED),
    ]
}

fn loaded_engine() -> Engine {
    let mut engine = Engine::new();
    engine
        .load(workspace_sources())
        .expect("replan workspace loads");
    engine
}

#[derive(Clone, Copy, Debug)]
enum ReplanStep {
    EditLeaf,
    RevertLeaf,
    ReparseUnchangedItem,
    LoadLeafVersion,
    RemoveLeafVersion,
    FailedItemBatch,
    LoadExtraSpec,
    RemoveExtraSpec,
}

const CYCLE: [ReplanStep; 8] = [
    ReplanStep::EditLeaf,
    ReplanStep::RevertLeaf,
    ReplanStep::ReparseUnchangedItem,
    ReplanStep::LoadLeafVersion,
    ReplanStep::RemoveLeafVersion,
    ReplanStep::FailedItemBatch,
    ReplanStep::LoadExtraSpec,
    ReplanStep::RemoveExtraSpec,
];

fn apply_step(engine: &mut Engine, step: ReplanStep) {
    match step {
        ReplanStep::EditLeaf => engine
            .update(None, LEAF_EDITED.to_string(), path("leaf.lemma"))
            .expect("leaf edit applies"),
        ReplanStep::RevertLeaf => engine
            .update(None, LEAF.to_string(), path("leaf.lemma"))
            .expect("leaf revert applies"),
        ReplanStep::ReparseUnchangedItem => engine
            .update(None, ITEM.to_string(), path("item.lemma"))
            .expect("unchanged item applies"),
        ReplanStep::LoadLeafVersion => engine
            .load([(path("leaf_2027.lemma"), LEAF_2027)])
            .expect("leaf version loads"),
        ReplanStep::RemoveLeafVersion => engine
            .remove(None, "leaf", Some(&day(2027, 6, 1)))
            .expect("leaf version removes"),
        ReplanStep::FailedItemBatch => {
            engine
                .update(None, ITEM_BROKEN.to_string(), path("item.lemma"))
                .expect_err("an item reading a missing rule fails to plan");
        }
        ReplanStep::LoadExtraSpec => engine
            .load([(path("extra.lemma"), EXTRA)])
            .expect("extra spec loads"),
        ReplanStep::RemoveExtraSpec => engine
            .remove(None, "extra", None)
            .expect("extra spec removes"),
    }
}

/// `stats_alloc` already books each `realloc` growth into `bytes_allocated` and each
/// shrink into `bytes_deallocated`.
fn net_bytes(stats: stats_alloc::Stats) -> isize {
    let allocated = isize::try_from(stats.bytes_allocated).expect("allocated bytes fit isize");
    let deallocated =
        isize::try_from(stats.bytes_deallocated).expect("deallocated bytes fit isize");
    allocated - deallocated
}

/// After warmup, further cycles must leave the engine's heap exactly where it was.
#[test]
fn replan_cycles_retain_no_heap() {
    let mut engine = loaded_engine();
    for _ in 0..5 {
        for step in CYCLE {
            apply_step(&mut engine, step);
        }
    }
    let mut retained_by_step = [0isize; CYCLE.len()];
    for _ in 0..50 {
        for (step, retained) in CYCLE.iter().zip(retained_by_step.iter_mut()) {
            let region = Region::new(GLOBAL);
            apply_step(&mut engine, *step);
            *retained += net_bytes(region.change());
        }
    }
    let retained: isize = retained_by_step.iter().sum();
    let by_step: Vec<String> = CYCLE
        .iter()
        .zip(retained_by_step)
        .map(|(step, bytes)| format!("{step:?}: {bytes}"))
        .collect();
    assert_eq!(
        retained,
        0,
        "50 replan cycles retained {retained} bytes of heap; by step: {}",
        by_step.join(", ")
    );
}

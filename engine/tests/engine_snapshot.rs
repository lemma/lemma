//! Engine binary snapshot: round-trip, determinism, idempotence, corrupt/stale bytes.

use lemma::{api, DateTimeValue, Engine, ResourceLimits, SourceType};
use std::collections::HashMap;
use std::str::FromStr;
use std::time::Duration;

fn load_limits() -> ResourceLimits {
    ResourceLimits {
        max_source_size_bytes: 100 * 1024 * 1024,
        max_expression_count: 500_000,
        max_normalized_expression_nodes: 500_000,
        ..ResourceLimits::default()
    }
}

fn effective_2024() -> DateTimeValue {
    DateTimeValue::from_str("2024-06-01").expect("effective")
}

fn rich_workspace() -> Engine {
    let mut engine = Engine::with_limits(load_limits());
    engine
        .load([(
            SourceType::Volatile,
            r#"
spec money
data currency: measure
  -> unit eur: 1
  -> unit cent: 0.01

spec pricing 2024-01-01
data amount: money.currency
rule doubled: amount * 2

spec pricing 2025-01-01
data amount: money.currency
rule doubled: amount * 3

spec checkout 2024-01-01
uses p: pricing
uses m: money

data qty: number
data unit_price: m.currency
  -> suggest 10 eur

rule line_total: qty * unit_price
rule with_tax: line_total * 1.21
rule flagged: veto "blocked"
  unless qty is 0 then 0
  unless qty > 100 then qty

rule bracket: qty
  unless qty is 1 then 10
  unless qty is 2 then 20
  unless qty is 3 then 30
  unless qty is 5 then 50
  unless qty is 8 then 80
"#
            .to_string(),
        )])
        .expect("rich workspace must load");
    engine
}

#[test]
fn snapshot_round_trip_preserves_list_show_run() {
    let engine = rich_workspace();
    let list_before = engine.list();
    let show_before = engine
        .show(None, "checkout", Some(&effective_2024()))
        .expect("show checkout");
    let show_json_before = serde_json::to_value(api::Show::from(&show_before)).expect("show json");
    let mut data = HashMap::new();
    data.insert("qty".to_string(), "2".into());
    data.insert("unit_price".to_string(), "10 eur".into());
    let run_before = engine
        .run(
            None,
            "checkout",
            Some(&effective_2024()),
            data.clone(),
            None,
            false,
        )
        .expect("run");
    let run_json_before = serde_json::to_value(api::Response::from(&run_before)).expect("run json");

    let bytes = engine.snapshot().expect("snapshot");
    let restored = Engine::from_snapshot(&bytes).expect("from_snapshot");

    assert_eq!(restored.list(), list_before);
    let show_after = restored
        .show(None, "checkout", Some(&effective_2024()))
        .expect("show after");
    assert_eq!(
        serde_json::to_value(api::Show::from(&show_after)).expect("show json after"),
        show_json_before
    );
    let run_after = restored
        .run(None, "checkout", Some(&effective_2024()), data, None, false)
        .expect("run after");
    assert_eq!(
        serde_json::to_value(api::Response::from(&run_after)).expect("run json after"),
        run_json_before
    );
}

#[test]
fn snapshot_is_idempotent() {
    let engine = rich_workspace();
    let bytes = engine.snapshot().expect("snapshot");
    let restored = Engine::from_snapshot(&bytes).expect("restore");
    let again = restored.snapshot().expect("re-snapshot");
    assert_eq!(bytes, again);
}

#[test]
fn same_sources_produce_identical_snapshot_bytes() {
    let left = rich_workspace();
    let right = rich_workspace();
    assert_eq!(
        left.snapshot().expect("left"),
        right.snapshot().expect("right")
    );
}

#[test]
fn restored_engine_accepts_update_and_remove() {
    let engine = rich_workspace();
    let bytes = engine.snapshot().expect("snapshot");
    let mut restored = Engine::from_snapshot(&bytes).expect("restore");
    restored
        .update(
            None,
            r#"
spec money
data currency: measure
  -> unit eur: 1
  -> unit cent: 0.01

spec pricing 2024-01-01
data amount: money.currency
rule doubled: amount * 2

spec pricing 2025-01-01
data amount: money.currency
rule doubled: amount * 3

spec checkout 2024-01-01
uses p: pricing
uses m: money

data qty: number
data unit_price: m.currency

rule line_total: qty * unit_price
"#
            .to_string(),
            SourceType::Volatile,
        )
        .expect("update must succeed");
    restored
        .remove(None, "checkout", Some(&effective_2024()))
        .expect("remove must succeed");
    assert!(
        restored
            .show(None, "checkout", Some(&effective_2024()))
            .is_err(),
        "checkout must be gone after remove"
    );
}

#[test]
fn wrong_magic_is_error() {
    let engine = rich_workspace();
    let mut bytes = engine.snapshot().expect("snapshot");
    bytes[0] = b'X';
    match Engine::from_snapshot(&bytes) {
        Ok(_) => panic!("wrong magic must fail"),
        Err(err) => assert!(
            err.message().contains("magic"),
            "unexpected: {}",
            err.message()
        ),
    }
}

#[test]
fn truncated_snapshot_is_error() {
    let engine = rich_workspace();
    let bytes = engine.snapshot().expect("snapshot");
    match Engine::from_snapshot(&bytes[..bytes.len() / 3]) {
        Ok(_) => panic!("truncated must fail"),
        Err(err) => assert!(
            err.message().contains("checksum mismatch"),
            "unexpected: {}",
            err.message()
        ),
    }
}

#[test]
fn flipped_crc_byte_is_error() {
    let engine = rich_workspace();
    let mut bytes = engine.snapshot().expect("snapshot");
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    match Engine::from_snapshot(&bytes) {
        Ok(_) => panic!("crc mismatch must fail"),
        Err(err) => assert!(
            err.message().contains("checksum mismatch"),
            "unexpected: {}",
            err.message()
        ),
    }
}

#[test]
fn wide_5000_snapshot_round_trips() {
    const N: usize = 5000;
    let mut code = String::with_capacity(N * 80);
    use std::fmt::Write;
    writeln!(code, "spec bench_wide").unwrap();
    for i in 0..N {
        writeln!(code, "data d{i}: number").unwrap();
    }
    for i in 0..N {
        writeln!(code, "rule p{i}: d{i} * 2").unwrap();
    }
    writeln!(code, "rule s0: p0").unwrap();
    for i in 1..N {
        writeln!(code, "rule s{i}: s{} + p{i}", i - 1).unwrap();
    }

    let mut engine = Engine::with_limits(load_limits());
    let start = std::time::Instant::now();
    engine
        .load([(SourceType::Volatile, code)])
        .expect("wide load");
    let load_elapsed = start.elapsed();
    assert!(
        load_elapsed < Duration::from_secs(30),
        "wide load too slow for snapshot test: {load_elapsed:?}"
    );

    let bytes = engine.snapshot().expect("snapshot wide");
    let restored = Engine::from_snapshot(&bytes).expect("restore wide");
    let show = restored.show(None, "bench_wide", None).expect("show wide");
    assert_eq!(show.data.len(), N);
    assert!(show.rules.contains_key("s4999"));
}

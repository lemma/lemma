---
nav_title: Rust
nav_order: 10
---

# Rust

`lemma-engine` is the engine itself. Crate name `lemma-engine`, imported as `lemma`.

## Install

```bash
cargo add lemma-engine --rename lemma
```

## Usage

```rust
use lemma::{DateTimeValue, Engine, SourceType};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

let mut engine = Engine::new();

engine.load([(
    SourceType::Path(Arc::new(PathBuf::from("example.lemma"))),
    r#"
    spec compensation
    data base_salary: 60000
    data bonus_rate: 10%
    rule bonus: base_salary * bonus_rate
    rule total: base_salary + bonus
"#
    .to_string(),
)])?;

let now = DateTimeValue::now();
let response = engine.run(
    None,
    "compensation",
    Some(&now),
    HashMap::new(),
    None,
    false,
)?;

for (rule_name, rule_result) in &response.results {
    if !rule_result.vetoed {
        println!("{rule_name}: {}", rule_result.result().unwrap_or(""));
    }
}
```

## Providing values at runtime

```rust
use lemma::{DateTimeValue, Engine, SourceType};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

let mut engine = Engine::new();

engine.load([(
    SourceType::Path(Arc::new(PathBuf::from("example.lemma"))),
    r#"
    spec shipping

    uses lemma units

    data weight: 5 kilogram
    data destination: "domestic"

    rule rate: 10
      unless weight > 10 kilogram           then 15
      unless destination is "international" then 25

    rule valid: weight <= 30 kilogram
      unless weight > 30 kilogram then veto "Package too heavy for shipping"
"#
    .to_string(),
)])?;

let mut values = HashMap::new();
values.insert("weight".to_string(), "12 kilogram".to_string());
values.insert("destination".to_string(), "international".to_string());

let now = DateTimeValue::now();
let response = engine.run(
    None,
    "shipping",
    Some(&now),
    values,
    None,
    false,
)?;
```

## Show vs run discovery

`Engine::show` returns the static planning catalog: every declared promptable data slot (with `path` for import identity), plus this spec's rule graph as `ShowRule` (`type`, `path`, `branches`, `depends_on_rules` as `input_key` — local plus reachable imports). Empty `needed_by_rules` means offered for reuse (`data x: alias.slot`), not needed by this spec's remaining rules. Default `run` evaluates empty-path rules; pass any Show key (e.g. `src.computed`) to target a reachable import.

For requirements on a partial run, call `run` and inspect each rule's `missing_data` (`string[]` input keys in evaluation / decision-tree order; first key is the next fact the live tree needs). Types, filled literals, and `-> suggest` hints are on `Engine::show` (`Show.data` values are `ShowData`) only. Bound inputs (caller run bindings or spec-filled values) are omitted from `missing_data`; suggestions do not bind until supplied in `run`'s data. Non-veto rule results flatten `RuleResultValue` onto each result (`result()` / typed fields). Pass `explain: true` as the last `run` argument to attach per-rule explanation trees ([api.v1.json](../../../engine/schemas/api.v1.json)).

```rust
let response = engine.run(
    None,
    "shipping",
    Some(&now),
    HashMap::new(),
    Some(&["rate".to_string()]),
    false,
)?;

for key in &response.results["rate"].missing_data {
    println!("need: {key}");
}
```

## Embedded units stdlib

`Engine::new()` loads `repo lemma` / `spec units` at compile time (import with `uses lemma units`). It always appears in `Engine::list`. Formatted source: `engine.source(Some("lemma"), None, None)?`.

## Binary snapshot

After `load` / `update`, the engine holds parsed specs and compiled execution plans. `snapshot` writes that state to bytes so a later process can `from_snapshot` and call `run` / `show` / `list` without re-parsing or re-planning. The restored engine also accepts further `update` / `remove`.

Bytes carry a version header (`CARGO_PKG_VERSION`) and a CRC32. A snapshot from another engine version, or corrupt bytes, returns `Error`. Same sources loaded into two engines produce identical snapshot bytes.

### Write to disk and restore

```rust
use lemma::{DateTimeValue, Engine, SourceType};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

fn build_and_save(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut engine = Engine::new();
    engine.load([(
        SourceType::Volatile,
        r#"
        spec shipping
        uses lemma units
        data weight: number
        rule rate: 10
          unless weight > 10 then 15
        "#
        .to_string(),
    )])?;

    fs::write(path, engine.snapshot()?)?;
    Ok(())
}

fn load_and_run(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = fs::read(path)?;
    let engine = Engine::from_snapshot(&bytes)?;

    let mut values = HashMap::new();
    values.insert("weight".to_string(), "12".to_string());
    let now = DateTimeValue::now();
    let response = engine.run(None, "shipping", Some(&now), values, None, false)?;

    println!("{}", response.results["rate"].result().unwrap_or(""));
    Ok(())
}
```

In-process only (no filesystem):

```rust
let bytes = engine.snapshot()?;
let restored = Engine::from_snapshot(&bytes)?;
```

JSON documents for `show` / `run` / bindings use `lemma::api` (`api::Show`, `api::Response`, …). Domain types keep exact serde for the snapshot path.

Same opaque bytes on the language SDKs: [JavaScript](javascript.md), [Java](java.md), [Elixir](elixir.md).

## API docs

Full Rust API documentation is published on [docs.rs](https://docs.rs/lemma-engine).

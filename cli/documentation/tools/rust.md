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
use lemma::{DateTimeValue, Engine, IndexMap, SourceType};
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
let response = engine.evaluate(
    None,
    "compensation",
    Some(&now),
    IndexMap::new(),
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
use lemma::{DateTimeValue, Engine, IndexMap, SourceType};
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

let mut values = IndexMap::new();
values.insert("weight".to_string(), "12 kilogram".into());
values.insert("destination".to_string(), "international".into());

let now = DateTimeValue::now();
let response = engine.evaluate(
    None,
    "shipping",
    Some(&now),
    values,
    None,
    false,
)?;
```

## Evaluation data

`evaluate` takes an `IndexMap<String, EvaluateDataValue>`. An `EvaluateDataValue` is a `Literal` (Lemma literal text such as `"12 kilogram"`, `"84"`, `"2025-03-01"`), a `Boolean`, a `Map` (a single-unit map like `{"eur": "84"}` for a measure or ratio) or a `List`. `&str` and `String` convert into `Literal` with `.into()`. Numbers travel as text so that they stay exact; see [Numeric precision](../learn/precision.md).

A list is one key holding a `List`: `Literal` items for a value list, `Map` items keyed by the row spec's data names for an instance list. The list replaces every row written in the spec. In the response, a list-valued rule carries `list` and a row-valued rule carries `row`, whose `index` points into `Response.rows`, where each referenced row appears once with its `data` and `rules`. See [Lists](../learn/lists.md).

## Show vs evaluate discovery

`Engine::show` returns the static planning catalog: `Show.types` (local data declarations), `Show.data` (inputs rules need: non-empty `needed_by_rules`, with `path` for import identity), plus this spec's rule graph as `ShowRule` (`type`, `path`, `branches`, `depends_on_rules` as `input_key`; local plus reachable imports). `lemma::api::show_document(&engine, &show)` turns it into the JSON document every surface returns: a Draft 2020-12 JSON Schema for the evaluation data (`properties`, `required`, `$defs` row schemas) with Lemma metadata under `x-lemma-*` (`x-lemma-type`, `x-lemma-path`, `x-lemma-needed-by-rules`, `x-lemma-value`, `x-lemma-suggestion`, `x-lemma-types`, `x-lemma-rules`, ...). On `$defs` row properties, `x-lemma-needed-by-rules` are parent Show.rules keys that need that field through the list. Default `evaluate` evaluates empty-path rules; pass any Show key (e.g. `src.computed`) to target a reachable import.

For requirements on a partial evaluation, call `evaluate` and inspect each rule's `missing_data` (`string[]` input keys in evaluation / decision-tree order; first key is the next fact the live tree needs). Local type catalog, filled literals, and `-> suggest` hints are on `Engine::show` (`Show.types` / `Show.data` values are `ShowData`) only. Bound inputs (caller evaluation bindings or spec-filled values) are omitted from `missing_data`; suggestions do not bind until supplied in `evaluate`'s data. Non-veto rule results flatten `RuleResultValue` onto each result (`result()` / typed fields). Pass `explain: true` as the last `evaluate` argument to attach per-rule explanation trees ([api.v1.json](../../../engine/schemas/api.v1.json)).

```rust
let response = engine.evaluate(
    None,
    "shipping",
    Some(&now),
    IndexMap::new(),
    Some(&["rate".to_string()]),
    false,
)?;

for key in &response.results["rate"].missing_data {
    println!("need: {key}");
}
```

## Embedded units stdlib

`Engine::new()` loads `repo lemma` / `spec units` at compile time (import with `uses lemma units`). It always appears in `Engine::specs`. Formatted source: `engine.source(Some("lemma"), None, None)?`.

## Binary snapshot

After `load` / `update`, the engine holds parsed specs and compiled execution plans. `snapshot` writes that state to bytes so a later process can `from_snapshot` and call `evaluate` / `show` / `specs` without re-parsing or re-planning. The restored engine also accepts further `update` / `remove`.

Bytes carry a version header (`CARGO_PKG_VERSION`) and a CRC32. A snapshot from another engine version, or corrupt bytes, returns `Error`. Same sources loaded into two engines produce identical snapshot bytes.

### Write to disk and restore

```rust
use lemma::{DateTimeValue, Engine, IndexMap, SourceType};
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

fn load_and_evaluate(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = fs::read(path)?;
    let engine = Engine::from_snapshot(&bytes)?;

    let mut values = IndexMap::new();
    values.insert("weight".to_string(), "12".into());
    let now = DateTimeValue::now();
    let response = engine.evaluate(None, "shipping", Some(&now), values, None, false)?;

    println!("{}", response.results["rate"].result().unwrap_or(""));
    Ok(())
}
```

In-process only (no filesystem):

```rust
let bytes = engine.snapshot()?;
let restored = Engine::from_snapshot(&bytes)?;
```

JSON documents for `show` / `evaluate` / bindings use `lemma::api` (`api::show_document`, `api::Response`, ...). Domain types keep exact serde for the snapshot path.

Same opaque bytes on the language SDKs: [JavaScript](javascript.md), [Java](java.md), [Elixir](elixir.md).

## API docs

Full Rust API documentation is published on [docs.rs](https://docs.rs/lemma-engine).

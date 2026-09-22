# Lemma Engine

> **A pure, declarative language for business rules.**

Lemma Engine is the Rust crate behind the Lemma language. It lets you parse, validate, and evaluate Lemma docs from your own applications while keeping the same natural, auditable semantics that the CLI exposes.

## Status

Lemma is pre-1.0. The language and APIs are stable for most use cases, but breaking changes may occur between minor versions. Pin your dependency version and review the [changelog](https://github.com/lemma/lemma/blob/main/CHANGELOG.md) before upgrading.

## Why Lemma?

- **Readable by business stakeholders**: rules look like the policies people already write
- **Deterministic and auditable**: opt in to a full explanation tree with `explain: true`
- **Type-aware**: dates, percentages, units, and automatic conversions are first-class
- **Composable**: specs extend and reference each other without boilerplate
- **Multi-platform**: use the engine from Rust, power the CLI/HTTP server, embed via npm (JavaScript/TypeScript), Hex (Elixir), or Maven (Java/Kotlin)

## Quick start

Add the crate:

```toml
[dependencies]
lemma-engine = "0.9.11"
```

### Minimal example

```rust
use lemma::{DateTimeValue, Engine, SourceType};
use std::collections::HashMap;
use std::sync::Arc;

let mut engine = Engine::new();

engine.load([(
    SourceType::Path(Arc::new(std::path::PathBuf::from("example.lemma"))),
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

### Providing values at runtime

```rust
use lemma::{DateTimeValue, Engine, SourceType};
use std::collections::HashMap;
use std::sync::Arc;

let mut engine = Engine::new();

engine.load([(
    SourceType::Path(Arc::new(std::path::PathBuf::from("shipping.lemma"))),
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

let now = DateTimeValue::now();
let response = engine.run(
    None,
    "shipping",
    Some(&now),
    HashMap::from([
        ("weight".to_string(), "12 kilogram".to_string()),
        ("destination".to_string(), "international".to_string()),
    ]),
    None,
    false,
)?;
```

### Discovering required data

`Engine::show` is static: every declared promptable data slot (with `path` for import identity), plus this spec's rule graph (`ShowRule` with result type, import `path`, default/`unless` branches, and `depends_on_rules` as `input_key` strings — local rules plus reachable imports) and temporal window. Empty `needed_by_rules` means offered for reuse (`data x: alias.slot`). Types, filled literals, and `-> suggest` hints live on `Show.data`. Default `run` evaluates this spec's outputs (empty-path rules); pass `rules` with any `Show.rules` key (e.g. `src.computed`) to target a reachable import. For run-data-aware requirements on a partial run, call `run` and inspect each rule's `missing_data`:

```rust
let response = engine.run(
    None,
    "shipping",
    Some(&now),
    HashMap::new(),
    Some(&["rate".to_string()]),
    false,
)?;

for (name, result) in &response.results {
    for key in &result.missing_data {
        println!("rule {name} still needs: {key}");
    }
}
```

Bound inputs (caller run bindings, spec-filled literals, or data vetoes) are omitted from `missing_data`. Suggestions on `Show.data` do not bind until supplied in `run`'s data map.

### Loading repositories from LemmaBase

The engine owns registry policy (`Registries`, `LemmaBase` bound to `https://lemmabase.com`) but performs no HTTP. Drive `Install` / `Resolve` with an `HttpTransport`, or use a host SDK `install`, then `load`:

```rust
use lemma::{Engine, Install, Registries, SourceType};
use std::fs;
use std::sync::Arc;

let registries = Registries::default();
let result = Install::run(&registries, "@org/pkg", &transport)?;

let mut engine = Engine::new();
engine.load([(
    SourceType::Dependency(result.id),
    result.source,
)])?;

let workspace_text = fs::read_to_string("workspace.lemma")?;
engine.load([(
    SourceType::Path(Arc::new(std::path::PathBuf::from("workspace.lemma"))),
    workspace_text,
)])?;
```

See [registry docs](../cli/documentation/reference/registry.md).

## Embedded units stdlib

`Engine::new()` loads the embedded `repo lemma` / `spec units` standard library internally via `SourceType::Dependency("lemma")`. It always appears in `Engine::list`. Formatted source: `engine.source(Some("lemma"), None, None)?`.

Public `load` rejects `SourceType::Dependency("lemma")`: that id is reserved for the embedded stdlib.

## Public Engine API

| Method | Purpose |
|--------|---------|
| `new()` / `with_limits()` | Construct engine (stdlib pre-loaded) |
| `load(sources)` | Load `(SourceType, text)` pairs; provenance from `SourceType` alone |
| `list()` | All loaded repositories (`ResolvedRepository[]`: name, effective_from, effective_to per row) |
| `show(repository, spec, effective)` | Interface + temporal window (`Show`; no Lemma text) |
| `source(repository, spec?, effective)` | Formatted Lemma source (repo-wide when `spec` omitted) |
| `run(repository, spec, effective, data, rules, explain)` | Evaluate; each `RuleResult` may include `missing_data`; `explanation` when `explain` is true ([schema](schemas/api.v1.json)) |
| `update(repository, code, source_type)` | Replace identities present in `code` in one planning pass |
| `remove(repository, spec, effective)` | Remove a temporal spec slice |
| `snapshot()` / `from_snapshot(bytes)` | Persist / restore parsed specs + plans + limits (no re-plan); see [Rust tools](../cli/documentation/tools/rust.md#binary-snapshot) |
| `limits()` | Resource limits |

Free function: `lemma::resolve_effective`.

## Consumer API tiers

**Tier 1: always available** (all targets): consumer verbs (`Engine`, `load`, `list`, `show`, `source`, `run`, `remove`), API types (`Show`, `Response`, `DataPath`, `ListedSpec`, `ResolvedRepository`, `DateTimeValue`, `TimezoneValue`, `Explanation`, `Cause`, `ExplanationNode`, `OperationResult`; explanation JSON uses `"type"` + `"name"`; see [`api.v1.json`](schemas/api.v1.json)), language surface for tooling (`parse`, `ParseResult`, `Lexer`, `TokenKind`, `DataValue`, `SpecRef`, `Span`, `Source`, `format_source`, `format_specs`, `format_parse_result`, `type_detail_lines`), limit constants (`MAX_*_NAME_LENGTH`). Language-surface exports are intentional for CLI/LSP/tests, not a minimal “verbs only” crate.

**Tier 2: registries (all targets):** `Registry`, `LemmaBase`, `Registries`, `RepositoryQualifier`, `Header`, `Fetch`, `HttpResponse`, `TransportFailure`, `HttpTransport`, `Install`, `InstallStep`, `Resolve`, `ResolveStep`, `RegistryBundle`, `RegistryError`, `RepositoryInstallResult`, `Context`, `LemmaRepository`, `LemmaSpec`, `LemmaSpecSet`.

**Tier 3: native only (`not(wasm32)`):** `lemma::deps` for filesystem dependency cache paths.

`EffectiveDate` is planning-internal and not exported. Language-surface `Span` and `Source` are exported (parse/AST). LSP and CLI use `DateTimeValue` and `SpecRef::resolved_instant` at temporal boundaries.

## Features

- **Rich type system**: percentages, mass, length, duration, temperature, pressure, power, energy, frequency, and data sizes
- **Automatic unit conversions**: convert between units inside expressions without extra code
- **Page composition**: extend specs, bind data, and reuse rules across modules
- **Audit trail**: with `explain: true`, each rule result carries a structured explanation (see [`engine/schemas/api.v1.json`](schemas/api.v1.json))
- **JavaScript / TypeScript**: `npm install @lemmabase/lemma-engine` for browser, Node, and edge runtimes
- **Elixir**: Hex package `lemma_engine` (precompiled NIFs)
- **Java / Kotlin**: `com.lemmabase:lemma-engine` on Maven Central (`BigDecimal`-first JNI binding)

Constraint-style **inversion** (what inputs would yield a given outcome?) is planned; it is not documented as a supported API yet.

## Installation options

### As a library

```bash
cargo add lemma-engine
```

### CLI tool

```bash
cargo install lemma
lemma run pricing quantity=10
```

### HTTP server

```bash
cargo install lemma
lemma server --port 8012
```

### JavaScript / TypeScript

```bash
npm install @lemmabase/lemma-engine
```

```javascript
import { Lemma } from '@lemmabase/lemma-engine';
const engine = await Lemma();
```

Build: `node build.js` (from `engine/packages/npm/`). See [packages/npm/README.md](packages/npm/README.md).

### Java / Kotlin

```xml
<dependency>
  <groupId>com.lemmabase</groupId>
  <artifactId>lemma-engine</artifactId>
  <version>0.9.11</version>
</dependency>
```

Build/test: `cargo build -p lemma_jni` then `./mvnw verify` under `engine/packages/maven/` (also via `cargo precommit`). See [packages/maven/README.md](packages/maven/README.md).

### Elixir

```elixir
# mix.exs
{:lemma_engine, "~> 0.9"}
```

```elixir
{:ok, engine} = Lemma.new()
:ok = Lemma.load(engine, source)
{:ok, response} = Lemma.run(engine, %{spec: "pricing"}, %{data: %{"quantity" => "25"}})
```

See [packages/hex/README.md](packages/hex/README.md).

## Documentation

- Learn guide: <https://github.com/lemma/lemma/blob/main/cli/documentation/learn/readme.md>
- API documentation: <https://docs.rs/lemma-engine>
- Examples: <https://github.com/lemma/lemma/tree/main/engine/documentation/examples>
- CLI usage: <https://github.com/lemma/lemma/blob/main/cli/documentation/reference/cli.md>

## Use cases

- Compensation plans and employment contracts
- Pricing, shipping, and discount policies
- Tax and finance calculations
- Insurance eligibility and premium rules
- Compliance and validation logic
- SLA and service-level calculations

## Contributing

Contributions are very welcome!

## License

Apache 2.0

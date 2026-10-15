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
- **Multi-platform**: use the engine from Rust, power the CLI/HTTP server, embed via npm (JavaScript/TypeScript), Hex (Elixir), Maven (Java/Kotlin), or NuGet (C# / .NET)

## Quick start

Add the crate:

```toml
[dependencies]
lemma-engine = "0.10.1"
```

### Minimal example

```rust
use lemma::{DateTimeValue, Engine, IndexMap, SourceType};
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

### Providing values at runtime

```rust
use lemma::{DateTimeValue, Engine, IndexMap, SourceType};
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
let response = engine.evaluate(
    None,
    "shipping",
    Some(&now),
    IndexMap::from([
        ("weight".to_string(), "12 kilogram".into()),
        ("destination".to_string(), "international".into()),
    ]),
    None,
    false,
)?;
```

### Discovering required data

`Engine::show` describes a spec without evaluating it. `Show.types` lists the data declarations of the spec itself. `Show.data` lists the inputs that rules read, keyed by input name (`quantity`, or `alias.field` for an import), each with its type, the `value` the spec sets, a `suggestion` from `-> suggest`, the `needed_by_rules` that read it, and `path` (the `uses` hops to the spec that declares it). `Show.rules` is the rule graph: each `ShowRule` has its result type, `path`, the default and `unless` branches, and `depends_on_rules`. The temporal window of the shown version is included.

`lemma::api::show_document(&engine, &show)` turns this into the JSON document that the CLI, HTTP server, MCP and SDKs return: a Draft 2020-12 JSON Schema for the evaluation data (`properties`, `required`, `$defs` for list rows) with the Lemma specifics under `x-lemma-*` keys. A evaluation-data payload can be validated against it as is.

By default `evaluate` evaluates the rules written in the spec itself. Pass `rules` with any `Show.rules` key, such as `src.computed`, to evaluate a rule reached through `uses`. To learn what a particular evaluation still needs, call `evaluate` and read each rule's `missing_data`:

```rust
let response = engine.evaluate(
    None,
    "shipping",
    Some(&now),
    IndexMap::new(),
    Some(&["rate".to_string()]),
    false,
)?;

for (name, result) in &response.results {
    for key in &result.missing_data {
        println!("rule {name} still needs: {key}");
    }
}
```

Inputs that already have a value (passed in the evaluation data or set by the spec) do not appear in `missing_data`. A `-> suggest` hint is only a hint: it binds nothing until the caller passes it in the evaluation data.

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

`Engine::new()` loads the embedded `repo lemma` / `spec units` standard library internally via `SourceType::Dependency("lemma")`. It always appears in `Engine::specs`. Formatted source: `engine.source(Some("lemma"), None, None)?`.

Public `load` rejects `SourceType::Dependency("lemma")`: that id is reserved for the embedded stdlib.

## Public Engine API

| Method | Purpose |
|--------|---------|
| `new()` / `with_limits()` | Construct engine (stdlib pre-loaded) |
| `load(sources)` | Load `(SourceType, text)` pairs; provenance from `SourceType` alone |
| `specs()` | All loaded repositories (`Repository[]`, each with `SpecVersion` rows: name, effective_from, effective_to) |
| `show(repository, spec, effective)` | Interface + temporal window (`Show`; no Lemma text) |
| `source(repository, spec?, effective)` | Formatted Lemma source (repo-wide when `spec` omitted) |
| `evaluate(repository, spec, effective, data, rules, explain)` | Evaluate; each `RuleResult` may include `missing_data`; `explanation` when `explain` is true ([schema](schemas/api.v1.json)) |
| `update(repository, code, source_type)` | Replace identities present in `code` in one planning pass |
| `remove(repository, spec, effective)` | Remove a temporal spec slice |
| `snapshot()` / `from_snapshot(bytes)` | Persist / restore parsed specs + plans + limits (no re-plan); see [Rust tools](../cli/documentation/tools/rust.md#binary-snapshot) |
| `limits()` | Resource limits |

Free function: `lemma::resolve_effective`.

## Consumer API tiers

**Tier 1: always available** (all targets): consumer verbs (`Engine`, `load`, `specs`, `show`, `source`, `evaluate`, `remove`), API types (`Show`, `Response`, `DataPath`, `SpecVersion`, `Repository`, `DateTimeValue`, `TimezoneValue`, `Explanation`, `Cause`, `ExplanationNode`, `OperationResult`; explanation JSON uses `"type"` + `"name"`; see [`api.v1.json`](schemas/api.v1.json)), language surface for tooling (`parse`, `ParseResult`, `Lexer`, `TokenKind`, `DataValue`, `SpecRef`, `Span`, `Source`, `format_source`, `format_specs`, `format_parse_result`, `type_detail_lines`), limit constants (`MAX_*_NAME_LENGTH`). Language-surface exports are intentional for CLI/LSP/tests, not a minimal “verbs only” crate.

**Tier 2: registries (all targets):** `Registry`, `LemmaBase`, `Registries`, `RepositoryQualifier`, `Header`, `Fetch`, `HttpResponse`, `TransportFailure`, `HttpTransport`, `Install`, `InstallStep`, `Resolve`, `ResolveStep`, `RegistryBundle`, `RegistryError`, `RepositoryInstallResult`, `Context`, `LemmaRepository`, `LemmaSpec`, `LemmaSpecSet`.

**Tier 3: native only (`not(wasm32)`):** `lemma::deps` for filesystem dependency cache paths.

`EffectiveDate` is planning-internal and not exported. Language-surface `Span` and `Source` are exported (parse/AST). LSP and CLI use `DateTimeValue` and `SpecRef::resolved_instant` at temporal boundaries.

## Features

- **Rich type system**: percentages, mass, length, duration, temperature, pressure, power, energy, frequency, data sizes, ranges and lists
- **Automatic unit conversions**: convert between units inside expressions without extra code
- **Spec composition**: extend specs, bind data, and reuse rules across modules
- **Lists**: rows of values or spec instances, read with `sum`, `where`, `sort`, `find` and friends
- **Audit trail**: with `explain: true`, each rule result carries a structured explanation (see [`engine/schemas/api.v1.json`](schemas/api.v1.json))
- **JavaScript / TypeScript**: `npm install @lemmabase/lemma-engine` for browser, Node, and edge runtimes
- **Elixir**: Hex package `lemma_engine` (precompiled NIFs)
- **Java / Kotlin**: `com.lemmabase:lemma-engine` on Maven Central (`BigDecimal`-first JNI binding)
- **C# / .NET**: `Lemmabase.Lemma.Engine` on NuGet (UniFFI + System.Text.Json)

Constraint-style **inversion** (what inputs would yield a given outcome?) is planned; it is not documented as a supported API yet.

## Installation options

### As a library

```bash
cargo add lemma-engine
```

### CLI tool

```bash
cargo install lemma
lemma evaluate pricing quantity=10
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
  <version>0.10.1</version>
</dependency>
```

Build/test: `cargo build -p lemma_jni` then `./mvnw verify` under `engine/packages/maven/` (also via `cargo precommit`). See [packages/maven/README.md](packages/maven/README.md).

### C# / .NET

```bash
dotnet add package Lemmabase.Lemma.Engine --version 0.10.1
```

Build/test: `cargo run -p xtask -- nuget-natives` then `dotnet test` under `engine/packages/nuget/` (also via `cargo precommit`). See [packages/nuget/Lemmabase.Lemma.Engine/README.md](packages/nuget/Lemmabase.Lemma.Engine/README.md).

### Elixir

```elixir
# mix.exs
{:lemma_engine, "~> 0.10"}
```

```elixir
{:ok, engine} = Lemma.new()
:ok = Lemma.load(engine, source)
{:ok, response} = Lemma.evaluate(engine, %{spec: "pricing"}, %{data: %{"quantity" => "25"}})
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

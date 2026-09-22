# Lemma

[![CI](https://github.com/lemma/lemma/workflows/CI/badge.svg)](https://github.com/lemma/lemma/actions/workflows/quality.yml)
[![Crates.io CLI](https://img.shields.io/crates/v/lemma.svg?label=lemma)](https://crates.io/crates/lemma)
[![Crates.io Engine](https://img.shields.io/crates/v/lemma-engine.svg?label=lemma-engine)](https://crates.io/crates/lemma-engine)
[![Documentation](https://docs.rs/lemma-engine/badge.svg)](https://docs.rs/lemma-engine)
[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)

## **A pure, declarative language for business rules.**

Lemma is a language for business rules, such as pricing, product terms, tax legislation, eligibility, etc. Applications and AI agents embed the **Lemma engine** to ask it for answers. Lemma provides those quickly, precisely and proves them with explanations, that show which rules applied and thus why a certain outcome was given.

A **spec** is a set of **data** (inputs) and **rules** (expressions). Specs can be composed neatly to facilitate even the most complex rulesets, modeled around business domains.

When **loading** Specs, the engine analyses them and verifies that they are valid. If loading fails, the engine is unaffected. When it succeeds, the specs and rules can be **evaluated** to compute each requested rule. Evaluation does not error and always returns a value, but that value might be a **veto** result, which explains why a certain rule had no valid outcome.

Evaluate Lemma with `--explain` or `explain: true`, to make the engine return explanations.

```lemma
spec pricing 2026-01-01

data quantity: number
data is_vip:   false

rule discount: 0%
  unless quantity >= 10  then 10%
  unless quantity >= 50  then 20%
  unless is_vip          then 25%

rule price: quantity * 20
rule discounted_price: price - price * discount
```

The last matching `unless` wins, mirroring how business rules, legal documents, and SOPs are written: "In principle X applies, unless Y, then Z..."


## Why Lemma?
Laws, policies, and business rules traditionally exist in natural language. While humans must understand these rules, we rely on systems to enforce them. Over time, organizations have built massive IT infrastructures to house these rules; however, as both the regulations and the systems evolve, they become harder to manage and the disconnect between them grows.

Lemma provides a single source of truth. Rules written in Lemma are human-readable, time-aware, and pure. Its logic engine guarantees deterministic and logically consistent outcomes through static analysis. Invalid specs are rejected before evaluation ever runs. Furthermore, Lemma provides unrivaled auditability: when explanations are requested, each result includes a structured tree of how rules were applied.

This allows you to implement policy changes rapidly without compromising compliance. Lemma requires no database and maintains no state; by design, it is secure, able to run within existing applications and yes, it is blazingly fast.

### Direction

Lemma aims to combine **deterministic evaluation**, **transparent explanations**, **temporal versioning** (rules that evolve on a timeline, separate from how you deploy code), **registry-style sharing** of specs, and **interop** (CLI, HTTP, MCP, and stable language bindings). Planned work includes **inversion** (constraint-style “what inputs satisfy this outcome?”), **tables** as a first-class data type for data-driven rules and **performance** competitive with high performance programming languages.

### What about AI?

AI models operate on approximations. The complexity of their neural networks makes tracing decisions ("explaining") practically impossible. While they excel at natural language, they are ill-suited for mathematics, strict protocols, or compliance.

Lemma provides certainty and transparency. Every result is exact, verifiable, and delivered in microsecond. Lemma offers seamless interoperability, allowing you to ground your AI systems in deterministic logic.

## Quick Start

### Installation

**CLI** (from [crates.io/crates/lemma](https://crates.io/crates/lemma)):

```bash
cargo install lemma
```

Or via npm:

```bash
npm install -g lemma
```

**Rust library** (from [crates.io/crates/lemma-engine](https://crates.io/crates/lemma-engine)):

```bash
cargo add lemma-engine --rename lemma
```

### Your first spec

Create `shipping.lemma`:

```lemma
spec shipping

data money: measure
  -> unit eur: 1.00
  -> unit usd: 0.91
  -> decimals 2
  -> minimum 0 eur

data weight: measure
  -> unit kilogram: 1
  -> unit gram: 0.001

data is_express:     true
data package_weight: 2.5 kilogram

rule express_fee: 0 eur
  unless is_express then 4.99 eur

rule base_shipping: 5.99 eur
  unless package_weight > 1 kilogram  then 8.99 eur
  unless package_weight > 5 kilogram  then 15.99 eur

rule total_cost: base_shipping + express_fee
```

Run it:

```
$ lemma run shipping
┌───────────────┬───────────┐
│ base_shipping ┆ 8.99 eur  │
├───────────────┼───────────┤
│ express_fee   ┆ 4.99 eur  │
├───────────────┼───────────┤
│ total_cost    ┆ 13.98 eur │
└───────────────┴───────────┘
```

Override data from the command line:

```
$ lemma run shipping is_express=false package_weight="6.0 kilogram"
┌───────────────┬───────────┐
│ base_shipping ┆ 15.99 eur │
├───────────────┼───────────┤
│ express_fee   ┆ 0.00 eur  │
├───────────────┼───────────┤
│ total_cost    ┆ 15.99 eur │
└───────────────┴───────────┘
```

## Key features

### Rich type system

Define custom types with units, constraints, and automatic conversion:

```lemma
spec type_examples

data money: measure
  -> unit eur: 1.00
  -> unit usd: 0.91
  -> decimals 2
  -> minimum 0 eur

data status: text
  -> option "active"
  -> option "inactive"

data discount: ratio
  -> minimum 0%
  -> maximum 100%
```

**Primitive types:** `boolean`, `number`, `measure` (with units; elapsed time and calendar periods via `uses lemma units`: `units.duration`, `units.calendar`, …)), `text`, `date`, `time`, `ratio`, and **ranges** (`date range`, `time range`, `number range`, `measure range`, `ratio range`, plus named `<type> range`).

### Spec composition

Reference data and rules across specs:

```lemma
spec employee

data years_service: 8


spec leave_policy

data base_leave_days:  25
data bonus_leave_days: 5
data senior_threshold: 5


spec leave_entitlement

uses employee
uses leave_policy

rule is_senior:
  employee.years_service >= leave_policy.senior_threshold

rule annual_leave_days: leave_policy.base_leave_days
  unless is_senior
    then leave_policy.base_leave_days + leave_policy.bonus_leave_days
```

### Temporal versioning

Multiple versions of a spec can coexist. The engine resolves the correct one based on a point in time:

```lemma
spec pricing

data base_price: 20
data quantity:   number

rule total: base_price * quantity


spec pricing 2025-01-01

data base_price: 25
data quantity:   number

rule total: base_price * quantity
```

```bash
lemma run pricing --effective 2024-06-01   # uses base_price: 20
lemma run pricing --effective 2025-06-01   # uses base_price: 25
```

### Veto

When type constraints are not enough, `veto` blocks a rule entirely:

```lemma
spec performance_review

data start_date: date
data review_date: date
data performance_score: number
  -> minimum 0
  -> maximum 100

rule bonus_percentage: 0%
  unless performance_score >= 70 then 5%
  unless performance_score >= 90 then 10%
  unless review_date < start_date
    then veto "Review date must be after start date"
```

A vetoed rule produces no result. See [veto](cli/documentation/learn/types_and_units.md#veto).

### Registry dependencies

Reference shared specs from a registry with `@`:

```lemma
spec invoicing

uses @iso/countries alpha2

data price: measure 
  -> unit eur: 1

data country: alpha2.code

rule tariff: 0 eur
  unless country is "NL" then price * 5%

rule total: price + tariff
```

```bash
lemma install --all           # install all @... repositories from LemmaBase
lemma install @iso/countries -f   # force re-install if content changed
```

## CLI

```bash
lemma run pricing                         # evaluate all rules
lemma run pricing --rules=total,tax       # specific rules only
lemma run pricing quantity=10 is_vip=true # override data
lemma run --interactive                   # interactive mode

lemma run pricing --effective 2025-01-01  # temporal query
lemma run pricing --json                 # JSON output
lemma run pricing -x                      # show reasoning

lemma show pricing                      # spec interface
lemma list                                # list all specs
lemma format                               # format .lemma files
lemma install --all                         # install all @... repositories from LemmaBase
lemma lsp                                 # language server (stdio)
```

### HTTP Server

```bash
lemma server --prefix ./policies

# Evaluate via query parameters
curl "http://localhost:8012/pricing?quantity=10&is_member=true"

# Evaluate via JSON body
curl -X POST http://localhost:8012/pricing \
  -H "Content-Type: application/json" \
  -d '{"quantity": 10, "is_member": true}'

# Evaluate specific rules
curl "http://localhost:8012/pricing/discount,total?quantity=10"
```

Routes: `GET /` (list specs), `GET /openapi.json`, `GET /docs` (interactive API docs), `GET /health`

Live-reload with `--watch`:

```bash
lemma server --prefix ./policies --watch
```

### MCP Server

AI assistants interact with Lemma specs via the Model Context Protocol:

```bash
lemma mcp             # read-only (run, list, show, source, check, guide)
lemma mcp --write     # also enable add_spec and other mutators
```

Authoring loop: `guide` → `check` (diagnostics) → `evaluate`. Resources expose `lemma://guide` and curated examples.
### JavaScript / TypeScript

```bash
npm install @lemmabase/lemma-engine
```

```javascript
import { Lemma } from '@lemmabase/lemma-engine';
const engine = await Lemma();
```

See [engine/packages/npm/README.md](engine/packages/npm/README.md).

### Elixir

```elixir
# mix.exs
{:lemma_engine, "~> 0.9"}
```

See [engine/packages/hex/README.md](engine/packages/hex/README.md) and [cli/documentation/tools/elixir.md](cli/documentation/tools/elixir.md).

### Java / Kotlin

```xml
<dependency>
  <groupId>com.lemmabase</groupId>
  <artifactId>lemma-engine</artifactId>
  <version>0.9.11</version>
</dependency>
```

See [engine/packages/maven/README.md](engine/packages/maven/README.md) and [cli/documentation/tools/java.md](cli/documentation/tools/java.md).

### Docker

```bash
docker pull ghcr.io/lemma/lemma:latest

# Run a spec
docker run --rm -v "$(pwd):/specs" ghcr.io/lemma/lemma \
  run --prefix /specs shipping

# Deploy as HTTP API
docker run -d -p 8012:8012 -v "$(pwd):/specs" ghcr.io/lemma/lemma \
  server --prefix /specs --host 0.0.0.0 --port 8012
```

Supports `linux/amd64` and `linux/arm64`.

## Documentation

- **[Learn guide](cli/documentation/learn/readme.md)** -- guided path from first spec to composing specs
- **[LLM guide (llms.txt)](cli/documentation/llms.txt)** -- authoring Lemma from business logic
- **[Composing specs](cli/documentation/learn/composing_specs.md)** -- `uses`, temporal versions, pins
- **[Reference](cli/documentation/reference/readme.md)** -- operators, literals, syntax
- **[Veto](cli/documentation/learn/types_and_units.md#veto)** -- when rules produce no value
- **[CLI Reference](cli/documentation/reference/cli.md)** -- all commands and flags
- **[Registry](cli/documentation/reference/registry.md)** -- shared specs and `@` references
- **[Examples](engine/documentation/examples/)** -- example `.lemma` files

## Status

Lemma is pre-1.0. The language and APIs are stable for most use cases, but breaking changes may occur between minor versions. Pin your dependency version and review the [changelog](CHANGELOG.md) before upgrading.

## Contributing

Contributions welcome! See [Contributing](cli/documentation/community/contributing.md) for setup and workflow.

CI runs **`cargo precommit --fuzz`**. That is the PR bar: same gate as local **`cargo precommit`**, then 30 minutes of fuzz total across [`engine/fuzz`](engine/fuzz) targets. Use bare **`cargo precommit`** as a faster local shortcut (no fuzz). The gate covers **`versions-verify`**, Hex `mix precommit`, VS Code `npm precommit`, `fmt --check`, Clippy (`--all-features`), Nextest (`--all-features`), WASM npm `build.js` + `test.js`, Maven `./mvnw -B verify` (after `lemma_jni` build), cargo-deny, and **`cargo coverage all --check`**. Install [`cargo-nextest`](https://nexte.st/), [`cargo-deny`](https://github.com/EmbarkStudios/cargo-deny), Elixir/Mix, [Node.js](https://nodejs.org/), [`wasm-pack`](https://rustwasm.github.io/wasm-pack/), and a **JDK 21+** first; for `--fuzz` also install nightly (`rustup install nightly`) and [`cargo-fuzz`](https://github.com/rust-fuzz/cargo-fuzz). Regenerate coverage with **`cargo coverage all`** when engine/cli sources change ([`cargo-llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov) required). `cargo nextest` alone is Rust tests only. When bumping the workspace release version, use **`cargo bump <version>`** and **`cargo verify`** (see [`xtask/README.md`](xtask/README.md)).

## License

Apache 2.0 -- see LICENSE for details.

---

**[GitHub](https://github.com/lemma/lemma)** -- **[Issues](https://github.com/lemma/lemma/issues)** -- **[Documentation](cli/documentation/readme.md)**

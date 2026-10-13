---
nav_title: Home
nav_order: 10
---

# A declarative language for business rules

Lemma is a pure, declarative and open source programming language for business rules, that is easy to read for non-technical stakeholders. Rules are written in Specs that humans can read and systems can evaluate precisely and deterministically. Lemma validates Rules before they can be evaluated. Invalid Specs are rejected at planning time; after that, each Rule returns a value or Veto, and optionally an explanation when you ask for one (`--explain` / `explain: true`).

```lemma
spec pricing 2026-01-01
"""
Applies a tiered discount and VAT to the base price, 
effective from 2026-01-01.
"""

data base_price: 100
data is_member:  false
data quantity:   number

rule discount:
  0%
  unless quantity >= 10 then 10%
  unless quantity >= 50 then 15%
  unless is_member      then 20%

rule discount_amount:
  base_price * discount

rule discounted_price:
  base_price - discount_amount

rule vat:
  discounted_price * 21%

rule total:
  discounted_price + vat
```

## Get started

If you have Cargo installed, you can run:

```bash
cargo install lemma
```

If you are using npm:

```bash
npm install -g lemma
```

Precompiled binaries for Linux, macOS, and Windows are included in the npm package automatically. Docker images are also available. See [Installation](installation.md) for all options.

## Writing your first Spec

Write a `.lemma` file and evaluate it. The [Learn guide](learn/readme.md) walks you through everything from your first Spec to composing Rules across time and registries. You'll be productive in minutes.

## Documentation

- **[Learn](learn/readme.md)**: guided path from first spec to composing specs
- **[Reference](reference/readme.md)**: operators, types, syntax, CLI, registry, test coverage, benchmarks
- **[Tools & SDKs](tools/readme.md)**: Rust, Elixir, JavaScript/TypeScript, and more
- **[Community](community/readme.md)**: contributing and publishing specs
- **[LLMs.txt](llms.md)**: authoring Lemma from business logic

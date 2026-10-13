---
nav_title: Getting started
nav_order: 10
---

# Getting started

Lemma is a pure, declarative language for business rules that stakeholders can read and systems can evaluate precisely. Rules live in Specs: named collections of Data (inputs) and Rules (computed outputs). Lemma validates every Spec before evaluation; after that, each Rule returns a value or a Veto (no value). Explanations are opt-in (`lemma evaluate -x` / `explain: true`).

This chapter walks through installation, your first Spec, and running it from the CLI.

## Install

Install the CLI from [crates.io/crates/lemma](https://crates.io/crates/lemma):

```bash
cargo install lemma
```

Or via npm:

```bash
npm install -g lemma
```

For library bindings (JavaScript/TypeScript, Elixir, Java/Kotlin), editor support, and Docker, see [Installation](../installation.md). To embed Lemma in your application, see [Tools & SDKs](../tools/readme.md).

## Your first Spec

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

rule express_fee:
  0 eur
  unless is_express then 4.99 eur

rule base_shipping:
  5.99 eur
  unless package_weight > 1 kilogram then 8.99 eur
  unless package_weight > 5 kilogram then 15.99 eur

rule total_cost:
  base_shipping + express_fee
```

Three building blocks appear here:

- `spec` names the Rule set.
- `data` defines inputs with types and constraints (`-> unit`, `-> decimals`, `-> minimum`).
- `rule` computes values; `unless` clauses add conditional logic (the last matching condition wins).

There is no inline comment syntax. The only in-source documentation is a commentary block: triple-quoted `"""..."""` placed immediately after the Spec line. The syntax relies on keywords instead of colons or brackets. Indentation is for readability only; newlines are optional. Apply the standard format with:

```bash
lemma format
```

## Evaluate it

```bash
lemma evaluate shipping
```

Output:

```
┌───────────────┬───────────┐
│ base_shipping ┆ 8.99 eur  │
├───────────────┼───────────┤
│ express_fee   ┆ 4.99 eur  │
├───────────────┼───────────┤
│ total_cost    ┆ 13.98 eur │
└───────────────┴───────────┘
```

Override Data from the command line:

```bash
lemma evaluate shipping is_express=false package_weight="6.0 kilogram"
```

Output:

```
┌───────────────┬───────────┐
│ base_shipping ┆ 15.99 eur │
├───────────────┼───────────┤
│ express_fee   ┆ 0.00 eur  │
├───────────────┼───────────┤
│ total_cost    ┆ 15.99 eur │
└───────────────┴───────────┘
```

Other useful flags:

```bash
lemma evaluate shipping --json
```

Machine-readable output.

```bash
lemma evaluate shipping -x
```

Show how each Rule was evaluated (human reasoning table; with `--json`, per-rule `explanation` objects per [api.v1.json](../../../engine/schemas/api.v1.json)).

```bash
lemma show shipping
```

Show the static catalog: Types (local data declarations), Data (inputs rules need), and rules. With `--json` it is a JSON Schema for the evaluation data (`properties` = Data; `x-lemma-types` = Types). Evaluation-data-aware needs for a concrete `evaluate` come from each rule's `missing_data`; types and suggestions are on `lemma show` only.

See [CLI reference](../reference/cli.md) for all commands and flags.

## Next up

[Specs, Data, and Rules](specs_data_rules.md): how Specs are structured, open inputs, constraints, and Rule references.

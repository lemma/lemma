---
nav_title: Elixir
nav_order: 20
---

# Elixir

`lemma_engine` provides precompiled NIFs for Elixir (>= 1.14). Erlang and Gleam use the same package.

Precompiled binaries are downloaded automatically for macOS (arm64/x86_64), Linux (gnu x86_64 and arm64), and Windows (arm64/x86_64).

## Install

Add to `mix.exs`:

```elixir
def deps do
  [{:lemma_engine, "~> 0.10"}]
end
```

Or from git:

```elixir
{:lemma_engine, git: "https://github.com/lemma/lemma", sparse: "engine/packages/hex"}
```

## Usage

```elixir
{:ok, engine} = Lemma.new()

:ok = Lemma.load(engine, """
spec pricing
data quantity: number
data price: 10
rule total: quantity * price
rule discount: 0
  unless quantity >= 10 then 5
  unless quantity >= 50 then 15
""")

{:ok, response} = Lemma.evaluate(engine, %{spec: "pricing"}, %{data: %{"quantity" => 25}})
```

Integers (any size) and binaries are exact. Elixir floats and atoms other than `true` / `false` / `nil` return `{:error, %{kind: "request"}}` (`pass as string`). `nil` omits a map value; a `nil` list item is rejected. Measure data also accepts `%{"eur" => "84"}`. Keys differing only in case (including `"qty"` and `:qty`) are rejected.

A list is passed as an Elixir list that replaces every source row: binaries for a value list (`%{"fares" => ["89 eur", "120 eur"]}`), maps keyed by row data names for an instance list (`%{"staff" => [%{"name" => "Ada", "salary" => "9000 eur"}]}`). In results, `Lemma.RuleResult.list` holds a list value's items. `Lemma.RuleResult.row` is a row reference whose `"index"` points into `Lemma.Response.rows`, where each referenced row appears once with its `"data"` and `"rules"`. In `Lemma.show/4`, an instance list with source rows has a `"value"` whose `"list"` items, in row order, carry only `"fields"`, the fields the row sets. Evaluation data for a slot bound to data or a rule on a `uses` (`uses i: inner -> with x: base`) overrides the wire.

Introspect loaded Specs:

```elixir
{:ok, groups} = Lemma.specs(engine)
{:ok, show} = Lemma.show(engine, nil, "pricing")
{:ok, workspace_source} = Lemma.source(engine, nil, nil, nil)
{:ok, stdlib} = Lemma.source(engine, "lemma", nil, nil)
```

Format source code (no engine needed):

```elixir
{:ok, formatted} = Lemma.format("spec foo\ndata x: 1\nrule y: x + 1")
```

Install a repository from LemmaBase (download only; then `load`). Default transport is Req; pass a 2-arity fun for tests or custom HTTP:

```elixir
{:ok, result} = Lemma.install(engine, "@iso/countries")
:ok = Lemma.load(engine, [{result[:id], result[:source]}])
```

## API

| Function | Description |
|----------|-------------|
| `Lemma.version/0` | Semver of the loaded native engine (e.g. `"0.9.12"`) |
| `Lemma.new/1` | Create engine (optional limits map) |
| `Lemma.limits/1` | Current resource limits |
| `Lemma.snapshot/1` | Opaque bytes of parsed specs + plans + limits |
| `Lemma.from_snapshot/1` | Restore engine from `snapshot/1` bytes |
| `Lemma.load/2` | Load sources: binary (volatile); map (lexicographic label order) or `[{label, code}, ...]` (caller order) |
| `Lemma.install/2-3` | Download a repository from LemmaBase (`{:ok, map}` with `:source` / `:id`); optional `(url, headers) -> …` transport (default `Lemma.Transport.get/2`); does not load and does not write `lemma_deps/` |
| `Lemma.specs/1` | Loaded Specs grouped by repository (includes embedded `lemma` / `spec units`) |
| `Lemma.source/4` | Formatted Lemma source (`repository`, `spec`, `effective`; omit `spec` for repo-wide) |
| `Lemma.show/4` | Evaluation-data JSON Schema (Draft 2020-12) + this spec's rule graph + temporal window (`repository`, `spec`, `effective`); Lemma metadata under `x-lemma-*`. `Lemma.Show.from_map/1` reads it: `types` is `x-lemma-types` (local declarations), `data` values are `Lemma.ShowData` (inputs rules need; full `type`, raw `schema`), `rows` are `Lemma.ShowRow`, `schema` is the document. `Show.data` / `Show.rules` carry `path` (`[]` = this spec). |
| `Lemma.evaluate/3` | Evaluate: `target` map (`repo`, `spec`, `effective`), `options` map (`data`, `rules`, `explain`). Each rule result may include `missing_data` (unbound input keys). Non-veto results carry flattened `result` + typed keys (`Lemma.RuleResult`). With `explain: true`, `explanation` matches [api.v1.json](../../../engine/schemas/api.v1.json) (`RuleResult.explanation` / `ExplanationNode`). Types and suggestions are on `Lemma.show/4` only. |
| `Lemma.remove/4` | Remove temporal slice: `repository`, `spec`, `effective` |
| `Lemma.update/3-4` | Upsert identities from code; optional source `attribute` |
| `Lemma.quality/1` | Structural quality recommendations across loaded specs (advisory only) |
| `Lemma.format/1` | Format Lemma source code (no engine needed) |

Persist and restore without re-parsing:

```elixir
{:ok, bytes} = Lemma.snapshot(engine)
File.write!("engine.lems", bytes)
{:ok, restored} = Lemma.from_snapshot(File.read!("engine.lems"))
```

`Lemma.Mcp` wraps the engine MCP catalog (`evaluate`, `specs`, `show`, `source`, `check`, `guide`, resources). `evaluate` returns formatted ASCII explanation trees (always on). `specs`, `show` and `check` return JSON; pass `"text" => true` for the CLI view. Write tools (`add_spec`, …) are CLI-only (`lemma mcp --write`).

`Lemma.OpenAPI` is a separate module (HTTP OpenAPI document helpers via `lemma_openapi`). It is not part of the core `Lemma` engine table above.

## Engine lifecycle

Each `Lemma.new/1` call creates an independent engine. The engine reference is safe to use from a single process. For shared access across processes, wrap it in a GenServer.

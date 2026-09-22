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
  [{:lemma_engine, "~> 0.9"}]
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

{:ok, response} = Lemma.run(engine, %{spec: "pricing"}, %{data: %{"quantity" => "25"}})
```

Introspect loaded Specs:

```elixir
{:ok, groups} = Lemma.list(engine)
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
| `Lemma.new/1` | Create engine (optional limits map) |
| `Lemma.limits/1` | Current resource limits |
| `Lemma.snapshot/1` | Opaque bytes of parsed specs + plans + limits |
| `Lemma.from_snapshot/1` | Restore engine from `snapshot/1` bytes |
| `Lemma.load/2` | Load sources: binary (volatile); map (lexicographic label order) or `[{label, code}, ...]` (caller order) |
| `Lemma.install/2-3` | Download a repository from LemmaBase (`{:ok, map}` with `:source` / `:id`); optional `(url, headers) -> …` transport (default `Lemma.Transport.get/2`); does not load and does not write `lemma_deps/` |
| `Lemma.list/1` | List loaded Specs (includes embedded `lemma` / `spec units`) |
| `Lemma.source/4` | Formatted Lemma source (`repository`, `spec`, `effective`; omit `spec` for repo-wide) |
| `Lemma.show/4` | Declared data catalog + this spec's rule graph + temporal window (`repository`, `spec`, `effective`). Empty `needed_by_rules` = reuse-only. `Show.data` / `Show.rules` carry `path` (`[]` = this spec). `Show.data` values are `Lemma.ShowData`. |
| `Lemma.run/3` | Evaluate: `target` map (`repo`, `spec`, `effective`), `options` map (`data`, `rules`, `explain`). Each rule result may include `missing_data` (unbound input keys). Non-veto results carry flattened `result` + typed keys (`Lemma.RuleResult`). With `explain: true`, `explanation` matches [api.v1.json](../../../engine/schemas/api.v1.json) (`RuleResult.explanation` / `ExplanationNode`). Types and suggestions are on `Lemma.show/4` only. |
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

`Lemma.Mcp` wraps the engine MCP catalog (`run`, `list`, `show`, `source`, `check`, `guide`, resources). `run` returns formatted ASCII explanation trees (always on). Write tools (`add_spec`, …) are CLI-only (`lemma mcp --write`).

`Lemma.OpenAPI` is a separate module (HTTP OpenAPI document helpers via `lemma_openapi`). It is not part of the core `Lemma` engine table above.

## Engine lifecycle

Each `Lemma.new/1` call creates an independent engine. The engine reference is safe to use from a single process. For shared access across processes, wrap it in a GenServer.

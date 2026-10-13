# Lemma

Elixir client for the [Lemma](https://github.com/lemma/lemma) rules engine, via precompiled NIFs.

## Requirements

- Elixir >= 1.14

Precompiled NIF binaries are downloaded automatically for macOS (arm64/x86_64), Linux (gnu x86_64 and arm64), and Windows (arm64/x86_64).

## Development

To compile the NIF from source (contributors), set `LEMMA_BUILD_NIF=1` and have Rust stable on `PATH`:

```bash
LEMMA_BUILD_NIF=1 mix compile
```

Pre-commit check (format, locked deps, compile, ExUnit):

```bash
mix precommit
```

Builds the NIF from source when no checksum file is present (typical in this repo).

## Installation

Add to `mix.exs`:

```elixir
def deps do
  [
    {:lemma_engine, "~> 0.10"}
  ]
end
```

Or from git:

```elixir
{:lemma_engine, git: "https://github.com/lemma/lemma", sparse: "engine/packages/hex"}
```

## Usage

```elixir
# Create an engine
{:ok, engine} = Lemma.new()

# Load inline source
:ok = Lemma.load(engine, """
spec pricing
data quantity: number
data price: 10
rule total: quantity * price
rule discount: 0
  unless quantity >= 10 then 5
  unless quantity >= 50 then 15
""")

# Or load labeled paths / dependencies in one planning pass
:ok = Lemma.load(engine, %{"pricing.lemma" => pricing_source})

# Evaluate with data
{:ok, response} =
  Lemma.evaluate(
    engine,
    %{spec: "pricing"},
    %{data: %{"quantity" => "25"}, explain: false}
  )

# Loaded specs (grouped by repository)
{:ok, groups} = Lemma.specs(engine)
# [%{repository: nil, specs: [...]}, %{repository: "lemma", specs: [...]}, ...]
{:ok, show} = Lemma.show(engine, nil, "pricing")

# Canonical Lemma source
{:ok, source} = Lemma.source(engine, nil, "pricing")

# Format source code (no engine needed)
{:ok, formatted} = Lemma.format("spec foo\ndata x: 1\nrule y: x + 1")

# Clean up
:ok = Lemma.remove(engine, nil, "pricing", "2025-01-01")
```

## API

| Function | Description |
|----------|-------------|
| `Lemma.new/1` | Create engine (optional limits map) |
| `Lemma.limits/1` | Current resource limits |
| `Lemma.load/2` | Load sources: binary (volatile); map (lexicographic label order) or list of `{label, code}` (caller order) |
| `Lemma.install/2-3` | Download a repository from LemmaBase (`{:ok, map}` with `:source` / `:id`); optional transport fun; does not load |
| `Lemma.specs/1` | Loaded specs grouped by repository (includes embedded `lemma` / `spec units`) |
| `Lemma.show/4` | Spec interface and temporal window (`repository`, `spec`, `effective`) |
| `Lemma.source/4` | Formatted canonical Lemma source |
| `Lemma.evaluate/3` | Evaluate a spec (`target` + `options` maps; `explain: true` adds per-rule explanation trees) |
| `Lemma.remove/4` | Remove a spec from the engine |
| `Lemma.update/3-4` | Upsert identities from `code`; Path/Dependency prune siblings |
| `Lemma.quality/1` | Structural quality recommendations across loaded specs (advisory only) |
| `Lemma.format/1` | Format Lemma source code (no engine needed) |

See `Lemma` module docs for full typespecs and options.

## Data values

Data values passed to `Lemma.evaluate/3` are binaries, booleans, integers (any size, exact), single-unit maps (`%{"eur" => "84"}`), or lists. `nil` is omitted. Elixir floats (IEEE 754 doubles) and other atoms are rejected with a request error to prevent silent precision loss: pass decimal values as strings instead:

```elixir
Lemma.evaluate(engine, %{spec: "pricing"}, %{data: %{"rate" => "0.075"}, explain: false})   # correct
Lemma.evaluate(engine, %{spec: "pricing"}, %{data: %{"quantity" => 42}, explain: false})     # correct (integer)
Lemma.evaluate(engine, %{spec: "pricing"}, %{data: %{"rate" => 0.075}, explain: false})      # {:error, %{kind: "request", ...}}
```

With `explain: true`, each rule result may include an `explanation` object matching [`api.v1.json`](../../../engine/schemas/api.v1.json) (`RuleResult.explanation`).

## Engine lifecycle

Each `Lemma.new/1` call creates an independent engine. The engine reference is safe to use from a single process. For shared access across processes, wrap it in a GenServer.

## License

Apache-2.0

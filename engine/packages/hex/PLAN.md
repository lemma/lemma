# Hex package (Rustler): current surface

Elixir/Erlang bindings via Rustler NIFs. Public API lives in `lib/lemma.ex`; NIFs in `native/lemma_hex/src/lib.rs`.

## NIFs

| NIF | Elixir | Purpose |
|-----|--------|---------|
| `lemma_new` | `Lemma.new/1` | Create engine (optional limits map) |
| `lemma_limits` | `Lemma.limits/1` | Current resource limits |
| `lemma_snapshot` | `Lemma.snapshot/1` | Opaque engine snapshot bytes |
| `lemma_from_snapshot` | `Lemma.from_snapshot/1` | Restore engine from snapshot bytes |
| `lemma_quality` | `Lemma.quality/1` | Structural quality recommendations |
| `lemma_load` | `Lemma.load/2` | Binary (volatile) or map/list (labeled) sources |
| `lemma_install_start` / `lemma_install_respond` | `Lemma.install/2-3` | Download LemmaBase repository (`source` + `id`); host transport; no load |
| `lemma_specs` | `Lemma.specs/1` | Loaded specs grouped by repository |
| `lemma_show` | `Lemma.show/4` | Spec interface + temporal window |
| `lemma_source` | `Lemma.source/4` | Formatted canonical Lemma source |
| `lemma_run` | `Lemma.evaluate/3` | Evaluate (`target` + `options` maps) |
| `lemma_remove` | `Lemma.remove/4` | Remove a temporal spec slice |
| `lemma_update` | `Lemma.update/3-4` | Upsert identities from `code`; Path/Dependency prune siblings |
| `lemma_format` | `Lemma.format/1` | Format Lemma source (no engine) |

## Out of scope

- `load_from_paths`, `inspect`, `schema`, `invert`, `remove_spec`, `execution_plan`
- Host reads files and calls `Lemma.load/2`

## Layout

```
engine/packages/hex/
  mix.exs
  lib/lemma.ex, lib/lemma/native.ex
  native/lemma_hex/          # workspace member, path dep on lemma-engine
  test/lemma_test.exs
```

## Development

```bash
LEMMA_BUILD_NIF=1 mix compile
mix precommit
```

# @lemmabase/lemma-engine

> [Lemma](https://github.com/lemma/lemma) is a declarative language for business rules. **This package is the Lemma engine for JavaScript and TypeScript**: browser, Node, Bun, Deno, Cloudflare Workers, Vercel Edge, etc.

Pricing tiers, tax brackets, leave entitlement, eligibility checks, discount stacks: the rules that change, that auditors ask about, that legal writes in PDFs and engineers re-implement in operational code... Lemma is a language built specifically for your business rules. It is readable by stakeholders, executable anywhere, and impossible to drift out of sync.

```lemma
spec pricing 2026-01-01

data money: measure
  -> unit eur: 1.00
  -> decimals 2

data quantity : number
data is_vip   : false

rule unit_price:
  20 eur
  unless quantity >= 10 then 18 eur
  unless quantity >= 50 then 16 eur
  unless is_vip         then 15 eur

rule total:
  unit_price * quantity
```

```javascript
import { Lemma } from '@lemmabase/lemma-engine';

const engine = await Lemma();
await engine.load({ 'pricing.lemma': pricing });

const response = engine.run({ spec: 'pricing', data: { quantity: 50, is_vip: false } });
// response.results.unit_price → 16 eur
// response.results.total      → 800 eur
```

The `Response` carries every rule's value (or `veto` if no result could be computed). When inputs are still unbound, that rule includes `missing_data` (`string[]` input keys). Types, filled literals, and suggestions are on `engine.show(...)` (`Show.data`) only, not on the evaluate response.

## Why use it from JavaScript?

- **Deterministic.** `(spec, data, effective_date) → result`. No DB, no clock, no ambient state. Same inputs → same outputs, every time.
- **Explainable.** Pass `explain: true` in your `run()` options to get a per-rule explanation tree; see [api.v1.json](https://github.com/lemma/lemma/blob/main/engine/schemas/api.v1.json). Pair with the [CLI](https://github.com/lemma/lemma) for human reasoning tables.
- **Time-aware.** Multiple versions of the same spec coexist. Pass an `effective` date and the engine resolves the version in force on that day.
- **Statically checked.** Type errors, missing data, cycles, measure-family mismatches - all caught at `load()` time. Bad specs never reach `run()`.
- **Runs anywhere JavaScript does.** ~2 MB package, no native binary, no postinstall script.
- **Editor in a tab.** Includes an in-process language server and a Monaco adapter, so you can build a real Lemma editor experience client-side - diagnostics, completion, formatting... even without setting up a server.

## Install

```bash
npm install @lemmabase/lemma-engine
```

## Browser

```javascript
import { Lemma } from '@lemmabase/lemma-engine';

const engine = await Lemma();
```

`Lemma()` initializes the engine once and returns an `Engine`. Serve over **http(s)**, not `file://`. For manual control: `init()` then `new Engine()`.

If your bundler emits IIFE, can't resolve `import.meta.url`, or refuses to ship the engine module as a separate asset, use the inlined entry. Everything ships in one JS bundle:

```javascript
import { Lemma } from '@lemmabase/lemma-engine/iife';
```

esbuild users get an auto-rewriting plugin:

```javascript
import { lemmaEngineEsbuildPlugin } from '@lemmabase/lemma-engine/esbuild';

esbuild.build({ /* ... */ plugins: [lemmaEngineEsbuildPlugin()] });
```

## Node

Identical to the browser path:

```javascript
import { Lemma } from '@lemmabase/lemma-engine';

const engine = await Lemma();
```

For zero-fetch startup: `initSync({ module })` then `new Engine()`.

## In-process LSP + Monaco

```javascript
import { init } from '@lemmabase/lemma-engine';
import { LspClient } from '@lemmabase/lemma-engine/lsp-client';

await init();
const client = new LspClient(monaco);
await client.start();
await client.initialize();

client.onDiagnostics((uri, diagnostics) => { /* render */ });
client.didOpen('file:///pricing.lemma', 'lemma', 1, source);
```

A pre-wired Monaco adapter ships at `@lemmabase/lemma-engine/monaco`.

## API

`Engine` (returned by `Lemma()` or `new Engine()`):

| Method | Description |
|--------|-------------|
| `load(code)` | Load inline Lemma source as a volatile source in the default repository |
| `load(sources)` | Load multiple sources in one planning pass (`Record<label, text>` or `[label, code][]`; object keys keep insertion order; `@org/pkg` keys tag LemmaBase repositories) |
| `Engine.withLimits(limits)` | Static: create engine with named limit overrides (unknown keys throw) |
| `Engine.fromSnapshot(bytes)` | Static: restore engine from `snapshot()` bytes (`Uint8Array`) |
| `install(name)` | Download a repository from LemmaBase; resolves with `{ source, id }`. Does not load. Rejects with `EngineError[]`. |
| `list()` | Slim catalog: `ResolvedRepository[]` with `repository` and temporal `specs` rows. Always includes embedded `lemma` / `spec units`. |
| `show(repo, name, effective?)` | Spec interface + temporal window; `repo` null for the default repository. |
| `source(repo, spec?, effective?)` | Canonical Lemma source text. Omit `spec` for whole repository. |
| `run({ spec, repository?, effective?, data?, rules?, explain? })` | Evaluate. Omit `rules` for all rules; pass a non-empty array to scope. `[]` errors. Returns a `Response`. `explain: true` adds per-rule explanation trees. |
| `remove(repo, name, effective?)` | Remove a temporal spec slice. |
| `update(repo, code, attribute?)` | Upsert identities from `code`; Path/Dependency prune siblings with that label. |
| `limits()` | Resource limits for this engine. |
| `snapshot()` | Opaque bytes of parsed specs + plans + limits. Restore with `Engine.fromSnapshot`. |
| `quality()` | Structural quality recommendations across loaded specs (advisory only). |
| `format(code, attribute?)` | Canonical formatting; throws `EngineError` on parse error. |

Full TypeScript types are bundled - see `lemma.d.ts`.

Persist and restore without re-parsing (Node):

```javascript
import { writeFileSync, readFileSync } from 'node:fs';

const bytes = engine.snapshot();
writeFileSync('engine.lems', bytes);
const restored = Engine.fromSnapshot(readFileSync('engine.lems'));
```

### Install from LemmaBase

Specs that reference `uses … @org/pkg` need that repository available. `install` downloads via the host `fetch` (LemmaBase at `https://lemmabase.com`); call `load` with the repository id as the source label, then load your workspace:

```javascript
import { Lemma } from '@lemmabase/lemma-engine';

const engine = await Lemma();
const { source, id } = await engine.install('@iso/countries');
await engine.load({ [id]: source, 'app.lemma': sourceThatUsesStd });
```

In the browser, LemmaBase must allow your origin (CORS). Use `https` or `http://localhost` when using `install`.

## Status

Lemma is pre-1.0. The JavaScript API is stable for most use cases, but breaking changes may occur between minor versions. Pin your dependency version and review the [changelog](https://github.com/lemma/lemma/blob/main/CHANGELOG.md) before upgrading.

### Runtime traps (internal bugs)

An internal invariant violation (a bug in the engine) traps the runtime. The call throws a `RuntimeError` which you can catch with `try/catch`, but the loaded module is poisoned: constructing a new `Engine()` from the same initialization is not safe. To recover, call `init()` again or run the engine in a Web Worker and respawn the worker on trap. Domain failures (invalid specs, bad data, impossible rules) are reported as `EngineError[]` or vetoes and never cause traps.

## Related

- [`lemmabase.com`](https://lemmabase.com): public database for Lemma Specs
- [`lemma`](https://crates.io/crates/lemma): REPL, HTTP server, MCP server, formatter
- [`lemma-engine`](https://crates.io/crates/lemma-engine): same engine as a Rust crate
- [`lemma_engine` on Hex](https://hex.pm/packages/lemma_engine): Elixir bindings via Rustler
- VS Code / Cursor extension: search "Lemma Language" in the marketplace

## License

Apache-2.0

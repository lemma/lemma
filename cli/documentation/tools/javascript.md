---
nav_title: JavaScript / TypeScript
nav_order: 30
---

# JavaScript / TypeScript

`@lemmabase/lemma-engine` runs in the browser, Node, Bun, Deno, and edge runtimes.

## Install

```bash
npm install @lemmabase/lemma-engine
```

## Usage

```javascript
import { Lemma } from '@lemmabase/lemma-engine';

const engine = await Lemma();
await engine.load({ 'pricing.lemma': pricing });

const response = engine.run({ spec: 'pricing', data: { quantity: 50, is_vip: false } });
// response.results.unit_price → 16 eur
// response.results.total      → 800 eur
```

`Lemma()` initializes the engine once and returns an `Engine`. The response carries each rule's value (or veto), per-rule `missing_data` when inputs are still unbound, and optional explanation trees when `run` is called with `explain: true` ([api.v1.json](../../../engine/schemas/api.v1.json)). Types and suggestions are on `engine.show(...)` (`Show.data` values are `ShowData`). Non-veto results flatten `RuleResultValue` (`result` + typed field) onto each `RuleResult`.

## Browser

```javascript
import { Lemma } from '@lemmabase/lemma-engine';

const engine = await Lemma();
```

Serve over http(s), not `file://`. For manual control: `init()` then `new Engine()`.

If your bundler emits IIFE, can't resolve `import.meta.url`, or refuses to ship the engine module as a separate asset, use the inlined entry:

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
| `Engine.withLimits(limits)` | Static: create engine with named limit overrides (unknown keys throw) |
| `Engine.fromSnapshot(bytes)` | Static: restore engine from `snapshot()` bytes (`Uint8Array`) |
| `load(code)` | Load inline Lemma source as a volatile source in the default repository |
| `load(sources)` | Load multiple sources in one planning pass (object or `[label, code][]`; object keys keep insertion order, array form is the explicit ordered API; `@owner/name` keys tag LemmaBase repositories) |
| `install(name)` | Download a repository from LemmaBase; resolves with `{ source, id }`. Does not load and does not write `lemma_deps/`. |
| `list()` | JSON array of `ResolvedRepository`: each has `repository` and `specs`. |
| `show(repo?, spec, effective?)` | `Show`: data catalog + this spec's rule graph (`ShowRule`: `type`, `path`, `branches`, `depends_on_rules` as `input_key`) + temporal window (no Lemma text; empty `needed_by_rules` = reuse-only) |
| `source(repo?, spec?, effective?)` | Formatted Lemma source (omit `spec` for whole repo) |
| `run({ spec, repository?, effective?, data?, rules?, explain? })` | Evaluate. Omit `rules` for all rules; pass a non-empty array to scope. `[]` errors. Returns a `Response`. With `explain: true`, per-rule `explanation` matches [api.v1.json](../../../engine/schemas/api.v1.json). |
| `remove(repo?, name, effective?)` | Remove a temporal spec slice. |
| `update(repo?, code, attribute?)` | Upsert identities from `code`; Path/Dependency prune siblings with that label. |
| `limits()` | Resource limits for this engine. |
| `snapshot()` | Opaque bytes of parsed specs + plans + limits. Restore with `Engine.fromSnapshot`. |
| `quality()` | Structural quality recommendations across loaded specs (advisory only). |
| `format(code, attribute?)` | Canonical formatting; throws `EngineError` on parse error. |

Full TypeScript types are bundled (see `lemma.d.ts`).

Persist and restore without re-parsing (Node):

```javascript
import { writeFileSync, readFileSync } from 'node:fs';

const bytes = engine.snapshot();
writeFileSync('engine.lems', bytes);
const restored = Engine.fromSnapshot(readFileSync('engine.lems'));
```

**API values (`RuleResultValue`):** when present, always `result`, plus exactly one typed field (`measure` / `ratio` / `number` / …) or `range` instead. Same shape on `ShowData.fill` / `ShowData.suggestion`; non-veto rule results flatten those fields onto `RuleResult` (no `value` wrapper). Measure and ratio maps hold every declared unit name → magnitude string so interactive prompts can switch units.

## Install from LemmaBase

Specs that `uses` a repository id such as `@iso/countries` need that repository available. `install` downloads via the host `fetch` (LemmaBase is hard-bound to `https://lemmabase.com`); call `load` with the repository id as the source label, then load your workspace:

```javascript
import { Lemma } from '@lemmabase/lemma-engine';

const engine = await Lemma();
const { source, id } = await engine.install('@iso/countries');
await engine.load({ [id]: source, 'app.lemma': sourceThatUsesStd });
```

In the browser, LemmaBase must allow your origin (CORS).

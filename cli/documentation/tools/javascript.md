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

const response = engine.evaluate({ spec: 'pricing', data: { quantity: 50n, is_vip: false } });
// response.results.unit_price → 16 eur
// response.results.total      → 800 eur
```

`Lemma()` initializes the engine once and returns an `Engine`. Evaluation `data` is a tree: strings, booleans, `bigint`, single-unit maps (`{ eur: "84" }`), arrays. JS `number` is always a double and is rejected (`pass as string: "84"`). `null` / `undefined` omit an object value; as an array item they are rejected (`list item 2 is null`). Keys differing only in case are rejected. Rejections throw an `EngineError` object (`kind: "request"`, `related_data` set to the top-level key). The response carries each rule's value (or veto), per-rule `missing_data` when inputs are still unbound, and optional explanation trees when `evaluate` is called with `explain: true` ([api.v1.json](../../../engine/schemas/api.v1.json)). Types and suggestions are on `engine.show(...)`: a Draft 2020-12 JSON Schema for the evaluation data (`Show.properties` values are `ShowProperty`, row schemas in `Show.$defs`) with Lemma metadata under `x-lemma-*`. Validate evaluation data against it as is. Non-veto results flatten `RuleResultValue` (`result` + typed field) onto each `RuleResult`.

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
| `specs()` | JSON array of `Repository`: each has `repository` and `specs` (`SpecVersion` rows: name, window, `dependencies`). |
| `show(repo?, spec, effective?)` | `Show`: evaluation-data JSON Schema (`properties` = inputs rules need, `required`, `$defs`) + local types (`x-lemma-types`) + this spec's rule graph (`x-lemma-rules`: `ShowRule` with `type`, `path`, `branches`, `depends_on_rules` as `input_key`) + temporal window of the shown version (`x-lemma-effective-from` / `-to`; all versions via `specs()`); no Lemma text |
| `source(repo?, spec?, effective?)` | Formatted Lemma source (omit `spec` for whole repo) |
| `evaluate({ spec, repository?, effective?, data?, rules?, explain? })` | Evaluate. Omit `rules` for all rules; pass a non-empty array to scope. `[]` errors. Returns a `Response`. With `explain: true`, per-rule `explanation` matches [api.v1.json](../../../engine/schemas/api.v1.json). |
| `remove(repo?, name, effective?)` | Remove a temporal spec slice. |
| `update(repo?, code, attribute?)` | Upsert identities from `code`; Path/Dependency prune siblings with that label. |
| `limits()` | Resource limits for this engine. |
| `snapshot()` | Opaque bytes of parsed specs + plans + limits. Restore with `Engine.fromSnapshot`. |
| `quality()` | Structural quality recommendations across loaded specs (advisory only). |
| `format(code, attribute?)` | Canonical formatting; throws `EngineError` on parse error. |
| `version` | Semver of the loaded WASM build (e.g. `"0.9.12"`). |

Full TypeScript types are bundled (see `lemma.d.ts`).

Persist and restore without re-parsing (Node):

```javascript
import { writeFileSync, readFileSync } from 'node:fs';

const bytes = engine.snapshot();
writeFileSync('engine.lems', bytes);
const restored = Engine.fromSnapshot(readFileSync('engine.lems'));
```

**API values (`RuleResultValue`):** when present, always `result`, plus exactly one typed field (`measure` / `ratio` / `number` / …) or `range` instead. Same shape on `ShowProperty["x-lemma-value"]` / `ShowProperty["x-lemma-suggestion"]`; non-veto rule results flatten those fields onto `RuleResult` (no `value` wrapper). Measure and ratio maps hold every declared unit name → magnitude string so interactive prompts can switch units.

**Lists:**

- Evaluation data takes an array for a list: strings for a value list (`{ fares: ['89 eur', '120 eur'] }`), objects keyed by row data names for an instance list (`{ staff: [{ name: 'Ada', salary: '9000 eur' }] }`). The array replaces every source row.
- A list-valued result has `list`, a `RuleResultValue[]`. A row-valued result has `row` (`RowReference`: `spec`, `effective`, `instance` such as `staff.2`, `index`).
- `index` points into `response.rows`, where each referenced row appears once with its `data` and `rules`. `rows` is absent when no result reaches a row.
- `SpecResult.path` lists each `uses` hop. Show `x-lemma-type` has `kind: 'list'` with `element` and `key` (`'numbered'` or `{ fields: [...] }`); the property is `type: 'array'` with `items` (an element literal schema, or a `$ref` into `Show.$defs`). An instance list with source rows has an `x-lemma-value` whose `list` items, in row order, carry only `fields`, the fields the row sets (no `result` on the list or its rows).
- Evaluation data for a slot bound to data or a rule (`uses i: inner -> with x: base`) is rejected (`data 'i.x' is bound to 'base' by the spec and cannot be supplied`).

## Install from LemmaBase

Specs that `uses` a repository id such as `@iso/countries` need that repository available. `install` downloads via the host `fetch` (LemmaBase is hard-bound to `https://lemmabase.com`); call `load` with the repository id as the source label, then load your workspace:

```javascript
import { Lemma } from '@lemmabase/lemma-engine';

const engine = await Lemma();
const { source, id } = await engine.install('@iso/countries');
await engine.load({ [id]: source, 'app.lemma': sourceThatUsesStd });
```

In the browser, LemmaBase must allow your origin (CORS).

---
nav_title: Lemma CLI
nav_order: 20
---

# Lemma CLI

See [Installation](../installation.md) for install options.

## Commands

### `lemma evaluate`: evaluate a spec

```bash
lemma evaluate [[repo] spec] [-] [name=value ...] [--path PATH] [--rules=RULES] [options]
```

**Syntax:**
- Positionals: optional repository qualifier (e.g. `@iso/countries`), then spec name (see `lemma evaluate --help`)
- `-`: read a JSON data object from stdin (`cat order.json | lemma evaluate pricing -`). Strings, booleans, single-unit maps (`{"weight": {"kilogram": "12"}}`), arrays. A list is one key holding an array that replaces every source row: strings for a value list (`{"fares": ["89 eur", "120 eur"]}`), objects keyed by row data names for an instance list (`{"staff": [{"name": "Ada", "salary": "9000 eur"}]}`). `null` omits a key; a `null` list item is an error (`list item 2 is null`). JSON numbers are rejected (`pass as string: "84"`). Combines with `name=value`; a key given in both (case-insensitive) is an error. With `-i`, bound keys are skipped in prompts. Without `-`, stdin is not read.
- `spec --rules=rule`: evaluate one rule
- `spec --rules=rule1,rule2`: evaluate specific rules (comma-separated)
- No arguments with `-i`: interactive mode

**Options:**
- `--path <path>`: workspace directory or `.lemma` file (default: current directory)
- `--rules <rules>`: comma-separated `x-lemma-rules` keys of `lemma show --json` (omit to evaluate this spec's outputs)
- `--json`: output results as JSON (default: human-readable table). Each rule result may include `missing_data` (unbound input keys in evaluation / decision-tree order). Types, filled values, and suggestions come from `lemma show`, not from evaluate JSON.
- `-x, --explain`: include explanation trees (human: reasoning tables per rule; JSON: per-rule `explanation` objects matching [`api.v1.json`](../../../engine/schemas/api.v1.json)). Unbound inputs appear in those tables when a rule still awaits input (`MissingData` veto). JSON still carries raw per-rule `missing_data` from the engine.
- `-i, --interactive`: guided spec/rule/data selection. Instance lists used by the selected rules are prompted: keep declared rows or enter new ones, then add rows and fill only the fields evaluation still needs. Nested lists on a new row are prompted the same way. Stdin/`name=value` keys stay bound.
- `--effective <datetime>`: evaluate at effective datetime (e.g. `2025`, `2025-03`, `2025-03-04`, or ISO 8601). Date-only forms are the UTC midnight start of that period (`2025` → `2025-01-01T00:00:00Z`)

**Examples:**

```bash
lemma evaluate pricing
lemma evaluate pricing --rules=total,tax
lemma evaluate --path ./policies nl/tax/net_salary --rules=net_salary -x
lemma evaluate pricing quantity=10 is_vip=true
cat order.json | lemma evaluate pricing -
lemma evaluate pricing --json
lemma evaluate pricing -x
lemma evaluate pricing --effective 2025-01-01
lemma evaluate -i
lemma evaluate '@iso/countries' alpha2
```

### `lemma show`: declared data catalog and rules

Shows declared data slots with types and constraints (minimum, maximum, units, decimals, text options), filled values, suggestions, import path, and this spec's rule graph (`ShowRule`: type, path, branches, depends_on_rules as `input_key`). Lemma source text is available via `Engine::source` (API) only.

With `--json`, the output is a [JSON Schema](https://json-schema.org/draft/2020-12) (Draft 2020-12) for the spec's evaluation data: validate a `lemma evaluate <spec> -` payload, an HTTP POST body or MCP `evaluate` data against it as is. `title` is the spec name, `description` its commentary, `properties` one entry per input rules need (non-empty `x-lemma-needed-by-rules`), `required` the inputs a rule needs that have no value, and `$defs` the row schemas of instance lists (`LemmaRow_*`). Each property states its JSON shape with standard keywords: `type`, `pattern` for number, measure and ratio literals, `enum` for text options, `minLength` / `maxLength`, `description` for help, `default` for a value the spec sets, `examples` for a `-> suggest` hint, and `items` / `maxItems` for lists. Date, time and range literals are strings without a pattern; the engine still validates them on `evaluate`. Lemma metadata sits under `x-lemma-*`: per property `x-lemma-type` (the type without what standard keywords state), `x-lemma-path`, `x-lemma-needed-by-rules`, `x-lemma-value`, `x-lemma-suggestion`; per document `x-lemma-repository`, `x-lemma-effective-from` / `-to` (the shown version only; `lemma specs` lists all versions), `x-lemma-start-line`, `x-lemma-source-type`, `x-lemma-meta`, `x-lemma-types` (local data declarations), `x-lemma-rules`.

`show` lists local Types (all data declarations) and Data (inputs rules need: non-empty `needed_by_rules`). Text has a `Types:` section after Meta. Slots bound to data or a rule on a `uses` (`-> with x: base`) appear in Data as optional override keys (wire target documented on `x-lemma-value.reference`); evaluation data for them overrides the wire. An instance list with source rows has a value whose `list` items, in row order, carry the `fields` each row sets; a row field bound to data (`income: income`) leaves the list without a value. `x-lemma-needed-by-rules` names `x-lemma-rules` keys that still need the slot after normalize; on `$defs` row properties those keys are the **parent** rules that need the field through the list. Evaluation-data-aware pruning for a concrete `evaluate` is per-rule `results.*.missing_data`.

```bash
lemma show [[repo] spec] [--path PATH] [--effective <datetime>] [--json]
```

**Options:**
- `[repo]`: optional repository qualifier (e.g. `@iso/countries`)
- `[spec]`: spec name (omit when workspace has a single spec)
- `--path <path>`: workspace directory or `.lemma` file (default: current directory)
- `--effective <datetime>`: effective datetime for temporal specs
- `--json`: output as JSON (default: human-readable table)

**Examples:**

```bash
lemma show pricing
lemma show --path ./policies net_salary
lemma show --path tax.lemma calculator
lemma show '@iso/countries' alpha2
lemma show pricing --json
```

### `lemma specs`: loaded specs by repository

Prints local specs (the unnamed workspace and any local `repo` sections), one temporal version per line. Each version is followed by its `uses` targets, indented. Named local repositories are headers with indented rows. Dependency repositories (`lemma`, `lemma_deps`) are omitted.

`lemma specs --all` prints every loaded repository as a catalog: no nested uses. Same windows as `lemma show` (`2025-01-01...2026-01-01` when both bounds exist, `2026-01-01` when only the start is set, `...2027` when an undated spec has a successor, or the name alone when undated with no end). Several versions of one spec are several lines.

```bash
lemma specs [--path PATH] [--json] [--all]
```

**Options:**
- `--path <path>`: workspace directory or `.lemma` file (default: current directory)
- `--json`: output `Engine::specs()` JSON filtered the same way as the text view: array of `Repository` (`repository`, `specs`) where each spec is a `SpecVersion` (`name`, optional `effective_from` / `effective_to`, optional `dependencies`). `--all` includes lemma and lemma_deps.
- `-a, --all`: every loaded repository; text omits nested uses

**Examples:**

```bash
lemma specs
lemma specs --path ./project
lemma specs --json
lemma specs --all
```

### `lemma install`: install repositories from LemmaBase

Resolves `@...` references and downloads repositories from LemmaBase into `lemma_deps/`.

```bash
lemma install [--path PATH] --all
lemma install [--path PATH] <repository> -f
```

**Options:**
- `--path <path>`: workspace directory or `.lemma` file (default: current directory)
- `-a, --all`: install all `@...` references in the workspace
- `-f, --force`: overwrite existing repositories when content has changed on LemmaBase

### `lemma format`: format .lemma files and ```lemma fences

```bash
lemma format [paths...] [--check] [--stdout]
```

Formats `.lemma` sources and rewrites `` ```lemma `` fence bodies inside `.md` / `.txt` (`` ```lemma-skip `` is left alone). Walks directories recursively and respects `.gitignore`.

**Options:**
- `--check`: check formatting without modifying (exit 1 if any file would change)
- `--stdout`: write formatted output to stdout

### `lemma server`: start HTTP server

```bash
lemma server [--path PATH] [--host <host>] [-p <port>] [--watch] [--explain] [--eval-timeout SECONDS] [--cors]
```

**Options:**
- `--path <path>`: workspace directory or `.lemma` file (default: current directory)
- `--host <host>`: bind address (default: `127.0.0.1`)
- `-p, --port <port>`: port (default: `8012`)
- `--watch`: live-reload on `.lemma` file changes
- `--explain`: enable explanation generation (clients send `x-explain` header; JSON shape [`api.v1.json`](../../../engine/schemas/api.v1.json))
- `--eval-timeout <second>`: wall-clock timeout for a single evaluation request (default: `10`)
- `--cors`: allow cross-origin browser requests from any origin (off by default)

SIGINT/SIGTERM stop accepting new connections, drain in-flight requests up to `--eval-timeout`, then exit 0.

**Routes:**

| Method | Route | Description |
|--------|-------|-------------|
| GET | `/` | Loaded specs (`lemma specs --all --json`) |
| GET | `/{spec}` | Show spec interface: the evaluation-data JSON Schema of `lemma show --json` |
| POST | `/{spec}` | Evaluate (data as JSON or form body; lists only in a JSON body). `?rules=a,b` limits which rules to evaluate |
| GET | `/openapi.json` | OpenAPI 3.1 specification |
| GET | `/docs` | Interactive API documentation (Scalar) |
| GET | `/health` | Health check |

**Example:**

```bash
lemma server --path ./policies --watch

curl http://localhost:8012/pricing

curl -X POST http://localhost:8012/pricing \
  -H "Content-Type: application/json" \
  -d '{"quantity": "10", "is_member": true}'

curl -X POST http://localhost:8012/payroll \
  -H "Content-Type: application/json" \
  -d '{"staff": [{"name": "Ada", "salary": "9000 eur"}, {"name": "Grace", "salary": "9500 eur"}]}'
```

### `lemma lsp`: start language server

Starts the Language Server Protocol server over stdio for editor integration (diagnostics, formatting, semantic tokens). The VS Code/Cursor extension invokes this automatically; a globally installed `lemma` CLI is the only requirement.

Workspace file discovery uses the same policy as `lemma specs` / `lemma evaluate`: project `.gitignore` rules, plus always loading `<workdir>/lemma_deps/**/*.lemma`. The CLI injects that discovery and a filesystem watch into the LSP. The watch covers discovered workspace `.lemma` files and `lemma_deps/` only (not a recursive whole-tree watch), so disk changes such as `lemma install` update diagnostics without hanging on large build directories. Editor open/change/save/close remain the primary sync path for buffers.

```bash
lemma lsp
```

### `lemma mcp`: start MCP server

Start the MCP server so AI assistants can work with workspace specs. Default transport is stdio. Pass `--http` for Streamable HTTP (`POST /mcp`). Full setup guide: [MCP](../tools/mcp.md).

```bash
lemma mcp [--path PATH] [--write] [--request-timeout SECONDS]
lemma mcp --http [--path PATH] [--write] [--request-timeout SECONDS] [--host HOST] [--port PORT] [--cors]
```

**Options:**
- `--path <path>`: workspace directory or `.lemma` file (default: current directory)
- `--write`: enable write tools (read-only by default)
- `--request-timeout <seconds>`: wall-clock timeout for a single request (default: `10`)
- `--http`: Streamable HTTP instead of stdio
- `--host <host>`: bind address when using `--http` (default: `127.0.0.1`)
- `--port <port>`: listen port when using `--http` (default: `8013`; `lemma server` uses `8012`)
- `--cors`: permissive CORS when using `--http` (off by default)

HTTP MCP has no built-in authentication or TLS. It binds to localhost by default; for non-localhost bind, put it behind a reverse proxy that terminates TLS and enforces access control.
## Workspace

A workspace is a directory containing `.lemma` files. Commands that load specs use `--path` to select the workspace (default: current directory). Every `.lemma` file is loaded recursively from that directory, plus any repositories in `lemma_deps/`.

```
policies/
  pricing.lemma
  shipping.lemma
  tax.lemma
```

## Resource Limits

Resource limits control parse-time and planning-time budgets. These are security boundaries that prevent unbounded resource consumption from untrusted input.

| Limit | Default | Purpose |
|-------|---------|---------|
| `max_sources` | 4096 | Maximum source files in one engine |
| `max_loaded_bytes` | 50 MB | Total source text across all files |
| `max_source_size_bytes` | 5 MB | Single source file size |
| `max_expression_depth` | 7 | AST nesting depth |
| `max_expression_count` | 65,536 | Expression nodes per source (parser) |
| `max_normalized_expression_nodes` | 30,000 | Unique normal-form cells reachable from one rule root after normalize (rule references count as one cell) |
| `max_data_value_bytes` | 1 KB | Single data value size (each item of a value list; whole lists are bounded by the `max_list_*` limits) |
| `max_list_rows` | 1024 | Rows in one evaluation-data list |
| `max_list_depth` | 32 | Nested self-list depth, the spec holding the list being level 1 |
| `max_list_nodes` | 4096 | Total rows across one evaluation-data list, all depths |
| `max_spec_dependency_depth` | 32 | `uses` chain depth |
| `max_dag_specs` | 4096 | Total specs in dependency DAG |

The `max_list_*` limits are checked when evaluation data is bound, before any rule evaluates; exceeding one is a request error.

`max_expression_depth` bounds nesting (parentheses, nested `^`, nested function arguments) in the parser, which walks source with an explicit stack. Operator chains (`+`, `-`, `*`, `/`, `mod`, `and`) are flat in the source and are bounded by `max_expression_count`, not by depth. `max_spec_dependency_depth` bounds `uses` chain depth. Raising `max_expression_depth` or `max_spec_dependency_depth` beyond the defaults requires stack analysis for your deployment.

## API Defaults

**Effective datetime**: when no `--effective` flag or `Accept-Datetime` header is provided, the engine uses the current instant to select the temporal version of the root spec.

**Accept-Datetime (HTTP)**: clients send `Accept-Datetime` with the same formats as `--effective`: `YYYY`, `YYYY-MM`, `YYYY-MM-DD`, or an ISO 8601 datetime. Date-only forms resolve to the UTC midnight start of that period (a precise evaluation instant). Empty or omitted → now. Invalid values are a bad request. Responses include `Vary: Accept-Datetime`. When the resolved spec row has an `effective_from`, the server also sets `Memento-Datetime` to that instant.

**Explanations**: disabled by default in CLI (`lemma evaluate`), HTTP, and SDKs. Use `--explain` (CLI and server) + `x-explain` (HTTP client), or `explain: true` (SDK) to opt in. MCP `evaluate`/`evaluate` always return formatted ASCII explanation trees (no `explain` arg, no opt-out) plus a `Missing data` block when inputs are unbound. Evaluate JSON (CLI `--json`, HTTP, SDKs) has no top-level `data` array: unbound inputs are per-rule `missing_data`; types and suggestions come from `lemma show` / MCP `show`. When explanations are enabled on JSON surfaces, each `results.<rule>.explanation` is a root rule node (`"type":"rule"`, `"name"`, `"result"`, `"body"`, optional `"causes"` / `"children"`, plus a `"rules"` table of reachable narrated rules) per [`api.v1.json`](../../../engine/schemas/api.v1.json). Nested rule uses are `"type":"rule_ref"` (`id` into `rules`); bound data `"type":"data"`; unused cause paths `"type":"data_unused"`; list reads may include `"type":"row"` and `"type":"filter"`.

## See Also

- [Learn guide](../learn/readme.md)
- [Installation](../installation.md)
- [Language reference](readme.md)
- [API schema](../../../engine/schemas/api.v1.json) (Show, Response, specs, errors, `RuleResult.explanation` / `ExplanationNode`)
- [LemmaBase](registry.md)
- [Engine test coverage](coverage/engine.md)
- [CLI test coverage](coverage/cli.md)
- [CLI benchmarks](benchmarks/cli.md)
- [Engine benchmarks](benchmarks/engine.md)

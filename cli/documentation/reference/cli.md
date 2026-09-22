---
nav_title: Lemma CLI
nav_order: 20
---

# Lemma CLI

See [Installation](../installation.md) for install options.

## Commands

### `lemma run`: evaluate a spec

```bash
lemma run [[repo] spec] [name=value ...] [--prefix PATH] [--rules=RULES] [options]
```

**Syntax:**
- Positionals: optional repository qualifier (e.g. `@iso/countries`), then spec name (see `lemma run --help`)
- `spec --rules=rule`: evaluate one rule
- `spec --rules=rule1,rule2`: evaluate specific rules (comma-separated)
- No arguments with `-i`: interactive mode

**Options:**
- `--prefix <path>`: workspace directory or `.lemma` file (default: current directory)
- `--rules <rules>`: comma-separated `Show.rules` keys (omit to evaluate this spec's outputs)
- `--json`: output results as JSON (default: human-readable table). Each rule result may include `missing_data` (unbound input keys in evaluation / decision-tree order). Types, filled values, and suggestions come from `lemma show`, not from evaluate JSON.
- `-x, --explain`: include explanation trees (human: reasoning tables per rule; JSON: per-rule `explanation` objects matching [`api.v1.json`](../../../engine/schemas/api.v1.json)). Unbound inputs appear in those tables when a rule still awaits input (`MissingData` veto). JSON still carries raw per-rule `missing_data` from the engine.
- `-i, --interactive`: guided spec/rule/data selection
- `--effective <datetime>`: evaluate at effective datetime (e.g. `2025`, `2025-03`, `2025-03-04`)

**Examples:**

```bash
lemma run pricing
lemma run pricing --rules=total,tax
lemma run --prefix ./policies nl/tax/net_salary --rules=net_salary -x
lemma run pricing quantity=10 is_vip=true
lemma run pricing --json
lemma run pricing -x
lemma run pricing --effective 2025-01-01
lemma run -i
lemma run '@iso/countries' alpha2
```

### `lemma show`: declared data catalog and rules

Shows declared data slots with types and constraints (minimum, maximum, units, decimals, text options), filled values, suggestions, import `path`, and this spec's rule graph (`ShowRule`: type, path, branches, depends_on_rules as `input_key`). Lemma source text is available via `Engine::source` (API) only.

`show` lists every declared promptable data slot (types, constraints, `fill`, suggestions, `path`). `needed_by_rules` names Show.rules keys that still need the slot after normalize; empty means offered for reuse (`data x: alias.slot`), not an eval intake key for this spec. Run-data-aware pruning for a concrete `run` is per-rule `results.*.missing_data`.

```bash
lemma show [[repo] spec] [--prefix PATH] [--effective <datetime>] [--json]
```

**Options:**
- `[repo]`: optional repository qualifier (e.g. `@iso/countries`)
- `[spec]`: spec name (omit when workspace has a single spec)
- `--prefix <path>`: workspace directory or `.lemma` file (default: current directory)
- `--effective <datetime>`: effective datetime for temporal specs
- `--json`: output as JSON (default: human-readable table)

**Examples:**

```bash
lemma show pricing
lemma show --prefix ./policies net_salary
lemma show --prefix tax.lemma calculator
lemma show '@iso/countries' alpha2
lemma show pricing --json
```

### `lemma list`: list loaded specs by repository

Lists every loaded spec, grouped by repository. Local specs (no repository qualifier) are printed unindented; named repositories (including embedded `lemma`) appear as headers with indented spec names.

```bash
lemma list [--prefix PATH] [--json]
```

**Options:**
- `--prefix <path>`: workspace directory or `.lemma` file (default: current directory)
- `--json`: output `Engine::list()` JSON: array of `{ "repository", "specs" }` where each spec is a `ListedSpec` (`name`, optional `effective_from` / `effective_to`). Human text lists unique spec names only.

**Examples:**

```bash
lemma list
lemma list --prefix ./project
lemma list --json
```

### `lemma install`: install repositories from LemmaBase

Resolves `@...` references and downloads repositories from LemmaBase into `lemma_deps/`.

```bash
lemma install [--prefix PATH] --all
lemma install [--prefix PATH] <repository> -f
```

**Options:**
- `--prefix <path>`: workspace directory or `.lemma` file (default: current directory)
- `-a, --all`: install all `@...` references in the workspace
- `-f, --force`: overwrite existing repositories when content has changed on LemmaBase

### `lemma format`: format .lemma files

```bash
lemma format [paths...] [--check] [--stdout]
```

**Options:**
- `--check`: check formatting without modifying (exit 1 if any file would change)
- `--stdout`: write formatted output to stdout

### `lemma server`: start HTTP server

```bash
lemma server [--prefix PATH] [--host <host>] [-p <port>] [--watch] [--explain] [--eval-timeout SECONDS] [--cors]
```

**Options:**
- `--prefix <path>`: workspace directory or `.lemma` file (default: current directory)
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
| GET | `/` | List all specs |
| GET | `/{spec}` | Show spec interface (data, rules, versions) |
| POST | `/{spec}` | Evaluate (data as JSON or form body) |
| GET/POST | `/{spec}/{rules}` | Evaluate specific rules (comma-separated) |
| GET | `/openapi.json` | OpenAPI 3.1 specification |
| GET | `/docs` | Interactive API documentation (Scalar) |
| GET | `/health` | Health check |

**Example:**

```bash
lemma server --prefix ./policies --watch

curl "http://localhost:8012/pricing?quantity=10&is_member=true"

curl -X POST http://localhost:8012/pricing \
  -H "Content-Type: application/json" \
  -d '{"quantity": 10, "is_member": true}'
```

### `lemma lsp`: start language server

Starts the Language Server Protocol server over stdio for editor integration (diagnostics, formatting, semantic tokens). The VS Code/Cursor extension invokes this automatically; a globally installed `lemma` CLI is the only requirement.

Workspace file discovery uses the same policy as `lemma list` / `lemma run`: project `.gitignore` rules, plus always loading `<workdir>/lemma_deps/**/*.lemma`. The CLI injects that discovery and a filesystem watch into the LSP. The watch covers discovered workspace `.lemma` files and `lemma_deps/` only (not a recursive whole-tree watch), so disk changes such as `lemma install` update diagnostics without hanging on large build directories. Editor open/change/save/close remain the primary sync path for buffers.

```bash
lemma lsp
```

### `lemma mcp`: start MCP server

Start the MCP server so AI assistants can work with workspace specs. Default transport is stdio. Pass `--http` for Streamable HTTP (`POST /mcp`). Full setup guide: [MCP](../tools/mcp.md).

```bash
lemma mcp [--prefix PATH] [--write] [--request-timeout SECONDS]
lemma mcp --http [--prefix PATH] [--write] [--request-timeout SECONDS] [--host HOST] [--port PORT] [--cors]
```

**Options:**
- `--prefix <path>`: workspace directory or `.lemma` file (default: current directory)
- `--write`: enable write tools (read-only by default)
- `--request-timeout <seconds>`: wall-clock timeout for a single request (default: `10`)
- `--http`: Streamable HTTP instead of stdio
- `--host <host>`: bind address when using `--http` (default: `127.0.0.1`)
- `--port <port>`: listen port when using `--http` (default: `8013`; `lemma server` uses `8012`)
- `--cors`: permissive CORS when using `--http` (off by default)

HTTP MCP has no built-in authentication or TLS. It binds to localhost by default; for non-localhost bind, put it behind a reverse proxy that terminates TLS and enforces access control.
## Workspace

A workspace is a directory containing `.lemma` files. Commands that load specs use `--prefix` to select the workspace (default: current directory). Every `.lemma` file is loaded recursively from that directory, plus any repositories in `lemma_deps/`.

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
| `max_normal_form_depth` | 4096 | Nesting depth of a rule's normalized NormalForm DAG (rule references count as one level) |
| `max_data_value_bytes` | 1 KB | Single data value size |
| `max_spec_dependency_depth` | 32 | `uses` chain depth |
| `max_dag_specs` | 4096 | Total specs in dependency DAG |

`max_expression_depth` and `max_spec_dependency_depth` bound recursion during parsing and planning. Raising these beyond the defaults requires stack analysis for your deployment.

## API Defaults

**Effective datetime**: when no `--effective` flag or `Accept-Datetime` header is provided, the engine uses the current instant to select the temporal version of the root spec.

**Accept-Datetime (HTTP)**: clients send `Accept-Datetime` with the same formats as `--effective`: `YYYY`, `YYYY-MM`, `YYYY-MM-DD`, or an ISO 8601 datetime. Empty or omitted → now. Invalid values are a bad request. Responses include `Vary: Accept-Datetime`. When the resolved spec row has an `effective_from`, the server also sets `Memento-Datetime` to that instant.

**Explanations**: disabled by default in CLI (`lemma run`), HTTP, and SDKs. Use `--explain` (CLI and server) + `x-explain` (HTTP client), or `explain: true` (SDK) to opt in. MCP `run`/`evaluate` always return formatted ASCII explanation trees (no `explain` arg, no opt-out) plus a `Missing data` block when inputs are unbound. Evaluate/run JSON (CLI `--json`, HTTP, SDKs) has no top-level `data` array: unbound inputs are per-rule `missing_data`; types and suggestions come from `lemma show` / MCP `show`. When explanations are enabled on JSON surfaces, each `results.<rule>.explanation` is a rule node (`"type":"rule"`, `"name"`, `"result"`, `"body"`, optional `"causes"` / `"children"`) per [`api.v1.json`](../../../engine/schemas/api.v1.json). Bound data uses `"type":"data"`, unused cause paths `"type":"data_unused"`.

## See Also

- [Learn guide](../learn/readme.md)
- [Installation](../installation.md)
- [Language reference](readme.md)
- [API schema](../../../engine/schemas/api.v1.json) (Show, Response, list, errors, `RuleResult.explanation` / `ExplanationNode`)
- [LemmaBase](registry.md)
- [Engine test coverage](coverage/engine.md)
- [CLI test coverage](coverage/cli.md)
- [CLI benchmarks](benchmarks/cli.md)
- [Engine benchmarks](benchmarks/engine.md)

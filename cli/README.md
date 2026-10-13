# Lemma CLI

> **A command-line interface for the Lemma language.**

This package provides the `lemma` CLI for evaluating, inspecting and serving Lemma specs.

## Status

Lemma is pre-1.0. The CLI is stable for most use cases, but breaking changes may occur between minor versions. Pin your dependency version and review the [changelog](https://github.com/lemma/lemma/blob/main/CHANGELOG.md) before upgrading.

## Installation

```bash
cargo install lemma
```

After installation the `lemma` binary is available on your PATH.

## Common commands

```bash
# Evaluate a spec (all rules)
lemma evaluate shipping

# Evaluate specific rules
lemma evaluate tax_calculation --rules=tax_owed

# Provide data values
lemma evaluate tax_calculation income=75000 filing_status="married"

# Provide data as JSON from stdin (lists are passed this way)
cat order.json | lemma evaluate order -

# Explore specs interactively
lemma evaluate --interactive

# Show the inputs, types and rules of a spec
lemma show pricing

# The same as a JSON Schema for the evaluation data
lemma show pricing --json

# Loaded specs grouped by repository
lemma specs
lemma specs --path ./my_project
lemma specs --json
lemma specs --all

# Start the HTTP server
lemma server --path ./engine/documentation/examples --port 8012

# Start the MCP server (AI assistant integration)
lemma mcp --path ./engine/documentation/examples
```

Each command supports `--help` for full usage details.

## Features

- **Deterministic evaluations**: same audit trail as the engine library
- **Interactive mode**: select specs, rules, and data without typing full paths
- **HTTP server**: evaluate specs over REST, perfect for integration tests and dashboards
- **MCP server**: expose Lemma to AI assistants via the Model Context Protocol
- **Machine-readable output**: `--json` flag for tooling and pipelines

## Example session

```
$ lemma evaluate shipping
┌───────────────┬───────────┐
│ base_shipping ┆ 8.99 eur  │
├───────────────┼───────────┤
│ express_fee   ┆ 4.99 eur  │
├───────────────┼───────────┤
│ total_cost    ┆ 13.98 eur │
└───────────────┴───────────┘
```

JSON for tooling:

```bash
lemma evaluate shipping --json > output.json
```

## Documentation

- CLI reference: <https://github.com/lemma/lemma/blob/main/cli/documentation/reference/cli.md>
- Learn guide: <https://github.com/lemma/lemma/blob/main/cli/documentation/learn/readme.md>
- API docs (engine): <https://docs.rs/lemma-engine>
- Examples: <https://github.com/lemma/lemma/tree/main/engine/documentation/examples>

## Contributing

Contributions are very welcome!

## License

Apache 2.0

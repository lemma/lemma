---
nav_title: Contributing
parent: Community
nav_order: 10
---

# Contributing

Open issues, propose language changes, improve docs, or build examples around real rules. Pull requests are welcome.

## Setup

```bash
git clone https://github.com/lemma/lemma
cd lemma
cargo nextest run --workspace
```

### Optional tools

For WASM development:

```bash
cargo install wasm-pack --version 0.15.0 --locked
```

For fuzzing (requires nightly Rust):

```bash
rustup install nightly
cargo install cargo-fuzz
```

For security audits:

```bash
cargo install cargo-deny
cargo deny check --config .cargo/deny.toml
```

## Making changes

1. Write a test first
2. Make your changes
3. Run before submitting (from repo root):

   ```bash
   cargo precommit
   ```

   That is the local gate. Pull requests run `cargo precommit --coverage` (same gate, then the committed coverage-report check). Push to `main` runs `cargo precommit --coverage --fuzz`. The gate: versions-verify, Hex `mix precommit`, VS Code `npm ci` + `npm audit --audit-level=low` + package, fmt, clippy (`--all-features`), nextest (including ignored benches), WASM npm build+test, Maven `./mvnw -B verify` (after `lemma_jni` build), NuGet `dotnet test` (after `lemma_dotnet` + UniFFI generate, formatted with CSharpier 1.3.0), cargo-deny. `cargo package --list` of the crates.io crates fails on a `warning:` line (package-set hygiene; release bottom-up publish owns registry compile). `--coverage` runs `cargo coverage all --check`. `--fuzz` runs 5 minutes total across `engine/fuzz` targets. Requires `cargo-nextest`, `cargo-deny`, Elixir/Mix, Node.js, `wasm-pack`, a **JDK 21+**, and the **.NET 8 SDK** (`uniffi-bindgen-cs` tag `v0.11.0+v0.31.0`); `--fuzz` also needs nightly and `cargo-fuzz`. Regenerate coverage with `cargo coverage all` when engine or CLI sources change (`cargo-llvm-cov` required). `cargo nextest` alone is Rust tests only.

### Release version (maintainers)

The workspace release is `[workspace.package] version` in the root `Cargo.toml`. The same number must appear in path-dep pins, Hex `mix.exs`, Maven `pom.xml`, NuGet `Lemmabase.Lemma.Engine.csproj` / `engine.version`, Maven and NuGet install snippets in root/`engine`/`cli/documentation/tools` READMEs, `engine/README.md` Cargo example, and the VS Code extension `package.json` (see `xtask/src/versions.rs` module `tracked`).

- **`cargo bump <semver>`**: update all locations, then refresh `Cargo.lock` (`cargo generate-lockfile`), Hex `mix.lock` (`mix deps.get`), and VS Code `package-lock.json` (`npm install --package-lock-only`).
- **`cargo verify`**: confirm everything matches (`versions-verify` is also the first step of `cargo precommit`).

Do not hand-edit those copies unless you keep them in sync.

## Pull requests

A pull request runs `cargo precommit --coverage`. Push to `main` runs `cargo precommit --coverage --fuzz`. That must pass.

## Project structure

- `cli/`: CLI application (HTTP server, MCP server, interactive mode, formatter)
- `engine/`: core parser, planner, and evaluator (parse → plan NormalForm DAG → evaluate; see [engine/README.md](https://github.com/lemma/lemma/blob/main/engine/README.md))
- `engine/fuzz/`: fuzz testing targets
- `openapi/`: Lemma-to-OpenAPI generation

## Testing

### Unit and integration tests

```bash
cargo nextest run --workspace
```

Hex NIF (ExUnit):

```bash
cd engine/packages/hex && mix test
```

See [engine/tests/README.md](https://github.com/lemma/lemma/blob/main/engine/tests/README.md) (catalog + semantics audit) and [cli/tests/README.md](https://github.com/lemma/lemma/blob/main/cli/tests/README.md).

### Fuzz testing

Push to `main` checks committed coverage reports, then runs 5 minutes of fuzz total:

```bash
cargo precommit --coverage --fuzz
```

Requires nightly Rust and `cargo-fuzz`. Local long campaign (1 hour total across targets, from repo root):

```bash
cargo fuzzing
# or: cargo fuzzing --seconds 300
```

Manual single-target run:

```bash
cd engine/fuzz
cargo +nightly fuzz list
mkdir -p corpus/fuzz_parser
cargo +nightly fuzz run fuzz_parser corpus/fuzz_parser seeds/fuzz_parser -- -max_total_time=60 -dict=lemma.dict
```

### WASM build and test

`cargo precommit` runs these from `engine/packages/npm` automatically. To run manually:

```bash
cd engine/packages/npm
node build.js   # wasm-pack → lemma.bindings.js; copies entrypoints and lsp-client
node test.js
```

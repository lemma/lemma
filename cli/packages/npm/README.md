# lemma

> **A pure, declarative language for business rules.** Command-line interface for [Lemma](https://github.com/lemma/lemma).

Run Lemma without installing anything globally:

```bash
npx lemma help
```

Or install globally:

```bash
npm install -g lemma
lemma --help
```

## How it works

This package is a thin Node.js launcher. The actual `lemma` binary is shipped as a platform-specific optional dependency (one of `@lemmabase/cli-linux-x64`, `@lemmabase/cli-linux-arm64`, `@lemmabase/cli-darwin-x64`, `@lemmabase/cli-darwin-arm64`, `@lemmabase/cli-win32-x64`, `@lemmabase/cli-win32-arm64`). npm installs only the package matching your OS and CPU; the launcher then `exec`s the native binary directly. No download steps, no `postinstall` scripts, works offline.

## Supported platforms

- Linux x86_64 (musl, statically linked)
- Linux aarch64 (musl, statically linked)
- macOS x86_64
- macOS aarch64 (Apple Silicon)
- Windows x86_64
- Windows aarch64 (ARM64)

If your platform is not supported, install via Cargo instead:

```bash
cargo install lemma
```

## Common commands

```bash
npx lemma evaluate shipping
npx lemma evaluate tax_calculation --rules=tax_owed
npx lemma evaluate tax_calculation income=75000 filing_status="married"
npx lemma specs
npx lemma show pricing
npx lemma server --path ./examples --port 8012
```

Each command supports `--help`.

## Documentation

- CLI reference: <https://github.com/lemma/lemma/blob/main/cli/documentation/reference/cli.md>
- Learn guide: <https://github.com/lemma/lemma/blob/main/cli/documentation/learn/readme.md>
- Examples: <https://github.com/lemma/lemma/tree/main/engine/documentation/examples>

## Local development

To rebuild and test this package locally:

```bash
cargo build --release -p lemma
mkdir -p /tmp/lemma-bins/linux-x64
cp target/release/lemma /tmp/lemma-bins/linux-x64/lemma
node cli/packages/npm/build.js --binaries /tmp/lemma-bins
cd cli/packages/npm/dist/lemma && npm pack --dry-run
```

End-to-end smoke test:

```bash
cd cli/packages/npm/dist
npm pack ./lemma ./platforms/linux-x64
mkdir -p /tmp/lemma-smoke && cd /tmp/lemma-smoke
npm init -y >/dev/null
npm install /path/to/cli/packages/npm/dist/lemma-*.tgz \
            /path/to/cli/packages/npm/dist/lemmabase-cli-linux-x64-*.tgz
npx lemma --help
```

## License

Apache-2.0

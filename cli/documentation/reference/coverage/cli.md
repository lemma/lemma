---
nav_title: CLI test coverage
parent: Reference
nav_order: 56
---

# CLI test coverage

Numbers are produced by `cargo coverage cli`.

## Methodology

- Tool: [`cargo-llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov) driving [`cargo-nextest`](https://nexte.st/) on native targets.
- Scope: `lemma` CLI crate library unit tests, integration tests (`cli/tests/**`), and the `lemma` binary entrypoint via `cargo llvm-cov nextest -p lemma --lib --tests --bin lemma`.
- Line, function, and region percentages come from LLVM source-based coverage.
- Each run starts with `cargo llvm-cov clean` on the target crate so repeated measurements stay deterministic.
- Tests run single-threaded (`NEXTEST_TEST_THREADS=1`) so coverage counters stay stable across runs.

### Out of scope

- `lemma-engine` source (see [Engine test coverage](engine.md); CLI integration tests exercise engine code but engine line coverage is authoritative there)
- WASM npm package, Hex NIF, LSP, and OpenAPI crates

## Environment

- Host: `Linux 7.0.0-31-generic x86_64`
- Rustc:

```
rustc 1.92.0 (ded5c06cf 2025-12-08)
binary: rustc
commit-hash: ded5c06cf21d2b93bffd5d884aa6e96934ee4234
commit-date: 2025-12-08
host: x86_64-unknown-linux-gnu
release: 1.92.0
LLVM version: 21.1.3
```

## Summary

| Metric | Covered | Total | Percent |
|--------|--------:|------:|--------:|
| Lines | 3259 | 5593 | 58.27% |
| Functions | 268 | 494 | 54.25% |
| Regions | 5064 | 8455 | 59.89% |

## Test run

- Total: 244
- Passed: 244
- Skipped: 0
- Failed: 0

## Per-module coverage

Sorted by line coverage ascending (weakest first). Only files under `src/` for this crate are listed.

| Module | Line % | Function % | Region % | Lines covered/total |
|--------|-------:|-----------:|---------:|--------------------:|
| `mcp/http.rs` | 7.46 | 9.52 | 9.25 | 25/335 |
| `server.rs` | 20.35 | 16.90 | 32.34 | 139/683 |
| `interactive.rs` | 38.57 | 29.58 | 39.01 | 373/967 |
| `main.rs` | 52.94 | 64.41 | 47.83 | 369/697 |
| `error_formatter.rs` | 61.76 | 100.00 | 62.26 | 42/68 |
| `workspace.rs` | 72.88 | 66.67 | 75.32 | 446/612 |
| `install.rs` | 79.08 | 67.44 | 76.47 | 412/521 |
| `formatter.rs` | 84.13 | 81.82 | 86.47 | 106/126 |
| `mcp/server.rs` | 84.16 | 81.45 | 83.29 | 1217/1446 |
| `data_json.rs` | 94.20 | 89.47 | 93.52 | 130/138 |

## Related docs

- [CLI integration test catalog](../../../cli/tests/README.md)
- [Engine test coverage](engine.md)
- [CLI benchmarks](../benchmarks/cli.md)
<!-- coverage-input-digest: 0ed926a42b6acab2 -->

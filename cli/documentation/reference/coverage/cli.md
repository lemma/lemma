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
| Lines | 3262 | 5587 | 58.39% |
| Functions | 271 | 493 | 54.97% |
| Regions | 5064 | 8443 | 59.98% |

## Test run

- Total: 243
- Passed: 243
- Skipped: 0
- Failed: 0

## Per-module coverage

Sorted by line coverage ascending (weakest first). Only files under `src/` for this crate are listed.

| Module | Line % | Function % | Region % | Lines covered/total |
|--------|-------:|-----------:|---------:|--------------------:|
| `mcp/http.rs` | 7.46 | 9.52 | 9.25 | 25/335 |
| `server.rs` | 20.20 | 16.44 | 31.91 | 138/683 |
| `interactive.rs` | 38.87 | 31.82 | 39.31 | 372/957 |
| `main.rs` | 52.94 | 64.41 | 47.83 | 369/697 |
| `error_formatter.rs` | 61.76 | 100.00 | 62.26 | 42/68 |
| `workspace.rs` | 73.04 | 66.67 | 75.41 | 447/612 |
| `install.rs` | 79.08 | 67.44 | 76.47 | 412/521 |
| `mcp/server.rs` | 84.16 | 81.45 | 83.29 | 1217/1446 |
| `formatter.rs` | 85.62 | 84.62 | 87.92 | 125/146 |
| `data_json.rs` | 94.26 | 94.74 | 94.39 | 115/122 |

## Related docs

- [CLI integration test catalog](../../../cli/tests/README.md)
- [Engine test coverage](engine.md)
- [CLI benchmarks](../benchmarks/cli.md)
<!-- coverage-input-digest: fac09f598b793d3b -->

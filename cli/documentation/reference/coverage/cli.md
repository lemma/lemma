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

- Host: `Linux 7.0.0-38-generic x86_64`
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
| Lines | 3926 | 6516 | 60.25% |
| Functions | 303 | 532 | 56.95% |
| Regions | 5912 | 9711 | 60.88% |

## Test run

- Total: 293
- Passed: 293
- Skipped: 0
- Failed: 0

## Per-module coverage

Sorted by line coverage ascending (weakest first). Only files under `src/` for this crate are listed.

| Module | Line % | Function % | Region % | Lines covered/total |
|--------|-------:|-----------:|---------:|--------------------:|
| `mcp/http.rs` | 7.37 | 9.52 | 9.25 | 25/339 |
| `server.rs` | 21.27 | 20.31 | 34.12 | 147/691 |
| `interactive.rs` | 50.22 | 47.15 | 47.82 | 896/1784 |
| `error_formatter.rs` | 61.76 | 100.00 | 62.26 | 42/68 |
| `main.rs` | 61.99 | 69.35 | 56.48 | 460/742 |
| `workspace.rs` | 73.59 | 66.67 | 75.62 | 443/602 |
| `install.rs` | 78.79 | 68.18 | 75.74 | 416/528 |
| `mcp/server.rs` | 83.00 | 80.83 | 82.30 | 1191/1435 |
| `formatter.rs` | 84.76 | 80.00 | 86.05 | 89/105 |
| `data_json.rs` | 96.67 | 80.00 | 93.88 | 29/30 |
| `doc_fences.rs` | 97.92 | 100.00 | 97.98 | 188/192 |

## Related docs

- [CLI integration test catalog](../../../cli/tests/README.md)
- [Engine test coverage](engine.md)
- [CLI benchmarks](../benchmarks/cli.md)
<!-- coverage-input-digest: f347284d2a7e40d5 -->

---
nav_title: Engine test coverage
parent: Reference
nav_order: 55
---

# Engine test coverage

Numbers are produced by `cargo coverage engine`.

## Methodology

- Tool: [`cargo-llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov) driving [`cargo-nextest`](https://nexte.st/) on native (non-wasm) targets.
- Scope: `lemma-engine` library unit tests (`engine/src/**`) plus integration tests (`engine/tests/**`) via `cargo llvm-cov nextest -p lemma-engine --lib --tests`.
- Line, function, and region percentages come from LLVM source-based coverage.
- Each run starts with `cargo llvm-cov clean` on the target crate so repeated measurements stay deterministic.
- Tests run single-threaded (`NEXTEST_TEST_THREADS=1`) so coverage counters stay stable across runs.

### Out of scope

- `engine/src/wasm.rs` (built for `wasm32-unknown-unknown` only)
- Fuzz targets under `engine/fuzz/`
- Hex NIF (`lemma_hex`), LSP, OpenAPI, and CLI crates

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
| Lines | 39958 | 47082 | 84.87% |
| Functions | 3109 | 3569 | 87.11% |
| Regions | 58241 | 69548 | 83.74% |

## Test run

- Total: 2671
- Passed: 2671
- Skipped: 0
- Failed: 0

## Per-module coverage

Sorted by line coverage ascending (weakest first). Only files under `src/` for this crate are listed.

| Module | Line % | Function % | Region % | Lines covered/total |
|--------|-------:|-----------:|---------:|--------------------:|
| `mcp/catalog.rs` | 0.00 | 0.00 | 0.00 | 0/171 |
| `lib.rs` | 7.27 | 10.00 | 7.95 | 4/55 |
| `mcp/error.rs` | 40.91 | 40.00 | 47.22 | 9/22 |
| `computation/operation_result.rs` | 44.57 | 45.00 | 43.36 | 41/92 |
| `computation/decimal_math.rs` | 48.15 | 100.00 | 40.18 | 26/54 |
| `computation/bigint/signed.rs` | 60.82 | 57.14 | 57.08 | 177/291 |
| `api/show.rs` | 61.67 | 88.89 | 61.80 | 74/120 |
| `computation/comparison.rs` | 68.64 | 85.71 | 66.67 | 116/169 |
| `mcp/tools.rs` | 70.95 | 60.00 | 68.59 | 232/327 |
| `evaluation/explanations.rs` | 74.89 | 80.00 | 74.03 | 173/231 |
| `api/value.rs` | 75.00 | 83.33 | 71.43 | 42/56 |
| `computation/measure_math.rs` | 75.36 | 100.00 | 83.18 | 52/69 |
| `computation/arithmetic.rs` | 76.78 | 76.92 | 74.76 | 787/1025 |
| `result_value.rs` | 76.99 | 64.52 | 72.35 | 271/352 |
| `documentation/mod.rs` | 77.22 | 76.92 | 78.63 | 61/79 |
| `error.rs` | 77.27 | 78.18 | 70.81 | 442/572 |
| `snapshot.rs` | 78.12 | 76.47 | 79.28 | 150/192 |
| `evaluation/run_data.rs` | 78.42 | 74.19 | 82.19 | 338/431 |
| `planning/execution_plan.rs` | 80.64 | 74.67 | 77.22 | 2028/2515 |
| `planning/graph.rs` | 80.95 | 87.53 | 81.75 | 7646/9445 |
| `evaluation/expression.rs` | 81.54 | 100.00 | 86.02 | 53/65 |
| `computation/units.rs` | 81.56 | 89.47 | 79.34 | 199/244 |
| `planning/semantics.rs` | 81.57 | 82.74 | 82.13 | 3752/4600 |
| `computation/range.rs` | 81.62 | 93.75 | 80.06 | 191/234 |
| `planning/typing.rs` | 82.12 | 100.00 | 78.77 | 317/386 |
| `parsing/ast.rs` | 82.36 | 88.51 | 74.39 | 1097/1332 |
| `computation/bigint/biguint.rs` | 83.53 | 89.80 | 81.61 | 431/516 |
| `literals.rs` | 83.58 | 77.98 | 84.17 | 611/731 |
| `parsing/parser.rs` | 85.02 | 88.82 | 81.06 | 1929/2269 |
| `computation/datetime.rs` | 85.42 | 80.43 | 83.81 | 967/1132 |
| `computation/rational.rs` | 85.56 | 97.00 | 79.25 | 770/900 |
| `planning/normalize/rewrite.rs` | 87.55 | 88.89 | 85.59 | 1013/1157 |
| `spec_set_id.rs` | 87.72 | 100.00 | 95.52 | 50/57 |
| `api/types.rs` | 87.82 | 95.00 | 88.29 | 209/238 |
| `planning/ordered_dispatch.rs` | 88.03 | 90.24 | 87.65 | 412/468 |
| `parsing/lexer.rs` | 88.35 | 100.00 | 84.97 | 637/721 |
| `planning/normalize.rs` | 89.26 | 88.20 | 88.29 | 2087/2338 |
| `string_distance.rs` | 90.48 | 83.33 | 93.22 | 57/63 |
| `evaluation/conversion_trace.rs` | 90.53 | 100.00 | 91.32 | 153/169 |
| `evaluation/narration.rs` | 90.77 | 87.50 | 91.95 | 462/509 |
| `planning/unit_index.rs` | 91.26 | 96.15 | 92.27 | 501/549 |
| `formatting/mod.rs` | 92.34 | 100.00 | 91.32 | 904/979 |
| `planning/show_expression.rs` | 92.37 | 87.50 | 93.45 | 109/118 |
| `registry.rs` | 92.45 | 97.44 | 94.47 | 968/1047 |
| `parsing/mod.rs` | 92.64 | 98.92 | 89.89 | 1321/1426 |
| `quality.rs` | 93.12 | 96.77 | 89.24 | 907/974 |
| `limits.rs` | 93.27 | 100.00 | 85.40 | 97/104 |
| `engine.rs` | 93.32 | 91.89 | 94.05 | 2138/2291 |
| `planning/discovery.rs` | 93.34 | 97.06 | 94.16 | 1233/1321 |
| `planning/mod.rs` | 93.66 | 96.15 | 94.61 | 1358/1450 |
| `evaluation/mod.rs` | 94.08 | 86.96 | 94.13 | 286/304 |
| `planning/explanation.rs` | 95.65 | 100.00 | 89.47 | 22/23 |
| `evaluation/tree.rs` | 96.12 | 100.00 | 94.43 | 396/412 |
| `deps.rs` | 96.23 | 100.00 | 96.47 | 51/53 |
| `planning/spec_set.rs` | 96.50 | 95.24 | 98.23 | 138/143 |
| `evaluation/branch_semantics.rs` | 96.77 | 100.00 | 90.18 | 90/93 |
| `planning/unit_family.rs` | 96.98 | 100.00 | 96.44 | 353/364 |
| `parsing/assignment_continuation_tests.rs` | 97.39 | 100.00 | 93.26 | 261/268 |
| `evaluation/response.rs` | 98.83 | 94.44 | 98.34 | 592/599 |
| `api/response.rs` | 100.00 | 100.00 | 100.00 | 22/22 |
| `computation/bigint/alloc.rs` | 100.00 | 100.00 | 97.37 | 20/20 |
| `parsing/source.rs` | 100.00 | 100.00 | 99.46 | 125/125 |

## Related docs

- [Engine integration test catalog](../../../engine/tests/README.md) — qualitative map of scenarios and subsystem overlap clusters
- [CLI test coverage](cli.md)
- [Engine benchmarks](../benchmarks/engine.md)
<!-- coverage-input-digest: 6cf18dcdeb4f121d -->

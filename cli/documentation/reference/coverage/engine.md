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
| Lines | 40344 | 47583 | 84.79% |
| Functions | 3115 | 3586 | 86.87% |
| Regions | 58575 | 70061 | 83.61% |

## Test run

- Total: 2715
- Passed: 2715
- Skipped: 0
- Failed: 0

## Per-module coverage

Sorted by line coverage ascending (weakest first). Only files under `src/` for this crate are listed.

| Module | Line % | Function % | Region % | Lines covered/total |
|--------|-------:|-----------:|---------:|--------------------:|
| `mcp/catalog.rs` | 0.00 | 0.00 | 0.00 | 0/171 |
| `lib.rs` | 7.27 | 10.00 | 7.95 | 4/55 |
| `mcp/error.rs` | 40.91 | 40.00 | 47.22 | 9/22 |
| `computation/operation_result.rs` | 48.00 | 50.00 | 47.65 | 48/100 |
| `computation/decimal_math.rs` | 48.15 | 100.00 | 40.18 | 26/54 |
| `computation/bigint/signed.rs` | 60.82 | 57.14 | 57.08 | 177/291 |
| `api/show.rs` | 64.03 | 81.82 | 63.86 | 89/139 |
| `computation/comparison.rs` | 70.88 | 85.71 | 67.29 | 129/182 |
| `mcp/tools.rs` | 70.95 | 60.00 | 68.59 | 232/327 |
| `computation/arithmetic.rs` | 73.79 | 69.23 | 69.21 | 760/1030 |
| `computation/measure_math.rs` | 75.36 | 100.00 | 82.69 | 52/69 |
| `result_value.rs` | 76.09 | 64.29 | 70.28 | 261/343 |
| `api/value.rs` | 76.27 | 83.33 | 72.06 | 45/59 |
| `evaluation/explanations.rs` | 76.36 | 80.95 | 74.37 | 210/275 |
| `documentation/mod.rs` | 77.22 | 76.92 | 78.63 | 61/79 |
| `error.rs` | 77.27 | 78.18 | 70.81 | 442/572 |
| `snapshot.rs` | 79.21 | 77.78 | 81.19 | 160/202 |
| `computation/units.rs` | 80.18 | 89.47 | 77.81 | 182/227 |
| `planning/graph.rs` | 80.86 | 87.34 | 81.67 | 7649/9460 |
| `planning/execution_plan.rs` | 80.97 | 75.00 | 77.48 | 2076/2564 |
| `evaluation/run_data.rs` | 81.26 | 77.14 | 82.66 | 425/523 |
| `planning/semantics.rs` | 81.47 | 82.15 | 82.16 | 3847/4722 |
| `evaluation/expression.rs` | 81.54 | 100.00 | 86.02 | 53/65 |
| `planning/typing.rs` | 82.12 | 100.00 | 78.77 | 317/386 |
| `computation/range.rs` | 82.16 | 93.75 | 80.35 | 198/241 |
| `parsing/ast.rs` | 82.36 | 88.51 | 74.39 | 1097/1332 |
| `computation/bigint/biguint.rs` | 83.53 | 89.80 | 81.61 | 431/516 |
| `literals.rs` | 83.59 | 78.50 | 84.33 | 606/725 |
| `parsing/parser.rs` | 84.19 | 87.42 | 80.47 | 1911/2270 |
| `computation/rational.rs` | 84.89 | 96.00 | 78.68 | 764/900 |
| `computation/datetime.rs` | 85.42 | 80.43 | 83.82 | 967/1132 |
| `planning/normalize/rewrite.rs` | 87.55 | 88.89 | 85.58 | 1013/1157 |
| `spec_set_id.rs` | 87.72 | 100.00 | 95.52 | 50/57 |
| `api/types.rs` | 87.76 | 95.00 | 88.29 | 215/245 |
| `planning/ordered_dispatch.rs` | 88.19 | 90.24 | 87.65 | 418/474 |
| `parsing/lexer.rs` | 88.35 | 100.00 | 84.97 | 637/721 |
| `planning/normalize.rs` | 89.26 | 88.20 | 88.29 | 2087/2338 |
| `string_distance.rs` | 90.48 | 83.33 | 93.22 | 57/63 |
| `evaluation/conversion_trace.rs` | 90.53 | 100.00 | 91.32 | 153/169 |
| `planning/unit_index.rs` | 90.71 | 94.23 | 91.66 | 498/549 |
| `evaluation/narration.rs` | 91.01 | 87.88 | 92.12 | 476/523 |
| `formatting/mod.rs` | 92.33 | 100.00 | 91.29 | 903/978 |
| `registry.rs` | 92.45 | 97.44 | 94.47 | 968/1047 |
| `parsing/mod.rs` | 92.69 | 98.96 | 89.72 | 1382/1491 |
| `quality.rs` | 93.12 | 96.77 | 89.24 | 907/974 |
| `limits.rs` | 93.27 | 100.00 | 85.40 | 97/104 |
| `planning/discovery.rs` | 93.34 | 97.06 | 94.16 | 1233/1321 |
| `engine.rs` | 93.48 | 92.43 | 94.25 | 2165/2316 |
| `planning/mod.rs` | 93.66 | 96.15 | 94.61 | 1358/1450 |
| `evaluation/mod.rs` | 94.46 | 86.36 | 94.56 | 307/325 |
| `evaluation/tree.rs` | 96.14 | 100.00 | 94.43 | 399/415 |
| `deps.rs` | 96.23 | 100.00 | 96.47 | 51/53 |
| `planning/spec_set.rs` | 96.50 | 95.24 | 98.23 | 138/143 |
| `planning/unit_family.rs` | 96.62 | 96.77 | 95.76 | 372/385 |
| `evaluation/branch_semantics.rs` | 96.77 | 100.00 | 90.18 | 90/93 |
| `planning/show_expression.rs` | 97.14 | 100.00 | 96.18 | 102/105 |
| `parsing/assignment_continuation_tests.rs` | 97.39 | 100.00 | 93.26 | 261/268 |
| `evaluation/response.rs` | 98.84 | 94.59 | 98.36 | 596/603 |
| `api/response.rs` | 100.00 | 100.00 | 100.00 | 22/22 |
| `computation/bigint/alloc.rs` | 100.00 | 100.00 | 97.37 | 20/20 |
| `parsing/source.rs` | 100.00 | 100.00 | 99.46 | 125/125 |
| `planning/explanation.rs` | 100.00 | 100.00 | 100.00 | 16/16 |

## Related docs

- [Engine integration test catalog](../../../engine/tests/README.md) — qualitative map of scenarios and subsystem overlap clusters
- [CLI test coverage](cli.md)
- [Engine benchmarks](../benchmarks/engine.md)
<!-- coverage-input-digest: e9f49d11ba4ccfea -->

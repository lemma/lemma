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
| Lines | 40543 | 47762 | 84.89% |
| Functions | 3122 | 3592 | 86.92% |
| Regions | 58887 | 70324 | 83.74% |

## Test run

- Total: 2734
- Passed: 2734
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
| `computation/comparison.rs` | 70.56 | 85.71 | 67.29 | 127/180 |
| `mcp/tools.rs` | 70.95 | 60.00 | 68.59 | 232/327 |
| `computation/arithmetic.rs` | 73.40 | 72.73 | 68.80 | 734/1000 |
| `computation/measure_math.rs` | 75.36 | 100.00 | 82.69 | 52/69 |
| `api/value.rs` | 76.67 | 83.33 | 72.86 | 46/60 |
| `documentation/mod.rs` | 77.22 | 76.92 | 78.63 | 61/79 |
| `error.rs` | 77.27 | 78.18 | 70.81 | 442/572 |
| `result_value.rs` | 77.69 | 68.75 | 73.32 | 303/390 |
| `snapshot.rs` | 79.21 | 77.78 | 81.19 | 160/202 |
| `computation/units.rs` | 79.62 | 100.00 | 77.61 | 168/211 |
| `evaluation/run_data.rs` | 80.72 | 75.00 | 81.93 | 427/529 |
| `planning/graph.rs` | 80.72 | 86.15 | 81.58 | 7699/9538 |
| `planning/execution_plan.rs` | 80.85 | 75.00 | 77.37 | 2069/2559 |
| `evaluation/expression.rs` | 81.54 | 100.00 | 86.02 | 53/65 |
| `planning/semantics.rs` | 81.78 | 82.13 | 82.26 | 3937/4814 |
| `computation/range.rs` | 82.23 | 93.75 | 80.41 | 199/242 |
| `parsing/ast.rs` | 82.36 | 88.51 | 74.39 | 1097/1332 |
| `planning/typing.rs` | 82.57 | 100.00 | 78.74 | 308/373 |
| `computation/bigint/biguint.rs` | 83.53 | 89.80 | 81.61 | 431/516 |
| `literals.rs` | 83.59 | 78.50 | 84.33 | 606/725 |
| `parsing/parser.rs` | 84.19 | 87.42 | 80.47 | 1911/2270 |
| `computation/rational.rs` | 84.89 | 96.00 | 78.68 | 764/900 |
| `computation/datetime.rs` | 85.53 | 80.43 | 83.87 | 975/1140 |
| `spec_set_id.rs` | 87.72 | 100.00 | 95.52 | 50/57 |
| `api/types.rs` | 87.90 | 95.00 | 88.45 | 218/248 |
| `planning/ordered_dispatch.rs` | 88.19 | 90.24 | 87.65 | 418/474 |
| `parsing/lexer.rs` | 88.35 | 100.00 | 84.97 | 637/721 |
| `planning/normalize/rewrite.rs` | 88.58 | 89.02 | 87.25 | 1047/1182 |
| `planning/normalize.rs` | 89.26 | 88.20 | 88.29 | 2087/2338 |
| `evaluation/explanations.rs` | 89.45 | 100.00 | 89.09 | 246/275 |
| `string_distance.rs` | 90.48 | 83.33 | 93.22 | 57/63 |
| `planning/unit_index.rs` | 90.74 | 94.23 | 91.68 | 500/551 |
| `evaluation/narration.rs` | 90.94 | 87.88 | 92.06 | 472/519 |
| `formatting/mod.rs` | 92.33 | 100.00 | 91.29 | 903/978 |
| `registry.rs` | 92.45 | 97.44 | 94.47 | 968/1047 |
| `parsing/mod.rs` | 92.69 | 98.96 | 89.72 | 1382/1491 |
| `evaluation/conversion_trace.rs` | 92.86 | 100.00 | 93.26 | 130/140 |
| `quality.rs` | 93.12 | 96.77 | 89.24 | 907/974 |
| `limits.rs` | 93.27 | 100.00 | 85.40 | 97/104 |
| `planning/discovery.rs` | 93.34 | 97.06 | 94.16 | 1233/1321 |
| `engine.rs` | 93.48 | 92.43 | 94.25 | 2165/2316 |
| `planning/mod.rs` | 93.66 | 96.15 | 94.61 | 1358/1450 |
| `evaluation/mod.rs` | 94.46 | 86.36 | 94.56 | 307/325 |
| `deps.rs` | 96.23 | 100.00 | 96.47 | 51/53 |
| `evaluation/tree.rs` | 96.23 | 100.00 | 94.54 | 408/424 |
| `planning/spec_set.rs` | 96.50 | 95.24 | 98.23 | 138/143 |
| `planning/unit_family.rs` | 96.65 | 96.77 | 95.78 | 375/388 |
| `evaluation/branch_semantics.rs` | 96.77 | 100.00 | 90.18 | 90/93 |
| `planning/show_expression.rs` | 97.14 | 100.00 | 96.18 | 102/105 |
| `parsing/assignment_continuation_tests.rs` | 97.39 | 100.00 | 93.26 | 261/268 |
| `evaluation/response.rs` | 98.84 | 94.59 | 98.37 | 599/606 |
| `api/response.rs` | 100.00 | 100.00 | 100.00 | 22/22 |
| `computation/bigint/alloc.rs` | 100.00 | 100.00 | 97.37 | 20/20 |
| `parsing/source.rs` | 100.00 | 100.00 | 99.46 | 125/125 |
| `planning/explanation.rs` | 100.00 | 100.00 | 100.00 | 16/16 |

## Related docs

- [Engine integration test catalog](../../../engine/tests/README.md) — qualitative map of scenarios and subsystem overlap clusters
- [CLI test coverage](cli.md)
- [Engine benchmarks](../benchmarks/engine.md)
<!-- coverage-input-digest: 5a3646aa9e4b47f4 -->

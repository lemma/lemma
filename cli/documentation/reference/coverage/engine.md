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
| Lines | 42073 | 49544 | 84.92% |
| Functions | 3204 | 3688 | 86.88% |
| Regions | 61207 | 72965 | 83.89% |

## Test run

- Total: 2794
- Passed: 2794
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
| `api/show.rs` | 60.54 | 81.82 | 60.56 | 89/147 |
| `computation/bigint/signed.rs` | 60.82 | 57.14 | 57.08 | 177/291 |
| `computation/comparison.rs` | 70.56 | 85.71 | 67.29 | 127/180 |
| `mcp/tools.rs` | 70.95 | 60.00 | 68.59 | 232/327 |
| `computation/arithmetic.rs` | 73.40 | 72.73 | 68.80 | 734/1000 |
| `result_value.rs` | 73.84 | 62.86 | 70.70 | 319/432 |
| `computation/measure_math.rs` | 75.36 | 100.00 | 82.69 | 52/69 |
| `api/value.rs` | 75.81 | 83.33 | 72.60 | 47/62 |
| `planning/spec_value.rs` | 76.67 | 92.31 | 76.80 | 253/330 |
| `documentation/mod.rs` | 77.22 | 76.92 | 78.63 | 61/79 |
| `error.rs` | 77.27 | 78.18 | 70.81 | 442/572 |
| `snapshot.rs` | 79.21 | 77.78 | 81.19 | 160/202 |
| `computation/units.rs` | 79.62 | 100.00 | 77.61 | 168/211 |
| `evaluation/run_data.rs` | 80.72 | 75.00 | 81.93 | 427/529 |
| `planning/execution_plan.rs` | 80.89 | 75.33 | 77.51 | 2079/2570 |
| `planning/graph.rs` | 81.24 | 86.90 | 82.18 | 8086/9953 |
| `evaluation/expression.rs` | 81.54 | 100.00 | 86.02 | 53/65 |
| `planning/semantics.rs` | 81.89 | 82.25 | 82.45 | 4020/4909 |
| `computation/range.rs` | 82.47 | 94.12 | 80.34 | 207/251 |
| `computation/bigint/biguint.rs` | 83.53 | 89.80 | 81.61 | 431/516 |
| `literals.rs` | 83.59 | 78.50 | 84.33 | 606/725 |
| `planning/typing.rs` | 83.61 | 100.00 | 79.88 | 347/415 |
| `parsing/ast.rs` | 83.73 | 88.59 | 77.43 | 1122/1340 |
| `parsing/parser.rs` | 84.28 | 87.01 | 80.54 | 1935/2296 |
| `computation/rational.rs` | 84.89 | 96.00 | 78.68 | 764/900 |
| `computation/datetime.rs` | 85.53 | 80.43 | 83.87 | 975/1140 |
| `api/types.rs` | 86.17 | 95.00 | 86.73 | 218/253 |
| `spec_set_id.rs` | 87.72 | 100.00 | 95.52 | 50/57 |
| `planning/normalize.rs` | 87.98 | 85.21 | 87.30 | 2152/2446 |
| `planning/ordered_dispatch.rs` | 88.19 | 90.24 | 87.65 | 418/474 |
| `parsing/lexer.rs` | 88.22 | 100.00 | 84.80 | 644/730 |
| `planning/normalize/rewrite.rs` | 88.61 | 89.02 | 87.23 | 1050/1185 |
| `evaluation/explanations.rs` | 89.45 | 100.00 | 89.09 | 246/275 |
| `string_distance.rs` | 90.48 | 83.33 | 93.22 | 57/63 |
| `evaluation/narration.rs` | 90.56 | 85.71 | 91.30 | 489/540 |
| `planning/unit_index.rs` | 91.29 | 96.15 | 92.17 | 503/551 |
| `parsing/mod.rs` | 92.36 | 99.03 | 89.04 | 1511/1636 |
| `registry.rs` | 92.45 | 97.44 | 94.47 | 968/1047 |
| `formatting/mod.rs` | 92.54 | 100.00 | 91.72 | 905/978 |
| `evaluation/conversion_trace.rs` | 92.86 | 100.00 | 93.26 | 130/140 |
| `planning/discovery.rs` | 92.96 | 97.53 | 94.08 | 1426/1534 |
| `quality.rs` | 93.03 | 96.77 | 89.15 | 907/975 |
| `limits.rs` | 93.27 | 100.00 | 85.40 | 97/104 |
| `engine.rs` | 93.60 | 92.47 | 94.37 | 2208/2359 |
| `planning/mod.rs` | 93.66 | 96.15 | 94.61 | 1358/1450 |
| `evaluation/mod.rs` | 94.56 | 86.36 | 94.48 | 313/331 |
| `evaluation/tree.rs` | 95.34 | 100.00 | 93.54 | 450/472 |
| `deps.rs` | 96.23 | 100.00 | 96.47 | 51/53 |
| `planning/spec_set.rs` | 96.50 | 95.24 | 98.23 | 138/143 |
| `planning/unit_family.rs` | 96.65 | 96.77 | 95.78 | 375/388 |
| `planning/show_expression.rs` | 96.69 | 100.00 | 95.70 | 117/121 |
| `evaluation/branch_semantics.rs` | 96.77 | 100.00 | 90.18 | 90/93 |
| `evaluation/response.rs` | 96.93 | 87.76 | 95.43 | 758/782 |
| `parsing/assignment_continuation_tests.rs` | 97.39 | 100.00 | 93.26 | 261/268 |
| `api/response.rs` | 100.00 | 100.00 | 100.00 | 22/22 |
| `computation/bigint/alloc.rs` | 100.00 | 100.00 | 97.37 | 20/20 |
| `parsing/source.rs` | 100.00 | 100.00 | 99.46 | 125/125 |
| `planning/explanation.rs` | 100.00 | 100.00 | 100.00 | 16/16 |

## Related docs

- [Engine integration test catalog](../../../engine/tests/README.md) — qualitative map of scenarios and subsystem overlap clusters
- [CLI test coverage](cli.md)
- [Engine benchmarks](../benchmarks/engine.md)
<!-- coverage-input-digest: b0fff720904a127b -->

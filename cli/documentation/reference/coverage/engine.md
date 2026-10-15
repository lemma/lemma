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
| Lines | 57094 | 65649 | 86.97% |
| Functions | 4405 | 4950 | 88.99% |
| Regions | 83814 | 97385 | 86.06% |

## Test run

- Total: 3497
- Passed: 3497
- Skipped: 0
- Failed: 0

## Per-module coverage

Sorted by line coverage ascending (weakest first). Only files under `src/` for this crate are listed.

| Module | Line % | Function % | Region % | Lines covered/total |
|--------|-------:|-----------:|---------:|--------------------:|
| `mcp/catalog.rs` | 0.00 | 0.00 | 0.00 | 0/179 |
| `lib.rs` | 6.45 | 9.09 | 6.86 | 4/62 |
| `mcp/error.rs` | 40.91 | 40.00 | 47.22 | 9/22 |
| `computation/operation_result.rs` | 56.69 | 56.00 | 56.41 | 72/127 |
| `computation/comparison.rs` | 72.63 | 87.50 | 67.50 | 130/179 |
| `computation/arithmetic.rs` | 73.25 | 66.67 | 70.37 | 671/916 |
| `api/value.rs` | 74.29 | 75.00 | 79.64 | 104/140 |
| `documentation/mod.rs` | 77.22 | 76.92 | 78.63 | 61/79 |
| `parsing/arena.rs` | 77.45 | 76.19 | 74.06 | 158/204 |
| `error.rs` | 78.46 | 82.14 | 72.89 | 459/585 |
| `computation/bigint/signed.rs` | 78.77 | 75.56 | 76.66 | 256/325 |
| `computation/units.rs` | 79.81 | 100.00 | 77.58 | 166/208 |
| `mcp/tools.rs` | 80.13 | 71.43 | 78.92 | 383/478 |
| `planning/graph.rs` | 80.80 | 87.77 | 81.27 | 10391/12860 |
| `evaluation/expression.rs` | 81.82 | 100.00 | 86.87 | 54/66 |
| `computation/measure_math.rs` | 82.86 | 100.00 | 84.44 | 58/70 |
| `planning/semantics.rs` | 83.07 | 84.03 | 82.83 | 4481/5394 |
| `evaluation/rows.rs` | 83.51 | 100.00 | 82.50 | 81/97 |
| `result_value.rs` | 84.46 | 80.56 | 80.73 | 413/489 |
| `planning/typing.rs` | 85.07 | 100.00 | 81.55 | 376/442 |
| `literals.rs` | 85.21 | 80.87 | 85.88 | 651/764 |
| `computation/datetime.rs` | 85.81 | 82.42 | 84.35 | 943/1099 |
| `planning/execution_plan.rs` | 85.82 | 77.37 | 83.60 | 3516/4097 |
| `parsing/lexer.rs` | 86.06 | 100.00 | 79.28 | 710/825 |
| `planning/normalize.rs` | 86.97 | 89.30 | 86.81 | 2557/2940 |
| `evaluation/mod.rs` | 87.04 | 78.57 | 88.67 | 571/656 |
| `planning/spec_value.rs` | 87.30 | 82.35 | 85.50 | 268/307 |
| `planning/instance_list.rs` | 87.36 | 88.57 | 87.53 | 228/261 |
| `spec_set_id.rs` | 87.72 | 100.00 | 95.52 | 50/57 |
| `parsing/parser.rs` | 87.79 | 89.77 | 82.45 | 2963/3375 |
| `parsing/ast.rs` | 88.03 | 89.94 | 84.25 | 1221/1387 |
| `computation/range.rs` | 88.10 | 94.12 | 83.33 | 185/210 |
| `snapshot.rs` | 88.17 | 92.59 | 87.34 | 246/279 |
| `planning/ordered_dispatch.rs` | 88.24 | 90.24 | 87.69 | 420/476 |
| `api/show.rs` | 88.27 | 87.01 | 89.21 | 888/1006 |
| `formatting/expression.rs` | 89.09 | 98.08 | 90.62 | 580/651 |
| `api/types.rs` | 90.00 | 96.43 | 91.61 | 288/320 |
| `evaluation/list_ops.rs` | 90.45 | 97.14 | 87.48 | 360/398 |
| `string_distance.rs` | 90.48 | 83.33 | 93.22 | 57/63 |
| `api/evaluate_data_json.rs` | 90.99 | 100.00 | 93.14 | 202/222 |
| `evaluation/explanations.rs` | 91.06 | 96.55 | 88.41 | 479/526 |
| `planning/unit_index.rs` | 91.12 | 97.20 | 91.17 | 913/1002 |
| `snapshot/shared.rs` | 91.44 | 88.89 | 91.38 | 235/257 |
| `planning/normalize/rewrite.rs` | 91.60 | 96.88 | 91.16 | 1320/1441 |
| `planning/show_expression.rs` | 92.27 | 100.00 | 93.72 | 406/440 |
| `evaluation/narration.rs` | 92.50 | 81.71 | 93.06 | 1160/1254 |
| `registry.rs` | 92.76 | 97.53 | 94.50 | 999/1077 |
| `computation/rational.rs` | 92.84 | 98.64 | 89.71 | 1257/1354 |
| `planning/mod.rs` | 92.98 | 96.59 | 94.46 | 1165/1253 |
| `parsing/mod.rs` | 93.11 | 99.15 | 89.49 | 1757/1887 |
| `limits.rs` | 93.27 | 100.00 | 83.45 | 97/104 |
| `engine.rs` | 93.32 | 92.66 | 93.38 | 2612/2799 |
| `computation/bigint/biguint.rs` | 93.54 | 97.44 | 91.39 | 738/789 |
| `planning/discovery.rs` | 93.73 | 97.75 | 94.72 | 1495/1595 |
| `quality.rs` | 93.95 | 97.80 | 90.59 | 885/942 |
| `formatting/mod.rs` | 94.09 | 100.00 | 94.05 | 972/1033 |
| `evaluation/imports.rs` | 94.59 | 90.00 | 93.46 | 175/185 |
| `evaluation/tree.rs` | 95.57 | 93.65 | 94.36 | 1552/1624 |
| `evaluation/response.rs` | 95.89 | 92.00 | 94.70 | 1051/1096 |
| `evaluation/conversion_trace.rs` | 95.95 | 100.00 | 96.96 | 237/247 |
| `planning/unit_family.rs` | 96.21 | 100.00 | 95.63 | 279/290 |
| `deps.rs` | 96.23 | 100.00 | 96.47 | 51/53 |
| `computation/decimal_math.rs` | 96.30 | 100.00 | 96.39 | 208/216 |
| `planning/spec_set.rs` | 96.50 | 95.24 | 98.23 | 138/143 |
| `evaluation/branch_semantics.rs` | 96.77 | 100.00 | 90.18 | 90/93 |
| `evaluation/evaluate_data.rs` | 97.17 | 97.20 | 95.97 | 1409/1450 |
| `parsing/source.rs` | 99.20 | 100.00 | 97.30 | 124/125 |
| `api/response.rs` | 100.00 | 100.00 | 100.00 | 23/23 |
| `computation/bigint/alloc.rs` | 100.00 | 100.00 | 97.37 | 20/20 |
| `planning/explanation.rs` | 100.00 | 100.00 | 100.00 | 16/16 |

## Related docs

- [Engine integration test catalog](../../../engine/tests/README.md) — qualitative map of scenarios and subsystem overlap clusters
- [CLI test coverage](cli.md)
- [Engine benchmarks](../benchmarks/engine.md)
<!-- coverage-input-digest: 09b45818179dc813 -->

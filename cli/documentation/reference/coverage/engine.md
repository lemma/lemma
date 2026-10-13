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
| Lines | 56730 | 65336 | 86.83% |
| Functions | 4380 | 4927 | 88.90% |
| Regions | 83248 | 96896 | 85.91% |

## Test run

- Total: 3374
- Passed: 3374
- Skipped: 0
- Failed: 0

## Per-module coverage

Sorted by line coverage ascending (weakest first). Only files under `src/` for this crate are listed.

| Module | Line % | Function % | Region % | Lines covered/total |
|--------|-------:|-----------:|---------:|--------------------:|
| `mcp/catalog.rs` | 0.00 | 0.00 | 0.00 | 0/179 |
| `lib.rs` | 6.45 | 9.09 | 6.86 | 4/62 |
| `mcp/error.rs` | 40.91 | 40.00 | 47.22 | 9/22 |
| `computation/operation_result.rs` | 49.02 | 50.00 | 49.02 | 50/102 |
| `computation/comparison.rs` | 72.63 | 87.50 | 67.50 | 130/179 |
| `computation/arithmetic.rs` | 73.57 | 66.67 | 70.37 | 671/912 |
| `api/value.rs` | 74.29 | 75.00 | 79.64 | 104/140 |
| `documentation/mod.rs` | 77.22 | 76.92 | 78.63 | 61/79 |
| `parsing/arena.rs` | 77.45 | 76.19 | 74.06 | 158/204 |
| `error.rs` | 78.46 | 82.14 | 72.89 | 459/585 |
| `computation/bigint/signed.rs` | 78.77 | 75.56 | 76.66 | 256/325 |
| `computation/units.rs` | 80.00 | 100.00 | 78.33 | 160/200 |
| `mcp/tools.rs` | 80.13 | 71.43 | 78.92 | 383/478 |
| `planning/graph.rs` | 80.76 | 87.67 | 81.18 | 10329/12789 |
| `evaluation/expression.rs` | 81.82 | 100.00 | 86.87 | 54/66 |
| `planning/semantics.rs` | 82.19 | 83.06 | 82.11 | 4415/5372 |
| `computation/measure_math.rs` | 82.86 | 100.00 | 84.44 | 58/70 |
| `evaluation/rows.rs` | 83.51 | 100.00 | 82.50 | 81/97 |
| `result_value.rs` | 84.46 | 80.56 | 80.73 | 413/489 |
| `planning/typing.rs` | 85.00 | 100.00 | 81.39 | 374/440 |
| `planning/execution_plan.rs` | 85.14 | 76.64 | 82.87 | 3489/4098 |
| `literals.rs` | 85.21 | 80.87 | 85.88 | 651/764 |
| `computation/datetime.rs` | 86.04 | 82.42 | 84.45 | 943/1096 |
| `parsing/lexer.rs` | 86.06 | 100.00 | 79.28 | 710/825 |
| `planning/normalize.rs` | 86.94 | 89.30 | 86.73 | 2556/2940 |
| `planning/spec_value.rs` | 87.30 | 82.35 | 85.50 | 268/307 |
| `planning/instance_list.rs` | 87.36 | 88.57 | 87.53 | 228/261 |
| `evaluation/mod.rs` | 87.44 | 78.05 | 88.78 | 571/653 |
| `spec_set_id.rs` | 87.72 | 100.00 | 95.52 | 50/57 |
| `parsing/parser.rs` | 87.79 | 89.77 | 82.45 | 2963/3375 |
| `computation/range.rs` | 87.80 | 94.12 | 82.76 | 180/205 |
| `parsing/ast.rs` | 87.89 | 89.94 | 84.25 | 1219/1387 |
| `snapshot.rs` | 88.17 | 92.59 | 87.34 | 246/279 |
| `planning/ordered_dispatch.rs` | 88.24 | 90.24 | 87.69 | 420/476 |
| `api/show.rs` | 88.33 | 87.18 | 89.20 | 893/1011 |
| `formatting/expression.rs` | 89.09 | 98.08 | 90.62 | 580/651 |
| `evaluation/explanations.rs` | 89.35 | 96.55 | 86.84 | 470/526 |
| `api/types.rs` | 90.00 | 96.43 | 91.61 | 288/320 |
| `string_distance.rs` | 90.48 | 83.33 | 93.22 | 57/63 |
| `evaluation/list_ops.rs` | 90.55 | 97.22 | 87.66 | 364/402 |
| `planning/unit_index.rs` | 90.92 | 97.20 | 90.97 | 911/1002 |
| `api/evaluate_data_json.rs` | 90.99 | 100.00 | 93.14 | 202/222 |
| `snapshot/shared.rs` | 91.44 | 88.89 | 91.38 | 235/257 |
| `planning/normalize/rewrite.rs` | 91.53 | 96.88 | 91.12 | 1319/1441 |
| `planning/show_expression.rs` | 92.27 | 100.00 | 93.72 | 406/440 |
| `registry.rs` | 92.76 | 97.53 | 94.50 | 999/1077 |
| `computation/rational.rs` | 92.90 | 98.64 | 89.71 | 1257/1353 |
| `planning/mod.rs` | 92.98 | 96.59 | 94.46 | 1165/1253 |
| `parsing/mod.rs` | 93.11 | 99.15 | 89.49 | 1757/1887 |
| `engine.rs` | 93.24 | 92.63 | 93.33 | 2594/2782 |
| `limits.rs` | 93.27 | 100.00 | 83.45 | 97/104 |
| `evaluation/narration.rs` | 93.31 | 85.07 | 93.57 | 976/1046 |
| `computation/bigint/biguint.rs` | 93.54 | 97.44 | 91.39 | 738/789 |
| `planning/discovery.rs` | 93.82 | 97.80 | 94.78 | 1519/1619 |
| `quality.rs` | 93.95 | 97.80 | 90.59 | 885/942 |
| `formatting/mod.rs` | 94.09 | 100.00 | 94.05 | 972/1033 |
| `evaluation/imports.rs` | 94.59 | 90.00 | 93.46 | 175/185 |
| `evaluation/conversion_trace.rs` | 95.55 | 100.00 | 96.71 | 236/247 |
| `evaluation/tree.rs` | 95.62 | 93.65 | 94.52 | 1549/1620 |
| `evaluation/response.rs` | 95.89 | 92.00 | 94.70 | 1050/1095 |
| `planning/unit_family.rs` | 96.21 | 100.00 | 95.63 | 279/290 |
| `deps.rs` | 96.23 | 100.00 | 96.47 | 51/53 |
| `computation/decimal_math.rs` | 96.30 | 100.00 | 96.39 | 208/216 |
| `evaluation/evaluate_data.rs` | 96.41 | 97.22 | 95.33 | 1424/1477 |
| `planning/spec_set.rs` | 96.50 | 95.24 | 98.23 | 138/143 |
| `evaluation/branch_semantics.rs` | 96.77 | 100.00 | 90.18 | 90/93 |
| `parsing/source.rs` | 99.20 | 100.00 | 97.30 | 124/125 |
| `api/response.rs` | 100.00 | 100.00 | 100.00 | 23/23 |
| `computation/bigint/alloc.rs` | 100.00 | 100.00 | 97.37 | 20/20 |
| `planning/explanation.rs` | 100.00 | 100.00 | 100.00 | 16/16 |

## Related docs

- [Engine integration test catalog](../../../engine/tests/README.md) — qualitative map of scenarios and subsystem overlap clusters
- [CLI test coverage](cli.md)
- [Engine benchmarks](../benchmarks/engine.md)
<!-- coverage-input-digest: d09c5d90511f91c6 -->

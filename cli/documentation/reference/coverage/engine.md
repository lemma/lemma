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

- Host: `Linux 7.0.0-42-generic x86_64`
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
| Lines | 66713 | 76708 | 86.97% |
| Functions | 5038 | 5658 | 89.04% |
| Regions | 98397 | 114258 | 86.12% |

## Test run

- Total: 4092
- Passed: 4092
- Skipped: 0
- Failed: 0

## Per-module coverage

Sorted by line coverage ascending (weakest first). Only files under `src/` for this crate are listed.

| Module | Line % | Function % | Region % | Lines covered/total |
|--------|-------:|-----------:|---------:|--------------------:|
| `mcp/catalog.rs` | 0.00 | 0.00 | 0.00 | 0/179 |
| `lib.rs` | 6.45 | 9.09 | 6.86 | 4/62 |
| `mcp/error.rs` | 40.91 | 40.00 | 47.22 | 9/22 |
| `api/response.rs` | 51.19 | 83.33 | 48.46 | 43/84 |
| `computation/operation_result.rs` | 56.78 | 56.52 | 57.71 | 67/118 |
| `evaluation/expression.rs` | 60.92 | 66.67 | 67.77 | 53/87 |
| `computation/comparison.rs` | 72.63 | 87.50 | 67.50 | 130/179 |
| `api/value.rs` | 73.76 | 75.00 | 79.28 | 104/141 |
| `computation/arithmetic.rs` | 75.91 | 72.55 | 72.51 | 747/984 |
| `computation/bigint/signed.rs` | 76.68 | 72.50 | 74.18 | 217/283 |
| `documentation/mod.rs` | 77.22 | 76.92 | 78.63 | 61/79 |
| `mcp/tools.rs` | 80.13 | 71.43 | 78.95 | 383/478 |
| `planning/types.rs` | 80.23 | 86.44 | 83.50 | 4372/5449 |
| `evaluation/mod.rs` | 81.26 | 75.71 | 80.65 | 789/971 |
| `planning/semantics.rs` | 81.90 | 84.11 | 81.34 | 5931/7242 |
| `computation/units.rs` | 82.05 | 100.00 | 79.05 | 192/234 |
| `planning/graph.rs` | 82.66 | 87.29 | 82.47 | 8573/10371 |
| `error.rs` | 83.74 | 83.02 | 78.51 | 443/529 |
| `literals.rs` | 84.32 | 81.54 | 84.93 | 753/893 |
| `planning/execution_plan.rs` | 85.15 | 78.52 | 82.59 | 3382/3972 |
| `api/types.rs` | 85.35 | 96.67 | 86.56 | 303/355 |
| `parsing/lexer.rs` | 85.63 | 97.62 | 78.19 | 739/863 |
| `result_value.rs` | 85.79 | 83.33 | 81.81 | 471/549 |
| `computation/datetime.rs` | 85.90 | 82.42 | 84.40 | 944/1099 |
| `evaluation/narration.rs` | 86.14 | 75.16 | 85.00 | 1989/2309 |
| `api/show.rs` | 87.69 | 84.51 | 88.85 | 819/934 |
| `spec_set_id.rs` | 87.72 | 100.00 | 95.52 | 50/57 |
| `computation/range.rs` | 88.10 | 94.12 | 83.33 | 185/210 |
| `parsing/parser.rs` | 88.55 | 98.86 | 83.58 | 3448/3894 |
| `parsing/ast.rs` | 88.68 | 91.62 | 85.39 | 1324/1493 |
| `planning/normalize.rs` | 88.82 | 90.65 | 87.83 | 2726/3069 |
| `planning/ordered_dispatch.rs` | 88.99 | 94.44 | 87.87 | 404/454 |
| `formatting/expression.rs` | 90.15 | 98.04 | 90.64 | 595/660 |
| `string_distance.rs` | 90.48 | 83.33 | 93.22 | 57/63 |
| `evaluation/tree.rs` | 90.51 | 87.93 | 89.16 | 2318/2561 |
| `planning/show_expression.rs` | 90.52 | 91.67 | 91.37 | 382/422 |
| `evaluation/list_ops.rs` | 90.75 | 100.00 | 87.89 | 353/389 |
| `api/evaluate_data_json.rs` | 90.76 | 100.00 | 92.16 | 216/238 |
| `evaluation/explanations.rs` | 91.46 | 91.11 | 90.49 | 632/691 |
| `planning/discovery.rs` | 91.48 | 97.94 | 92.76 | 1599/1748 |
| `planning/normalize/rewrite.rs` | 91.75 | 96.88 | 91.50 | 1334/1454 |
| `computation/rational.rs` | 91.81 | 98.50 | 88.75 | 1144/1246 |
| `formatting/mod.rs` | 91.90 | 100.00 | 91.28 | 1134/1234 |
| `parsing/arena.rs` | 92.03 | 88.00 | 92.27 | 254/276 |
| `planning/typing.rs` | 92.53 | 98.25 | 90.09 | 1065/1151 |
| `registry.rs` | 92.76 | 97.53 | 94.50 | 999/1077 |
| `parsing/mod.rs` | 93.01 | 99.25 | 89.25 | 1984/2133 |
| `limits.rs` | 93.27 | 100.00 | 83.45 | 97/104 |
| `engine.rs` | 93.38 | 92.41 | 93.24 | 2823/3023 |
| `planning/unit_index.rs` | 93.85 | 95.90 | 93.97 | 1267/1350 |
| `quality.rs` | 93.96 | 97.80 | 90.60 | 887/944 |
| `planning/spec_value.rs` | 93.99 | 86.67 | 94.55 | 344/366 |
| `evaluation/rows.rs` | 94.38 | 93.33 | 92.50 | 151/160 |
| `planning/mod.rs` | 94.38 | 95.90 | 95.16 | 1595/1690 |
| `evaluation/evaluate_data.rs` | 94.50 | 96.69 | 93.89 | 1651/1747 |
| `planning/spec_set.rs` | 94.80 | 92.31 | 96.57 | 164/173 |
| `planning/unit_family.rs` | 95.43 | 100.00 | 95.07 | 355/372 |
| `computation/bigint/biguint.rs` | 95.62 | 98.63 | 93.28 | 721/754 |
| `evaluation/conversion_trace.rs` | 95.86 | 100.00 | 96.87 | 255/266 |
| `computation/measure_math.rs` | 96.13 | 100.00 | 94.14 | 149/155 |
| `evaluation/response.rs` | 96.20 | 94.59 | 94.87 | 1064/1106 |
| `deps.rs` | 96.23 | 100.00 | 96.47 | 51/53 |
| `computation/decimal_math.rs` | 96.30 | 100.00 | 96.39 | 208/216 |
| `evaluation/branch_semantics.rs` | 96.77 | 100.00 | 90.18 | 90/93 |
| `planning/instance_list.rs` | 97.07 | 95.60 | 95.50 | 960/989 |
| `parsing/source.rs` | 99.20 | 100.00 | 97.30 | 124/125 |
| `computation/bigint/alloc.rs` | 100.00 | 100.00 | 97.37 | 20/20 |
| `planning/explanation.rs` | 100.00 | 100.00 | 100.00 | 16/16 |

## Related docs

- [Engine integration test catalog](../../../engine/tests/README.md) — qualitative map of scenarios and subsystem overlap clusters
- [CLI test coverage](cli.md)
- [Engine benchmarks](../benchmarks/engine.md)
<!-- coverage-input-digest: d2dd52de578fbac9 -->

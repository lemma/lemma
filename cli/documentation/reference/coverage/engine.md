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
| Lines | 65925 | 75710 | 87.08% |
| Functions | 4981 | 5578 | 89.30% |
| Regions | 97158 | 112686 | 86.22% |

## Test run

- Total: 4021
- Passed: 4021
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
| `computation/operation_result.rs` | 57.26 | 56.52 | 57.71 | 67/117 |
| `evaluation/expression.rs` | 61.63 | 66.67 | 68.33 | 53/86 |
| `computation/comparison.rs` | 72.63 | 87.50 | 67.50 | 130/179 |
| `api/value.rs` | 73.76 | 75.00 | 79.28 | 104/141 |
| `computation/arithmetic.rs` | 76.23 | 72.55 | 72.78 | 731/959 |
| `computation/bigint/signed.rs` | 76.68 | 72.50 | 74.18 | 217/283 |
| `documentation/mod.rs` | 77.22 | 76.92 | 78.63 | 61/79 |
| `mcp/tools.rs` | 80.13 | 71.43 | 78.95 | 383/478 |
| `evaluation/mod.rs` | 80.93 | 75.71 | 80.36 | 781/965 |
| `computation/units.rs` | 81.33 | 100.00 | 78.81 | 183/225 |
| `planning/graph.rs` | 81.90 | 86.95 | 82.87 | 13308/16249 |
| `planning/semantics.rs` | 82.22 | 84.14 | 81.61 | 5670/6896 |
| `error.rs` | 83.74 | 83.02 | 78.51 | 443/529 |
| `planning/execution_plan.rs` | 85.51 | 79.03 | 82.82 | 3347/3914 |
| `parsing/lexer.rs` | 85.63 | 97.62 | 78.19 | 739/863 |
| `result_value.rs` | 85.82 | 83.33 | 81.83 | 472/550 |
| `literals.rs` | 85.83 | 82.40 | 86.31 | 745/868 |
| `computation/datetime.rs` | 85.90 | 82.42 | 84.40 | 944/1099 |
| `evaluation/narration.rs` | 86.95 | 80.00 | 85.72 | 1773/2039 |
| `api/show.rs` | 87.69 | 84.51 | 88.85 | 819/934 |
| `spec_set_id.rs` | 87.72 | 100.00 | 95.52 | 50/57 |
| `computation/range.rs` | 88.10 | 94.12 | 83.33 | 185/210 |
| `parsing/parser.rs` | 88.55 | 98.86 | 83.58 | 3448/3894 |
| `api/types.rs` | 88.56 | 96.67 | 90.06 | 302/341 |
| `parsing/ast.rs` | 88.75 | 91.62 | 85.44 | 1325/1493 |
| `planning/normalize.rs` | 88.76 | 90.58 | 87.75 | 2693/3034 |
| `planning/ordered_dispatch.rs` | 88.99 | 94.44 | 87.87 | 404/454 |
| `formatting/expression.rs` | 90.15 | 98.04 | 90.64 | 595/660 |
| `evaluation/list_ops.rs` | 90.31 | 100.00 | 87.43 | 317/351 |
| `string_distance.rs` | 90.48 | 83.33 | 93.22 | 57/63 |
| `planning/show_expression.rs` | 90.52 | 91.67 | 91.37 | 382/422 |
| `api/evaluate_data_json.rs` | 90.76 | 100.00 | 92.16 | 216/238 |
| `planning/typing.rs` | 91.02 | 97.92 | 88.73 | 841/924 |
| `evaluation/explanations.rs` | 91.46 | 91.11 | 90.49 | 632/691 |
| `planning/discovery.rs` | 91.48 | 97.94 | 92.76 | 1599/1748 |
| `planning/normalize/rewrite.rs` | 91.68 | 96.88 | 91.46 | 1333/1454 |
| `computation/rational.rs` | 91.81 | 98.50 | 88.75 | 1144/1246 |
| `evaluation/tree.rs` | 91.86 | 91.35 | 90.30 | 2144/2334 |
| `parsing/arena.rs` | 92.03 | 88.00 | 92.27 | 254/276 |
| `formatting/mod.rs` | 92.18 | 100.00 | 91.88 | 1120/1215 |
| `registry.rs` | 92.76 | 97.53 | 94.50 | 999/1077 |
| `parsing/mod.rs` | 93.01 | 99.25 | 89.25 | 1984/2133 |
| `planning/unit_index.rs` | 93.16 | 95.04 | 93.43 | 1213/1302 |
| `limits.rs` | 93.27 | 100.00 | 83.45 | 97/104 |
| `engine.rs` | 93.38 | 92.41 | 93.24 | 2823/3023 |
| `quality.rs` | 93.96 | 97.80 | 90.60 | 887/944 |
| `planning/spec_value.rs` | 93.99 | 86.67 | 94.55 | 344/366 |
| `evaluation/rows.rs` | 94.34 | 93.33 | 92.46 | 150/159 |
| `planning/mod.rs` | 94.38 | 95.90 | 95.16 | 1595/1690 |
| `evaluation/evaluate_data.rs` | 94.64 | 96.75 | 94.13 | 1678/1773 |
| `planning/spec_set.rs` | 94.80 | 92.31 | 96.57 | 164/173 |
| `computation/bigint/biguint.rs` | 95.62 | 98.63 | 93.28 | 721/754 |
| `evaluation/conversion_trace.rs` | 95.86 | 100.00 | 96.87 | 255/266 |
| `computation/measure_math.rs` | 96.03 | 100.00 | 94.04 | 145/151 |
| `planning/unit_family.rs` | 96.17 | 100.00 | 95.16 | 276/287 |
| `deps.rs` | 96.23 | 100.00 | 96.47 | 51/53 |
| `computation/decimal_math.rs` | 96.30 | 100.00 | 96.39 | 208/216 |
| `evaluation/branch_semantics.rs` | 96.77 | 100.00 | 90.18 | 90/93 |
| `evaluation/response.rs` | 96.98 | 94.59 | 95.58 | 1058/1091 |
| `planning/instance_list.rs` | 97.07 | 95.60 | 95.50 | 960/989 |
| `parsing/source.rs` | 99.20 | 100.00 | 97.30 | 124/125 |
| `computation/bigint/alloc.rs` | 100.00 | 100.00 | 97.37 | 20/20 |
| `planning/explanation.rs` | 100.00 | 100.00 | 100.00 | 16/16 |

## Related docs

- [Engine integration test catalog](../../../engine/tests/README.md) — qualitative map of scenarios and subsystem overlap clusters
- [CLI test coverage](cli.md)
- [Engine benchmarks](../benchmarks/engine.md)
<!-- coverage-input-digest: 5180457271f27572 -->

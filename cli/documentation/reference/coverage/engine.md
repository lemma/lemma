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
| Lines | 60021 | 69216 | 86.72% |
| Functions | 4587 | 5174 | 88.65% |
| Regions | 88118 | 102595 | 85.89% |

## Test run

- Total: 3697
- Passed: 3697
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
| `computation/operation_result.rs` | 58.27 | 56.00 | 58.97 | 74/127 |
| `computation/comparison.rs` | 72.63 | 87.50 | 67.50 | 130/179 |
| `api/value.rs` | 74.29 | 75.00 | 79.64 | 104/140 |
| `computation/arithmetic.rs` | 75.27 | 68.63 | 72.20 | 691/918 |
| `documentation/mod.rs` | 77.22 | 76.92 | 78.63 | 61/79 |
| `parsing/arena.rs` | 77.45 | 76.19 | 74.06 | 158/204 |
| `error.rs` | 78.46 | 82.14 | 72.89 | 459/585 |
| `computation/bigint/signed.rs` | 78.77 | 75.56 | 76.66 | 256/325 |
| `computation/units.rs` | 79.81 | 100.00 | 77.58 | 166/208 |
| `mcp/tools.rs` | 80.13 | 71.43 | 78.92 | 383/478 |
| `computation/measure_math.rs` | 80.18 | 75.00 | 78.77 | 89/111 |
| `planning/graph.rs` | 81.01 | 88.04 | 81.56 | 11134/13744 |
| `planning/semantics.rs` | 81.56 | 83.88 | 81.65 | 4975/6100 |
| `evaluation/expression.rs` | 82.86 | 100.00 | 87.96 | 58/70 |
| `result_value.rs` | 84.46 | 80.56 | 80.73 | 413/489 |
| `evaluation/list_ops.rs` | 84.96 | 90.00 | 82.84 | 418/492 |
| `literals.rs` | 85.34 | 80.87 | 86.06 | 652/764 |
| `computation/datetime.rs` | 85.81 | 82.42 | 84.35 | 943/1099 |
| `planning/execution_plan.rs` | 85.99 | 78.05 | 84.02 | 3776/4391 |
| `parsing/lexer.rs` | 86.06 | 100.00 | 79.28 | 710/825 |
| `planning/normalize.rs` | 86.77 | 89.38 | 86.59 | 2577/2970 |
| `evaluation/imports.rs` | 87.11 | 75.00 | 86.05 | 196/225 |
| `planning/spec_value.rs` | 87.30 | 82.35 | 85.50 | 268/307 |
| `planning/instance_list.rs` | 87.36 | 88.57 | 87.53 | 228/261 |
| `spec_set_id.rs` | 87.72 | 100.00 | 95.52 | 50/57 |
| `parsing/ast.rs` | 88.05 | 90.06 | 84.23 | 1252/1422 |
| `computation/range.rs` | 88.10 | 94.12 | 83.33 | 185/210 |
| `formatting/expression.rs` | 88.15 | 98.08 | 89.36 | 580/658 |
| `planning/ordered_dispatch.rs` | 88.24 | 90.24 | 87.69 | 420/476 |
| `parsing/parser.rs` | 88.50 | 90.32 | 82.88 | 3116/3521 |
| `api/show.rs` | 88.51 | 87.34 | 89.56 | 901/1018 |
| `evaluation/mod.rs` | 89.67 | 81.67 | 90.39 | 799/891 |
| `api/types.rs` | 90.00 | 96.55 | 91.61 | 297/330 |
| `planning/unit_index.rs` | 90.22 | 94.39 | 90.30 | 904/1002 |
| `string_distance.rs` | 90.48 | 83.33 | 93.22 | 57/63 |
| `planning/typing.rs` | 90.96 | 97.78 | 88.86 | 745/819 |
| `api/evaluate_data_json.rs` | 90.99 | 100.00 | 93.14 | 202/222 |
| `evaluation/explanations.rs` | 91.06 | 96.55 | 88.41 | 479/526 |
| `planning/normalize/rewrite.rs` | 91.67 | 96.88 | 91.20 | 1321/1441 |
| `formatting/mod.rs` | 91.99 | 100.00 | 91.92 | 1010/1098 |
| `evaluation/evaluate_data.rs` | 92.32 | 90.62 | 90.56 | 1611/1745 |
| `evaluation/rows.rs` | 92.47 | 100.00 | 90.68 | 86/93 |
| `evaluation/narration.rs` | 92.50 | 81.71 | 93.06 | 1160/1254 |
| `registry.rs` | 92.76 | 97.53 | 94.50 | 999/1077 |
| `computation/rational.rs` | 92.84 | 98.64 | 89.74 | 1257/1354 |
| `planning/mod.rs` | 92.98 | 96.59 | 94.46 | 1165/1253 |
| `parsing/mod.rs` | 93.10 | 99.15 | 89.48 | 1753/1883 |
| `engine.rs` | 93.18 | 92.66 | 93.34 | 2611/2802 |
| `limits.rs` | 93.27 | 100.00 | 83.45 | 97/104 |
| `planning/show_expression.rs` | 93.32 | 100.00 | 94.58 | 419/449 |
| `planning/discovery.rs` | 93.42 | 96.74 | 94.23 | 1519/1626 |
| `computation/bigint/biguint.rs` | 93.54 | 97.44 | 91.39 | 738/789 |
| `quality.rs` | 93.95 | 97.80 | 90.59 | 885/942 |
| `evaluation/tree.rs` | 94.94 | 88.61 | 93.79 | 1706/1797 |
| `evaluation/conversion_trace.rs` | 95.95 | 100.00 | 96.96 | 237/247 |
| `planning/unit_family.rs` | 96.21 | 100.00 | 95.63 | 279/290 |
| `deps.rs` | 96.23 | 100.00 | 96.47 | 51/53 |
| `computation/decimal_math.rs` | 96.30 | 100.00 | 96.39 | 208/216 |
| `planning/spec_set.rs` | 96.50 | 95.24 | 98.23 | 138/143 |
| `evaluation/response.rs` | 96.51 | 93.59 | 95.45 | 1078/1117 |
| `evaluation/branch_semantics.rs` | 96.77 | 100.00 | 90.18 | 90/93 |
| `parsing/source.rs` | 99.20 | 100.00 | 97.30 | 124/125 |
| `computation/bigint/alloc.rs` | 100.00 | 100.00 | 97.37 | 20/20 |
| `planning/explanation.rs` | 100.00 | 100.00 | 100.00 | 16/16 |

## Related docs

- [Engine integration test catalog](../../../engine/tests/README.md) — qualitative map of scenarios and subsystem overlap clusters
- [CLI test coverage](cli.md)
- [Engine benchmarks](../benchmarks/engine.md)
<!-- coverage-input-digest: 49be2224df7fb293 -->

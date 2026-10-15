---
nav_title: Engine benchmarks
parent: Reference
nav_order: 60
---

# Engine evaluation benchmarks

Numbers are produced by `cargo benchmarks engine`. Hand-written Lemma specs and hand-written Python ports of the same business rules are measured on identical inline inputs.

## Methodology

- Hand-written Lemma specs vs hand-written Python ports of the same rules, on identical inline inputs.
- Cross-language latency compares per-request evaluation only: like comparing optimized C execution to Python, not C compile time to Python runtime.
- Lemma: compile (`Engine::new()` + `load(in_memory_source)`, parse + plan) once before measurement; timed loop = inline input literals + `Engine::evaluate` → terminal rule. Terminal rule is `total` (shipping, pricing) or `grand_total` (order_pipeline).
- Python: import module once before measurement; timed loop = inline input literals + `build_inputs(raw)` + `compute_terminal(inputs)`. `compute_terminal` evaluates only the terminal rule's dependency closure, matching Lemma's requested-rule walk.
- Literal constants are parsed once: Lemma at plan time, Python at import time (module-level `Fraction` constants). Rounding number types (`Decimal`, `float`) are out of scope; the comparison is exact arithmetic vs exact arithmetic.
- No disk I/O, no JSON input sidecars, no pre-built input maps outside the timed loop.
- Effective pinned to `2026-01-01T00:00:00Z` (no timezone) on the Lemma side; Python rules carry no temporal logic.
- Latency: Criterion (3s warmup, 30s measurement) for Lemma; 100 warmup + 10_000 measured `time.perf_counter_ns()` samples with `gc.disable()` bracketing for Python. Median and standard deviation reported.
- Numerical precision: a separate untimed pass compares all rule outputs. Lemma's `outputs` bench evaluates every local rule with explanations; Python's `compute(inputs)` returns a full `Outputs` dataclass. Both sides use exact rational arithmetic (`RationalInteger` / `fractions.Fraction`) and commit to decimal strings at the output boundary. The accuracy table compares both sides via `rust_decimal::Decimal` (28-digit precision).
- Memory: `stats_alloc` over 100 warmup + 1_000 measured eval-only `evaluate` calls per fixture (`cargo bench -p lemma-engine --bench memory`). Engine loaded once per fixture; each iteration wraps inline inputs + `Engine::evaluate` in a fresh region. Plan memory: same bench, 5 warmup + 50 measured compiles (`Engine::new()` + `load`, parse + plan) per fixture, each in a fresh region.
- Snapshot: `cargo bench -p lemma-engine --bench logistics` (embedded table) on the generated logistics ladder (1050 / 6300 / 18900 / 126000 rate cells; byte-identical to the Java `SpecGenerator.logistics` fixtures). One `load`, then 1 warmup + 10 measured `Engine::snapshot`, `Engine::from_snapshot`, and eval (`shipment inputs` + `Engine::evaluate` → `cheapest`; catalog already in the plan). Heap columns are `stats_alloc` net bytes retained by the loaded and by the restored engine. The restored engine must evaluate `rate_shop.cheapest` to the same value as the loaded engine.
- External catalog: same `cargo bench -p lemma-engine --bench logistics` (external table) on the same ladder with empty keyed lists in source and catalog rows held as JSON. Timed eval = `parse_evaluate_data_object` + `Engine::evaluate` (1 warmup + 10 measured); that includes JSON parse, unlike embedded eval. Load / snapshot / restore measure the skeleton only. Each rung must match embedded `rate_shop.cheapest`.

## Environment

- Host: `Linux 7.0.0-38-generic x86_64`
- Lemma git SHA: `b9c1a8bdf5b1c215d9b2c905788e6f2f8694f146`
- Generated (UTC): `2026-10-15T16:33:49Z`
- Python: `Python 3.12.3`
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

## Compile (Lemma, parse + plan)

One-time cost per spec load. Not included in the Python/Lemma latency ratio; amortized across requests in production.

| Spec | Median | Std dev |
|------|-------:|--------:|
| `bench_shipping` | 1.069 ms | 14.28 us |
| `bench_pricing` | 1.376 ms | 14.14 us |
| `bench_order_pipeline` | 1.954 ms | 19.02 us |

## Latency

| Spec | Terminal rule | Lemma median | Lemma std dev | Python median | Python iter | Python std dev | Python / Lemma |
|------|---------------|-------------:|--------------:|--------------:|------------:|---------------:|---------------:|
| `bench_shipping` | `total` | 8.25 us | 325 ns | 4.80 us | 10000 | 1.24 us | 0.5819 |
| `bench_pricing` | `total` | 20.29 us | 909 ns | 15.61 us | 10000 | 1.42 us | 0.7693 |
| `bench_order_pipeline` | `grand_total` | 36.99 us | 646 ns | 27.81 us | 10000 | 4.01 us | 0.7519 |

## Explain latency (`evaluate_explain`)

Same fixtures and terminal rules as the latency table, with `explain: true`. Ratio is explain median divided by `evaluate` median on the same machine run.

| Spec | Terminal rule | `evaluate` median | `evaluate_explain` median | Explain / `evaluate` |
|------|---------------|------------------:|--------------------------:|---------------------:|
| `bench_shipping` | `total` | 8.25 us | 91.42 us | 11.08 |
| `bench_pricing` | `total` | 20.29 us | 199.03 us | 9.808 |
| `bench_order_pipeline` | `grand_total` | 36.99 us | 423.08 us | 11.44 |

## Memory (per `evaluate` call)

| Spec | Iterations | Allocations/eval | Bytes allocated/eval | Reallocations/eval |
|------|-----------:|-----------------:|---------------------:|-------------------:|
| `bench_shipping` | 1000 | 49.00 | 16295 | 4.00 |
| `bench_pricing` | 1000 | 88.00 | 30018 | 6.00 |
| `bench_order_pipeline` | 1000 | 183.00 | 44422 | 6.00 |

## Memory (per compile, parse + plan)

| Spec | Iterations | Allocations/plan | Bytes allocated/plan | Reallocations/plan |
|------|-----------:|-----------------:|---------------------:|-------------------:|
| `bench_shipping` | 50 | 12670.00 | 1567101 | 270.00 |
| `bench_pricing` | 50 | 14563.00 | 1838595 | 400.00 |
| `bench_order_pipeline` | 50 | 18702.00 | 2390699 | 739.00 |

## Snapshot (logistics ladder)

Multi-spec rating workspace: `rates_*` cards with 1050 list rows each (`-> key` / `find`), `zones_*` with 900 ZIP3 rows per service, fuel brackets as range-keyed lists, one quote pipeline per carrier and service, and a `rate_shop` that `uses` every quote. Eval median is shipment inputs + `run` to `cheapest` (catalog already planned). Restore is the number to watch for snapshot cost; snapshot bytes and restored heap are dominated by the per-cell type payload the plan store ships today.

| Profile | Rate cells | Source | Load | Loaded heap | Snapshot | Encode median | Restore median | Eval median | Allocations/restore | Restored heap | Restored / loaded heap |
|---------|-----------:|-------:|-----:|------------:|---------:|--------------:|---------------:|------------:|--------------------:|--------------:|-----------------------:|
| `logistics_ground` | 1050 | 0.1 MiB | 22.777 ms | 5.3 MiB | 0.7 MiB | 1.354 ms | 3.336 ms | 3.678 ms | 36557 | 6.3 MiB | 1.191 |
| `logistics_carrier` | 6300 | 0.5 MiB | 127.943 ms | 31.2 MiB | 5.5 MiB | 9.600 ms | 16.840 ms | 32.822 ms | 160296 | 27.5 MiB | 0.8815 |
| `logistics_d2c` | 18900 | 1.5 MiB | 357.208 ms | 90.1 MiB | 16.7 MiB | 35.117 ms | 50.025 ms | 99.549 ms | 456660 | 79.5 MiB | 0.8822 |
| `logistics_enterprise` | 126000 | 9.7 MiB | 2686.067 ms | 603.2 MiB | 120.9 MiB | 230.378 ms | 341.310 ms | 666.278 ms | 2975761 | 529.9 MiB | 0.8786 |

## Snapshot / eval (logistics ladder, external catalog)

Same rate-shop workspace as the embedded ladder, but keyed catalog lists (`cells`, ZIP3 zones, fuel brackets) stay empty in source and arrive as JSON evaluation data on `rate_shop` (`qN.r.cells`, `qN.z.{service}_zones`, `qN.f.brackets`). Eval median is JSON parse + `run` to `cheapest`.

| Profile | Rate cells | Source | Load | Loaded heap | Snapshot | Encode median | Restore median | Eval median | Restored heap | Restored / loaded heap |
|---------|-----------:|-------:|-----:|------------:|---------:|--------------:|---------------:|------------:|--------------:|-----------------------:|
| `logistics_ext_ground` | 1050 | 0.0 MiB | 3.011 ms | 0.8 MiB | 0.1 MiB | 221.00 us | 1.116 ms | 18.793 ms | 2.5 MiB | 2.984 |
| `logistics_ext_carrier` | 6300 | 0.0 MiB | 11.068 ms | 2.3 MiB | 0.3 MiB | 491.00 us | 2.103 ms | 127.727 ms | 3.7 MiB | 1.586 |
| `logistics_ext_d2c` | 18900 | 0.0 MiB | 34.141 ms | 6.0 MiB | 0.7 MiB | 1.170 ms | 4.534 ms | 391.324 ms | 6.6 MiB | 1.095 |
| `logistics_ext_enterprise` | 126000 | 0.2 MiB | 488.896 ms | 37.9 MiB | 4.8 MiB | 9.687 ms | 25.272 ms | 2633.482 ms | 32.7 MiB | 0.8638 |

## Numerical accuracy

60 rule outputs compared across the three fixtures; 0 deviations.

## Python implementation

Hand-written ports of the three Lemma specs live in [`engine/benches/python/business_rules`](https://github.com/lemma/lemma/tree/b9c1a8bdf5b1c215d9b2c905788e6f2f8694f146/engine/benches/python/business_rules). Each module exports `Inputs`, `Outputs`, `TERMINAL_RULE`, `build_inputs(raw)`, `compute_terminal(inputs)`, and `compute(inputs)`. Standard library only (`fractions`, `dataclasses`, `importlib`, `time`, `gc`, `pathlib`, `statistics`). The Python benchmark harness is [`engine/benches/python/benchmark.py`](https://github.com/lemma/lemma/blob/b9c1a8bdf5b1c215d9b2c905788e6f2f8694f146/engine/benches/python/benchmark.py).

## Inputs

All fixtures share `effective = 2026-01-01T00:00:00Z` (no timezone). Input values are inline string literals built inside every timed iteration on both sides.

### `bench_shipping`

Lemma source: [`engine/benches/specs/shipping.lemma`](https://github.com/lemma/lemma/blob/b9c1a8bdf5b1c215d9b2c905788e6f2f8694f146/engine/benches/specs/shipping.lemma). Python module: `business_rules.shipping`.

| Field | Value |
|-------|-------|
| `weight` | `3` |
| `destination` | `domestic` |
| `is_member` | `false` |

### `bench_pricing`

Lemma source: [`engine/benches/specs/pricing.lemma`](https://github.com/lemma/lemma/blob/b9c1a8bdf5b1c215d9b2c905788e6f2f8694f146/engine/benches/specs/pricing.lemma). Python module: `business_rules.pricing`.

| Field | Value |
|-------|-------|
| `product_type` | `premium` |
| `quantity` | `25` |
| `unit_price` | `100` |
| `coupon_percent` | `5` |
| `loyalty_years` | `2` |
| `is_member` | `true` |
| `is_loyalty` | `true` |
| `is_tax_exempt` | `false` |

### `bench_order_pipeline`

Lemma source: [`engine/benches/specs/order_pipeline.lemma`](https://github.com/lemma/lemma/blob/b9c1a8bdf5b1c215d9b2c905788e6f2f8694f146/engine/benches/specs/order_pipeline.lemma). Python module: `business_rules.order_pipeline`.

| Field | Value |
|-------|-------|
| `customer_tier` | `gold` |
| `payment_method` | `credit` |
| `shipping_zone` | `national` |
| `quantity` | `12` |
| `unit_price` | `85` |
| `package_weight` | `3.5` |
| `delivery_distance` | `180` |
| `loyalty_points` | `6500` |
| `coupon_percent` | `10` |
| `is_fragile` | `true` |
| `is_express` | `true` |
| `is_hazardous` | `false` |
| `is_gift` | `false` |
| `is_first_time` | `false` |


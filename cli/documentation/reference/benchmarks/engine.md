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
- Logistics ladder: `cargo bench -p lemma-engine --bench logistics` (embedded table) on the generated logistics ladder (1050 / 6300 / 18900 / 126000 rate cells; byte-identical to the Java `SpecGenerator.logistics` fixtures). One `load`, then 1 warmup + 10 measured eval (`shipment inputs` + `Engine::evaluate` → `cheapest`; catalog already in the plan). Loaded heap is `stats_alloc` net bytes retained by the loaded engine.
- External catalog: same `cargo bench -p lemma-engine --bench logistics` (external table) on the same ladder with empty keyed lists in source and catalog rows held as JSON. Timed eval = `parse_evaluate_data_object` + `Engine::evaluate` (1 warmup + 10 measured); that includes JSON parse, unlike embedded eval. Load measures the skeleton only. Each rung must match embedded `rate_shop.cheapest`.

## Environment

- Host: `Linux 7.0.0-38-generic x86_64`
- Lemma git SHA: `14c79492a7b558328c11c787e5f11ebfc282afdb`
- Generated (UTC): `2026-11-04T14:37:52Z`
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
| `bench_shipping` | 1.421 ms | 68.04 us |
| `bench_pricing` | 1.894 ms | 154.79 us |
| `bench_order_pipeline` | 2.677 ms | 164.54 us |

## Latency

| Spec | Terminal rule | Lemma median | Lemma std dev | Python median | Python iter | Python std dev | Python / Lemma |
|------|---------------|-------------:|--------------:|--------------:|------------:|---------------:|---------------:|
| `bench_shipping` | `total` | 9.09 us | 618 ns | 4.93 us | 10000 | 1.13 us | 0.5428 |
| `bench_pricing` | `total` | 21.77 us | 1.89 us | 15.75 us | 10000 | 5.08 us | 0.7236 |
| `bench_order_pipeline` | `grand_total` | 37.29 us | 2.59 us | 29.07 us | 10000 | 8.44 us | 0.7796 |

## Explain latency (`evaluate_explain`)

Same fixtures and terminal rules as the latency table, with `explain: true`. Ratio is explain median divided by `evaluate` median on the same machine run.

| Spec | Terminal rule | `evaluate` median | `evaluate_explain` median | Explain / `evaluate` |
|------|---------------|------------------:|--------------------------:|---------------------:|
| `bench_shipping` | `total` | 9.09 us | 114.07 us | 12.56 |
| `bench_pricing` | `total` | 21.77 us | 245.23 us | 11.27 |
| `bench_order_pipeline` | `grand_total` | 37.29 us | 532.80 us | 14.29 |

## Memory (per `evaluate` call)

| Spec | Iterations | Allocations/eval | Bytes allocated/eval | Reallocations/eval |
|------|-----------:|-----------------:|---------------------:|-------------------:|
| `bench_shipping` | 1000 | 44.00 | 17261 | 4.00 |
| `bench_pricing` | 1000 | 74.00 | 31506 | 6.00 |
| `bench_order_pipeline` | 1000 | 146.00 | 46311 | 6.00 |

## Memory (per compile, parse + plan)

| Spec | Iterations | Allocations/plan | Bytes allocated/plan | Reallocations/plan |
|------|-----------:|-----------------:|---------------------:|-------------------:|
| `bench_shipping` | 50 | 14864.00 | 2032802 | 189.00 |
| `bench_pricing` | 50 | 16865.00 | 2388802 | 303.00 |
| `bench_order_pipeline` | 50 | 21263.00 | 3149834 | 585.00 |

## Load and eval (logistics ladder)

Multi-spec rating workspace: `rates_*` cards with 1050 list rows each (`-> key` / `find`), `zones_*` with 900 ZIP3 rows per service, fuel brackets as range-keyed lists, one quote pipeline per carrier and service, and a `rate_shop` that `uses` every quote. Eval median is shipment inputs + `run` to `cheapest` (catalog already planned). Spec-written rows are checked once against the row spec's shared template plan, and every nested consumer Arc-shares the one interned row store, so load and loaded heap scale with the unique catalog bodies, not with rows x consumers. Path strings are deduped by content across all path copies.

| Profile | Rate cells | Source | Load | Loaded heap | Eval median |
|---------|-----------:|-------:|-----:|------------:|------------:|
| `logistics_ground` | 1050 | 0.1 MiB | 15.230 ms | 5.5 MiB | 55.00 us |
| `logistics_carrier` | 6300 | 0.5 MiB | 76.795 ms | 29.2 MiB | 260.00 us |
| `logistics_d2c` | 18900 | 1.5 MiB | 210.518 ms | 85.6 MiB | 808.00 us |
| `logistics_enterprise` | 126000 | 9.7 MiB | 1565.382 ms | 571.4 MiB | 8.868 ms |

## Load and eval (logistics ladder, external catalog)

Same rate-shop workspace as the embedded ladder, but keyed catalog lists (`cells`, ZIP3 zones, fuel brackets) stay empty in source and arrive as JSON evaluation data on `rate_shop` (`qN.r.cells`, `qN.z.{service}_zones`, `qN.f.brackets`). Eval median is JSON parse + `run` to `cheapest`.

| Profile | Rate cells | Source | Load | Loaded heap | Eval median |
|---------|-----------:|-------:|-----:|------------:|------------:|
| `logistics_ext_ground` | 1050 | 0.0 MiB | 3.261 ms | 0.9 MiB | 8.096 ms |
| `logistics_ext_carrier` | 6300 | 0.0 MiB | 10.088 ms | 2.1 MiB | 53.536 ms |
| `logistics_ext_d2c` | 18900 | 0.0 MiB | 28.191 ms | 5.0 MiB | 167.739 ms |
| `logistics_ext_enterprise` | 126000 | 0.2 MiB | 183.311 ms | 30.3 MiB | 1122.205 ms |

## Numerical accuracy

60 rule outputs compared across the three fixtures; 0 deviations.

## Python implementation

Hand-written ports of the three Lemma specs live in [`engine/benches/python/business_rules`](https://github.com/lemma/lemma/tree/14c79492a7b558328c11c787e5f11ebfc282afdb/engine/benches/python/business_rules). Each module exports `Inputs`, `Outputs`, `TERMINAL_RULE`, `build_inputs(raw)`, `compute_terminal(inputs)`, and `compute(inputs)`. Standard library only (`fractions`, `dataclasses`, `importlib`, `time`, `gc`, `pathlib`, `statistics`). The Python benchmark harness is [`engine/benches/python/benchmark.py`](https://github.com/lemma/lemma/blob/14c79492a7b558328c11c787e5f11ebfc282afdb/engine/benches/python/benchmark.py).

## Inputs

All fixtures share `effective = 2026-01-01T00:00:00Z` (no timezone). Input values are inline string literals built inside every timed iteration on both sides.

### `bench_shipping`

Lemma source: [`engine/benches/specs/shipping.lemma`](https://github.com/lemma/lemma/blob/14c79492a7b558328c11c787e5f11ebfc282afdb/engine/benches/specs/shipping.lemma). Python module: `business_rules.shipping`.

| Field | Value |
|-------|-------|
| `weight` | `3` |
| `destination` | `domestic` |
| `is_member` | `false` |

### `bench_pricing`

Lemma source: [`engine/benches/specs/pricing.lemma`](https://github.com/lemma/lemma/blob/14c79492a7b558328c11c787e5f11ebfc282afdb/engine/benches/specs/pricing.lemma). Python module: `business_rules.pricing`.

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

Lemma source: [`engine/benches/specs/order_pipeline.lemma`](https://github.com/lemma/lemma/blob/14c79492a7b558328c11c787e5f11ebfc282afdb/engine/benches/specs/order_pipeline.lemma). Python module: `business_rules.order_pipeline`.

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


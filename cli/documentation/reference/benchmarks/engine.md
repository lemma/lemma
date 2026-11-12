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

## Environment

- Host: `Linux 7.0.0-42-generic x86_64`
- Lemma git SHA: `ed698fae7d18748da63701064891fdac4b8eaf7e`
- Generated (UTC): `2026-11-12T22:26:44Z`
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
| `bench_shipping` | 1.647 ms | 84.07 us |
| `bench_pricing` | 2.114 ms | 211.76 us |
| `bench_order_pipeline` | 2.951 ms | 169.00 us |

## Latency

| Spec | Terminal rule | Lemma median | Lemma std dev | Python median | Python iter | Python std dev | Python / Lemma |
|------|---------------|-------------:|--------------:|--------------:|------------:|---------------:|---------------:|
| `bench_shipping` | `total` | 9.11 us | 682 ns | 4.88 us | 10000 | 2.74 us | 0.5361 |
| `bench_pricing` | `total` | 20.87 us | 1.28 us | 15.73 us | 10000 | 5.84 us | 0.7535 |
| `bench_order_pipeline` | `grand_total` | 36.84 us | 1.71 us | 28.52 us | 10000 | 6.88 us | 0.7742 |

## Explain latency (`evaluate_explain`)

Same fixtures and terminal rules as the latency table, with `explain: true`. Ratio is explain median divided by `evaluate` median on the same machine run.

| Spec | Terminal rule | `evaluate` median | `evaluate_explain` median | Explain / `evaluate` |
|------|---------------|------------------:|--------------------------:|---------------------:|
| `bench_shipping` | `total` | 9.11 us | 115.54 us | 12.69 |
| `bench_pricing` | `total` | 20.87 us | 249.81 us | 11.97 |
| `bench_order_pipeline` | `grand_total` | 36.84 us | 546.42 us | 14.83 |

## Memory (per `evaluate` call)

| Spec | Iterations | Allocations/eval | Bytes allocated/eval | Reallocations/eval |
|------|-----------:|-----------------:|---------------------:|-------------------:|
| `bench_shipping` | 1000 | 44.00 | 19037 | 4.00 |
| `bench_pricing` | 1000 | 74.00 | 35010 | 6.00 |
| `bench_order_pipeline` | 1000 | 146.00 | 51847 | 6.00 |

## Memory (per compile, parse + plan)

| Spec | Iterations | Allocations/plan | Bytes allocated/plan | Reallocations/plan |
|------|-----------:|-----------------:|---------------------:|-------------------:|
| `bench_shipping` | 50 | 21697.00 | 2360349 | 179.00 |
| `bench_pricing` | 50 | 24190.00 | 2798353 | 301.00 |
| `bench_order_pipeline` | 50 | 29593.00 | 3719912 | 599.00 |

## Numerical accuracy

60 rule outputs compared across the three fixtures; 0 deviations.

## Python implementation

Hand-written ports of the three Lemma specs live in [`engine/benches/python/business_rules`](https://github.com/lemma/lemma/tree/ed698fae7d18748da63701064891fdac4b8eaf7e/engine/benches/python/business_rules). Each module exports `Inputs`, `Outputs`, `TERMINAL_RULE`, `build_inputs(raw)`, `compute_terminal(inputs)`, and `compute(inputs)`. Standard library only (`fractions`, `dataclasses`, `importlib`, `time`, `gc`, `pathlib`, `statistics`). The Python benchmark harness is [`engine/benches/python/benchmark.py`](https://github.com/lemma/lemma/blob/ed698fae7d18748da63701064891fdac4b8eaf7e/engine/benches/python/benchmark.py).

## Inputs

All fixtures share `effective = 2026-01-01T00:00:00Z` (no timezone). Input values are inline string literals built inside every timed iteration on both sides.

### `bench_shipping`

Lemma source: [`engine/benches/specs/shipping.lemma`](https://github.com/lemma/lemma/blob/ed698fae7d18748da63701064891fdac4b8eaf7e/engine/benches/specs/shipping.lemma). Python module: `business_rules.shipping`.

| Field | Value |
|-------|-------|
| `weight` | `3` |
| `destination` | `domestic` |
| `is_member` | `false` |

### `bench_pricing`

Lemma source: [`engine/benches/specs/pricing.lemma`](https://github.com/lemma/lemma/blob/ed698fae7d18748da63701064891fdac4b8eaf7e/engine/benches/specs/pricing.lemma). Python module: `business_rules.pricing`.

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

Lemma source: [`engine/benches/specs/order_pipeline.lemma`](https://github.com/lemma/lemma/blob/ed698fae7d18748da63701064891fdac4b8eaf7e/engine/benches/specs/order_pipeline.lemma). Python module: `business_rules.order_pipeline`.

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


---
nav_title: Logistics benchmarks
parent: Reference
nav_order: 55
---

# Logistics benchmarks

Numbers are produced by `cargo benchmarks logistics`. Large multi-spec rate-shop workspaces on the research ladder (1050 / 6300 / 18900 / 126000 rate cells; byte-identical to the Java `SpecGenerator.logistics` fixtures).

## Methodology

- Embedded ladder: `cargo bench -p lemma-engine --bench logistics` (embedded table). One `load`, then 1 warmup + 10 measured eval (`shipment inputs` + `Engine::evaluate` → `cheapest`; catalog already in the plan). Loaded heap is `stats_alloc` net bytes retained by the loaded engine.
- External catalog: same bench (external table) with empty keyed lists in source and catalog rows held as JSON. Timed eval = `parse_evaluate_data_object` + `Engine::evaluate` (1 warmup + 10 measured); that includes JSON parse, unlike embedded eval. Load measures the skeleton only. Each rung must match embedded `rate_shop.cheapest`.

## Environment

- Host: `Linux 7.0.0-42-generic x86_64`
- Lemma git SHA: `83e5179496f8e490ba5f64b7750476f1aa5fc899`
- Generated (UTC): `2026-11-09T15:32:57Z`
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

## Load and eval (embedded catalog)

Multi-spec rating workspace: `rates_*` cards with 1050 list rows each (`-> key` / `find`), `zones_*` with 900 ZIP3 rows per service, fuel brackets as range-keyed lists, one quote pipeline per carrier and service, and a `rate_shop` that `uses` every quote. Eval median is shipment inputs + `run` to `cheapest` (catalog already planned). Spec-written rows are checked once against the row spec's shared template plan, and every nested consumer Arc-shares the one interned row store, so load and loaded heap scale with the unique catalog bodies, not with rows x consumers. Path strings are deduped by content across all path copies.

| Profile | Rate cells | Source | Load | Loaded heap | Eval median |
|---------|-----------:|-------:|-----:|------------:|------------:|
| `logistics_ground` | 1050 | 0.1 MiB | 13.745 ms | 5.3 MiB | 58.00 us |
| `logistics_carrier` | 6300 | 0.5 MiB | 66.606 ms | 27.5 MiB | 263.00 us |
| `logistics_d2c` | 18900 | 1.5 MiB | 195.462 ms | 80.9 MiB | 1.028 ms |
| `logistics_enterprise` | 126000 | 9.7 MiB | 1268.418 ms | 535.1 MiB | 6.536 ms |

## Load and eval (external catalog)

Same rate-shop workspace as the embedded ladder, but keyed catalog lists (`cells`, ZIP3 zones, fuel brackets) stay empty in source and arrive as JSON evaluation data on `rate_shop` (`qN.r.cells`, `qN.z.{service}_zones`, `qN.f.brackets`). Eval median is JSON parse + `run` to `cheapest`.

| Profile | Rate cells | Source | Load | Loaded heap | Eval median |
|---------|-----------:|-------:|-----:|------------:|------------:|
| `logistics_ext_ground` | 1050 | 0.0 MiB | 3.111 ms | 0.8 MiB | 8.170 ms |
| `logistics_ext_carrier` | 6300 | 0.0 MiB | 9.625 ms | 1.6 MiB | 51.221 ms |
| `logistics_ext_d2c` | 18900 | 0.0 MiB | 25.000 ms | 3.6 MiB | 157.817 ms |
| `logistics_ext_enterprise` | 126000 | 0.2 MiB | 168.448 ms | 20.6 MiB | 1028.262 ms |


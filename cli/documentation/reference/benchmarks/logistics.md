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
- Lemma git SHA: `ed698fae7d18748da63701064891fdac4b8eaf7e`
- Generated (UTC): `2026-11-12T22:27:05Z`
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
| `logistics_ground` | 1050 | 0.1 MiB | 15.942 ms | 5.6 MiB | 86.00 us |
| `logistics_carrier` | 6300 | 0.5 MiB | 67.584 ms | 28.4 MiB | 301.00 us |
| `logistics_d2c` | 18900 | 1.5 MiB | 211.144 ms | 83.1 MiB | 933.00 us |
| `logistics_enterprise` | 126000 | 9.7 MiB | 1363.709 ms | 548.5 MiB | 8.781 ms |

## Load and eval (external catalog)

Same rate-shop workspace as the embedded ladder, but keyed catalog lists (`cells`, ZIP3 zones, fuel brackets) stay empty in source and arrive as JSON evaluation data on `rate_shop` (`qN.r.cells`, `qN.z.{service}_zones`, `qN.f.brackets`). Eval median is JSON parse + `run` to `cheapest`.

| Profile | Rate cells | Source | Load | Loaded heap | Eval median |
|---------|-----------:|-------:|-----:|------------:|------------:|
| `logistics_ext_ground` | 1050 | 0.0 MiB | 4.456 ms | 1.0 MiB | 7.884 ms |
| `logistics_ext_carrier` | 6300 | 0.0 MiB | 11.611 ms | 2.0 MiB | 51.292 ms |
| `logistics_ext_d2c` | 18900 | 0.0 MiB | 33.278 ms | 4.4 MiB | 156.250 ms |
| `logistics_ext_enterprise` | 126000 | 0.2 MiB | 209.292 ms | 25.0 MiB | 1075.128 ms |


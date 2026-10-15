---
nav_title: CLI benchmarks
parent: Reference
nav_order: 50
---

# CLI benchmarks

Numbers are produced by `cargo benchmarks cli`. Measures the `lemma` binary and in-process engine wrappers used by the CLI.

## Methodology

### HTTP evaluate (`http_evaluate`)

- Spawns `lemma server --path engine/documentation/examples` on `127.0.0.1:19877` once per Criterion group.
- Each iteration: blocking `reqwest` POST with `application/x-www-form-urlencoded` body (coffee order, library fees, Dutch net salary) or GET for show-only retrieval.
- Examples loaded from [`engine/documentation/examples`](https://github.com/lemma/lemma/tree/b9c1a8bdf5b1c215d9b2c905788e6f2f8694f146/engine/documentation/examples).
- Latency: Criterion (3s warmup, 10s measurement for evaluate group, 5s for show). Median and standard deviation reported.

### Engine profile (`engine_profile`)

- In-process: loads all `.lemma` files from `engine/documentation/examples` into one `Engine`.
- Fixture: Dutch net salary (`net_salary`) with `gross_salary=5000 eur`, `pay_period=month`, `income_source=employment`, `pension_contribution=150 eur`, `payroll_tax_credit=true`; effective is `DateTimeValue::now()` per iteration setup.
- Breakdown benches isolate evaluate, overlay resolve, single-rule run, and JSON serialization paths.
- Latency: Criterion (3s warmup, 5s measurement). Median and standard deviation reported.

## Environment

- Host: `Linux 7.0.0-38-generic x86_64`
- Lemma git SHA: `b9c1a8bdf5b1c215d9b2c905788e6f2f8694f146`
- Generated (UTC): `2026-10-15T16:36:25Z`
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

## HTTP evaluate latency

| Case | Median | Std dev |
|------|-------:|--------:|
| POST `/coffee_order` | 164.94 us | 13.32 us |
| POST `/library_fees` | 149.90 us | 9.07 us |
| POST `/net_salary` | 275.71 us | 19.09 us |
| GET `/net_salary` (show only) | 1.001 ms | 75.55 us |

## Engine profile latency (Dutch net salary)

| Case | Median | Std dev |
|------|-------:|--------:|
| Full `Engine::evaluate` | 101.76 us | 1.45 us |
| Single-rule evaluate (`periods_per_year`) | 10.61 us | 188 ns |
| Envelope JSON serialize | 21.31 us | 448 ns |
| Raw response JSON serialize | 13.00 us | 749 ns |


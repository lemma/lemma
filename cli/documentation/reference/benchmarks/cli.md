---
nav_title: CLI benchmarks
parent: Reference
nav_order: 50
---

# CLI benchmarks

Numbers are produced by `cargo benchmarks cli`. Measures the `lemma` binary and in-process engine wrappers used by the CLI.

## Methodology

### HTTP evaluate (`http_evaluate`)

- Spawns `lemma server --prefix engine/documentation/examples` on `127.0.0.1:19877` once per Criterion group.
- Each iteration: blocking `reqwest` POST with `application/x-www-form-urlencoded` body (coffee order, library fees, Dutch net salary) or GET for show-only retrieval.
- Examples loaded from [`engine/documentation/examples`](https://github.com/lemma/lemma/tree/bf5e55391a49d2e7683aa4b39a98023ef7ee0083/engine/documentation/examples).
- Latency: Criterion (3s warmup, 10s measurement for evaluate group, 5s for show). Median and standard deviation reported.

### Engine profile (`engine_profile`)

- In-process: loads all `.lemma` files from `engine/documentation/examples` into one `Engine`.
- Fixture: Dutch net salary (`net_salary`) with `gross_salary=5000 eur`, `pay_period=month`, `income_source=employment`, `pension_contribution=150 eur`, `payroll_tax_credit=true`; effective is `DateTimeValue::now()` per iteration setup.
- Breakdown benches isolate evaluate, overlay resolve, single-rule run, and JSON serialization paths.
- Latency: Criterion (3s warmup, 5s measurement). Median and standard deviation reported.

## Environment

- Host: `Linux 7.0.0-31-generic x86_64`
- Lemma git SHA: `bf5e55391a49d2e7683aa4b39a98023ef7ee0083`
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
| POST `/coffee_order` | 189.88 us | 28.70 us |
| POST `/library_fees` | 139.71 us | 16.06 us |
| POST `/net_salary` | 241.09 us | 13.50 us |
| GET `/net_salary` (show only) | 521.42 us | 23.36 us |

## Engine profile latency (Dutch net salary)

| Case | Median | Std dev |
|------|-------:|--------:|
| Full `Engine::run` | 84.39 us | 6.44 us |
| Single-rule evaluate (`periods_per_year`) | 9.31 us | 129 ns |
| Envelope JSON serialize | 21.30 us | 370 ns |
| Raw response JSON serialize | 9.93 us | 157 ns |


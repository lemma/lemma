---
nav_title: Numeric precision
nav_order: 70
---

# Numeric precision

Lemma stores and computes numbers as exact rationals. This chapter explains how that works, where decimal appears, and what Spec authors and API clients need to know.

## How Lemma handles numbers

Lemma uses three layers:

1. **Compute (ℚ):** magnitudes stored as exact rationals (`BigInt` numerator/denominator of at most 32,768 bits each). Arithmetic, comparisons, and unit conversions run in ℚ. No per-step decimal rounding.
2. **Boundary (decimal):** a single conversion to `rust_decimal` at boundaries: API/JSON output, show/API serialization, transcendental function operands.
3. **Output (decimal string):** API responses send magnitudes as JSON strings (`"37"`, `"99.50"`), never JSON number literals.

```
spec → parse (Decimal → ℚ) → plan → evaluate (ℚ) → try_to_decimal → decimal string
```

Input parsing and response decimal strings both enforce [`Decimal::MAX_SCALE`](https://docs.rs/rust_decimal/latest/rust_decimal/struct.Decimal.html#associatedconstant.MAX_SCALE) (28 decimal digits).

## Limits

| Layer | Constraint |
|-------|------------|
| Literals, JSON input, API input | Magnitude ±79,228,162,514,264,337,593,543,950,335 (~7.92×10²⁸); at most 28 decimal digits (`Decimal::MAX_SCALE`) |
| Internal compute (ℚ) | Numerator and denominator of every operator result at most 32,768 bits each (up to 9,865 decimal digits); all BigInt allocation is fallible |

Intermediate values during evaluation may exceed the decimal range or use more precision than 28 digits. Internal computation never precision-fails. Only top-level rule results written into the response envelope are rounded to `Decimal::MAX_SCALE`. Magnitude overflow (`|value| > Decimal::MAX`) vetoes with `Calculated result exceeds decimal value limit`. That rounding applies when building the response envelope, not when storing in `rule_values` (which keeps exact ℚ values).

Every operator result (`+`, `-`, `*`, `/`, `mod`, `^` on numbers, ratios and quantities) is checked against the 32,768-bit limit on its numerator and on its denominator. A result beyond it vetoes with `numeric result exceeds the maximum of 32768 bits`. With `data base: number` bound to `2`, `base ^ 32767` (32,768 bits) evaluates exactly and `base ^ 32768` vetoes. A power whose result provably exceeds the limit vetoes before it is computed, so `2 ^ years` with a huge `years` fails immediately. Constant number expressions (every operand a number literal) are folded during planning; one that exceeds the limit is a planning Error instead of a Veto, like `1 / 0`.

When an exact rational grows past what memory allows, evaluation vetoes with `out of memory` instead of crashing the process. This is resource exhaustion, not a decimal-scale failure.

JSON/API output never emits fraction strings (`"37/47"`) or scientific notation.

## Display vs API output

CLI and human-readable formatting show a decimal when [`RationalInteger::try_to_decimal`] succeeds. On magnitude overflow only, display falls back to `numer/denom` (debugging). JSON and API surfaces use decimal strings only; oversized top-level rule results veto instead of emitting a fraction.

## Exact paths

These stay in ℚ end-to-end (no mid-pipeline decimal):

- `+`, `-`, `*`, `/`, `mod` on numbers, ratios, quantities
- Unit conversion (`as <unit>`): `magnitude × from_factor ÷ to_factor`
- Comparisons
- Integer powers whose result stays within the 32,768-bit limit; rational powers when the result is exact (e.g. `4 ^ 0.5` → `2`)

Long conversion chains telescope in ℚ. Example: `37 base` through fourteen prime units forward and back stays exactly `37`; stepwise decimal at fixed precision drifts to `36.999…`.

## Approximation paths

| Case | Behavior |
|------|----------|
| `^` on irrationals (e.g. `2 ^ 0.5`) | `Decimal::MAX_SCALE` digit fallback when no exact root |
| `sqrt`, `sin`, `cos`, `log`, … | `Decimal::MAX_SCALE` digit semantics; operands converted via `try_to_decimal` |
| `tan` | Computed as `sin / cos` at `Decimal::MAX_SCALE` digit semantics. Near π/2 the relative error is at most about `1e-28 / (π/2 − \|x\|)`, because `cos` is evaluated from the 28-digit π/2 constant. Veto where `cos` is exactly zero or the quotient overflows |
| `floor`, `ceil`, `round` on measures | Operate at Decimal precision |
| ℚ result beyond 32,768 bits | Veto (`numeric result exceeds the maximum of 32768 bits`); planning Error for constant number expressions; no decimal fallback |
| ℚ allocation failure | Veto (`out of memory`); no decimal fallback |
| Division by zero | Literal zero divisor in a Rule (e.g. `1 / 0`) → planning Error. Zero from runtime Data → Veto (never approximated) |

## Boundary number contract

All API surfaces enforce a uniform rule for numeric data inputs:

| Input type | Accepted? | Rationale |
|-----------|-----------|-----------|
| Integer (npm) | Digit **string** or **`bigint`** | A JS `number` is always a double, so it is rejected whatever its value |
| Integer (JSON / HTTP) | **Rejected** | JSON numbers have no exact native type; pass `"42"` |
| Integer (JS `bigint`, Java integer/`BigDecimal`, Elixir integer) | Yes | Exact decimal text |
| Decimal / float (JS `number`, Java `Float`/`Double`, Elixir float) | **Rejected** | IEEE 754 f64 cannot represent most decimals exactly |
| String `"0.1"`, `"99.50"` | Yes | Parsed as exact decimal → ℚ |

Non-integer numeric values and JSON numbers are rejected with `pass as string: "84"`. Rust callers using `Engine::evaluate` pass `EvaluateDataValue`. Java callers pass `BigDecimal` or decimal strings through `EvaluateRequest.data(...)`, never `double`/`float` for domain decimals.

## Clients (JavaScript / HTTP / Java / Kotlin)

**Sending data:** decimals as strings. On npm, integers as digit strings or `bigint` (JS `number` is rejected):

```javascript
engine.evaluate({
  spec: 'pricing',
  data: {
    quantity: 42n,
    rate: '0.075',
    account_id: '79228162514264337593543950335', // or 79228162514264337593543950335n
  },
});
```

HTTP/JSON rejects numbers; pass decimal strings. On Java / Kotlin, pass `BigDecimal` or decimal strings in `EvaluateRequest.data(...)`: see [Java / Kotlin](../tools/java.md).

**Reading results:** parse numeric fields as decimal strings, not floats:

```javascript
import Decimal from "decimal.js";
const n = new Decimal(json.results.price.number);
```

Never `Number()` / `parseFloat()` / `float()`. Precision breaks above ~9×10¹⁵. The same string magnitudes appear in HTTP JSON responses; parse them with a decimal library in your language. On Java / Kotlin, rule magnitudes are already `BigDecimal`.

## Spec authors

- Money, units, ratios: exact by default; define unit factors as clean literals.
- `decimals n`: constrains decimal scale when validating Data inputs, suggestions, and value copies, not internal ℚ compute or intermediate Rule steps.
- Keep deliverable top-level Rule results within the 28-digit decimal limit; magnitude overflow at output vetoes.

## Related

- [Reference: primitive types](../reference/readme.md#primitive-types)
- [Veto](types_and_units.md#veto): when results exceed limits or division by zero occurs at runtime

You have completed the Learn guide. For exhaustive syntax and operators, see the [Language reference](../reference/readme.md). To embed Lemma, see [Tools & SDKs](../tools/readme.md).

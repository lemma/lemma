---
nav_title: Types and units
nav_order: 40
---

# Types and units

Lemma has a rich type system built from primitives, quantities with units, ratios, and ranges. This chapter covers literals, operators, the standard library, conversions, ranges, dates, and Veto (the domain outcome when a Rule cannot produce a value).

## Literal types

| Type | Example | Notes |
|------|---------|-------|
| Number | `42`, `3.14`, `1.23e10` | Exact rational arithmetic |
| Text | `"hello"` | String literals |
| Boolean | `true`, `false`, `yes`, `no` | Aliases |
| Date | `2024-01-15`, `2024-01-15T14:30:00Z` | ISO 8601 |
| Time | `14:30:00` | Time of day |
| Measure | `100 eur`, `40 hour`, `12 kilogram` | Unit must be declared by a measure type in scope |
| Ratio | `15 percent`, `15%`, `5 permille`, `5%%` | Proportional values |
| Range | `0...100`, `2024-01-01...2024-06-15`, `18 year...67 year` | Half-open intervals |

See [Primitive types in the language reference](../reference/readme.md#primitive-types).

## Qualifying units

Qualify a unit when its bare name is ambiguous in scope; otherwise qualification is optional.

| Form | Meaning |
|------|---------|
| `kilogram` | Bare: the one type in scope that declares the unit |
| `my_weight.kilogram` | Extension (or owning) type |
| `units.mass.kilogram` | Import alias + type |

Bare `5 kilogram` binds the declarer. Qualified forms are `Type.unit` or `alias.Type.unit` (not `alias.unit`). When two independent types in scope declare the same unit name, bare use is a planning error that lists the qualified forms; write one of them. Extensions inherit: bare stays on the declarer; use qualification or cast for the extension.

Custom units on `ratio` types follow the same rule. Builtin `percent` and `permille` (`%` / `%%`) are primitive: always usable bare, and part of every ratio type. `as percent` / `as permille` always works; `as` into a uniquely in-scope custom ratio unit works from any ratio.

```lemma
spec wages

data money: measure
  -> unit eur: 1

data price: money
  -> minimum 0 eur

rule bare:
  5 eur

rule priced:
  5 price.eur
```

The right-hand side of `-> unit <name>:` on a measure may be a number, a compound expression, or a reference to a `data` or `rule` of type `number`:

```lemma
spec fx

data rate: number
data money: measure
  -> unit eur: 1
  -> unit gbp: rate

rule total:
  100 gbp
```

Scales are evaluated before every rule that holds a value of that measure. A value written in a dynamic unit takes that unit's scale: unbound scale data is missing data, a vetoing scale passes its veto through, and a scale of zero or less is a computation veto. A scale the value does not need leaves the rule's result alone; only that unit's column is left out. A scale rule that itself holds a value of its measure is a circular dependency. A unit factor is set per instance: each `uses` alias and each list row has its own factor, never shared. Through a `uses` alias you write, convert, read, and display a dynamic unit the same way as a literal one; a value read from a row keeps that row's factors, and values combined across rows keep only factors the rows agree on. Dynamic units may appear in compound unit expressions (`gbp/kilogram`, `0.001 gbp/kilogram`) and in `minimum`, `maximum`, `suggest`, and `option`. `ratio` units stay literal-only.

```lemma
spec packing

uses lemma units

data crate_weight: measure
  -> unit kilogram: 25

rule crates:
  4 crate_weight.kilogram

rule net:
  80 units.mass.kilogram
```
## Arithmetic

```lemma
spec arithmetic_examples

data price:     100
data tax:       21
data quantity:  3
data principal: 1_000
data rate:      0.05
data year:      10

rule total:
  (price + tax) * quantity

rule compound:
  principal * (1 + rate) ^ year
```

Operators: `+`, `-`, `*`, `/`, `mod`, `^`

`a mod b` takes two numbers. It is the remainder of `a / b`, floored, so it takes the sign of the divisor: `-7 mod 3` is `2`. A ratio or a measure on either side is a planning error. The `%` sign is reserved for percent literals such as `50%`.

## Comparison

```lemma
spec comparison_examples

data status: text
data age:    number
data income: number

rule status_ok:
  status is "approved"

rule not_cancelled:
  status is not "cancelled"

rule is_eligible:
  age >= 18 and income > 30_000
```

Operators: `>`, `<`, `>=`, `<=`, `is`, `is not`, `is veto`, `is not veto`

## Logical

```lemma
spec loan_approval

data credit_score:    number
data income_verified: boolean
data has_bankruptcy:  boolean

rule can_approve_loan:
  credit_score >= 650
  and income_verified
  and not has_bankruptcy
```

Operators: `and`, `or`, `not`. `or` binds looser than `and`. A `true` operand decides `or` whatever the other operands are; a `false` operand decides `and` the same way.

## Mathematical

```lemma
spec math_examples

data a:     3
data b:     4
data angle: 0.5

rule hypotenuse:
  sqrt(a ^ 2 + b ^ 2)

rule sine_value:
  sin angle

rule log_value:
  log 10
```

Prefix operators: `sqrt`, `sin`, `cos`, `tan`, `log`, `exp`, `abs`, `lower`, `upper`, and also `not`. Write `abs(total) + 1` to add to the result, or `abs (total + 1)` to take the absolute value of the sum. Without parentheses the argument is one operand and nothing but `then`, `unless`, `)`, `,` or the end may follow, so `abs total` is fine and `abs total + 1` is a parse error. `lower` and `upper` read a range's endpoints (see [Ranges](#ranges)). Rounding is postfix only (see [Rounding](#rounding)).

### Rounding

`x -> round`, `x -> floor` and `x -> ceil` round a number to an integer. An exact half goes away from zero: `5 / 2 -> round` is `3`, `-5 / 2 -> round` is `-3`. A measure or a ratio has no single integer to round to: 1.5 kilogram is 1500 gram. Round those to a step, with `-> round`, `-> floor` or `-> ceil` followed by a literal:

```lemma
spec invoice

data money: measure
  -> unit eur: 1.00

data duration: measure
  -> unit minute: 60
  -> unit hour: 3600

data hourly_rate: money
data time_worked: duration
data vat:         ratio

rule billed_time:
  time_worked -> ceil 15 minute

rule net:
  billed_time as hour as number * hourly_rate

rule total:
  net * (100% + vat) -> round 0.05 eur

rule vat_shown:
  vat -> round 1%
```

With 100 minute at 85.30 eur and 20.6% VAT, `billed_time` is 105 minute, `net` is 149.275 eur, `total` is 180.05 eur, and `vat_shown` is 21%.

- `-> round 0.05 eur` rounds to the nearest multiple of 5 cent. An exact half goes away from zero.
- The step decides the result's unit: `time_worked -> ceil 15 minute` is in minute, whatever unit `time_worked` was given in.
- `->` applies to everything on its left, up to a comparison, `and`, `or` or `not`. `net * (100% + vat) -> round 0.05 eur` rounds the product. Put parentheses around the command to compute with its result: `(total -> round 1 eur) - net`.
- The step must be a positive literal of the same kind: a number for a number (`x -> round 0.01`), a ratio for a ratio, and a unit of the same measure for a measure.
- A step followed by a comparison takes the call form: `total -> round(1 eur) > 100 eur`.

## Standard library: `uses lemma units`

Lemma embeds SI bases, derived compounds, imperial, and information units in the standard library (Repo `lemma`, Spec `units`). Import with `uses lemma units`, then use units directly in literals or reference types as `units.mass`, `units.duration`, `units.length`, `units.calendar`, `units.force`, and others. When a local type reuses a stdlib unit name, qualify (`units.mass.kilogram`): see [Qualifying units](#qualifying-units).

Names are singular only (`8 hour`, not `8 hours`). Length uses American `meter` (not `metre`).

```lemma
spec logistics

uses lemma units

data package_weight: 12 kilogram
data shift_length:   8 hour
data route_distance: 45 kilometer

rule weight_grams:
  package_weight as gram

rule is_heavy:
  package_weight > 20 kilogram

rule is_long_shift:
  shift_length >= 8 hour
```

Duration units (`hour`, `day`, `week`, ...) come from `units.duration`; calendar periods (`year`, `month`) from `units.calendar`. Prefer the stdlib types over redefining kilogram or hour in every Spec.

## Unit conversions

Convert within a unit family with `as`:

```lemma
spec conversion_examples

data money: measure
  -> unit eur: 1.00
  -> unit usd: 0.91

data price: 100 eur

rule price_usd:
  price as usd

rule as_percent:
  0.25 as percent
```

Durations convert the same way:

```lemma
spec schedule

uses lemma units

data workweek: 40 hour

rule workweek_days:
  workweek as day
```

Strip to a bare number with a chained cast: `amount as eur as number`. See [Type cast in the language reference](../reference/readme.md#type-cast-as).

## Ranges

Intervals use `lo...hi` (lower inclusive, upper exclusive). Test membership with `in`; project width with `(lo...hi) as <unit>`. Read an endpoint with `lower` / `upper`: `lower` is the included end, `upper` is the excluded end (`upper(5...10)` is `10`, and `10` is not `in` the range). `...` binds tighter, so `lower 5...10` is the lower end of that range. Range slots use `number range`, `date range`, `time range`, `measure range`, `ratio range`, or a named `<type> range`. Constrain endpoints with `-> lower` / `-> upper` and span width with `-> minimum` / `-> maximum`. Those commands are not the operators, and `lower` / `upper` cannot be data or rule names:

```lemma
spec eligibility

uses lemma units

data age:   25 year
data score: 50
data window: date range
  -> lower 2020-01-01
  -> upper 2030-12-31
  -> minimum 1 day
  -> maximum 90 day

rule in_working_age:
  age in 18 year...67 year

rule in_band:
  score in 0...100

rule days_in_q2:
  2024-04-01...2024-07-01 as day

rule window_start:
  lower window
```

See [Ranges in the language reference](../reference/readme.md#ranges) and [Data commands](../reference/readme.md#data-commands).

## Date and time

Dates compare directly; spans between dates are ranges projected to a unit; durations add to dates:

```lemma
spec deadlines

uses lemma units

data today:    2024-09-30
data deadline: 2024-12-31

rule days_until_deadline:
  today...deadline as day

rule is_overdue:
  today > deadline

rule follow_up_date:
  deadline + 14 day
```

### Calendar vs duration arithmetic

Calendar units (`year`, `month`) use calendar-aware arithmetic; duration units (`day`, `hour`, `second`, ...) use fixed-length arithmetic. Adding one month to March 1 gives April 1 regardless of month length, while adding 30 day always adds exactly 30 × 86400 second.

**Month-end clamping**: adding month or year clamps to the last valid day of the target month. January 31 + 1 month → February 28 (or 29 in a leap year). March 31 − 1 month → February 28/29.

### Relative to `now`

`now` is the evaluation instant. Compare dates to it, or to a sliding window / calendar period (duration units from `uses lemma units`):

```lemma
spec recency

uses lemma units

data event_date: date

rule was_before_now:
  event_date in past

rule recent:
  event_date in past 7 day

rule this_year:
  event_date in calendar year

rule last_month:
  event_date in past calendar month
```

Also: `in future`, `future N day`, `in past|future calendar year|month|week`, `not in calendar …`, and bare windows `past 7 day`. Full table: [Date predicates in the language reference](../reference/readme.md#date-predicates-and-windows).

## Veto

Use Veto when a Rule cannot produce a meaningful value: the domain says "no answer here." Use Veto for impossible situations, not for negative business results. A Rule that evaluates to `false` or `0` is still a valid result.

**Litmus test:** Can the question be answered? If yes, even when the answer is negative, use `true`/`false`. If the question itself is unanswerable for this input, use veto. "Is the customer eligible?" is always answerable (`true` or `false`). "What is the price of this coffee?" when the product is not on the menu is unanswerable (veto).

A vetoed Rule is not `false`. `x is false` does not match a vetoed `x`. To test whether a Rule vetoed, use `x is veto`.

| Situation | Use |
|-----------|-----|
| Out-of-range input (negative score, age above 120) | `-> minimum` / `-> maximum` on `data` |
| Closed option list | `-> option` on `data` |
| Normal business "no" | `false` or `no` |
| Lookup / no mapped result | default `veto` + `unless` arm per known case |
| Test veto without propagating | `x is veto` (returns boolean) |

Out-of-range values and failed constraints on `data` bind as a veto on that slot at runtime; express bounds on the `data` declaration instead of vetoing in a Rule.

### Lookup (default veto + unless arms)

When a value is on a closed list but has no mapped outcome, default to `veto` and map each known case with `unless`:

```lemma
spec coffee_prices

data money: measure
  -> unit eur: 1.00

data drink: text
  -> option "latte"
  -> option "cappuccino"

rule base_price:
  veto "Unknown product"
  unless drink is "latte"      then 3.50 eur
  unless drink is "cappuccino" then 3.50 eur

rule total:
  base_price * 2
```

If `drink` is not provided, `base_price` vetoes with "Missing data: drink", not the default arm's message. The `total` Rule also vetoes because its dependency has no value.

### Bounds on data, not veto in rules

```lemma
spec age_gate

data customer_age: number
  -> minimum 0
  -> maximum 120
  -> help "How old is the customer?"

rule is_adult:
  customer_age >= 18
```

### Veto does not apply when Unless provides a fallback

```lemma
spec shipping_estimate

uses lemma units

data weight: units.mass
  -> minimum 0 kilogram

data use_estimated: boolean

rule shipping_weight:
  weight
  unless use_estimated then 5 kilogram
```

If `weight` is missing or vetoed on constraints but `use_estimated` is true, `shipping_weight` = `5 kilogram` because the Unless arm does not need `weight`.

### Missing Data propagates as Veto

When a Data field has no value (not provided), Rules that depend on it Veto with a "Missing data" reason. See the lookup example above when `drink` is absent.

After a `MissingData` on one operand of an arithmetic or comparison operator, evaluation still walks the other operand: a definitive veto there settles the result, and nested control can record for explain and prune. `and` and `or` are order-independent for their absorbing operand. A `false` operand decides the `and`, and a `true` operand decides the `or`, whatever the other operands are, so a missing or vetoed sibling never appears on `missing_data`. Without an absorbing operand, the first missing or vetoed operand in source order decides the result, and every operand not yet known to be the identity (`true` for `and`, `false` for `or`) stays on `missing_data`. Intake keeps unbound keys on `missing_data` only when some completion can still produce a **value**. Unbound `unless 5 * x > 3 then y` therefore lists both `x` and `y` until the condition decides; a definitive Computation veto in the condition settles without asking for the then-arm. Product and other operators that need both values settle on a definitive factor and clear keys that cannot un-veto. `is veto` stays a boolean probe and does not change this intake rule.

### `is veto` (boolean test)

Test whether an expression produced no value and branch on a boolean, without propagating the operand's Veto through the test:

```lemma
spec fallback_total

data price: number
  -> minimum 0

data quantity: number

rule line_total:
  price * quantity
  unless price is veto then 0
```

When `price` vetoes (for example a failed constraint override), `price is veto` is true and `line_total` can take the fallback `0`. The test never returns Veto; only the Rule's final arm can.

Equivalent forms: `veto is price`, `price is not veto`, `not veto is price`.

When the operand is a Rule reference (`validated_price is veto`), the test reads that Rule's already-computed result. When the operand is a compound expression (`price * quantity is veto`), that expression is evaluated and the test is true when the result is a Veto. To test a single failing operand inside a sum or product, apply `is veto` to that operand (`b is veto`) rather than to the whole expression (`a + b is veto`).

You can Veto again based on a test: `unless x is veto then veto "outer"`. The Rule's result message is then `"outer"`; explanations may still show the inner Veto beneath the `is veto` operand.

`veto` and `veto "message"` are only valid as a Rule or Unless result. See [Special expressions in the language reference](../reference/readme.md#special-expressions).

### Veto vs Error vs Panic

Lemma distinguishes three outcomes:

| Outcome | When | Example |
|---------|------|---------|
| Planning Error | Invalid Spec (wrong types, unsupported operations) | `5 and "text"` (logical AND requires boolean operands); `1 / 0` (literal division by zero) |
| Request Error | Malformed evaluate request (before evaluation) | Duplicate evaluation data keys that canonicalize to the same name (`Age` and `age`) |
| Veto | Domain "no value" | Division by zero from Data, missing Data, invalid Data override, user `veto "..."`, date overflow |
| Panic | Bug (invariant violated; should never happen after planning) | Internal consistency failure |

After planning succeeds, a well-formed evaluate completes with Rule results (values or Vetoes). Data overrides that violate type constraints, minimum/maximum bounds, or allowed options bind as a Veto on that Data (dependent Rules veto); they are not a planning error. Unknown evaluation data keys and import aliases are ignored; a `MissingData` veto may suggest a near match from ignored keys. Duplicate canonical keys in the same request are a request Error and abort before evaluation. Numbers accept an optional sign, underscores, and an exponent (`1e3`).

Veto is only for domain-level "no value", not for type errors or invalid operations in the Spec itself. Those are caught at planning time.

## Next up

[Extending Data](extending_data.md): parent types, Data commands, and reuse across Specs.

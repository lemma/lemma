---
nav_title: Reference
nav_order: 40
---

# Lemma Language Reference

Lemma syntax is meant to be hard to misuse. It favours readable keywords over punctuation, ignores insignificant whitespace (indentation is for people; newlines are optional), and treats identifiers as case-insensitive.

## Building blocks

Lemma can be written in `.lemma` files however you like. The CLI ignores file names and reads only what those files contain. A file may hold one `spec` or many, optionally organized under a `repo`. Inside a spec, after an optional commentary, `data`, `uses` (optional `  -> with` bindings), `rule`, and `meta` may appear in any order.

### `repo`

A `repo <name>` declaration groups the specs that follow, until the next `repo` line. You can leave out a `repo` statement and the specs in your file join the default repository with everything else that has no name. The name `lemma` is reserved for the embedded units stdlib (`uses lemma units`). See [Composing specs: Repositories](../learn/composing_specs.md#repositories).

### `spec`

`spec <name>` starts a namespace for data and rules. An optional effective datetime on the same line (`spec pricing 2026-01-01`) places that body on a temporal timeline. A name is one opaque token of `[A-Za-z0-9_./-]` characters (`spec employee/contract`, `spec nl.tax.brackets`); registry qualifiers add a leading `@` (`@iso/countries`). Versioning is temporal only: there is no version tag in the name. A file may contain several specs.

### Commentary

The only documentation allowed inside source is a triple-quoted commentary block immediately after the `spec` line (after an optional effective datetime). There is no inline comment syntax like `#`, `//`, or `--`, to keep specs readable and avoid comments that may contradict the implementation. The goal is a single source of truth, after all.

```lemma
spec exams 2026-01-01
"""
This spec determines graduation.
"""

data score: number

rule graduates:
  score > 5
```

Commentary after `data`, `rule`, or any other keyword is a parse error.

### `meta`

`meta` attaches documentation metadata to a spec: citations, gazette references, case law, and similar links that explain where a rule comes from. Values are not used in evaluation. They appear on `show.meta`.

```lemma
spec overtime_pay 2004-08-02
"""
Maximum average weekly working time.
"""
meta title: "Working time: weekly average"
meta directive: "Directive 2003/88/EC, Article 6"

data hours_per_week: number

rule within_limit:
  hours_per_week <= 48
```

The right-hand side is a literal. Duplicate meta keys fail planning.

### `data`

`data` declares every input slot and every named type in the spec: a literal value, an open typed input, or a parent type other data can extend. Constraints and units chain with `->`. Details are under [Data](#data) and [Extending data](#extending-data).

### `rule`

A `rule` is a named computed value. Rules may reference data and other rules by name; the engine resolves dependencies. Conditional overrides use `unless` (next section).

## Unless clauses

A rule starts with a default expression and can optionally be overridden by any `unless <condition> then <result>` clauses. As the natural language implies, the last matching clause wins. Put general cases first and specific overrides later. This mimics how law, policies and contracts are often written down: in principle X applies, unless in specific case Y, then Z.

```lemma
spec order_discount

data quantity: number
data is_vip:   false

rule discount:
  0%
  unless quantity >= 10 then 10%
  unless quantity >= 50 then 20%
  unless is_vip         then 25%
```

Overrides stay flat: later rules can change outcomes by conditioning on earlier rules. There is no nesting and no encapsulation. There is no `or` operator; disjunction is expressed with unless chains or separate boolean rules. More patterns: [Conditional logic](../learn/conditional_logic.md).

The example’s layout makes the `unless` chain read as a flat decision table. Last-match-wins removes the need for explicit negations, which keeps the table short and clear.

## Operators



### Arithmetic


| Operator | Description                                   | Example            |
| -------- | --------------------------------------------- | ------------------ |
| `+`      | Addition                                      | `price + tax`      |
| `-`      | Subtraction                                   | `total - discount` |
| `*`      | Multiplication                                | `price * quantity` |
| `/`      | Division (exact rational)                     | `total / count`    |
| `mod`    | Remainder, floored (sign follows the divisor) | `value mod 10`     |
| `^`      | Exponentiation                                | `base ^ exponent`  |


**Modulo**: `a mod b` is floored: `a == floor(a / b) * b + (a mod b)`. Negative operands: `-7 mod 3` = `2`, `7 mod -3` = `-2`. `%` is the percent suffix only (`50%`), so a number before `%` is a percent literal and `7 % 3` is a parse error.

**Rounding operators** (`round`, `floor`, `ceil`) on measures operate on the magnitude in the operand's declared unit. `round(x as gram)` rounds the gram magnitude, not the original unit's.

### Comparison


| Operator      | Description          | Example                       |
| ------------- | -------------------- | ----------------------------- |
| `>`           | Greater than         | `age > 18`                    |
| `<`           | Less than            | `price < 100`                 |
| `>=`          | Greater or equal     | `score >= 70`                 |
| `<=`          | Less or equal        | `weight <= 50`                |
| `is`          | Equal                | `status is "approved"`        |
| `is not`      | Not equal            | `status is not "cancelled"`   |
| `is veto`     | Operand has no value | `validated_price is veto`     |
| `is not veto` | Operand has a value  | `validated_price is not veto` |


Equality is `is` **/** `is not` **only**. There is no `=`, `==`, or `!=`.

See [Veto](../learn/types_and_units.md#veto) to learn more about `veto` with forms and examples.

### Logical


| Operator | Description | Example                       |
| -------- | ----------- | ----------------------------- |
| `and`    | Logical AND | `is_valid and not is_blocked` |
| `not`    | Logical NOT | `not is_suspended`, `not(score >= 70)` |


There is no `or` operator. Express disjunction with `unless` chains or separate boolean rules. Within one condition, use `and` and `not`.

`and` is order-independent. A `false` operand decides the result whatever the other operands are, so a missing or vetoed sibling does not matter. Without a `false` operand, the first missing or vetoed operand in source order decides the result.

### Mathematical

Prefix operators (and `not`) take their argument in one of two forms. **Call form** `floor(x)` (a space before `(` is allowed): the argument ends at the matching `)`, and operators after it apply to the result, so `floor(x) + 1` adds to the floored value and `floor (x + 1)` floors the sum. **Bare form** `floor x`: the argument is exactly one operand (a literal, reference, `->` chain, range literal, parenthesised expression, or another bare prefix operator such as `floor abs x`), and only `->`, `then`, `unless`, `)`, `,` or the end of the expression may follow it. `floor x + 1`, `floor x as gram`, `sqrt x ^ 2`, `not x > 3` and `not a and b` are parse errors that suggest both readings (`floor(x + 1)` or `floor(x) + 1`). A bare argument never starts with a sign: write `abs(-1)`. The formatter writes the bare form when it is allowed and the call form otherwise. `sqrt`, `sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `log`, and `exp` require a **number**. `abs`, `ceil`, `floor`, and `round` also accept a **measure**: the unit is preserved and the operator applies to the magnitude in the operand's bound unit. `lower` and `upper` require a **range** (see [Range endpoints](#range-endpoints)).


| Operator | Description                     | Example                         |
| -------- | ------------------------------- | ------------------------------- |
| `sqrt`   | Square root                     | `sqrt(value)` or `sqrt value`   |
| `sin`    | Sine                            | `sin(angle)` or `sin angle`     |
| `cos`    | Cosine                          | `cos(angle)` or `cos angle`     |
| `tan`    | Tangent                         | `tan(angle)` or `tan angle`     |
| `asin`   | Inverse sine                    | `asin(value)` or `asin value`   |
| `acos`   | Inverse cosine                  | `acos(value)` or `acos value`   |
| `atan`   | Inverse tangent                 | `atan(value)` or `atan value`   |
| `log`    | Natural logarithm               | `log(value)` or `log value`     |
| `exp`    | Exponential                     | `exp(value)` or `exp value`     |
| `abs`    | Absolute value (quantities too) | `abs(value)` or `abs value`     |
| `floor`  | Round down (quantities too)     | `floor(value)` or `floor value` |
| `ceil`   | Round up (quantities too)       | `ceil(value)` or `ceil value`   |
| `round`  | Round nearest (quantities too)  | `round(value)` or `round value` |
| `lower`  | Smaller endpoint of a range     | `lower(5...10)` or `lower period` |
| `upper`  | Larger endpoint of a range (excluded) | `upper(5...10)` or `upper period` |




### Type cast (`as`)

Chained casts nest from left to right: `expr as unit as unit … as number`.


| Form        | Description                                                                         | Example                                      |
| ----------- | ----------------------------------------------------------------------------------- | -------------------------------------------- |
| `as <unit>` | Convert, relabel, or construct a measure/ratio in that unit                         | `mass as gram`, `5 as eur`, `rate as hour`  |
| `as number` | Strip to raw magnitude (requires explicit unit on prior step for quantities/ranges) | `10 eur as number`, `span as day as number` |
| `as time`   | Time-of-day from a `date` (or identity on `time`); date-only yields midnight        | `now as time`, `2024-06-01 as time`          |

`<unit>` may be bare (`eur`) or qualified (`money.eur`, `units.mass.kilogram`). Optionally qualify; must qualify when the bare name is ambiguous in scope: see [Qualifying units](#qualifying-units).

Same-family conversion applies factors (`2 kilogram as gram` → `2000 gram`). Cross-family relabel keeps magnitude (`5 eur as kg` → `5 kg`).

`as` binds tighter than `*`, `/`, and `mod`. In `balance / rate as month`, the `as month` applies to `rate`, not to the quotient. To convert the result of an arithmetic expression, use parentheses: `(balance / rate) as month`.

Literal quantities and ratios carry an explicit unit, so `10 eur as number` and `25% as number` plan without an extra unit step. Named data references need the full chain: `amount as eur as number`, not `amount as number`.

Date, measure, and **time** ranges cannot use bare `as number` (unit is ambiguous). Project span width with `as <unit> as number`, e.g. `start...end as day as number`, `(09:00...17:00) as hour as number`, `(30 kilogram...35 kilogram) as stone as number`. Calendar span units are singular `year` and `month`. Duration span units (`day`, `hour`, `second`, …) require `uses lemma units` when not declared locally.

Number ranges allow bare `as number` (span width with no unit).

### Precedence

Lower binds looser. Use parentheses when an operand would otherwise bind too tightly.


| Level | Constructs |
| ----- | ---------- |
| Lowest | `and` |
| | `not` (see [Mathematical](#mathematical) for its argument) |
| | Comparisons (`>`, `<`, `>=`, `<=`, `is`, `is not`), `in`, date predicates, `is veto` |
| | `+`, `-` |
| | `*`, `/`, `mod` |
| | `^` |
| | `as` |
| | Prefix operators (`sqrt`, `floor`, `lower`, `upper`, …): call form `floor(x)`, or one bare operand |
| | `...` (range literal) |
| Highest | Atoms, `veto`, `now`, `past`/`future` windows |


So `balance / rate as month` parses as `balance / (rate as month)`, and `rate * a...b` parses as `rate * (a...b)` because `...` binds tighter than `*`. Write `(rate * a)...b` when the product should be a range endpoint.

`...` also binds tighter than a prefix operator, so `lower 5...10` is the lower end of `5...10`, not a range from `lower 5`. `past` / `future` are not prefix operators: `past 7 day + 1 day` still includes the sum.

## Using other Specs

Specs are composed with `uses` and `  -> with` bindings under each import. There is no inheritance. For unpinned versus pinned imports, temporal slices, coverage, and interface checks, see [Composing specs](../learn/composing_specs.md).

Write `uses spec_name` to import with an alias equal to the full target name, or `uses alias: spec_name` for an explicit alias. Add an effective datetime after the target to pin that edge (`uses spec_name 2025-01-01`, or a bare year `YYYY` for that year's Jan 1 00:00). Versioning is temporal only: multiple rows of the same name with different `effective_from` datetimes.

### Temporal versions

The same spec **name** may appear several times with different effective datetimes. Each row is immutable; you add a new row on the timeline instead of editing history in place.

```lemma
spec points

data base_points: 100
```

```lemma
spec points 2025-01-01

data base_points: 120
```

```lemma
spec order

uses p: points

rule total:
  p.base_points
```

Evaluating `order` before 2025-01-01 uses `base_points: 100`; from 2025-01-01 onward, `120`. See [Composing specs](../learn/composing_specs.md) for unpinned vs pinned imports.

### Pinning and evaluation instant


| Mechanism         | Syntax                                                                        | Effect                                                            |
| ----------------- | ----------------------------------------------------------------------------- | ----------------------------------------------------------------- |
| **Spec row**      | `spec points 2025-01-01`                                                      | Declares a body effective from that datetime                      |
| **Pinned import** | `uses f: finance 2025-06-01` or `uses f: finance 2025`                        | Locks the dependency (and its transitive imports) to that instant |
| **Evaluation instant**   | `lemma evaluate points --effective 2025-03-01` (CLI) or **Accept-Datetime** (HTTP) | Picks which temporal row of the **root spec** is active           |


Bare year on a pin (`2025`) means that year's Jan 1 00:00, same as datetime literals. The evaluation instant (`--effective` / Accept-Datetime) always resolves to a precise datetime: date-only forms become UTC midnight at the start of that period (`2025` → `2025-01-01T00:00:00Z`, `2025-03-01` → `2025-03-01T00:00:00Z`). HTTP details for Accept-Datetime, `Vary`, and `Memento-Datetime`: [CLI: API Defaults](cli.md#api-defaults).

### Self-reference restriction

A spec may depend on an **earlier temporal body with the same base name** when the
reference resolves to a **different** `effective_from` row (planning compares spec
bodies by identity, not by bare name):

```lemma
spec finance 2026-01-01

data rate: 1
```

```lemma
spec finance 2027-01-01

uses finance 2026-01-01

rule ok:
  finance.rate
```

Both `uses finance 2026-01-01` (implicit alias) and `uses prev: finance 2026-01-01`
(explicit alias) are valid. Qualified access uses the import alias (`finance.rate` or
`prev.rate`).

Planning **rejects** a reference that resolves to the **same** spec body, for example
`spec finance` with `uses finance`, or `spec finance 2026-01-01` with
`uses finance 2026-01-01`. Dependency cycles across temporal rows (for example 2026
depending on 2027 while 2027 depends on 2026) are rejected as spec dependency cycles.

### LemmaBase references

LemmaBase repositories use the `@` prefix:

```lemma
spec ledger_spec

uses iso: @iso/countries alpha2

data country: iso.code
```

### Cross-repo references

Local multi-repo workspaces qualify the target with a repository name (no `@`):

```lemma
repo accounting

spec invoice

data amount: number
```

```lemma
repo billing

spec statement

uses inv: accounting invoice

rule total:
  inv.amount
```

`uses inv: accounting invoice` imports `invoice` from the `accounting` repository. LemmaBase imports keep the `@` form (`uses iso: @iso/countries alpha2`). See [Composing specs: Repositories](../learn/composing_specs.md#repositories).

### Unpinned vs pinned imports

An **unpinned** import (`uses p: policy`) follows the dependency's timeline: planning may split the consumer into temporal slices at dependency `effective_from` boundaries, and resolved values can change when the dependency gains a new row. A **pinned** import (`uses f: finance 2025-06-01`) freezes that edge (and its transitive imports) to the body active at that instant. Coverage gaps and interface compatibility across slices are checked at planning: details in [Composing specs](../learn/composing_specs.md#unpinned-vs-pinned-uses).

### `uses` and imported types

Bring another spec into scope with `uses <alias>: <target>` (optional effective datetime on the target). Reference data types defined there as a parent type with the alias: `data x: alias.name`. A bare name that an import declares is a planning error naming the qualified form(s). Temporal pins belong on the `uses` line.

```lemma
spec base_types

data currency: text
  -> option "EUR"
  -> option "USD"
```

```lemma
spec pricing

data rate: ratio
  -> maximum 100 percent
```

```lemma
spec product_pricing

uses base: base_types

uses rates: pricing

data currency: base.currency
data discount_rate: rates.rate
  -> maximum 50 percent
```

Pin which temporal version of a dependency applies for that edge:

```lemma
spec finance 2026-01-01

data money: measure
  -> unit eur: 1.00
```

```lemma
spec accounts

uses fin: finance 2026-01-15

data wallet: fin.money
```

These edges participate in temporal slicing: the engine creates slice boundaries when a dependency has multiple temporal versions and the consumer references it without a per-edge pin.

### Setting data on an imported spec (`uses` block)

Under a `uses` line, `  -> with path: value` sets a literal or reference on a **data slot of the imported spec**. The path is relative to that import (no alias prefix). Several pairs on one line are comma-separated (`-> with name: "Ada", monthly_salary: 7_500`). Standalone `with alias.field: …` is a parse error; nest under `uses`. Use `data` for local slots.

Constraint chains (`-> ...`) on binding values are not allowed; they belong on `data`.

```lemma
spec base_employee

data name:           text
data monthly_salary: number
```

```lemma
spec specific_employee

uses employee: base_employee
  -> with name: "Alice Smith"
  -> with monthly_salary: 7_500

rule employee_summary:
  employee.name
```

### A rule can return a spec instance

A rule body may be a `uses` alias. The result is that instance. Dot on the rule reads the chosen instance's data or rules, the same way dot reads a `uses` alias. Every `unless` branch must be the same spec (the same resolved version). A spec instance is not a number or text: arithmetic, comparison, and `with` reject it. `is veto` is allowed.

```lemma
spec tax_bracket

data rate: ratio


spec income_tax

uses basic: tax_bracket
  -> with rate: 36%

uses higher: tax_bracket
  -> with rate: 37%

data high_income: boolean

rule top_bracket:
  basic
  unless high_income then higher

rule marginal_rate:
  top_bracket.rate
```

The API result is the whole instance: spec name, resolved effective date, every data value as evaluation-data text, and every rule result. Evaluating the spec again with those data values at the same effective instant reproduces the rule results. A bare alias counts as reading the whole imported spec, so an unpinned `uses` fails planning if any data or rule of that spec changes type across versions.

Read imported data or rules in expressions without `with` when you do not need to override values:

```lemma
spec inner

data x: 1
```

```lemma
spec outer

uses inner

rule r:
  inner.x
```



### Scenario parameters (same import, different values)

```lemma
spec pricing

data price:    100
data discount: 0%

rule final_price:
  price * (100% - discount)
```

```lemma
spec scenarios

uses retail: pricing
  -> with discount: 5%

uses wholesale: pricing
  -> with discount: 15%
  -> with price: 80

rule retail_final:
  retail.final_price

rule wholesale_final:
  wholesale.final_price
```



### Binding with a local source

When the RHS is a name in the enclosing spec (not a dotted import path), the LHS must still be an import path:

```lemma
spec inner

data slot: number
  -> minimum 0
  -> maximum 100
```

```lemma
spec outer

uses i: inner
  -> with slot: src

data src: number
  -> suggest 42

rule r:
  i.slot
```

By contrast, `data x: someident` declares `x` with `someident` as its parent type: it points at another `data` declaration rather than copying that declaration's value.

## Primitive types

Lemma provides these primitive types:

- `boolean` - true/false values
- `number` - dimensionless numeric values (no units)
- `number range` - half-open numeric intervals
- `measure` - numeric values with units (mass, money, time periods via `units.duration`, etc.)
- `measure range` - half-open intervals with measure endpoints in one unit family
- `text` - string values
- `date` - ISO 8601 dates
- `date range` - half-open date/datetime intervals
- `time` - time values
- `time range` - half-open time-of-day intervals (`09:00...17:00`)
- `ratio` - proportional values (percent, permille)
- `ratio range` - half-open ratio intervals

Numbers are stored and computed as **exact rationals** (ℚ); API output is a **decimal string**. See [Numeric precision](../learn/precision.md).

### Number literals

- Integers and decimals: `42`, `3.14`, `75_000` (underscores are digit separators)
- Scientific notation: `1.23e10`, `5e-3`

## Ranges

Ranges express **half-open** intervals: **lower inclusive, upper exclusive**. Containment uses `in`:

```lemma
spec membership_bands

uses lemma units

data age:   25 year
data score: 50
data event: 2024-03-15

rule adult:
  age in 18 year...67 year

rule in_band:
  score in 0...100

rule in_period:
  event in 2024-01-01...2024-06-15
```

At the upper bound, `in` is false (`67 year` is not inside `18 year...67 year`).

### Range kinds


| Type            | Endpoints                                                   | Typical use                               |
| --------------- | ----------------------------------------------------------- | ----------------------------------------- |
| `date range`    | Dates or datetimes (`2024-01-01...2024-06-15`)              | Employment periods, billing windows       |
| `time range`    | Times (`09:00...17:00`)                                     | Business hour, shift windows             |
| `number range`  | Dimensionless numbers (`0...100`)                           | Scores, tiers                             |
| `measure range` | Quantities in one unit family (`30 kilogram...35 kilogram`) | Weight bands, duration bands, money bands |
| `ratio range`   | Ratios (`0%...50%`)                                         | Allowed discount bands                    |


Any **rangeable named measure type** can also be declared with a `range` suffix, e.g. `data estimated: money range` or `data band: units.calendar range -> suggest 18 year...67 year`.

**Calendar intervals** use month/year units from `uses lemma units` (`units.calendar`). Inline literals (`18 year...67 year`) and `in` work like other ranges. Endpoints must be calendar units (`year`, `month`). Do not mix calendar and duration units in one literal (`12 year...7 day` is a planning error). Do not mix dates and calendar endpoints (`2024-01-01...18 year` is rejected).

**Time ranges** are half-open like all ranges. Endpoints must share the same timezone (including both absent). Literal order does not imply midnight wraparound: `22:00...02:00` is ordered `[02:00, 22:00)` with an 20-hour span, not an overnight window. A `date` may be tested with `in` a time range: the date's clock is projected the same way as `date as time` (so `now in shift` equals `now as time in shift`).

Declare range slots on `data`:

```lemma
spec range_slots

uses lemma units

data band: units.calendar range
  -> suggest 18 year...67 year

data window: time range
  -> suggest 09:00:00...17:00:00

data period: date range
  -> suggest 2024-01-01...2024-12-31

data tier: number range
  -> suggest 0...100
```

**Data commands** on range types: `help`, `suggest`. Ranges take no `unit` rows: units belong to a `measure` or `ratio` type, and a range over it is written `<type> range`.

### Span: `(lo...hi) as <unit>`

Parentheses around a range literal or expression, then `as`, yield the **width** of the interval in the target unit (a scalar), not another range:

```lemma
spec range_spans

uses lemma units

rule days_between:
  2024-06-01...2024-06-15 as day

rule width_kg:
  30 kilogram...35 kilogram as kilogram

rule span_years:
  1990-05-20...2024-06-15 as year
```


| Range type                                                         | `as` targets (examples)                                        | Notes                                                                 |
| ------------------------------------------------------------------ | -------------------------------------------------------------- | --------------------------------------------------------------------- |
| **date range**                                                     | `day`, `month`, `year`, duration units, calendar units      | Calendar-aware where applicable                                       |
| **time range**                                                     | Duration units only (`hour`, `minute`, `second`, …)         | No calendar units; bare `as number` rejected                          |
| **number range**                                                   | `number`, duration units                                       |                                                                       |
| **measure range**                                                  | Same-family measure units (mass, money, duration, calendar, …) | Cross-family span (e.g. mass range `as day`) is rejected at planning |
| **ratio range**                                                    | Units of the element ratio type (`percent`, …)                 |                                                                       |
| **calendar interval** (inline literal or `units.calendar` suggestion) | Same-family calendar units                                     | Span uses month/year arithmetic                                       |




### Range arithmetic and comparison

Ranges support comparison and arithmetic consistent with their kind.

`+` and `-` on the **right endpoint** are part of the range literal (endpoint extension):

```lemma
spec range_arithmetic

uses lemma units

data start:  date
data length: units.calendar

rule long_enough:
  2024-06-01...2024-06-15 >= 7 day

rule shifted:
  18 year...(67 year + 2 year)

rule extended:
  2024-01-01...(2024-06-15 + 1 month)

rule membership:
  now in start...(start + length)

rule span_days:
  2024-02-15...2024-03-15 as day + 3 day
```

**Span arithmetic** (adding to the width of an interval, not shifting an endpoint) requires parentheses around the range, as in `span_days` above. The `span_days` rule converts the span from a range to a duration, and then adds one day (span_days: 32 day)

### Range endpoints

`lower` and `upper` are prefix operators. `lower` returns the smaller endpoint (included). `upper` returns the larger endpoint, which is still excluded: `upper(18 year...67 year)` is `67 year`, and that value is not `in` the range.

Written order is not the stored order. `lower(10...5)` is `5` and `upper(10...5)` is `10`, the same order `in` uses.

The result has the endpoint's type. A `date range` gives a `date`, a `time range` a `time`, a `number range` a `number`. A `money range` gives `money`, and the endpoint keeps the unit it was written in (`lower(10 eur...40 eur)` is `10 eur`).

```lemma
spec employment

data period: date range

rule starts:
  lower period

rule ends:
  upper period
```

`lower` and `upper` are reserved names. They remain valid as the constraint commands `-> lower` and `-> upper`, which limit which endpoints a range slot may hold. Those commands are not the operators.

Date endpoints can be built from separate `date` values: `hire_date...today`.

For duration quantities, import SI types with `uses lemma units` so literals like `8 hour` and `7 day` resolve. Declare duration slots with `data elapsed: duration` (or qualified `units.duration`). Calendar periods (`25 year`, `18 year...67 year`) use `calendar` (or `units.calendar`).

## Lists

A list is an ordered sequence of rows of one type. See [Lists](../learn/lists.md) in the learn guide for a walkthrough.

### Declarations

| Form | Rows | Overridable |
|------|------|-------------|
| `data x: V list` | values of data type `V` (`money list`, `date range list`) | yes, as a whole |
| `uses x: spec list` | instances of `spec` | yes, as a whole |
| `rule x: V list -> with ...` | fixed values | no |
| `rule x: <list expression>` | computed | no |

- `list` follows the complete Spec reference, pin included: `uses brackets: tax_bracket 2024 list`, `uses rates: @carrier/tariffs rate 2025-06-01 list`.
- `data x: spec list` is a planning error that suggests `uses x: spec list`.
- A list without rows is empty. A list is never missing data.
- In a type position, a list alias names the row Spec's types: `data ordered: menu.drink`. In an expression, a dot on a list alias is allowed only inside a command argument, where it names the current row.

### Rows: `-> with`

```lemma
spec rows

data money: measure
  -> unit eur: 1.00

data fares: money list
  -> with 89 eur, 120 eur
  -> with 64 eur
```

- On a value list, `-> with` takes comma-separated literals.
- On an instance list, `-> with field: value, field: value` adds one inline row. Fields are paths relative to the row Spec (`product.price`). A row may also continue over several lines after each comma.
- `-> with ada, grace` adds named instances, declared with `uses`, as rows. One `-> with` cannot mix field pairs and references.
- `-> with` only appends; it never replaces a row. In a Rule, `fares -> with 64 eur` appends to the current value of `fares`.
- A Spec may declare a list of its own rows (`uses components: part list` inside `spec part`). Such a self-list has no source rows. Single-instance self-reference stays an error.

### Keys

- `-> key field, field` (instance lists only, before the first `-> with`) identifies rows by those fields of the row Spec. Dotted key fields are an error.
- Without `-> key`, rows are numbered `1` to `n` in row order. Value lists are always numbered.
- In source rows, a duplicate key, or overlapping ranges on a range key, is a planning error. When a key field has options, each row key must be one of those options; options need not all have a row.
- In evaluation-data rows, a duplicate key, an invalid option or overlapping ranges vetoes the whole list.
- `find` with no matching row vetoes. A range key matches by containment, with unit conversion as `in` does.

### Commands

List commands are postfix `->` expressions in Rules and `unless` results. On a `data` or `uses` declaration they are a planning error. A command takes its argument bare when the argument is one operand (`staff -> sum bonus`). Otherwise it uses the call form (`lines -> sum(qty * price)`), with several arguments comma-separated inside the parentheses. `->` binds tighter than every binary operator.

| Command | Value list | Instance list | Result |
|---------|-----------|---------------|--------|
| `sum`, `avg` | no argument | expression | number, measure or ratio |
| `product` | no argument | expression | number or ratio |
| `min`, `max` | no argument | expression | ordered value |
| `count` | no argument | no argument | number |
| `any`, `all` | `op value` | condition | boolean |
| `where` | `op value` | condition | same list type |
| `sort [asc\|desc]` | no argument | expression | same list type, stable |
| `top n` | `n` | `n` | same list type, first `n` rows |
| `find` | row number | one value per key field | row |
| `first`, `last` | no argument | no argument | row |
| `with` | values | field pairs | same list type |

- A value-list condition is exactly one comparison with the row value as left operand: `fares -> where >= 100 eur`, `fares -> where in 100 eur...200 eur`, `codes -> any is "A1"`.
- In an instance-list argument, the row's Data and Rules and the enclosing Spec's names are in scope. A name defined on both sides must be qualified with the list name (`components.quantity`).
- `top n` takes a literal or a reference of type `number`. A reference that is not a whole number of at least 0 vetoes.
- Row values are read with a dot: `selected_drink.price`, `(staff -> sort salary desc -> first).bonus`.
- Veto: `avg`, `product`, `min`, `max`, `first`, `last` on an empty list; `find` with no row. If any row vetoes, the reduction or filter vetoes with the first such row's reason. `sum` over no rows is 0, `count` is 0, `any` is false, `all` is true, `where` gives an empty list.

### Reserved keywords

`list`, `key`, `find`, `first`, `last`, `sum`, `avg`, `product`, `count`, `min`, `max`, `any`, `all`, `where`, `sort`, `top`, `asc`, `desc`, `is`, `past`, `future`, `permille` and `mod` are reserved in any case. They cannot be data, rule, spec, alias or unit names.

### Evaluation data and results

- A list is one input key with an array that replaces every source row: literal strings for a value list, objects keyed by row data names for an instance list. Per-row keys such as `staff.2.salary` are request errors. `name=value` and form bodies cannot carry a list.
- Missing row fields are reported by row number: `staff.2.salary`, `components.1.components.2.own_cost`.
- A list result has `list` (one value per row). A row result has `row` (`spec`, `effective`, `instance`, `index`); `index` points into the response's top-level `rows`, which lists each referenced row once with its `data` and `rules`.
- Show types have `kind: "list"` with `element` and `key` (`"numbered"` or `{"fields": [...]}`).

## Data

Every input slot and every named type is declared with `data`. The right-hand side is either a **literal value** or a **type** (a primitive like `number`, or another data name as parent type). Constraints chain with `->`.

### Values

A literal **fills** the slot and Lemma infers the type. Callers may override at evaluation time; interactive UIs typically skip review for filled fields (including literal `uses` block `  -> with` bindings on templates):

```lemma
spec employment

uses lemma units

data name:       "Alice"
data age:        35
data start_date: 2024-01-15
data tax_rate:   15%
data is_manager: true
data workweek:   40 hour
data salary:     75_000
```



### Open inputs

Declare a type without a value to leave the slot open for evaluation. Add constraints on the same declaration. Use `-> with` to set a committed value on a typed slot (same evaluation behavior as a literal; callers may override). Use `-> suggest` for a **suggestion** (UIs may offer it; Enter to accept); suggestions do not commit — the engine still requires the value unless `-> with` or a literal is present. `-> with` and `-> suggest` cannot appear on the same declaration.

```lemma
spec loan_application

data birth_date: date
data rating: number
  -> minimum 0
  -> maximum 100
  -> with 50

data status: text
  -> option "active"
  -> option "inactive"
  -> suggest "active"

data amount: measure
  -> unit eur: 1.00
  -> unit usd: 0.91
  -> decimals 2
```

A literal on the right-hand side fills the slot. A type alone leaves it open. `-> with <value>` fills a typed slot. `-> suggest` is only a suggestion for callers or UIs; it does not fill and so the engine still requires the value to be provided.

Constraints such as `-> minimum`, `-> maximum`, `-> option`, `-> unit`, `-> decimals`, `-> with`, `-> suggest`, and `-> help` depend on the primitive: see [Data commands](#data-commands). At evaluation, an open input with no supplied value yields a missing-data veto when the rule needs it; a bad value (wrong type, failed constraint, disallowed option, excess decimals) vetoes that data. Duplicate names that canonicalize to the same identifier (for example `Age` and `age`) are a request error.

### Named types

When the right-hand side is a primitive or another data name (not a literal), the declaration defines a **named data definition** that other data can extend:

```lemma
spec warehouse

data mass: measure
  -> unit kilogram: 1.0
  -> unit pound: 0.453592

data weight:         mass
data package_weight: 75 kilogram
```

`data weight: mass` is an open input whose parent is `mass`. `data package_weight: 75 kilogram` is a value whose parent is inferred as `mass`.

## Extending data

A `data` declaration whose right-hand side is a primitive or another data name can carry `->` constraints. Other data extend it by naming it on the right-hand side.

### Example: money

```lemma
spec money_type

data money: measure
  -> unit eur: 1.00
  -> unit usd: 0.91
  -> decimals 2
  -> minimum 0 eur
```

A data declaration can also extend another data name:

```lemma
spec extended_types

data money: measure
  -> unit eur: 1.00
  -> unit usd: 0.91

data price: money
  -> minimum 0 eur
```

On `measure` parents, a child `-> unit` may **add** new units. Redeclaring a unit already inherited from the parent with a **different** factor or decomposition is a planning error (same as `ratio`); an identical redeclare is allowed.

### Data commands

Each `->` row on a `data` declaration is a **data command**. Built-in primitives ship default `help` text (override with `-> help "…"`):


| Type            | Default help                                                     |
| --------------- | ---------------------------------------------------------------- |
| `boolean`       | Whether this holds (true or false).                              |
| `number`        | A dimensionless number.                                          |
| `number range`  | The lower and upper bound of the number range.                   |
| `text`          | A text value.                                                    |
| `measure`       | A numeric amount in one of this type's units.                    |
| `measure range` | The lower and upper bound of the measure range. |
| `ratio`         | A ratio in one of this type's units (e.g. percent).              |
| `ratio range`   | The lower and upper bound of the ratio range.                    |
| `date`          | A date, or a date and time with optional timezone.               |
| `date range`    | The start date and end date of the date range.                   |
| `time`          | A time of day, with optional timezone.                           |
| `time range`    | The start time and end time of the time range.                   |


**For** `measure` **and** `number`**:**

- `unit <name> <value>` - Define a unit (measure only; see [Compound units](#compound-units) for derived forms). Literals and `as` targets that use the unit follow [Qualifying units](#qualifying-units).
- `decimals <n>` - Set decimal precision (0-255)
- `minimum <value>` - Set minimum value
- `maximum <value>` - Set maximum value
- `help "<text>"` - Add help text
- `with <value>` - Fill the slot with a value (committed; callers may override). Cannot combine with `suggest` on the same declaration
- `suggest <value>` - Set suggestion value (UI hint; does not commit)

**For** `ratio`**:**

- `unit <name> <value>` - Define custom ratio units
- `minimum <value>` - Set minimum value
- `maximum <value>` - Set maximum value
- `help "<text>"` - Add help text
- `with <value>` - Fill the slot with a value (committed; callers may override). Cannot combine with `suggest` on the same declaration
- `suggest <value>` - Set suggestion value (UI hint; does not commit)

In the **show JSON**, `measure` and `ratio` types expose type-level `minimum` / `maximum` in **canonical** form (same as evaluation). Each `units[]` entry may also include `minimum`, `maximum`, and `suggestion` as magnitudes in that unit, so a UI can bind to them without converting on the client.

**For** `text`**:**

- `option "<value>"` - Add a single allowed option
- `options "<value1>", "<value2>", ...` - Add multiple allowed options
- `length <n>` - Exact string length
- `help "<text>"` - Add help text
- `with "<value>"` - Fill the slot with a value (committed; callers may override). Cannot combine with `suggest` on the same declaration
- `suggest "<value>"` - Set suggestion value (UI hint; does not commit)

**For** `date` **and** `time`**:**

- `minimum <value>` - Minimum date/time
- `maximum <value>` - Maximum date/time
- `help "<text>"` - Add help text
- `with <value>` - Fill the slot with a value (committed; callers may override). Cannot combine with `suggest` on the same declaration
- `suggest <value>` - Set suggestion value (UI hint; does not commit)

**For** `date range`**,** `number range`**,** `measure range`**,** `time range`**, and** `ratio range`**:**

- `lower <value>` - Minimum left endpoint (element type)
- `upper <value>` - Maximum right endpoint (element type)
- `minimum <value>` - Minimum span width (see span table; not an endpoint envelope)
- `maximum <value>` - Maximum span width
- `help "<text>"` - Add help text
- `with <lo>...<hi>` - Fill the slot with an interval (half-open; committed). Cannot combine with `suggest` on the same declaration
- `suggest <lo>...<hi>` - Suggested interval (half-open; UI hint)

On ranges, `minimum` / `maximum` always mean **width**. Endpoint limits use `lower` / `upper`. Measure and ratio range bounds may use any declaring unit of the type (same mixed-unit model as scalar measure min/max). Date range width may be duration or calendar, but not both on one type; time range width is duration only.

Ranges take no `unit` rows. Endpoints of a `measure range` share one unit family, the family of its element type.

**For** `boolean`**:**

- `help "<text>"` - Add help text
- `with <value>` - Fill the slot with a value (committed; callers may override). Cannot combine with `suggest` on the same declaration
- `suggest <value>` - Set suggestion value (UI hint; does not commit)



### Qualifying units

Qualify a unit when its bare name is ambiguous in scope; otherwise qualification is optional.

| Form | Meaning |
|------|---------|
| `kilogram` | Bare: the one type in scope that declares the unit |
| `my_weight.kilogram` | Extension (or owning) type |
| `units.mass.kilogram` | Import alias + type |

Several independent types may declare the same unit name, locally or through imports (`data bag: measure -> unit kilogram: 999` beside `uses lemma units`). Bare literals and `as` targets bind the only declarer in scope; when two or more types declare the name, a bare use is a planning error that lists the qualified forms to write instead. Qualified paths are `Type.unit` or `alias.Type.unit` (not `alias.unit`). Extensions inherit units: bare still binds the declarer; use `Type.unit` or cast to bind the extension. The same rule applies to each factor in a compound unit expression. Types of one family (a type and its extensions, or versions of one spec) must give a shared unit name the same value.

Custom units on `ratio` types follow the same rule. Builtin `percent` and `permille` (`%` / `%%`) are primitive: bare use always binds them (never require `Type.percent`), and they sit on every ratio type's unit list. `as percent` / `as permille` always works; `as` into a uniquely in-scope custom ratio unit (for example `basis_points`) works from any ratio.

In `evaluate` and `show` results, measure unit maps use bare declared names (`eur`, not `price.eur`). Qualification exists only in Lemma source.

### Compound units

On `measure`, `-> unit` may define a **named** unit from other units with `/`, `*`, and `^`. Declare base units on earlier measure types in the same spec (or import them) so dimensions resolve. Import `uses lemma units` when the expression needs stdlib time bases (`second`, `hour`, …). Factor labels in the compound expression follow [Qualifying units](#qualifying-units).

```lemma
spec rates

uses lemma units

data money: measure
  -> unit eur: 1.00
  -> unit usd: 0.90

data rate: measure
  -> unit eur_per_hour: eur/hour

data time_worked: 120 hour
data wage:        60 eur_per_hour

rule total:
  wage * time_worked
```

For `total` you can just multiply the rate by the time worked. Lemma keeps the units consistent, so the product is an amount of money in the currencies you defined. You do not need to write any conversions yourself and you prohibit anyone from accidentally adding money and time together.

A range over a named compound measure is written `<type> range`. The range takes its units from the measure:

```lemma
spec rates

uses lemma units

data money: measure
  -> unit eur: 1.00

data rate: measure
  -> unit eur_per_hour: eur/hour

data wage_band: rate range
  -> with 10 eur_per_hour...100 eur_per_hour

data wage: 60 eur_per_hour

rule wage_in_band:
  wage in wage_band
```

Layer compounds: a later unit may reference an earlier named compound (`eur_per_hour/employee`). Planning checks dimensional consistency.

### Built-in Lemma units (`uses lemma units`)

`Engine::new()` embeds `repo lemma` / `spec units`. Import with `uses lemma units` (or `uses si: lemma units`).

Unit names are **singular only** (`8 hour`, not `8 hours`). Length uses American spelling (`meter`, not `metre`). Thermodynamic temperature is `kelvin` only (no Celsius/Fahrenheit: affine scales need a local single-unit type).

| Type | Role | Notes |
| ---- | ---- | ----- |
| `units.duration` | Time periods | Canonical `second`; includes `nanosecond`…`week` |
| `units.length` | Length | Canonical `meter`; SI scales + imperial (`inch`, `foot`, `yard`, `mile`) |
| `units.mass` | Mass | Canonical `kilogram`; SI scales + imperial (`ounce`, `pound`, `ton`) |
| `units.current` | Electric current | Canonical `ampere` |
| `units.temperature` | Thermodynamic temperature | Canonical `kelvin` |
| `units.substance` | Amount of substance | Canonical `mole` |
| `units.luminous_intensity` | Luminous intensity | Canonical `candela` |
| `units.calendar` | Calendar periods | Canonical `month`; `year` = 12 × `month` |
| `units.area` | Area | `square_meter`, `acre` |
| `units.volume` | Volume | `cubic_meter`, `liter`, `milliliter`, `gallon` |
| `units.force` | Force | `newton`, `kilonewton` (compounds of mass×length/time²) |
| `units.pressure` | Pressure | `pascal`, `kilopascal`, `bar` |
| `units.energy` | Energy | `joule`, `kilojoule`, `watt_hour` |
| `units.power` | Power | `watt`, `kilowatt` |
| `units.frequency` | Frequency | `hertz`, `kilohertz` |
| `units.charge` | Electric charge | `coulomb` |
| `units.voltage` | Electric potential | `volt`, `millivolt`, `kilovolt` |
| `units.resistance` | Electric resistance | `ohm`, `kilohm` |
| `units.capacitance` | Capacitance | `farad`, `microfarad` |
| `units.information` | Digital information | `bit`, `byte`; SI `kilobyte`…`petabyte` (1000ⁿ); IEC `kibibyte`…`pebibyte` (1024ⁿ) |

After `uses lemma units`, declare slots against those types directly:

```lemma
spec shipment

uses lemma units

data package_weight: units.mass
data transit_time:   units.duration
```

Or use unit literals once the types are in scope (`12 kilogram`, `8 hour`, `1 inch`). Prefer the stdlib types over redefining kilogram or hour in every spec.

User loads cannot claim the reserved repository name `lemma`.

## Boolean Literals

Multiple aliases for readability: `true` = `yes` and `false` = `no`.

All are interchangeable:

```lemma
spec boolean_examples

data is_active:   true
data is_approved: yes
data can_decline: false
```



## Special Expressions



### `now`

`now` is the evaluation instant (the run's effective datetime). It equals the CLI `--effective` / HTTP Accept-Datetime value when set; otherwise the current clock. Use `now as time` for the time-of-day of that instant, or `now in <time range>` as sugar for membership in a shift window.

### Date predicates and windows

Require duration/calendar units from `uses lemma units` (or local equivalents) where noted.


| Form                                                   | Meaning                                                                                        |
| ------------------------------------------------------ | ---------------------------------------------------------------------------------------------- |
| `date in past` / `date in future`                      | Strictly before / after `now` (parenthesize when another declaration follows on the next line) |
| `date in past N <duration>` / `in future N <duration>` | Within the last / next `N` duration units of `now` (inclusive window)                          |
| `past N <duration>` / `future N <duration>`            | A date range window relative to `now` (usable as a range value or with `in`)                   |
| `date in calendar year\|month\|week`                   | Same calendar period as `now`                                                                  |
| `date in past\|future calendar year\|month\|week`      | Adjacent calendar period relative to `now`                                                     |
| `date not in calendar year\|month\|week`               | Not the current calendar period                                                                |


```lemma
spec recency

uses lemma units

data event_date: date
data delivered:  date

rule was_before_now:
  event_date in past

rule recent:
  delivered in past 7 day

rule this_year:
  event_date in calendar year

rule last_month:
  event_date in past calendar month

rule this_week:
  event_date in calendar week

rule window:
  past 7 day
```



### Veto

Blocks the rule entirely (no valid result):

```lemma
spec veto_example

data value:               number
data constraint_violated: boolean

rule result:
  value
  unless constraint_violated
    then veto "Constraint violated"
```

A veto is not a boolean: it prevents the rule from producing any result at all. If another rule needs a vetoed value, the veto propagates. An `unless` arm that does not evaluate the vetoed dependency can still produce a value (the fallback wins). Probe with `is veto` / `is not veto` without requiring the value. Missing open inputs and failed runtime constraints also surface as veto. See [Veto](../learn/types_and_units.md#veto).

## Date and time formats

### Dates and datetimes

ISO 8601 format:

```lemma
spec date_formats

data date_only:     2024-01-15
data date_time:     2024-01-15T14:30:00Z
data with_timezone: 2024-01-15T14:30:00+01:00
```

Datetimes that denote the same absolute instant compare equal even when offsets differ (`2024-01-15T14:30:00Z` is `2024-01-15T15:30:00+01:00`).

### Times

Time-of-day literals use `HH:MM:SS` with an optional timezone. Time-range endpoints must share a timezone (both offset/Z or both local); mixing is a planning error.

```lemma
spec time_formats

data open:  09:00:00
data close: 17:30:00+01:00
```

See also [date predicates](#date-predicates-and-windows).

### Date arithmetic

Dates compare directly. Add or subtract a duration or calendar quantity to shift a date (requires `uses lemma units` or local equivalents):

```lemma
spec deadlines

uses lemma units

data today:    2024-09-30
data deadline: 2024-12-31

rule days_until:
  today...deadline as day

rule follow_up:
  deadline + 14 day
```

**Calendar vs duration:** calendar units (`year`, `month`) use calendar-aware arithmetic; duration units (`day`, `hour`, `second`, …) use fixed length. Adding one month to March 1 yields April 1; adding 30 day always adds exactly 30 × 86400 second.

**Month-end clamping:** adding month or year clamps to the last valid day of the target month. January 31 + 1 month → February 28 (or 29 in a leap year).

## Ratios

Ratio values represent proportions. The `ratio` type includes `percent` and `permille` units by default.

**Literal syntax:**

- `15 percent` or `15%`: 15 percent (canonical multiplier 0.15)
- `5 permille` or `5%%`: 5 permille (canonical multiplier 0.005)

```lemma
spec ratio_literals

data tax_rate:   15%
data discount:   20%
data completion: 87.5%
data error_rate: 2%%
```

**Custom ratio types:**

```lemma
spec custom_ratios

data discount_ratio: ratio
  -> minimum 0 percent
  -> maximum 100 percent

data discount: 25%
```

**Use in calculations:**

```lemma
spec ratio_math

data price:         100
data discount_rate: 20%

rule discount_amount:
  price * discount_rate

rule after_discount:
  price * (100% - discount_rate)
```

**Number to ratio conversion:**

```lemma
spec number_to_ratio

rule discount_as_percent:
  0.25 as percent
```

`as percent` / `as permille` always work. When a custom ratio unit is unique in scope, any ratio may convert into it (`margin as basis_points`).

## Evaluate JSON / explanations

Evaluate responses (`lemma evaluate --json`, HTTP POST, SDK `evaluate`) carry per-rule values, optional `missing_data`, and, when explanations are opted in, a per-rule `explanation` tree. Shape: [`api.v1.json`](../../../engine/schemas/api.v1.json) (root `"type":"rule"` with a `"rules"` table; nested uses `"type":"rule_ref"` by `id`; bound data `"type":"data"`; unused cause paths `"type":"data_unused"`; list reads `"type":"row"` / `"type":"filter"`). See [CLI: Explanations](cli.md#api-defaults).

## Test coverage

- [Engine test coverage](coverage/engine.md)
- [CLI test coverage](coverage/cli.md)

Regenerate with `cargo coverage all` (requires `[cargo-llvm-cov](https://github.com/taiki-e/cargo-llvm-cov)`).

## Benchmarks

- [CLI benchmarks](benchmarks/cli.md)
- [Engine benchmarks](benchmarks/engine.md)


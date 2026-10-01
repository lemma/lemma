**Standard library: `uses lemma units`**

Lemma embeds SI bases, derived compounds (force, pressure, energy, power, frequency, electrical), imperial, area/volume, and information (`bit`/`byte`) in `repo lemma` / `spec units`. Import: `uses lemma units`. Reference types: `units.mass`, `units.duration`, `units.length`, `units.force`. Unit names: **singular only** (`8 hour`). Length uses American `meter`. After `uses lemma units`, duration literals (`hour`, `day`, `week`) work; `units.duration` is the type name when you declare a duration slot. No Celsius/Fahrenheit (kelvin only).

```lemma
spec logistics
"""
Physical shipment constraints using SI units from the standard library.
"""

uses lemma units

data package_weight: 12 kilogram
data shift_length:   8 hour
data route_distance: 45 kilometer


rule is_heavy:
  package_weight > 20 kilogram

rule is_long_shift:
  shift_length >= 8 hour
```

Prefer `units.mass`, `units.duration`, `units.length` over redefining units. Convert in family: `as <unit>`. Strip unit: `amount as eur as number`. Cross-family relabel: `5 eur as kg` -> `5 kg`. Name the concept, not the unit (see **Anti-patterns**).

**Ranges: half-open intervals**

Ranges: lower bound inclusive, upper bound exclusive (`lo...hi`). Test with `in`. Width: `lo...hi as <unit>` (duration/measure). Add `as number` only when a bare number is required. Bare `as number` on date/measure ranges fails. Read an endpoint with `lower` / `upper` (`lower 5...10`, `upper period`). `lower` is included; `upper` is excluded. `...` binds tighter than those operators. `-> lower` / `-> upper` still constrain a range slot; they are not the operators. `lower` and `upper` are reserved names. Typedefs: `number range`, `date range`, `measure range`, `ratio range`. Month/year intervals: `uses lemma units` and inline literals (`18 year...67 year`) or `units.calendar range`. Prefix operators (`floor`, `sqrt`, `lower`, …) take one range operand: `floor x + 1` is `(floor x) + 1`.

The snippet below is a syntax sample, not one policy.

Working age (calendar range):
```lemma
spec working_age

uses lemma units

data employee_age: 42 year
data eligible_band: units.calendar range
  -> suggest 18 year...67 year


rule is_working_age:
  employee_age in eligible_band
```

Upper bound exclusive: `67 year` is NOT inside `18 year...67 year`.

Custom measure types can declare their own `measure range` without importing SI; see Reference.

**Derived measures: compound units**

Build compound units with `/`, `*`, `^`. Name derived unit, then give compound expression. Prior measure types must declare referenced base units. Import `uses lemma units` if using time (`eur/hour`).

```lemma
spec contractor

uses lemma units

data money: measure
  -> unit eur: 1.00

data wage_rate: measure
  -> unit eur_per_hour: eur/hour

data time_worked: 120 hour
data wage: wage_rate
  -> suggest 85 eur_per_hour


rule total:
  wage * time_worked
```

Layer compound units: `eur_per_hour` builds on `eur` and `hour`. Dimensional checks run at plan time.

**Date predicates relative to `now`**

`now` is evaluation/effective instant. Import `uses lemma units` for duration windows.

| Form | Meaning |
|------|---------|
| `date in past` / `in future` | Before / after `now` |
| `date in past N day` / `in future N day` | In last / next N duration units |
| `past N day` / `future N day` | Relative date-range window |
| `date in calendar year\|month\|week` | Current calendar period |
| `date in past\|future calendar year\|month\|week` | Adjacent calendar period |
| `date not in calendar year\|month\|week` | Not current calendar period |

```lemma
spec recency

uses lemma units

data event_date: date
  -> help "When did the event happen?"


rule recent:
  event_date in past 7 day

rule this_year:
  event_date in calendar year
```

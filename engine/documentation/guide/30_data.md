**Data: spoken question, not precomputed answer**

`data` declares variables. Constraints define validity. Type-only `data` (no value) is an input slot. Use real domain values. Never `"TODO"` or dummy placeholders.

**Spoken question**

Each `data` field's `-> help` is the sentence you would say to a person about their situation. If that sentence requires counting, converting, applying a legal threshold, or interpreting the policy, the field is wrong: those are `rule`s. Dates, amounts, and yes/no facts of the situation stay as `data`.

WRONG (elapsed time as a bare number; unit baked into the name):
```lemma-skip
data days_overdue: number
  -> help "How many days late is the book?"
```

RIGHT (facts of the situation; duration keeps its unit):
```lemma
spec library_return

uses lemma units

data due_date: date
  -> help "When was the book due?"

data return_date: date
  -> help "When was the book returned?"

rule overdue:
  due_date...return_date as day
```

Name the concept; keep the unit on the value, not in the field name (`see **Units**`).

WRONG (policy threshold already applied):
```lemma-skip
data qualifies_for_free_shipping: boolean
  -> help "Is the order €50 or more?"
```

RIGHT:
```lemma
spec shipping_threshold

data money: measure
  -> unit eur: 1.00
  -> decimals 2

data order_total: money
  -> help "What is the order total?"

rule free_shipping:
  order_total >= 50 eur
```

Age-in-years can stay when that is what you would actually ask (e.g. senior discount). A birthday gate wants a birth date.

**Measure**

Use `measure` when the fact carries a unit: money, mass, distance, duration, rates, energy, and other dimensional quantities. Use `number` only when the value is truly dimensionless (counts, scores, IDs). Use `ratio` for proportions (`15%`, `50 permille`), not for currency amounts.

Declare a **named measure parent** once, then extend it on input slots:

```lemma
spec invoice

data money: measure
  -> unit eur: 1.00
  -> decimals 2
  -> minimum 0 eur

data invoice_total: money
  -> help "What is the invoice total?"
```

`-> unit` rows define which units the type accepts; literals and comparisons use those units (`50 eur`, not a bare `50`). Name the concept (`invoice_total`, `package_weight`), not the unit in the field name (`price_eur`, `weight_kg`).

For physical quantities and time, import the standard library (`uses lemma units`) and prefer `units.mass`, `units.duration`, `units.length` over redefining SI units. Duration literals (`8 hour`, `25 year`), conversion (`as <unit>`), compound units (`eur/hour`), measure ranges, and calendar intervals are all covered in **Units**.

**Boolean naming**

Name booleans for the fact you mean; prefer the natural predicate with `-> suggest` for the usual case (`data item_damaged: boolean -> suggest false`). Do not invent awkward opposites (`item_undamaged`) or negated names (`not_*`, `no_*`, `has_no_*`).

**Constraints**

**Types**

Primitives: `boolean`, `number`, `text`, `measure`, `ratio`, `date`, `time`, and `number range`, `measure range`, `date range`, `time range`, `ratio range`. Named parents (`data money: measure`) inherit constraints; see [Reference](https://lemma.run/reference#extending-data).

**Data commands by type**

| Type | `->` commands |
|------|----------------|
| `boolean` | `help`, `with`, `suggest` |
| `number` | `decimals`, `minimum`, `maximum`, `help`, `with`, `suggest` |
| `measure` | `unit`, `decimals`, `minimum`, `maximum`, `help`, `with`, `suggest` (see **Measure**, **Units**) |
| `ratio` | `unit`, `minimum`, `maximum`, `help`, `with`, `suggest` |
| `text` | `option`, `options`, `length`, `help`, `with`, `suggest` |
| `date`, `time` | `minimum`, `maximum`, `help`, `with`, `suggest` |
| `number range`, `date range`, `time range`, `ratio range` | `lower`, `upper`, `minimum`, `maximum`, `help`, `with`, `suggest` |
| `measure range` | above |

On range types, `lower` / `upper` bound endpoints; `minimum` / `maximum` bound **span width**, not endpoints.

**Usage**

- Chain `->` rows on `data` only (rules have no constraints; see **Syntax**).
- `-> help` = spoken question (CS ask string).
- `-> with` = commit a value on a typed slot (callers may override); cannot combine with `suggest` on the same declaration.
- `-> suggest` = UI hint; does not fill or commit.
- Prefer `-> option` for closed text sets.
- Bounded domain on constraints (`-> minimum 0`), not veto (see **Veto**).
- Examples: type catalog [01_coffee_order.lemma](https://raw.githubusercontent.com/lemma/lemma/main/engine/documentation/examples/01_coffee_order.lemma); fee policy [02_library_fees.lemma](https://raw.githubusercontent.com/lemma/lemma/main/engine/documentation/examples/02_library_fees.lemma).
- Edge cases (compound units, qualifying units, calendar ranges): **Units** and [Reference](https://lemma.run/reference#data-commands).

**Lists**

A list is ordered rows of one type. `data fares: money list` holds values; `uses staff: employee list` holds instances of spec `employee`, whose `data` are the columns and whose rules are computed per row. No list literals: each `-> with` adds rows, comma-separated. A list without rows is empty, never missing data. Evaluation data replaces a whole list with one JSON array (strings for values, objects keyed by row data names for instances). Do not name data, rules, specs, aliases, or units after list keywords (see **Syntax**).

Use a list when rows are uniform and combined (summed, searched, filtered, counted, or their number varies). Use named `uses` instances when each has its own role (`buyer`, `seller`).

```lemma
spec menu_item

data drink: text
  -> options "espresso", "latte"

data price: measure
  -> unit eur: 1.00
  -> decimals 2


spec coffee_order

uses menu: menu_item list
  -> key drink
  -> with drink: "espresso", price: 2.50 eur
  -> with drink: "latte",    price: 3.50 eur

data ordered:  menu.drink
data quantity: number

rule selected_drink:
  menu -> find ordered

rule total:
  selected_drink.price * quantity
```

- `-> key drink` identifies rows by field; without it rows are numbered `1` to `n`. Duplicate source keys are planning errors; a row key must be an allowed option when the field has options; `find` with no row vetoes.
- `menu.drink` in a type position reuses the row spec's type.
- A row reads only its own data. A value the row needs from the enclosing spec is supplied per row (`-> with band: 0 eur...38000 eur, rate: 36%, income: income`).
- A spec may hold a list of its own rows (`uses components: part list` inside `spec part`); rows come from evaluation data only.
- List commands (`find`, `sum`, `where`, ...) are rule expressions, never `data` commands (see **Rules**).

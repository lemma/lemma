---
nav_title: Lists
nav_order: 65
---

# Lists

A list is an ordered sequence of rows of one type. A row is either a value (`money list`) or an instance of a Spec (`employee list`). A Spec describes the columns: each Data is a column, and each Rule is a computed column that is evaluated per row.

Lists have no literal syntax. You build them line by line with `-> with`, like everything else in Lemma.

For syntax details see [Lists](../reference/readme.md#lists) in the reference.

## Value lists

`V list` is a type, written the same way as `V range`. Each `-> with` adds rows, separated by commas.

```lemma
spec fares

data money: measure
  -> unit eur: 1.00

data fares: money list
  -> with 89 eur, 120 eur, 64 eur

rule cheapest_fare:
  fares -> min

rule total:
  fares -> sum

rule high_fares:
  fares -> where >= 100 eur

rule fare_count:
  fares -> count
```

- `data fares: money list -> with ...` is an overridable default. Evaluation data replaces the whole list, or patches individual cells with dotted keys (see [Evaluation data](#evaluation-data)).
- `data fares: money list` without rows is an empty list.
- On a value list, a condition (`where`, `any`, `all`) starts with the comparison operator. The row value is the left operand: `fares -> where >= 100 eur`. Compound `and` / `or` needs call form: `fares -> where(> 5 or is 3)`. Bare `where > 5 or is 3` is a parse error. Chained `where` still means conjunction.
- Value lists are numbered. `-> key` is only for instance lists.

## Instance lists

Caller-owned instance lists are `data`: `data staff: employee list`. Import the row Spec with `uses`; its data and rules become local to this Spec. Each `-> with` line adds one row as comma-separated `field: value` pairs.

```lemma
spec employee

data name: text
data salary: measure
  -> unit eur: 1.00

rule bonus:
  salary * 10%


spec payroll

uses employee

data staff: employee list
  -> with name: "Ada",   salary: 9_000 eur
  -> with name: "Grace", salary: 9_500 eur

rule total_bonus:
  staff -> sum bonus

rule top_earner:
  staff -> sort salary desc -> first

rule top_bonus:
  top_earner.bonus
```

- Write `uses employee` for one sealed import, or `data staff: employee list` for a caller-owned list. A sealed catalog is a list-valued rule. Instance-list conditions are ordinary boolean expressions: `staff -> where(bonus > 500 eur or salary < 3_000 eur)`.
- `bonus` runs once per row. Each row reads only its own Data.
- In a command argument, both the row's names (`bonus`, `salary`) and the enclosing Spec's names are in scope. A name that exists in both places must be qualified with the list name, as in `staff.salary`.
- `top_earner` is a single row. Read its members with a dot, as you would on a `uses` instance.
- A row member can itself be a `uses` instance. The returned value stays bound to that row, so `(lines -> first).item` returns a row value such as `lines.1.item`, and its Data and Rules can be read with another dot.
- Type position names the row field through the `uses` alias (`data ordered: menu_item.drink`) or through a caller-owned list (`data ordered: menu.drink`). A rule named `menu` is not a type path.

An imported instance can seal one of its list slots from other named instances:

```lemma
spec node

data money: measure
  -> unit eur: 1.00

data is_leaf:    boolean
data unit_price: money
data children:   node list


spec tree

uses left: node
  -> with is_leaf: true
  -> with unit_price: 5 eur

uses right: node
  -> with is_leaf: true
  -> with unit_price: 9 eur

uses mix: node
  -> with is_leaf: false
  -> with children: left, right

rule child_count:
  mix.children -> count
```

`mix.children` is sealed, so evaluation data cannot replace it. Its rows follow the comma order, not declaration order. Repeating an alias is allowed without `-> key`; keyed lists still require unique sealed literal keys. Cyclic wiring between sealed list slots is a planning error.

## Keys and `find`

Every list is keyed. With `-> key` (at most once, before the first `-> with`), rows are identified by those fields. Without it, rows are numbered `1` to `n` in row order.

```lemma
spec menu_item

data drink: text
  -> options "espresso", "latte"

data price: measure
  -> unit eur: 1.00


spec coffee_order

uses menu_item

data menu: menu_item list
  -> key drink
  -> with drink: "espresso", price: 2.50 eur
  -> with drink: "latte",    price: 3.50 eur

data ordered:  menu_item.drink
data quantity: number

rule selected_drink:
  menu -> find ordered

rule total:
  selected_drink.price * quantity
```

- `find` is the only lookup. It takes one value per key field, in `-> key` order: `rates -> find parcel_weight, parcel_volume`. On a numbered list, it takes a row number.
- Planning checks the source rows: a duplicate key is an error. When the key field has options, a row key must be one of those options; options need not all have a row.
- A `find` with no matching row vetoes. A missing row never turns into a default.
- `menu_item.drink` in a type position names the row Spec's type through the `uses` alias, so `ordered` accepts only the menu's drinks. `data ordered: menu.drink` names that same type through the caller-owned list. A list-valued rule is not a type path.

A key field of a range type matches by containment. Lists chosen by a Rule must have the same key:

```lemma
spec rate

data weight: measure
  -> unit kilogram: 1.00

data money: measure
  -> unit eur: 1.00

data band:  weight range
data price: money


spec parcel_shipping

uses rate

data standard_rates: rate list
  -> key band
  -> with band: 0 kilogram...2 kilogram,   price: 4.95 eur
  -> with band: 2 kilogram...10 kilogram,  price: 6.95 eur
  -> with band: 10 kilogram...23 kilogram, price: 13.95 eur

data express_rates: rate list
  -> key band
  -> with band: 0 kilogram...2 kilogram,   price: 9.95 eur
  -> with band: 2 kilogram...10 kilogram,  price: 12.95 eur
  -> with band: 10 kilogram...23 kilogram, price: 19.95 eur

data parcel_weight: rate.weight
data express:       boolean

rule rates:
  standard_rates
  unless express then express_rates

rule shipping_cost:
  (rates -> find parcel_weight).price
```

A parcel weighing 1 kilogram costs 4.95 eur standard and 9.95 eur express. A weight outside every band vetoes.

## Commands

List commands are written after `->` in Rules. A command takes its argument bare when the argument is one operand (`staff -> sum bonus`). Otherwise it uses parentheses (`lines -> sum(qty * price)`).

`->` applies to everything on its left up to a comparison, `in`, `not`, `and`, `or`, `then` or `unless`. So `items -> count > 0` means `(items -> count) > 0`, and `a + b -> sum x` means `(a + b) -> sum x`. To compute with the result, put parentheses around the command: `own_cost + (components -> sum unit_cost)`. Writing `items -> count + 1` is an error that asks for those parentheses.

| Command | Result | Empty list |
|---------|--------|------------|
| `sum`, `avg`, `product` | total, mean, product | `sum` is 0; `avg` and `product` veto |
| `min`, `max` | smallest, largest value | veto |
| `count` | number of rows | 0 |
| `any`, `all` | boolean | `any` is false; `all` is true |
| `where` | matching rows | empty list |
| `sort ... asc` / `sort ... desc` | rows in that order (stable) | empty list |
| `top n` | first `n` rows | empty list |
| `first`, `last`, `find` | one row | veto |
| `with` | rows appended | |

If any row vetoes inside `sum`, `avg`, `product`, `min`, `max`, `any`, `all`, `where` or `sort`, the command vetoes with the first such row's reason. `count` never vetoes because of a row.

## A list of its own rows

A Spec may hold a list of its own rows. The depth then comes from evaluation data, and evaluation runs bottom-up:

```lemma
spec part

data components: part list
data money: measure
  -> unit eur: 1.00

data quantity: number
data own_cost: money

rule unit_cost:
  own_cost
  + (components -> sum(components.quantity * components.unit_cost))
```

- Only `data x: own_spec list` inside that Spec may refer to the Spec itself. A self-list has no source rows; its rows come only from evaluation data.
- Values flow up only. A row never reads its parent.
- Pin the row Spec when the body must stay fixed: `uses part 2024-01-01` then `data components: part list`.

## Evaluation data

A data list is one input. You may replace every row with one JSON array, or patch individual unbound fields with dotted keys that match ask keys (`staff.2.salary`).

- Value list: an array of literal strings, `{"fares": ["89 eur", "120 eur"]}`.
- Instance list: an array of objects, keyed by the row Spec's Data names: `{"staff": [{"name": "Linus", "salary": "7000 eur"}]}`.
- Dotted patch: `{"staff.2.salary": "8000 eur"}` fills that cell when the list already has a row (from source defaults or a prior whole-list bind). Sending both the whole list key and a dotted key for the same list in one request is an error.
- `[]` is an explicit empty list.
- A `null` item is a request error. A `null` field inside a row object leaves that field unsupplied, so it is reported as missing data with its row number.

Lists are passed as JSON through `lemma evaluate <spec> -` on stdin, the HTTP POST JSON body, MCP `evaluate`, and the SDKs. `name=value` arguments and form bodies cannot carry a list.

```bash
echo '{"staff": [{"name": "Linus", "salary": "7000 eur"}, {"name": "Ada", "salary": "9000 eur"}]}' \
  | lemma evaluate payroll - --json
```

## Results

A Rule that returns a list has a `list` field with one entry per row. A Rule that returns a row, including a `uses` instance projected from a row, has a `row` reference: the row Spec, its effective date, the instance path (`staff.2` or `lines.1.item`) and an `index`. The index points into the response's `rows` array, which holds each referenced row once, with its Data and Rule results:

```json
{
  "results": {
    "top_earner": {
      "rule_type": "employee",
      "result": "staff.2",
      "row": { "spec": "employee", "effective": "", "instance": "staff.2", "index": 0 }
    }
  },
  "rows": [
    {
      "spec": "employee",
      "effective": "",
      "data": { "name": { "result": { "result": "Ada", "text": "Ada" } } },
      "rules": { "bonus": { "result": { "result": "900 eur", "measure": { "eur": "900" } } } }
    }
  ]
}
```

`lemma show --json` describes a list as a JSON array. The `items` of an instance list refer to the row schema in `$defs`. With source rows, `default` is the evaluation data those rows stand for, and `x-lemma-value` holds the fields each row sets, in row order:

```json
"menu": {
  "type": "array",
  "items": { "$ref": "#/$defs/LemmaRow_menu__item" },
  "maxItems": 1024,
  "default": [{ "drink": "espresso" }, { "drink": "latte" }],
  "x-lemma-value": {
    "list": [
      { "fields": { "drink": { "result": "espresso", "text": "espresso" } } },
      { "fields": { "drink": { "result": "latte", "text": "latte" } } }
    ]
  }
}
```

A row field bound to enclosing data (`income: income`) has no value before a run, so that list has no `default` and no `x-lemma-value`.

## List or named instances

- Use named instances (`uses buyer: party`, `uses seller: party`) when each instance has its own role. `uses -> with` seals those slots: evaluate data cannot override them.
- Use a data list when the rows are uniform and combined: summed, searched, filtered or counted, or when their number varies. Use a list-valued rule for a sealed catalog the caller must not replace.

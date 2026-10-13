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
  -> decimals 2

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

- `data fares: money list -> with ...` is an overridable default. Evaluation data replaces the whole list.
- `data fares: money list` without rows is an empty list. A list is never missing data.
- On a value list, a condition (`where`, `any`, `all`) starts with the comparison operator. The row value is the left operand: `fares -> where >= 100 eur`.

## Instance lists

An instance list imports a Spec as rows: `uses staff: employee list`. Each `-> with` line adds one row as comma-separated `field: value` pairs.

```lemma
spec employee

data name: text
data salary: measure
  -> unit eur: 1.00
  -> decimals 2

rule bonus:
  salary * 10%


spec payroll

uses staff: employee list
  -> with name: "Ada",   salary: 9_000 eur
  -> with name: "Grace", salary: 9_500 eur

rule total_bonus:
  staff -> sum bonus

rule top_earner:
  staff -> sort salary desc -> first

rule top_bonus:
  top_earner.bonus
```

- `bonus` runs once per row. Each row reads only its own Data.
- In a command argument, both the row's names (`bonus`, `salary`) and the enclosing Spec's names are in scope. A name that exists in both places must be qualified with the list name, as in `staff.salary`.
- `top_earner` is a single row. Read its members with a dot, as you would on a `uses` instance.

## Keys and `find`

Every list is keyed. With `-> key`, rows are identified by those fields. Without it, rows are numbered `1` to `n` in row order.

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

- `find` is the only lookup. It takes one value per key field, in `-> key` order: `rates -> find parcel_weight, parcel_volume`. On a numbered list, it takes a row number.
- Planning checks the source rows: a duplicate key is an error. When the key field has options, a row key must be one of those options; options need not all have a row.
- A `find` with no matching row vetoes. A missing row never turns into a default.
- `menu.drink` in a type position names the row Spec's type, so `ordered` accepts only the menu's drinks.

A key field of a range type matches by containment. Lists chosen by a Rule must have the same key:

```lemma
spec rate

data weight: measure
  -> unit kilogram: 1.00

data money: measure
  -> unit eur: 1.00
  -> decimals 2

data band:  weight range
data price: money


spec parcel_shipping

uses standard_rates: rate list
  -> key band
  -> with
       band: 0 kilogram...2 kilogram,
       price: 4.95 eur
  -> with
       band: 2 kilogram...10 kilogram,
       price: 6.95 eur
  -> with
       band: 10 kilogram...23 kilogram,
       price: 13.95 eur

uses express_rates: rate list
  -> key band
  -> with
       band: 0 kilogram...2 kilogram,
       price: 9.95 eur
  -> with
       band: 2 kilogram...10 kilogram,
       price: 12.95 eur
  -> with
       band: 10 kilogram...23 kilogram,
       price: 19.95 eur

data parcel_weight: standard_rates.weight
data express:       boolean

rule rates:
  standard_rates
  unless express then express_rates

rule shipping_cost:
  (rates -> find parcel_weight).price
```

A parcel weighing 1 kilogram costs 4.95 eur standard and 9.95 eur express. A weight outside every band vetoes.

## Commands

List commands are written after `->` in Rules. A command takes its argument bare when the argument is one operand (`staff -> sum bonus`). Otherwise it uses parentheses (`lines -> sum(qty * price)`). `->` binds tighter than every operator, so `items -> count > 0` means `(items -> count) > 0`.

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

uses components: part list

data money: measure
  -> unit eur: 1.00
  -> decimals 2

data quantity: number
data own_cost: money

rule unit_cost:
  own_cost
  + components -> sum(components.quantity * components.unit_cost)
```

- Only `uses x: own_spec list` may refer to the Spec itself. A self-list has no source rows; its rows come only from evaluation data.
- Values flow up only. A row never reads its parent.

## Evaluation data

A list is one input key holding a JSON array. The array replaces every source row.

- Value list: an array of literal strings, `{"fares": ["89 eur", "120 eur"]}`.
- Instance list: an array of objects, keyed by the row Spec's Data names: `{"staff": [{"name": "Linus", "salary": "7000 eur"}]}`.
- `[]` is an explicit empty list.
- A `null` item is a request error. A `null` field inside a row object leaves that field unsupplied, so it is reported as missing data with its row number.

Lists are passed as JSON through `lemma evaluate <spec> -` on stdin, the HTTP POST JSON body, MCP `evaluate`, and the SDKs. `name=value` arguments and form bodies cannot carry a list. A missing row field is reported with its row number, such as `staff.2.salary`. To supply it, send the whole array again.

```bash
echo '{"staff": [{"name": "Linus", "salary": "7000 eur"}, {"name": "Ada", "salary": "9000 eur"}]}' \
  | lemma evaluate payroll - --json
```

## Results

A Rule that returns a list has a `list` field with one entry per row. A Rule that returns a row has a `row` reference: the row Spec, its effective date, the instance path (`staff.2`) and an `index`. The index points into the response's `rows` array, which holds each referenced row once, with its Data and Rule results:

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
      "rules": { "bonus": { "result": { "result": "900.00 eur", "measure": { "eur": "900.00" } } } }
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

- Use named instances (`uses buyer: party`, `uses seller: party`) when each instance has its own role.
- Use a list when the rows are uniform and combined: summed, searched, filtered or counted, or when their number varies.

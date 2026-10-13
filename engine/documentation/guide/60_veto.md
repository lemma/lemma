**Veto: impossible to answer, not `false`**

Veto is like Rust's `Err`: rule has **no value** and propagates to dependents. Use veto when the question has no answer, not when the business answer is false, and not for out-of-range inputs.

| Situation | Use |
|-----------|-----|
| Out-of-range (negative score, age above 120) | `-> minimum` / `-> maximum` on `data` |
| Closed choice list | `-> option` on `data` |
| Normal business "no" | `false` or `no` |
| Lookup / no mapped result | default `veto` + `unless` arm per known case |
| Test veto without propagating | `x is veto` (returns boolean) |

**Litmus test:** Can the question be answered? If yes, even when the answer is negative, use `true`/`false`. If the question itself is unanswerable for this input, use veto. "Is the customer eligible?" is always answerable. "What is the price of this product?" when it is not on the list is unanswerable (veto). Out-of-range age is `-> maximum 120`, not veto.

A vetoed rule is not `false`. `x is false` does not match a vetoed `x`. To test whether a rule vetoed, use `x is veto`.

**Lookup default vs unless veto**

Lookup: the default expression is `veto "…"`; each `unless` arm maps a known case. `-> option` is the intake set; default `veto` is the mapping when a listed value has no price arm.

An `unless … then veto` arm (business denial, not lookup) follows last-wins like any other `unless` (see **Rules**).

**Lookup (normal)**

```lemma
spec coffee_pricing

data money: measure
  -> unit eur: 1.00
  -> decimals 2

data drink: text
  -> option "espresso"
  -> option "latte"
  -> option "cappuccino"
  -> option "mocha"

rule base_price:
  veto "Unknown type of coffee"
  unless drink is "espresso"   then 2.5 eur
  unless drink is "latte"      then 3.5 eur
  unless drink is "cappuccino" then 3.5 eur
  unless drink is "mocha"      then 4 eur
```

If `base_price` vetoes, dependents that need its value veto too (propagates). Full example: [01_coffee_order.lemma](https://raw.githubusercontent.com/lemma/lemma/main/engine/documentation/examples/01_coffee_order.lemma).

**Veto vs boolean vs constraint**

WRONG: veto for business denial or range check:
```lemma-skip
rule is_eligible:
  true
  unless customer_age < 18  then veto "Must be 18+"
  unless customer_age > 120 then veto "Invalid age"
```

RIGHT: bounds on data; boolean for business no:
```lemma
spec age_gate

data customer_age: number
  -> minimum 0
  -> maximum 120
  -> help "How old is the customer?"

rule is_adult:
  customer_age >= 18
```

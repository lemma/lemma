---
nav_title: Extending data
nav_order: 50
---

# Extending Data

Lemma has no separate type system. Types are declared inline on Data slots and extended with `->` commands. This chapter covers declaring parent types and reusing them across Specs.

## Declaring parent types

Declare a primitive or parent Data name on the right-hand side, then chain `->` Data commands. Other Data extend it by naming it:

```lemma
spec warehouse_types

data money: measure
  -> unit eur: 1.00
  -> unit usd: 0.91
  -> decimals 2
  -> minimum 0 eur

data mass: measure
  -> unit kilogram: 1.0
  -> unit gram: 0.001
  -> unit pound: 0.453592

data price:  100 eur
data weight: 75 kilogram
```

## Reuse across Specs

Import a Spec with Uses and reference its Data declarations as parent types with the alias (`alias.field`):

```lemma
spec base_types

data currency: text
  -> option "EUR"
  -> option "USD"

data rate: ratio
  -> maximum 100 percent


spec checkout

uses base: base_types

data payment_currency: base.currency
data discount_rate: base.rate
  -> maximum 50 percent
```

The imported `data currency` declaration becomes the type for `payment_currency`; the local slot can add further constraints (`-> maximum 50%` on `discount_rate`).

See [Extending Data in the language reference](../reference/readme.md#extending-data) for the full list of Data commands.

## Next up

[Composing Specs](composing_specs.md): Uses, temporal versions, planning checks, repositories, and the registry.

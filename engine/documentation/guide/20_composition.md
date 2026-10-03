**Organization: spec → rule**

Default: one file, one implicit repo. No `repo` blocks unless multi-namespace workspace requested. Structure: **spec → rule**.

**Spec** = namespace for `data` and `rules`. **Rule** = named computed value. Reference rules by name; engine resolves if name is data or rule. One file can have multiple specs.
Hierarchical names: `spec employee/contract`. Effective date for temporal changes: `spec pricing 2026-01-01`.
Commentary placement: see **Syntax**.

**Example: multi-spec composition (same file)**

```lemma
spec base_config

data tax_rate:          21%
data standard_discount: 5%

data price: measure
  -> unit eur: 1.00
  -> decimals 2

rule tax_amount:
  price * tax_rate

rule price_with_tax:
  price + tax_amount

rule discount_amount:
  price * standard_discount

rule discounted_price:
  price - discount_amount

rule final_price:
  discounted_price * (100% + tax_rate)


spec line_item

uses pricing: base_config
  -> with price: 10 eur

data qty: number
  -> minimum 0
  -> suggest 10


rule line_total:
  pricing.final_price * qty

rule has_discount:
  pricing.standard_discount > 0%
```

- `uses alias: target_spec`: imports spec in same file.
- Reference members: `alias.field` or `alias.rule_name`.
- Under `uses`, `  -> with path: value` sets imported data (path relative to imported spec). Do not use `data alias.field`. Standalone `with alias.field: …` is deprecated (still parses; `quality` recommends block form). Local slots use `data`.
- A rule may return a `uses` alias (`rule chosen: basic unless flag then higher`). Dot on that rule reads the chosen instance (`chosen.rate`). All `unless` branches must be the same spec and resolved version. Do not add, compare, or `with` a spec instance; `is veto` is allowed. The result JSON includes `spec` (name, effective, instance path, `data` run-data texts, `rules`). Feeding `data` back into that spec at the same effective instant reproduces the rules. A bare alias reads the whole imported spec for unpinned drift checks.

**LemmaBase: shared repositories**

Repositories on [LemmaBase.com](https://lemmabase.com) imported with `@` repo qualifiers. Search: [lemmabase.com/search?q=](https://lemmabase.com/search?q=) (e.g. `?q=finance`).

```lemma
spec invoicing

uses lemma units

uses iso: @iso/countries alpha2 2026-01-01

data price: measure
  -> unit eur: 1

data country: iso.code


rule tariff:
  0 eur
  unless country is "NL" then price * 5%

rule total:
  price + tariff
```

Forms:
- `uses @user/repo spec_name`: import LemmaBase spec (alias = spec name)
- `uses alias: @user/repo spec_name`: import with alias (`iso.field`)
- `uses @user/repo spec_name 2026-01-01`: pin effective date

Reference imported members: `iso.code`. Detail: [LemmaBase](https://lemma.run/reference/registry).

`repo` blocks namespace specs across contexts (e.g., `repo accounting`). Skip unless asked. Details: [Composing specs](https://lemma.run/learn/composing_specs).

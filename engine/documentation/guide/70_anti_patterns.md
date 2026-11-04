**Anti-patterns**

Inline comments (WRONG: `#` fails to parse): `data customer_age: number -> minimum 0  # input`, `rule discount: 0%  # default`.
RIGHT: no inline comments; commentary after `spec` only (see **Syntax**).

Commentary after `uses` (WRONG). RIGHT: commentary immediately after `spec` (see **Syntax**).

Mega-rule (WRONG): opaque nested `unless` inside one expression. RIGHT: named pipeline rules (see **Rules**).

Hardcoded input (WRONG): `rule discount: 10 * 0.1`
RIGHT: `data qty: number` then `rule discount: qty * 0.1`

Wrong unless order (WRONG: VIP gets 20% not 25%):
```lemma-skip
rule discount:
  0%
  unless is_vip    then 25%
  unless qty >= 50 then 20%
```
RIGHT: specific override last (`qty` tiers first, `is_vip` last; see **Rules**).

Placeholder (WRONG): `data customer_name: "TODO"`
RIGHT: `data customer_name: text`

Elapsed time / pre-interpreted threshold as data (WRONG):
```lemma-skip
data days_overdue:                number
data qualifies_for_free_shipping: boolean
```
RIGHT: dates and amounts as data; duration/threshold rules without unit names (see **Data**).

Unit in the name (WRONG — common agent failure):
```lemma-skip
rule days_overdue:
  due_date...return_date as day as number

rule weight_kg:
  package_weight as kilogram

rule price_eur:
  total
```
RIGHT: name the concept; keep the unit on the value (see **Data**, **Units**).

Veto for bounds (WRONG):
```lemma-skip
rule validated_score:
  score
  unless score < 0 then veto "Invalid score"
```
RIGHT: `data score: number -> minimum 0`. Veto is unanswerable, not out-of-range (see **Veto**).

Error vs Veto: `5 and "text"` = planning Error. Unmapped product with default `veto "…"` = runtime Veto.

Veto-as-rejection (WRONG: denial is answerable; see **Veto**):
```lemma-skip
rule is_eligible:
  true
  unless age < 18   then veto "Must be 18+"
  unless not has_id then veto "ID required"
```
RIGHT: boolean rules (`is_adult` and `has_valid_id`).

Unnecessary `repo` (WRONG). RIGHT: specs in the repository, no `repo` block, unless a second namespace is required.

Constraints on rules (WRONG): `rule discount: 10% -> help "…"`. `->` is data-only (see **Syntax**).

Domain-blind polarity / double denial (WRONG):
```lemma-skip
rule can_ship:
  yes
  unless not in_stock         then no
  unless not address_complete then no
```
RIGHT: domain principle + positive grant (see **Rules**).

Useless default/unless for boolean rules (WRONG):
```lemma-skip
rule can_ship:
  no
  unless in_stock and address_complete then yes
```
RIGHT: `rule can_ship: in_stock and address_complete`. When one `unless … then yes` is the whole answer, write the condition directly; `unless` is for exceptions, not a boolean wrapper.
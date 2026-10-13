**Rules and unless: last matching clause wins**

Default expression, then `unless <condition> then <result>`. Source order; **bottommost match wins**. General first, specific last. Snake_case names; boolean predicates (`is_eligible`, `can_ship`). Named pipeline rules, no opaque mega-expressions.

**Domain-principle default**

Default is not "prefer yes/no." It is the answer **in principle for this rule's domain**. Experts must read top-to-bottom as: "In principle X; unless Y, then Z."

1. Name the question (`can_ship` → "Can we ship this order?").
2. Before special cases, what is true in principle? That is the default: from *this* rule's domain, not optimism or "start true and subtract failures."
3. What positive facts change the answer? Those are `unless` conditions, not negated flips (`unless not ready then no`).
4. Write `rule name: <principle> unless <positive conditions> then <exception>`.

Examples by domain: shipping often earned (`no` unless grant); discount often `0%`; fees use the policy's usual fee, not "free unless expensive."

Forbidden: double denial (`yes` / `unless not … then no`); fail-each-check cascades; invented `*_compliant` default-yes helpers; a single `no` / `unless … then yes` when the condition is the whole answer (see **Anti-patterns**). In `and` chains, do not mix a bare name with `is false` (WRONG: `x and y is false`); use parallel probes (`x is false and y is true`) or parentheses. `not` before `and` takes the call form: `not(x) and y`.

**Overlapping unless (last wins)**

```lemma
spec vip_discount

data qty: number
  -> minimum 0
  -> help "How many items?"

data is_vip: boolean
  -> suggest false
  -> help "Is the customer a VIP?"

rule discount:
  0%
  unless qty >= 10 then 10%
  unless qty >= 50 then 20%
  unless is_vip    then 25%
```

VIP ordering 75 items gets **25%** (not 20%): both `qty >= 50` and `is_vip` match; bottommost wins.

**List commands**

Postfix `->` in rule expressions: `staff -> sum bonus`, `menu -> find ordered`, `staff -> sort salary desc -> first`. One operand is bare; anything wider takes parentheses (`lines -> sum(qty * price)`). `->` binds tighter than operators: `items -> count > 0`.

- Reduce: `sum`, `avg`, `product`, `min`, `max`, `count`, `any`, `all`.
- Transform (same list type): `where`, `sort ... asc|desc`, `top n`, `with`.
- One row: `find`, `first`, `last`. Read a row with a dot: `(staff -> first).bonus`.
- Value lists take no expression; conditions start with the operator: `fares -> where >= 100 eur`.
- A name on both the row and the enclosing spec must be qualified with the list name (`components.quantity`).
- Those command words are reserved names (see **Syntax**); do not reuse them for data or rules.
- Empty: `sum` 0, `count` 0, `any` false, `all` true; `avg`, `product`, `min`, `max`, `first`, `last` veto. Any vetoed row vetoes the reduction. Guard compounding with `count`, not `is veto`.
- Not expressible: running balances, row-to-row comparison on a flat list, group-by over unknown groups. Say so; do not fake them with `unless` chains.

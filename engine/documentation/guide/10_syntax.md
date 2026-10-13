**Recommended spec opening order** (`lemma format` emits this; the parser allows `meta` / `uses` / `data` / `rule` in any order after commentary):

```
spec <name> [<effective>]
[""" commentary: optional, but if present must be HERE """]
meta ...
uses ...
data ...
rule ...
```

Commentary after `uses` or `data` is invalid. Optional `meta key: value` after commentary (provenance, not policy). `rule name:` with the body on the next indented line. No `#`, `//`, `--` comments. Use descriptive names. Put user explanations outside code fences, never inside ` ```lemma ` blocks.

**Gotchas (parse errors)**

- No `or` operator. Disjunction via `unless` chains or separate boolean rules.
- Constraints (`-> help`, `-> option`, `-> minimum`, etc.) apply to `data` only. Rules have no constraints.
- Comma is only a list separator: `1,000` is invalid (write `1_000` or `1000`). `-> options "a", "b"` and `-> with qty: 3, sku: "latte"` use commas between items; space-separated options or field pairs are invalid.
- Remainder is `mod` (`value mod 7`, floored, sign of the divisor). `%` is the percent suffix only: `50%` is a ratio literal, `7 % 3` is invalid.
- List keywords are reserved in any case and cannot be data, rule, spec, alias, or unit names: `list`, `key`, `find`, `first`, `last`, `sum`, `avg`, `product`, `count`, `min`, `max`, `any`, `all`, `where`, `sort`, `top`, `asc`, `desc`. Also reserved: `lower`, `upper` (see **Units**), `is`, `mod`, `past`, `future`, `permille`.

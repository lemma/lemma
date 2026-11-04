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

- Constraints (`-> help`, `-> option`, `-> minimum`, etc.) apply to `data` only. Rules have no constraints.
- Comma is only a list separator: `1,000` is invalid (write `1_000` or `1000`). `-> options "a", "b"` and `-> with qty: 3, sku: "latte"` use commas between items; space-separated options or field pairs are invalid.
- Remainder is `mod` (`value mod 7`, floored, sign of the divisor). `%` is the percent suffix only: `50%` is a ratio literal, `7 % 3` is invalid.
- Rounding is postfix only: `x -> round` rounds a number to an integer (an exact half goes away from zero: `5 / 2 -> round` is `3`). Round a measure or ratio to a literal step with the same kind: `price -> round 0.05 eur`, `duration -> ceil 15 minute`, `rate -> floor 0.1%`. The result has the step's unit. Numbers too: `x -> round 0.01`. Prefix `round(x)` is a parse error.
- `->` binds looser than `+ - * / mod ^ as` and tighter than comparisons, `in`, `not`, `and`, `or`: `net * 1.21 -> round 0.01 eur` rounds the product. Arithmetic after a command is a parse error; wrap the command: `(fares -> sum) - 5 eur`. A bare command argument before a comparison is a parse error; use the call form: `price -> round(1 eur) > 10 eur`.
- List keywords are reserved in any case and cannot be data, rule, spec, alias, or unit names: `list`, `key`, `find`, `first`, `last`, `sum`, `avg`, `product`, `count`, `min`, `max`, `any`, `all`, `where`, `sort`, `top`, `asc`, `desc`. Also reserved: `lower`, `upper` (see **Units**), `is`, `mod`, `past`, `future`, `permille`, `or`, `that`.

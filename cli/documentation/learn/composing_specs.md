---
nav_title: Composing specs
nav_order: 60
---

# Composing Specs

Once a Spec can reference its own Data and Rules, it can import another Spec with Uses and reference its members through an alias. This chapter covers composition, temporal versions, planning checks, and (last) repositories and registry imports for advanced usage.

For syntax details see [Using other Specs](../reference/readme.md#using-other-specs).

## Importing another Spec

A Lemma source file defines one or more Specs. Each Spec is a namespace of Data and Rules. With Uses, you import another Spec under an alias and refer to its members through that alias. Read a value with `alias.field`, use a dependency Rule with `alias.rule_name`, and reuse an imported Data declaration as a parent type: `data country: iso.code` (qualified) or `data country: code` (bare, when the name is unambiguous across imports).

```lemma
spec premium_membership

data discount_rate: 10%

rule free_shipping_threshold: 50

rule monthly_bonus_points: 100


spec membership_benefits

uses membership: premium_membership

data monthly_spend: 150

rule discount: monthly_spend * membership.discount_rate

rule bonus_points: membership.monthly_bonus_points
```

- `uses membership: premium_membership` binds alias `membership` to Spec `premium_membership`.
- `uses premium_membership` uses an implicit alias equal to the full target name.
- `membership.discount_rate` reads Data from the imported Spec.
- `membership.monthly_bonus_points` uses a Rule from the imported Spec.

A minimal two-Spec example:

```lemma
spec base_employee

data name:   "John Doe"
data salary: 5000


spec manager

uses employee: base_employee
  -> with name: "Alice Smith"
  -> with salary: 8000

rule manager_bonus: employee.salary * 0.15
```

`uses alias: target` imports a Spec under an explicit alias; a bare `uses target` defaults the alias to the full target name. Read Data and Rules as `alias.field` or `alias.rule_name`.

## Setting Data on an import (`uses` block)

`uses` registers an import; it does not set runtime values on the dependency. Bind imported slots under the `uses` line with `-> with path: …` (path relative to the imported spec):

```lemma
spec inner

data x: number
  -> suggest 1


spec outer

uses i: inner
  -> with x: 42

rule r: i.x
```

- `-> with x: 42` sets Data `x` on `inner` to `42`.
- `-> with x: i` is an error: `i` is a Spec reference, not a value.
- Standalone `with copy: i.x` still parses but is deprecated; use `  -> with` under `uses` (see [Reference](../reference/readme.md#setting-data-on-an-imported-spec-uses-block)).

The binding path must be relative to the imported spec (`x`, not `i.x`). Local slots use `data`. Runtime inputs to `run` can still override bound import paths (e.g. `i.x`) where planning allows.

See [Setting data on an imported spec](../reference/readme.md#setting-data-on-an-imported-spec-uses-block).

## Temporal versions

The same Spec name may appear several times with different effective datetimes (`spec Name 2025-05-01`). Each declaration is immutable; you add a new row on the timeline instead of editing history in place.

```lemma
spec pricing

data base_price: 20
data quantity:        number

rule total: base_price * quantity


spec pricing 2025-01-01

data base_price: 25
data quantity:   number

rule total: base_price * quantity
```

Run with different effective dates:

```bash
lemma run pricing --effective 2024-06-01   # uses base_price: 20
lemma run pricing --effective 2025-06-01   # uses base_price: 25
```

Which row applies at evaluation time is determined by the `--effective` instant (CLI) or `Accept-Datetime` (HTTP) for the Spec you run.

### Unpinned vs pinned Uses

| Form | Meaning |
|------|--------|
| `uses dep` | Unpinned. Every temporal version of `dep` whose range intersects this Spec's range can matter. Planning may split this Spec into temporal slices at dependency `effective_from` boundaries. Values resolved through the import can change when `dep` gains a new row. |
| `uses dep 2025-06-01` | Pinned to an instant. One body of `dep` active at that datetime, including its transitive imports. Later rows of `dep` do not affect this edge. |

An unpinned import (`uses pricing`) follows the dependency's timeline; a pinned import (`uses p: pricing 2025-06-01`) freezes the dependency at that instant.

#### Unpinned: values follow the timeline

```lemma
spec policy

rule discount: 10


spec policy 2025-05-01

rule discount: 25


spec shop 2025-01-01

uses p: policy

rule d: p.discount
```

Evaluating `shop` in early 2025 yields `d = 10`; from May 2025 onward, `d = 25`. The Uses line is unchanged; the resolved policy body changes at the slice boundary.

A consumer with no effective date on its Spec line (origin) still gets slices when a dependency's `effective_from` falls inside its range.

#### Pinned: freeze a dependency at one instant

```lemma
spec finance

data money: measure
  -> unit eur: 1.00


spec finance 2025-07-01

data money: measure
  -> unit eur: 1.00
  -> unit usd: 0.91


spec shop 2025-01-01

uses f: finance 2025-02-01

data money: f.money
data price: money

rule doubled: price * 2
```

`shop` keeps the February 2025 finance shape (EUR only) even when you evaluate in September 2025. Pinning locks a tariff or regulation snapshot into the consumer.

## Planning checks

Before `run`, planning validates the dependency graph. Two temporal failures authors see most often:

### Temporal coverage (gaps)

Unpinned `uses dep` requires `dep` to exist for every instant in the consumer's temporal range. If the consumer starts in January but `dep` only exists from July, planning fails (no active version / not active at that instant).

Fix: add an earlier `spec dep` row, move the consumer's `effective_from` later, or pin `uses dep 2025-08-01` so the consumer no longer requires `dep` to cover its whole range.

### Interface compatibility (contract changes)

When unpinned imports span multiple rows of the same dependency, every row the consumer touches must expose compatible types for the same names (Rule result types, Data types, compatible measure units, etc.). New names only in a later row are fine if the consumer does not use them.

Incompatible example: `money` gains a `usd` unit in a later `finance` row while `shop` still has unpinned `uses finance` and `data price: finance.money`. Planning reports that the dependency changed its interface between temporal slices.

Fix: pin `uses f: finance 2025-02-01`, or add a new temporal row of `shop` written for the new finance body.

Coverage is presence on the timeline; interface validation is type sameness across slices the consumer needs.

### Same name, different bodies

You may import an earlier temporal row that shares your base name:

```lemma
spec finance 2026-01-01

data rate: 1


spec finance 2027-01-01

uses f26: finance 2026-01-01

rule ok: f26.rate
```

Planning rejects a Uses edge that resolves to the same Spec body (self-reference):

```lemma-skip
spec finance

uses finance


spec finance 2026-01-01

uses finance 2026-01-01
```

A later row must not use unpinned `uses finance` when that resolves to itself at that row's instant. Pin an earlier row (`uses f26: finance 2026-01-01`) instead.

Cycles across temporal rows (2026 → 2027 → 2026) are rejected as dependency cycles.

## Repositories

When Specs in the same project share a name, Repo blocks namespace them so they do not collide. Cross-repo targets use a repo qualifier on the Uses line:

```lemma
repo accounting

spec invoice

data total: 1


spec billing

uses inv: accounting invoice

rule out: inv.total
```

Cross-repo targets use a repo qualifier on the Uses line (`accounting invoice`). When you `run` a Spec from the default repository, use its unqualified name; the CLI does not pick between two loaded Specs with the same name in different repos.

Most projects never need Repo blocks: files without one belong to the default repository. Repositories installed from LemmaBase live under their own `@owner/name` names (see [LemmaBase](#lemmabase) below).

## LemmaBase

When Specs live outside your project, import them from LemmaBase with `@owner/repo` qualifiers:

```lemma
spec invoicing

uses @iso/countries alpha2

data price: measure
  -> unit eur: 1

data country: alpha2.code

rule tariff: 0 eur
  unless country is "NL" then price * 5%

rule total: price + tariff
```

Install repositories before running:

```bash
lemma install --all           # install all @... repositories into lemma_deps/
lemma install @iso/countries -f   # force re-install if content changed
```

Importing from LemmaBase uses `@owner/name` on the `uses` line (for example `uses iso: @iso/countries alpha2`). `Engine` does not auto-fetch; install with `lemma install` (or your SDK's `install`) first, then load. See [LemmaBase](../reference/registry.md).

## Evaluating a composed Spec

You always `run` (or call the API for) a named root Spec in a repository, with an effective instant:

```bash
lemma run membership_benefits --effective 2025-03-01
```

- The engine selects the consumer's temporal slice that contains that instant.
- Unpinned imports: paths such as `p.discount` use dependency bodies resolved for that slice.
- Pinned imports: the dependency body stays at the pinned instant.

To inspect the declared data catalog and rules for that temporal slice (`needed_by_rules` empty = reuse-only):

```bash
lemma show membership_benefits --effective 2025-03-01
```

Run-data-aware discovery of what a concrete `run` still needs comes from each rule's `missing_data`, not from `show`. Library specs with no rules still list their data slots on `show` for reuse inspection.
## Quick decision guide

| Goal | Pattern |
|------|---------|
| Track compatible dependency rows across the consumer's lifetime | `uses dep` (unpinned) |
| Lock a regulation / tariff / spec version at a known date | `uses dep 2025-06-01` |
| Import an earlier row of the same Spec name | `uses prev: finance 2026-01-01` |
| Reuse a Data shape from a library | `uses iso: @iso/countries alpha2` and `data x: iso.code` |
| Set Data on an imported Spec | `uses alias: spec` then `  -> with path: value` |
| Read import Data in a Rule | `rule r: alias.field` |

If planning fails, check whether the message is coverage, interface, self-reference, or cycle. Each remedy is different (see above).

## Next up

[Numeric precision](precision.md): exact rational arithmetic, limits, and client parsing.

---
nav_title: Learn
nav_order: 30
---

# Learn Lemma

Lemma evaluates your rules deterministically, every time, and can show why. This guide explains all of the features the language has to offer.

### 1. Getting started

Start with [Getting started](getting_started.md). It walks you through installing Lemma, writing your first Spec, and evaluating it from the CLI.

### 2. Specs, Data, and Rules

[Specs, Data, and Rules](specs_data_rules.md) explains how a Spec is structured. You learn about open inputs, constraints, and how Rules reference each other.

### 3. Conditional logic

[Conditional logic](conditional_logic.md) covers Unless chains and boolean literals, which is how a Rule chooses between outcomes.

### 4. Types and units

[Types and units](types_and_units.md) is the widest chapter. It covers literals, operators, the stdlib units, conversions, ranges, and dates, and it introduces [Veto](types_and_units.md#veto).

### 5. Extending Data

[Extending Data](extending_data.md) shows how to build on parent types, apply Data commands, and reuse definitions across Specs.

### 6. Composing Specs

[Composing Specs](composing_specs.md) brings everything together with Uses, temporal versions, planning checks, repositories, and the registry.

### 7. Lists

[Lists](lists.md) covers value lists and instance lists, keys and `find`, list commands such as `sum` and `where`, lists of a Spec's own rows, and how lists travel in evaluation data and results.

### 8. Numeric precision

[Numeric precision](precision.md) closes the guide by explaining exact rational arithmetic and how clients should parse the numbers Lemma returns.

### Language reference

For exhaustive syntax, operators, and Data commands, see the [Language reference](../reference/readme.md).

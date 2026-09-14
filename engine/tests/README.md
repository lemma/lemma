# Engine integration tests

Rust integration tests for `lemma-engine`. Each `*.rs` file here is a separate test binary (public API only). Unit tests belong in `engine/src/**` under `#[cfg(test)]` (or the shared `engine/src/tests/` harness for private helpers).

Published line-coverage totals and per-module tables: [`cli/documentation/reference/coverage/engine.md`](../../cli/documentation/reference/coverage/engine.md) (regenerate with `cargo coverage engine`).

Run:

```bash
cargo nextest run -p lemma-engine --tests
```

## Layout elsewhere

| Location | Role |
|----------|------|
| `engine/src/**` `mod tests` | Unit tests (private API) |
| `engine/src/tests/` | Unit tests wired from `lib.rs` (AST, bindings, engine, error, serializers) |
| `cli/tests/` | CLI / MCP / HTTP black-box tests |
| `engine/packages/npm/test.js` | WASM npm contract |
| `engine/packages/hex/test/` | Hex NIF ExUnit |
| `engine/fuzz/` | `cargo-fuzz` targets (workspace excluded) |

## Catalog by subsystem

### Evaluation, veto, missing data

| File | Focus |
|------|--------|
| `veto.rs` | Veto propagation, unless interaction |
| `missing_data_propagation.rs` | Missing data through rules |
| `branch_aware_missing_data.rs` | Per-rule `missing_data` and missing-data veto propagation |
| `per_rule_missing_data.rs` | Per-rule `missing_data` contract matrix: overlays, requested rules, unless pruning, And Propagate, Product/Comparison sibling walks, short-circuit release |
| `linear_rule_chain_stack.rs` | Linear rule-chain stack safety (tip-only eval depth; load NF depth) |
| `load_scaling.rs` | Deep/wide/unless load wall-clock bounds; wide show transitive needed_by pin |
| `rule_ref_schedule.rs` | Requested-rule missing_data through unless and rule references (And left, Piecewise condition); cross-spec requested run |
| `eval_allocations.rs` | `SHIPPING_EVAL_ALLOCATIONS` pin: allocations per `Engine::run` on the shipping fixture |

### Temporal versioning & planning

| File | Focus |
|------|--------|
| `temporal_slicing.rs` | Multi-slice execution plans |
| `temporal_range_references.rs` | Range refs across versions |
| `temporal_type_resolver_instant.rs` | Parent type resolution at slice instant |
| `temporal_timezone_ordering.rs` | TZ ordering across slices |
| `temporal_interface_deep_slice.rs` | Deep temporal interface |
| `temporal_boundary_explosion.rs` | Boundary cases |
| `temporal_cycle_panic.rs` | Invalid temporal cycles |
| `spec_reference_scenarios.rs` | Composability contracts (unpinned/pinned `uses`, coverage, self-ref, `with`; see [composing_specs.md](../../cli/documentation/learn/composing_specs.md)) |
| `temporal_self_uses.rs` | Cross-temporal same-name `uses` (implicit or explicit alias) |
| `type_import_temporal.rs` | Type-only deps + temporal versions |
| `spec_name_repository_plan_collision.rs` | Cross-repo spec name collision (regression) |
| `uses_lemma_compound_unit_planning.rs` | Compound units via `uses lemma units` |
| `validator_type_checking.rs` | Type validation at plan time |
| `semantic_validation.rs` | Semantic checks |
| `duration_trait_planning.rs` | Duration trait planning |

### Datetime, duration, calendar

| File | Focus |
|------|--------|
| `datetime_sugar.rs` | `now`, `in past`, calendar sugar |
| `datetime_edge_cases.rs` | Date/time edge cases |
| `datetime_edge_hunting.rs` | Adversarial datetime scenarios |
| `timezone.rs` | Timezone parsing/eval |
| `date_range.rs` | Date ranges |
| `calendar_duration_split.rs` | Calendar vs duration split |
| `duration_trait_anonymous.rs` | Anonymous duration types |
| `duration_trait_arithmetic.rs` | Duration arithmetic |
| `duration_trait_temporal.rs` | Duration + temporal |
| `duration_trait_precision.rs` | Duration precision |

### Arithmetic, units, quantities, ratios

| File | Focus |
|------|--------|
| `arithmetic_exactness.rs` | Division planning/runtime behavior; measure wage integration |
| `arithmetic_type_combinations.rs` | Typed arithmetic matrix |
| `math_ops.rs` | Math operators |
| `modulo_power.rs` | `%` and power |
| `equal_operator.rs` | Equality |
| `type_aware_arithmetic.rs` | Type-aware ops |
| `measure_unit_conversion.rs` | Unit conversion + validation bounds |
| `measure_number_refactoring.rs` | Measure/number interactions |
| `measure_duration_arithmetic_types.rs` | Measure + duration types |
| `unified_ratio_units.rs` | Ratio vs measure units |
| `ratio_runtime_input.rs` | Runtime ratio input grammar |
| `multidim_unit_system.rs` | Multi-dimensional units |
| `unit_percentage_operations.rs` | Percentage on units |
| `decimal_storage_pipeline.rs` | Decimal storage in results/JSON |

### Data, bindings, literals, show

| File | Focus |
|------|--------|
| `data_literals_coverage.rs` | Inline literal RHS on data |
| `data_binding_type_validation.rs` | Binding type checks |
| `data_type_declarations_coverage.rs` | Type declarations on data |
| `data_nested_bindings_coverage.rs` | Nested bindings |
| `data_references.rs` | Cross-data references |
| `data_with_values_contract.rs` | Run data contract |
| `typed_values.rs` | Typed value handling |
| `show_suggestion_distinction.rs` | Show vs suggestions (`-> suggest` never commits) |
| `show_only_rule_used_data.rs` | Show lists declared data; needed_by_rules marks intake vs reuse |
| `show_rule_graph.rs` | Show.rules is ShowRule: branches, depends_on_rules, rule-target with-binding |
| `show_with_bindings.rs` | Show + `with` / fill bindings |
| `show_unless_last_wins_pruning.rs` | Show after static unless collapse |
| `show_unit_constraints.rs` | Show unit / suggest magnitudes |
| `show_uncommittable_bounds.rs` | Show bounds that cannot commit |
| `accept_reject_removed.rs` | `accept` / `reject` keywords removed |
| `overlay_veto_as_value.rs` | Run data veto as value |
| `meta_fields.rs` | Meta fields on specs |

### Explanation / NormalForm eval

| File | Focus |
|------|--------|
| `explanation_e2e.rs` | Explanation / operation traces |
| `explanation_format.rs` | Explanation formatting + JSON |
| `explanation_parity.rs` | Explain true/false value parity |
| `explanation_complete_narration.rs` | Piecewise / algebra / conversion explanation contracts |
| `explanation_provenance_oracles.rs` | Origin / rule-ref explanation oracles |
| `tree_evaluator.rs` | NormalForm tree evaluator |
| `normalization_strict_semantics.rs` | Normalize contracts |
| `transitive_normalization.rs` | Cross-rule sharing / fold contracts |

### Spec graph, imports, references

| File | Focus |
|------|--------|
| `nested_spec_references.rs` | Nested `uses` / refs |
| `cross_spec_references.rs` | Cross-spec refs |
| `inline_type_imports.rs` | Inline type imports |
| `type_definitions.rs` | User type definitions |
| `required_data_names_nested_spec.rs` | Required data in nested specs |
| `repo_keyword.rs` | `repo` keyword |

### Parsing, syntax, errors

| File | Focus |
|------|--------|
| `expression_syntax.rs` | Expression grammar |
| `error_messages.rs` | Error text/locations |

### Formatting

| File | Focus |
|------|--------|
| `formatter.rs` | Formatter behavior |
| `format_weather_clothing_integration.rs` | Formatter integration example |

### End-to-end / examples

| File | Focus |
|------|--------|
| `integration_comprehensive.rs` | Broad integration scenarios |
| `integration_examples.rs` | CLI example `.lemma` files |
| `coffee_order.rs` | Coffee order example |

### Registry, WASM, limits

| File | Focus |
|------|--------|
| `engine_snapshot.rs` | Binary snapshot round-trip, determinism, corrupt/stale bytes, wide N=5000 |
| `load_batch_wasm_planning_parity.rs` | npm (WASM binding) vs native planning parity for batch loads |
| `repro_finance_dual_slice_registry_uses.rs` | Registry + dual slice |
| `resource_limits_test.rs` | Resource limits |

### Fuzz smoke

| File | Focus |
|------|--------|
| `duration_conversion.rs` | `uses lemma units` duration unit conversion |
| `fuzz_api_surface.rs` | Fuzz-related API smoke |

### Repro / regression guards

| File | Focus |
|------|--------|
| `repro_named_types_panic.rs` | Named types panic |
| `repro_finance_dual_slice_registry_uses.rs` | Finance/registry slice |

---

## `engine/src` modules without inline unit tests

Coverage for these lives mainly in this directory (integration) or in `engine/src/tests/`:

| Module | Notes |
|--------|--------|
| `computation/arithmetic.rs` | |
| `computation/comparison.rs` | |
| `computation/range.rs` | |
| `computation/decimal_math.rs` | |
| `computation/mod.rs` | |
| `evaluation/expression.rs` | |
| `evaluation/explanations.rs` |
| `literals.rs` | Partial coverage via `src/tests/ast.rs` |
| `parsing/parser.rs` | Lexer/AST have unit tests |
| `serialization/mod.rs` | `json.rs` has unit tests |
| `limits.rs`, `deps.rs`, `stdlib.rs`, `wasm.rs` | |

Prefer adding **unit** tests beside the module when testing private helpers; add **integration** tests here when exercising full load → plan → run paths.

---

## Semantics audit

### Unit vs integration split

| Layer | Where | API | Typical assertion |
|-------|--------|-----|-------------------|
| Unit | `engine/src/**` `#[cfg(test)]`, `engine/src/tests/` | Private + public | Parser nodes, planner invariants, `LiteralValue`, serde |
| Integration (this dir) | `engine/tests/*.rs` | Public `lemma::*` only | `Engine::load` → `run` / `show` / `invert` |
| CLI | `cli/tests/integrations/*` | `lemma` binary, MCP JSON | stdout, HTTP, tool payloads |
| WASM npm | `engine/packages/npm/test.js` | JS `Engine` | Shape of load/run/list/show |
| Hex | `engine/packages/hex/test/` | `Lemma.*` NIF | Lifecycle, list groups, run JSON |

**Integration binaries:** one per `engine/tests/*.rs` file (**144** as of this refresh). Run `cargo nextest run -p lemma-engine --lib` / `--tests` for current test counts.

### Primary APIs used here

| API | ~Files | Role |
|-----|--------|------|
| `Engine::load` | most | Parse + plan workspace sources |
| `Engine::run` | many | Evaluate rules |
| `Engine::show` | many (`show_*`, response/data contracts) | Spec interface API without full eval |
| `Engine::load` (batch) | few | Dependency bundles (multi-source load) |
| `parse` / `format_source` | 2 | `formatter.rs`, `format_weather_clothing_integration.rs` (no `Engine`) |

### Unit-test coverage by production module

| Module | Inline unit tests | Integration backstop |
|--------|-------------------|----------------------|
| `parsing/` (lexer, ast, mod) | Heavy (~110+) | `expression_syntax`, `error_messages`, `data_literals_coverage` |
| `planning/` (graph, normalize, semantics, execution_plan) | Heavy (~150+) | `temporal_*`, `validator_*`, `uses_lemma_*`, `type_definitions` |
| `computation/datetime`, `rational`, `units` | datetime/rational heavy | `datetime_*`, `arithmetic_*`, `measure_*`, `ratio_*` |
| `computation/arithmetic`, `comparison`, `range` | **None** | `arithmetic_type_combinations`, `math_ops`, `range_*`, `equal_operator` |
| `evaluation/expression`, `explanations` | **None** | E2E via `run` in most files |
| `engine.rs` | ~32 | `integration_*`, `coffee_order`, registry repros |
| `formatting/` | ~25 | `formatter`, `format_weather_clothing_integration` |
| `registry` | ~8 | `repo_keyword`, `load_batch_wasm_planning_parity`, repros |
| `literals.rs` | via `src/tests/ast.rs` | `data_literals_coverage`, `ratio_runtime_input` |

### Overlap clusters (consolidation candidates)

When changing behavior, run the whole cluster: scenarios often duplicate.

| Cluster | Files |
|---------|--------|
| Datetime eval | `datetime_sugar`, `datetime_edge_cases`, `datetime_edge_hunting`, `timezone`, `date_range` |
| Duration trait | `duration_trait_anonymous`, `_arithmetic`, `_precision`, `_temporal`, `_planning` |
| Measure / ratio | `measure_unit_conversion`, `measure_number_refactoring`, `measure_duration_arithmetic_types`, `unified_ratio_units`, `ratio_runtime_input`, `unit_percentage_operations`, `type_aware_arithmetic`, `multidim_unit_system` |
| Decimal eval precision | `measure_unit_conversion` (`precision_*` stress: prime chains from 37, API unit toggles, mixed `*`/`/`); `arithmetic_exactness` (`runtime_data_ten_divide_three_*`) |
| Arithmetic | `arithmetic_type_combinations`, `arithmetic_exactness`, `math_ops`, `modulo_power`, `equal_operator` |
| Range | `range_generic`, `range_semantics_table`, `date_range` |
| Spec graph | `nested_spec_references`, `cross_spec_references`, `required_data_names_nested_spec`, `inline_type_imports` |
| Temporal | `temporal_slicing`, `type_import_temporal`, `temporal_range_references`, `temporal_type_resolver_instant`, `temporal_timezone_ordering`, `temporal_interface_deep_slice`, `temporal_boundary_explosion` |
| Registry / plan identity | `spec_name_repository_plan_collision`, `repro_finance_dual_slice_registry_uses`, `load_batch_wasm_planning_parity` |
| Example E2E | `coffee_order`, `integration_examples`, `integration_comprehensive` |
| Data QA matrix | `data_literals_coverage`, `data_type_declarations_coverage`, `data_binding_type_validation`, `data_with_values_contract`, `data_nested_bindings_coverage`, `data_references` |

### Regression guards and intentional reds

| File | Intent |
|------|--------|
| `spec_name_repository_plan_collision.rs` | **Passes** when `get_plan` returns distinct plans per repository for same basename; **fails** if `plan_sets` aliases by name only (see module doc) |
| `repro_finance_dual_slice_registry_uses.rs` | Same for WASM batch `load` + duplicate `finance` basename |
| `data_literals_coverage.rs` | Pins literal RHS invariants; header says some cases may stay red; currently all green |
| `data_type_declarations_coverage.rs`, `data_binding_type_validation.rs`, `data_references.rs` | Constraint matrices; do not weaken assertions |
| `temporal_slicing.rs` | TDD guard for multi-slice planning |
| `repo_keyword.rs` | Signals until `repo` semantics complete |
| `resource_limits_test.rs` | `#[ignore]` on `performance_test_10k_rules`, `bench_deep_chains` (manual benches) |

No integration file uses `#[should_panic]`.

### Gaps and recommendations

| Gap | Recommendation |
|-----|----------------|
| No `computation/*` unit tests except datetime/rational/units | Add unit tests beside `arithmetic.rs` / `comparison.rs` for edge cases; keep integration matrix |
| `evaluation/expression.rs` untested in isolation | Unit-test eval of individual ops; integration already heavy via `run` |
| Duplicate example runners | `integration_examples` vs CLI `documentation_examples`: different roots; keep both |
| Stale “must fail” comments | `spec_name_*` / `repro_finance_*` **pass** when fixed; comments describe failure mode if bug returns |

### CLI integration map

See [cli/tests/README.md](../../cli/tests/README.md): `run`, `mcp`, `server`, `examples`: 57 tests, black-box on the `lemma` binary.

### Shared integration helpers

[support/](support/): per-file helpers (`get_rule_value`, `eval_rule_bool`, `make_effective*`). Each integration test imports only the files it needs via `#[path = "support/..."]`.

# Fuzzing Lemma Parser

Fuzz targets for malformed and adversarial Lemma input.

## Setup

```bash
nix develop
```

## Run like CI

Pull requests to `main` run the full gate, the coverage-report check, then 5 minutes of fuzz total (split evenly across targets):

```bash
cargo precommit --coverage --fuzz
```

Bare `cargo precommit` skips the coverage check and fuzz. `cargo precommit --fuzz` runs fuzz without the coverage check.

## Running Fuzz Tests Manually

### Quick Test (30 second)

```bash
cd engine/fuzz
mkdir -p corpus/fuzz_parser
cargo +nightly fuzz run fuzz_parser corpus/fuzz_parser seeds/fuzz_parser -- -max_total_time=30 -dict=lemma.dict -max_len=65536
```

### Full Fuzzing Session (1 hour)

From the repo root (all targets, 1 hour total). `fuzz_parser` gets half of that hour. The other four targets share the rest. `cargo precommit --fuzz` still splits its 5 minutes evenly.

```bash
cargo fuzzing
# or: cargo fuzzing --seconds 3600
```

`cargo fuzzing` refreshes `seeds/fixtures/` from the real `.lemma` documents under `engine/tests/fixtures`, `engine/documentation/examples`, `engine/benches/specs`, and `cli/tests/integrations/examples`. Documents larger than 32KiB are skipped (`engine/benches/specs/iso_countries.lemma` is a 775KiB catalog). Those fixtures are also embedded in the `fuzz_parser` mutator, so a direct `cargo fuzz run` still starts from them.

Single target for a full hour:

```bash
mkdir -p corpus/fuzz_parser
cargo +nightly fuzz run fuzz_parser corpus/fuzz_parser seeds/fuzz_parser -- -max_total_time=3600 -dict=lemma.dict -max_len=65536
```

### Available Targets

1. `fuzz_parser` - Lemma source. About one mutation in four is raw libFuzzer noise. The rest keeps a document that already contains `spec`, or restarts from an embedded fixture, then applies one to three local edits (literals, operators, wrappers, casts, `unless`, types, units, list commands, deleted `->` lines, swapped identifiers, effective dates, date windows, `uses lemma units`, `is veto`). On every input: `format_source` (idempotent when it parses), `load`, then for every loaded spec at `2024-06-15T12:00:00Z` and at that spec's `effective_from` when it differs: `show`, `source`, and `evaluate` twice with `explain: true`. Explanations format, and Show and Response JSON must serialize. Pass writable `corpus/fuzz_parser` first, then `seeds/fuzz_parser/` and `seeds/fixtures/`; pass `-dict=lemma.dict -max_len=65536`.
2. `fuzz_expressions` - Structured typed expressions inside a fixed preamble: arithmetic, `unless`, list pipelines, `sqrt`/`log`/`exp`/`sin` and the other math prefixes, `as`, `is veto`, `now`/`past`/`future`, `lower`/`upper`, ratios, and `(lo...hi) as day as number`. Depth stays at or below 7. Same format/show/evaluate path as `fuzz_parser`.
3. `fuzz_literals` - Structured literal generator (number, long digit strings, text, measure, ratio including `0%` and `100%`, far dates, time, boolean, range) spliced into a one-line `data` declaration. Same production path.
4. `fuzz_data_bindings` - Multi-type and instance-list data overlays: varied keys (dotted, numbered, case-variant, unknown) and Literal/Boolean/Map/List shapes; duplicate map keys black-box the Err and continue. Overlays run through the same show/evaluate path with `explain: true`.
5. `fuzz_lists_uses` - Instance-list and value-list pipelines (`where` / `sort` / `sum` / `product` / `any` / `all` / `top` / `find` / `first` / `last`, including an empty list) and multi-spec `uses` composition. Same production path.

### Running Specific Targets

```bash
cargo +nightly fuzz run fuzz_expressions -- -max_total_time=60 -dict=lemma.dict
cargo +nightly fuzz run fuzz_literals -- -max_total_time=60 -dict=lemma.dict
cargo +nightly fuzz run fuzz_data_bindings -- -max_total_time=60 -dict=lemma.dict
cargo +nightly fuzz run fuzz_lists_uses -- -max_total_time=60 -dict=lemma.dict
```

### Reproducing Crashes

When a crash is found, it's saved to `artifacts/<target>/crash-*`:

```bash
cargo +nightly fuzz run fuzz_parser artifacts/fuzz_parser/crash-abc123 -- -dict=lemma.dict -max_len=65536
```

### Minimizing Test Cases

```bash
cargo +nightly fuzz tmin fuzz_parser artifacts/fuzz_parser/crash-abc123
```

## Known Issues

### Memory Leaks

The fuzzer may report memory leaks from global initialization. These are typically **expected and harmless**:

- Global static storage for atom tables and arenas
- These are one-time allocations, not per-operation leaks

To run without leak detection:

```bash
ASAN_OPTIONS="detect_leaks=0" cargo +nightly fuzz run fuzz_parser corpus/fuzz_parser seeds/fuzz_parser -- -dict=lemma.dict -max_len=65536
```

## Interpreting Results

- **Crashes (exit code 1)**: Parser panic or assertion failure - **needs fixing**
- **Timeouts**: Input causes excessive computation - review for DOS potential
- **Leaks (exit code 77)**: Usually from global initialization - **can ignore**

## Adding New Targets

1. Create `fuzz_targets/fuzz_your_target.rs`
2. Add a `[[bin]]` entry in `Cargo.toml`
3. Follow the pattern from existing targets (fixed `DateTimeValue`, serialize-must-succeed, evaluate twice for determinism)
4. Add the target name to `FUZZ_TARGETS` in `xtask/src/main.rs` so `cargo precommit --fuzz` / CI runs it

## Continuous Fuzzing

For long-running fuzzing campaigns, put the writable corpus first so new inputs are not written into curated seeds:

```bash
mkdir -p corpus/fuzz_parser
cargo +nightly fuzz run fuzz_parser corpus/fuzz_parser seeds/fuzz_parser -- -max_total_time=86400 -dict=lemma.dict -max_len=65536
```

`cargo fuzzing --seconds 86400` gives `fuzz_parser` half of that budget and passes `seeds/fixtures` as a third corpus. Interesting inputs land under `corpus/` (gitignored).

# Fuzzing Lemma Parser

Fuzz targets for malformed and adversarial Lemma input.

## Setup

```bash
rustup install nightly
cargo install cargo-fuzz
```

## Run like CI

Push to `main` runs the full gate, the coverage-report check, then 5 minutes of fuzz total (split evenly across targets):

```bash
cargo precommit --coverage --fuzz
```

Bare `cargo precommit` skips the coverage check and fuzz. `cargo precommit --fuzz` runs fuzz without the coverage check.

## Running Fuzz Tests Manually

### Quick Test (30 second)

```bash
cd engine/fuzz
mkdir -p corpus/fuzz_parser
cargo +nightly fuzz run fuzz_parser corpus/fuzz_parser seeds/fuzz_parser -- -max_total_time=30 -dict=lemma.dict
```

### Full Fuzzing Session (1 hour)

From the repo root (all targets, 1 hour total, split evenly):

```bash
cargo fuzzing
# or: cargo fuzzing --seconds 3600
```

Single target for a full hour:

```bash
mkdir -p corpus/fuzz_parser
cargo +nightly fuzz run fuzz_parser corpus/fuzz_parser seeds/fuzz_parser -- -max_total_time=3600 -dict=lemma.dict
```

### Available Targets

1. `fuzz_parser` - Full spec parsing with random input; on successful load, evaluate twice with a fixed effective datetime and `explain: true`, require explanations on non-vetoed results, format explanations, and serialize the API Response (serialize failure fails the fuzzer). Pass writable `corpus/fuzz_parser` first, curated seeds in `seeds/fuzz_parser/` second; pass `-dict=lemma.dict`.
2. `fuzz_expressions` - Structured typed leaf/operator expressions (numbers, measures, booleans, text), `unless` chains, list pipelines, and deep nesting inside a fixed typed preamble; load then evaluate twice for determinism
3. `fuzz_literals` - Structured literal generator (number, text, measure, ratio, date, time, boolean, range) spliced into a one-line `data` declaration
4. `fuzz_data_bindings` - Multi-type and instance-list data overlays: varied keys (dotted, numbered, case-variant, unknown) and Literal/Boolean/Map/List shapes; duplicate map keys black-box the Err and continue
5. `fuzz_lists_uses` - Structured list pipelines (`where` / `sort` / `sum` / `find` / …) and multi-spec `uses` composition inside a fixed preamble

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
cargo +nightly fuzz run fuzz_parser artifacts/fuzz_parser/crash-abc123 -- -dict=lemma.dict
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
ASAN_OPTIONS="detect_leaks=0" cargo +nightly fuzz run fuzz_parser corpus/fuzz_parser seeds/fuzz_parser -- -dict=lemma.dict
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
cargo +nightly fuzz run fuzz_parser corpus/fuzz_parser seeds/fuzz_parser -- -max_total_time=86400 -dict=lemma.dict
```

This will save interesting inputs under `corpus/` (gitignored) and gradually improve coverage over time.

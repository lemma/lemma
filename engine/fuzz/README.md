# Fuzzing Lemma Parser

Fuzz targets for malformed and adversarial Lemma input.

## Setup

```bash
rustup install nightly
cargo install cargo-fuzz
```

## Run like CI

CI runs the full gate then 5 minutes of fuzz total (split evenly across targets):

```bash
cargo precommit --fuzz
```

Bare `cargo precommit` skips fuzz (local shortcut only).

## Running Fuzz Tests Manually

### Quick Test (30 second)

```bash
cd engine/fuzz
cargo +nightly fuzz run fuzz_parser -- -max_total_time=30
```

### Full Fuzzing Session (1 hour)

```bash
cargo +nightly fuzz run fuzz_parser -- -max_total_time=3600
```

### Available Targets

1. `fuzz_parser` - Full spec parsing with random input
2. `fuzz_expressions` - Expression parsing within valid spec context
3. `fuzz_literals` - Literal value parsing (numbers, strings, units)
4. `fuzz_deeply_nested` - Nested expressions to test stack limits
5. `fuzz_data_bindings` - Data binding parsing and evaluation
6. `fuzz_snapshot_decode` - Snapshot decoding of arbitrary bytes (malformed input returns `Err`; a decoded snapshot must encode again)

### Running Specific Targets

```bash
cargo +nightly fuzz run fuzz_expressions -- -max_total_time=60
cargo +nightly fuzz run fuzz_literals -- -max_total_time=60
cargo +nightly fuzz run fuzz_deeply_nested -- -max_total_time=60
cargo +nightly fuzz run fuzz_data_bindings -- -max_total_time=60
```

### Reproducing Crashes

When a crash is found, it's saved to `artifacts/<target>/crash-*`:

```bash
cargo +nightly fuzz run fuzz_parser artifacts/fuzz_parser/crash-abc123
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
ASAN_OPTIONS="detect_leaks=0" cargo +nightly fuzz run fuzz_parser
```

## Interpreting Results

- **Crashes (exit code 1)**: Parser panic or assertion failure - **needs fixing**
- **Timeouts**: Input causes excessive computation - review for DOS potential
- **Leaks (exit code 77)**: Usually from global initialization - **can ignore**

## Adding New Targets

1. Create `fuzz_targets/fuzz_your_target.rs`
2. Add a `[[bin]]` entry in `Cargo.toml`
3. Follow the pattern from existing targets
4. Add the target name to `FUZZ_TARGETS` in `xtask/src/main.rs` so `cargo precommit --fuzz` / CI runs it

## Continuous Fuzzing

For long-running fuzzing campaigns, use a corpus directory:

```bash
mkdir -p corpus/fuzz_parser
cargo +nightly fuzz run fuzz_parser corpus/fuzz_parser -- -max_total_time=86400
```

This will save interesting inputs and gradually improve coverage over time.

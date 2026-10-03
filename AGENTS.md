# AGENTS.md: Mandatory rules for AI agents working on Lemma

These are **non-negotiable rules**, not guidelines, for code and for the value you deliver while working. Violating any of them is a bug you introduced. This file does not describe Lemma's language; read `cli/documentation/`, `README.md`, and the codebase for that.

---

## Interaction protocol

These rules govern what you deliver to the user. They are as mandatory as the coding rules.

**Be useful.** The only measure of a response is the value it brings: correct work, clear requirements, concrete next steps. How the message sounds is irrelevant. Politeness, reassurance, and narration are not value.

### Default response on correction, rejection, or missed requirements

Unless the user **explicitly** asks for explanation, diagnosis, or retrospective, every response must open with:

1. **Requirements**: what must be true, in concrete terms.
2. **Next steps**: what you will do now.
3. **Execute**: do it. Do not ask permission unless blocked on a decision only the user can make.

The first paragraph of any correction response must be Requirements, not a retrospective. No preamble. No closing offers to do work you should already be doing.

### Forbidden unless explicitly requested

These add no value. Do not use them:

- Apologies
- Post-mortems ("what I did wrong", "the issue was…")
- Excuses or process narration ("I didn't search first")
- Affirmations or reassurance ("you're right", "good catch")
- Defending prior output
- "Would you like me to…" when the action is already clear
- Em dashes in any output, including this file

### When output was wrong or incomplete

Restate requirements. Propose fix. Implement. Prior attempt is irrelevant unless the user asks for analysis.

### When explanation is appropriate

Diagnosis and tradeoffs only when explicitly requested: "why", "explain", "what was wrong", "review this". Ask-mode how-it-works questions count. Corrections and redirects do not.

### During execution

Do not halt mid-task to summarize or recap. Report when done or when blocked.

Pre-existing blocker discovered during implementation: state blocker and decision needed. That is signal, not narration.

### Warnings and errors from tests, clippy, and Java

**NEVER** suppress warnings or errors: no `_` prefixes, no `#[allow(...)]`, no `#[expect(...)]`, no quick fixes that hide the problem. Fix the cause completely or leave the warning visible.

Precommit treats warnings as errors across Rust (`clippy -D warnings`), npm (`npm warn` / `WARNING` lines), and Maven (`[WARNING]` lines plus `maven-compiler-plugin` `-Werror` / `failOnWarning` and `maven-javadoc-plugin` `failOnWarnings` with `doclint` `all`).

---

## The 10 Rules

### 1. Find existing code before writing new code

**Before writing any function, type, or code path, search the codebase for existing implementations that already do what you need.** If the value you need is already computed, use it. If a function already exists, call it. If a type already carries the data, use that type.

Do not write a parallel implementation. Do not re-derive a value that is already stored. Do not create a second function that transforms the same inputs into the same outputs. If you are about to do any of these things, STOP and find the existing code.

This is the single most common mistake. Treat discovery as mandatory.

### 2. Invariant violations crash: no exceptions

**NEVER write a soft error path (error-and-return, default value, empty fallback) for a condition that cannot happen if the code is correct.** Use `expect()`, `unreachable!()`, or `panic!("BUG: ...")`.

- `if x.is_none() { return Err(...) }` for an invariant = **WRONG**
- `x.unwrap_or(default)` for an invariant = **WRONG**
- `x.unwrap_or_else(|| panic!(...))` when `.expect()` works = **WRONG**
- `x.expect("BUG: x must be present after phase Y")` = **CORRECT**

A wrong value that silently propagates is infinitely worse than a crash. This is rocket-ship code. A crash is safe. A wrong value kills.

### 3. Error vs Veto vs panic: three-way split, no gray area

| Phase | Bad input / invalid spec | Domain "no value" | Bug / impossible state |
|-------|-------------------------|-------------------|----------------------|
| Parse/Planning | `Err(Error)` | none | `panic!` / `unreachable!` |
| Evaluation | none | `Veto(reason)` | `panic!` / `unreachable!` |

- **Error** = user wrote invalid Lemma. Return it with source location.
- **Veto** = valid spec, but data make a rule impossible (division by zero, missing data, user `veto`). Veto is a result, not an error.
- **panic/unreachable** = the code has a bug. Crash immediately with a `"BUG: ..."` message.

There is no fourth option. Do not invent one.

### 4. No silent defaults, heuristics, or guesses

If the semantics do not define a value, the code must Error, Veto, or panic. Never infer, guess, or substitute a default to "keep going." Never return an empty string, zero, `None`, or a fallback value when the real answer is "this shouldn't happen" or "this is undefined."

### 5. No placeholders anywhere

Placeholders in `.lemma` sources, documentation, tests, or examples are forbidden. No dummy values, no "TODO" literals, no fake numbers, no `"example"` strings. Use real domain values or omit and fail validation.

### 6. Validation before execution: always

Planning fully validates the spec. Only after planning succeeds does evaluation run. Invalid Lemma is rejected with Error during planning. Never "try to run and see."

After planning succeeds, evaluation is **guaranteed** to complete. The execution plan is self-contained. If something impossible happens during evaluation, it is a bug; panic.

### 7. Determinism is non-negotiable

Same spec + same data = same evaluation order = same results (or same Veto). Do not iterate over unordered collections in ways that affect output. Rules evaluate in topological order by dependency.

### 8. Failing tests are valuable: never suppress them

Failing tests reveal missing or broken functionality. Do not:
- Delete or skip a failing test to make the suite green
- Weaken an assertion to match wrong output
- Comment out a test "temporarily"
- Add `#[allow(...)]`, `#[expect(...)]`, or prefix identifiers with `_` to silence compiler, clippy, or test warnings

If a test fails, either fix the code or fix the test to match correct behavior. A red test is information. A deleted test is a hidden bug. Warnings and errors are information. Fix the cause or leave them visible. Never hide them.

### 9. Partial implementations must be guarded

If you cannot finish an implementation completely, every unfinished path must have a `todo!()` macro. Do not write code that compiles and runs but silently skips unimplemented branches. Rust will warn about unreachable code after `todo!()`; that warning is correct and intentional. A partial implementation that pretends to be complete is worse than one that refuses to compile.

### 10. Use `cargo nextest run`, not `cargo test`

Unit tests go in the same module as the code (to test private functions). Engine integration tests go in `engine/tests/`; CLI integration tests in `cli/tests/`. Hex ExUnit tests live in `engine/packages/hex/test/`. Always run with `cargo nextest run` (Rust tests only). CI runs `cargo precommit --fuzz`. Local shortcut: `cargo precommit` (no fuzz). The gate: versions-verify, mix/hex, vscode npm ci + publish CLI smoke, fmt, clippy `--all-features`, nextest, npm WASM build+test, Maven `./mvnw -B verify` after `lemma_jni`, NuGet `dotnet test` after `lemma_dotnet` + UniFFI generate, cargo-deny, coverage `--check`; `--fuzz` then runs `engine/fuzz` for 30 minutes total, split across targets (needs nightly + `cargo-fuzz`). JDK 21+ for Maven. .NET 8 SDK for NuGet. `uniffi-bindgen-cs` tag `v0.11.0+v0.31.0` (uniffi 0.31.0).

---

## Where to read more

| Resource | Purpose |
|----------|---------|
| **README.md** | Project overview, quick start |
| **xtask/README.md** | precommit, `cargo bump` / `cargo verify`, release paths |
| **cli/documentation/learn/readme.md** | Learn guide: language concepts |
| **cli/documentation/reference/readme.md** | Operators, types, literals, syntax |
| **cli/documentation/learn/types_and_units.md#veto** | Veto propagation rules |

# xtask

Workspace automation from the repo root.

| Command | Alias | Purpose |
|---------|-------|---------|
| `cargo run -p xtask -- [precommit] [--fuzz]` | `cargo precommit [--fuzz]` | `versions-verify`, mix precommit (hex: format, locked deps, `hex.audit`, compile, ExUnit), vscode `npm ci` + compile + vsce package + publish CLI smoke (`@node-rs/crc32`, `ovsx`, `vsce`; npm/Maven warnings are errors), `fmt --check`, clippy host + wasm32 (`-D warnings`), nextest (`--all-features`), npm WASM build+test, Maven `./mvnw -B verify` (after `lemma_jni` build; `-Werror` + `failOnWarnings` + `doclint=all` + `[WARNING]` gate), cargo-deny, `coverage all --check`. With `--fuzz` (CI): 30 minutes total across `engine/fuzz` targets (nightly + `cargo-fuzz`). Bare `cargo precommit` skips fuzz. |
| `cargo run -p xtask -- versions-verify` | `cargo verify` | Ensure release version matches everywhere (see below) |
| `cargo run -p xtask -- versions-bump <semver>` | `cargo bump <semver>` | Bump `[workspace.package] version` and all mirrored copies, then `cargo generate-lockfile`, `mix deps.get` (hex), `npm install --package-lock-only` (vscode) |
| `cargo run -p xtask -- versions-diff [semver]` | `cargo changelog [semver]` | `git fetch --tags`, then `git diff --stat`, `git log`, then `git diff`. **No arg:** latest release tag (`lemma-v*`, or legacy `cli-v*`) → **working tree** (includes uncommitted changes); log is `tag..HEAD`. **`versions-diff <semver>`:** previous tag → requested version's tag on history only. |
| `cargo benchmarks <engine\|cli\|all>` | `cargo run -p xtask -- benchmarks <suite>` | Run benchmark suites and write `cli/documentation/reference/benchmarks/engine.md` and/or `cli/documentation/reference/benchmarks/cli.md`. `engine`: Criterion evaluate/outputs + Python harness + `memory` and `snapshot` (logistics ladder, ~2 minutes) benches. `cli`: `http_evaluate` + `engine_profile`. Keep engine `FIXTURES` in `xtask/src/benchmarks/engine.rs` synced with `engine/benches/common/mod.rs`; CLI cases in `xtask/src/benchmarks/cli.rs` synced with `cli/benches/*.rs`. |
| `cargo coverage <engine\|cli\|all> [--check]` | `cargo run -p xtask -- coverage <suite> [--check]` | Without `--check`: run `cargo llvm-cov nextest` and write `cli/documentation/reference/coverage/engine.md` and/or `cli/documentation/reference/coverage/cli.md` (requires [`cargo-llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov) and `cargo-nextest`). With `--check`: verify committed reports match source inputs (no llvm-cov). `cargo precommit` runs `coverage all --check`. |
| `cargo run -p xtask -- llms` | | Regenerate `cli/documentation/llms.txt` from `engine/documentation/guide/*.md`. Checked-in copy must match engine embed (`cli/tests/integrations/llms_txt.rs`). |
| `cargo run -p xtask -- schema` | | Regenerate `engine/schemas/api.v1.json`. |

Aliases are in [`.cargo/config.toml`](../.cargo/config.toml) (`-q` on bump/verify/changelog reduces Cargo noise).

**Precommit prerequisites:** [`cargo-nextest`](https://nexte.st/), [`cargo-deny`](https://github.com/EmbarkStudios/cargo-deny), Elixir/Mix, Node.js **24** (VS Code extension `npm ci` in precommit; matches GitHub Actions `node-version: 24` and repo [`.tool-versions`](../../.tool-versions) for asdf), `wasm-pack` exact **0.15.0** (`cargo install wasm-pack --version 0.15.0 --locked`; must match CI `WASM_PACK_VERSION` / crates.io newest), JDK 21+ (Maven wrapper under `engine/packages/maven/`). For `--fuzz` / CI: Rust nightly (`rustup install nightly`) and [`cargo-fuzz`](https://github.com/rust-fuzz/cargo-fuzz). [`cargo-llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov) is only needed to regenerate coverage reports (`cargo coverage all`), not for precommit. Mix, VS Code packaging via xtask, and Maven precommits always run (not path-gated).

Release version must match in:

- `Cargo.toml` (`[workspace.package]`)
- Path dependency pins in `cli/`, `openapi/`, `engine/lsp/` `Cargo.toml` files (`lemma` / `lemma-openapi`, `=…` exact pins)
- `engine/packages/hex/mix.exs` (`@version`)
- `engine/packages/maven/pom.xml` (project `<version>`)
- Maven install snippets: root `README.md`, `engine/README.md`, `cli/documentation/tools/java.md`, `engine/packages/maven/README.md` (XML and/or Gradle coords)
- `engine/README.md` (quick-start `lemma-engine = "…"`)
- `engine/lsp/editors/vscode/package.json` (`version`)

Single source of truth for those paths: [`src/versions.rs`](src/versions.rs) module `tracked`.

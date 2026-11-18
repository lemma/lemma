# Lemma: Zed extension

Language support for `.lemma` files in [Zed](https://zed.dev). The extension runs `lemma lsp` over stdio, so diagnostics, completion, go to definition, formatting (format on save), and semantic tokens all come from the Lemma language server. A small lexical tree-sitter grammar (`grammars/tree-sitter-lemma`) provides baseline highlighting, bracket matching, and the outline (specs, repos, data, rules, meta).

## Prerequisites

Install the `lemma` CLI:

```bash
npm install -g lemma
# or: cargo install lemma
```

## Language server discovery

Same order as the VS Code extension:

1. `lsp.lemma.binary.path` from Zed settings (`arguments` default to `["lsp"]`; `env` is merged into the worktree shell environment).
2. `<worktree>/target/release/lemma`, then `<worktree>/target/debug/lemma`. If both run, the one whose `--version` matches the extension version wins. This probe uses the `process:exec` capability declared in `extension.toml`.
3. `lemma` on the worktree shell `$PATH`.

```json
{
  "lsp": {
    "lemma": {
      "binary": { "path": "/path/to/lemma" }
    }
  }
}
```

## Semantic highlighting

Zed turns LSP semantic tokens off by default. Turn them on for Lemma to get the full coloring the VS Code extension shows. The custom token types (`controlKeyword`, `declarationKeyword`, `dataBody`, `punctuation`, `reference`) map to theme styles in `languages/lemma/semantic_token_rules.json`.

```json
{
  "languages": {
    "Lemma": { "semantic_tokens": "combined" }
  }
}
```

## Development install

1. Build the CLI from the repo root: `cargo build --release -p lemma`.
2. In Zed run **zed: install dev extension** and pick `engine/lsp/editors/zed`. Zed compiles `src/lib.rs` for `wasm32-wasip2` and builds the grammar from the `[grammars.lemma]` `rev` in `extension.toml`.
3. Open a `.lemma` file. **dev: open language server logs** shows the `lemma lsp` session.

Zed builds the grammar from the committed `rev`, not from your working tree. To try uncommitted grammar edits, temporarily point `[grammars.lemma] repository` at `file:///<absolute repo path>` and `rev` at a local commit.

## Changing the grammar

1. Edit `grammars/tree-sitter-lemma/grammar.js`, then run `tree-sitter generate` and `tree-sitter test` in that directory (inside `nix develop`). Commit the regenerated `src/`.
2. Run `cargo run -p xtask -- zed grammar-rev` to pin `[grammars.lemma] rev` to that commit, then commit `extension.toml`.
3. Merge the pull request into `main` with a **merge commit**, not squash or rebase. Both of those rewrite the branch's SHAs, so the pinned `rev` would not be part of `main` or the release tag: `cargo verify` fails on `main` and the registry builds from a commit outside the release history.

`cargo precommit` (via `cargo run -p xtask -- zed check`) regenerates the parser and compares it with the committed `src/`. It also runs the corpus tests and parses every fixture `.lemma` file. Finally it formats, lints, tests, and builds the extension crate. `cargo verify` fails when `rev` does not contain the grammar or the grammar changed after `rev`. `engine/lsp/src/zed_drift.rs` keeps `highlights.scm` and `semantic_token_rules.json` in sync with the lexer keywords, constraint commands, and LSP token legend.

## Publishing

The extension is published in the [Zed extension registry](https://github.com/zed-industries/extensions) as a submodule of this repository, with `path = "engine/lsp/editors/zed"` in the registry's `extensions.toml`.

- **First submission (manual):** open a PR on `zed-industries/extensions` that adds this repository as the submodule `extensions/lemma` and an `extensions.toml` entry `[lemma]` with `submodule = "extensions/lemma"`, `path = "engine/lsp/editors/zed"`, and `version` equal to `extension.toml`'s `version`.
- **Later releases:** the `publish-zed` job in `.github/workflows/release.yml` opens the bump PR from a fork of `zed-industries/extensions`. It runs once the repository variable `ZED_EXTENSIONS_FORK` (`owner/extensions`) and the secret `ZED_COMMITTER_TOKEN` (a PAT with `repo` and `workflow` scopes) are set.

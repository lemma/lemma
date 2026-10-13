# Lemma LSP

Language Server Protocol implementation for [Lemma](https://github.com/lemma/lemma). Provides inline diagnostics and editor features for `.lemma` files.

## Features

- **Diagnostics**: Parse and planning (semantic) errors are published as you type. Parse errors are shown immediately; a debounced (250ms) full workspace validation adds planning errors. Errors use source spans where available; diagnostics are cleared when a file is closed. When the same spec name, repository, and `effective_from` are declared in two workspace files, each declaring file gets its own error naming the other path.
- **Workspace validation**: On native, when the client provides a workspace root, the CLI injects disk discovery (same gitignore-aware policy as `lemma specs`, always including `lemma_deps/`) and a filesystem watch into the LSP. The watch is planted only on discovered workspace `.lemma` files and `lemma_deps/` (not a recursive whole-tree watch). Cross-spec planning diagnostics are reported per file. Open/change/save/close update the editor buffer overlay; disk create/change under watched paths (for example after `lemma install`) is picked up by the CLI watch. On WASM, only the open file is validated (no filesystem).
- **Completion**: `textDocument/completion` offers context-aware items: lexer keywords, `->` commands from `TypeConstraintCommand`, local data/rule/`uses` names from a successful parse, and `Engine::show` / `Engine::specs` catalogs when planning succeeded. Member completion after `.` is not offered in this version.
- **LemmaBase hover**: Hovering on a registry `uses` reference shows a popup with a markdown link from `Registries::registry_for(...).navigation_url(...)` (LemmaBase is hard-bound to `https://lemmabase.com`).
- **LemmaBase links**: `@`-prefixed references (e.g. `uses @iso/countries alpha2`) are turned into clickable document links pointing at the locally fetched bundle under `<workspace>/lemma_deps/<qualifier>.lemma` at the resolved `spec ...` line.
- **Embedded stdlib navigation**: `uses lemma units` (and any other reserved `repo lemma` import) becomes a clickable link to a view-only snapshot at `<workspace>/lemma_deps/lemma.std`. The file is written lazily the first time `document_link` runs on a spec that imports it, and is excluded from every `.lemma` discovery pass because of its `.std` extension. The repository qualifier link opens the file; the spec name link opens it at the `spec ...` line. The VS Code extension registers the `lemma.std` filename so the snapshot still gets full Lemma syntax highlighting.
- **Text document sync**: Full file sync on open, change, and close; no incremental sync.

The server uses engine `Registries` navigation URLs for `@...` identifier hover/links and communicates over stdio (native) or browser streams (WASM).

## Build

From the **repository root**:

```bash
cargo build --release -p lemma
```

The LSP runs as `lemma lsp` (stdio).

The crate also supports a **WASM** build for in-browser use; the library entry point is `lsp::browser::serve`. The Lemma WASM playground does not use the LSP for diagnostics; it uses the engine’s `getDiagnostics` API directly for inline errors.

## Usage

Run the server over stdio:

```bash
lemma lsp
```

- **VS Code / Cursor**: Use the extension under [editors/vscode](editors/vscode). It runs `lemma lsp` automatically and looks for `target/release/lemma` when the workspace root is the Lemma repo, or uses the `lemma.lspServerPath` setting. Format-on-save and diagnostics only work when the LSP is running; see [editors/vscode/README.md](editors/vscode/README.md) if format on save does nothing.
- **Other editors**: Point your editor’s LSP client at `lemma` with argument `lsp` and stdio transport.

## Layout

- **`src/`**: LSP server (tower-lsp): server, diagnostics, completion, LemmaBase links, workspace model.
- **`editors/`**: Editor-specific clients and config (e.g. VS Code extension); see [editors/README.md](editors/README.md).

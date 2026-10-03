---
nav_title: Tools & SDKs
nav_order: 50
---

# Tools & SDKs

Ways to run Lemma, from embedding the engine in your app to driving it from the command line.

## Lemma SDKs

Embed Lemma directly in your language:

- [Rust](rust.md)
- [Elixir](elixir.md)
- [JavaScript / TypeScript](javascript.md)
- [Java / Kotlin](java.md)
- [C# / .NET](dotnet.md)
- [Python](python.md) (coming soon)

More SDKs are on the way. Precompiled binaries make each new one straightforward to add.

## Command line & servers

- [Lemma CLI](../reference/cli.md): `lemma run`, `show`, `list`, `format`, `install`, plus `server`, `lsp`, and `mcp`
- [Lemma MCP](mcp.md): connect Claude, Gemini CLI, or Cursor to your specs over the Model Context Protocol
- [LemmaBase](../reference/registry.md): install shared repositories via `@owner/name` imports on `uses`

## Notes that apply to every SDK

- Same engine underneath, so results are identical across languages.
- `load` validates: invalid specs are rejected there, never at run time.
- The engine never hits the network. Resolve `@...` [registry](../reference/registry.md) references before loading (hosts own the HTTP socket).
- Explanations are opt-in (`explain: true` / `--explain`). Wire shape: [`api.v1.json`](../../engine/schemas/api.v1.json) (`RuleResult.explanation` → `RuleNode`; nested tree under `ExplanationNode`).
- Java / Kotlin package requires **JDK 21+**.
- Editor support (any language): the [VS Code / Cursor extension](../installation.md) drives `lemma lsp`.

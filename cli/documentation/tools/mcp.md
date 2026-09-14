---
nav_title: MCP
parent: Tools & SDKs
nav_order: 70
---

# MCP

Connect Claude, Gemini CLI, or Cursor to your Lemma specs over the [Model Context Protocol](https://modelcontextprotocol.io/). Ask an assistant to evaluate rules, explain results, inspect specs, and, when you allow it, draft or update specs in your workspace.

## Prerequisites

Install the Lemma CLI so `lemma` is on your `PATH`. See [Installation](../installation.md).

Point the server at a workspace directory (or a single `.lemma` file) with `--prefix`. That is the set of specs the assistant can see.

## Connect

Clients can use stdio (spawn `lemma mcp`) or Streamable HTTP (`lemma mcp --http`). Replace `/path/to/workspace` with your project directory. Add `--write` to the args only when the assistant should create, update, remove, or install repositories from LemmaBase (see [Read-only vs write](#read-only-vs-write)).

### Stdio

Shared shape:

```json
{
  "command": "lemma",
  "args": ["mcp", "--prefix", "/path/to/workspace"]
}
```

Older MCP clients use `initialize` / `notifications/initialized` (protocol `2025-11-25`; later `tools/list` / `tools/call` need no per-request `_meta`). Clients on `2026-07-28` send `_meta.io.modelcontextprotocol/protocolVersion` on every request and may call `server/discover` first.

### Streamable HTTP

Start a long-lived server (default `http://127.0.0.1:8013/mcp`):

```bash
lemma mcp --http --prefix /path/to/workspace
```

Point an HTTP MCP client at that URL. Example Cursor / Claude Desktop `url` entry:

```json
{
  "mcpServers": {
    "lemma": {
      "url": "http://127.0.0.1:8013/mcp"
    }
  }
}
```

Optional flags: `--host`, `--port` (default `8013`), `--cors`. No built-in auth or TLS; prefer localhost or a reverse proxy.

### Claude

**Claude Desktop**: Settings → Developer → Edit Config. Add a server under `mcpServers` in `claude_desktop_config.json`, then fully restart Claude Desktop:

```json
{
  "mcpServers": {
    "lemma": {
      "command": "lemma",
      "args": ["mcp", "--prefix", "/path/to/workspace"]
    }
  }
}
```

**Claude Code**: from a terminal:

```bash
claude mcp add --transport stdio lemma -- lemma mcp --prefix /path/to/workspace
```

### Gemini CLI

Add the same `mcpServers` entry to `~/.gemini/settings.json`, or to `.gemini/settings.json` in your project:

```json
{
  "mcpServers": {
    "lemma": {
      "command": "lemma",
      "args": ["mcp", "--prefix", "/path/to/workspace"]
    }
  }
}
```

Or:

```bash
gemini mcp add --transport stdio lemma lemma mcp --prefix /path/to/workspace
```

Restart Gemini CLI (or `/mcp reload`) and confirm the server is connected.

### Cursor

Cursor Settings → MCP, or a project `.cursor/mcp.json` / user MCP config. Add:

```json
{
  "mcpServers": {
    "lemma": {
      "command": "lemma",
      "args": ["mcp", "--prefix", "/path/to/workspace"]
    }
  }
}
```

Enable the server in Cursor’s MCP UI if it is listed but off.
## What you can do

Once connected, ask the assistant in natural language, for example:

- Evaluate a spec with given inputs and explain how a result was reached
- List specs in the workspace or show what data and rules a page defines
- Check whether draft Lemma source is valid before you keep it
- Install a repository from LemmaBase into the workspace (needs write access)

The server already ships authoring and evaluate guidance for the model. For learning the language yourself, see [Learn](../learn/readme.md) and [LLMs.txt](../llms.md).

## Read-only vs write

By default the server is read-only: evaluate and inspect, no changes to the engine or disk.

Pass `--write` when you want the assistant to load or replace specs, remove them, clear the workspace load, or `install` (download a LemmaBase repository into `lemma_deps/` and load). Only enable that for workspaces and agents you trust.

## How to work

1. Install the CLI and set `--prefix` to your specs directory.
2. Connect Claude, Gemini CLI, or Cursor via stdio or Streamable HTTP as above (restart the client after config changes).
3. Ask questions against your specs; ask for explanations when you care how a rule fired.
4. For drafting new specs, enable `--write` and ask the assistant to validate, then update the workspace when you are ready.

## See also

- [Lemma CLI](../reference/cli.md): `lemma mcp` flags
- [Learn](../learn/readme.md): language guide
- [LLMs.txt](../llms.md): authoring Lemma from business logic
- [Installation](../installation.md)

---
nav_title: C# / .NET
nav_order: 60
---

# C# / .NET

`Lemmabase.Lemma.Engine` embeds the Lemma engine in .NET 8+ via UniFFI. Prebuilt natives ship for macOS (arm64/x86_64), Linux glibc (x86_64/arm64), and Windows (arm64/x86_64). musl / Alpine is unsupported and fails closed at load.

## Install

```bash
dotnet add package Lemmabase.Lemma.Engine --version 0.9.11
```

## Usage

```csharp
using Lemmabase.Lemma.Engine;

using var engine = Engine.Create();
engine.Load("""
    spec order
    data quantity: number
    data unit_price: number
    rule total: quantity * unit_price
    """);
Response response = engine.Run(
    RunRequest.Of("order").WithData(new Dictionary<string, object?> {
        ["quantity"] = 3,
        ["unit_price"] = 19.99m,
    }));
decimal total = ((RuleResult.Number)response.Results["total"]).NumberValue;
```

Introspect loaded specs:

```csharp
var groups = engine.List();
var show = engine.Show(null, "order");
var workspace = engine.Source(null);
```

Format source (no engine instance needed):

```csharp
string formatted = Engine.Format("spec foo\ndata x: 1\nrule y: x + 1");
```

Install a repository from LemmaBase (download only; then `Load`). Default transport is `HttpClient`; pass a client for tests or custom HTTP:

```csharp
RepositoryInstallResult result = engine.Install("@iso/countries");
engine.Load(new Dictionary<string, string> { [result.Id] = result.Source });
```

## API

| Member | Description |
|--------|-------------|
| `Engine.Create` / `Create(limits)` | Create engine (optional sparse limits builder) |
| `Engine.Limits` | Current resource limits |
| `Engine.Snapshot` / `FromSnapshot` | Opaque bytes of parsed specs + plans + limits |
| `Engine.Load` | Load sources: string, label map, or `FileInfo` |
| `Engine.Install` | Download a repository from LemmaBase; does not load and does not write `lemma_deps/` |
| `Engine.List` | List loaded specs |
| `Engine.Source` | Formatted Lemma source |
| `Engine.Show` | Declared data catalog + rule graph (nested types stay as `JsonElement`) |
| `Engine.Run` | Evaluate via `RunRequest` (`WithData`, `WithRules`, `WithExplain`, …). Rejects `float`/`double`. |
| `Engine.Remove` / `Update` | Temporal remove / atomic upsert |
| `Engine.Quality` | Structural quality recommendations (advisory) |
| `Engine.Format` | Format Lemma source (static; not `Lemma.Format` to avoid CS0542) |

Persist and restore without re-parsing:

```csharp
byte[] bytes = engine.Snapshot();
File.WriteAllBytes("engine.lems", bytes);
using var restored = Engine.FromSnapshot(File.ReadAllBytes("engine.lems"));
```

## Failure modes

- Planning / request errors → `LemmaException` with `EngineError` list (serde JSON).
- Evaluation `veto` → stays inside `Response` / `RuleResult.Veto`.
- Bugs / panics across the boundary → `LemmaBugException`.
- Native load failures → `LemmaNativeException`.

## Thread safety

`Engine` is thread-safe: every method acquires an internal lock. Prefer one engine per worker for throughput.

## Native library loading

The package embeds `lemma_dotnet` natives (glibc Linux, macOS, Windows; no musl / Alpine). On first use, the SDK extracts the native for the current platform to a content-hashed cache and loads it. Override:

| Priority | Source | Example |
|----------|--------|---------|
| 1 | Environment variable `LEMMA_DOTNET_LIBRARY` | `LEMMA_DOTNET_LIBRARY=/opt/liblemma_dotnet.so` |
| 2 | Bundled package resource (extracted to cache) | Automatic |

Cache path: `~/.cache/lemma-dotnet/{version}-{triple}/{sha256}/`. Override the cache root with `LEMMA_DOTNET_CACHE_DIR`.

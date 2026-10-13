---
nav_title: LemmaBase
nav_order: 30
---

# LemmaBase

`@org/repo` is part of the Lemma language. The engine parses it, plans against it, and reports `Error::Registry` for it. That means the engine must know which registry a qualifier names. Registries live in the engine (`engine/src/registry.rs`). Hosts never invent URLs, status mappings, or error text.

Today every `@org/repo` names **LemmaBase**, bound to `https://lemmabase.com`. That binding is not configurable: no argument, no environment variable, no debug/release split. A future registry is a new identifier form in the language plus a new engine-side `Registry` implementation; never a retargeted LemmaBase.

The engine performs **no I/O**. It emits `Fetch` requests and consumes `HttpResponse` values. The socket is always the host's HTTP stack, so JVM trust stores and proxy properties, Erlang application configuration, browser settings, Node CA / dispatcher config, and CLI system proxies all apply.

You can run entirely offline if every `@` repository is already available as source (for example under `lemma_deps/`).

## Resolve first, then load

`Engine` never auto-fetches on `load` or `evaluate`. Every external `@` reference must be resolved into source text first, then loaded.

Sans-IO protocol (engine):

- `Registries::default()` — catalogue; today LemmaBase only
- `registry_for(qualifier)` — which registry owns an `@…` name
- `Install` — one repository download (`start` / `respond` / `run`)
- `Resolve` — transitive missing `@` references (`start` / `respond` / `run`)
- `HttpTransport` — sync driver for Rust hosts (`get(&Fetch)`)

Hosts loop: receive `Fetch` (URL + request headers), perform GET, hand back `HttpResponse` (status + response headers + body) or `TransportFailure`.

How you resolve depends on how you run Lemma:

- **CLI**: `lemma install --all` (or `lemma install @owner/repo`) downloads from LemmaBase and writes `lemma_deps/`. Other commands read workspace `.lemma` files plus that cache. Commit `lemma_deps`; there is no lock file.
- **Rust embedders**: implement `HttpTransport`, then `Install::run` / `Resolve::run` with `Registries::default()`, or drive the step machines yourself. Load returned source with `SourceType::Dependency`.
- **npm / Hex / Maven**: call `install` (download only; no load, no `lemma_deps/` write), then `load` with the returned id as the source label before loading workspace specs.

If you load a spec while some `@` references are still unresolved, planning reports those as missing.

## Resolving repositories from Rust

```rust
use lemma::{Engine, HttpTransport, Registries, Resolve, ResourceLimits, SourceType};
use std::collections::HashMap;
use std::sync::Arc;

let registries = Registries::default();
let transport = /* your HttpTransport */;
let mut context = lemma::Context::new();
let mut sources = HashMap::new();
// ... insert local workspace specs into `context`, mirror text in `sources` ...

Resolve::run(
    &registries,
    &mut context,
    &mut sources,
    &ResourceLimits::default(),
    &transport,
)?;

let mut engine = Engine::new();
let batch: Vec<(SourceType, String)> = sources
    .into_iter()
    .map(|(path, code)| (SourceType::Path(Arc::new(path)), code))
    .collect();
engine.load(batch)?;
```

Single repository:

```rust
use lemma::{Install, Registries};

let registries = Registries::default();
let result = Install::run(&registries, "@iso/countries", &transport)?;
// result.source, result.id
```

## Per-host transports

| Host | Socket | Corporate configuration |
|------|--------|-------------------------|
| CLI / Rust `ReqwestTransport` | reqwest (`rustls` + `system-proxy`) | `HTTPS_PROXY` / `HTTP_PROXY` / `NO_PROXY`, OS trust store |
| npm | host `fetch` (wasm binding) | browser settings; Node `NODE_EXTRA_CA_CERTS`, undici dispatchers |
| Maven | `java.net.http.HttpClient` (default or injected) | `javax.net.ssl.trustStore`, `http.proxyHost`, `java.net.useSystemProxies`, custom `SSLContext` |
| Hex | `Lemma.Transport.get/2` (Req) or injected fun | application Req / Finch / cert config |
| LSP | no fetch; hover uses `navigation_url` | n/a |

Hosts forward `Fetch.headers` on the request and return response headers. LemmaBase sends no headers today.

## Private host source

You can still load source you obtained yourself with `@owner/name` labels:

```rust
engine.load([(
    SourceType::Dependency("@myorg/rules".to_string()),
    my_source_text,
)])?;
```

There is no authentication in the public LemmaBase API yet; request headers exist so a future private registry can send `Authorization`.

## Bundle requirements

A bundle is ordinary Lemma source. A published LemmaBase bundle opens with a `repo` line whose name matches the repository id, for example `repo @iso/countries`. That `@` name is assigned when published. Installed repositories stay isolated: a LemmaBase repository never merges with your workspace or another installed repository, and every spec in a repository must come from the same place.

Within a bundle, spec names are normal identifiers (`spec billing`). Cross-repository imports use `@`, as in `uses rates: @acme/finance rates`. Unqualified `uses x: rates` only looks inside the same repository. In workspace files, declare local repos without `@` (`repo finance`).

The resolver keeps fetching unresolved references until everything is satisfied. One `.lemma` response per identifier is enough.

## Adding a registry later

1. New identifier form in the language / `RepositoryQualifier`.
2. New `impl Registry` in the engine (`fetch_for`, `bundle_from`, `navigation_url`).
3. New arm in `Registries::registry_for` (and optionally host-constructed config on `Registries`).

Hosts keep answering `Fetch` with GET. A non-HTTP protocol adds a new exhaustive `InstallStep` / `ResolveStep` variant; every host must handle the new tag (Rust fails to compile; Java/Elixir raise).

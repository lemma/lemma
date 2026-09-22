# Changelog

Releases cover the Lemma engine, `lemma` CLI, OpenAPI crate, LSP, SDKs and VS Code extension. They all follow the same version everywhere. The release version is `[workspace.package] version` in the root `Cargo.toml`. Git tags follow `lemma-v{version}` (for example `lemma-v0.8.20`); releases before the rename used `cli-v{version}`. Draft notes for the next version quickly by running `cargo changelog` to print `git diff` / `git log` since the latest release tag (`xtask` `versions-diff`). Tip: feed that into an LLM to create a summary for this changelog.

## [0.9.11] - 2026-09-22

`show` now names where imported data and rules live. The result string field is `result`, not `display`. Bare `uses` takes the target name as the alias.

### Added

- **Import paths on `show`**: each data slot and rule has `path`: a list of hops `{ uses, repository?, spec }`. Empty `path` means this spec. `Show.repository` is set when the spec lives in a named repository. `Show.rules` includes rules you can reach through `uses`, not only rules written in this spec. Keys look like `src.computed`. Same shape in the SDKs, schema, MCP, and HTTP JSON.
- **Large integers on npm**: pass integers bigger than `Number.MAX_SAFE_INTEGER` as a digit string or `bigint` (up to 28 digits). Small integers may still be JS numbers. Decimals stay strings. Unit maps (`{ eur: 84n }`) follow the same rule.
- **Typed explanation nodes**: rule and data nodes carry the same typed fields as a rule result (`result`, plus `measure` / `ratio` / `number` / …). Arithmetic compose nodes include `operator` (`add`, `subtract`, `multiply`, `divide`, `modulo`, `power`).

### Changed

- **[breaking] `display` is now `result`**: JSON, Rust `RuleResult::result()`, Hex, npm, and Java. Same one-line string as before.
- **[breaking] `run` rule names match `show`**: omit `rules` to evaluate this spec's own outputs. Pass `rules: ["src.computed"]` (or CLI `--rules`) to evaluate an imported rule. Response keys use those names.
- **[breaking] Bare `uses` alias is the target name**: `uses premium_membership` then `premium_membership.discount_rate`. Explicit: `uses membership: premium_membership` then `membership.discount_rate`.
- **Result units follow what you passed**: if you bind `84 eur`, the one-liner stays in `eur`, not another unit of the same type. A spec `as` clause still wins. On `+`/`-`, the left operand's unit wins when they disagree.
- **CLI `--explain`**: one reasoning table per rule. Missing inputs show in that table when the rule is waiting on data. The extra "Missing data" block at the top is gone.

## [0.9.10] - 2026-09-14

Show rule graph on `Engine::show`, MCP Streamable HTTP, and evaluation that walks only live `rule_ref` (the 0.9.9 authored schedule is gone).

### Added

- **Show rule graph**: `Show.rules` is `ShowRule` with `type`, `branches` (default then each `unless`), and `depends_on_rules` (stored local planning topo). Follow `rule` leaves across `Show.rules` for the static decision graph; not an inlined mega-tree. SDKs, schema, and MCP/HTTP JSON follow.
- **`lemma mcp --http`**: Streamable HTTP transport (MCP `2026-07-28`) on `POST /mcp` (default `127.0.0.1:8013`). Flags `--host`, `--port`, `--cors`. Stdio remains the default. JSON-only responses (no SSE). Separate from `lemma server` REST evaluate API.

### Changed

- **[breaking] `Show.rules`**: values are `ShowRule` objects, not bare `LemmaType`. Rule units live under `type` (e.g. `rules.total.type.units`).
- **Evaluation walks only what the requested rule needs**: a rule runs when a `rule_ref` is visited. The 0.9.9 authored dependency schedule is gone. Dead `unless` arms, unused OrderedDispatch regions, and `and` short-circuit no longer force other rules. Requested-rule `missing_data` is for that walk only.
- **OrderedDispatch for conjunctive LUTs**: `unless key is K and residual then body` chains (e.g. logistics `zone is Z and billable_lb >= L`) fold to a nested OrderedDispatch — outer point table on the shared `Is` key, inner tables on the residuals — so evaluation is binary search instead of a linear arm scan. Region painting is O(n α(n)); boundaries ship as `Arc<[DispatchKey]>`. Scrutinee typing uses the stamped NormalForm type, so determined expression scrutinees fold too.
- **`--explain` last-wins causes**: unless narration lists later not-taken arms then the winner (matching evaluation). Exclusive `scrutinee is K` OrderedDispatch tables keep only the winning cause; on the default they attach the scrutinee and omit the `is not` dump. ASCII omits a Data child that only restates a `name is display` cause line (JSON keeps it). False `and` causes state the short-circuit deciding conjunct (flipped like a lone condition), not the authored `and` tagged `is false`. Exhaustive walks still visit rewrite pre-images and arithmetic siblings after a definitive veto; values are unchanged. `and` still short-circuits.
- **One arithmetic typing scope per plan**: validation, NormalForm stamping, and evaluation share one measure scope (main spec unit index, imports merged) and the same promotion rules. MathOp (`ceil`/`floor`/`round`/`abs`) result types are planned, including named units for measure magnitude ops.
- **Engine snapshots**: plan layout changed (Show rule graph cache, Sum/Product fold types). `from_snapshot` rejects bytes from other engine versions.

### Removed

- **`ExecutableRule.depends_on_rules`**: evaluation no longer ships an authored rule-dependency list on the plan (Show still exposes the planning list on `ShowRule`).
- **`UnitResolutionContext`**: dead unit-resolution switch removed from the computation API.

### Fixed

- **`and` right operand must be boolean**: validation already required a boolean left; a non-boolean right slipped through and is now rejected, matching documented Logical AND.
- **Rule reference as `and` left conjunct with unbound right**: no longer panics with `BUG: MissingData path … not in missing_data []`; `missing_data` matches the evaluator.
- **Hex `hex.audit` in precommit**: vulnerable Hex deps fail the gate. `mint` 1.9.3 → 1.10.0, `ex_doc` 0.40.3 → 0.40.4.
- **Nested `-> with` reference paths**: wrapping a spec that itself `uses` and binds data no longer panics at planning. Binding RHS targets keep ancestor spec names, matching the walk `add_data` already stored.
- **`RationalInteger` compare OOM**: removed `Ord`/`PartialOrd`. Planning sorts and bound checks use `try_cmp` → `Error`; explain conversion-trace reorder after a successful span is `unreachable!` on compare failure. Named-range default refresh and measure recanonicalization push validation `Error` instead of panicking.

## [0.9.9] - 2026-09-05

Sans-IO registries and SDK `install`, engine binary snapshot across Rust/npm/Maven/Hex, rule-embed evaluation boundaries, Java SDK sealed results and native cache, scoped replanning, unqualified imported types, and inline rational arithmetic.

### Added

- **Sans-IO registries**: `Registry` trait, hard-bound `LemmaBase` (`https://lemmabase.com`), `Registries` catalogue, `Header` / `Fetch` / `HttpResponse` / `TransportFailure`, exhaustive `InstallStep` / `ResolveStep`, `Install` and `Resolve` step machines, sync `HttpTransport` plus `Install::run` / `Resolve::run`, `RepositoryInstallResult` / `RegistryBundle` / `RegistryError` in the engine.
- **SDK `install`**: Hex `Lemma.install/2-3` (optional transport fun; default Req), Maven `Engine.install(String)` and `install(String, HttpClient)`, npm `Engine.install` (wasm over host `fetch`). Download only (`{ source, id }`); no load, no `lemma_deps/` write. Wire shape in `api.v1.json`.
- **CLI `ReqwestTransport`**: reqwest with `rustls` + `system-proxy`; MCP `McpServer` parameterized by `HttpTransport`.
- **Offline fixtures**: `__test_support::FixtureTransport` over `engine/tests/registry_fixtures` (native).
- **Unqualified imported type**: `data x: TypeName` resolves to an imported type when exactly one `uses` import exports that name. Ambiguous names error with `"Qualify as alias.TypeName"`.
- **Engine binary snapshot**: `Engine::snapshot()` / `Engine::from_snapshot(&[u8])` encode Context AST + execution plans + limits as compact postcard bytes (whole-slice CRC32C + engine-version header). Same sources → identical bytes. Stale version / corrupt bytes return `Error`. JSON API documents (`Show`, `Response`, `LemmaType`, …) serialize through `lemma::api` only; domain types use exact postcard-safe serde. SDKs expose the same opaque bytes: npm `snapshot` / `fromSnapshot`, Java `snapshot()` / `fromSnapshot(byte[])`, Hex `Lemma.snapshot/1` / `Lemma.from_snapshot/1`.
- **`cargo benchmarks engine` snapshot table**: logistics ladder (1050 / 6300 / 18900 / 126000 rate cells, byte-identical to the Java `SpecGenerator.logistics` fixtures) reporting load, loaded heap, snapshot bytes, encode / restore medians, allocations per restore and restored heap; written to `engine.md` as "Snapshot (logistics ladder)".
- **Java `ResourceLimits.builder()`**: named limit overrides for `Engine.create(Builder)`. Unset keys keep engine defaults (numbers live in Rust only). `Engine.create(ResourceLimits)` still accepts a full limits copy from `limits()`.
- **Java sealed `RuleResult` / `RuleResultValue`**: pattern-match variants (veto, missing data, number, text, boolean, date, time, measure, ratio, calendar, range). Dates/times are `java.time`; literal booleans are `boolean`.
- **Java `LemmaNativeException`**: deploy-time native load failures (missing `.so`, unsupported arch, cache). Use-after-close stays `LemmaBugError`.
- **Java native cache**: `~/.cache/lemma-jni/{version}-{triple}/{sha256}/` so same-version rebuilds pick up new natives.
- **Java `Engine.load(Path)` / `loadResource(Class, String)`**: file and classpath load helpers.
- **Java schema package**: `LemmaType`, `TypeExtends`, `LiteralValue`, `ExplanationNode` live under `com.lemmabase.lemma.schema`.

### Fixed

- **Linear rule-chain stack overflow**: load and tip-only run of long `r_i: r_{i-1} + 1` chains no longer abort the native process. JNI, npm, and Hex hosts get results or a structured `Error`, never SIGSEGV.
- **Maven natives**: `xtask maven-natives` and precommit build `lemma_jni` with `--release` and package only `target/release`; a stale `target/debug` library is no longer picked up, so the Java package and `benchmarks/java` measure release code.

### Changed

- **Snapshot codec**: body is checksummed once as a slice with hardware CRC32C (`crc32c`) instead of one digest update per varint byte inside the postcard flavor; `RationalInteger` serializes its native representation (two `i128` varints for the inline case, bignums only when the pair does not fit) and rejects unreduced, non-positive-denominator or `Big`-that-fits-`Small` input; `DispatchKey::Text` deserializes straight into `Arc<str>` without an intermediate `String`. On the 126000-cell logistics fixture (10.1 MiB source): restore 766 ms → 559 ms, encode 439 ms → 291 ms, allocations per restore 11.43 M → 8.95 M.
- **Rule embeds are evaluation boundaries**: a run evaluates the requested rules' dependency closure in plan topological order; embed cells read the stored rule value. Tip-only `missing_data` matches full-run `missing_data` and is ordered by the evaluation tree. `max_normal_form_depth` and `max_normalized_expression_nodes` count rule embeds as one cell (intra-rule nesting / IR size only). Show `needed_by_rules` is stored as rule positions per data slot on the plan, built eagerly in `build_execution_plan` with bitset unions over embeds (no per-entry `DataPath`/`String` clones or per-list sorts). Data-reference chains resolve once in planning (`ReferenceEnd` on the graph/plan); evaluation no longer walks them.
- **MCP `run`/`evaluate`**: success text is `format_explanation` ASCII trees plus a `Missing data` block when inputs are unbound (not Engine Response JSON). Args unchanged; `list`/`show`/`check` still JSON.
- **`Engine::update`**: `update(repository, code, source_type)` — identities come from parsing `code` (upsert). For `Path` / `Dependency`, live specs with that `source_type` absent from `code` are removed in the same apply — including when `code` has zero specs (prune every live row of that source). `Volatile` never prunes siblings and still requires at least one spec. Dropped `spec` and `effective` parameters (they disagreed with `code`). Bindings: npm/Java/Hex/MCP same arity change.
- **`Engine::remove`**: public signature unchanged; resolves active-at then removes by exact `effective_from`.
- **First-pass planning**: no-op normalize rewrites no longer clone and re-hash wide Piecewise nodes; type inference caches within a graph build (cleared between rule-reference resolve and type check); OrderedDispatch text keys use `Arc<str>`; one normal-form interner is shared across temporal slices of a spec set in a planning pass. Large unless-chain fixtures plan substantially faster on a cold load.
- **Parse**: string literal tokens carry unquoted content (no quote round-trip); keyword and constraint-command matching folds ASCII without allocating. LSP semantic tokens reconstruct quotes for `StringLit` highlight lengths.
- **Scoped replanning**: `load` / `update` / `remove` replan only the spec sets the batch touched plus their transitive consumers; every other spec set keeps the plans it already had (moved, not rebuilt). Editing one file in a 257 spec set workspace: 42.2 ms → 0.22 ms. `apply` stays all-or-nothing, so a batch that fails leaves the previous plans in place.
- **HTTP `--watch` refresh**: builds a scratch engine outside the write lock and swaps only on success; failure keeps the previous engine (never emptied, never partially refreshed).
- **LSP workspace**: initial / multi-file validation batch-loads (order-independent, sorted attributes); single-file edits use per-file `update` and keep the attribute dirty on failure. Disk delete/rename queues an empty Path/Dependency prune applied on validate (planning failures surface as diagnostics, not panics).
- **CLI / MCP wording**: install repositories from LemmaBase (arg `repository`), not “dependencies from the registry”.
- **LemmaBase / registries**: engine owns registry policy and performs no I/O; hosts supply the socket only. LemmaBase is hard-bound to `https://lemmabase.com` (not overridable). Request and response headers on `Fetch` / `HttpResponse`. Install ids use the repository-qualifier grammar (rejects URL/path injection).
- **Maven install**: step protocol over JNI (`installStart` / `installStep` / `installRespond` / `installFail` / `installFree`); Java HTTP loop uses injected or default `HttpClient`; does not hold the engine lock across the download; handle always freed in `finally`.
- **Hex install**: default Req transport uses `decode_body: false`; `lemma_install_respond` on DirtyCpu; success payload is engine JSON (`Jason.decode!`) with string keys.
- **Scoped replanning `PlanView`**: slice-mode members expose committed slices outside the dirty ranges plus replanned dirty slices (fixes spurious/missed interface-drift checks and mid-edit panics).
- **Result/show rule unit maps**: `results.<rule>.measure` / `.ratio` and `show.rules.<rule>.units` list every unit in the measure/ratio family (including units declared on extending types), from a plan-time family unit catalog. Show data input types stay declared-only.
- **Show data catalog**: `Show.data` lists every declared promptable slot (not only rule-used). Empty `needed_by_rules` means offered for reuse (`data x: alias.slot`). OpenAPI request bodies still include only slots with non-empty `needed_by_rules`.
- **`Show.meta`**: values are literal JSON (`{"text": "..."}`, `{"number": "1"}`, etc.), not the former `MetaValue` wrapper (`literal` / `unquoted` tags). Any literal form is valid; `meta title` is no longer restricted to text.
- **Rational arithmetic**: inline `i128` fast path for small numerator/denominator pairs (zero allocation); promotes to heap `BigInt` only on overflow. Reduces allocation pressure in arithmetic-heavy specs.

### Removed

- Engine HTTP stack and old registry helpers: `reqwest`, `async-trait`, `gloo-net`, `registry` feature, `LEMMABASE_URL`, `lemmabase_source_url` / `lemmabase_navigation_url` / `lemmabase_install_failure`, callback `resolve_registry_references`, `RegistryFetchFailure`, `LEMMA_BASE_URL`, debug-build `localhost:4222`, `LemmaBase::test`.
- **npm** JS `LemmaBase` helper / env / `Engine.prototype.install` patch (wasm `Engine.install` is the surface).
- **npm `Engine.fetch`**: renamed to `Engine.install` (same download-only contract).
- **`MetaValue` wire tag** and unquoted meta identifiers in source.

## [0.9.8] - 2026-08-26

0.9.8 aligns MCP with SDK `run`, requires one declaring type per unit name, and unifies HTTP explain with CLI `--explain`.

### Added

- **Hex `Lemma.Mcp.run/2`**: same catalog as CLI MCP `run`. `evaluate/2` remains as a deprecated alias.
- **MCP `repository`**: optional on `run`, `show`, and `source` (with `spec` for a repo spec, or `repository` alone for a whole repo).

### Changed

- **Unit uniqueness**: a unit name may be declared on only one type in scope. A second independent `-> unit` with the same name is a planning error. Extensions inherit; bare binds the declarer; use `Type.unit` or `alias.Type.unit` for the extension.
- **Import-alias unit sugar**: `alias.unit` (e.g. `units.kilogram`) is rejected. Write `alias.Type.unit` (`units.mass.kilogram`).
- **Custom ratio units**: same uniqueness as measures. Builtin `percent` / `permille` stay shared across named ratio types when factors match.
- **MCP `run`**: renamed from `evaluate` (deprecated alias kept). Args match SDK `run`: `rules` (string or array; `rule` rejected), JSON `data` object (not `name=value` strings). Returns Engine `Response` JSON; explanations always on (no `explain` arg).
- **MCP `--write`**: replaces `--admin` for write tools (`add_spec`, `update_spec`, `remove_spec`, `clear`, `install`). `attribute` replaces `source_id`.
- **MCP `check`**: success is `quality()` JSON array. Engine failures on `run` / `show` / `source` return `EngineError` JSON with `isError`.
- **HTTP `--explain`**: `lemma server --explain` and client header `x-explain` replace `--explanations` / `x-explanations`. Hex `Lemma.OpenAPI.generate_openapi` option is `:explain`.
- **HTTP GET `/{spec}`**: body is `Show` JSON (no `spec_set_id` wrapper).
- **Runtime data vetoes**: reasons are `Data field [type]: …`. Empty non-text input vetoes before parse. Interactive prompts use the same field `[type]` labels.
- **LLM authoring method**: Method fragment rewritten (inventory then distill, hard stop before write, resources of truth).

### Removed

- **`lemma mcp --admin`**: use `--write`.
- **`alias.unit` qualification**: use `Type.unit` or `alias.Type.unit`.
- **`lemma server --explanations`**: use `--explain`. Header `x-explanations` → `x-explain`.

## [0.9.7] - 2026-08-23

0.9.7 aligns Learn docs with the LLM authoring guide, fixes release packaging for crates.io and Open VSX, and corrects SDK and reference documentation.

### Changed

- **Learn veto pedagogy**: bounds on `data` (`-> minimum` / `-> maximum`), boolean denial, lookup default `veto` + `unless` arms; removed veto-for-bounds and "place Veto last" guidance.
- **SDK docs**: JavaScript `run({ spec, data, … })` options object; `quality()` on npm, Java, Elixir, and package READMEs; Hex `limits` / `update` on Hex README.
- **READMEs**: root and engine README add Elixir; npm section titled JavaScript/TypeScript (not WebAssembly); Rust shipping examples use `uses lemma units`.
- **Reference**: `year` / `month` calendar literals (`units.calendar`, not `units.duration`); workspace deps in `lemma_deps/`; API schema links point at `engine/schemas/api.v1.json`.
- **LLM guide**: `10_syntax.md` opening order is formatter convention; parser still allows any order after commentary.
- **Site home**: planning-time errors vs veto; typo fix.

### Removed

- **`cli/build.rs`**: generated `llms.txt` at compile time from a sibling engine path not present in the crates.io tarball.

### Fixed

- **Crates.io `lemma` publish**: removed `cli/build.rs`; `llms.txt` is generated via `cargo run -p xtask -- llms` and checked in.
- **Open VSX publish**: `npm ci` no longer omits optional deps (which dropped `@node-rs/crc32` required by `ovsx`); precommit smoke-loads `@node-rs/crc32`, `ovsx`, and `@vscode/vsce`.

## [0.9.6] - 2026-08-23

0.9.6 moves the MCP tool catalog into the engine, refines import and unit syntax, and tightens Java and npm release gates.

### Changed

- **MCP tool catalog**: `lemma::mcp` (`evaluate`, `list`, `show`, `source`, `check`, `guide`, resources). CLI MCP and Hex `Lemma.Mcp` use the engine module; JSON-RPC stays in the CLI.
- **Formatter**: `lemma format` puts the rule body on the line after `rule name:` (CLI, LSP, MCP `source`, SDKs).
- **Import bindings under `uses`**: canonical form is `uses alias: spec` with nested `-> with path: value` (paths relative to the imported spec). `lemma format` emits this block form.
- **Deprecated standalone `with`**: `with alias.field: …` still parses when a matching `uses` exists; bindings merge into the import. `Engine::quality` recommends nesting as `  -> with field: …`. `lemma format` does not emit standalone `with`.
- **`-> unit` syntax**: canonical `-> unit <name>: <value>`; legacy `-> unit <name> <value>` still parses. `Engine::quality` recommends the colon form; `lemma format` emits colon syntax.
- **Authoring guide**: tightened LLM guide fragments (one owner per fact, less repetition, veto default-vs-last clarified); Markdown fragments under `engine/documentation/`.
- **Java Maven SDK**: shaded jackson-core no longer contributes `META-INF/MANIFEST.MF` or JPMS `module-info`; published manifest is Lemma's. JNI native cache key reads package `engine.version` resource instead of classpath `META-INF/MANIFEST.MF`. Maven build treats warnings as errors: `javac -Werror`, compiler `failOnWarning`, javadoc `failOnWarnings` with `doclint=all`, and xtask `[WARNING]` gate on `./mvnw verify`.
- **Precommit warnings**: xtask treats `npm warn` on `npm ci`, `npm run compile`, and `npm install --package-lock-only` as errors. VS Code extension build uses Node.js 24 (same as CI and Nix dev shell).

### Removed

- **`lemma-mcp` crate**: deleted; was `publish = false` and blocked `cargo publish -p lemma`.
- **`DataValue::With`**: removed from the AST/serde surface; bindings live only on `DataValue::Import`.

## [0.9.5] - 2026-08-21

0.9.5 sharpens per-rule missing-data reporting, extracts a reusable MCP tool catalog (CLI + Hex), embeds authoring docs in the engine, and accepts the older MCP `initialize` handshake alongside modern per-request `_meta`.

### Added

- **`lemma-mcp` crate**: MCP tool catalog and handlers (`evaluate`, `list`, `show`, `source`, `check`, `guide`, resources) over an `Engine`, without JSON-RPC or session. The CLI MCP server uses this crate.
- **Hex `Lemma.Mcp`**: Elixir bindings for the same catalog/tools/resources path.
- **Engine-embedded docs**: authoring guide fragments, evaluate guide, examples, and `llms.txt` under `engine/src/documentation/` so MCP and SDKs can serve them without the CLI tree.
- **`RuleResult::awaits_missing_data()`**: true only when the rule result is a `MissingData` veto.
- **MCP `initialize`**: older clients that handshake with `initialize` / `notifications/initialized` (`2025-11-25`) can call tools without per-request `_meta`. Clients on `2026-07-28` still send `_meta.io.modelcontextprotocol/protocolVersion` on each request and may call `server/discover` first.

### Changed

- **[breaking] `missing_data` on rule results**: populated only when that rule's result is a `MissingData` veto. Settled answers no longer carry leftover unbound keys. Interactive `lemma run` follows the same rule.
- **MissingData vs definitive veto**: later siblings still evaluate after `MissingData` or a veto so nested control can record. For `and`, an unbound left stays `MissingData` even if a later conjunct definitively vetoes (`false` can still answer). Product and other both-operand operators settle on a later definitive veto and drop keys that cannot un-veto.
- **Veto-typed operands at plan time**: date/calendar sugar, past/future ranges, range literals/`in`, piecewise and unless accept a `veto`-typed operand (parity with AND/math). Runtime still propagates the veto.
- **[breaking] Duplicate type constraints**: `-> suggest`, `minimum`, `maximum`, and `decimals` may appear at most once per declaration (planning error). A child typedef may override an inherited bound or suggest.
- **Versions bump/verify**: path-dep pin tracking includes `mcp/Cargo.toml`.

### Fixed

- **Number scale**: literals and runtime number input with more than 28 fractional digits are rejected (no silent truncate).
- **`data x: <rule>`**: planning error names the rule and points at a type name or a reference expression, instead of a generic unknown-parent message.

## [0.9.4] - 2026-08-07

0.9.4 renames registry download to `install`, aligns SDK limit/update surfaces, reshapes structural quality recommendations, and exposes `Engine.quality()` on npm, Hex, and Maven.

### Added

- **`lemma install`**: download a registry package and persist it under `lemma_deps/` (replaces `lemma fetch`).
- **MCP `install`**: admin tool shares the same download/conflict helpers as the CLI.
- **SDK `update`**: Java and Elixir expose transactional `Engine::update`. Hex also exposes `limits/1`.
- **Named limit overrides**: `ResourceLimits::apply` is the single path; Hex/JNI/npm use it (Hex gains `max_normal_form_depth`). npm `Engine.withLimits(...)` takes named overrides without deserializing full `ResourceLimits`.
- **`Engine.quality()`**: npm, Hex, and Maven return structural quality recommendations (message, `effective_from`, repository, `source`).
- **Java / Kotlin docs** at `/tools/java` (Maven path is a hidden stub that links there).

### Changed

- **[breaking] CLI / MCP**: `fetch` renamed to `install`.
- **[breaking] `Recommendation`**: tagged `kind` removed; wire shape is advisory `message` plus `spec`, `effective_from`, `repository`, and `source`. Checks focus on missing `-> help`, open text without `-> option`, quantity bounds without `-> minimum`/`-> maximum`, and veto-as-rejection cascades (no longer flags missing commentary, effective date, or `-> suggest`).
- **LSP**: quality recommendations are no longer published as Hint diagnostics.
- **`LemmaBase`**: registry identifiers must start with `@`; corrupt `lemma_deps` files hard-error.
- **Release / precommit**: VS Code packaging via xtask (esbuild bundle, pinned vsce); wasm-pack 0.15.0; wasm32 clippy `-D warnings`.

### Fixed

- VS Code extension dependency / packaging build.

## [0.9.3] - 2026-08-05

0.9.3 makes engine mutations transactional, adds structural quality Recommendations, and expands the MCP authoring/evaluate loop (`update_spec`, evaluate guide, richer admin tools).

### Added

- **Transactional `Engine::update`**: replace a temporal spec slice with new source in one atomic apply (rollback on failure). Exposed on WASM/TS as `update(...)`.
- **MCP admin mutators**: `update_spec`, `remove_spec`, `clear`, and `fetch` (with `--admin`).
- **Structural quality**: `Engine::quality()` returns advisory `Recommendation` values (missing commentary/effective date/`-> help`, open text without options, open inputs without suggest, veto-as-rejection cascades). MCP `check` appends them on success; LSP publishes them as Hint diagnostics.
- **Evaluate guide**: MCP `guide` with no topic (and `lemma://guide`) returns the CS evaluate guide. Authoring uses `topic: "full"` / `lemma://guide/full`. New section topics: `method`, `natural_language`.

### Changed

- **`load` / `remove` / `update`**: share one transactional apply path; failed batches leave the engine unchanged.
- **MCP `source`**: available in read-only mode (no longer requires `--admin`).
- **Authoring guide**: refreshed fragments (rules method, anti-patterns, veto, data, composition) and `llms.txt`.

## [0.9.2] - 2026-08-03

0.9.2 strengthens the MCP authoring loop, relocates published docs next to the CLI, and exposes a single serializable `EngineError` wire type from the Rust engine.

### Added

- **MCP authoring loop**: read-only `check` (batch of labeled sources, non-mutating parse/plan with structured diagnostics) and `guide` (topic slices of the embedded authoring guide); `resources/list` and `resources/read` for `lemma://guide`, `lemma://guide/{topic}`, and `lemma://examples/...`.
- **Embedded authoring guide**: fragment files under `cli/documentation/guide/` (built into the CLI) plus refreshed `llms.txt` / `llms.md`.
- **`EngineError` / `EngineErrorSource`**: public Rust types matching `api.v1.json` / TS `EngineError`; WASM and MCP load failures serialize this shape instead of parallel hand-built projections.

### Changed

- **Documentation layout**: docs live under `cli/documentation/`; consumer wire schema at [`engine/schemas/api.v1.json`](engine/schemas/api.v1.json); agent rules at root [`AGENTS.md`](AGENTS.md).
- **MCP tool surfaces**: `show` returns JSON `Show` (rule units visible). `add_spec` returns a success confirmation. `evaluate` renders full measure/ratio unit maps. Load failures return `isError` tool results with `EngineError` diagnostics.
- **Quality CI**: pull requests run `cargo precommit`; push events still run `cargo precommit --fuzz`.

## [0.9.1] - 2026-08-01

0.9.1 consolidates the consumer API schema into a single `api.v1.json` document, completes the Java Maven SDK (typed Show/ExplanationNode/LemmaType, BigDecimal magnitudes, JDK 21, thread-safe Engine), aligns the TypeScript and Elixir SDKs to the canonical wire format, hardens release quality gates, adds qualified units, and folds long `unless` chains into ordered lookups.

### Added

- **Qualified units**: optionally qualify unit names (`Type.unit`, `alias.unit`, `alias.Type.unit`); must qualify when the bare name is ambiguous in scope. Cross-type duplicate unit names no longer block loading the spec: bare use sites report a planning error with legal qualifiers.

### Changed

- **API schema**: sole consumer wire schema is [`api.v1.json`](documentation/schemas/api.v1.json) (Show, Response, list, errors, explanation trees). Removed standalone `explanation.v1.json`.
- **[breaking] NPM TypeScript SDK**: `Engine.run()` takes an options object (`RunOptions`) instead of positional arguments. Response type renamed `EvaluationResponse` → `Response`; adds `spec_effective_from`/`spec_effective_to`. Rule results extend `RuleResultValue` (flattened value fields). `TypeExtends` is now a discriminated union with `kind`. Temporal fields are ISO strings (removed `DateTimeValueJson`). Renamed: `DataEntry` → `ShowData`, `ExplanationCause` → `Cause`, `ListedSpecJson` → `ListedSpec`, `ResolvedRepositoryJson` → `ResolvedRepository`, `ResourceLimitsJson` → `ResourceLimits`, `UnitDef` → `MeasureUnit`, `RatioUnitDef` → `RatioUnit`. `EngineError` expands with `registry_kind`, `request_kind`, and resource-limit detail fields.
- **Hex Elixir SDK**: Error maps now include canonical fields (`source`, `related_data`, `registry_kind`, `request_kind`, limit fields). Added typed struct modules: `Lemma.EngineError`, `Lemma.Response`, `Lemma.RuleResult`, `Lemma.Show`, `Lemma.ShowData`, `Lemma.ShowVersion`.
- **Java Maven SDK**: typed `Show` / `ExplanationNode` / `LemmaType` surface on JDK 21; rule magnitudes and schema `DecimalString` fields map to `BigDecimal`; `RangeResult` endpoints are `RuleResultValueEndpoint` (no nested range); `RuleResult.explanation` is `ExplanationNode.Rule`; `Cause.value` is required `String` end-to-end; Engine is internally thread-safe (`ReentrantLock`); Maven Central / quality CI use JDK 21. Jackson-core is shaded/relocated; no transitive dependencies except `jspecify` (provided scope).
- **Native library loading**: Java SDK honors `lemma.native.library` system property and `LEMMA_JNI_LIBRARY` environment variable. Bundled natives are extracted to a version-keyed cache (`~/.lemma/native/{version}/{platform}/`) with atomic rename.
- **ExplanationNode**: removed dead `UnitEquivalence` variant from Rust, schema, TypeScript, and Java.
- **Performance**: engine evaluation ~10% faster, memory per evaluate call reduced ~15%, compile/plan up to 20% faster on complex specs.
- **Performance: long `unless` chains**: planning folds a chain of `unless` clauses that all test the same value into an ordered lookup, so evaluation binary-searches instead of testing every clause in turn. Applies when each clause compares one shared scrutinee against a literal text, number, measure, ratio, date, datetime or time, using `is`, `is not`, or an ordering operator; mixed scrutinees, mixed value types, and non-literal comparands keep the existing behaviour. On the engine evaluate fixtures, evaluation is about 3–7% faster. Explain mode still narrates from the original `unless` chain (so explanation text, vetoes and missing-data reporting stay the same) and therefore evaluates the lookup for the value and then replays the pre-image for narration: a small constant cost on the explain path.
- **Release CI**: each release quality gate runs `cargo precommit --fuzz`: 30 minutes of fuzz testing total, split across `engine/fuzz` targets.

### Fixed

- **Unary minus on numeric literals in expressions**: `rule x: -2` and `unless … then -2` format as `-2` again (parser keeps a signed literal). Explicit `0 - 2` is unchanged; non-literal unary minus (e.g. `-(a + b)`) still desugars to subtract-from-zero.
- **Source formatter dropped same-precedence parentheses**: left-associative arithmetic no longer loses required parens on the right operand (`a - (b + c)`, `a - (b - c)`, `0 - (a + b)`). Uses the same parenthesis policy as expression display.
- **`show` hid inputs that `run` still asked for**: if you filled a nested field via `with` from a local input (e.g. `with prev.code: code`), evaluation correctly said `code` was missing, but `show` listed no inputs and error text named the nested path instead of `code`. Interactive prompts and OpenAPI bodies built from `show` therefore could not ask for the right field. Fixed so `show`, missing-data lists, and veto messages all name the same caller-facing input.
- JS plain-object `load` no longer loses key order via `HashMap` (uses `IndexMap`); Hex map `load` sorts labels lexicographically so order is not VM-dependent. List/array forms already preserved caller order.

## [0.9.0] - 2026-07-25

0.9.0 cleans the public consumer API and replaces the compiled instruction VM with a shared **normalized graph** (NormalForm DAG). The evaluator walks that graph to evaluate rules, build explanations, and compute per-rule unbound inputs: one representation, several walks. Migration highlights: **`schema` → `show`**, **`get` → `list`**, repo text via **`Engine::source`**, unified **`load`**, **`-> suggest`** (was `-> default`), and **`accept`/`reject`** demoted from boolean literals. Overlay-aware unbound inputs live on each **`RuleResult.missing_data`** (`string[]` input keys); types, prefilled literals, and suggestions live on **`Engine::show`** (`Show.data`) only.

### Removed

- **`lemma units` plural and British spellings**: stdlib unit names are singular only (`8 hour`, not `8 hours`) and length uses American `meter` (not `metre`). Migration: rename plurals → singular; `metre*` → `meter*`.
- **`Engine::schema`**, **`Engine::inspect`**, **`Engine::inspect_repo`**: use **`Engine::show`** (interface + temporal window) and **`Engine::source`** (Lemma text).
- **JS `Engine.schema`**, **`Engine.inspect`**, **`Engine.inspectRepo`**: use **`Engine.show`** and **`Engine.source`**.
- **Hex `Lemma.schema/4`**, **`Lemma.inspect_repo/1`**, **`Lemma.inspect_repo/2`**: use **`Lemma.show/4`** and **`Lemma.source/4`**.
- **MCP `get_schema`**: use **`show`** (dropped dead `rule` param).
- **CLI `lemma schema`**: use **`lemma show`**.
- **`-> default` / `DataEntry.default`**: use **`-> suggest`** and **`DataEntry.suggestion`** (UI hint only; never commits). Same rename on Show data entries and measure/ratio unit magnitude fields (`suggestion` instead of `default`).
- **`accept` / `reject` boolean literals**: use `true`/`yes` or `false`/`no`; `accept` and `reject` are ordinary identifiers.
- **`Engine::get`**, **`Engine::get_repository`**: use **`Engine::list()`**.
- **`SourceType::Registry`**: use **`SourceType::Dependency(String)`** for dependency provenance.
- **`Engine::format_repository`**, **`Lemma.format_repository/2`**, JS **`format_repository`**: use **`Engine::source(...)`**.
- **`Engine::get`**, **`Lemma.get/2`**, **`lemma_get`**: use **`Engine::list()`** / **`Lemma.list/1`**.
- **`RunRequest`**, **`SchemaRequest`**: use flat positional args on **`Engine::run`**, **`Engine::show`**, **`Engine::remove`**.
- **`Lemma.execution_plan/3`**: serialized execution plans had no run path in any binding.
- **`collect_lemma_sources`**, **`Engine::load_from_paths`**, JS **`load(code, attribute)`**: host reads files and calls unified **`load`** (see Added).
- **`Lemma.load_from_paths/2`**, **`lemma_load_from_paths`**: use **`Lemma.load/2`**.
- **JS `Engine.loadBatch`**, **Hex `Lemma.load_batch/2`**, **`lemma_load_batch`**: use unified **`load`** (string/binary → volatile; object/map or `[label, code][]` → labeled). `null`/`undefined` rejected on JS (no silent empty load).
- **`DataEntry::supplied`**, **`SpecSchema`**, **`scope_to_rules`**, **`ExecutionPlan::schema_for_rules`**: static interface is **`show`**; overlay-aware unbound keys are per-rule **`missing_data`** on **`run`**.
- **`Response.data`**, **`DataGroup`**, **`Response::required_data_ordered()`**: overlay-aware input discovery is per-rule only via **`RuleResult.missing_data`**. Types, prefilled literals, and suggestions live on **`Engine::show`** only.
- **Compiled instruction VM**: register-based instruction streams, dual optimized/source streams, and recorded-execution explanation from VM traces are gone. Evaluation walks drift-free NormalForm cells by id.
- Explanation wire keys **`rule`** (identity), **`data_input`**, **`unbound_data_input`**, and identity field **`data`**: use **`name`**, **`data`**, **`data_unused`**.

### Added

- **Java Maven package** `com.lemmabase:lemma-engine`: JNI bridge (`lemma_jni`), `BigDecimal`-first API, `RunRequest`, AutoCloseable `Engine`, prebuilt natives in the JAR, Maven Central publish from release workflow. Docs: `cli/documentation/tools/java.md`.
- **Range endpoint and width constraints**: `* range` data accept `-> lower` / `-> upper` (endpoint envelope) and `-> minimum` / `-> maximum` (span width). Measure/ratio bounds use the same mixed declaring-unit model as scalar measure. Date range width is duration or calendar (not both on one type); time range width is duration only. Named element min/max inherit as range lower/upper.
- **`lemma units` catalog expansion**: SI scales (`nanosecond`, `nanometer`, …), derived compounds (`newton`, `pascal`, `joule`, `watt`, `hertz`, electrical), `area`/`volume`, imperial (`inch`, `pound`, `gallon`, …), and `information` (`bit`/`byte`/…); still no affine Celsius/Fahrenheit.
- **`Engine::show(repository?, spec, effective?)`**: returns **`Show`**: interface + temporal window (no Lemma text).
- **`Engine::source(repository?, spec?, effective?)`**: returns formatted Lemma **`String`** (repo-wide when `spec` omitted).
- **`Engine::list()`**: all loaded repositories with listed spec rows (replaces **`get`** / **`get_repository`**).
- **JS `Engine.show`**, **`Engine.source`**, **`Engine.list`**, **`Engine.remove`**, **`Engine.limits`**.
- **Hex `Lemma.show/4`**, **`Lemma.source/4`**, **`Lemma.list/1`**, **`Lemma.run/3`**, **`Lemma.remove/4`**.
- **`RuleResult.missing_data`**: overlay-aware unbound input keys for that rule (`string[]`; omitted when empty). Same keys as **`Show.data`**.
- **`SourceType::from_binding_label`**: decode JS/Hex batch labels (`path`, `@org/pkg`).
- **`Engine::load(sources)`**: single load verb; `(SourceType, text)` pairs; duplicate key rejection.
- JS **`Engine.load`**: string → volatile; object or `[label, code][]` → labeled batch. NIF / Hex **`Lemma.load/2`** same shapes (binary → volatile; map or `[{label, code}, ...]` → labeled).
- **`explain`** required on **`Engine::run`**; convention **`false`** (WASM/Hex default **`false`**; was **`true`** in bindings).
- **`lemma::resolve_effective`**: free function (lifted from `Engine::resolve_effective`).
- **`RegistryBundle { repository, source }`**: fetch result without duplicate provenance fields.
- Public re-exports: **`Explanation`**, **`Cause`**, **`ExplanationNode`**, **`DataPath`**, **`TimezoneValue`**. Rational helpers are not a supported consumer API (in-tree tests use a hidden module).
- Show `DataEntry` wire field **`lemma_type`** (was `schema_type` on older schema shapes).
- Registry feature also exports **`LemmaRepository`**, **`LemmaSpec`**, **`LemmaSpecSet`** for typed `Context` use.

### Changed

- **Evaluation architecture**: `ExecutionPlan` ships a dense shared NormalForm table; rules name roots; Rule references lower as Kind-sharing overlays (not closed per-rule Expression inlines). Runtime walks that DAG for evaluation (value memo by cell id; Rule embeds evaluate the named rule once), explanation (fill planning-time static trees when `explain` is set), and per-rule unbound-input discovery: no second instruction stream.
- **Resource limits**: `max_normalized_expression_nodes` counts **unique reachable NormalForm cells** per rule root (not tree-expanded size). `max_normal_form_depth` bounds DAG nesting for recursive eval. Self-doubling rule chains stay linear under sharing and are not rejected solely for that pattern; non-sharing blowups still hit the cell budget.
- **Show vs run data**: **`show`** lists statically reachable data after normalize (all remaining unless arms; no caller overlay) with types, prefilled literals, and suggestions. Overlay-aware unbound keys for a concrete run are per-rule **`missing_data`** only: evaluate JSON has no top-level `data` array.
- **Evaluate JSON** (CLI `--json`, HTTP POST, WASM/Hex/JS `run`): each rule result may include **`missing_data`**. Human `lemma run` prints a **Missing data** section when any requested rule lists unbound keys.
- **Explanation wire**: every node uses language-facing `type` + `name`. Root `results.<rule>.explanation` is the same shape as nested rule nodes (`"type":"rule"`, `"name"`, …). Bound data is `"type":"data"`; cause paths never looked up are `"type":"data_unused"`. Documented in [`api.v1.json`](engine/schemas/api.v1.json) (`RuleNode` / `ExplanationNode`). No legacy aliases.
- **MCP:** `list_specs` → **`list`**, `get_spec_source` → **`source`**; **`add_spec`** returns structured JSON `{ message, specs: Show[] }`.
- **HTTP GET `/`:** returns **`Engine.list()`** JSON (`ResolvedRepository[]`); no per-spec show payloads or `?effective=` on the list route.
- **Static interface**: **`Engine::show`** lists only data used by the spec's rules (plus local rule result types). Rule-scoped unbound keys on a partial run come from each result's **`missing_data`**.
- **Evaluation**: `run` no longer aborts with `Err` for unknown input keys or per-field bind failures (size limits, import overrides); evaluation completes with computation vetoes on affected rules.
- **Overlay bindings**: bad Data overrides (parse, constraints, options, decimals, oversize) bind as `OperationResult::Veto` on that Data: no separate `violated` map. Import aliases are ignored like unknown keys. Duplicate canonical keys (`Age` + `age`) → request `Error`. `MissingData` may suggest a near match from ignored keys.
- **Provenance**: `SourceType` is the sole load-time provenance input. `Path` / `Volatile` = workspace; `Dependency(id)` tags repositories (including embedded stdlib bootstrap as `Dependency("lemma")` internally).
- **JS/Hex load API**: unified **`load`** / **`Lemma.load/2`** (string/binary → volatile; object/map or list → labeled). Empty-string volatile labels removed: use inline string/binary volatile load.
- **Hex/JS bindings**: batch load via `Dependency(id)` source labels (`@org/pkg` keys).
- **Calendar range slots**: declare `units.calendar range` (named-type `range` suffix). A range-shaped `-> suggest` on scalar `units.calendar` no longer promotes the slot to a measure range.

## [0.8.22] - 2026-07-09

0.8.22 adds Windows ARM64 release artifacts and published coverage reports, defines API wire format for ratio and measure literals (canonical in the VM, per-unit on schema and response JSON), sharpens schema/data overlay semantics for interactive and API prompts, defers decimal commit vetoes to response materialization, speeds up evaluation with shared VM values, and fixes range-containment parsing plus last-match-wins unless schema pruning.

### Added

- **Windows ARM64 prebuilt binaries**: release workflow builds CLI and Hex NIF for `aarch64-pc-windows-msvc` on `windows-11-vs2026-arm`; npm publishes `@lemmabase/cli-win32-arm64`.
- **Published test coverage reports**: `cargo coverage <engine|cli|all>` regenerates [`cli/documentation/reference/coverage/`](cli/documentation/reference/coverage/) from `cargo-llvm-cov` + nextest; `cargo coverage all --check` verifies reports match inputs; `cargo precommit` runs the check at the end.
- **`prefilled` / `supplied` / `default` schema fields**: API and interactive schema distinguish spec literals, caller overlay values, and `-> default` suggestions; only caller-supplied values decide unless-arm pruning.
- **API wire format contract tests**: `engine/tests/api_wire_format.rs` encodes ratio, measure, and range wire rules, plan-persistence separation, and overlay behavior; npm WASM tests mirror schema and response JSON serialization.

### Changed

- **API wire format for literals**: schema defaults and `response.data` emit per-unit magnitudes in `value` for measure and named-unit ratios (for example `"25"` for `eur_per_hour`, `"15"` for percent, `"500"` for basis_points); optional top-level `measure` / `ratio` unit maps mirror rule results when the type declares multiple units. Bare ratios stay canonical (`"0.5"`). Evaluation and VM plan constants keep canonical rationals in memory.
- **Wire serde boundary**: canonical `LiteralValue` serde restored for execution-plan constants; API-only wire adapters on `DataEntry` and response data values (`api_wire_literal`).
- **Decimal commit veto deferred to materialization**: evaluation keeps exact rationals in the VM; decimal-scale overflow vetoes happen when building the response, not mid-evaluation.
- **Shared VM operand values**: rule and intermediate results use `Arc<LiteralValue>` with borrow-based operand reads, cutting allocation churn on large specs (order_pipeline benchmark allocs roughly halved).

### Fixed

- **Ratio and measure JSON serialization**: global wire patching on `LiteralValue` leaked per-unit magnitudes into execution-plan constants and mis-serialized percent and basis_points; WASM `JSON.stringify(response)` no longer crashes or double-scales measure values.
- **Decimal prompt defaults**: `magnitude_suggestion_for_decimal_prompt` uses the same per-unit materialization as API wire (basis_points prompts show `"500"`, not `"0.05"`).
- **Measure overlay rejects double-canonical input**: per-unit API magnitudes supplied as full-precision canonical values are rejected at overlay resolution instead of silently evaluating wrong results.
- **Range endpoint parsing**: `start...start + length` binds `+ length` inside the range literal (matches expression precedence); parenthesized span-add with `in` is rejected at planning time.
- **Unless schema pruning respects last-match-wins**: `collect_needed_data_paths` walks unless arms in reverse source order, so a decided later unless arm no longer pulls in data only needed by earlier arms (fixes interactive over-prompting for imported spec fields).

## [0.8.21] - 2026-07-04

0.8.21 renames `quantity` to `measure` throughout the language and API, hardens all server boundaries against resource exhaustion, fixes the ISO-week rollover bug, guarantees deterministic planning order, and adds granular LSP semantic tokens.

### Added

- **`declarationKeyword` semantic token type**: `spec`, `data`, `with`, `rule`, `repo`, `uses`, and `meta` keywords now emit as `declarationKeyword` (index 12), allowing editors to style structural keywords independently from the names they introduce.
- **Granular data-body tokenization**: type keywords (`number`, `measure`, `text`, etc.) and constraint words (`minimum`, `unit`, `option`, etc.) after `->` now emit as `keyword` instead of `dataBody`.
- **Colored dots in reference paths**: dots within identifiers like `units.mass` now emit as `reference` tokens.
- **Legend sync test**: validates `monaco.js` and VS Code `package.json` semantic token definitions match the Rust legend.
- **Parse-time limits**: `MAX_NUMBER_DIGITS` and `MAX_TEXT_VALUE_LENGTH` enforced in the lexer with source-located errors.
- **Spec-dependency limits**: new `ResourceLimits` fields `max_spec_dependency_depth` (default 32) and `max_dag_specs` (default 4096); exceeded → planning error.
- **HTTP wall-clock timeout**: `--eval-timeout` flag; zero-budget requests return 503 with a JSON error.
- **MCP request timeout and line-length cap**: `--request-timeout` flag; oversized stdin lines rejected with JSON-RPC error.
- **Restrictive CORS default**: permissive CORS requires `--cors` flag; non-localhost bind logs a warning.
- **Plan determinism test**: repeated loads in one process assert byte-identical serialized plans.
- **NIF `max_normalized_expression_nodes`**: exposed in `Lemma.new/1` limits map.
- **NIF DirtyCpu scheduling**: `lemma_load`, `lemma_load_from_paths`, `lemma_load_batch`, `lemma_run` no longer block BEAM scheduler threads.

### Changed

- **`quantity` → `measure`** throughout the language, engine API, tests, and documentation. The keyword `measure` replaces `quantity` in specs; existing type semantics are unchanged.
- **HTTP server lock scope**: `Arc<RwLock<Engine>>` → `RwLock<Arc<Engine>>`; evaluation runs on a cloned Arc inside `spawn_blocking` so the lock is never held during work.
- **HTTP schema list surfaces errors**: per-spec `{name, error}` entries instead of silently dropping broken specs.
- **MCP `get_schema` propagates real errors** instead of mapping all failures to "spec not found".
- **NIF `lemma_list` error style**: plan/schema failures return `{:error, map}` instead of raising.
- **Documentation restructured**: learn guide, reference, tools sections; removed whitepaper, blueprint, old CLI docs.
- **Fuzz targets strengthened**: property assertions, depth range crossing limits, corpus rename.

### Fixed

- **ISO-week boundary rollover**: `calendar_boundaries` now uses `NaiveDate::from_isoywd_opt` with shifted week, handling year rollover (week 1 → prior year week 52/53) and 53-week year.
- **Deterministic planning order**: `sort_derived_measure_types_for_resolution` sorts type-name vecs; `TypeResolver::resolve_types_internal` sorts `data_defs.keys()` before Kahn queue; `HashSet` dependency sets replaced with `BTreeSet`.
- **Invariant enforcement**: soft-skip `continue` → `expect("BUG: ...")` in `check_rule_types` and `infer_rule_types`; dead `cast_ratio_to_unit` measure-family branch removed; AST `source_location` validated at `Graph::build` entry.
- **BigInt allocation**: removed `unsafe` block; uses safe fallible allocation.
- **Explanation causes**: flipped conditions state the true form; implicit unit reconciliation shown as equivalence facts.

## [0.8.20] - 2026-06-19

0.8.20 CI-gates every ```lemma fence in the repo and adds offline registry fixtures for tests.

### Added

- **`documentation_fences` integration test**: parses, loads, and runs every ```lemma fence in repo `*.md` / `*.txt` files.
- **`LemmaBase::test()` / `with_fixture_dir`**: offline registry backed by bundled fixtures in `engine/tests/registry_fixtures/` (no network).
- **`lemma lsp` in CLI docs**: documents the language server command.

### Changed

- **Documentation overhaul**: registry examples and test fixtures migrated from `@lemma/std` to `@iso/countries`; embedded stdlib remains `uses lemma units`.
- **README direction**: inversion listed as planned work rather than already exposed.

### Removed

- **Internal plan documents** under `cli/documentation/plans/` (5 files).

## [0.8.19] - 2026-06-11

0.8.19 fixes registry resolution and measure planning/materialization bugs; removes a redundant SDK method.

### Added

- **Measure ceil/floor/round/abs**: preserve operand unit.

### Fixed

- **Registry resolve skips non-`@` repository qualifiers**: workspace-local repository references no longer trigger registry fetches.
- **Decomposition promotion**: binding aliases no longer collide across measure families.
- **Materialization**: converted quantities honor type `decimals`; decimal overflow vetoes the rule.
- **Inherited units**: conflicting inherited unit definitions rejected at planning.
- **Unit-index validation**: structural plan checks run at planning and deserialize, not first `run`.

### Removed

- **`Engine.repositories()` / `Lemma.repositories/1`**: returned only `{ name, dependency }` per loaded repository: the same fields already on `list()[].repository`. Use `list()` for loaded-repo discovery.

## [0.8.18] - 2026-06-10

0.8.18 completes the recorded-execution explanation architecture: explanations now read all runtime facts (register values, branch decisions, winning arm) from a recorded execution of the rule's source-shaped instruction stream: they never re-evaluate expressions. The language server is unified into the `lemma` CLI binary, eliminating the standalone `lsp` crate.

### Added

- **Explanations from recorded execution**: each rule now carries a second instruction stream (`source_instructions`) compiled from the unoptimized source expression graph. When explanations are requested the VM executes this stream, records a `RuleRecording` (register values, `BranchDecision` per `JumpIfFalse`, winning `Return` pc), and the explanation builder reads that recording: it never calls back into the evaluator. This makes it structurally impossible for an explanation to disagree with its result.
- **Arm and conversion tags on instructions**: `Instructions` now carries `arm_tags` (mapping each `JumpIfFalse`/`Return` to a source branch index and `ArmRole`) and `conversion_tags` (mapping `UnitConversion` instructions to their source context). These let the explanation builder correlate recorded execution with source structure without re-parsing.
- **`UnitEquivalence` explanation node**: implicit unit conversions inside arithmetic now emit an equivalence fact (`1 mile is 1.60934 kilometer`) as a child node, so cross-unit math is auditable without external lookup tables.
- **`result` field on `Rule` explanation node**: every rule explanation now includes the computed result as a formatted string alongside the body and causes.
- **Cause `children`**: `Cause` nodes now carry child `ExplanationNode`s showing the data values and embedded rule explanations that drove the condition.
- **Negated-condition causes as true facts**: a failed comparison is flipped to its complement (`distance < 5 mile` that failed → `distance >= 5 mile`) so the explanation states what held rather than what was tested.
- **Differential optimization test suite** (`engine/tests/differential_optimize.rs`): pins the optimized and source instruction streams to identical results across the test corpus, catching optimizer divergence automatically.
- **LSP built into the CLI**: the language server now compiles directly into the `lemma` binary (`cli/src/lsp/`) using `tower-lsp` instead of depending on the separate `lsp` crate. `lemma lsp` works as before: editors need no configuration changes.

### Changed

- **Explanation builder reads recordings, not the evaluator**: `winning_source_branch_and_causes` and the body walker receive an `ExplainCtx` containing the immutable `EvaluationContext` and `RuleRecording`, removing the `&mut` evaluator dependency that allowed re-evaluation divergence.
- **`branch_semantics` functions `and_conjunct_outcome` / `or_disjunct_outcome` are now `#[cfg(test)]`**: the explanation walker no longer calls them at runtime: they remain as executable specifications verified by unit tests.
- **Instruction stream version bumped to 2**: `INSTRUCTIONS_VERSION` incremented for the new `arm_tags`, `conversion_tags`, and `source_instructions` fields; stale serialized plans are rejected at load.
- **Identity conversions omitted from explanations**: when an operand is already in the target unit, the redundant source step is suppressed.
- **Conversion multipliers prefer decimal display**: unit factors that round-trip exactly through decimal render as `1.60934` rather than a rational fraction.
- **Release workflow**: increased crates.io index propagation wait (30 s → 60 s) to reduce transient publish failures in CI.

### Removed

- **`lsp` crate dependency from CLI**: `cli/Cargo.toml` no longer depends on the workspace `lsp` crate; `tower-lsp` is used directly.
- **`unique_data_value_by_name`**: the fallback data-path lookup used by the old re-evaluating explanation walker is removed.
- **Source expression re-evaluation in explanations**: `resolve_source_expression_values` is no longer called by the explanation builder (the function remains for other internal uses).

## [0.8.17] - 2026-06-10

0.8.17 replaces tree-walking evaluation with a compiled virtual machine, makes exact math hold at any magnitude and gives every result a machine-readable explanation. Planning now compiles each rule into a validated instruction stream that a register-based VM executes, so evaluation costs only what the requested rules cost: the engine skips unrequested rules and builds explanations only on demand. Execution plans are no longer cloned per request. Larger calculations whose intermediate values exceed machine-integer range stay exact instead of switching to approximation. Current measured performance is published in [`cli/documentation/reference/benchmarks/`](cli/documentation/reference/benchmarks/).

### Added

- **Explanations fit for audit trails**: every rule result carries a flat explanation object holding the rule's body, its operand values, the branch that applied, and the condition that vetoed. The format is specified in the consumer API schema ([`engine/schemas/api.v1.json`](engine/schemas/api.v1.json)); the previous trace format was undocumented and is replaced.
- **Explanations read recorded execution, never re-evaluate**: when explanations are requested, the engine executes a source-shaped instruction stream (compiled from the same inlined rule equation with the optimizer's rewrite passes skipped) and records what happened: branch decisions, the winning arm, register values. The explanation is rendered purely from source structure plus that recording, and the recorded run's result is the response result, so an explanation can never disagree with the answer it explains. The previous implementation re-evaluated source expressions in a parallel interpreter, which could silently diverge from the VM. A differential test suite pins both instruction streams to identical results across the test corpus and documentation examples.
- **Explanations state causes as facts**: evaluated unless conditions appear as true statements: a failed `distance < 5 mile` is stated as `distance >= 5 mile`, with the data values that drove them as children. Causes render at the rule level (they explain branch selection, not the body computation), literal operands are no longer repeated below expressions that already display them, embedded rule references show `name: result` and carry their full explanation tree wherever they appear. Implicit unit reconciliation inside arithmetic and comparisons is stated as an equivalence fact (`1 mile is 1.60934 kilometer`, decimal when exact) so cross-unit math is followable without external lookup tables; identity conversions and steps that would restate an already-visible value are omitted. JSON consumers: `causes[].condition` now holds the true-form condition expression instead of a datum name, `causes[].children`, rule-node `result`, and the `unit_equivalence` node are new, and the wrapping `compose` node duplicating the rule body is gone (operands are direct children).
- **One-binary editor setup**: installing the `lemma` CLI is now the only requirement for editor support: the new `lemma lsp` subcommand starts the language server over stdio. This removes the separate language-server binary and the version skew it allowed.
- **A shared server survives bad specs**: a service evaluating specs it did not author can no longer be hung or crashed by them. Self-doubling rule chains are rejected at planning with a resource-limit error (`ResourceLimits::max_normalized_expression_nodes`, default 30,000) instead of exhausting memory; tampered or stale serialized execution plans are rejected at load by full instruction validation instead of crashing the virtual machine; a step budget halts instruction streams that loop.
- **Reproducible performance reports**: `cargo benchmarks <engine|cli|all>` regenerates the engine and CLI benchmark reports in [`cli/documentation/reference/benchmarks/`](cli/documentation/reference/benchmarks/), so the published numbers can be independently re-measured from the repository.

### Changed

- **Compiled virtual machine**: rules are compiled at planning into register-based instruction streams that the engine executes directly, replacing per-request tree-walking of the expression graph. Compilation happens once per plan; evaluation then dispatches flat instructions over a register file. Run output is unchanged.
- **Greater precision for math with large numbers**: financial and scientific calculations whose intermediate values grow very large now stay exact end to end. Previously, magnitudes were bounded by `i128` (~1.7×10³⁸) and arithmetic beyond that bound fell back to decimal approximation; that fallback is gone. A calculation that genuinely exhausts memory vetoes the affected rule with `out of memory` rather than taking the process down. Transcendental functions (`sqrt`, `sin`, `log`, …) compute in decimal as before; see [`cli/documentation/learn/precision.md`](cli/documentation/learn/precision.md).
- **Improved performance**: callers that need one answer no longer pay for the whole spec. Evaluation computes only the requested rules (`rules: Option<&[String]>` on `Engine::run` / `Engine::run_plan`), explanations are built only when `explain` is set, and immutable plans (`DataOverlay`) remove the per-request plan clone; the VM (above) removes per-request expression-tree walking. On the benchmark specs, a single-rule evaluation measures 20–169 µs where 0.8.16 measured 285 µs–6.2 ms evaluating every rule with per-call JSON parsing: methodology and numbers in [`cli/documentation/reference/benchmarks/engine.md`](cli/documentation/reference/benchmarks/engine.md). API: `None` means all local rules, and `lemma::plan(context)` is now `lemma::plan(context, &ResourceLimits)`.
- **Plans serve concurrent requests**: an `ExecutionPlan` is immutable: data values ride alongside in a `DataOverlay` instead of mutating the plan, so one compiled plan can be shared across requests and memory allocation is reduced. Run output unchanged.
- **Decisions always show what they depended on**: the optimizer can no longer change which inputs a result requires: algebraic folds (`x * 0`, `false and …`, …) apply only to literal operands, so `rule r: x * 0` still requires `x` and vetoes when it is missing. `response.data` again lists the effective values of the data behind the requested rules (it had regressed to always empty). Together these guarantee an auditor sees the true inputs of every decision.
- **Consistent explanations**: a result exceeding the decimal output limit now vetoes identically in downstream references, `is veto` checks, explanations, and the response: previously these could disagree. Explanations of vetoed unless conditions now name the vetoing condition and carry its veto instead of describing a branch that never ran. Callers and auditors can no longer receive contradictory accounts of the same evaluation.
- **Range error messages**: mixed-type range literals (`data x: 1 ... yes`), text range literals, type references into a spec that failed its own type resolution, and temporal slices that change a type mid-history now fail planning with a descriptive error where they previously crashed the engine.
- **LSP integration**: extensions call `lemma lsp`; requires a globally installed `lemma` CLI. Release the CLI before publishing the extension update. `cargo lsp` (`xtask`) release-builds `lemma` accordingly.
- **Honest cross-language benchmarks**: the Lemma-vs-Python latency ratio compares per-request evaluation on identical inline inputs: compile/import once, then timed eval on both sides (like C vs Python). Lemma compile (parse + plan) is reported separately. Python ports use exact `fractions.Fraction` arithmetic matching Lemma's rational model.

### Removed

- **Dependencies**: `num-rational`, `num-integer`, `postcard`, `sha2`, and `boolean_expression` dropped from the engine; `proptest` and `insta` dropped from dev-dependencies. Fewer third-party crates to audit and update.
- **Legacy trace API**: `EvaluationTrace` / `TraceNode`, `format_provenance_explanation`, and `Response::filter_rules` are replaced by the explanation object, `format_explanation`, and the `rules` evaluation parameter.
- **Inversion module**: the experimental inversion API was unfinished and has been removed from this release. `Engine::invert`, Elixir `Lemma.invert`, and the `lemma_invert` NIF are no longer available. Inversion will return in a future release.

## [0.8.16] - 2026-06-03

0.8.16 makes unit math smarter and the API simpler. Measure arithmetic now flows across types (`rule wage: rate * hour` resolves to a money amount on its own) and every measure or ratio result reports all of its declared units, so callers read the unit they want instead of passing display-conversion flags. Calendar periods (year, month) are now ordinary measure units from the standard library, and spec authors set values on imported specs with the clearer `with` keyword.

```lemma
spec employment_contract

data salary: measure 
  -> unit eur: 1

rule net: salary * 1.3


spec employment

uses contract: employment_contract
  -> with salary: 5000 eur

rule net_salary: contract.net
```

### Added

- **Cross-type measure arithmetic**: multiplying or dividing quantities of different types now produces the right unit automatically and promotes the result to a matching named type when one exists in scope (e.g. `rate * hour` → money). Ambiguous results are rejected at planning rather than guessed.
- **Cross-type measure comparison**: dimensionally equal quantities (e.g. a per-hour rate vs a per-minute rate) compare correctly in rule conditions and inversion.
- **Named type ranges**: declare a range over any rangeable named type, e.g. `data estimate: money range`. Unsupported bases (`text range`, …) are rejected at planning.
- **`time range`**: half-open time-of-day intervals such as `09:00...17:00`, with `in` containment and span in duration units. Endpoints must share a timezone; reversed literals do not wrap past midnight.
- **Measure-range span**: any specialized `measure range` (mass, money, duration, …) projects its width with `(lo...hi) as <unit> as number` when the unit is in the same family; cross-family span is rejected.
- **Structured data input**: JSON unit maps (`{"eur": "84"}`) are accepted at the CLI, HTTP, and WASM boundaries.

### Changed

- **Binding keyword `fill` → `with`**: set values on an imported spec with `with alias.field: …`. Local `with name: …` is rejected: use `data` for slots in the current spec.
- **In-spec unit conversion only**: display-time conversion flags (`lemma run --as`, HTTP `as_units`, WASM `rule_result_units`) are removed. Convert with `as <unit>` in the spec; measure and ratio rule results now return every declared unit as a map.
- **Calendar periods are units**: year and month are measure units in the standard library via `uses lemma units` (`units.calendar`). The standalone `calendar` and `calendar range` types are removed; a calendar range comes from `units.calendar -> default 18 year...67 year` or inline literals like `18 year...67 year`. The names `month`, `year`, `week`, and `day` are reserved for calendar/duration units.
- **No canonical unit required**: a `measure` type no longer needs a factor-1 unit; magnitudes stay anchored to the units you declare.
- **Compound unit display**: results whose unit is a combination render in operator style (e.g. `26.66… eur·hour/minute`); single-unit values stay `<magnitude> <unit>`.

### Fixed

- Unit-conversion explanations no longer drop a step when both the source and target units are explicitly declared.
- Comparing dimensionally compatible quantities of different types during inversion no longer crashes.

### Breaking

- **`fill` → `with`**: update binding rows and tooling; the serde `DataValue` tag is now `"with"`. A bare `with name:` / `fill name:` (no import alias) is a parse error.
- **Display-conversion API removed**: drop `--as`, `as_units`, `rule_result_units`, and `EvaluationRequest`; read the unit you need from each rule result's unit map (`results.<rule>.measure`, etc.). Evaluate/load no longer accept legacy `{value, unit}` payloads: use unit maps.
- **Calendar types removed**: replace `data band: calendar range` with `uses lemma units` and `data band: units.calendar -> default 18 year...67 year`. The API `kind` tags `calendar` and `calendar_range` are gone.

## [0.8.15] - 2026-05-25

### Added

- **Cross-type result unit derivation via symbolic unit signatures**: arithmetic between named measure types now derives a result unit from the user-chosen operand units. `batch_size_ce / packaging_speed` (with `packaging_speed` declared as `ce/minute`) produces `<n> minute` directly, with no `as <unit>` cast required. Combined signatures that resolve unambiguously to a single named unit in scope auto-promote the anonymous intermediate to that named type; ambiguous signatures (the same composite signature matching units in two distinct types) are now a planning error that asks the spec to rename one of the conflicting units or differentiate the factor.
- **Unified ratio units across types**: same unit name (e.g. `percent`, `permille`, `basis_points`) may be reused across distinct `ratio` typedefs in the same spec as long as the conversion factors match. Mismatched factors still error at planning. Built-in `percent` / `permille` collisions across multiple `data: ratio` fields are now valid; cross-type ratio rule-result conversion (`lemma run --as rule:unit`) works across the unified unit space.
- **Ratio range defaults**: ratio ranges may declare a default literal range, e.g. `data band: ratio range -> default 10%...50%`. The default participates in schema (`SpecSchema.data[].default`) the same way scalar ratio defaults do.
- **LSP navigation for `uses` references**: a `uses @org/repo spec` line becomes a single clickable link that jumps to the resolved dependency file in `lemma_deps/` at the spec's starting line; hover shows the LemmaBase URL. `uses lemma units` opens an on-demand snapshot at `lemma_deps/lemma.std`.
- **`cli/documentation/llms.txt`**: authoring guide for LLMs translating natural-language policy into Lemma specs. Linked from `cli/documentation/index.md` and the root README.
- **`lemma` CLI on npm**: install without Rust via `npm install -g lemma` or run ad-hoc with `npx lemma`. The umbrella `lemma` package resolves a per-platform binary from `@lemmabase/cli-{linux,darwin,win32}-{x64,arm64}` optional dependencies; no postinstall scripts, works offline once installed.

### Changed

- **Per-measure-type unit normalisation removed**: the engine no longer rescales a measure's natural-factor units to a per-type canonical at planning. Stored magnitudes follow the unit declarations as written; cross-type arithmetic combines natural factors directly, so `1 ce_per_minute * 1 minute` now lands on `1 ce` rather than going through an opaque per-type scale. Specs that relied on hidden rescaling for derived types lacking a factor-1 unit must add one (e.g. declare the canonical base unit explicitly) so that result magnitudes remain anchored to a known unit. No user-visible value change for specs whose canonical unit was already factor 1.
- **Case-insensitive logical identifiers**: spec, data, rule, unit, and repo names are canonicalised to lowercase at parse. `repo` blocks that differ only by case are merged. API surfaces (spec lookup, data override keys, `rule_result_units` keys) lowercase inputs at the boundary; internal `eq_ignore_ascii_case` lookups are replaced with exact match on canonical names. The formatter emits identifiers in lowercase.
- Test registry references and the `12_registry_references` integration example modernised to `uses lemma units` plus reformatted `uses @iso/countries alpha2` blocks.
- Quality CI workflow declares an explicit `contents: read` permission.

### Fixed

- Local test runs no longer load the embedded stdlib twice (deduplicated stdlib include when validating workspaces that already reference `lemma units`).
- Ratio typedef `default` now requires a unit (matching `minimum` / `maximum` and the 0.8.14 contract): `-> default 0.015` is rejected; use `-> default 1.5%` or `-> default 500 basis_points`.

### Breaking

- **Workspace dependency directory renamed from `.deps/` to `lemma_deps/`**. Move any existing `.deps/` content into `lemma_deps/`; old `.deps/` directories are no longer recognised. The public `LEMMA_DEPS_DIR_NAME` constant in `lemma::deps` is the single source of truth. Embedded stdlib snapshot path moved from `engine/src/lemma/si.lemma` to `engine/src/lemma/units.lemma.std` (and surfaces in workspaces as `lemma_deps/lemma.std`).
- **Identifier canonicalisation is lossy**: any integrator that round-tripped mixed-case identifiers through the engine (API calls, override maps, rule-result unit maps) now receives lowercase. Callers comparing identifiers case-sensitively to engine output must lowercase their side too.

## [0.8.14] - 2026-05-21

- Branch on failed rules: `is veto` / `is not veto` (e.g. `unless price is veto then fallback`).
- Return a rule’s result in another unit without changing the spec: CLI `lemma run --as rule:unit`, HTTP `?as_units=rule:unit,...`, MCP/WASM `rule_result_units` (measure conversion or ratio relabel).
- Time: elapsed intervals via `uses lemma units` and types like `units.duration` (no built-in `duration` type); calendar periods (`year`, `month`, `week`) on **calendar** and **date** types, not mixed with elapsed durations.
- **Calendar** type and **calendar range** for calendar-aware periods; **date**, **number**, **measure**, and **ratio** ranges with half-open `lo...hi`; width via `(lo...hi) as <unit>` for number, duration measure, and ratio ranges.
- Compound measure units (e.g. rates built from SI units in `uses lemma units`).
- Arithmetic stays exact until API output; JSON magnitudes are decimal strings (see `cli/documentation/learn/precision.md`). Division by zero in rule bodies is rejected at planning time.

### Breaking (0.8.13 → 0.8.14)

- `scale` → `measure` (and `scale range` → `measure range`) in specs and API `kind` tags.
- `uses lemma` → `uses lemma units`; stdlib is embedded `repo lemma` / `spec units` (`units.duration`, `units.length`, …).
- `Engine::run`, `run_plan`, `run_plan_without_defaults`, and `evaluate_plan` take `EvaluationRequest` after `record_operations`: pass `EvaluationRequest::default()` when you do not need display conversion.
- Value-copy rows use `fill` (removed `from` keyword); integrators: `rule_result_measure_units` → `rule_result_units`.
- Ratio typedef `minimum` / `maximum` / `default` must include units (`10%`, not bare `10`).

## [0.8.13] - 2026-05-06

### Added

- Public `lemma::deps`: `lemma_deps_dir`, `relative_dependency_cache_path`, `dependency_identifier_from_dependency_path`, `dependency_cache_file`: `.deps/` layout shared by CLI fetch, workspace load, and LSP.
- `cargo lsp` (`xtask`): release-build `lemma`, then `npm ci` + `npm run compile` in `engine/lsp/editors/vscode`; `cargo lsp vsix` runs `npm run package` and prints the newest `.vsix` path.
- `@lemmabase/lemma-engine` npm bundle: `LspClient.didClose` sends `textDocument/didClose`.

### Changed

**Engine public API**
- Removed `Engine::list_specs()`. Use `Engine::get_workspace()` or `Engine::get_repository(qualifier)` instead: both return `ResolvedRepository { repository: Arc<LemmaRepository>, specs: Vec<LemmaSpecSet> }`.
- `Engine::get_workspace()` returns `ResolvedRepository` (was `Arc<LemmaRepository>`).
- `Engine::get_repository(qualifier)` returns `Result<ResolvedRepository, Error>` (was `Result<Arc<LemmaRepository>, Error>`).
- `Engine::list()` returns `Vec<ResolvedRepository>` (was `Vec<Arc<LemmaRepository>>`).
- `Engine::load(code, source_type)` replaces the old `load(HashMap<SourceType, String>)`. Single source, single call.
- `Engine::load_batch(sources, dependency)` replaces `load_files` / `load_dependency`. Accepts `HashMap<SourceType, String>`.
- `collect_lemma_sources` replaces `collect_lemma_files` (filesystem path expansion helper).
- `ResourceLimits`: `max_sources` and `max_source_size_bytes` replace `max_files` and `max_file_size_bytes`; matching resource-limit error ids updated.
- `SourceType::Path` replaces `SourceType::File` (Serde variant `"path"` instead of `"file"`).
- `Engine::sources()` removed: error formatting uses `Error`'s `Display` impl directly.
- `repo: None` in `get_plan` / `get_spec` / `remove` means workspace (not global search).
- `ExecutionPlan::with_defaults` simplified; call order in `run_plan` is now `with_defaults()` then `set_data_values()`.
- `ResolvedRepository` / `Engine::list()` documented: `LemmaRepository` and each `LemmaSpec` carry `start_line` and `source_type` from parse/load.

**WASM / NPM package**
- WASM `Engine.list()` matches `Engine::list()` JSON (`ResolvedRepository[]`: each `specs` is `LemmaSpecSet[]`; each set’s `specs` is `LemmaSpec[]` with full AST nodes, not flat catalog rows). `Engine.schema` / `Engine.run` take optional `repository` first (workspace when `null`/empty), matching the Rust engine.
- Various improvements to the LSP and syntax highlighting (removed redundant TM)

**Hex**
- `Lemma.list/1` returns specs grouped by repository via `Engine::list()`; each `repository` and each spec row includes `start_line` and `attribute` (load source label).
- Moved `temporal_api_sources` and `generate_openapi` to `Lemma.OpenAPI` module.
- Engine limits map keys: `max_sources` and `max_source_size_bytes` replace `max_files` and `max_file_size_bytes`.

**Parsing / AST**

- Removed the unused `TokenKind::DurationKw` surface and `PrimitiveKind::Duration` / `ConversionTarget::Duration` AST variants: the word `duration` is a normal identifier (e.g. a typedef may be named `duration`). The old built-in duration value/type shapes are gone; time periods are **measure** values whose type declares `-> trait duration` (canonical **second**), carried as `Value::Measure` / `ValueKind::Measure` only.
- `parse_value_from_string` has no separate duration primitive; duration-shaped values are measure literals resolved against in-scope trait-duration quantities.
- On measure types that declare `-> trait duration`, `minimum`, `maximum`, and `default` constraints accept legacy duration-shaped literals and normalize them through the measure unit table (or canonical base when the unit name is spelled differently).
- Removed the `-> precision` type constraint command on `measure` and `number` types (use `-> decimals` for decimal-place limits).
- Schema `units[]` on `measure` and `ratio` types include per-unit `minimum`, `maximum`, and `default` magnitudes (type-level bounds stay canonical).
- `DataValue::Fill` with `FillRhs` (`Literal` | `Reference`): every `fill` row uses this variant so `fill` is never encoded as `Definition`. Literal and reference right-hand sides for `fill` share no AST shape with `data …: <literal>`.
- `SpecRef` records optional `repository_span` and `target_span` (serde omits when absent) for tooling; parser fills spans on registry qualifiers and spec-reference targets.

**Formatter**

- Spec bodies indent `meta`, sorted `data`, and each rule line consistently; `data` definitions with `->` constraints wrap constraints onto indented continuation lines under the head.

**LSP**

- `documentLink` uses parsed `SpecRef` spans: registry URL on the qualifier span when available; on native, resolved target opens the dependency file (`SourceType::Path` when known, else `.deps` cache path) with `#L<start_line>`.
- Workspace tracks a host root for those paths; file discovery includes `.deps/**/*.lemma` (other dot-directories still skipped). Debounced validation loads fetched bundles under `.deps/` like the CLI when present.
- Removed regex-based `spec_links` module in favor of AST-driven links.

**CLI**

- Fetch and workspace loading call `lemma::deps::*` instead of duplicate helpers.

**VS Code extension**

- Ships `LICENSE` alongside the extension manifest.

## [0.8.12] - 2026-04-28

### Fixed

- **Hex publish**: the standalone `Cargo.toml` rewrite for the published Hex tarball now also rewrites `lemma-openapi` from a workspace path dep to the matching registry version, mirroring the existing `lemma-engine` rewrite. Without this, end users compiling from the Hex tarball (or `mix hex.publish`'s own verification compile) saw two distinct `lemma-engine` instances (one pulled from crates.io via `lemma_hex`, one from the local path via `lemma-openapi`) producing type mismatches on shared types like `Engine` and `DateTimeValue`.

### Changed

- **Schema: prefilled vs default vs supplied**: `SpecSchema` `DataEntry` replaces `bound_value` with `prefilled` (spec literal or literal `with` binding), adds `supplied` (caller overlay when building schema), and keeps `default` (`-> default ...` suggestion only). Branch-skip analysis still uses caller overlay only; prefilled values do not prune unless arms.
- **Evaluator**: Reference resolution copies only from the target path's binding; it does not read `local_default` (defaults are plan-prep only).
- **npm release workflow**: `publish-npm` now uses npm Trusted Publishing (OIDC) via `npm/publish@v1.0.1` with `id-token: write`, eliminating the long-lived `NPM_TOKEN` secret and the `EOTP` 2FA failure mode for automation.
- **npm package metadata**: `engine/packages/npm/build.js` emits `repository.url` as `git+https://github.com/lemma/lemma.git`, silencing npm's autocorrect warning on publish.

## [0.8.11] - 2026-04-28

### Added

**Data references (value-copy)**
- New `DataValue::Reference` AST variant: `data license2: l.other` or `data i.slot: src` copies the value of another data or rule result into the declared name. Dotted RHS paths always produce a reference; a non-dotted RHS in a binding LHS (e.g. `data i.slot: src`) also produces a reference. `data x: someident` without a dotted path or binding LHS remains a type annotation.
- Reference targets may be data paths or rule results. Rule-target references are resolved lazily in topological order at evaluation time.
- Local `-> ...` constraints on a reference (e.g. `data clamped: l.price -> maximum 1000 eur`) are merged with the LHS-declared type and validated against the copied value at runtime: a violation produces a Veto, not a planning error.
- `-> default N` on a reference supplies a fallback when the target has no value (missing input or rule veto). The default is also surfaced in the spec schema (`SpecSchema.data[].default`).
- Planning rejects a reference whose LHS-declared measure family differs from the target's family (e.g. `eur` vs `celsius`): same `measure` discriminant is no longer sufficient.
- Runtime `LiteralValue` stored under a reference path carries the reference's `resolved_type` (LHS-merged), not the target's looser type.
- `engine/tests/data_references.rs` covers the full reference surface: value copy, chain resolution, user-value override, cycle detection, type mismatch, rule-target lazy resolution, measure-family mismatch, local default in schema, runtime type invariant.

**Temporal ranges**
- `Engine::get_spec_set`, `LemmaSpecSet::iter_with_ranges`, `Context::iter_with_ranges`, `Engine::list_specs_with_ranges`: catalog queries returning half-open `[effective_from, effective_to)` ranges per temporal version.
- HTTP schema JSON `versions[]`: `effective_to` alongside `effective_from`. OpenAPI: `x-effective-from` / `x-effective-to` on spec path items; `versions` schema documents both bounds; legacy `/schema/*` routes omitted from generated OpenAPI.
- Hex `Lemma.list/1`: `:effective_to` per entry. WASM `WasmEngine::list`: compact `{name, effective_from, effective_to}`.
- `engine/tests/temporal_range_references.rs`: blueprint §2.1 test suite: qualified ref transitive subtree resolution, qualified-only edges do not split consumer slices, qualified ref skips coverage requirement, unqualified still requires full-range coverage, mixed qualified/unqualified slice counts, qualified type-import instant isolation.

**Literal layer**
- `MeasureUnits` / `RatioUnits` structs replacing unstructured vecs; `MeasureUnit` / `RatioUnit` carry name + factor.
- Stricter `NumberWithUnit` and `RatioLiteral` parsing: unit must be present for measure and ratio literals.

**CLI and tooling**
- Interactive mode improvements.
- Veto type enum for classification in responses.

**Documentation**
- `cli/documentation/blueprint.md`: normative semantics document covering goals, temporal composition, planning architecture, feature catalog.
- `cli/documentation/reference.md`: new "Data References" section; corrected text / duration type command tables; duration gains `minimum` / `maximum`.

### Changed

**Terminology**
- `fact` / `type` keywords unified into `data` everywhere: integration examples (`01_simple_data.lemma`), engine tests (`data_bindings`), fuzz targets (`fuzz_data_bindings`), all docs and examples.

**Planning subsystem**
- Major refactor: `graph.rs`, `execution_plan.rs`, `semantics.rs`: consolidated from standalone `fingerprint`, `temporal`, `types`, `validation`, `slice_interface` modules into core planning files.
- New `PageSetId` module for parsing and identifying spec-set identifiers.
- New `discovery` module: `resolve_spec_ref`, `dependency_edges`, `validate_dependency_interfaces`, `build_dag_for_spec` for topological sort and cycle detection.
- `LemmaSpecSet`: `effective_range`, `temporal_boundaries`, `effective_dates`, `coverage_gaps` for temporal slice computation.
- `SpecSchema.data[].default` now uses `DataDefinition::schema_default()`, which surfaces `-> default N` from both `TypeDeclaration` and `Reference` entries. Previously references silently dropped their declared default.
- `CommandArg` enum collapsed to `Literal(Value)`: command arguments are directly typed literals rather than raw strings.

**Types**
- `TypePageification::Text` drops `minimum` / `maximum` length-range constraints; only `length` (exact match) remains. Specs using `text -> minimum N` or `text -> maximum N` are rejected at planning.
- `TypePageification::Duration` gains `minimum` / `maximum`.
- Reference kind compatibility check replaced discriminant-only comparison with `has_same_base_type` + `same_measure_family`: measure types in different families are now correctly rejected.

**Inversion subsystem**
- Refactored into separate modules: constraints, domain, solve, world, target.

**Other**
- Parser, lexer, AST, evaluation, formatting improvements.
- LSP: workspace, spec links, server improvements.
- OpenAPI crate rewrite.
- Hex NIF native API and tests.
- npm package renamed `@lemmabase/lemma-engine`; repository moved to `github.com/lemma/lemma`.

### Removed

- `engine/tests/wasm_build.rs`.
- Tracked scratch files `plan.txt`, `deleted_tests.txt`.
- Superseded engine integration tests: `bdd`, cross-spec interface contract, end-to-end, older inversion suites, `type_propagation`, `missing_fact_propagation` (replaced by focused missing-data tests).
- `cli/tests/integrations/interactive.rs` (superseded by interactive mode tests).
- `cli/documentation/plans/temporal_ranges_blueprint_alignment.md` and `temporal_ranges_tests.md` (implementation complete; absorbed into `blueprint.md §2.1` and `engine/tests/temporal_range_references.rs`).
- `cli/documentation/plans/tables.md` (obsolete syntax; tables not yet implemented; direction noted in `blueprint.md §3.14`).
- `TypePageification::Text` `minimum` / `maximum` length commands (breaking change; use `length` for exact length).

## [0.8.10] - 2026-03-31

### Added

- Nix flake dev shell (Rust from `rust-toolchain.toml`, cargo-nextest, cargo-deny, wasm-pack, Node 24, Elixir, nixpkgs-fmt formatter) plus `flake.lock`.
- `rust-toolchain.toml`: `wasm32-unknown-unknown` target.
- Test cases for temporal type imports.
- `ExecutionPlan.sources`: keyed `PageSources` map (`IndexMap<(name, effective_from), source>`) with AST-reconstructed canonical source for every spec in the plan. Custom serde serializes as `[{name, effective_from, source}]` for downstream consumers.

### Changed

- CLI: workspace or `.lemma` file is a positional argument (`run`/`schema`/`fetch [source] [spec]…`, `list`/`server`/`mcp [source]`); `-d`/`--dir` removed. Page auto-selected when the source defines exactly one spec; multiple specs without a name yield an error listing names (or use `-i`). Lemma source from filesystem only; positional `-` is rejected (not a valid path).
- Planning: `DataDefinition::SpecRef.resolved_plan_hash` is a required `String`; fingerprints always build `PageId` from it (no optional fallback to bare spec name).
- Graph / types: missing plan hash on type-import or spec-reference binding yields validation errors instead of `unreachable!` when a dependency spec failed validation or is absent from the hash registry.
- `build_graph` test helper pre-plans dependency specs so `PlanHashRegistry` matches topological `plan()` behavior.
- `.gitignore`: `result` / `result-*` (Nix build outputs).
- Fixes for temporal type imports, to properly pin and resolve them.
- Fix for docker image building in CI.
- Formatter cleanup: deterministic output improvements.
- Deterministic fingerprinting on semantics.
- Type resolver: rename contributing-spec registration to `register_dependency_specs` to clarify scope.

### Removed

- `==` / `!=` syntax (use `=` and `!=` was already removed).
- Raw source text from operation records and expression evaluation (replaced by plan-level `sources`).

## [0.8.9] - 2026-03-30

### Changed

- Precompiled Hex NIFs: drop `x86_64-unknown-linux-musl` from the release build-nif-binaries matrix and RustlerPrecompiled `targets` (that triple cannot build this `cdylib`); Linux x86_64 uses `x86_64-unknown-linux-gnu` only.
- Hex README: precompiled Linux wording matches (gnu x86_64 + arm64).
- Workspace / crates / VS Code extension / lockfiles bumped to 0.8.9 (routine version alignment).
- Linux `lemma` CLI release assets are musl static only (`lemma-*-linux-musl.tar.gz`); publish-docker copies them into `FROM scratch`. GNU Linux CLI tarballs removed. Hex NIF prebuilds stay linux-gnu (`cdylib`).

## [0.8.8] - 2026-03-29

### Added

- Release workflow build-nif-binaries job: cross-build `lemma_hex` for macOS (arm64/x86_64), Linux (gnu arm64/x86_64, musl x86_64), Windows x86_64; package `.so`/`.dll` as versioned tarballs and upload to the `cli-v*` GitHub release.
- Hex package uses rustler_precompiled: consumers download matching NIFs from release assets; contributors can still compile from source with `LEMMA_BUILD_NIF=1` and Rust on `PATH`.
- publish-hex runs `mix rustler_precompiled.download Lemma.Native --all --print` (with `GITHUB_TOKEN`) so checksum files are generated before publish; job depends on build-nif-binaries completing.

### Changed

- Hex `mix.exs` OTP application `:lemma` → `:lemma_engine`; package `files` list includes checksum scripts and trimmed native sources for precompiled workflow.
- Hex README: documents precompiled targets and dev workflow (`LEMMA_BUILD_NIF=1` for `mix compile` / `mix precommit`).
- Workspace / crates / VS Code extension / lockfiles bumped to 0.8.8 (routine version alignment).

### Removed

- `engine/packages/hex/.mise.toml` (Erlang/Elixir pin no longer shipped in the package tree).

## [0.8.7] - 2026-03-28

### Added

- `PageId` type (`name` + `plan_hash`) with `Display` impl (`name~hash`); replaces ad-hoc `Arc<ExecutionPlan>` set and `format!` string concatenation in fingerprints.
- Execution plans now carry `dependencies: IndexSet<PageId>` populated from dependency rules in topological order.
- Six dependency-tracking unit tests: basic cross-spec, standalone, multiple deps, hash correctness, unused spec ref, and implicit dep via rules.

### Changed

- Cross-spec interface validation improvements and stricter test assertions.
- Fingerprint `spec_id` fields use `PageId::to_string()` instead of raw `format!("{}~{}", ...)`.

### Removed

- `serde(alias = "expected_hash_pin")` backwards-compat shim and its test.

## [0.8.6] - 2025-03-27

### Changed

- Hex publishes the Elixir package as `lemma_engine` instead of `lemma`. Replace `{:lemma, ...}` with `{:lemma_engine, ...}` in `mix.exs`, README, and the GitHub release workflow Elixir snippet; `mix.exs` sets `package` `name: "lemma_engine"`.
- Workspace and artifacts are bumped to 0.8.6 (root `Cargo.toml` / lockfile, `lemma-cli`, `lemma-engine`, `lemma-openapi`, `lsp`, VS Code `package.json` / lockfile, Hex `@version`).
- Root README rewrites the “Why Lemma?” and “What about AI?” sections: clearer story on rules vs systems, single source of truth, determinism and auditability, and how Lemma differs from approximate AI for compliance-style logic.

## [0.8.5] - 2025-03-27

### Added

- Cargo aliases `cargo bump`, `cargo verify`, and `cargo changelog` wired to xtask: centralized versions-bump (workspace semver + mirrored pins in CLI/OpenAPI/LSP manifests, Hex `mix.exs`, `engine/README.md`, VS Code `package.json`), versions-verify, and versions-diff (tag-to-tree or tag-range changelog helper).
- versions-verify step in the quality workflow lint job so CI matches local precommit.
- `xtask/README.md` and a maintainer Release version section in `cli/documentation/contributing.md`; `README.md` documents running versions-verify in precommit and using bump/verify when changing the release.

### Changed

- Workspace release 0.8.5 across crates, `Cargo.lock`, exact path-dep pins, Hex `@version`, engine README quick-start line, and VS Code extension version (aligned with the workspace release; release workflow no longer rewrites extension version in a separate Node step).
- `cargo precommit` runs versions-verify before fmt, Clippy, nextest, and cargo-deny. Also triggers SDK precommits (npm precommit, mix precommit).
- Release workflow: Intel macOS build uses macos-15-intel instead of macos-13.
- Hex `mix.exs`: ex_doc added as a dev-only dependency; dependency ordering/lockfile updated.

### Removed

- Jekyll/GitHub Pages scaffolding: `cli/documentation/Gemfile` and `cli/documentation/_config.yml`.

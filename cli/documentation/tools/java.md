---
nav_title: Java / Kotlin
nav_order: 40
---

# Java / Kotlin

Embed the Lemma engine in Java, Kotlin, or Scala via `com.lemmabase:lemma-engine` on Maven Central. The JAR ships prebuilt `lemma_jni` natives for the same six targets as the Hex package (glibc-only; no musl). Alpine Linux images are unsupported and fail closed at native load.

Decimals use `java.math.BigDecimal`. `float` / `double` data values are rejected so binary floating point cannot silently corrupt exact magnitudes. See [Precision](../learn/precision.md).

## Install

Maven:

```xml
<dependency>
  <groupId>com.lemmabase</groupId>
  <artifactId>lemma-engine</artifactId>
  <version>0.9.10</version>
</dependency>
```

Gradle (Kotlin DSL): consume the published artifact; this repository builds the package with Maven only:

```kotlin
implementation("com.lemmabase:lemma-engine:0.9.10")
```

Requires JDK 21+.

## Java example

```java
import com.lemmabase.lemma.Engine;
import com.lemmabase.lemma.Response;
import com.lemmabase.lemma.RunRequest;
import com.lemmabase.lemma.RuleResult;
import java.math.BigDecimal;
import java.util.Map;

try (Engine engine = Engine.create()) {
  engine.load("""
      spec order
      data quantity: number
      data unit_price: number
      data tax_rate: number
      rule subtotal: quantity * unit_price
      rule tax: subtotal * tax_rate
      rule total: subtotal + tax
      """);

  Response response = engine.run(
      RunRequest.of("order")
          .data(Map.of(
              "quantity", 3,
              "unit_price", new BigDecimal("19.99"),
              "tax_rate", new BigDecimal("0.21"))));

  RuleResult.Number total = (RuleResult.Number) response.results().get("total");
  BigDecimal amount = total.number();
}
```

Named resource-limit overrides (unset keys keep engine defaults; numbers live in the engine, not in Java):

```java
try (Engine engine = Engine.create(ResourceLimits.builder().maxSources(64))) {
  // ...
}
```

Copy resource limits from another engine with `Engine.create(other.limits())`.

## Kotlin example

Same artifact:

```kotlin
import com.lemmabase.lemma.Engine
import com.lemmabase.lemma.RuleResult
import com.lemmabase.lemma.RunRequest
import java.math.BigDecimal

Engine.create().use { engine ->
    engine.load("""
        spec order
        data quantity: number
        data unit_price: number
        data tax_rate: number
        rule subtotal: quantity * unit_price
        rule tax: subtotal * tax_rate
        rule total: subtotal + tax
        """.trimIndent())

    val response = engine.run(
        RunRequest.of("order").data(
            mapOf(
                "quantity" to 3,
                "unit_price" to BigDecimal("19.99"),
                "tax_rate" to BigDecimal("0.21"),
            )
        )
    )
    val total = response.results()["total"] as RuleResult.Number
    val amount: BigDecimal = total.number()
}
```

## API surface

| Method | Role |
|--------|------|
| `Engine.create()` / `Engine.create(ResourceLimits)` / `Engine.create(ResourceLimits.Builder)` | Construct engine (named limit overrides; unset keys keep engine defaults) |
| `Engine.fromSnapshot(byte[])` | Restore engine from `snapshot()` bytes |
| `load(String)` / `load(Map<String,String>)` / `load(Path)` / `loadResource(Class, String)` | Validate and load sources (volatile, labeled, file, or classpath) |
| `install(String)` / `install(String, HttpClient)` | Download only from LemmaBase (`RepositoryInstallResult`); then `load`; does not write `lemma_deps/` |
| `run(RunRequest)` | Evaluate; named fields only |
| `list()` / `show(...)` / `source(...)` / `remove(...)` / `update(...)` | Inspect and manage loaded specs |
| `limits()` | Current resource limits |
| `snapshot()` | Opaque bytes of parsed specs + plans + limits. Restore with `fromSnapshot`. |
| `quality()` | Structural quality recommendations across loaded specs (advisory only) |
| `Lemma.format(String)` | Format source without an engine |
| `close()` | Drop native engine (`AutoCloseable` + `Cleaner`) |

`RunRequest.of(spec)` defaults: repository null, effective null, rules null (all rules), explain false. An empty `rules` list is a request error. With `explain(true)`, a value/veto/missing-data `RuleResult` may carry an `ExplanationNode.Rule` whose children are a sealed `ExplanationNode` tree. `show(...)` returns `Show`; each `Show.data` entry is `ShowData` (empty `neededByRules` = reuse-only). `RuleResult` and `RuleResultValue` are sealed; pattern-match on the variant.

Persist and restore without re-parsing:

```java
import java.nio.file.Files;
import java.nio.file.Path;

byte[] bytes = engine.snapshot();
Files.write(Path.of("engine.lems"), bytes);
try (Engine restored = Engine.fromSnapshot(Files.readAllBytes(Path.of("engine.lems")))) {
  // run / show / list
}
```

Downloads a repository from LemmaBase (download only; then `load`). Default `HttpClient` follows JVM proxy/trust defaults; pass a configured client when needed:

```java
RepositoryInstallResult installed = engine.install("@iso/countries");
engine.load(Map.of(installed.id(), installed.source()));
```

## Errors and veto

- Invalid Lemma or bad requests → unchecked `LemmaException` with `EngineError` entries (same wire shape as [api.v1.json](../../../engine/schemas/api.v1.json)).
- Domain `veto` → successful `Response` with `RuleResult.Veto`. Unbound inputs → `RuleResult.MissingData`.
- Use-after-close or invariant failures → `LemmaBugError` (`Error`).
- Missing native, unsupported platform, unwritable cache → `LemmaNativeException` (`RuntimeException`).

## Thread safety

`Engine` is thread-safe: every method acquires an internal `ReentrantLock`. Multiple threads may share one instance without external synchronization. Long-running evaluations block other callers on that lock, so prefer one engine per worker for throughput.

## Native library loading

The JAR embeds `lemma_jni` natives (glibc Linux, macOS, Windows; no musl / Alpine). On first use, the SDK extracts the native for the current platform to a content-hashed cache and loads it. Override the automatic resolution:

| Priority | Source | Example |
|----------|--------|---------|
| 1 | System property `lemma.native.library` | `-Dlemma.native.library=/opt/liblemma_jni.so` |
| 2 | Environment variable `LEMMA_JNI_LIBRARY` | `LEMMA_JNI_LIBRARY=/opt/liblemma_jni.so` |
| 3 | Bundled JAR resource (extracted to cache) | Automatic |

The cache path is `~/.cache/lemma-jni/{version}-{triple}/{sha256}/`. Override the cache root with the system property `lemma.native.cache.dir`.

## ExplanationNode dispatch

`ExplanationNode` is a sealed interface under `com.lemmabase.lemma.schema`. Each variant record implements `ExplanationNode.type()` returning its discriminator (`rule`, `compose`, `data`, `data_unused`, `conversion`, `veto`). Use pattern matching (Java 21+) or switch on `type()`:

```java
switch (node.type()) {
    case "rule" -> handleRule((ExplanationNode.Rule) node);
    case "compose" -> handleCompose((ExplanationNode.Compose) node);
    case "data" -> handleData((ExplanationNode.Data) node);
    case "data_unused" -> handleDataUnused((ExplanationNode.DataUnused) node);
    case "conversion" -> handleConversion((ExplanationNode.Conversion) node);
    case "veto" -> handleVeto((ExplanationNode.Veto) node);
    default -> throw new IllegalStateException("unknown explanation type: " + node.type());
}
```

`LemmaType` follows the same pattern with `kind()`.

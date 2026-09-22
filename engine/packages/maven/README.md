# lemma-engine (Maven)

JVM binding for the Lemma rules engine. Coordinates: `com.lemmabase:lemma-engine`.

```xml
<dependency>
  <groupId>com.lemmabase</groupId>
  <artifactId>lemma-engine</artifactId>
  <version>0.9.11</version>
</dependency>
```

Docs: [cli/documentation/tools/java.md](../../../cli/documentation/tools/java.md). With `RunRequest.explain(true)`, `RuleResult.explanation()` is an `ExplanationNode.Rule` tree.

## Develop

From the repository root:

```bash
cargo build -p lemma_jni
./mvnw -B test
```

The Maven wrapper copies the host `lemma_jni` shared library into `src/main/resources/native/<triple>/` before tests. That directory is gitignored; release packaging fills all six triples in CI.

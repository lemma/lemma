package com.lemmabase.lemma;

import com.fasterxml.jackson.core.JsonParser;
import com.fasterxml.jackson.core.JsonToken;
import com.lemmabase.lemma.schema.LemmaType;
import com.lemmabase.lemma.schema.LiteralValue;
import com.lemmabase.lemma.schema.ShowPropertyType;
import java.io.IOException;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import org.jspecify.annotations.Nullable;

/**
 * Show: the engine's Draft 2020-12 JSON Schema for a spec's evaluation-data inputs, read into typed
 * accessors. {@link #json()} is the document itself, for JSON Schema tooling.
 *
 * @param repository interned repository name ({@code x-lemma-repository}); null for the unnamed
 *     workspace
 * @param spec spec name ({@code title})
 * @param commentary commentary ({@code description})
 * @param effectiveFrom effectiveFrom ({@code x-lemma-effective-from})
 * @param effectiveTo effectiveTo ({@code x-lemma-effective-to})
 * @param startLine startLine ({@code x-lemma-start-line})
 * @param sourceType sourceType ({@code x-lemma-source-type})
 * @param types local data declarations by name ({@code x-lemma-types})
 * @param data inputs rules need by evaluation-data key ({@code properties})
 * @param required inputs a rule needs that have no committed value, sorted
 * @param rows instance-list row schemas by {@code $defs} name
 * @param rules this spec's rule graph, local plus reachable imports ({@code x-lemma-rules})
 * @param meta meta ({@code x-lemma-meta})
 * @param json the Show document
 */
public record Show(
    @Nullable String repository,
    String spec,
    @Nullable String commentary,
    @Nullable String effectiveFrom,
    @Nullable String effectiveTo,
    int startLine,
    @Nullable SourceType sourceType,
    Map<String, LemmaType> types,
    Map<String, ShowData> data,
    List<String> required,
    Map<String, ShowRow> rows,
    Map<String, ShowRule> rules,
    Map<String, LiteralValue> meta,
    String json) {
  private static final String DIALECT = "https://json-schema.org/draft/2020-12/schema";

  /**
   * One uses hop on a Show data or rule path.
   *
   * @param uses uses alias for this hop
   * @param repository target repository name; null for the unnamed workspace
   * @param spec resolved target spec name
   */
  public record PathSegment(String uses, @Nullable String repository, String spec) {
    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    static PathSegment read(JsonParser p) throws IOException {
      JsonReading.expectStartObject(p, "PathSegment");
      String uses = null;
      String repository = null;
      String spec = null;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "uses" -> uses = JsonReading.readString(p);
          case "repository" -> repository = JsonReading.readString(p);
          case "spec" -> spec = JsonReading.readString(p);
          default -> JsonReading.unknownField(field, "PathSegment");
        }
      }
      if (uses == null) {
        JsonReading.missingRequired("uses", "PathSegment");
      }
      if (spec == null) {
        JsonReading.missingRequired("spec", "PathSegment");
      }
      return new PathSegment(uses, repository, spec);
    }
  }

  /**
   * One evaluation-data input: a property of the Show document.
   *
   * @param type {@code x-lemma-type}: Lemma type without help or text options/length, which the
   *     property states as {@code description}, {@code enum} and {@code minLength}
   * @param path import hops to this slot; empty means this spec
   * @param value spec literal or literal {@code with} binding ({@code x-lemma-value})
   * @param suggestion suggestion ({@code x-lemma-suggestion})
   * @param neededByRules rules keys that need this slot; empty = reuse-only
   * @param schema the property's standard JSON Schema keywords ({@code type}, {@code pattern},
   *     {@code enum}, {@code default}, {@code examples}, {@code items}, ...)
   */
  public record ShowData(
      ShowPropertyType type,
      List<PathSegment> path,
      @Nullable RuleResultValue value,
      @Nullable RuleResultValue suggestion,
      List<String> neededByRules,
      Map<String, @Nullable Object> schema) {
    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    static ShowData read(JsonParser p) throws IOException {
      JsonReading.expectStartObject(p, "ShowProperty");
      Map<String, @Nullable Object> schema = new LinkedHashMap<>();
      ShowPropertyType inputType = null;
      List<PathSegment> path = null;
      RuleResultValue value = null;
      RuleResultValue suggestion = null;
      List<String> neededByRules = null;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "type",
              "oneOf",
              "pattern",
              "enum",
              "minLength",
              "maxLength",
              "description",
              "default",
              "examples",
              "items",
              "maxItems" ->
              schema.put(field, JsonReading.readJsonValue(p));
          case "x-lemma-type" -> inputType = ShowPropertyType.read(p);
          case "x-lemma-path" -> path = JsonReading.readList(p, PathSegment::read);
          case "x-lemma-needed-by-rules" ->
              neededByRules = JsonReading.readList(p, JsonReading::readString);
          case "x-lemma-value" -> value = RuleResultValue.read(p);
          case "x-lemma-suggestion" -> suggestion = RuleResultValue.read(p);
          default -> JsonReading.unknownField(field, "ShowProperty");
        }
      }
      if (!schema.containsKey("type") && !schema.containsKey("oneOf")) {
        JsonReading.missingRequired("type or oneOf", "ShowProperty");
      }
      if (inputType == null) {
        JsonReading.missingRequired("x-lemma-type", "ShowProperty");
      }
      if (path == null) {
        JsonReading.missingRequired("x-lemma-path", "ShowProperty");
      }
      if (neededByRules == null) {
        JsonReading.missingRequired("x-lemma-needed-by-rules", "ShowProperty");
      }
      return new ShowData(
          inputType, path, value, suggestion, neededByRules, Collections.unmodifiableMap(schema));
    }
  }

  /**
   * Row object schema of an instance list: one {@code $defs} entry per row spec.
   *
   * @param title row schema title
   * @param data row fields by key relative to the row spec
   * @param required row fields a row rule needs that have no committed value, sorted
   */
  public record ShowRow(String title, Map<String, ShowData> data, List<String> required) {
    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    static ShowRow read(JsonParser p) throws IOException {
      JsonReading.expectStartObject(p, "ShowRowSchema");
      String title = null;
      Map<String, ShowData> data = null;
      List<String> required = List.of();
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "title" -> title = JsonReading.readString(p);
          case "type" -> expectValue(p, "object", "ShowRowSchema.type");
          case "additionalProperties" -> expectTrue(p, "ShowRowSchema.additionalProperties");
          case "properties" -> data = JsonReading.readMap(p, ShowData::read);
          case "required" -> required = JsonReading.readList(p, JsonReading::readString);
          default -> JsonReading.unknownField(field, "ShowRowSchema");
        }
      }
      if (title == null) {
        JsonReading.missingRequired("title", "ShowRowSchema");
      }
      if (data == null) {
        JsonReading.missingRequired("properties", "ShowRowSchema");
      }
      return new ShowRow(title, data, required);
    }
  }

  /**
   * One arm of a local rule's flat last-match table.
   *
   * @param condition absent on the default arm; raw ShowExpression JSON object when present
   * @param result arm result expression as a raw ShowExpression JSON object
   */
  public record ShowBranch(
      @Nullable Map<String, Object> condition, Map<String, Object> result) {
    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    static ShowBranch read(JsonParser p) throws IOException {
      JsonReading.expectStartObject(p, "ShowBranch");
      Map<String, Object> condition = null;
      Map<String, Object> result = null;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "condition" -> condition = readExpressionObject(p, "ShowBranch.condition");
          case "result" -> result = readExpressionObject(p, "ShowBranch.result");
          default -> JsonReading.unknownField(field, "ShowBranch");
        }
      }
      if (result == null) {
        JsonReading.missingRequired("result", "ShowBranch");
      }
      return new ShowBranch(condition, result);
    }

    @SuppressWarnings("unchecked")
    private static Map<String, Object> readExpressionObject(JsonParser p, String label)
        throws IOException {
      Object value = JsonReading.readJsonValue(p);
      if (!(value instanceof Map<?, ?> map)) {
        throw new LemmaBugError("BUG: expected object for " + label);
      }
      return (Map<String, Object>) map;
    }
  }

  /**
   * Rule on Show: result type, path, authored branches, depends_on_rules.
   *
   * @param type result type
   * @param path import hops to this rule; empty means this spec
   * @param branches default then unless arms
   * @param dependsOnRules Show.rules keys this rule depends on
   */
  public record ShowRule(
      LemmaType type, List<PathSegment> path, List<ShowBranch> branches, List<String> dependsOnRules) {
    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    static ShowRule read(JsonParser p) throws IOException {
      JsonReading.expectStartObject(p, "ShowRule");
      LemmaType type = null;
      List<PathSegment> path = null;
      List<ShowBranch> branches = null;
      List<String> dependsOnRules = null;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "type" -> type = LemmaType.read(p);
          case "path" -> path = JsonReading.readList(p, PathSegment::read);
          case "branches" -> branches = JsonReading.readList(p, ShowBranch::read);
          case "depends_on_rules" -> dependsOnRules = JsonReading.readList(p, JsonReading::readString);
          default -> JsonReading.unknownField(field, "ShowRule");
        }
      }
      if (type == null) {
        JsonReading.missingRequired("type", "ShowRule");
      }
      if (path == null) {
        JsonReading.missingRequired("path", "ShowRule");
      }
      if (branches == null) {
        JsonReading.missingRequired("branches", "ShowRule");
      }
      if (dependsOnRules == null) {
        JsonReading.missingRequired("depends_on_rules", "ShowRule");
      }
      return new ShowRule(type, path, branches, dependsOnRules);
    }
  }

  private static void expectValue(JsonParser p, String expected, String label) throws IOException {
    String actual = JsonReading.readString(p);
    if (!expected.equals(actual)) {
      throw new LemmaBugError("BUG: " + label + " must be " + expected + ", got " + actual);
    }
  }

  private static void expectTrue(JsonParser p, String label) throws IOException {
    if (!JsonReading.readBoolean(p)) {
      throw new LemmaBugError("BUG: " + label + " must be true");
    }
  }

  /**
   * Parses JSON.
   *
   * @param p parser at value start
   * @param json the document text, kept as {@link #json()}
   * @return parsed value
   * @throws IOException if JSON IO fails
   */
  static Show read(JsonParser p, String json) throws IOException {
    JsonReading.expectStartObject(p, "Show");
    String repository = null;
    String spec = null;
    String commentary = null;
    String effectiveFrom = null;
    String effectiveTo = null;
    Integer startLine = null;
    SourceType sourceType = null;
    Map<String, LemmaType> types = null;
    Map<String, ShowData> data = null;
    List<String> required = List.of();
    Map<String, ShowRow> rows = Map.of();
    Map<String, ShowRule> rules = null;
    Map<String, LiteralValue> meta = null;
    while (p.nextToken() != JsonToken.END_OBJECT) {
      String field = p.currentName();
      p.nextToken();
      switch (field) {
        case "$schema" -> expectValue(p, DIALECT, "Show.$schema");
        case "title" -> spec = JsonReading.readString(p);
        case "description" -> commentary = JsonReading.readString(p);
        case "type" -> expectValue(p, "object", "Show.type");
        case "additionalProperties" -> expectTrue(p, "Show.additionalProperties");
        case "properties" -> data = JsonReading.readMap(p, ShowData::read);
        case "required" -> required = JsonReading.readList(p, JsonReading::readString);
        case "$defs" -> rows = JsonReading.readMap(p, ShowRow::read);
        case "x-lemma-repository" -> repository = JsonReading.readString(p);
        case "x-lemma-effective-from" -> effectiveFrom = JsonReading.readString(p);
        case "x-lemma-effective-to" -> effectiveTo = JsonReading.readString(p);
        case "x-lemma-start-line" -> startLine = JsonReading.readInt(p);
        case "x-lemma-source-type" -> sourceType = SourceType.read(p);
        case "x-lemma-meta" -> meta = JsonReading.readMap(p, LiteralValue::read);
        case "x-lemma-types" -> types = JsonReading.readMap(p, LemmaType::read);
        case "x-lemma-rules" -> rules = JsonReading.readMap(p, ShowRule::read);
        default -> JsonReading.unknownField(field, "Show");
      }
    }
    if (spec == null) {
      JsonReading.missingRequired("title", "Show");
    }
    if (startLine == null) {
      JsonReading.missingRequired("x-lemma-start-line", "Show");
    }
    if (data == null) {
      JsonReading.missingRequired("properties", "Show");
    }
    if (types == null) {
      JsonReading.missingRequired("x-lemma-types", "Show");
    }
    if (rules == null) {
      JsonReading.missingRequired("x-lemma-rules", "Show");
    }
    if (meta == null) {
      JsonReading.missingRequired("x-lemma-meta", "Show");
    }
    return new Show(
        repository,
        spec,
        commentary,
        effectiveFrom,
        effectiveTo,
        startLine,
        sourceType,
        types,
        data,
        required,
        rows,
        rules,
        meta,
        json);
  }
}

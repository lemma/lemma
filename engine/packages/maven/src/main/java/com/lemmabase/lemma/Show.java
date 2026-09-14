package com.lemmabase.lemma;

import com.fasterxml.jackson.core.JsonParser;
import com.fasterxml.jackson.core.JsonToken;
import com.lemmabase.lemma.schema.LemmaType;
import com.lemmabase.lemma.schema.LiteralValue;
import java.io.IOException;
import java.util.List;
import java.util.Map;
import org.jspecify.annotations.Nullable;

/**
 * Show.
 *
 * @param spec spec
 * @param commentary commentary
 * @param effectiveFrom effectiveFrom
 * @param effectiveTo effectiveTo
 * @param versions versions
 * @param startLine startLine
 * @param sourceType sourceType
 * @param data data
 * @param rules local rule graph
 * @param meta meta
 */
public record Show(
    String spec,
    @Nullable String commentary,
    @Nullable String effectiveFrom,
    @Nullable String effectiveTo,
    @Nullable List<ShowVersion> versions,
    int startLine,
    @Nullable SourceType sourceType,
    Map<String, ShowData> data,
    Map<String, ShowRule> rules,
    Map<String, LiteralValue> meta) {
  /**
   * One declared data slot.
   *
   * @param type type
   * @param fill spec literal or literal {@code with} binding
   * @param suggestion suggestion
   * @param neededByRules local rules that need this slot; empty = reuse-only
   */
  public record ShowData(
      LemmaType type,
      @Nullable RuleResultValue fill,
      @Nullable RuleResultValue suggestion,
      List<String> neededByRules) {
    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    static ShowData read(JsonParser p) throws IOException {
      JsonReading.expectStartObject(p, "ShowData");
      LemmaType type = null;
      RuleResultValue fill = null;
      RuleResultValue suggestion = null;
      List<String> neededByRules = null;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "type" -> type = LemmaType.read(p);
          case "fill" -> fill = RuleResultValue.read(p);
          case "suggestion" -> suggestion = RuleResultValue.read(p);
          case "needed_by_rules" -> neededByRules = JsonReading.readList(p, JsonReading::readString);
          default -> JsonReading.unknownField(field, "ShowData");
        }
      }
      if (type == null) {
        JsonReading.missingRequired("type", "ShowData");
      }
      if (neededByRules == null) {
        JsonReading.missingRequired("needed_by_rules", "ShowData");
      }
      return new ShowData(type, fill, suggestion, neededByRules);
    }
  }

  /**
   * ShowVersion.
   *
   * @param effectiveFrom effectiveFrom
   * @param effectiveTo effectiveTo
   */
  public record ShowVersion(@Nullable String effectiveFrom, @Nullable String effectiveTo) {
    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    static ShowVersion read(JsonParser p) throws IOException {
      JsonReading.expectStartObject(p, "ShowVersion");
      String effectiveFrom = null;
      String effectiveTo = null;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "effective_from" -> effectiveFrom = JsonReading.readString(p);
          case "effective_to" -> effectiveTo = JsonReading.readString(p);
          default -> JsonReading.unknownField(field, "ShowVersion");
        }
      }
      return new ShowVersion(effectiveFrom, effectiveTo);
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
   * Local rule on Show: result type, authored branches, stored depends_on_rules.
   *
   * @param type result type
   * @param branches default then unless arms
   * @param dependsOnRules local topo deps
   */
  public record ShowRule(LemmaType type, List<ShowBranch> branches, List<String> dependsOnRules) {
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
      List<ShowBranch> branches = null;
      List<String> dependsOnRules = null;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "type" -> type = LemmaType.read(p);
          case "branches" -> branches = JsonReading.readList(p, ShowBranch::read);
          case "depends_on_rules" -> dependsOnRules = JsonReading.readList(p, JsonReading::readString);
          default -> JsonReading.unknownField(field, "ShowRule");
        }
      }
      if (type == null) {
        JsonReading.missingRequired("type", "ShowRule");
      }
      if (branches == null) {
        JsonReading.missingRequired("branches", "ShowRule");
      }
      if (dependsOnRules == null) {
        JsonReading.missingRequired("depends_on_rules", "ShowRule");
      }
      return new ShowRule(type, branches, dependsOnRules);
    }
  }

  /**
   * Parses JSON.
   *
   * @param p parser at value start
   * @return parsed value
   * @throws IOException if JSON IO fails
   */
  static Show read(JsonParser p) throws IOException {
    JsonReading.expectStartObject(p, "Show");
    String spec = null;
    String commentary = null;
    String effectiveFrom = null;
    String effectiveTo = null;
    List<ShowVersion> versions = null;
    Integer startLine = null;
    SourceType sourceType = null;
    Map<String, ShowData> data = null;
    Map<String, ShowRule> rules = null;
    Map<String, LiteralValue> meta = null;
    while (p.nextToken() != JsonToken.END_OBJECT) {
      String field = p.currentName();
      p.nextToken();
      switch (field) {
        case "spec" -> spec = JsonReading.readString(p);
        case "commentary" -> commentary = JsonReading.readString(p);
        case "effective_from" -> effectiveFrom = JsonReading.readString(p);
        case "effective_to" -> effectiveTo = JsonReading.readString(p);
        case "versions" -> versions = JsonReading.readList(p, ShowVersion::read);
        case "start_line" -> startLine = JsonReading.readInt(p);
        case "source_type" -> sourceType = SourceType.read(p);
        case "data" -> data = JsonReading.readMap(p, ShowData::read);
        case "rules" -> rules = JsonReading.readMap(p, ShowRule::read);
        case "meta" -> meta = JsonReading.readMap(p, LiteralValue::read);
        default -> JsonReading.unknownField(field, "Show");
      }
    }
    if (spec == null) {
      JsonReading.missingRequired("spec", "Show");
    }
    if (startLine == null) {
      JsonReading.missingRequired("start_line", "Show");
    }
    if (data == null) {
      JsonReading.missingRequired("data", "Show");
    }
    if (rules == null) {
      JsonReading.missingRequired("rules", "Show");
    }
    if (meta == null) {
      JsonReading.missingRequired("meta", "Show");
    }
    return new Show(
        spec,
        commentary,
        effectiveFrom,
        effectiveTo,
        versions,
        startLine,
        sourceType,
        data,
        rules,
        meta);
  }
}

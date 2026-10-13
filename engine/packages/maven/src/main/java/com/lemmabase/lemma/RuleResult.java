package com.lemmabase.lemma;

import com.fasterxml.jackson.core.JsonParser;
import com.fasterxml.jackson.core.JsonToken;
import com.lemmabase.lemma.schema.ExplanationNode;
import java.io.IOException;
import java.math.BigDecimal;
import java.time.LocalDate;
import java.time.LocalTime;
import java.util.List;
import java.util.Map;
import org.jspecify.annotations.Nullable;

/**
 * Result of evaluating one rule. Pattern-match on the variant: veto, missing data, or a typed
 * value.
 */
public sealed interface RuleResult {
  /**
   * Declared rule type name.
   *
   * @return rule type
   */
  String ruleType();

  /**
   * Explanation tree when {@code explain} was requested; otherwise null.
   *
   * @return explanation or null
   */
  ExplanationNode.@Nullable Rule explanation();

  /**
   * Domain or engine veto (not unbound inputs).
   *
   * @param vetoReason veto text; may be null
   * @param ruleType rule type
   * @param explanation explanation or null
   */
  record Veto(
      @Nullable String vetoReason,
      String ruleType,
      ExplanationNode.@Nullable Rule explanation)
      implements RuleResult {}

  /**
   * Rule result is a MissingData veto; {@code missingData} lists unbound keys.
   *
   * @param missingData unbound data keys in evaluation order
   * @param ruleType rule type
   * @param explanation explanation or null
   */
  record MissingData(
      List<String> missingData,
      String ruleType,
      ExplanationNode.@Nullable Rule explanation)
      implements RuleResult {}

  /**
   * Number result.
   *
   * @param result engine display string
   * @param number magnitude
   * @param ruleType rule type
   * @param explanation explanation or null
   */
  record Number(
      String result,
      BigDecimal number,
      String ruleType,
      ExplanationNode.@Nullable Rule explanation)
      implements RuleResult {}

  /**
   * Text result.
   *
   * @param result engine display string
   * @param text text value
   * @param ruleType rule type
   * @param explanation explanation or null
   */
  record Text(
      String result,
      String text,
      String ruleType,
      ExplanationNode.@Nullable Rule explanation)
      implements RuleResult {}

  /**
   * Boolean result.
   *
   * @param result engine display string
   * @param booleanValue boolean value
   * @param ruleType rule type
   * @param explanation explanation or null
   */
  record BooleanValue(
      String result,
      boolean booleanValue,
      String ruleType,
      ExplanationNode.@Nullable Rule explanation)
      implements RuleResult {}

  /**
   * Date result.
   *
   * @param result engine display string
   * @param date calendar date
   * @param ruleType rule type
   * @param explanation explanation or null
   */
  record Date(
      String result,
      LocalDate date,
      String ruleType,
      ExplanationNode.@Nullable Rule explanation)
      implements RuleResult {}

  /**
   * Time result.
   *
   * @param result engine display string
   * @param time time of day
   * @param ruleType rule type
   * @param explanation explanation or null
   */
  record Time(
      String result,
      LocalTime time,
      String ruleType,
      ExplanationNode.@Nullable Rule explanation)
      implements RuleResult {}

  /**
   * Measure result (unit name to magnitude).
   *
   * @param result engine display string
   * @param measure unit map
   * @param ruleType rule type
   * @param explanation explanation or null
   */
  record Measure(
      String result,
      Map<String, BigDecimal> measure,
      String ruleType,
      ExplanationNode.@Nullable Rule explanation)
      implements RuleResult {}

  /**
   * Ratio result (unit name to magnitude).
   *
   * @param result engine display string
   * @param ratio unit map
   * @param ruleType rule type
   * @param explanation explanation or null
   */
  record Ratio(
      String result,
      Map<String, BigDecimal> ratio,
      String ruleType,
      ExplanationNode.@Nullable Rule explanation)
      implements RuleResult {}

  /**
   * Calendar measure result.
   *
   * @param result engine display string
   * @param calendar calendar value
   * @param ruleType rule type
   * @param explanation explanation or null
   */
  record Calendar(
      String result,
      RuleResultValue.CalendarResult calendar,
      String ruleType,
      ExplanationNode.@Nullable Rule explanation)
      implements RuleResult {}

  /**
   * Range result.
   *
   * @param result engine display string
   * @param range endpoints
   * @param ruleType rule type
   * @param explanation explanation or null
   */
  record Range(
      String result,
      RuleResultValue.RangeResult range,
      String ruleType,
      ExplanationNode.@Nullable Rule explanation)
      implements RuleResult {}

  /**
   * One rule inside a spec instance.
   *
   * @param vetoed whether the rule vetoed
   * @param vetoReason veto text or null
   * @param ruleType rule type
   * @param result rule value, or null when absent
   */
  record SpecRuleResult(
      boolean vetoed,
      @Nullable String vetoReason,
      String ruleType,
      @Nullable RuleResultValue result) {
    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    static SpecRuleResult read(JsonParser p) throws IOException {
      JsonReading.expectStartObject(p, "SpecRuleResult");
      Boolean vetoed = null;
      String vetoReason = null;
      String ruleType = null;
      RuleResultValue result = null;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "vetoed" -> vetoed = JsonReading.readBoolean(p);
          case "veto_reason" -> vetoReason = JsonReading.readString(p);
          case "rule_type" -> ruleType = JsonReading.readString(p);
          case "result" -> {
            if (p.currentToken() == JsonToken.VALUE_NULL) {
              result = null;
            } else {
              result = RuleResultValue.read(p);
            }
          }
          default -> JsonReading.unknownField(field, "SpecRuleResult");
        }
      }
      if (vetoed == null) {
        JsonReading.missingRequired("vetoed", "SpecRuleResult");
      }
      if (ruleType == null) {
        JsonReading.missingRequired("rule_type", "SpecRuleResult");
      }
      return new SpecRuleResult(vetoed, vetoReason, ruleType, result);
    }
  }

  /**
   * Spec instance carried by a spec-valued rule.
   *
   * @param repository repository or null
   * @param spec spec name
   * @param effective resolved row effective date
   * @param instance uses path
   * @param path one hop per {@code uses} from the evaluated spec to the instance; never empty
   * @param data evaluated data slots relative to the spec
   * @param rules rule results relative to the spec
   */
  record SpecResult(
      @Nullable String repository,
      String spec,
      String effective,
      String instance,
      List<Show.PathSegment> path,
      Map<String, SpecRuleResult> data,
      Map<String, SpecRuleResult> rules) {
    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    static SpecResult read(JsonParser p) throws IOException {
      JsonReading.expectStartObject(p, "SpecResult");
      String repository = null;
      String spec = null;
      String effective = null;
      String instance = null;
      List<Show.PathSegment> path = null;
      Map<String, SpecRuleResult> data = null;
      Map<String, SpecRuleResult> rules = null;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "repository" -> repository = JsonReading.readString(p);
          case "spec" -> spec = JsonReading.readString(p);
          case "effective" -> effective = JsonReading.readString(p);
          case "instance" -> instance = JsonReading.readString(p);
          case "path" -> path = JsonReading.readList(p, Show.PathSegment::read);
          case "data" -> data = JsonReading.readMap(p, SpecRuleResult::read);
          case "rules" -> rules = JsonReading.readMap(p, SpecRuleResult::read);
          default -> JsonReading.unknownField(field, "SpecResult");
        }
      }
      if (spec == null) {
        JsonReading.missingRequired("spec", "SpecResult");
      }
      if (effective == null) {
        JsonReading.missingRequired("effective", "SpecResult");
      }
      if (instance == null) {
        JsonReading.missingRequired("instance", "SpecResult");
      }
      if (path == null || path.isEmpty()) {
        JsonReading.missingRequired("path", "SpecResult");
      }
      if (data == null) {
        JsonReading.missingRequired("data", "SpecResult");
      }
      if (rules == null) {
        JsonReading.missingRequired("rules", "SpecResult");
      }
      return new SpecResult(repository, spec, effective, instance, List.copyOf(path), data, rules);
    }
  }

  /**
   * List row carried by a row-valued result: an index into {@link Response#rows()}.
   *
   * @param repository repository or null
   * @param spec row spec name
   * @param effective resolved row effective date, empty for the origin row
   * @param instance row path relative to its holder, e.g. {@code lines.2}
   * @param index position in {@link Response#rows()}
   */
  record RowReference(
      @Nullable String repository, String spec, String effective, String instance, int index) {
    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    static RowReference read(JsonParser p) throws IOException {
      JsonReading.expectStartObject(p, "RowReference");
      String repository = null;
      String spec = null;
      String effective = null;
      String instance = null;
      Integer index = null;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "repository" -> repository = JsonReading.readString(p);
          case "spec" -> spec = JsonReading.readString(p);
          case "effective" -> effective = JsonReading.readString(p);
          case "instance" -> instance = JsonReading.readString(p);
          case "index" -> index = JsonReading.readInt(p);
          default -> JsonReading.unknownField(field, "RowReference");
        }
      }
      if (spec == null) {
        JsonReading.missingRequired("spec", "RowReference");
      }
      if (effective == null) {
        JsonReading.missingRequired("effective", "RowReference");
      }
      if (instance == null) {
        JsonReading.missingRequired("instance", "RowReference");
      }
      if (index == null) {
        JsonReading.missingRequired("index", "RowReference");
      }
      return new RowReference(repository, spec, effective, instance, index);
    }
  }

  /**
   * One list row reached by the results, filled once.
   *
   * @param repository repository or null
   * @param spec row spec name
   * @param effective resolved row effective date, empty for the origin row
   * @param data evaluated row data
   * @param rules evaluated row rules
   */
  record RowResult(
      @Nullable String repository,
      String spec,
      String effective,
      Map<String, SpecRuleResult> data,
      Map<String, SpecRuleResult> rules) {
    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    static RowResult read(JsonParser p) throws IOException {
      JsonReading.expectStartObject(p, "RowResult");
      String repository = null;
      String spec = null;
      String effective = null;
      Map<String, SpecRuleResult> data = null;
      Map<String, SpecRuleResult> rules = null;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "repository" -> repository = JsonReading.readString(p);
          case "spec" -> spec = JsonReading.readString(p);
          case "effective" -> effective = JsonReading.readString(p);
          case "data" -> data = JsonReading.readMap(p, SpecRuleResult::read);
          case "rules" -> rules = JsonReading.readMap(p, SpecRuleResult::read);
          default -> JsonReading.unknownField(field, "RowResult");
        }
      }
      if (spec == null) {
        JsonReading.missingRequired("spec", "RowResult");
      }
      if (effective == null) {
        JsonReading.missingRequired("effective", "RowResult");
      }
      if (data == null) {
        JsonReading.missingRequired("data", "RowResult");
      }
      if (rules == null) {
        JsonReading.missingRequired("rules", "RowResult");
      }
      return new RowResult(repository, spec, effective, data, rules);
    }
  }

  /**
   * Row-valued rule result.
   *
   * @param result display of the row path
   * @param row the row reference
   * @param ruleType rule type
   * @param explanation explanation or null
   */
  record Row(
      String result,
      RowReference row,
      String ruleType,
      ExplanationNode.@Nullable Rule explanation)
      implements RuleResult {}

  /**
   * List-valued rule result.
   *
   * @param result engine display string
   * @param items items in list order; rows are {@link RuleResultValue.Row} values
   * @param ruleType rule type
   * @param explanation explanation or null
   */
  record ListResult(
      String result,
      List<RuleResultValue> items,
      String ruleType,
      ExplanationNode.@Nullable Rule explanation)
      implements RuleResult {}

  /**
   * Spec-valued rule result.
   *
   * @param result display of the instance path
   * @param spec the instance
   * @param ruleType rule type
   * @param explanation explanation or null
   */
  record Spec(
      String result,
      SpecResult spec,
      String ruleType,
      ExplanationNode.@Nullable Rule explanation)
      implements RuleResult {}

  /**
   * Parses JSON.
   *
   * @param p parser at value start
   * @return parsed value
   * @throws IOException if JSON IO fails
   */
  static RuleResult read(JsonParser p) throws IOException {
    JsonReading.expectStartObject(p, "RuleResult");
    String result = null;
    Map<String, BigDecimal> measure = null;
    Map<String, BigDecimal> ratio = null;
    BigDecimal number = null;
    Boolean booleanValue = null;
    String text = null;
    LocalDate date = null;
    LocalTime time = null;
    RuleResultValue.CalendarResult calendar = null;
    RuleResultValue.RangeResult range = null;
    SpecResult spec = null;
    RowReference row = null;
    List<RuleResultValue> items = null;
    Boolean vetoed = null;
    String vetoReason = null;
    String ruleType = null;
    List<String> missingData = null;
    ExplanationNode.Rule explanation = null;
    while (p.nextToken() != JsonToken.END_OBJECT) {
      String field = p.currentName();
      p.nextToken();
      switch (field) {
        case "result" -> result = JsonReading.readString(p);
        case "measure" -> measure = JsonReading.readMap(p, JsonReading::readDecimal);
        case "ratio" -> ratio = JsonReading.readMap(p, JsonReading::readDecimal);
        case "number" -> number = JsonReading.readDecimal(p);
        case "boolean" -> booleanValue = JsonReading.readBoolean(p);
        case "text" -> text = JsonReading.readString(p);
        case "date" -> date = JsonReading.readLocalDate(p);
        case "time" -> time = JsonReading.readLocalTime(p);
        case "calendar" -> calendar = RuleResultValue.CalendarResult.read(p);
        case "range" -> range = RuleResultValue.RangeResult.read(p);
        case "spec" -> spec = SpecResult.read(p);
        case "row" -> row = RowReference.read(p);
        case "list" -> items = JsonReading.readList(p, RuleResultValue::read);
        case "unit" -> JsonReading.readString(p);
        case "vetoed" -> vetoed = JsonReading.readBoolean(p);
        case "veto_reason" -> vetoReason = JsonReading.readString(p);
        case "rule_type" -> ruleType = JsonReading.readString(p);
        case "missing_data" -> missingData = JsonReading.readList(p, JsonReading::readString);
        case "explanation" -> explanation = ExplanationNode.Rule.read(p);
        default -> JsonReading.unknownField(field, "RuleResult");
      }
    }
    if (vetoed == null) {
      JsonReading.missingRequired("vetoed", "RuleResult");
    }
    if (ruleType == null) {
      JsonReading.missingRequired("rule_type", "RuleResult");
    }
    if (missingData != null && !missingData.isEmpty()) {
      return new MissingData(List.copyOf(missingData), ruleType, explanation);
    }
    if (vetoed) {
      return new Veto(vetoReason, ruleType, explanation);
    }
    if (result == null) {
      JsonReading.missingRequired("result", "RuleResult");
    }
    if (number != null) {
      return new Number(result, number, ruleType, explanation);
    }
    if (text != null) {
      return new Text(result, text, ruleType, explanation);
    }
    if (booleanValue != null) {
      return new BooleanValue(result, booleanValue, ruleType, explanation);
    }
    if (date != null) {
      return new Date(result, date, ruleType, explanation);
    }
    if (time != null) {
      return new Time(result, time, ruleType, explanation);
    }
    if (measure != null) {
      return new Measure(result, measure, ruleType, explanation);
    }
    if (ratio != null) {
      return new Ratio(result, ratio, ruleType, explanation);
    }
    if (calendar != null) {
      return new Calendar(result, calendar, ruleType, explanation);
    }
    if (range != null) {
      return new Range(result, range, ruleType, explanation);
    }
    if (spec != null) {
      return new Spec(result, spec, ruleType, explanation);
    }
    if (row != null) {
      return new Row(result, row, ruleType, explanation);
    }
    if (items != null) {
      return new ListResult(result, List.copyOf(items), ruleType, explanation);
    }
    throw new LemmaBugError("BUG: non-veto RuleResult has no typed value field");
  }
}

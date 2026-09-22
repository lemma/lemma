package com.lemmabase.lemma.schema;

import com.lemmabase.lemma.JsonReading;
import com.lemmabase.lemma.LemmaBugError;
import com.lemmabase.lemma.RuleResultValue;

import com.fasterxml.jackson.core.JsonParser;
import com.fasterxml.jackson.core.JsonToken;
import java.io.IOException;
import java.math.BigDecimal;
import java.time.LocalDate;
import java.time.LocalTime;
import java.util.List;
import java.util.Map;
import org.jspecify.annotations.Nullable;

/** Nested explanation tree node (tagged by {@code type}). */
public sealed interface ExplanationNode {
  /**
   * Returns the node type tag.
   *
   * @return type tag
   */
  String type();
  /**
   * Cause.
   * @param condition condition
   * @param value value
   * @param children children
   */
  record Cause(String condition, String value, @Nullable List<ExplanationNode> children) {
    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    public static Cause read(JsonParser p) throws IOException {
      JsonReading.expectStartObject(p, "Cause");
      String condition = null;
      String value = null;
      List<ExplanationNode> children = null;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "condition" -> condition = JsonReading.readString(p);
          case "value" -> value = JsonReading.readString(p);
          case "children" -> children = JsonReading.readList(p, ExplanationNode::read);
          default -> JsonReading.unknownField(field, "Cause");
        }
      }
      if (condition == null) {
        JsonReading.missingRequired("condition", "Cause");
      }
      if (value == null) {
        JsonReading.missingRequired("value", "Cause");
      }
      return new Cause(condition, value, children);
    }
  }

  /**
   * ConversionStep.
   * @param role role
   * @param text text
   */
  record ConversionStep(String role, String text) {
    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    public static ConversionStep read(JsonParser p) throws IOException {
      JsonReading.expectStartObject(p, "ConversionStep");
      String role = null;
      String text = null;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "role" -> role = JsonReading.readString(p);
          case "text" -> text = JsonReading.readString(p);
          default -> JsonReading.unknownField(field, "ConversionStep");
        }
      }
      if (role == null) {
        JsonReading.missingRequired("role", "ConversionStep");
      }
      if (text == null) {
        JsonReading.missingRequired("text", "ConversionStep");
      }
      if (!("outcome".equals(role) || "rule".equals(role) || "source".equals(role))) {
        throw new LemmaBugError("BUG: invalid ConversionStep role '" + role + "'");
      }
      return new ConversionStep(role, text);
    }
  }

  /**
   * Rule.
   *
   * @param name name
   * @param result flattened RuleResultValue (one-liner plus typed maps)
   * @param body body
   * @param causes causes
   * @param children children
   */
  record Rule(
      String name,
      RuleResultValue result,
      String body,
      @Nullable List<Cause> causes,
      @Nullable List<ExplanationNode> children)
      implements ExplanationNode {
    /** {@inheritDoc} */
    @Override
    public String type() {
      return "rule";
    }

    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    public static Rule read(JsonParser p) throws IOException {
      JsonReading.expectStartObject(p, "ExplanationNode.Rule");
      String name = null;
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
      String body = null;
      List<Cause> causes = null;
      List<ExplanationNode> children = null;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "type" -> expectType(p, "rule");
          case "name" -> name = JsonReading.readString(p);
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
          case "body" -> body = JsonReading.readString(p);
          case "causes" -> causes = JsonReading.readList(p, Cause::read);
          case "children" -> children = JsonReading.readList(p, ExplanationNode::read);
          default -> JsonReading.unknownField(field, "ExplanationNode.Rule");
        }
      }
      if (name == null) {
        JsonReading.missingRequired("name", "ExplanationNode.Rule");
      }
      if (result == null) {
        JsonReading.missingRequired("result", "ExplanationNode.Rule");
      }
      if (body == null) {
        JsonReading.missingRequired("body", "ExplanationNode.Rule");
      }
      return new Rule(
          name,
          typedValue(
              result, measure, ratio, number, booleanValue, text, date, time, calendar, range),
          body,
          causes,
          children);
    }
  }

  /**
   * Compose.
   * @param expression expression
   * @param operator arithmetic operator when this compose is arithmetic; null otherwise
   * @param operands operands
   */
  record Compose(
      String expression, @Nullable String operator, List<ExplanationNode> operands)
      implements ExplanationNode {
    /** {@inheritDoc} */
    @Override
    public String type() {
      return "compose";
    }

    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    public static Compose read(JsonParser p) throws IOException {
      JsonReading.expectStartObject(p, "ExplanationNode.Compose");
      String expression = null;
      String operator = null;
      List<ExplanationNode> operands = null;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "type" -> expectType(p, "compose");
          case "expression" -> expression = JsonReading.readString(p);
          case "operator" -> operator = JsonReading.readString(p);
          case "operands" -> operands = JsonReading.readList(p, ExplanationNode::read);
          default -> JsonReading.unknownField(field, "ExplanationNode.Compose");
        }
      }
      if (expression == null) {
        JsonReading.missingRequired("expression", "ExplanationNode.Compose");
      }
      if (operands == null) {
        JsonReading.missingRequired("operands", "ExplanationNode.Compose");
      }
      if (operator != null
          && !(operator.equals("add")
              || operator.equals("subtract")
              || operator.equals("multiply")
              || operator.equals("divide")
              || operator.equals("modulo")
              || operator.equals("power"))) {
        throw new LemmaBugError("BUG: invalid Compose operator '" + operator + "'");
      }
      return new Compose(expression, operator, operands);
    }
  }

  /**
   * Data.
   *
   * @param name name
   * @param result flattened RuleResultValue (one-liner plus typed maps)
   */
  record Data(String name, RuleResultValue result) implements ExplanationNode {
    /** {@inheritDoc} */
    @Override
    public String type() {
      return "data";
    }

    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    public static Data read(JsonParser p) throws IOException {
      JsonReading.expectStartObject(p, "ExplanationNode.Data");
      String name = null;
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
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "type" -> expectType(p, "data");
          case "name" -> name = JsonReading.readString(p);
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
          default -> JsonReading.unknownField(field, "ExplanationNode.Data");
        }
      }
      if (name == null) {
        JsonReading.missingRequired("name", "ExplanationNode.Data");
      }
      if (result == null) {
        JsonReading.missingRequired("result", "ExplanationNode.Data");
      }
      return new Data(
          name,
          typedValue(
              result, measure, ratio, number, booleanValue, text, date, time, calendar, range));
    }
  }

  /**
   * DataUnused.
   * @param name name
   */
  record DataUnused(String name) implements ExplanationNode {
    /** {@inheritDoc} */
    @Override
    public String type() {
      return "data_unused";
    }

    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    public static DataUnused read(JsonParser p) throws IOException {
      JsonReading.expectStartObject(p, "ExplanationNode.DataUnused");
      String name = null;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "type" -> expectType(p, "data_unused");
          case "name" -> name = JsonReading.readString(p);
          default -> JsonReading.unknownField(field, "ExplanationNode.DataUnused");
        }
      }
      if (name == null) {
        JsonReading.missingRequired("name", "ExplanationNode.DataUnused");
      }
      return new DataUnused(name);
    }
  }

  /**
   * Conversion.
   * @param expression expression
   * @param steps steps
   * @param operands operands
   */
  record Conversion(
      String expression, List<ConversionStep> steps, List<ExplanationNode> operands)
      implements ExplanationNode {
    /** {@inheritDoc} */
    @Override
    public String type() {
      return "conversion";
    }

    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    public static Conversion read(JsonParser p) throws IOException {
      JsonReading.expectStartObject(p, "ExplanationNode.Conversion");
      String expression = null;
      List<ConversionStep> steps = null;
      List<ExplanationNode> operands = null;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "type" -> expectType(p, "conversion");
          case "expression" -> expression = JsonReading.readString(p);
          case "steps" -> steps = JsonReading.readList(p, ConversionStep::read);
          case "operands" -> operands = JsonReading.readList(p, ExplanationNode::read);
          default -> JsonReading.unknownField(field, "ExplanationNode.Conversion");
        }
      }
      if (expression == null) {
        JsonReading.missingRequired("expression", "ExplanationNode.Conversion");
      }
      if (steps == null) {
        JsonReading.missingRequired("steps", "ExplanationNode.Conversion");
      }
      if (operands == null) {
        JsonReading.missingRequired("operands", "ExplanationNode.Conversion");
      }
      return new Conversion(expression, steps, operands);
    }
  }

  /**
   * Veto.
   * @param message message
   */
  record Veto(@Nullable String message) implements ExplanationNode {
    /** {@inheritDoc} */
    @Override
    public String type() {
      return "veto";
    }

    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    public static Veto read(JsonParser p) throws IOException {
      JsonReading.expectStartObject(p, "ExplanationNode.Veto");
      String message = null;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "type" -> expectType(p, "veto");
          case "message" -> message = JsonReading.readString(p);
          default -> JsonReading.unknownField(field, "ExplanationNode.Veto");
        }
      }
      return new Veto(message);
    }
  }

  private static RuleResultValue typedValue(
      String result,
      @Nullable Map<String, BigDecimal> measure,
      @Nullable Map<String, BigDecimal> ratio,
      @Nullable BigDecimal number,
      @Nullable Boolean booleanValue,
      @Nullable String text,
      @Nullable LocalDate date,
      @Nullable LocalTime time,
      RuleResultValue.@Nullable CalendarResult calendar,
      RuleResultValue.@Nullable RangeResult range) {
    if (number != null) {
      return new RuleResultValue.Number(result, number);
    }
    if (text != null) {
      return new RuleResultValue.Text(result, text);
    }
    if (booleanValue != null) {
      return new RuleResultValue.BooleanValue(result, booleanValue);
    }
    if (date != null) {
      return new RuleResultValue.Date(result, date);
    }
    if (time != null) {
      return new RuleResultValue.Time(result, time);
    }
    if (measure != null) {
      return new RuleResultValue.Measure(result, measure);
    }
    if (ratio != null) {
      return new RuleResultValue.Ratio(result, ratio);
    }
    if (calendar != null) {
      return new RuleResultValue.Calendar(result, calendar);
    }
    if (range != null) {
      return new RuleResultValue.Range(result, range);
    }
    return new RuleResultValue.ResultOnly(result);
  }

  private static void expectType(JsonParser p, String expected) throws IOException {
    String type = JsonReading.readString(p);
    if (!expected.equals(type)) {
      throw new LemmaBugError("BUG: expected type '" + expected + "', got '" + type + "'");
    }
  }

  /**
   * Parses JSON.
   *
   * @param p parser at value start
   * @return parsed value
   * @throws IOException if JSON IO fails
   */
  public static ExplanationNode read(JsonParser p) throws IOException {
    JsonReading.expectStartObject(p, "ExplanationNode");
    String json = JsonReading.bufferObjectAsString(p);
    String type = JsonReading.findTag(json, "type");
    if (type == null) {
      throw new LemmaBugError("BUG: missing 'type' in ExplanationNode");
    }
    try (JsonParser reader = JsonReading.parserFor(json)) {
      return switch (type) {
        case "rule" -> Rule.read(reader);
        case "compose" -> Compose.read(reader);
        case "data" -> Data.read(reader);
        case "data_unused" -> DataUnused.read(reader);
        case "conversion" -> Conversion.read(reader);
        case "veto" -> Veto.read(reader);
        default -> throw new LemmaBugError("BUG: unknown type value: " + type);
      };
    }
  }
}

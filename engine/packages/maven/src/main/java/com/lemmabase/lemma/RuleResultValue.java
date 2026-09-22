package com.lemmabase.lemma;

import com.fasterxml.jackson.core.JsonParser;
import com.fasterxml.jackson.core.JsonToken;
import java.io.IOException;
import java.math.BigDecimal;
import java.time.LocalDate;
import java.time.LocalTime;
import java.util.Map;
import org.jspecify.annotations.Nullable;

/**
 * Typed value shared by show fill/suggestion. Pattern-match on the variant.
 */
public sealed interface RuleResultValue {
  /**
   * Engine result string when present.
   *
   * @return result or null
   */
  @Nullable
  String result();

  /**
   * Calendar value (measure whose unit is a calendar unit).
   *
   * @param value magnitude
   * @param unit calendar unit name
   */
  record CalendarResult(BigDecimal value, String unit) {
    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    public static CalendarResult read(JsonParser p) throws IOException {
      JsonReading.expectStartObject(p, "CalendarResult");
      BigDecimal value = null;
      String unit = null;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "value" -> value = JsonReading.readDecimal(p);
          case "unit" -> unit = JsonReading.readString(p);
          default -> JsonReading.unknownField(field, "CalendarResult");
        }
      }
      if (value == null) {
        JsonReading.missingRequired("value", "CalendarResult");
      }
      if (unit == null) {
        JsonReading.missingRequired("unit", "CalendarResult");
      }
      return new CalendarResult(value, unit);
    }
  }

  /**
   * Non-range endpoint of a range value.
   */
  sealed interface Endpoint {
    /**
     * Engine result string when present.
     *
     * @return result or null
     */
    @Nullable
    String result();

    /**
     * Number endpoint.
     *
     * @param result display or null
     * @param number magnitude
     */
    record Number(@Nullable String result, BigDecimal number) implements Endpoint {}

    /**
     * Text endpoint.
     *
     * @param result display or null
     * @param text text value
     */
    record Text(@Nullable String result, String text) implements Endpoint {}

    /**
     * Boolean endpoint.
     *
     * @param result display or null
     * @param booleanValue boolean value
     */
    record BooleanValue(@Nullable String result, boolean booleanValue) implements Endpoint {}

    /**
     * Date endpoint.
     *
     * @param result display or null
     * @param date calendar date
     */
    record Date(@Nullable String result, LocalDate date) implements Endpoint {}

    /**
     * Time endpoint.
     *
     * @param result display or null
     * @param time time of day
     */
    record Time(@Nullable String result, LocalTime time) implements Endpoint {}

    /**
     * Measure endpoint.
     *
     * @param result display or null
     * @param measure unit map
     */
    record Measure(@Nullable String result, Map<String, BigDecimal> measure) implements Endpoint {}

    /**
     * Ratio endpoint.
     *
     * @param result display or null
     * @param ratio unit map
     */
    record Ratio(@Nullable String result, Map<String, BigDecimal> ratio) implements Endpoint {}

    /**
     * Calendar endpoint.
     *
     * @param result display or null
     * @param calendar calendar value
     */
    record Calendar(@Nullable String result, CalendarResult calendar) implements Endpoint {}

    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    static Endpoint read(JsonParser p) throws IOException {
      JsonReading.expectStartObject(p, "RuleResultValueEndpoint");
      String result = null;
      Map<String, BigDecimal> measure = null;
      Map<String, BigDecimal> ratio = null;
      BigDecimal number = null;
      Boolean booleanValue = null;
      String text = null;
      LocalDate date = null;
      LocalTime time = null;
      CalendarResult calendar = null;
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
          case "calendar" -> calendar = CalendarResult.read(p);
          default -> JsonReading.unknownField(field, "RuleResultValueEndpoint");
        }
      }
      if (number != null) {
        return new Number(result, number);
      }
      if (text != null) {
        return new Text(result, text);
      }
      if (booleanValue != null) {
        return new BooleanValue(result, booleanValue);
      }
      if (date != null) {
        return new Date(result, date);
      }
      if (time != null) {
        return new Time(result, time);
      }
      if (measure != null) {
        return new Measure(result, measure);
      }
      if (ratio != null) {
        return new Ratio(result, ratio);
      }
      if (calendar != null) {
        return new Calendar(result, calendar);
      }
      throw new LemmaBugError("BUG: RuleResultValueEndpoint has no typed value field");
    }
  }

  /**
   * Range endpoints.
   *
   * @param from start
   * @param to end
   */
  record RangeResult(Endpoint from, Endpoint to) {
    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    public static RangeResult read(JsonParser p) throws IOException {
      JsonReading.expectStartObject(p, "RangeResult");
      Endpoint from = null;
      Endpoint to = null;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "from" -> from = Endpoint.read(p);
          case "to" -> to = Endpoint.read(p);
          default -> JsonReading.unknownField(field, "RangeResult");
        }
      }
      if (from == null) {
        JsonReading.missingRequired("from", "RangeResult");
      }
      if (to == null) {
        JsonReading.missingRequired("to", "RangeResult");
      }
      return new RangeResult(from, to);
    }
  }

  /**
   * Number value.
   *
   * @param result display or null
   * @param number magnitude
   */
  record Number(@Nullable String result, BigDecimal number) implements RuleResultValue {}

  /**
   * Text value.
   *
   * @param result display or null
   * @param text text value
   */
  record Text(@Nullable String result, String text) implements RuleResultValue {}

  /**
   * Boolean value.
   *
   * @param result display or null
   * @param booleanValue boolean value
   */
  record BooleanValue(@Nullable String result, boolean booleanValue) implements RuleResultValue {}

  /**
   * Date value.
   *
   * @param result display or null
   * @param date calendar date
   */
  record Date(@Nullable String result, LocalDate date) implements RuleResultValue {}

  /**
   * Time value.
   *
   * @param result display or null
   * @param time time of day
   */
  record Time(@Nullable String result, LocalTime time) implements RuleResultValue {}

  /**
   * Measure value.
   *
   * @param result display or null
   * @param measure unit map
   */
  record Measure(@Nullable String result, Map<String, BigDecimal> measure)
      implements RuleResultValue {}

  /**
   * Ratio value.
   *
   * @param result display or null
   * @param ratio unit map
   */
  record Ratio(@Nullable String result, Map<String, BigDecimal> ratio) implements RuleResultValue {}

  /**
   * Calendar value.
   *
   * @param result display or null
   * @param calendar calendar value
   */
  record Calendar(@Nullable String result, CalendarResult calendar) implements RuleResultValue {}

  /**
   * Range value.
   *
   * @param result display or null
   * @param range endpoints
   */
  record Range(@Nullable String result, RangeResult range) implements RuleResultValue {}

  /**
   * One-liner only (explanation veto text; no typed magnitude map).
   *
   * @param result one-liner
   */
  record ResultOnly(@Nullable String result) implements RuleResultValue {}

  /**
   * Parses JSON.
   *
   * @param p parser at value start
   * @return parsed value
   * @throws IOException if JSON IO fails
   */
  static RuleResultValue read(JsonParser p) throws IOException {
    JsonReading.expectStartObject(p, "RuleResultValue");
    String result = null;
    Map<String, BigDecimal> measure = null;
    Map<String, BigDecimal> ratio = null;
    BigDecimal number = null;
    Boolean booleanValue = null;
    String text = null;
    LocalDate date = null;
    LocalTime time = null;
    CalendarResult calendar = null;
    RangeResult range = null;
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
        case "calendar" -> calendar = CalendarResult.read(p);
        case "range" -> range = RangeResult.read(p);
        default -> JsonReading.unknownField(field, "RuleResultValue");
      }
    }
    if (number != null) {
      return new Number(result, number);
    }
    if (text != null) {
      return new Text(result, text);
    }
    if (booleanValue != null) {
      return new BooleanValue(result, booleanValue);
    }
    if (date != null) {
      return new Date(result, date);
    }
    if (time != null) {
      return new Time(result, time);
    }
    if (measure != null) {
      return new Measure(result, measure);
    }
    if (ratio != null) {
      return new Ratio(result, ratio);
    }
    if (calendar != null) {
      return new Calendar(result, calendar);
    }
    if (range != null) {
      return new Range(result, range);
    }
    throw new LemmaBugError("BUG: RuleResultValue has no typed value field");
  }
}

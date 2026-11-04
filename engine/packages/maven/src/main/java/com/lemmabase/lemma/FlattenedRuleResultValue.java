package com.lemmabase.lemma;

import com.lemmabase.lemma.RuleResult;
import com.lemmabase.lemma.RuleResultValue;
import com.lemmabase.lemma.JsonReading;
import com.fasterxml.jackson.core.JsonParser;
import com.fasterxml.jackson.core.JsonToken;
import java.io.IOException;
import java.math.BigDecimal;
import java.time.LocalDate;
import java.time.LocalTime;
import java.util.List;
import java.util.Map;
import org.jspecify.annotations.Nullable;

/**
 * Accumulates flattened {@link RuleResultValue} fields while the enclosing
 * {@link com.lemmabase.lemma.schema.ExplanationNode} reads its own fields, then builds the value.
 * Used by {@code Compose}, {@code ListCommand}, and {@code Conversion},
 * whose JSON flattens {@code RuleResultValue} into the node.
 */
public final class FlattenedRuleResultValue {
  /** Creates an empty accumulator. */
  public FlattenedRuleResultValue() {}

  private @Nullable String result;
  private @Nullable Map<String, BigDecimal> measure;
  private @Nullable Map<String, BigDecimal> ratio;
  private @Nullable BigDecimal number;
  private @Nullable Boolean booleanValue;
  private @Nullable String text;
  private @Nullable LocalDate date;
  private @Nullable LocalTime time;
  private RuleResultValue.@Nullable CalendarResult calendar;
  private RuleResultValue.@Nullable RangeResult range;
  private @Nullable String unit;
  private RuleResult.@Nullable SpecResult specResult;
  private RuleResult.@Nullable RowReference row;
  private @Nullable List<RuleResultValue> items;
  private @Nullable Map<String, RuleResultValue> fields;
  private @Nullable String reference;

  /**
   * Reads one flattened field. Returns true if the field was consumed.
   *
   * @param p parser at value start
   * @param field field name
   * @return true if the field was a RuleResultValue field
   * @throws IOException if JSON IO fails
   */
  public boolean readField(JsonParser p, String field) throws IOException {
    switch (field) {
      case "result" -> {
        result = JsonReading.readString(p);
        return true;
      }
      case "measure" -> {
        measure = JsonReading.readMap(p, JsonReading::readDecimal);
        return true;
      }
      case "ratio" -> {
        ratio = JsonReading.readMap(p, JsonReading::readDecimal);
        return true;
      }
      case "number" -> {
        number = JsonReading.readDecimal(p);
        return true;
      }
      case "boolean" -> {
        booleanValue = JsonReading.readBoolean(p);
        return true;
      }
      case "text" -> {
        text = JsonReading.readString(p);
        return true;
      }
      case "date" -> {
        date = JsonReading.readLocalDate(p);
        return true;
      }
      case "time" -> {
        time = JsonReading.readLocalTime(p);
        return true;
      }
      case "calendar" -> {
        calendar = RuleResultValue.CalendarResult.read(p);
        return true;
      }
      case "range" -> {
        range = RuleResultValue.RangeResult.read(p);
        return true;
      }
      case "unit" -> {
        unit = JsonReading.readString(p);
        return true;
      }
      case "spec" -> {
        specResult = RuleResult.SpecResult.read(p);
        return true;
      }
      case "row" -> {
        row = RuleResult.RowReference.read(p);
        return true;
      }
      case "list" -> {
        items = JsonReading.readList(p, RuleResultValue::read);
        return true;
      }
      case "fields" -> {
        fields = JsonReading.readMap(p, RuleResultValue::read);
        return true;
      }
      case "reference" -> {
        reference = JsonReading.readString(p);
        return true;
      }
      default -> {
        return false;
      }
    }
  }

  /**
   * Builds the {@link RuleResultValue} from the accumulated fields.
   *
   * @return the constructed value, or {@code ResultOnly(null)} when no typed field was present
   */
  public RuleResultValue build() {
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
      return new RuleResultValue.Measure(result, measure, unit);
    }
    if (ratio != null) {
      return new RuleResultValue.Ratio(result, ratio, unit);
    }
    if (calendar != null) {
      return new RuleResultValue.Calendar(result, calendar);
    }
    if (range != null) {
      return new RuleResultValue.Range(result, range);
    }
    if (specResult != null) {
      return new RuleResultValue.Spec(result, specResult);
    }
    if (row != null) {
      return new RuleResultValue.Row(result, row);
    }
    if (items != null) {
      return new RuleResultValue.ListValue(result, List.copyOf(items));
    }
    if (fields != null) {
      if (result != null) {
        throw new com.lemmabase.lemma.LemmaBugError("BUG: show source row carries a result");
      }
      return new RuleResultValue.SourceRow(java.util.Collections.unmodifiableMap(fields));
    }
    if (reference != null) {
      if (result != null) {
        throw new com.lemmabase.lemma.LemmaBugError("BUG: show reference field carries a result");
      }
      return new RuleResultValue.Reference(reference);
    }
    return new RuleResultValue.ResultOnly(result);
  }
}

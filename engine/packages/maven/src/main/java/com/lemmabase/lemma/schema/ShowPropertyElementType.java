package com.lemmabase.lemma.schema;

import com.lemmabase.lemma.JsonReading;
import com.lemmabase.lemma.LemmaBugError;

import com.fasterxml.jackson.core.JsonParser;
import com.fasterxml.jackson.core.JsonToken;
import java.io.IOException;
import java.math.BigDecimal;
import java.util.List;
import java.util.Map;
import org.jspecify.annotations.Nullable;

/**
 * Element of a list {@link ShowPropertyType}: type constraints without help or text options/length.
 */
public sealed interface ShowPropertyElementType
    permits ShowPropertyElementType.BooleanType,
        ShowPropertyElementType.Measure,
        ShowPropertyElementType.NumberType,
        ShowPropertyElementType.NumberRange,
        ShowPropertyElementType.Ratio,
        ShowPropertyElementType.RatioRange,
        ShowPropertyElementType.Text,
        ShowPropertyElementType.DateType,
        ShowPropertyElementType.DateRange,
        ShowPropertyElementType.TimeType,
        ShowPropertyElementType.TimeRange,
        ShowPropertyElementType.MeasureRange,
        ShowPropertyElementType.Spec {
  /**
   * Kind tag for this type.
   *
   * @return kind tag
   */
  String kind();

  private static void expectKind(JsonParser p, String expected) throws IOException {
    String kind = JsonReading.readString(p);
    if (!expected.equals(kind)) {
      throw new LemmaBugError("BUG: expected kind '" + expected + "', got '" + kind + "'");
    }
  }

  private static @Nullable BigDecimal readNullableDecimal(JsonParser p) throws IOException {
    if (p.currentToken() == JsonToken.VALUE_NULL) {
      return null;
    }
    return JsonReading.readDecimal(p);
  }

  /** Boolean element. */
  record BooleanType() implements ShowPropertyElementType {
    @Override
    public String kind() {
      return "boolean";
    }

    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    public static BooleanType read(JsonParser p) throws IOException {
      JsonReading.expectStartObject(p, "ShowPropertyElementType.Boolean");
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "kind" -> expectKind(p, "boolean");
          default -> JsonReading.unknownField(field, "ShowPropertyElementType.Boolean");
        }
      }
      return new BooleanType();
    }
  }

  /**
   * Measure element.
   *
   * @param minimum minimum
   * @param maximum maximum
   * @param decimals decimals
   * @param units units
   * @param traits traits
   * @param decomposition decomposition
   * @param unit written unit when describing a value, or null
   */
  record Measure(
      LemmaType.@Nullable NamedBound minimum,
      LemmaType.@Nullable NamedBound maximum,
      @Nullable Integer decimals,
      List<LemmaType.MeasureUnit> units,
      List<String> traits,
      @Nullable Map<String, Integer> decomposition,
      @Nullable String unit)
      implements ShowPropertyElementType {
    @Override
    public String kind() {
      return "measure";
    }

    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    public static Measure read(JsonParser p) throws IOException {
      JsonReading.expectStartObject(p, "ShowPropertyElementType.Measure");
      LemmaType.NamedBound minimum = null;
      boolean minimumSeen = false;
      LemmaType.NamedBound maximum = null;
      boolean maximumSeen = false;
      Integer decimals = null;
      boolean decimalsSeen = false;
      List<LemmaType.MeasureUnit> units = null;
      List<String> traits = null;
      Map<String, Integer> decomposition = null;
      boolean decompositionSeen = false;
      String unit = null;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "kind" -> expectKind(p, "measure");
          case "minimum" -> {
            minimumSeen = true;
            minimum = LemmaType.NamedBound.readNullable(p);
          }
          case "maximum" -> {
            maximumSeen = true;
            maximum = LemmaType.NamedBound.readNullable(p);
          }
          case "decimals" -> {
            decimalsSeen = true;
            decimals = JsonReading.readInt(p);
          }
          case "units" -> units = JsonReading.readList(p, LemmaType.MeasureUnit::read);
          case "traits" -> traits = JsonReading.readList(p, JsonReading::readString);
          case "decomposition" -> {
            decompositionSeen = true;
            decomposition =
                p.currentToken() == JsonToken.VALUE_NULL
                    ? null
                    : JsonReading.readMap(p, JsonReading::readInt);
          }
          case "unit" -> unit = JsonReading.readString(p);
          default -> JsonReading.unknownField(field, "ShowPropertyElementType.Measure");
        }
      }
      if (!minimumSeen) {
        JsonReading.missingRequired("minimum", "ShowPropertyElementType.Measure");
      }
      if (!maximumSeen) {
        JsonReading.missingRequired("maximum", "ShowPropertyElementType.Measure");
      }
      if (!decimalsSeen) {
        JsonReading.missingRequired("decimals", "ShowPropertyElementType.Measure");
      }
      if (units == null) {
        JsonReading.missingRequired("units", "ShowPropertyElementType.Measure");
      }
      if (traits == null) {
        JsonReading.missingRequired("traits", "ShowPropertyElementType.Measure");
      }
      if (!decompositionSeen) {
        JsonReading.missingRequired("decomposition", "ShowPropertyElementType.Measure");
      }
      return new Measure(minimum, maximum, decimals, units, traits, decomposition, unit);
    }
  }

  /**
   * Number element.
   *
   * @param minimum minimum
   * @param maximum maximum
   * @param decimals decimals
   */
  record NumberType(
      @Nullable BigDecimal minimum,
      @Nullable BigDecimal maximum,
      @Nullable Integer decimals)
      implements ShowPropertyElementType {
    @Override
    public String kind() {
      return "number";
    }

    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    public static NumberType read(JsonParser p) throws IOException {
      JsonReading.expectStartObject(p, "ShowPropertyElementType.Number");
      BigDecimal minimum = null;
      boolean minimumSeen = false;
      BigDecimal maximum = null;
      boolean maximumSeen = false;
      Integer decimals = null;
      boolean decimalsSeen = false;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "kind" -> expectKind(p, "number");
          case "minimum" -> {
            minimumSeen = true;
            minimum = readNullableDecimal(p);
          }
          case "maximum" -> {
            maximumSeen = true;
            maximum = readNullableDecimal(p);
          }
          case "decimals" -> {
            decimalsSeen = true;
            decimals = JsonReading.readInt(p);
          }
          default -> JsonReading.unknownField(field, "ShowPropertyElementType.Number");
        }
      }
      if (!minimumSeen) {
        JsonReading.missingRequired("minimum", "ShowPropertyElementType.Number");
      }
      if (!maximumSeen) {
        JsonReading.missingRequired("maximum", "ShowPropertyElementType.Number");
      }
      if (!decimalsSeen) {
        JsonReading.missingRequired("decimals", "ShowPropertyElementType.Number");
      }
      return new NumberType(minimum, maximum, decimals);
    }
  }

  /**
   * Number range element.
   *
   * @param lower lower
   * @param upper upper
   * @param minimum minimum
   * @param maximum maximum
   */
  record NumberRange(
      @Nullable BigDecimal lower,
      @Nullable BigDecimal upper,
      @Nullable BigDecimal minimum,
      @Nullable BigDecimal maximum)
      implements ShowPropertyElementType {
    @Override
    public String kind() {
      return "number_range";
    }

    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    public static NumberRange read(JsonParser p) throws IOException {
      JsonReading.expectStartObject(p, "ShowPropertyElementType.NumberRange");
      BigDecimal lower = null;
      boolean lowerSeen = false;
      BigDecimal upper = null;
      boolean upperSeen = false;
      BigDecimal minimum = null;
      boolean minimumSeen = false;
      BigDecimal maximum = null;
      boolean maximumSeen = false;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "kind" -> expectKind(p, "number_range");
          case "lower" -> {
            lowerSeen = true;
            lower = readNullableDecimal(p);
          }
          case "upper" -> {
            upperSeen = true;
            upper = readNullableDecimal(p);
          }
          case "minimum" -> {
            minimumSeen = true;
            minimum = readNullableDecimal(p);
          }
          case "maximum" -> {
            maximumSeen = true;
            maximum = readNullableDecimal(p);
          }
          default -> JsonReading.unknownField(field, "ShowPropertyElementType.NumberRange");
        }
      }
      if (!lowerSeen) {
        JsonReading.missingRequired("lower", "ShowPropertyElementType.NumberRange");
      }
      if (!upperSeen) {
        JsonReading.missingRequired("upper", "ShowPropertyElementType.NumberRange");
      }
      if (!minimumSeen) {
        JsonReading.missingRequired("minimum", "ShowPropertyElementType.NumberRange");
      }
      if (!maximumSeen) {
        JsonReading.missingRequired("maximum", "ShowPropertyElementType.NumberRange");
      }
      return new NumberRange(lower, upper, minimum, maximum);
    }
  }

  /**
   * Ratio element.
   *
   * @param minimum minimum
   * @param maximum maximum
   * @param decimals decimals
   * @param units units
   * @param unit written unit when describing a value, or null
   */
  record Ratio(
      @Nullable BigDecimal minimum,
      @Nullable BigDecimal maximum,
      @Nullable Integer decimals,
      List<LemmaType.RatioUnit> units,
      @Nullable String unit)
      implements ShowPropertyElementType {
    @Override
    public String kind() {
      return "ratio";
    }

    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    public static Ratio read(JsonParser p) throws IOException {
      JsonReading.expectStartObject(p, "ShowPropertyElementType.Ratio");
      BigDecimal minimum = null;
      boolean minimumSeen = false;
      BigDecimal maximum = null;
      boolean maximumSeen = false;
      Integer decimals = null;
      boolean decimalsSeen = false;
      List<LemmaType.RatioUnit> units = null;
      String unit = null;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "kind" -> expectKind(p, "ratio");
          case "minimum" -> {
            minimumSeen = true;
            minimum = readNullableDecimal(p);
          }
          case "maximum" -> {
            maximumSeen = true;
            maximum = readNullableDecimal(p);
          }
          case "decimals" -> {
            decimalsSeen = true;
            decimals = JsonReading.readInt(p);
          }
          case "units" -> units = JsonReading.readList(p, LemmaType.RatioUnit::read);
          case "unit" -> unit = JsonReading.readString(p);
          default -> JsonReading.unknownField(field, "ShowPropertyElementType.Ratio");
        }
      }
      if (!minimumSeen) {
        JsonReading.missingRequired("minimum", "ShowPropertyElementType.Ratio");
      }
      if (!maximumSeen) {
        JsonReading.missingRequired("maximum", "ShowPropertyElementType.Ratio");
      }
      if (!decimalsSeen) {
        JsonReading.missingRequired("decimals", "ShowPropertyElementType.Ratio");
      }
      if (units == null) {
        JsonReading.missingRequired("units", "ShowPropertyElementType.Ratio");
      }
      return new Ratio(minimum, maximum, decimals, units, unit);
    }
  }

  /**
   * Ratio range element.
   *
   * @param lower lower
   * @param upper upper
   * @param minimum minimum
   * @param maximum maximum
   * @param units units
   */
  record RatioRange(
      @Nullable BigDecimal lower,
      @Nullable BigDecimal upper,
      @Nullable BigDecimal minimum,
      @Nullable BigDecimal maximum,
      List<LemmaType.RatioUnit> units)
      implements ShowPropertyElementType {
    @Override
    public String kind() {
      return "ratio_range";
    }

    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    public static RatioRange read(JsonParser p) throws IOException {
      JsonReading.expectStartObject(p, "ShowPropertyElementType.RatioRange");
      BigDecimal lower = null;
      boolean lowerSeen = false;
      BigDecimal upper = null;
      boolean upperSeen = false;
      BigDecimal minimum = null;
      boolean minimumSeen = false;
      BigDecimal maximum = null;
      boolean maximumSeen = false;
      List<LemmaType.RatioUnit> units = null;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "kind" -> expectKind(p, "ratio_range");
          case "lower" -> {
            lowerSeen = true;
            lower = readNullableDecimal(p);
          }
          case "upper" -> {
            upperSeen = true;
            upper = readNullableDecimal(p);
          }
          case "minimum" -> {
            minimumSeen = true;
            minimum = readNullableDecimal(p);
          }
          case "maximum" -> {
            maximumSeen = true;
            maximum = readNullableDecimal(p);
          }
          case "units" -> units = JsonReading.readList(p, LemmaType.RatioUnit::read);
          default -> JsonReading.unknownField(field, "ShowPropertyElementType.RatioRange");
        }
      }
      if (!lowerSeen) {
        JsonReading.missingRequired("lower", "ShowPropertyElementType.RatioRange");
      }
      if (!upperSeen) {
        JsonReading.missingRequired("upper", "ShowPropertyElementType.RatioRange");
      }
      if (!minimumSeen) {
        JsonReading.missingRequired("minimum", "ShowPropertyElementType.RatioRange");
      }
      if (!maximumSeen) {
        JsonReading.missingRequired("maximum", "ShowPropertyElementType.RatioRange");
      }
      if (units == null) {
        JsonReading.missingRequired("units", "ShowPropertyElementType.RatioRange");
      }
      return new RatioRange(lower, upper, minimum, maximum, units);
    }
  }

  /** Text element. */
  record Text() implements ShowPropertyElementType {
    @Override
    public String kind() {
      return "text";
    }

    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    public static Text read(JsonParser p) throws IOException {
      JsonReading.expectStartObject(p, "ShowPropertyElementType.Text");
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "kind" -> expectKind(p, "text");
          default -> JsonReading.unknownField(field, "ShowPropertyElementType.Text");
        }
      }
      return new Text();
    }
  }

  /**
   * Date element.
   *
   * @param minimum minimum
   * @param maximum maximum
   */
  record DateType(@Nullable String minimum, @Nullable String maximum)
      implements ShowPropertyElementType {
    @Override
    public String kind() {
      return "date";
    }

    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    public static DateType read(JsonParser p) throws IOException {
      return readStringBounds(p, "date", "ShowPropertyElementType.Date");
    }
  }

  /**
   * Time element.
   *
   * @param minimum minimum
   * @param maximum maximum
   */
  record TimeType(@Nullable String minimum, @Nullable String maximum)
      implements ShowPropertyElementType {
    @Override
    public String kind() {
      return "time";
    }

    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    public static TimeType read(JsonParser p) throws IOException {
      var bounds = readStringBounds(p, "time", "ShowPropertyElementType.Time");
      return new TimeType(bounds.minimum(), bounds.maximum());
    }
  }

  private static DateType readStringBounds(JsonParser p, String kind, String typeName)
      throws IOException {
    JsonReading.expectStartObject(p, typeName);
    String minimum = null;
    boolean minimumSeen = false;
    String maximum = null;
    boolean maximumSeen = false;
    while (p.nextToken() != JsonToken.END_OBJECT) {
      String field = p.currentName();
      p.nextToken();
      switch (field) {
        case "kind" -> expectKind(p, kind);
        case "minimum" -> {
          minimumSeen = true;
          minimum = JsonReading.readString(p);
        }
        case "maximum" -> {
          maximumSeen = true;
          maximum = JsonReading.readString(p);
        }
        default -> JsonReading.unknownField(field, typeName);
      }
    }
    if (!minimumSeen) {
      JsonReading.missingRequired("minimum", typeName);
    }
    if (!maximumSeen) {
      JsonReading.missingRequired("maximum", typeName);
    }
    return new DateType(minimum, maximum);
  }

  /**
   * Date range element.
   *
   * @param lower lower
   * @param upper upper
   * @param minimum minimum
   * @param maximum maximum
   */
  record DateRange(
      @Nullable String lower,
      @Nullable String upper,
      LemmaType.@Nullable NamedBound minimum,
      LemmaType.@Nullable NamedBound maximum)
      implements ShowPropertyElementType {
    @Override
    public String kind() {
      return "date_range";
    }

    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    public static DateRange read(JsonParser p) throws IOException {
      return readCalendarRange(p, "date_range", "ShowPropertyElementType.DateRange");
    }
  }

  /**
   * Time range element.
   *
   * @param lower lower
   * @param upper upper
   * @param minimum minimum
   * @param maximum maximum
   */
  record TimeRange(
      @Nullable String lower,
      @Nullable String upper,
      LemmaType.@Nullable NamedBound minimum,
      LemmaType.@Nullable NamedBound maximum)
      implements ShowPropertyElementType {
    @Override
    public String kind() {
      return "time_range";
    }

    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    public static TimeRange read(JsonParser p) throws IOException {
      var range = readCalendarRange(p, "time_range", "ShowPropertyElementType.TimeRange");
      return new TimeRange(range.lower(), range.upper(), range.minimum(), range.maximum());
    }
  }

  private static DateRange readCalendarRange(JsonParser p, String kind, String typeName)
      throws IOException {
    JsonReading.expectStartObject(p, typeName);
    String lower = null;
    boolean lowerSeen = false;
    String upper = null;
    boolean upperSeen = false;
    LemmaType.NamedBound minimum = null;
    boolean minimumSeen = false;
    LemmaType.NamedBound maximum = null;
    boolean maximumSeen = false;
    while (p.nextToken() != JsonToken.END_OBJECT) {
      String field = p.currentName();
      p.nextToken();
      switch (field) {
        case "kind" -> expectKind(p, kind);
        case "lower" -> {
          lowerSeen = true;
          lower = JsonReading.readString(p);
        }
        case "upper" -> {
          upperSeen = true;
          upper = JsonReading.readString(p);
        }
        case "minimum" -> {
          minimumSeen = true;
          minimum = LemmaType.NamedBound.readNullable(p);
        }
        case "maximum" -> {
          maximumSeen = true;
          maximum = LemmaType.NamedBound.readNullable(p);
        }
        default -> JsonReading.unknownField(field, typeName);
      }
    }
    if (!lowerSeen) {
      JsonReading.missingRequired("lower", typeName);
    }
    if (!upperSeen) {
      JsonReading.missingRequired("upper", typeName);
    }
    if (!minimumSeen) {
      JsonReading.missingRequired("minimum", typeName);
    }
    if (!maximumSeen) {
      JsonReading.missingRequired("maximum", typeName);
    }
    return new DateRange(lower, upper, minimum, maximum);
  }

  /**
   * Measure range element.
   *
   * @param lower lower
   * @param upper upper
   * @param minimum minimum
   * @param maximum maximum
   * @param units units
   * @param decomposition decomposition
   */
  record MeasureRange(
      LemmaType.@Nullable NamedBound lower,
      LemmaType.@Nullable NamedBound upper,
      LemmaType.@Nullable NamedBound minimum,
      LemmaType.@Nullable NamedBound maximum,
      List<LemmaType.MeasureUnit> units,
      @Nullable Map<String, Integer> decomposition)
      implements ShowPropertyElementType {
    @Override
    public String kind() {
      return "measure_range";
    }

    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    public static MeasureRange read(JsonParser p) throws IOException {
      JsonReading.expectStartObject(p, "ShowPropertyElementType.MeasureRange");
      LemmaType.NamedBound lower = null;
      boolean lowerSeen = false;
      LemmaType.NamedBound upper = null;
      boolean upperSeen = false;
      LemmaType.NamedBound minimum = null;
      boolean minimumSeen = false;
      LemmaType.NamedBound maximum = null;
      boolean maximumSeen = false;
      List<LemmaType.MeasureUnit> units = null;
      Map<String, Integer> decomposition = null;
      boolean decompositionSeen = false;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "kind" -> expectKind(p, "measure_range");
          case "lower" -> {
            lowerSeen = true;
            lower = LemmaType.NamedBound.readNullable(p);
          }
          case "upper" -> {
            upperSeen = true;
            upper = LemmaType.NamedBound.readNullable(p);
          }
          case "minimum" -> {
            minimumSeen = true;
            minimum = LemmaType.NamedBound.readNullable(p);
          }
          case "maximum" -> {
            maximumSeen = true;
            maximum = LemmaType.NamedBound.readNullable(p);
          }
          case "units" -> units = JsonReading.readList(p, LemmaType.MeasureUnit::read);
          case "decomposition" -> {
            decompositionSeen = true;
            decomposition =
                p.currentToken() == JsonToken.VALUE_NULL
                    ? null
                    : JsonReading.readMap(p, JsonReading::readInt);
          }
          default -> JsonReading.unknownField(field, "ShowPropertyElementType.MeasureRange");
        }
      }
      if (!lowerSeen) {
        JsonReading.missingRequired("lower", "ShowPropertyElementType.MeasureRange");
      }
      if (!upperSeen) {
        JsonReading.missingRequired("upper", "ShowPropertyElementType.MeasureRange");
      }
      if (!minimumSeen) {
        JsonReading.missingRequired("minimum", "ShowPropertyElementType.MeasureRange");
      }
      if (!maximumSeen) {
        JsonReading.missingRequired("maximum", "ShowPropertyElementType.MeasureRange");
      }
      if (units == null) {
        JsonReading.missingRequired("units", "ShowPropertyElementType.MeasureRange");
      }
      if (!decompositionSeen) {
        JsonReading.missingRequired("decomposition", "ShowPropertyElementType.MeasureRange");
      }
      return new MeasureRange(lower, upper, minimum, maximum, units, decomposition);
    }
  }

  /**
   * Spec element for instance lists.
   *
   * @param repository repository or null for the unnamed workspace
   * @param spec spec name
   * @param effective resolved row effective date, empty for the origin row
   */
  record Spec(@Nullable String repository, String spec, String effective)
      implements ShowPropertyElementType {
    @Override
    public String kind() {
      return "spec";
    }

    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    public static Spec read(JsonParser p) throws IOException {
      JsonReading.expectStartObject(p, "ShowPropertyElementType.Spec");
      String repository = null;
      String spec = null;
      String effective = null;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "kind" -> expectKind(p, "spec");
          case "repository" -> repository = JsonReading.readString(p);
          case "spec" -> spec = JsonReading.readString(p);
          case "effective" -> effective = JsonReading.readString(p);
          default -> JsonReading.unknownField(field, "ShowPropertyElementType.Spec");
        }
      }
      if (spec == null) {
        JsonReading.missingRequired("spec", "ShowPropertyElementType.Spec");
      }
      if (effective == null) {
        JsonReading.missingRequired("effective", "ShowPropertyElementType.Spec");
      }
      return new Spec(repository, spec, effective);
    }
  }

  /**
   * Parses JSON.
   *
   * @param p parser at value start
   * @return parsed value
   * @throws IOException if JSON IO fails
   */
  public static ShowPropertyElementType read(JsonParser p) throws IOException {
    JsonReading.expectStartObject(p, "ShowPropertyElementType");
    String json = JsonReading.bufferObjectAsString(p);
    String kind = JsonReading.findTag(json, "kind");
    if (kind == null) {
      throw new LemmaBugError("BUG: missing 'kind' in ShowPropertyElementType");
    }
    if ("list".equals(kind)) {
      throw new LemmaBugError("BUG: list element is a list: " + json);
    }
    try (JsonParser reader = JsonReading.parserFor(json)) {
      return switch (kind) {
        case "boolean" -> BooleanType.read(reader);
        case "measure" -> Measure.read(reader);
        case "number" -> NumberType.read(reader);
        case "number_range" -> NumberRange.read(reader);
        case "ratio" -> Ratio.read(reader);
        case "ratio_range" -> RatioRange.read(reader);
        case "text" -> Text.read(reader);
        case "date" -> DateType.read(reader);
        case "date_range" -> DateRange.read(reader);
        case "time" -> TimeType.read(reader);
        case "time_range" -> TimeRange.read(reader);
        case "measure_range" -> MeasureRange.read(reader);
        case "spec" -> Spec.read(reader);
        default -> throw new LemmaBugError("BUG: unknown kind value: " + kind);
      };
    }
  }
}

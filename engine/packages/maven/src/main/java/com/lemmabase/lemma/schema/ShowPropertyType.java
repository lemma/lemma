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
 * {@code x-lemma-type} of a Show property: {@link LemmaType} without help and text options/length,
 * which the property states as {@code description}, {@code enum} and {@code minLength/maxLength}.
 */
public sealed interface ShowPropertyType
    permits ShowPropertyType.BooleanType,
        ShowPropertyType.Measure,
        ShowPropertyType.NumberType,
        ShowPropertyType.NumberRange,
        ShowPropertyType.Ratio,
        ShowPropertyType.RatioRange,
        ShowPropertyType.Text,
        ShowPropertyType.DateType,
        ShowPropertyType.DateRange,
        ShowPropertyType.TimeType,
        ShowPropertyType.TimeRange,
        ShowPropertyType.MeasureRange,
        ShowPropertyType.ListType {
  /**
   * Kind tag for this type.
   *
   * @return kind tag
   */
  String kind();

  /**
   * Typedef name when declared.
   *
   * @return typedef name or null
   */
  @Nullable
  String name();

  /**
   * Extends clause for this property type.
   *
   * @return extends clause
   */
  TypeExtends extendsType();

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

  private static @Nullable String readRequiredName(
      boolean nameSeen, @Nullable String name, String typeName) {
    if (!nameSeen) {
      JsonReading.missingRequired("name", typeName);
    }
    return name;
  }

  private static TypeExtends readRequiredExtends(
      @Nullable TypeExtends extendsType, String typeName) {
    if (extendsType == null) {
      JsonReading.missingRequired("extends", typeName);
    }
    return extendsType;
  }

  /**
   * Boolean property type.
   *
   * @param name name
   * @param extendsType extendsType
   */
  record BooleanType(@Nullable String name, TypeExtends extendsType) implements ShowPropertyType {
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
      JsonReading.expectStartObject(p, "ShowPropertyType.Boolean");
      String name = null;
      boolean nameSeen = false;
      TypeExtends extendsType = null;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "name" -> {
            nameSeen = true;
            name = JsonReading.readString(p);
          }
          case "kind" -> expectKind(p, "boolean");
          case "extends" -> extendsType = TypeExtends.read(p);
          default -> JsonReading.unknownField(field, "ShowPropertyType.Boolean");
        }
      }
      name = readRequiredName(nameSeen, name, "ShowPropertyType.Boolean");
      extendsType = readRequiredExtends(extendsType, "ShowPropertyType.Boolean");
      return new BooleanType(name, extendsType);
    }
  }

  /**
   * Measure property type.
   *
   * @param name name
   * @param minimum minimum
   * @param maximum maximum
   * @param decimals decimals
   * @param units units
   * @param traits traits
   * @param decomposition decomposition
   * @param unit written unit when describing a value, or null
   * @param extendsType extendsType
   */
  record Measure(
      @Nullable String name,
      LemmaType.@Nullable NamedBound minimum,
      LemmaType.@Nullable NamedBound maximum,
      @Nullable Integer decimals,
      List<LemmaType.MeasureUnit> units,
      List<String> traits,
      @Nullable Map<String, Integer> decomposition,
      @Nullable String unit,
      TypeExtends extendsType)
      implements ShowPropertyType {
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
      JsonReading.expectStartObject(p, "ShowPropertyType.Measure");
      String name = null;
      boolean nameSeen = false;
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
      TypeExtends extendsType = null;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "name" -> {
            nameSeen = true;
            name = JsonReading.readString(p);
          }
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
          case "extends" -> extendsType = TypeExtends.read(p);
          default -> JsonReading.unknownField(field, "ShowPropertyType.Measure");
        }
      }
      name = readRequiredName(nameSeen, name, "ShowPropertyType.Measure");
      if (!minimumSeen) {
        JsonReading.missingRequired("minimum", "ShowPropertyType.Measure");
      }
      if (!maximumSeen) {
        JsonReading.missingRequired("maximum", "ShowPropertyType.Measure");
      }
      if (!decimalsSeen) {
        JsonReading.missingRequired("decimals", "ShowPropertyType.Measure");
      }
      if (units == null) {
        JsonReading.missingRequired("units", "ShowPropertyType.Measure");
      }
      if (traits == null) {
        JsonReading.missingRequired("traits", "ShowPropertyType.Measure");
      }
      if (!decompositionSeen) {
        JsonReading.missingRequired("decomposition", "ShowPropertyType.Measure");
      }
      extendsType = readRequiredExtends(extendsType, "ShowPropertyType.Measure");
      return new Measure(
          name, minimum, maximum, decimals, units, traits, decomposition, unit, extendsType);
    }
  }

  /**
   * Number property type.
   *
   * @param name name
   * @param minimum minimum
   * @param maximum maximum
   * @param decimals decimals
   * @param extendsType extendsType
   */
  record NumberType(
      @Nullable String name,
      @Nullable BigDecimal minimum,
      @Nullable BigDecimal maximum,
      @Nullable Integer decimals,
      TypeExtends extendsType)
      implements ShowPropertyType {
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
      JsonReading.expectStartObject(p, "ShowPropertyType.Number");
      String name = null;
      boolean nameSeen = false;
      BigDecimal minimum = null;
      boolean minimumSeen = false;
      BigDecimal maximum = null;
      boolean maximumSeen = false;
      Integer decimals = null;
      boolean decimalsSeen = false;
      TypeExtends extendsType = null;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "name" -> {
            nameSeen = true;
            name = JsonReading.readString(p);
          }
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
          case "extends" -> extendsType = TypeExtends.read(p);
          default -> JsonReading.unknownField(field, "ShowPropertyType.Number");
        }
      }
      name = readRequiredName(nameSeen, name, "ShowPropertyType.Number");
      if (!minimumSeen) {
        JsonReading.missingRequired("minimum", "ShowPropertyType.Number");
      }
      if (!maximumSeen) {
        JsonReading.missingRequired("maximum", "ShowPropertyType.Number");
      }
      if (!decimalsSeen) {
        JsonReading.missingRequired("decimals", "ShowPropertyType.Number");
      }
      extendsType = readRequiredExtends(extendsType, "ShowPropertyType.Number");
      return new NumberType(name, minimum, maximum, decimals, extendsType);
    }
  }

  /**
   * Number range property type.
   *
   * @param name name
   * @param lower lower
   * @param upper upper
   * @param minimum minimum
   * @param maximum maximum
   * @param extendsType extendsType
   */
  record NumberRange(
      @Nullable String name,
      @Nullable BigDecimal lower,
      @Nullable BigDecimal upper,
      @Nullable BigDecimal minimum,
      @Nullable BigDecimal maximum,
      TypeExtends extendsType)
      implements ShowPropertyType {
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
      JsonReading.expectStartObject(p, "ShowPropertyType.NumberRange");
      String name = null;
      boolean nameSeen = false;
      BigDecimal lower = null;
      boolean lowerSeen = false;
      BigDecimal upper = null;
      boolean upperSeen = false;
      BigDecimal minimum = null;
      boolean minimumSeen = false;
      BigDecimal maximum = null;
      boolean maximumSeen = false;
      TypeExtends extendsType = null;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "name" -> {
            nameSeen = true;
            name = JsonReading.readString(p);
          }
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
          case "extends" -> extendsType = TypeExtends.read(p);
          default -> JsonReading.unknownField(field, "ShowPropertyType.NumberRange");
        }
      }
      name = readRequiredName(nameSeen, name, "ShowPropertyType.NumberRange");
      if (!lowerSeen) {
        JsonReading.missingRequired("lower", "ShowPropertyType.NumberRange");
      }
      if (!upperSeen) {
        JsonReading.missingRequired("upper", "ShowPropertyType.NumberRange");
      }
      if (!minimumSeen) {
        JsonReading.missingRequired("minimum", "ShowPropertyType.NumberRange");
      }
      if (!maximumSeen) {
        JsonReading.missingRequired("maximum", "ShowPropertyType.NumberRange");
      }
      extendsType = readRequiredExtends(extendsType, "ShowPropertyType.NumberRange");
      return new NumberRange(name, lower, upper, minimum, maximum, extendsType);
    }
  }

  /**
   * Ratio property type.
   *
   * @param name name
   * @param minimum minimum
   * @param maximum maximum
   * @param decimals decimals
   * @param units units
   * @param unit written unit when describing a value, or null
   * @param extendsType extendsType
   */
  record Ratio(
      @Nullable String name,
      @Nullable BigDecimal minimum,
      @Nullable BigDecimal maximum,
      @Nullable Integer decimals,
      List<LemmaType.RatioUnit> units,
      @Nullable String unit,
      TypeExtends extendsType)
      implements ShowPropertyType {
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
      JsonReading.expectStartObject(p, "ShowPropertyType.Ratio");
      String name = null;
      boolean nameSeen = false;
      BigDecimal minimum = null;
      boolean minimumSeen = false;
      BigDecimal maximum = null;
      boolean maximumSeen = false;
      Integer decimals = null;
      boolean decimalsSeen = false;
      List<LemmaType.RatioUnit> units = null;
      String unit = null;
      TypeExtends extendsType = null;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "name" -> {
            nameSeen = true;
            name = JsonReading.readString(p);
          }
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
          case "extends" -> extendsType = TypeExtends.read(p);
          default -> JsonReading.unknownField(field, "ShowPropertyType.Ratio");
        }
      }
      name = readRequiredName(nameSeen, name, "ShowPropertyType.Ratio");
      if (!minimumSeen) {
        JsonReading.missingRequired("minimum", "ShowPropertyType.Ratio");
      }
      if (!maximumSeen) {
        JsonReading.missingRequired("maximum", "ShowPropertyType.Ratio");
      }
      if (!decimalsSeen) {
        JsonReading.missingRequired("decimals", "ShowPropertyType.Ratio");
      }
      if (units == null) {
        JsonReading.missingRequired("units", "ShowPropertyType.Ratio");
      }
      extendsType = readRequiredExtends(extendsType, "ShowPropertyType.Ratio");
      return new Ratio(name, minimum, maximum, decimals, units, unit, extendsType);
    }
  }

  /**
   * Ratio range property type.
   *
   * @param name name
   * @param lower lower
   * @param upper upper
   * @param minimum minimum
   * @param maximum maximum
   * @param units units
   * @param extendsType extendsType
   */
  record RatioRange(
      @Nullable String name,
      @Nullable BigDecimal lower,
      @Nullable BigDecimal upper,
      @Nullable BigDecimal minimum,
      @Nullable BigDecimal maximum,
      List<LemmaType.RatioUnit> units,
      TypeExtends extendsType)
      implements ShowPropertyType {
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
      JsonReading.expectStartObject(p, "ShowPropertyType.RatioRange");
      String name = null;
      boolean nameSeen = false;
      BigDecimal lower = null;
      boolean lowerSeen = false;
      BigDecimal upper = null;
      boolean upperSeen = false;
      BigDecimal minimum = null;
      boolean minimumSeen = false;
      BigDecimal maximum = null;
      boolean maximumSeen = false;
      List<LemmaType.RatioUnit> units = null;
      TypeExtends extendsType = null;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "name" -> {
            nameSeen = true;
            name = JsonReading.readString(p);
          }
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
          case "extends" -> extendsType = TypeExtends.read(p);
          default -> JsonReading.unknownField(field, "ShowPropertyType.RatioRange");
        }
      }
      name = readRequiredName(nameSeen, name, "ShowPropertyType.RatioRange");
      if (!lowerSeen) {
        JsonReading.missingRequired("lower", "ShowPropertyType.RatioRange");
      }
      if (!upperSeen) {
        JsonReading.missingRequired("upper", "ShowPropertyType.RatioRange");
      }
      if (!minimumSeen) {
        JsonReading.missingRequired("minimum", "ShowPropertyType.RatioRange");
      }
      if (!maximumSeen) {
        JsonReading.missingRequired("maximum", "ShowPropertyType.RatioRange");
      }
      if (units == null) {
        JsonReading.missingRequired("units", "ShowPropertyType.RatioRange");
      }
      extendsType = readRequiredExtends(extendsType, "ShowPropertyType.RatioRange");
      return new RatioRange(name, lower, upper, minimum, maximum, units, extendsType);
    }
  }

  /**
   * Text property type.
   *
   * @param name name
   * @param extendsType extendsType
   */
  record Text(@Nullable String name, TypeExtends extendsType) implements ShowPropertyType {
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
      JsonReading.expectStartObject(p, "ShowPropertyType.Text");
      String name = null;
      boolean nameSeen = false;
      TypeExtends extendsType = null;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "name" -> {
            nameSeen = true;
            name = JsonReading.readString(p);
          }
          case "kind" -> expectKind(p, "text");
          case "extends" -> extendsType = TypeExtends.read(p);
          default -> JsonReading.unknownField(field, "ShowPropertyType.Text");
        }
      }
      name = readRequiredName(nameSeen, name, "ShowPropertyType.Text");
      extendsType = readRequiredExtends(extendsType, "ShowPropertyType.Text");
      return new Text(name, extendsType);
    }
  }

  /**
   * Date property type.
   *
   * @param name name
   * @param minimum minimum
   * @param maximum maximum
   * @param extendsType extendsType
   */
  record DateType(
      @Nullable String name,
      @Nullable String minimum,
      @Nullable String maximum,
      TypeExtends extendsType)
      implements ShowPropertyType {
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
      return readStringBounds(p, "date", "ShowPropertyType.Date");
    }
  }

  /**
   * Time property type.
   *
   * @param name name
   * @param minimum minimum
   * @param maximum maximum
   * @param extendsType extendsType
   */
  record TimeType(
      @Nullable String name,
      @Nullable String minimum,
      @Nullable String maximum,
      TypeExtends extendsType)
      implements ShowPropertyType {
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
      var bounds = readStringBounds(p, "time", "ShowPropertyType.Time");
      return new TimeType(bounds.name(), bounds.minimum(), bounds.maximum(), bounds.extendsType());
    }
  }

  private static DateType readStringBounds(JsonParser p, String kind, String typeName)
      throws IOException {
    JsonReading.expectStartObject(p, typeName);
    String name = null;
    boolean nameSeen = false;
    String minimum = null;
    boolean minimumSeen = false;
    String maximum = null;
    boolean maximumSeen = false;
    TypeExtends extendsType = null;
    while (p.nextToken() != JsonToken.END_OBJECT) {
      String field = p.currentName();
      p.nextToken();
      switch (field) {
        case "name" -> {
          nameSeen = true;
          name = JsonReading.readString(p);
        }
        case "kind" -> expectKind(p, kind);
        case "minimum" -> {
          minimumSeen = true;
          minimum = JsonReading.readString(p);
        }
        case "maximum" -> {
          maximumSeen = true;
          maximum = JsonReading.readString(p);
        }
        case "extends" -> extendsType = TypeExtends.read(p);
        default -> JsonReading.unknownField(field, typeName);
      }
    }
    name = readRequiredName(nameSeen, name, typeName);
    if (!minimumSeen) {
      JsonReading.missingRequired("minimum", typeName);
    }
    if (!maximumSeen) {
      JsonReading.missingRequired("maximum", typeName);
    }
    extendsType = readRequiredExtends(extendsType, typeName);
    return new DateType(name, minimum, maximum, extendsType);
  }

  /**
   * Date range property type.
   *
   * @param name name
   * @param lower lower
   * @param upper upper
   * @param minimum minimum
   * @param maximum maximum
   * @param extendsType extendsType
   */
  record DateRange(
      @Nullable String name,
      @Nullable String lower,
      @Nullable String upper,
      LemmaType.@Nullable NamedBound minimum,
      LemmaType.@Nullable NamedBound maximum,
      TypeExtends extendsType)
      implements ShowPropertyType {
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
      return readCalendarRange(p, "date_range", "ShowPropertyType.DateRange");
    }
  }

  /**
   * Time range property type.
   *
   * @param name name
   * @param lower lower
   * @param upper upper
   * @param minimum minimum
   * @param maximum maximum
   * @param extendsType extendsType
   */
  record TimeRange(
      @Nullable String name,
      @Nullable String lower,
      @Nullable String upper,
      LemmaType.@Nullable NamedBound minimum,
      LemmaType.@Nullable NamedBound maximum,
      TypeExtends extendsType)
      implements ShowPropertyType {
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
      var range = readCalendarRange(p, "time_range", "ShowPropertyType.TimeRange");
      return new TimeRange(
          range.name(), range.lower(), range.upper(), range.minimum(), range.maximum(),
          range.extendsType());
    }
  }

  private static DateRange readCalendarRange(JsonParser p, String kind, String typeName)
      throws IOException {
    JsonReading.expectStartObject(p, typeName);
    String name = null;
    boolean nameSeen = false;
    String lower = null;
    boolean lowerSeen = false;
    String upper = null;
    boolean upperSeen = false;
    LemmaType.NamedBound minimum = null;
    boolean minimumSeen = false;
    LemmaType.NamedBound maximum = null;
    boolean maximumSeen = false;
    TypeExtends extendsType = null;
    while (p.nextToken() != JsonToken.END_OBJECT) {
      String field = p.currentName();
      p.nextToken();
      switch (field) {
        case "name" -> {
          nameSeen = true;
          name = JsonReading.readString(p);
        }
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
        case "extends" -> extendsType = TypeExtends.read(p);
        default -> JsonReading.unknownField(field, typeName);
      }
    }
    name = readRequiredName(nameSeen, name, typeName);
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
    extendsType = readRequiredExtends(extendsType, typeName);
    return new DateRange(name, lower, upper, minimum, maximum, extendsType);
  }

  /**
   * Measure range property type.
   *
   * @param name name
   * @param lower lower
   * @param upper upper
   * @param minimum minimum
   * @param maximum maximum
   * @param units units
   * @param decomposition decomposition
   * @param extendsType extendsType
   */
  record MeasureRange(
      @Nullable String name,
      LemmaType.@Nullable NamedBound lower,
      LemmaType.@Nullable NamedBound upper,
      LemmaType.@Nullable NamedBound minimum,
      LemmaType.@Nullable NamedBound maximum,
      List<LemmaType.MeasureUnit> units,
      @Nullable Map<String, Integer> decomposition,
      TypeExtends extendsType)
      implements ShowPropertyType {
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
      JsonReading.expectStartObject(p, "ShowPropertyType.MeasureRange");
      String name = null;
      boolean nameSeen = false;
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
      TypeExtends extendsType = null;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "name" -> {
            nameSeen = true;
            name = JsonReading.readString(p);
          }
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
          case "extends" -> extendsType = TypeExtends.read(p);
          default -> JsonReading.unknownField(field, "ShowPropertyType.MeasureRange");
        }
      }
      name = readRequiredName(nameSeen, name, "ShowPropertyType.MeasureRange");
      if (!lowerSeen) {
        JsonReading.missingRequired("lower", "ShowPropertyType.MeasureRange");
      }
      if (!upperSeen) {
        JsonReading.missingRequired("upper", "ShowPropertyType.MeasureRange");
      }
      if (!minimumSeen) {
        JsonReading.missingRequired("minimum", "ShowPropertyType.MeasureRange");
      }
      if (!maximumSeen) {
        JsonReading.missingRequired("maximum", "ShowPropertyType.MeasureRange");
      }
      if (units == null) {
        JsonReading.missingRequired("units", "ShowPropertyType.MeasureRange");
      }
      if (!decompositionSeen) {
        JsonReading.missingRequired("decomposition", "ShowPropertyType.MeasureRange");
      }
      extendsType = readRequiredExtends(extendsType, "ShowPropertyType.MeasureRange");
      return new MeasureRange(
          name, lower, upper, minimum, maximum, units, decomposition, extendsType);
    }
  }

  /**
   * List property type.
   *
   * @param name name
   * @param element row element type
   * @param key how rows are addressed
   * @param extendsType extendsType
   */
  record ListType(
      @Nullable String name,
      ShowPropertyElementType element,
      LemmaType.ListKey key,
      TypeExtends extendsType)
      implements ShowPropertyType {
    @Override
    public String kind() {
      return "list";
    }

    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    public static ListType read(JsonParser p) throws IOException {
      JsonReading.expectStartObject(p, "ShowPropertyType.ListType");
      String name = null;
      boolean nameSeen = false;
      ShowPropertyElementType element = null;
      LemmaType.ListKey key = null;
      TypeExtends extendsType = null;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "name" -> {
            nameSeen = true;
            name = JsonReading.readString(p);
          }
          case "kind" -> expectKind(p, "list");
          case "element" -> element = ShowPropertyElementType.read(p);
          case "key" -> key = LemmaType.ListKey.read(p);
          case "extends" -> extendsType = TypeExtends.read(p);
          default -> JsonReading.unknownField(field, "ShowPropertyType.ListType");
        }
      }
      name = readRequiredName(nameSeen, name, "ShowPropertyType.ListType");
      if (element == null) {
        JsonReading.missingRequired("element", "ShowPropertyType.ListType");
      }
      if (key == null) {
        JsonReading.missingRequired("key", "ShowPropertyType.ListType");
      }
      extendsType = readRequiredExtends(extendsType, "ShowPropertyType.ListType");
      return new ListType(name, element, key, extendsType);
    }
  }

  /**
   * Parses JSON.
   *
   * @param p parser at value start
   * @return parsed value
   * @throws IOException if JSON IO fails
   */
  public static ShowPropertyType read(JsonParser p) throws IOException {
    JsonReading.expectStartObject(p, "ShowPropertyType");
    String json = JsonReading.bufferObjectAsString(p);
    String kind = JsonReading.findTag(json, "kind");
    if (kind == null) {
      throw new LemmaBugError("BUG: missing 'kind' in ShowPropertyType");
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
        case "list" -> ListType.read(reader);
        default -> throw new LemmaBugError("BUG: unknown kind value: " + kind);
      };
    }
  }
}

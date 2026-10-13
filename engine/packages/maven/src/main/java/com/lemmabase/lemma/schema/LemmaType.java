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
 * Resolved Lemma type at the API boundary. Discriminated by {@link #kind()}. Sentinel-only
 * specifications never reach a successfully planned response.
 */
public sealed interface LemmaType {
  /**
   * Returns the kind tag.
   *
   * @return kind tag
   */
  String kind();

  /**
   * Optional typedef name.
   *
   * @return type name or null
   */
  @Nullable
  String name();

  /**
   * Optional extends clause.
   *
   * @return extends type
   */
  TypeExtends extendsType();

  /**
   * RationalFactor.
   * @param numer numer
   * @param denom denom
   */
  record RationalFactor(BigDecimal numer, BigDecimal denom) {
    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    public static RationalFactor read(JsonParser p) throws IOException {
      JsonReading.expectStartObject(p, "RationalFactor");
      BigDecimal numer = null;
      BigDecimal denom = null;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "numer" -> numer = JsonReading.readDecimal(p);
          case "denom" -> denom = JsonReading.readDecimal(p);
          default -> JsonReading.unknownField(field, "RationalFactor");
        }
      }
      if (numer == null) {
        JsonReading.missingRequired("numer", "RationalFactor");
      }
      if (denom == null) {
        JsonReading.missingRequired("denom", "RationalFactor");
      }
      return new RationalFactor(numer, denom);
    }
  }

  /**
   * NamedBound.
   * @param value value
   * @param unit unit
   */
  record NamedBound(BigDecimal value, String unit) {
    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    public static NamedBound read(JsonParser p) throws IOException {
      JsonReading.expectStartObject(p, "NamedBound");
      BigDecimal value = null;
      String unit = null;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "value" -> value = JsonReading.readDecimal(p);
          case "unit" -> unit = JsonReading.readString(p);
          default -> JsonReading.unknownField(field, "NamedBound");
        }
      }
      if (value == null) {
        JsonReading.missingRequired("value", "NamedBound");
      }
      if (unit == null) {
        JsonReading.missingRequired("unit", "NamedBound");
      }
      return new NamedBound(value, unit);
    }

    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    public static @Nullable NamedBound readNullable(JsonParser p) throws IOException {
      if (p.currentToken() == JsonToken.VALUE_NULL) {
        return null;
      }
      return read(p);
    }
  }

  /**
   * MeasureUnit.
   * @param name name
   * @param factor factor
   * @param derivedMeasureFactors derivedMeasureFactors
   * @param decomposition decomposition
   * @param minimum minimum
   * @param maximum maximum
   * @param suggestion suggestion
   */
  record MeasureUnit(
      String name,
      RationalFactor factor,
      List<DerivedMeasureFactor> derivedMeasureFactors,
      Map<String, Integer> decomposition,
      @Nullable BigDecimal minimum,
      @Nullable BigDecimal maximum,
      @Nullable BigDecimal suggestion) {
    /**
     * DerivedMeasureFactor.
     * @param measureRef measureRef
     * @param exponent exponent
     */
    record DerivedMeasureFactor(String measureRef, int exponent) {}

    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    public static MeasureUnit read(JsonParser p) throws IOException {
      JsonReading.expectStartObject(p, "MeasureUnit");
      String name = null;
      RationalFactor factor = null;
      List<DerivedMeasureFactor> derived = null;
      Map<String, Integer> decomposition = null;
      BigDecimal minimum = null;
      BigDecimal maximum = null;
      BigDecimal suggestion = null;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "name" -> name = JsonReading.readString(p);
          case "factor" -> factor = RationalFactor.read(p);
          case "derived_measure_factors" ->
              derived =
                  JsonReading.readList(
                      p,
                      q -> {
                        var t = JsonReading.readTuple2(q, JsonReading::readString, JsonReading::readInt);
                        return new DerivedMeasureFactor(t.first(), t.second());
                      });
          case "decomposition" -> decomposition = JsonReading.readMap(p, JsonReading::readInt);
          case "minimum" -> minimum = JsonReading.readDecimal(p);
          case "maximum" -> maximum = JsonReading.readDecimal(p);
          case "suggestion" -> suggestion = JsonReading.readDecimal(p);
          default -> JsonReading.unknownField(field, "MeasureUnit");
        }
      }
      if (name == null) {
        JsonReading.missingRequired("name", "MeasureUnit");
      }
      if (factor == null) {
        JsonReading.missingRequired("factor", "MeasureUnit");
      }
      if (derived == null) {
        JsonReading.missingRequired("derived_measure_factors", "MeasureUnit");
      }
      if (decomposition == null) {
        JsonReading.missingRequired("decomposition", "MeasureUnit");
      }
      return new MeasureUnit(name, factor, derived, decomposition, minimum, maximum, suggestion);
    }
  }

  /**
   * RatioUnit.
   * @param name name
   * @param value value
   * @param minimum minimum
   * @param maximum maximum
   * @param suggestion suggestion
   */
  record RatioUnit(
      String name,
      RationalFactor value,
      @Nullable BigDecimal minimum,
      @Nullable BigDecimal maximum,
      @Nullable BigDecimal suggestion) {
    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    public static RatioUnit read(JsonParser p) throws IOException {
      JsonReading.expectStartObject(p, "RatioUnit");
      String name = null;
      RationalFactor value = null;
      BigDecimal minimum = null;
      BigDecimal maximum = null;
      BigDecimal suggestion = null;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "name" -> name = JsonReading.readString(p);
          case "value" -> value = RationalFactor.read(p);
          case "minimum" -> minimum = JsonReading.readDecimal(p);
          case "maximum" -> maximum = JsonReading.readDecimal(p);
          case "suggestion" -> suggestion = JsonReading.readDecimal(p);
          default -> JsonReading.unknownField(field, "RatioUnit");
        }
      }
      if (name == null) {
        JsonReading.missingRequired("name", "RatioUnit");
      }
      if (value == null) {
        JsonReading.missingRequired("value", "RatioUnit");
      }
      return new RatioUnit(name, value, minimum, maximum, suggestion);
    }
  }

  private static void expectKind(JsonParser p, String expected) throws IOException {
    String kind = JsonReading.readString(p);
    if (!expected.equals(kind)) {
      throw new LemmaBugError("BUG: expected kind '" + expected + "', got '" + kind + "'");
    }
  }

  /**
   * Name field for a named type, or null for a specification.
   *
   * @param nameSeen whether {@code name} was present
   * @param name parsed name
   * @param identity named type vs specification
   * @param typeName JSON type name for errors
   * @return name or null
   */
  private static @Nullable String identityName(
      boolean nameSeen, @Nullable String name, TypeIdentity identity, String typeName) {
    if (identity == TypeIdentity.NAMED) {
      if (!nameSeen) {
        JsonReading.missingRequired("name", typeName);
      }
      return name;
    }
    if (nameSeen) {
      throw new LemmaBugError("BUG: TypeSpecification carries name");
    }
    return null;
  }

  /**
   * Extends field for a named type, or primitive for a specification.
   *
   * @param extendsType parsed extends
   * @param identity named type vs specification
   * @param typeName JSON type name for errors
   * @return extends
   */
  private static TypeExtends identityExtends(
      @Nullable TypeExtends extendsType, TypeIdentity identity, String typeName) {
    if (identity == TypeIdentity.NAMED) {
      if (extendsType == null) {
        JsonReading.missingRequired("extends", typeName);
      }
      return extendsType;
    }
    if (extendsType != null) {
      throw new LemmaBugError("BUG: TypeSpecification carries extends");
    }
    return new TypeExtends.Primitive();
  }

  /**
   * Parses JSON.
   *
   * @param p parser at value start
   * @return parsed value
   * @throws IOException if JSON IO fails
   */
  private static @Nullable BigDecimal readNullableDecimal(JsonParser p) throws IOException {
    if (p.currentToken() == JsonToken.VALUE_NULL) {
      return null;
    }
    return JsonReading.readDecimal(p);
  }

  /**
   * BooleanType.
   * @param name name
   * @param help help
   * @param extendsType extendsType
   */
  record BooleanType(@Nullable String name, String help, TypeExtends extendsType)
      implements LemmaType {
    /** {@inheritDoc} */
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
      return read(p, TypeIdentity.NAMED);
    }

    static BooleanType read(JsonParser p, TypeIdentity identity) throws IOException {
      JsonReading.expectStartObject(p, "LemmaType.Boolean");
      String name = null;
      boolean nameSeen = false;
      String help = null;
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
          case "help" -> help = JsonReading.readString(p);
          case "extends" -> extendsType = TypeExtends.read(p);
          default -> JsonReading.unknownField(field, "LemmaType.Boolean");
        }
      }
      name = identityName(nameSeen, name, identity, "LemmaType.Boolean");
      if (help == null) {
        JsonReading.missingRequired("help", "LemmaType.Boolean");
      }
      extendsType = identityExtends(extendsType, identity, "LemmaType.Boolean");
      return new BooleanType(name, help, extendsType);
    }
  }

  /**
   * Measure.
   * @param name name
   * @param minimum minimum
   * @param maximum maximum
   * @param decimals decimals
   * @param units units
   * @param traits traits
   * @param decomposition decomposition
   * @param unit written unit when describing a value, or null
   * @param help help
   * @param extendsType extendsType
   */
  record Measure(
      @Nullable String name,
      @Nullable NamedBound minimum,
      @Nullable NamedBound maximum,
      @Nullable Integer decimals,
      List<MeasureUnit> units,
      List<String> traits,
      @Nullable Map<String, Integer> decomposition,
      @Nullable String unit,
      String help,
      TypeExtends extendsType)
      implements LemmaType {
    /** {@inheritDoc} */
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
      return read(p, TypeIdentity.NAMED);
    }

    static Measure read(JsonParser p, TypeIdentity identity) throws IOException {
      JsonReading.expectStartObject(p, "LemmaType.Measure");
      String name = null;
      boolean nameSeen = false;
      NamedBound minimum = null;
      boolean minimumSeen = false;
      NamedBound maximum = null;
      boolean maximumSeen = false;
      Integer decimals = null;
      boolean decimalsSeen = false;
      List<MeasureUnit> units = null;
      List<String> traits = null;
      Map<String, Integer> decomposition = null;
      boolean decompositionSeen = false;
      String unit = null;
      String help = null;
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
            minimum = NamedBound.readNullable(p);
          }
          case "maximum" -> {
            maximumSeen = true;
            maximum = NamedBound.readNullable(p);
          }
          case "decimals" -> {
            decimalsSeen = true;
            decimals = JsonReading.readInt(p);
          }
          case "units" -> units = JsonReading.readList(p, MeasureUnit::read);
          case "traits" -> traits = JsonReading.readList(p, JsonReading::readString);
          case "decomposition" -> {
            decompositionSeen = true;
            decomposition =
                p.currentToken() == JsonToken.VALUE_NULL
                    ? null
                    : JsonReading.readMap(p, JsonReading::readInt);
          }
          case "unit" -> unit = JsonReading.readString(p);
          case "help" -> help = JsonReading.readString(p);
          case "extends" -> extendsType = TypeExtends.read(p);
          default -> JsonReading.unknownField(field, "LemmaType.Measure");
        }
      }
      name = identityName(nameSeen, name, identity, "LemmaType.Measure");
      if (!minimumSeen) {
        JsonReading.missingRequired("minimum", "LemmaType.Measure");
      }
      if (!maximumSeen) {
        JsonReading.missingRequired("maximum", "LemmaType.Measure");
      }
      if (!decimalsSeen) {
        JsonReading.missingRequired("decimals", "LemmaType.Measure");
      }
      if (units == null) {
        JsonReading.missingRequired("units", "LemmaType.Measure");
      }
      if (traits == null) {
        JsonReading.missingRequired("traits", "LemmaType.Measure");
      }
      if (!decompositionSeen) {
        JsonReading.missingRequired("decomposition", "LemmaType.Measure");
      }
      if (help == null) {
        JsonReading.missingRequired("help", "LemmaType.Measure");
      }
      extendsType = identityExtends(extendsType, identity, "LemmaType.Measure");
      return new Measure(
          name, minimum, maximum, decimals, units, traits, decomposition, unit, help, extendsType);
    }
  }

  /**
   * NumberType.
   * @param name name
   * @param minimum minimum
   * @param maximum maximum
   * @param decimals decimals
   * @param help help
   * @param extendsType extendsType
   */
  record NumberType(
      @Nullable String name,
      @Nullable BigDecimal minimum,
      @Nullable BigDecimal maximum,
      @Nullable Integer decimals,
      String help,
      TypeExtends extendsType)
      implements LemmaType {
    /** {@inheritDoc} */
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
      return read(p, TypeIdentity.NAMED);
    }

    static NumberType read(JsonParser p, TypeIdentity identity) throws IOException {
      JsonReading.expectStartObject(p, "LemmaType.Number");
      String name = null;
      boolean nameSeen = false;
      BigDecimal minimum = null;
      boolean minimumSeen = false;
      BigDecimal maximum = null;
      boolean maximumSeen = false;
      Integer decimals = null;
      boolean decimalsSeen = false;
      String help = null;
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
          case "help" -> help = JsonReading.readString(p);
          case "extends" -> extendsType = TypeExtends.read(p);
          default -> JsonReading.unknownField(field, "LemmaType.Number");
        }
      }
      name = identityName(nameSeen, name, identity, "LemmaType.Number");
      if (!minimumSeen) {
        JsonReading.missingRequired("minimum", "LemmaType.Number");
      }
      if (!maximumSeen) {
        JsonReading.missingRequired("maximum", "LemmaType.Number");
      }
      if (!decimalsSeen) {
        JsonReading.missingRequired("decimals", "LemmaType.Number");
      }
      if (help == null) {
        JsonReading.missingRequired("help", "LemmaType.Number");
      }
      extendsType = identityExtends(extendsType, identity, "LemmaType.Number");
      return new NumberType(name, minimum, maximum, decimals, help, extendsType);
    }
  }

  /**
   * NumberRange.
   * @param name name
   * @param lower lower
   * @param upper upper
   * @param minimum minimum
   * @param maximum maximum
   * @param help help
   * @param extendsType extendsType
   */
  record NumberRange(
      @Nullable String name,
      @Nullable BigDecimal lower,
      @Nullable BigDecimal upper,
      @Nullable BigDecimal minimum,
      @Nullable BigDecimal maximum,
      String help,
      TypeExtends extendsType)
      implements LemmaType {
    /** {@inheritDoc} */
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
      return read(p, TypeIdentity.NAMED);
    }

    static NumberRange read(JsonParser p, TypeIdentity identity) throws IOException {
      JsonReading.expectStartObject(p, "LemmaType.NumberRange");
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
      String help = null;
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
          case "help" -> help = JsonReading.readString(p);
          case "extends" -> extendsType = TypeExtends.read(p);
          default -> JsonReading.unknownField(field, "LemmaType.NumberRange");
        }
      }
      name = identityName(nameSeen, name, identity, "LemmaType.NumberRange");
      if (!lowerSeen) {
        JsonReading.missingRequired("lower", "LemmaType.NumberRange");
      }
      if (!upperSeen) {
        JsonReading.missingRequired("upper", "LemmaType.NumberRange");
      }
      if (!minimumSeen) {
        JsonReading.missingRequired("minimum", "LemmaType.NumberRange");
      }
      if (!maximumSeen) {
        JsonReading.missingRequired("maximum", "LemmaType.NumberRange");
      }
      if (help == null) {
        JsonReading.missingRequired("help", "LemmaType.NumberRange");
      }
      extendsType = identityExtends(extendsType, identity, "LemmaType.NumberRange");
      return new NumberRange(name, lower, upper, minimum, maximum, help, extendsType);
    }
  }

  /**
   * Ratio.
   * @param name name
   * @param minimum minimum
   * @param maximum maximum
   * @param decimals decimals
   * @param units units
   * @param unit written unit when describing a value, or null
   * @param help help
   * @param extendsType extendsType
   */
  record Ratio(
      @Nullable String name,
      @Nullable BigDecimal minimum,
      @Nullable BigDecimal maximum,
      @Nullable Integer decimals,
      List<RatioUnit> units,
      @Nullable String unit,
      String help,
      TypeExtends extendsType)
      implements LemmaType {
    /** {@inheritDoc} */
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
      return read(p, TypeIdentity.NAMED);
    }

    static Ratio read(JsonParser p, TypeIdentity identity) throws IOException {
      JsonReading.expectStartObject(p, "LemmaType.Ratio");
      String name = null;
      boolean nameSeen = false;
      BigDecimal minimum = null;
      boolean minimumSeen = false;
      BigDecimal maximum = null;
      boolean maximumSeen = false;
      Integer decimals = null;
      boolean decimalsSeen = false;
      List<RatioUnit> units = null;
      String unit = null;
      String help = null;
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
          case "units" -> units = JsonReading.readList(p, RatioUnit::read);
          case "unit" -> unit = JsonReading.readString(p);
          case "help" -> help = JsonReading.readString(p);
          case "extends" -> extendsType = TypeExtends.read(p);
          default -> JsonReading.unknownField(field, "LemmaType.Ratio");
        }
      }
      name = identityName(nameSeen, name, identity, "LemmaType.Ratio");
      if (!minimumSeen) {
        JsonReading.missingRequired("minimum", "LemmaType.Ratio");
      }
      if (!maximumSeen) {
        JsonReading.missingRequired("maximum", "LemmaType.Ratio");
      }
      if (!decimalsSeen) {
        JsonReading.missingRequired("decimals", "LemmaType.Ratio");
      }
      if (units == null) {
        JsonReading.missingRequired("units", "LemmaType.Ratio");
      }
      if (help == null) {
        JsonReading.missingRequired("help", "LemmaType.Ratio");
      }
      extendsType = identityExtends(extendsType, identity, "LemmaType.Ratio");
      return new Ratio(name, minimum, maximum, decimals, units, unit, help, extendsType);
    }
  }

  /**
   * RatioRange.
   * @param name name
   * @param lower lower
   * @param upper upper
   * @param minimum minimum
   * @param maximum maximum
   * @param units units
   * @param help help
   * @param extendsType extendsType
   */
  record RatioRange(
      @Nullable String name,
      @Nullable BigDecimal lower,
      @Nullable BigDecimal upper,
      @Nullable BigDecimal minimum,
      @Nullable BigDecimal maximum,
      List<RatioUnit> units,
      String help,
      TypeExtends extendsType)
      implements LemmaType {
    /** {@inheritDoc} */
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
      return read(p, TypeIdentity.NAMED);
    }

    static RatioRange read(JsonParser p, TypeIdentity identity) throws IOException {
      JsonReading.expectStartObject(p, "LemmaType.RatioRange");
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
      List<RatioUnit> units = null;
      String help = null;
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
          case "units" -> units = JsonReading.readList(p, RatioUnit::read);
          case "help" -> help = JsonReading.readString(p);
          case "extends" -> extendsType = TypeExtends.read(p);
          default -> JsonReading.unknownField(field, "LemmaType.RatioRange");
        }
      }
      name = identityName(nameSeen, name, identity, "LemmaType.RatioRange");
      if (!lowerSeen) {
        JsonReading.missingRequired("lower", "LemmaType.RatioRange");
      }
      if (!upperSeen) {
        JsonReading.missingRequired("upper", "LemmaType.RatioRange");
      }
      if (!minimumSeen) {
        JsonReading.missingRequired("minimum", "LemmaType.RatioRange");
      }
      if (!maximumSeen) {
        JsonReading.missingRequired("maximum", "LemmaType.RatioRange");
      }
      if (units == null) {
        JsonReading.missingRequired("units", "LemmaType.RatioRange");
      }
      if (help == null) {
        JsonReading.missingRequired("help", "LemmaType.RatioRange");
      }
      extendsType = identityExtends(extendsType, identity, "LemmaType.RatioRange");
      return new RatioRange(name, lower, upper, minimum, maximum, units, help, extendsType);
    }
  }

  /**
   * Text.
   * @param name name
   * @param length length
   * @param options options
   * @param help help
   * @param extendsType extendsType
   */
  record Text(
      @Nullable String name,
      @Nullable Integer length,
      List<String> options,
      String help,
      TypeExtends extendsType)
      implements LemmaType {
    /** {@inheritDoc} */
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
      return read(p, TypeIdentity.NAMED);
    }

    static Text read(JsonParser p, TypeIdentity identity) throws IOException {
      JsonReading.expectStartObject(p, "LemmaType.Text");
      String name = null;
      boolean nameSeen = false;
      Integer length = null;
      boolean lengthSeen = false;
      List<String> options = null;
      String help = null;
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
          case "length" -> {
            lengthSeen = true;
            length = JsonReading.readInt(p);
          }
          case "options" -> options = JsonReading.readList(p, JsonReading::readString);
          case "help" -> help = JsonReading.readString(p);
          case "extends" -> extendsType = TypeExtends.read(p);
          default -> JsonReading.unknownField(field, "LemmaType.Text");
        }
      }
      name = identityName(nameSeen, name, identity, "LemmaType.Text");
      if (!lengthSeen) {
        JsonReading.missingRequired("length", "LemmaType.Text");
      }
      if (options == null) {
        JsonReading.missingRequired("options", "LemmaType.Text");
      }
      if (help == null) {
        JsonReading.missingRequired("help", "LemmaType.Text");
      }
      extendsType = identityExtends(extendsType, identity, "LemmaType.Text");
      return new Text(name, length, options, help, extendsType);
    }
  }

  /**
   * DateType.
   * @param name name
   * @param minimum minimum
   * @param maximum maximum
   * @param help help
   * @param extendsType extendsType
   */
  record DateType(
      @Nullable String name,
      @Nullable String minimum,
      @Nullable String maximum,
      String help,
      TypeExtends extendsType)
      implements LemmaType {
    /** {@inheritDoc} */
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
      return read(p, TypeIdentity.NAMED);
    }

    static DateType read(JsonParser p, TypeIdentity identity) throws IOException {
      return readStringBounds(p, "date", "LemmaType.Date", identity);
    }
  }

  /**
   * TimeType.
   * @param name name
   * @param minimum minimum
   * @param maximum maximum
   * @param help help
   * @param extendsType extendsType
   */
  record TimeType(
      @Nullable String name,
      @Nullable String minimum,
      @Nullable String maximum,
      String help,
      TypeExtends extendsType)
      implements LemmaType {
    /** {@inheritDoc} */
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
      return read(p, TypeIdentity.NAMED);
    }

    static TimeType read(JsonParser p, TypeIdentity identity) throws IOException {
      var r = readStringBounds(p, "time", "LemmaType.Time", identity);
      return new TimeType(r.name(), r.minimum(), r.maximum(), r.help(), r.extendsType());
    }
  }

  private static DateType readStringBounds(JsonParser p, String kind, String typeName, TypeIdentity identity)
      throws IOException {
    JsonReading.expectStartObject(p, typeName);
    String name = null;
    boolean nameSeen = false;
    String minimum = null;
    boolean minimumSeen = false;
    String maximum = null;
    boolean maximumSeen = false;
    String help = null;
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
        case "help" -> help = JsonReading.readString(p);
        case "extends" -> extendsType = TypeExtends.read(p);
        default -> JsonReading.unknownField(field, typeName);
      }
    }
    if (!maximumSeen) {
      JsonReading.missingRequired("maximum", typeName);
    }
    if (help == null) {
      JsonReading.missingRequired("help", typeName);
    }
    name = identityName(nameSeen, name, identity, typeName);
    extendsType = identityExtends(extendsType, identity, typeName);
    return new DateType(name, minimum, maximum, help, extendsType);
  }

  /**
   * DateRange.
   * @param name name
   * @param lower lower
   * @param upper upper
   * @param minimum minimum
   * @param maximum maximum
   * @param help help
   * @param extendsType extendsType
   */
  record DateRange(
      @Nullable String name,
      @Nullable String lower,
      @Nullable String upper,
      @Nullable NamedBound minimum,
      @Nullable NamedBound maximum,
      String help,
      TypeExtends extendsType)
      implements LemmaType {
    /** {@inheritDoc} */
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
      return read(p, TypeIdentity.NAMED);
    }

    static DateRange read(JsonParser p, TypeIdentity identity) throws IOException {
      return readCalendarRange(p, "date_range", "LemmaType.DateRange", identity);
    }
  }

  /**
   * TimeRange.
   * @param name name
   * @param lower lower
   * @param upper upper
   * @param minimum minimum
   * @param maximum maximum
   * @param help help
   * @param extendsType extendsType
   */
  record TimeRange(
      @Nullable String name,
      @Nullable String lower,
      @Nullable String upper,
      @Nullable NamedBound minimum,
      @Nullable NamedBound maximum,
      String help,
      TypeExtends extendsType)
      implements LemmaType {
    /** {@inheritDoc} */
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
      return read(p, TypeIdentity.NAMED);
    }

    static TimeRange read(JsonParser p, TypeIdentity identity) throws IOException {
      var r = readCalendarRange(p, "time_range", "LemmaType.TimeRange", identity);
      return new TimeRange(
          r.name(), r.lower(), r.upper(), r.minimum(), r.maximum(), r.help(), r.extendsType());
    }
  }

  private static DateRange readCalendarRange(
      JsonParser p, String kind, String typeName, TypeIdentity identity)
      throws IOException {
    JsonReading.expectStartObject(p, typeName);
    String name = null;
    boolean nameSeen = false;
    String lower = null;
    boolean lowerSeen = false;
    String upper = null;
    boolean upperSeen = false;
    NamedBound minimum = null;
    boolean minimumSeen = false;
    NamedBound maximum = null;
    boolean maximumSeen = false;
    String help = null;
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
          minimum = NamedBound.readNullable(p);
        }
        case "maximum" -> {
          maximumSeen = true;
          maximum = NamedBound.readNullable(p);
        }
        case "help" -> help = JsonReading.readString(p);
        case "extends" -> extendsType = TypeExtends.read(p);
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
    if (help == null) {
      JsonReading.missingRequired("help", typeName);
    }
    name = identityName(nameSeen, name, identity, typeName);
    extendsType = identityExtends(extendsType, identity, typeName);
    return new DateRange(name, lower, upper, minimum, maximum, help, extendsType);
  }

  /**
   * MeasureRange.
   * @param name name
   * @param lower lower
   * @param upper upper
   * @param minimum minimum
   * @param maximum maximum
   * @param units units
   * @param decomposition decomposition
   * @param help help
   * @param extendsType extendsType
   */
  record MeasureRange(
      @Nullable String name,
      @Nullable NamedBound lower,
      @Nullable NamedBound upper,
      @Nullable NamedBound minimum,
      @Nullable NamedBound maximum,
      List<MeasureUnit> units,
      @Nullable Map<String, Integer> decomposition,
      String help,
      TypeExtends extendsType)
      implements LemmaType {
    /** {@inheritDoc} */
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
      return read(p, TypeIdentity.NAMED);
    }

    static MeasureRange read(JsonParser p, TypeIdentity identity) throws IOException {
      JsonReading.expectStartObject(p, "LemmaType.MeasureRange");
      String name = null;
      boolean nameSeen = false;
      NamedBound lower = null;
      boolean lowerSeen = false;
      NamedBound upper = null;
      boolean upperSeen = false;
      NamedBound minimum = null;
      boolean minimumSeen = false;
      NamedBound maximum = null;
      boolean maximumSeen = false;
      List<MeasureUnit> units = null;
      Map<String, Integer> decomposition = null;
      boolean decompositionSeen = false;
      String help = null;
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
            lower = NamedBound.readNullable(p);
          }
          case "upper" -> {
            upperSeen = true;
            upper = NamedBound.readNullable(p);
          }
          case "minimum" -> {
            minimumSeen = true;
            minimum = NamedBound.readNullable(p);
          }
          case "maximum" -> {
            maximumSeen = true;
            maximum = NamedBound.readNullable(p);
          }
          case "units" -> units = JsonReading.readList(p, MeasureUnit::read);
          case "decomposition" -> {
            decompositionSeen = true;
            decomposition =
                p.currentToken() == JsonToken.VALUE_NULL
                    ? null
                    : JsonReading.readMap(p, JsonReading::readInt);
          }
          case "help" -> help = JsonReading.readString(p);
          case "extends" -> extendsType = TypeExtends.read(p);
          default -> JsonReading.unknownField(field, "LemmaType.MeasureRange");
        }
      }
      name = identityName(nameSeen, name, identity, "LemmaType.MeasureRange");
      if (!lowerSeen) {
        JsonReading.missingRequired("lower", "LemmaType.MeasureRange");
      }
      if (!upperSeen) {
        JsonReading.missingRequired("upper", "LemmaType.MeasureRange");
      }
      if (!minimumSeen) {
        JsonReading.missingRequired("minimum", "LemmaType.MeasureRange");
      }
      if (!maximumSeen) {
        JsonReading.missingRequired("maximum", "LemmaType.MeasureRange");
      }
      if (units == null) {
        JsonReading.missingRequired("units", "LemmaType.MeasureRange");
      }
      if (!decompositionSeen) {
        JsonReading.missingRequired("decomposition", "LemmaType.MeasureRange");
      }
      if (help == null) {
        JsonReading.missingRequired("help", "LemmaType.MeasureRange");
      }
      extendsType = identityExtends(extendsType, identity, "LemmaType.MeasureRange");
      return new MeasureRange(
          name, lower, upper, minimum, maximum, units, decomposition, help, extendsType);
    }
  }

  /**
   * Spec instance type.
   *
   * @param name name
   * @param repository repository or null for the unnamed workspace
   * @param spec spec name
   * @param effective resolved row effective date, empty for the origin row
   * @param extendsType extends
   */
  record Spec(
      @Nullable String name,
      @Nullable String repository,
      String spec,
      String effective,
      TypeExtends extendsType)
      implements LemmaType {
    /** {@inheritDoc} */
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
      return read(p, TypeIdentity.NAMED);
    }

    static Spec read(JsonParser p, TypeIdentity identity) throws IOException {
      JsonReading.expectStartObject(p, "LemmaType.Spec");
      String name = null;
      boolean nameSeen = false;
      String repository = null;
      String spec = null;
      String effective = null;
      TypeExtends extendsType = null;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "name" -> {
            nameSeen = true;
            name = JsonReading.readString(p);
          }
          case "kind" -> expectKind(p, "spec");
          case "repository" -> repository = JsonReading.readString(p);
          case "spec" -> spec = JsonReading.readString(p);
          case "effective" -> effective = JsonReading.readString(p);
          case "extends" -> extendsType = TypeExtends.read(p);
          default -> JsonReading.unknownField(field, "LemmaType.Spec");
        }
      }
      name = identityName(nameSeen, name, identity, "LemmaType.Spec");
      if (spec == null) {
        JsonReading.missingRequired("spec", "LemmaType.Spec");
      }
      if (effective == null) {
        JsonReading.missingRequired("effective", "LemmaType.Spec");
      }
      extendsType = identityExtends(extendsType, identity, "LemmaType.Spec");
      return new Spec(name, repository, spec, effective, extendsType);
    }
  }

  /** How a list addresses its rows. */
  sealed interface ListKey {
    /** Rows addressed by 1-based position. */
    record Numbered() implements ListKey {}

    /**
     * Rows addressed by the values of key fields.
     *
     * @param fields key field names in declaration order
     */
    record Fields(List<String> fields) implements ListKey {}

    /**
     * Parses JSON: {@code "numbered"} or {@code {"fields": [...]}}.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    static ListKey read(JsonParser p) throws IOException {
      if (p.currentToken() == JsonToken.VALUE_STRING) {
        String tag = p.getText();
        if (!"numbered".equals(tag)) {
          throw new LemmaBugError("BUG: unknown ListKey '" + tag + "'");
        }
        return new Numbered();
      }
      JsonReading.expectStartObject(p, "ListKey");
      List<String> fields = null;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "fields" -> fields = JsonReading.readList(p, JsonReading::readString);
          default -> JsonReading.unknownField(field, "ListKey");
        }
      }
      if (fields == null || fields.isEmpty()) {
        JsonReading.missingRequired("fields", "ListKey");
      }
      return new Fields(List.copyOf(fields));
    }
  }

  /**
   * List type.
   *
   * @param name name
   * @param element row type: the unnamed primitive type of the element specification, a {@link
   *     Spec} for instance lists; never a list, lists do not nest
   * @param key how rows are addressed
   * @param extendsType extends
   */
  record ListType(@Nullable String name, LemmaType element, ListKey key, TypeExtends extendsType)
      implements LemmaType {
    /** {@inheritDoc} */
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
      return read(p, TypeIdentity.NAMED);
    }

    static ListType read(JsonParser p, TypeIdentity identity) throws IOException {
      JsonReading.expectStartObject(p, "LemmaType.ListType");
      String name = null;
      boolean nameSeen = false;
      LemmaType element = null;
      ListKey key = null;
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
          case "element" -> element = readSpecification(p);
          case "key" -> key = ListKey.read(p);
          case "extends" -> extendsType = TypeExtends.read(p);
          default -> JsonReading.unknownField(field, "LemmaType.ListType");
        }
      }
      name = identityName(nameSeen, name, identity, "LemmaType.ListType");
      if (element == null) {
        JsonReading.missingRequired("element", "LemmaType.ListType");
      }
      if (key == null) {
        JsonReading.missingRequired("key", "LemmaType.ListType");
      }
      extendsType = identityExtends(extendsType, identity, "LemmaType.ListType");
      return new ListType(name, element, key, extendsType);
    }
  }

  /**
   * Parses a bare type specification (a list element: kind and constraints, no name or extends)
   * as the unnamed primitive type with that specification.
   *
   * @param p parser at value start
   * @return parsed value
   * @throws IOException if JSON IO fails
   */
  private static LemmaType readSpecification(JsonParser p) throws IOException {
    JsonReading.expectStartObject(p, "TypeSpecification");
    String json = JsonReading.bufferObjectAsString(p);
    String kind = JsonReading.findTag(json, "kind");
    if (kind == null) {
      throw new LemmaBugError("BUG: missing 'kind' in TypeSpecification");
    }
    if ("list".equals(kind)) {
      throw new LemmaBugError("BUG: list element is a list: " + json);
    }
    try (JsonParser reader = JsonReading.parserFor(json)) {
      return switch (kind) {
        case "boolean" -> BooleanType.read(reader, TypeIdentity.SPECIFICATION);
        case "measure" -> Measure.read(reader, TypeIdentity.SPECIFICATION);
        case "number" -> NumberType.read(reader, TypeIdentity.SPECIFICATION);
        case "number_range" -> NumberRange.read(reader, TypeIdentity.SPECIFICATION);
        case "ratio" -> Ratio.read(reader, TypeIdentity.SPECIFICATION);
        case "ratio_range" -> RatioRange.read(reader, TypeIdentity.SPECIFICATION);
        case "text" -> Text.read(reader, TypeIdentity.SPECIFICATION);
        case "date" -> DateType.read(reader, TypeIdentity.SPECIFICATION);
        case "date_range" -> DateRange.read(reader, TypeIdentity.SPECIFICATION);
        case "time" -> TimeType.read(reader, TypeIdentity.SPECIFICATION);
        case "time_range" -> TimeRange.read(reader, TypeIdentity.SPECIFICATION);
        case "measure_range" -> MeasureRange.read(reader, TypeIdentity.SPECIFICATION);
        case "spec" -> Spec.read(reader, TypeIdentity.SPECIFICATION);
        default -> throw new LemmaBugError("BUG: unknown kind value: " + kind);
      };
    }
  }

  /**
   * Parses JSON.
   *
   * @param p parser at value start
   * @return parsed value
   * @throws IOException if JSON IO fails
   */
  public static LemmaType read(JsonParser p) throws IOException {
    JsonReading.expectStartObject(p, "LemmaType");
    String json = JsonReading.bufferObjectAsString(p);
    String kind = JsonReading.findTag(json, "kind");
    if (kind == null) {
      throw new LemmaBugError("BUG: missing 'kind' in LemmaType");
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
        case "list" -> ListType.read(reader);
        default -> throw new LemmaBugError("BUG: unknown kind value: " + kind);
      };
    }
  }
}

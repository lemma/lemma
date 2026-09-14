package com.lemmabase.lemma;

import com.fasterxml.jackson.core.JsonFactory;
import com.fasterxml.jackson.core.JsonGenerator;
import com.fasterxml.jackson.core.JsonParser;
import com.fasterxml.jackson.core.JsonToken;
import java.io.IOException;
import java.io.StringWriter;
import java.math.BigDecimal;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import org.jspecify.annotations.Nullable;

/**
 * JSON parse primitives shared by SDK wire types and {@code com.lemmabase.lemma.schema}.
 */
public final class JsonReading {
  /** Jackson parser factory. */
  public static final JsonFactory FACTORY = new JsonFactory();

  /** Prevents instantiation. */
  private JsonReading() {}

  /**
   * Reads one JSON value from a parser.
   *
   * @param <T> value type
   */
  @FunctionalInterface
  public interface Reader<T> {
    /**
     * Reads one value.
     *
     * @param p parser positioned at the value
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    T read(JsonParser p) throws IOException;
  }

  /**
   * Two-element tuple.
   *
   * @param <A> first type
   * @param <B> second type
   * @param first first element
   * @param second second element
   */
  public record Tuple2<A, B>(A first, B second) {}

  /**
   * Reads a JSON array with {@code reader} per element.
   *
   * @param <T> element type
   * @param p parser at {@code START_ARRAY}
   * @param reader element reader
   * @return element list
   * @throws IOException if JSON IO fails
   */
  public static <T> List<T> readList(JsonParser p, Reader<T> reader) throws IOException {
    if (p.currentToken() != JsonToken.START_ARRAY) {
      throw new LemmaBugError("BUG: expected START_ARRAY");
    }
    List<T> list = new ArrayList<>();
    while (p.nextToken() != JsonToken.END_ARRAY) {
      list.add(reader.read(p));
    }
    return list;
  }

  /**
   * Reads a JSON object map with {@code reader} per value.
   *
   * @param <T> value type
   * @param p parser at {@code START_OBJECT}
   * @param reader value reader
   * @return map of field names to values
   * @throws IOException if JSON IO fails
   */
  public static <T> Map<String, T> readMap(JsonParser p, Reader<T> reader) throws IOException {
    if (p.currentToken() != JsonToken.START_OBJECT) {
      throw new LemmaBugError("BUG: expected START_OBJECT");
    }
    Map<String, T> map = new LinkedHashMap<>();
    while (p.nextToken() != JsonToken.END_OBJECT) {
      String key = p.currentName();
      p.nextToken();
      map.put(key, reader.read(p));
    }
    return map;
  }

  /**
   * Reads a JSON number as {@link BigDecimal}.
   *
   * @param p parser at a numeric token
   * @return decimal value
   * @throws IOException if JSON IO fails
   */
  public static BigDecimal readDecimal(JsonParser p) throws IOException {
    String text = p.getText();
    try {
      return new BigDecimal(text);
    } catch (NumberFormatException e) {
      throw new LemmaBugError("BUG: invalid decimal '" + text + "'");
    }
  }

  /**
   * Reads a JSON string or null.
   *
   * @param p parser at string or null token
   * @return string or null
   * @throws IOException if JSON IO fails
   */
  public static @Nullable String readString(JsonParser p) throws IOException {
    if (p.currentToken() == JsonToken.VALUE_NULL) {
      return null;
    }
    return p.getText();
  }

  /**
   * Reads a JSON integer or null.
   *
   * @param p parser at integer or null token
   * @return integer or null
   * @throws IOException if JSON IO fails
   */
  public static @Nullable Integer readInt(JsonParser p) throws IOException {
    if (p.currentToken() == JsonToken.VALUE_NULL) {
      return null;
    }
    return p.getIntValue();
  }

  /**
   * Reads a JSON boolean.
   *
   * @param p parser at boolean token
   * @return boolean value
   * @throws IOException if JSON IO fails
   */
  public static boolean readBoolean(JsonParser p) throws IOException {
    return p.getBooleanValue();
  }

  /**
   * Reads a two-element JSON array tuple.
   *
   * @param <A> first element type
   * @param <B> second element type
   * @param p parser at {@code START_ARRAY}
   * @param ra first element reader
   * @param rb second element reader
   * @return two-element tuple
   * @throws IOException if JSON IO fails
   */
  public static <A, B> Tuple2<A, B> readTuple2(JsonParser p, Reader<A> ra, Reader<B> rb)
      throws IOException {
    if (p.currentToken() != JsonToken.START_ARRAY) {
      throw new LemmaBugError("BUG: expected START_ARRAY for tuple");
    }
    p.nextToken();
    A a = ra.read(p);
    p.nextToken();
    B b = rb.read(p);
    p.nextToken();
    if (p.currentToken() != JsonToken.END_ARRAY) {
      throw new LemmaBugError("BUG: expected END_ARRAY after tuple");
    }
    return new Tuple2<>(a, b);
  }

  /**
   * Copies the current JSON object to a string.
   *
   * @param p parser inside an object
   * @return JSON object text
   * @throws IOException if JSON IO fails
   */
  public static String bufferObjectAsString(JsonParser p) throws IOException {
    return bufferCurrentAsString(p);
  }

  /**
   * Copies the current JSON value (object, array, or scalar) to a string.
   *
   * @param p parser at the value
   * @return JSON text
   * @throws IOException if JSON IO fails
   */
  public static String bufferCurrentAsString(JsonParser p) throws IOException {
    StringWriter sw = new StringWriter();
    try (JsonGenerator g = FACTORY.createGenerator(sw)) {
      g.copyCurrentStructure(p);
    }
    return sw.toString();
  }

  /**
   * Creates a parser for a JSON document.
   *
   * @param json JSON text
   * @return parser advanced to the first token
   * @throws IOException if JSON IO fails
   */
  public static JsonParser parserFor(String json) throws IOException {
    JsonParser p = FACTORY.createParser(json);
    p.nextToken();
    return p;
  }

  /**
   * Reads one JSON value as a generic Java object ({@link Map}, {@link List}, string, number,
   * boolean, or {@code null}).
   *
   * @param p parser at the value
   * @return parsed value
   * @throws IOException if JSON IO fails
   */
  public static @Nullable Object readJsonValue(JsonParser p) throws IOException {
    JsonToken token = p.currentToken();
    if (token == null) {
      throw new LemmaBugError("BUG: expected JSON value");
    }
    return switch (token) {
      case START_OBJECT -> readMap(p, JsonReading::readJsonValue);
      case START_ARRAY -> readList(p, JsonReading::readJsonValue);
      case VALUE_STRING -> p.getText();
      case VALUE_NUMBER_INT, VALUE_NUMBER_FLOAT -> new BigDecimal(p.getText());
      case VALUE_TRUE -> Boolean.TRUE;
      case VALUE_FALSE -> Boolean.FALSE;
      case VALUE_NULL -> null;
      default -> throw new LemmaBugError("BUG: unexpected token " + token + " for JSON value");
    };
  }

  /**
   * Finds a string field value in a shallow JSON object.
   *
   * @param json JSON object text
   * @param tagField field name to read
   * @return field value or null when absent
   * @throws IOException if JSON IO fails
   */
  public static @Nullable String findTag(String json, String tagField) throws IOException {
    try (JsonParser p = parserFor(json)) {
      if (p.currentToken() != JsonToken.START_OBJECT) {
        throw new LemmaBugError("BUG: expected START_OBJECT for findTag");
      }
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        if (tagField.equals(field)) {
          return p.getText();
        }
        p.skipChildren();
      }
    }
    return null;
  }

  /**
   * Requires the parser to be at {@code START_OBJECT}.
   *
   * @param p parser to check
   * @param typeName type name for error messages
   * @throws IOException if JSON IO fails
   */
  public static void expectStartObject(JsonParser p, String typeName) throws IOException {
    if (p.currentToken() != JsonToken.START_OBJECT) {
      throw new LemmaBugError("BUG: expected START_OBJECT for " + typeName);
    }
  }

  /**
   * Reports an unknown JSON field.
   *
   * @param field field name
   * @param typeName type being parsed
   */
  public static void unknownField(String field, String typeName) {
    throw new LemmaBugError("BUG: unknown field '" + field + "' in " + typeName);
  }

  /**
   * Reports a missing required JSON field.
   *
   * @param field field name
   * @param typeName type being parsed
   */
  public static void missingRequired(String field, String typeName) {
    throw new LemmaBugError("BUG: missing required field '" + field + "' in " + typeName);
  }

  /**
   * Reads an ISO local date (or datetime truncated to date).
   *
   * @param p parser at a string token
   * @return local date
   * @throws IOException if JSON IO fails
   */
  public static java.time.LocalDate readLocalDate(JsonParser p) throws IOException {
    String raw = readString(p);
    try {
      if (raw.indexOf('T') >= 0) {
        return java.time.LocalDateTime.parse(raw).toLocalDate();
      }
      return java.time.LocalDate.parse(raw);
    } catch (java.time.format.DateTimeParseException e) {
      throw new LemmaBugError("BUG: invalid date '" + raw + "': " + e.getMessage());
    }
  }

  /**
   * Reads an ISO local time (offset times reduce to local time).
   *
   * @param p parser at a string token
   * @return local time
   * @throws IOException if JSON IO fails
   */
  public static java.time.LocalTime readLocalTime(JsonParser p) throws IOException {
    String raw = readString(p);
    try {
      return java.time.LocalTime.parse(raw);
    } catch (java.time.format.DateTimeParseException ignored) {
      try {
        return java.time.OffsetTime.parse(raw).toLocalTime();
      } catch (java.time.format.DateTimeParseException e) {
        throw new LemmaBugError("BUG: invalid time '" + raw + "': " + e.getMessage());
      }
    }
  }
}

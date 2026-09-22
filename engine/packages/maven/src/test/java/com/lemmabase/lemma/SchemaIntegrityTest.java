package com.lemmabase.lemma;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.junit.jupiter.api.Assertions.fail;

import com.fasterxml.jackson.core.JsonFactory;
import com.fasterxml.jackson.core.JsonParser;
import com.fasterxml.jackson.core.JsonToken;
import com.lemmabase.lemma.schema.ExplanationNode;
import com.lemmabase.lemma.schema.LemmaType;
import com.lemmabase.lemma.schema.LiteralValue;
import com.lemmabase.lemma.schema.TypeExtends;
import java.math.BigDecimal;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.stream.Collectors;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

/**
 * Sealed API discriminators must match {@code engine/schemas/api.v1.json} {@code $defs}.
 * Extracts {@code const} / external-tag property names only — not a full schema walker.
 */
final class SchemaIntegrityTest {
  private static final JsonFactory FACTORY = new JsonFactory();

  private static Map<String, Json> defs;

  @BeforeAll
  static void loadSchema() throws Exception {
    Path schema = schemaPath();
    assertTrue(Files.isRegularFile(schema), "api.v1.json missing at " + schema);
    try (JsonParser p = FACTORY.createParser(Files.readString(schema))) {
      p.nextToken();
      Json root = Json.read(p);
      Json defsNode = root.object().get("$defs");
      if (defsNode == null) {
        fail("BUG: api.v1.json missing $defs");
      }
      defs = defsNode.object();
    }
  }

  private static Path schemaPath() {
    Path moduleDir = Path.of(System.getProperty("user.dir")).toAbsolutePath().normalize();
    return moduleDir.resolve("../../schemas/api.v1.json").normalize();
  }

  @Test
  void lemmaTypeKindsMatchSchema() throws Exception {
    Set<String> schemaKinds = refOneOfConsts(defs.get("LemmaType"), "kind");
    Map<String, LemmaType> byKind = lemmaTypesFromFixture();
    assertEquals(schemaKinds, byKind.keySet());

    Set<String> permitted =
        Arrays.stream(LemmaType.class.getPermittedSubclasses())
            .map(Class::getSimpleName)
            .collect(Collectors.toSet());
    Set<String> instanceClasses =
        byKind.values().stream()
            .map(t -> t.getClass().getSimpleName())
            .collect(Collectors.toSet());
    assertEquals(permitted, instanceClasses);

    for (Map.Entry<String, LemmaType> e : byKind.entrySet()) {
      assertEquals(e.getKey(), e.getValue().kind());
    }
  }

  @Test
  void explanationNodeTypesMatchSchema() {
    Set<String> schemaTypes = refOneOfConsts(defs.get("ExplanationNode"), "type");
    Map<String, ExplanationNode> samples =
        Map.of(
            "rule",
                new ExplanationNode.Rule(
                    "n", new RuleResultValue.ResultOnly("d"), "b", null, null),
            "compose", new ExplanationNode.Compose("e", null, List.of()),
            "data", new ExplanationNode.Data("n", new RuleResultValue.ResultOnly("d")),
            "data_unused", new ExplanationNode.DataUnused("n"),
            "conversion", new ExplanationNode.Conversion("e", List.of(), List.of()),
            "veto", new ExplanationNode.Veto(null));
    assertEquals(schemaTypes, samples.keySet());

    Set<String> permitted =
        Arrays.stream(ExplanationNode.class.getPermittedSubclasses())
            .map(Class::getSimpleName)
            .collect(Collectors.toSet());
    Set<String> instanceClasses =
        samples.values().stream()
            .map(n -> n.getClass().getSimpleName())
            .collect(Collectors.toSet());
    assertEquals(permitted, instanceClasses);

    for (Map.Entry<String, ExplanationNode> e : samples.entrySet()) {
      assertEquals(e.getKey(), e.getValue().type());
    }
  }

  @Test
  void sourceTypeVariantsMatchSchema() {
    Set<String> schemaTags = externalOneOfTags(defs.get("SourceType"));
    assertEquals(Set.of("volatile", "path", "dependency"), schemaTags);

    Map<String, Class<?>> tagToClass =
        Map.of(
            "volatile", SourceType.Volatile.class,
            "path", SourceType.Path.class,
            "dependency", SourceType.Dependency.class);
    assertEquals(
        schemaTags.stream().map(tagToClass::get).map(Class::getSimpleName).collect(Collectors.toSet()),
        Arrays.stream(SourceType.class.getPermittedSubclasses())
            .map(Class::getSimpleName)
            .collect(Collectors.toSet()));
  }

  @Test
  void typeExtendsKindsMatchSchema() {
    Set<String> extendsKinds = inlineOneOfConsts(defs.get("TypeExtends"), "kind");
    Set<String> definingKinds = inlineOneOfConsts(defs.get("TypeDefiningSpec"), "kind");
    assertEquals(Set.of("primitive", "custom"), extendsKinds);
    assertEquals(Set.of("local", "import"), definingKinds);

    assertEquals(
        Set.of("Primitive", "Custom"),
        Arrays.stream(TypeExtends.class.getPermittedSubclasses())
            .map(Class::getSimpleName)
            .collect(Collectors.toSet()));
    assertEquals(
        Set.of("Local", "Import"),
        Arrays.stream(TypeExtends.TypeDefiningSpec.class.getPermittedSubclasses())
            .map(Class::getSimpleName)
            .collect(Collectors.toSet()));

    assertEquals("primitive", new TypeExtends.Primitive().kind());
    assertEquals(
        "custom",
        new TypeExtends.Custom("number", "amount", new TypeExtends.TypeDefiningSpec.Local())
            .kind());
    assertEquals("local", new TypeExtends.TypeDefiningSpec.Local().kind());
    assertEquals("import", new TypeExtends.TypeDefiningSpec.Import().kind());
  }

  @Test
  void literalValueVariantsMatchSchema() {
    Set<String> literalTags = externalOneOfTags(defs.get("LiteralValue"));
    assertEquals(
        Set.of("number", "number_with_unit", "text", "date", "time", "boolean", "range"),
        literalTags);

    Map<String, String> literalTagToClass =
        Map.of(
            "number", "Number",
            "number_with_unit", "NumberWithUnit",
            "text", "Text",
            "date", "Date",
            "time", "Time",
            "boolean", "BooleanLit",
            "range", "Range");
    assertEquals(
        literalTags.stream().map(literalTagToClass::get).collect(Collectors.toSet()),
        Arrays.stream(LiteralValue.class.getPermittedSubclasses())
            .map(Class::getSimpleName)
            .collect(Collectors.toSet()));
  }

  private static Map<String, LemmaType> lemmaTypesFromFixture() throws Exception {
    Path fixture =
        Path.of(System.getProperty("user.dir"))
            .toAbsolutePath()
            .normalize()
            .resolve("../../tests/fixtures/api/type_specification_kinds.json")
            .normalize();
    Map<String, LemmaType> byKind = new HashMap<>();
    try (JsonParser p = FACTORY.createParser(Files.readString(fixture))) {
      p.nextToken();
      while (p.nextToken() != JsonToken.END_OBJECT) {
        p.nextToken();
        LemmaType type = LemmaType.read(p);
        byKind.putIfAbsent(type.kind(), type);
      }
    }
    return byKind;
  }

  /** LemmaType / ExplanationNode style: oneOf of {@code $ref} targets with {@code properties.X.const}. */
  private static Set<String> refOneOfConsts(Json def, String discriminatorField) {
    if (def == null) {
      fail("BUG: missing $def for ref oneOf extraction");
    }
    Json oneOf = def.object().get("oneOf");
    if (oneOf == null) {
      fail("BUG: $def missing oneOf");
    }
    Set<String> consts = new LinkedHashSet<>();
    for (Json item : oneOf.array()) {
      String ref = item.object().get("$ref").asString();
      if (!ref.startsWith("#/$defs/")) {
        fail("BUG: unexpected $ref " + ref);
      }
      String name = ref.substring("#/$defs/".length());
      Json target = defs.get(name);
      if (target == null) {
        fail("BUG: unresolved $ref " + ref);
      }
      consts.add(requireConst(target, discriminatorField));
    }
    return consts;
  }

  /** TypeExtends / TypeDefiningSpec style: inline oneOf objects with {@code properties.kind.const}. */
  private static Set<String> inlineOneOfConsts(Json def, String discriminatorField) {
    if (def == null) {
      fail("BUG: missing $def for inline oneOf extraction");
    }
    Json oneOf = def.object().get("oneOf");
    if (oneOf == null) {
      fail("BUG: $def missing oneOf");
    }
    Set<String> consts = new LinkedHashSet<>();
    for (Json item : oneOf.array()) {
      consts.add(requireConst(item, discriminatorField));
    }
    return consts;
  }

  /**
   * Externally tagged oneOf: each arm is either a string {@code const} (SourceType volatile) or an
   * object whose sole property name is the tag.
   */
  private static Set<String> externalOneOfTags(Json def) {
    if (def == null) {
      fail("BUG: missing $def for external tag extraction");
    }
    Json oneOf = def.object().get("oneOf");
    if (oneOf == null) {
      fail("BUG: $def missing oneOf");
    }
    Set<String> tags = new LinkedHashSet<>();
    for (Json item : oneOf.array()) {
      Map<String, Json> obj = item.object();
      if (obj.containsKey("const")) {
        tags.add(obj.get("const").asString());
        continue;
      }
      Json properties = obj.get("properties");
      if (properties == null) {
        fail("BUG: external oneOf arm missing properties and const");
      }
      Set<String> keys = properties.object().keySet();
      if (keys.size() != 1) {
        fail("BUG: expected exactly one external tag property, got " + keys);
      }
      tags.add(keys.iterator().next());
    }
    return tags;
  }

  private static String requireConst(Json schemaObject, String field) {
    Json properties = schemaObject.object().get("properties");
    if (properties == null) {
      fail("BUG: schema object missing properties for const field '" + field + "'");
    }
    Json prop = properties.object().get(field);
    if (prop == null) {
      fail("BUG: missing properties." + field);
    }
    Json c = prop.object().get("const");
    if (c == null) {
      fail("BUG: missing properties." + field + ".const");
    }
    return c.asString();
  }

  /** Minimal jackson-core JSON tree for test-local schema walking. */
  private sealed interface Json {
    default Map<String, Json> object() {
      if (!(this instanceof Obj o)) {
        fail("BUG: expected JSON object, got " + this.getClass().getSimpleName());
        throw new AssertionError("unreachable");
      }
      return o.fields;
    }

    default List<Json> array() {
      if (!(this instanceof Arr a)) {
        fail("BUG: expected JSON array, got " + this.getClass().getSimpleName());
        throw new AssertionError("unreachable");
      }
      return a.items;
    }

    default String asString() {
      if (!(this instanceof Str s)) {
        fail("BUG: expected JSON string, got " + this.getClass().getSimpleName());
        throw new AssertionError("unreachable");
      }
      return s.value;
    }

    static Json read(JsonParser p) throws Exception {
      JsonToken t = p.currentToken();
      if (t == null) {
        fail("BUG: null token while reading schema JSON");
      }
      return switch (t) {
        case START_OBJECT -> {
          Map<String, Json> fields = new LinkedHashMap<>();
          while (p.nextToken() != JsonToken.END_OBJECT) {
            String name = p.currentName();
            p.nextToken();
            fields.put(name, read(p));
          }
          yield new Obj(fields);
        }
        case START_ARRAY -> {
          List<Json> items = new ArrayList<>();
          while (p.nextToken() != JsonToken.END_ARRAY) {
            items.add(read(p));
          }
          yield new Arr(items);
        }
        case VALUE_STRING -> new Str(p.getText());
        case VALUE_NUMBER_INT, VALUE_NUMBER_FLOAT -> new Num(p.getDecimalValue());
        case VALUE_TRUE -> new Bool(true);
        case VALUE_FALSE -> new Bool(false);
        case VALUE_NULL -> new Null();
        default -> {
          fail("BUG: unexpected token " + t);
          throw new AssertionError("unreachable");
        }
      };
    }

    record Obj(Map<String, Json> fields) implements Json {}

    record Arr(List<Json> items) implements Json {}

    record Str(String value) implements Json {}

    record Num(BigDecimal value) implements Json {}

    record Bool(boolean value) implements Json {}

    record Null() implements Json {}
  }
}

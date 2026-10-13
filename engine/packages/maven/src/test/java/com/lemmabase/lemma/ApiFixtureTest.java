package com.lemmabase.lemma;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertInstanceOf;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.junit.jupiter.api.Assertions.fail;

import com.fasterxml.jackson.core.JsonFactory;
import com.fasterxml.jackson.core.JsonParser;
import com.fasterxml.jackson.core.JsonToken;
import com.lemmabase.lemma.schema.ExplanationNode;
import com.lemmabase.lemma.schema.LiteralValue;
import com.lemmabase.lemma.schema.LemmaType;
import com.lemmabase.lemma.schema.ShowPropertyElementType;
import com.lemmabase.lemma.schema.ShowPropertyType;
import com.lemmabase.lemma.schema.TypeExtends;
import java.math.BigDecimal;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.stream.Stream;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.params.ParameterizedTest;
import org.junit.jupiter.params.provider.MethodSource;

final class ApiFixtureTest {
  private static final JsonFactory FACTORY = new JsonFactory();

  private static final Set<String> LEMMA_TYPE_KINDS =
      Set.of(
          "boolean",
          "measure",
          "number",
          "number_range",
          "ratio",
          "ratio_range",
          "spec",
          "text",
          "date",
          "date_range",
          "time",
          "time_range",
          "measure_range",
          "list");

  private static final Set<String> EXPLANATION_TYPES =
      Set.of(
          "rule",
          "rule_ref",
          "compose",
          "data",
          "data_unused",
          "conversion",
          "veto",
          "row",
          "filter");

  private static Path fixturesDir() {
    Path moduleDir = Path.of(System.getProperty("user.dir")).toAbsolutePath().normalize();
    Path candidates = moduleDir.resolve("../../tests/fixtures/api").normalize();
    assertTrue(Files.isDirectory(candidates), "fixtures dir missing at " + candidates);
    return candidates;
  }

  static Stream<Path> allJsonFixtures() throws Exception {
    return Files.list(fixturesDir()).filter(p -> p.getFileName().toString().endsWith(".json")).sorted();
  }

  @ParameterizedTest
  @MethodSource("allJsonFixtures")
  void everyFixtureParses(Path fixture) throws Exception {
    String name = fixture.getFileName().toString();
    String json = Files.readString(fixture);
    switch (name) {
      case "show_minimal.json" -> {
        Show show = JsonSupport.parseShow(json);
        assertEquals("sample", show.spec());
        assertNull(show.repository());
        assertInstanceOf(LiteralValue.Text.class, show.meta().get("title"));
        Show.ShowData amount = show.data().get("amount");
        assertNotNull(amount);
        assertTrue(amount.path().isEmpty());
        ShowPropertyType.NumberType amountType =
            assertInstanceOf(ShowPropertyType.NumberType.class, amount.type());
        assertEquals("A dimensionless number.", amount.schema().get("description"));
        assertEquals("string", amount.schema().get("type"));
        assertEquals(List.of("1"), amount.schema().get("examples"));
        assertInstanceOf(RuleResultValue.Number.class, amount.suggestion());
        assertEquals(List.of("amount"), show.required());
        assertTrue(show.rows().isEmpty());
        assertEquals(json, show.json());
        Show.ShowRule ok = show.rules().get("ok");
        assertNotNull(ok);
        assertTrue(ok.path().isEmpty());
      }
      case "source_type_variants.json" -> parseSourceTypeVariants(json);
      case "literal_value_variants.json" -> parseLiteralValueVariants(json);
      case "explanation_node_variants.json" -> parseExplanationNodeVariants(json);
      case "error_kind_variants.json" -> {
        List<EngineError> errors = JsonSupport.parseEngineErrors(json);
        assertEquals(13, errors.size());
      }
      case "type_extends_variants.json" -> parseTypeExtendsVariants(json);
      case "type_specification_kinds.json" -> parseTypeSpecificationKinds(json);
      case "show_property_type_kinds.json" -> parseShowPropertyTypeKinds(json);
      case "show_property_element_type_kinds.json" -> parseShowPropertyElementTypeKinds(json);
      case "repository_install_result.json" -> {
        RepositoryInstallResult result = JsonSupport.parseInstallResult(json);
        assertEquals("@iso/countries", result.id());
        assertTrue(result.source().contains("repo @iso/countries"));
      }
      case "show_expression_variant_strings.json" -> parseShowExpressionVariantStrings(json);
      case "show_list.json" -> parseShowList(json);
      case "response_list.json" -> parseResponseList(json);
      default -> fail("no parse dispatch for fixture: " + name);
    }
  }

  private static void parseShowList(String json) {
    Show show = JsonSupport.parseShow(json);
    assertEquals("coffee_order", show.spec());
    ShowPropertyType.ListType menu =
        assertInstanceOf(ShowPropertyType.ListType.class, show.data().get("menu").type());
    ShowPropertyElementType.Spec row =
        assertInstanceOf(ShowPropertyElementType.Spec.class, menu.element());
    assertEquals("menu_item", row.spec());
    assertEquals(
        new LemmaType.ListKey.Fields(List.of("drink")),
        assertInstanceOf(LemmaType.ListKey.Fields.class, menu.key()));
    assertTrue(show.data().get("menu").neededByRules().contains("total"));
    RuleResultValue.ListValue menuValue =
        assertInstanceOf(RuleResultValue.ListValue.class, show.data().get("menu").value());
    assertNull(menuValue.result());
    assertEquals(2, menuValue.items().size());
    RuleResultValue.SourceRow espresso =
        assertInstanceOf(RuleResultValue.SourceRow.class, menuValue.items().get(0));
    assertEquals(List.of("drink", "price", "size"), List.copyOf(espresso.fields().keySet()));
    assertEquals("espresso", espresso.fields().get("drink").result());
    RuleResultValue.Measure price =
        assertInstanceOf(RuleResultValue.Measure.class, espresso.fields().get("price"));
    assertEquals(new BigDecimal("2.5"), price.measure().get("eur"));
    Show.ShowRow menuRow = show.rows().get("LemmaRow_menu__item");
    assertNotNull(menuRow);
    assertEquals("menu_item row", menuRow.title());
    assertInstanceOf(ShowPropertyType.Text.class, menuRow.data().get("drink").type());
    assertEquals(
        Map.of("$ref", "#/$defs/LemmaRow_menu__item"), show.data().get("menu").schema().get("items"));

    Show.ShowData faresData = show.data().get("fares");
    ShowPropertyType.ListType fares =
        assertInstanceOf(ShowPropertyType.ListType.class, faresData.type());
    assertInstanceOf(ShowPropertyElementType.Measure.class, fares.element());
    assertInstanceOf(LemmaType.ListKey.Numbered.class, fares.key());
    RuleResultValue.ListValue value =
        assertInstanceOf(RuleResultValue.ListValue.class, faresData.value());
    assertEquals(2, value.items().size());
    assertEquals("89 eur", value.items().get(0).result());
  }

  private static void parseResponseList(String json) {
    Response response = JsonSupport.parseResponse(json);
    RuleResult.Row selected =
        assertInstanceOf(RuleResult.Row.class, response.results().get("selected"));
    assertEquals("menu.2", selected.row().instance());
    RuleResult.RowResult latte = response.rows().get(selected.row().index());
    assertEquals("menu_item", latte.spec());
    assertEquals("latte", latte.data().get("drink").result().result());

    RuleResult.ListResult large =
        assertInstanceOf(RuleResult.ListResult.class, response.results().get("large"));
    RuleResultValue.Row largeRow =
        assertInstanceOf(RuleResultValue.Row.class, large.items().get(0));
    assertEquals(selected.row().index(), largeRow.row().index());

    RuleResult.ListResult highFares =
        assertInstanceOf(RuleResult.ListResult.class, response.results().get("high_fares"));
    RuleResultValue.Measure fare =
        assertInstanceOf(RuleResultValue.Measure.class, highFares.items().get(0));
    assertEquals(new BigDecimal("120"), fare.measure().get("eur"));
  }

  @Test
  void unknownFieldRejectedOnShow() {
    String json =
        """
        {
          "$schema": "https://json-schema.org/draft/2020-12/schema",
          "title": "sample",
          "type": "object",
          "additionalProperties": true,
          "properties": {},
          "x-lemma-start-line": 1,
          "x-lemma-meta": {},
          "x-lemma-rules": {},
          "not_a_real_field": true
        }
        """;
    LemmaBugError thrown = assertThrows(LemmaBugError.class, () -> JsonSupport.parseShow(json));
    assertTrue(thrown.getMessage().contains("not_a_real_field"), thrown.getMessage());
  }

  @Test
  void longDecimalSurvivesAsBigDecimal() {
    String json =
        """
        {
          "spec": "sample",
          "effective": "2024-01-01",
          "results": {
            "ok": {
              "vetoed": false,
              "result": "0.3333333333333333333333333333",
              "rule_type": "number",
              "number": "0.3333333333333333333333333333"
            }
          }
        }
        """;
    Response response = JsonSupport.parseResponse(json);
    BigDecimal number = ((RuleResult.Number) response.results().get("ok")).number();
    assertEquals(new BigDecimal("0.3333333333333333333333333333"), number);
  }

  @Test
  void engineErrorKindIsString() throws Exception {
    assertEquals(String.class, EngineError.class.getMethod("kind").getReturnType());
  }

  private static void parseSourceTypeVariants(String json) throws Exception {
    try (JsonParser p = FACTORY.createParser(json)) {
      p.nextToken();
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String label = p.currentName();
        p.nextToken();
        SourceType parsed = SourceType.read(p);
        switch (label) {
          case "path" -> assertInstanceOf(SourceType.Path.class, parsed);
          case "dependency" -> assertInstanceOf(SourceType.Dependency.class, parsed);
          case "volatile" -> assertInstanceOf(SourceType.Volatile.class, parsed);
          default -> fail("unexpected label " + label);
        }
      }
    }
  }

  private static void parseLiteralValueVariants(String json) throws Exception {
    Set<Class<?>> classes = new HashSet<>();
    try (JsonParser p = FACTORY.createParser(json)) {
      p.nextToken();
      while (p.nextToken() != JsonToken.END_OBJECT) {
        p.nextToken();
        classes.add(LiteralValue.read(p).getClass());
      }
    }
    assertEquals(
        Set.of(
            LiteralValue.Number.class,
            LiteralValue.NumberWithUnit.class,
            LiteralValue.Text.class,
            LiteralValue.Date.class,
            LiteralValue.Time.class,
            LiteralValue.BooleanLit.class,
            LiteralValue.Range.class),
        classes);
  }

  private static void parseExplanationNodeVariants(String json) throws Exception {
    try (JsonParser p = FACTORY.createParser(json)) {
      p.nextToken();
      ExplanationNode.Rule root = ExplanationNode.Rule.read(p);
      Set<String> types = new HashSet<>();
      types.add(root.type());
      if (root.causes() != null) {
        for (ExplanationNode.Cause cause : root.causes()) {
          if (cause.children() != null) {
            for (ExplanationNode child : cause.children()) {
              collectExplanationTypes(child, types);
            }
          }
        }
      }
      if (root.children() != null) {
        for (ExplanationNode child : root.children()) {
          collectExplanationTypes(child, types);
        }
      }
      if (root.rules() != null) {
        for (ExplanationNode.ExplainedRule rule : root.rules()) {
          if (rule.causes() != null) {
            for (ExplanationNode.Cause cause : rule.causes()) {
              if (cause.children() != null) {
                for (ExplanationNode child : cause.children()) {
                  collectExplanationTypes(child, types);
                }
              }
            }
          }
          if (rule.children() != null) {
            for (ExplanationNode child : rule.children()) {
              collectExplanationTypes(child, types);
            }
          }
        }
      }
      assertEquals(EXPLANATION_TYPES, types);
    }
  }

  private static void collectExplanationTypes(ExplanationNode node, Set<String> types) {
    types.add(node.type());
    switch (node) {
      case ExplanationNode.RuleRef ignored -> {}
      case ExplanationNode.Compose compose -> {
        for (ExplanationNode child : compose.operands()) {
          collectExplanationTypes(child, types);
        }
      }
      case ExplanationNode.Conversion conversion -> {
        for (ExplanationNode child : conversion.operands()) {
          collectExplanationTypes(child, types);
        }
      }
      case ExplanationNode.Data ignored -> {}
      case ExplanationNode.DataUnused ignored -> {}
      case ExplanationNode.Veto ignored -> {}
      case ExplanationNode.Row row -> {
        if (row.key() != null) {
          for (ExplanationNode child : row.key()) {
            collectExplanationTypes(child, types);
          }
        }
        for (ExplanationNode child : row.fields()) {
          collectExplanationTypes(child, types);
        }
      }
      case ExplanationNode.Filter filter -> {
        if (filter.children() != null) {
          for (ExplanationNode child : filter.children()) {
            collectExplanationTypes(child, types);
          }
        }
      }
    }
  }

  private static void parseTypeExtendsVariants(String json) throws Exception {
    try (JsonParser p = FACTORY.createParser(json)) {
      p.nextToken();
      while (p.nextToken() != JsonToken.END_OBJECT) {
        p.nextToken();
        TypeExtends.read(p);
      }
    }
  }

  private static void parseTypeSpecificationKinds(String json) throws Exception {
    Set<String> kinds = new HashSet<>();
    try (JsonParser p = FACTORY.createParser(json)) {
      p.nextToken();
      while (p.nextToken() != JsonToken.END_OBJECT) {
        p.nextToken();
        kinds.add(LemmaType.read(p).kind());
      }
    }
    assertEquals(LEMMA_TYPE_KINDS, kinds);
  }

  private static void parseShowPropertyTypeKinds(String json) throws Exception {
    Set<String> kinds = new HashSet<>();
    try (JsonParser p = FACTORY.createParser(json)) {
      p.nextToken();
      while (p.nextToken() != JsonToken.END_OBJECT) {
        p.nextToken();
        kinds.add(ShowPropertyType.read(p).kind());
      }
    }
    assertEquals(13, kinds.size());
    assertTrue(kinds.contains("list"));
    assertTrue(!kinds.contains("spec"));
  }

  private static void parseShowPropertyElementTypeKinds(String json) throws Exception {
    Set<String> kinds = new HashSet<>();
    try (JsonParser p = FACTORY.createParser(json)) {
      p.nextToken();
      while (p.nextToken() != JsonToken.END_OBJECT) {
        p.nextToken();
        kinds.add(ShowPropertyElementType.read(p).kind());
      }
    }
    assertEquals(13, kinds.size());
    assertTrue(kinds.contains("spec"));
    assertTrue(!kinds.contains("list"));
  }

  private static void parseShowExpressionVariantStrings(String json) throws Exception {
    try (JsonParser p = FACTORY.createParser(json)) {
      p.nextToken();
      JsonReading.expectStartObject(p, "show_expression_variant_strings");
      List<String> variants = null;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "description" -> JsonReading.readString(p);
          case "variants" -> variants = JsonReading.readList(p, JsonReading::readString);
          default -> JsonReading.unknownField(field, "show_expression_variant_strings");
        }
      }
      if (variants == null) {
        JsonReading.missingRequired("variants", "show_expression_variant_strings");
      }
      assertTrue(!variants.isEmpty());
      assertTrue(variants.contains("literal"));
      assertTrue(variants.contains("data"));
      assertTrue(variants.contains("rule"));
      assertTrue(variants.contains("arithmetic"));
    }
  }
}

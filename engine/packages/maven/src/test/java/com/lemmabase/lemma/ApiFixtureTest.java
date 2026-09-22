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
import com.lemmabase.lemma.schema.TypeExtends;
import java.math.BigDecimal;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.HashSet;
import java.util.List;
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
          "numberrange",
          "ratio",
          "ratiorange",
          "text",
          "date",
          "daterange",
          "time",
          "timerange",
          "measurerange");

  private static final Set<String> EXPLANATION_TYPES =
      Set.of("rule", "compose", "data", "data_unused", "conversion", "veto");

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
        assertInstanceOf(LiteralValue.Text.class, show.meta().get("author"));
        Show.ShowData amount = show.data().get("amount");
        assertNotNull(amount);
        assertTrue(amount.path().isEmpty());
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
      case "repository_install_result.json" -> {
        RepositoryInstallResult result = JsonSupport.parseInstallResult(json);
        assertEquals("@iso/countries", result.id());
        assertTrue(result.source().contains("repo @iso/countries"));
      }
      case "show_expression_variant_strings.json" -> parseShowExpressionVariantStrings(json);
      default -> fail("no parse dispatch for fixture: " + name);
    }
  }

  @Test
  void unknownFieldRejectedOnShow() {
    String json =
        """
        {
          "spec": "sample",
          "start_line": 1,
          "data": {},
          "rules": {},
          "meta": {},
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
      ExplanationNode node = ExplanationNode.read(p);
      assertInstanceOf(ExplanationNode.Rule.class, node);
      Set<String> types = new HashSet<>();
      collectExplanationTypes(node, types);
      assertEquals(EXPLANATION_TYPES, types);
    }
  }

  private static void collectExplanationTypes(ExplanationNode node, Set<String> types) {
    types.add(node.type());
    switch (node) {
      case ExplanationNode.Rule rule -> {
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

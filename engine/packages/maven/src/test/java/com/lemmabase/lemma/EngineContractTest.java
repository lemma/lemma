package com.lemmabase.lemma;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.nio.file.Files;
import java.nio.file.Path;
import org.junit.jupiter.api.Test;

final class EngineContractTest {

  @Test
  void versionReturnsSemver() {
    String version = Engine.version();
    assertTrue(version.matches("\\d+\\.\\d+\\.\\d+.*"), "version must be semver, got " + version);
  }

  @Test
  void updateReplacesSpecSlice() {
    try (Engine engine = Engine.create()) {
      engine.load(
          """
          spec pricing
          data quantity: 1
          rule total: quantity * 10
          """);
      engine.update(
          null,
          """
          spec pricing
          data quantity: 1
          rule total: quantity * 20
          """,
          null);
      Response response = engine.evaluate(EvaluateRequest.of("pricing"));
      assertEquals("20", ((RuleResult.Number) response.results().get("total")).number().toPlainString());
    }
  }

  @Test
  void listDataInAndRowsOut() {
    try (Engine engine = Engine.create()) {
      engine.load(
          """
          spec line
          data price: number
          data qty: number
          rule cost: price * qty

          spec order
          uses lines: line list
          data discounts: number list
          rule total: lines -> sum cost
          rule priciest: lines -> sort cost desc -> first
          rule all_lines: lines
          rule discount_total: discounts -> sum
          """);
      Response response =
          engine.evaluate(
              EvaluateRequest.of("order")
                  .data(
                      Map.of(
                          "lines",
                          List.of(
                              Map.of("price", "2", "qty", 3),
                              Map.of("price", "10", "qty", 1)),
                          "discounts",
                          List.of("1", "4"))));
      assertEquals(
          "16", ((RuleResult.Number) response.results().get("total")).number().toPlainString());
      assertEquals(
          "5",
          ((RuleResult.Number) response.results().get("discount_total"))
              .number()
              .toPlainString());
      RuleResult.Row priciest = (RuleResult.Row) response.results().get("priciest");
      assertEquals("lines.2", priciest.row().instance());
      RuleResult.RowResult row = response.rows().get(priciest.row().index());
      assertEquals("line", row.spec());
      assertEquals("10", row.rules().get("cost").result().result());
      RuleResult.ListResult all = (RuleResult.ListResult) response.results().get("all_lines");
      assertEquals(2, all.items().size());
      assertEquals(2, response.rows().size());
    }
  }

  @Test
  void qualityReportsMissingHelp() {
    try (Engine engine = Engine.create()) {
      engine.load(
          """
          spec pricing 2026-01-01
          \"\"\"
          Bulk pricing.
          \"\"\"

          data qty: number
          rule total: qty
          """);
      List<Recommendation> recs = engine.quality();
      assertFalse(recs.isEmpty());
      Recommendation hit =
          recs.stream()
              .filter(r -> r.message().contains("no `-> help`"))
              .findFirst()
              .orElseThrow();
      assertEquals("pricing", hit.spec());
      assertEquals("2026-01-01", hit.effectiveFrom());
      assertTrue(hit.source().attribute().length() > 0);
      assertTrue(hit.source().line() > 0);
    }
  }

  @Test
  void qualityEmptyForCleanSpec() {
    try (Engine engine = Engine.create()) {
      engine.load(
          """
          spec pricing 2026-01-01
          \"\"\"
          Bulk pricing.
          \"\"\"

          data qty: number
            -> minimum 0
            -> maximum 1000000
            -> help "Order quantity."

          rule total: qty
          """);
      assertTrue(engine.quality().isEmpty());
    }
  }

  @Test
  void rejectsDoubleRunDataValues() {
    try (Engine engine = Engine.create()) {
      engine.load(
          """
          spec pricing
          data amount: number
          rule doubled: amount * 2
          """);
      LemmaException thrown =
          assertThrows(
              LemmaException.class,
              () ->
                  engine.evaluate(
                      EvaluateRequest.of("pricing").data(Map.of("amount", 1.5))));
      assertTrue(
          thrown.errors().get(0).message().contains("pass as string"),
          thrown.errors().get(0).message());
      assertFalse(thrown.errors().isEmpty());
      assertEquals("request", thrown.errors().get(0).kind());
      assertEquals("amount", thrown.errors().get(0).relatedData());
    }
  }

  @Test
  void rejectsNullListItem() {
    try (Engine engine = Engine.create()) {
      engine.load(
          """
          spec fares
          data money: measure
            -> unit eur: 1.00
          data fares: money list
          rule total:
            fares -> sum
          """);
      List<Object> fares = new ArrayList<>();
      fares.add("89 eur");
      fares.add(null);
      fares.add("120 eur");
      LemmaException thrown =
          assertThrows(
              LemmaException.class,
              () -> engine.evaluate(EvaluateRequest.of("fares").data(Map.of("fares", fares))));
      assertEquals("request", thrown.errors().get(0).kind());
      assertEquals("fares", thrown.errors().get(0).relatedData());
      assertEquals("list item 2 is null", thrown.errors().get(0).message());
    }
  }

  @Test
  void deeplyNestedRunDataIsRequestErrorNotCrash() {
    try (Engine engine = Engine.create()) {
      engine.load(
          """
          spec pricing
          data amount: number
          rule doubled: amount * 2
          """);
      Object deepList = "1";
      Object deepMap = "1";
      for (int i = 0; i < 100_000; i++) {
        List<Object> wrapList = new ArrayList<>();
        wrapList.add(deepList);
        deepList = wrapList;
        java.util.Map<String, Object> wrapMap = new java.util.LinkedHashMap<>();
        wrapMap.put("n", deepMap);
        deepMap = wrapMap;
      }
      Object listRoot = deepList;
      Object mapRoot = deepMap;
      LemmaException listError =
          assertThrows(
              LemmaException.class,
              () -> engine.evaluate(EvaluateRequest.of("pricing").data(Map.of("amount", listRoot))));
      assertEquals("request", listError.errors().get(0).kind());
      assertEquals("amount", listError.errors().get(0).relatedData());
      LemmaException mapError =
          assertThrows(
              LemmaException.class,
              () -> engine.evaluate(EvaluateRequest.of("pricing").data(Map.of("amount", mapRoot))));
      assertEquals("request", mapError.errors().get(0).kind());
      assertEquals("amount", mapError.errors().get(0).relatedData());
    }
  }

  @Test
  void nestedWrongShapeListItemIsRequestErrorNotCrash() {
    try (Engine engine = Engine.create()) {
      engine.load(
          """
          spec fares
          data money: measure
            -> unit eur: 1.00
          data fares: money list
          """);
      for (Object badItem : List.of(Boolean.TRUE, List.of(List.of("1 eur")))) {
        Map<String, Object> data = Map.of("fares", List.of(badItem));
        LemmaException thrown =
            assertThrows(
                LemmaException.class,
                () -> engine.evaluate(EvaluateRequest.of("fares").data(data)));
        assertEquals("request", thrown.errors().get(0).kind());
        assertEquals("fares", thrown.errors().get(0).relatedData());
      }
    }
  }

  @Test
  void rejectsRunDataForBoundSlot() {
    try (Engine engine = Engine.create()) {
      engine.load(
          """
          spec inner
          data slot: number

          spec outer
          data base: number
          uses i: inner
            -> with slot: base
          rule r: i.slot
          """);
      LemmaException thrown =
          assertThrows(
              LemmaException.class,
              () -> engine.evaluate(EvaluateRequest.of("outer").data(Map.of("i.slot", "5"))));
      assertEquals("request", thrown.errors().get(0).kind());
      assertEquals("i.slot", thrown.errors().get(0).relatedData());
      assertEquals(
          "data 'i.slot' is bound to 'base' by the spec and cannot be supplied",
          thrown.errors().get(0).message());
    }
  }

  @Test
  void nestedRunDataErrorsAttributeToTopLevelKey() {
    try (Engine engine = Engine.create()) {
      engine.load(
          """
          spec shipping
          data weight: measure
            -> unit kilogram: 1
          rule heavy: weight > 10 kilogram
          """);
      LemmaException nestedDouble =
          assertThrows(
              LemmaException.class,
              () ->
                  engine.evaluate(
                      EvaluateRequest.of("shipping")
                          .data(Map.of("weight", Map.of("kilogram", 12.0)))));
      assertEquals("weight", nestedDouble.errors().get(0).relatedData());
      assertTrue(
          nestedDouble.errors().get(0).message().contains("pass as string: \"12.0\""),
          nestedDouble.errors().get(0).message());

      java.util.Map<String, Object> unit = new java.util.LinkedHashMap<>();
      unit.put("KILOGRAM", "12");
      unit.put("kilogram", "13");
      LemmaException duplicate =
          assertThrows(
              LemmaException.class,
              () -> engine.evaluate(EvaluateRequest.of("shipping").data(Map.of("weight", unit))));
      assertTrue(
          duplicate.errors().get(0).message().contains("Duplicate object key 'kilogram'"),
          duplicate.errors().get(0).message());

      LemmaException nonStringKey =
          assertThrows(
              LemmaException.class,
              () ->
                  engine.evaluate(
                      EvaluateRequest.of("shipping").data(Map.of("weight", Map.of(1, "12")))));
      assertTrue(
          nonStringKey.errors().get(0).message().contains("keys must be String"),
          nonStringKey.errors().get(0).message());
    }
  }

  @Test
  void nestedNullUnitMapValueIsOmitted() {
    try (Engine engine = Engine.create()) {
      engine.load(
          """
          spec shipping
          data weight: measure
            -> unit kilogram: 1
          rule heavy: weight > 10 kilogram
          """);
      java.util.Map<String, Object> unit = new java.util.LinkedHashMap<>();
      unit.put("kilogram", "12");
      unit.put("gram", null);
      Response response = engine.evaluate(EvaluateRequest.of("shipping").data(Map.of("weight", unit)));
      RuleResult heavy = response.results().get("heavy");
      assertTrue(heavy instanceof RuleResult.BooleanValue);
      assertEquals(true, ((RuleResult.BooleanValue) heavy).booleanValue());
    }
  }

  @Test
  void rejectsFloatRunDataValues() {
    try (Engine engine = Engine.create()) {
      engine.load(
          """
          spec pricing
          data amount: number
          rule doubled: amount * 2
          """);
      assertThrows(
          LemmaException.class,
          () ->
              engine.evaluate(
                  EvaluateRequest.of("pricing").data(Map.of("amount", 1.5f))));
    }
  }

  @Test
  void acceptsBigDecimalAndSingleUnitMap() {
    try (Engine engine = Engine.create()) {
      engine.load(
          """
          spec shipping
          data weight: measure
            -> unit kilogram: 1
          data amount: number
          rule heavy: weight > 10 kilogram
          rule doubled: amount * 2
          """);
      Response response =
          engine.evaluate(
              EvaluateRequest.of("shipping")
                  .data(
                      Map.of(
                          "weight", Map.of("kilogram", "12"),
                          "amount", new java.math.BigDecimal("3"))));
      RuleResult heavy = response.results().get("heavy");
      assertTrue(heavy instanceof RuleResult.BooleanValue);
      assertEquals(true, ((RuleResult.BooleanValue) heavy).booleanValue());
      RuleResult doubled = response.results().get("doubled");
      assertTrue(doubled instanceof RuleResult.Number);
      assertEquals(new java.math.BigDecimal("6"), ((RuleResult.Number) doubled).number());
    }
  }

  @Test
  void rejectsMultiKeyUnitMap() {
    try (Engine engine = Engine.create()) {
      engine.load(
          """
          spec shipping
          data weight: measure
            -> unit kilogram: 1
            -> unit gram: 0.001
          rule heavy: weight > 10 kilogram
          """);
      LemmaException thrown =
          assertThrows(
              LemmaException.class,
              () ->
                  engine.evaluate(
                      EvaluateRequest.of("shipping")
                          .data(Map.of("weight", Map.of("kilogram", "12", "gram", "12000")))));
      assertTrue(
          thrown.errors().get(0).message().contains("single-unit"),
          thrown.errors().get(0).message());
    }
  }

  @Test
  void vetoRemainsInRuleResult() {
    try (Engine engine = Engine.create()) {
      engine.load(
          """
          spec deny
          rule outcome: veto "not allowed"
          """);
      Response response = engine.evaluate(EvaluateRequest.of("deny"));
      RuleResult outcome = response.results().get("outcome");
      assertTrue(outcome instanceof RuleResult.Veto);
      assertEquals("not allowed", ((RuleResult.Veto) outcome).vetoReason());
    }
  }

  @Test
  void emptyRulesListFails() {
    try (Engine engine = Engine.create()) {
      engine.load(
          """
          spec sample
          rule value: 1
          """);
      LemmaException thrown =
          assertThrows(
              LemmaException.class,
              () -> engine.evaluate(EvaluateRequest.of("sample").rules(List.of())));
      assertTrue(thrown.getMessage().contains("evaluate failed"), thrown.getMessage());
      assertTrue(
          thrown.errors().stream().anyMatch(e -> e.message().contains("rules must not be empty")),
          thrown.errors().toString());
    }
  }

  @Test
  void invalidLoadThrowsLemmaException() {
    try (Engine engine = Engine.create()) {
      LemmaException thrown =
          assertThrows(LemmaException.class, () -> engine.load("this is not lemma"));
      assertFalse(thrown.errors().isEmpty());
    }
  }

  @Test
  void installEmptyRepositoryThrowsLemmaException() {
    try (Engine engine = Engine.create()) {
      LemmaException thrown = assertThrows(LemmaException.class, () -> engine.install("   "));
      assertFalse(thrown.errors().isEmpty());
      assertTrue(
          thrown.errors().stream().anyMatch(err -> "registry".equals(err.kind())),
          "expected registry error for empty id");
    }
  }

  @Test
  void useAfterCloseThrowsLemmaBugError() {
    Engine engine = Engine.create();
    engine.close();
    assertThrows(LemmaBugError.class, () -> engine.load("spec x\nrule y: 1"));
  }

  @Test
  void convenienceUnitStringAndDecimalResult() {
    try (Engine engine = Engine.create()) {
      engine.load(
          """
          spec shipping
          data weight: measure
            -> unit kilogram: 1
          rule heavy: weight > 10 kilogram
          """);
      Response response =
          engine.evaluate(
              EvaluateRequest.of("shipping")
                  .data(Map.of("weight", "12 kilogram"))
                  .rules(List.of("heavy")));
      RuleResult heavy = response.results().get("heavy");
      assertTrue(heavy instanceof RuleResult.BooleanValue);
      assertEquals(true, ((RuleResult.BooleanValue) heavy).booleanValue());
    }
  }

  @Test
  void formatReturnsNormalizedSource() {
    String formatted =
        Lemma.format(
            """
            spec demo
            rule x:1
            """);
    assertTrue(formatted.contains("spec demo"));
    assertTrue(formatted.contains("rule x"));
  }

  @Test
  void snapshotRoundTripRestoresListAndRun() {
    try (Engine source = Engine.create()) {
      source.load(
          """
          spec snap_demo
          data x: number
          rule y: x + 1
          """);
      byte[] bytes = source.snapshot();
      assertTrue(bytes.length > 0);
      try (Engine restored = Engine.fromSnapshot(bytes)) {
        assertEquals(source.specs(), restored.specs());
        Response runSource =
            source.evaluate(EvaluateRequest.of("snap_demo").data(Map.of("x", "41")));
        Response runRestored =
            restored.evaluate(EvaluateRequest.of("snap_demo").data(Map.of("x", "41")));
        assertEquals(runSource.results().keySet(), runRestored.results().keySet());
        assertEquals(
            ((RuleResult.Number) runSource.results().get("y")).number(),
            ((RuleResult.Number) runRestored.results().get("y")).number());
      }
      bytes[0] = 0x58;
      assertThrows(LemmaException.class, () -> Engine.fromSnapshot(bytes));
    }
  }

  @Test
  void versionedSpecShowSpecsAndRunSucceedWithIsoStrings() {
    try (Engine engine = Engine.create()) {
      engine.load(
          """
          spec policy 2024-01-01
          data amount: number
          rule ok: amount

          spec policy 2025-06-01
          data amount: number
          rule ok: amount * 2
          """);

      Show show = engine.show(null, "policy", "2024-06-01");
      assertEquals("2024-01-01", show.effectiveFrom());
      assertEquals("2025-06-01", show.effectiveTo());

      List<String> policyEffectiveFroms = new java.util.ArrayList<>();
      for (Repository repo : engine.specs()) {
        for (Repository.SpecVersion spec : repo.specs()) {
          if ("policy".equals(spec.name())) {
            policyEffectiveFroms.add(spec.effectiveFrom());
          }
        }
      }
      assertEquals(List.of("2024-01-01", "2025-06-01"), policyEffectiveFroms);

      Response response =
          engine.evaluate(
              EvaluateRequest.of("policy")
                  .effective("2024-06-01")
                  .data(Map.of("amount", "3")));
      assertTrue(response.results().get("ok") instanceof RuleResult.Number);
      assertEquals("3", ((RuleResult.Number) response.results().get("ok")).result());
    }
  }

  @Test
  void singleVersionShowOmitsEffectiveToAsNull() {
    try (Engine engine = Engine.create()) {
      engine.load(
          """
          spec solo 2024-01-01
          data amount: number
          rule ok: amount
          """);
      Show show = engine.show(null, "solo", "2024-06-01");
      assertEquals("2024-01-01", show.effectiveFrom());
      assertEquals(null, show.effectiveTo());
    }
  }

  @Test
  void showDataAndResponseResultsPreserveDeclarationOrder() {
    try (Engine engine = Engine.create()) {
      engine.load(
          """
          spec topo
          data zebra: number
          data yankee: number
          data alpha: number
          rule z: zebra
          rule a: z + yankee + alpha
          """);
      Show show = engine.show(null, "topo", null);
      assertEquals(
          java.util.List.of("zebra", "yankee", "alpha"),
          java.util.List.copyOf(show.data().keySet()));
      assertEquals(java.util.List.of("z", "a"), java.util.List.copyOf(show.rules().keySet()));

      Response response =
          engine.evaluate(
              EvaluateRequest.of("topo")
                  .data(
                      java.util.Map.of(
                          "zebra", "1",
                          "yankee", "2",
                          "alpha", "3")));
      assertEquals(
          java.util.List.copyOf(show.rules().keySet()),
          java.util.List.copyOf(response.results().keySet()));
    }
  }

  @Test
  void nullDataValueRaisesAttributedLemmaExceptionNotNpe() {
    try (Engine engine = Engine.create()) {
      engine.load(
          """
          spec pricing
          data amount: number
          rule ok: amount
          """);
      java.util.Map<String, Object> data = new java.util.LinkedHashMap<>();
      data.put("amount", null);
      LemmaException thrown =
          assertThrows(
              LemmaException.class,
              () -> engine.evaluate(EvaluateRequest.of("pricing").data(data)));
      assertFalse(
          thrown.getCause() instanceof NullPointerException
              || thrown.getMessage().contains("NullPointerException"),
          "must not surface bare NPE from Map.copyOf");
      assertFalse(thrown.errors().isEmpty());
      assertEquals("amount", thrown.errors().get(0).relatedData());
      assertTrue(
          thrown.errors().get(0).message().contains("must not be null"),
          thrown.errors().get(0).message());
    }
  }

  @Test
  void engineErrorKindIsStringType() throws Exception {
    var errorRecord = EngineError.class;
    var kindMethod = errorRecord.getMethod("kind");
    assertEquals(String.class, kindMethod.getReturnType(), "EngineError.kind() should return String");
  }

  @Test
  void unrecognizedErrorKindReadsAsString() {
    String json =
        "[{\"kind\":\"not_a_real_kind\",\"message\":\"x\",\"related_data\":null,\"spec\":null,\"related_spec\":null,\"source\":null,\"suggestion\":null,\"repository\":null,\"registry_kind\":null,\"request_kind\":null,\"limit_name\":null,\"limit_value\":null,\"actual_value\":null}]";
    var errors = new LemmaException("x", json).errors();
    assertEquals(1, errors.size());
    assertEquals("not_a_real_kind", errors.get(0).kind());
  }

  @Test
  void multipleInvalidRunDataValuesReportInInputOrder() {
    try (Engine engine = Engine.create()) {
      engine.load(
          """
          spec multi
          data a: number
          data b: number
          data c: number
          data d: number
          data e: number
          rule r: a
          """);
      java.util.Map<String, Object> data = new java.util.LinkedHashMap<>();
      data.put("a", null);
      data.put("b", "1");
      data.put("c", null);
      data.put("d", "2");
      data.put("e", null);
      LemmaException thrown =
          assertThrows(
              LemmaException.class,
              () -> engine.evaluate(EvaluateRequest.of("multi").data(data)));
      assertFalse(
          thrown.getCause() instanceof NullPointerException,
          "must not surface bare NPE from Map.copyOf");
      java.util.List<String> related =
          thrown.errors().stream()
              .map(EngineError::relatedData)
              .filter(java.util.Objects::nonNull)
              .toList();
      assertEquals(java.util.List.of("a", "c", "e"), related);
    }
  }

  @Test
  void deepLinearChainTipOnlyEval() {
    StringBuilder src = new StringBuilder();
    src.append("spec bench_deep\n");
    src.append("data x0: number\n");
    src.append("rule r1: x0 + 1\n");
    for (int i = 2; i <= 200; i++) {
      src.append("rule r").append(i).append(": r").append(i - 1).append(" + 1\n");
    }
    try (Engine engine = Engine.create()) {
      engine.load(src.toString());
      Response response =
          engine.evaluate(
              EvaluateRequest.of("bench_deep")
                  .data(Map.of("x0", java.math.BigDecimal.ZERO))
                  .rules(List.of("r200")));
      assertEquals(1, response.results().size());
      assertEquals(
          "200",
          ((RuleResult.Number) response.results().get("r200")).number().toPlainString());
    }
  }

  @Test
  void deepLinearChainLoad2000() {
    StringBuilder src = new StringBuilder();
    src.append("spec bench_deep\n");
    src.append("data x0: number\n");
    src.append("rule r1: x0 + 1\n");
    for (int i = 2; i <= 2000; i++) {
      src.append("rule r").append(i).append(": r").append(i - 1).append(" + 1\n");
    }
    try (Engine engine = Engine.create()) {
      engine.load(src.toString());
    }
  }

  @Test
  void resourceLimitsBuilderOverridesOneFieldKeepsEngineDefaults() {
    try (Engine baseline = Engine.create();
        Engine limited = Engine.create(ResourceLimits.builder().maxSources(7))) {
      ResourceLimits defaults = baseline.limits();
      ResourceLimits actual = limited.limits();
      assertEquals(7, actual.maxSources());
      assertEquals(defaults.maxSourceSizeBytes(), actual.maxSourceSizeBytes());
      assertEquals(defaults.maxExpressionDepth(), actual.maxExpressionDepth());
      assertEquals(defaults.maxExpressionCount(), actual.maxExpressionCount());
      assertEquals(defaults.maxDataValueBytes(), actual.maxDataValueBytes());
      assertEquals(defaults.maxLoadedBytes(), actual.maxLoadedBytes());
      assertEquals(
          defaults.maxNormalizedExpressionNodes(), actual.maxNormalizedExpressionNodes());
      assertEquals(defaults.maxSpecDependencyDepth(), actual.maxSpecDependencyDepth());
      assertEquals(defaults.maxDagSpecs(), actual.maxDagSpecs());
    }
  }

  @Test
  void resourceLimitsBuilderMaxNormalizedExpressionNodesRoundTrips() {
    try (Engine engine =
        Engine.create(ResourceLimits.builder().maxNormalizedExpressionNodes(99))) {
      assertEquals(99, engine.limits().maxNormalizedExpressionNodes());
    }
  }

  @Test
  void resourceLimitsEmptyBuilderMatchesCreateDefaults() {
    try (Engine baseline = Engine.create();
        Engine fromBuilder = Engine.create(ResourceLimits.builder())) {
      assertEquals(baseline.limits(), fromBuilder.limits());
    }
  }

  @Test
  void resourceLimitsNegativeOverrideThrowsLemmaException() {
    assertThrows(
        LemmaException.class,
        () -> Engine.create(ResourceLimits.builder().maxSources(-1)));
  }

  @Test
  void resourceLimitsSnapshotCreatePreservesValues() {
    try (Engine source = Engine.create(ResourceLimits.builder().maxSources(12));
        Engine copy = Engine.create(source.limits())) {
      assertEquals(source.limits(), copy.limits());
    }
  }

  @Test
  void loadPathReadsUtf8File() throws Exception {
    Path path = Files.createTempFile("lemma-load", ".lemma");
    Files.writeString(
        path,
        """
        spec from_path
        rule value: 7
        """);
    try (Engine engine = Engine.create()) {
      engine.load(path);
      Response response = engine.evaluate(EvaluateRequest.of("from_path"));
      assertEquals(
          "7", ((RuleResult.Number) response.results().get("value")).number().toPlainString());
    } finally {
      Files.deleteIfExists(path);
    }
  }

  @Test
  void loadPathMissingFileThrowsLemmaException() {
    assertThrows(
        LemmaException.class,
        () -> {
          try (Engine engine = Engine.create()) {
            engine.load(Path.of("/nonexistent/lemma/path/that/does/not/exist.lemma"));
          }
        });
  }

  @Test
  void loadPathFailureWithQuoteInPathSurfacesTypedEngineError() {
    Path weird = Path.of("/nonexistent/lemma/\"quoted\"/missing.lemma");
    LemmaException thrown =
        assertThrows(
            LemmaException.class,
            () -> {
              try (Engine engine = Engine.create()) {
                engine.load(weird);
              }
            });
    assertFalse(thrown.errors().isEmpty());
    assertTrue(
        thrown.errors().get(0).message().contains("\"quoted\""),
        thrown.errors().get(0).message());
    assertEquals("request", thrown.errors().get(0).kind());
  }
}

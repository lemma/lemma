package com.lemmabase.lemma;

import com.fasterxml.jackson.core.JsonParser;
import com.fasterxml.jackson.core.JsonToken;
import java.io.IOException;
import java.util.List;
import java.util.Map;
import org.jspecify.annotations.Nullable;
/**
 * Response.
 * @param spec spec
 * @param effective effective
 * @param specEffectiveFrom specEffectiveFrom
 * @param specEffectiveTo specEffectiveTo
 * @param results results
 * @param rows list rows the results reach, in first-reached order; a {@link
 *     RuleResult.RowReference#index()} points here
 */
public record Response(
    String spec,
    String effective,
    @Nullable String specEffectiveFrom,
    @Nullable String specEffectiveTo,
    Map<String, RuleResult> results,
    List<RuleResult.RowResult> rows) {

  /**
   * Parses JSON.
   *
   * @param p parser at value start
   * @return parsed value
   * @throws IOException if JSON IO fails
   */
  static Response read(JsonParser p) throws IOException {
    JsonReading.expectStartObject(p, "Response");
    String spec = null;
    String effective = null;
    String specEffectiveFrom = null;
    String specEffectiveTo = null;
    Map<String, RuleResult> results = null;
    List<RuleResult.RowResult> rows = List.of();
    while (p.nextToken() != JsonToken.END_OBJECT) {
      String field = p.currentName();
      p.nextToken();
      switch (field) {
        case "spec" -> spec = JsonReading.readString(p);
        case "effective" -> effective = JsonReading.readString(p);
        case "spec_effective_from" -> specEffectiveFrom = JsonReading.readString(p);
        case "spec_effective_to" -> specEffectiveTo = JsonReading.readString(p);
        case "results" -> results = JsonReading.readMap(p, RuleResult::read);
        case "rows" -> rows = List.copyOf(JsonReading.readList(p, RuleResult.RowResult::read));
        default -> JsonReading.unknownField(field, "Response");
      }
    }
    if (spec == null) {
      JsonReading.missingRequired("spec", "Response");
    }
    if (effective == null) {
      JsonReading.missingRequired("effective", "Response");
    }
    if (results == null) {
      JsonReading.missingRequired("results", "Response");
    }
    return new Response(spec, effective, specEffectiveFrom, specEffectiveTo, results, rows);
  }
}

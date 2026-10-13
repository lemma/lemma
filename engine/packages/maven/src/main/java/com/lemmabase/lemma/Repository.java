package com.lemmabase.lemma;

import com.fasterxml.jackson.core.JsonParser;
import com.fasterxml.jackson.core.JsonToken;
import java.io.IOException;
import java.util.List;
import org.jspecify.annotations.Nullable;
/**
 * Repository.
 * @param repository repository
 * @param specs specs
 */
public record Repository(@Nullable String repository, List<SpecVersion> specs) {
  /**
   * One uses target on a spec version.
   *
   * @param repository repository qualifier written on the uses line; null for same-repository
   * @param name target spec name
   * @param effective optional effective pin written on the uses line
   */
  public record SpecDependency(
      @Nullable String repository, String name, @Nullable String effective) {
    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    static SpecDependency read(JsonParser p) throws IOException {
      JsonReading.expectStartObject(p, "SpecDependency");
      String repository = null;
      String name = null;
      String effective = null;
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "repository" -> repository = JsonReading.readString(p);
          case "name" -> name = JsonReading.readString(p);
          case "effective" -> effective = JsonReading.readString(p);
          default -> JsonReading.unknownField(field, "SpecDependency");
        }
      }
      if (name == null) {
        JsonReading.missingRequired("name", "SpecDependency");
      }
      return new SpecDependency(repository, name, effective);
    }
  }

  /**
   * SpecVersion.
   * @param name name
   * @param effectiveFrom effectiveFrom
   * @param effectiveTo effectiveTo
   * @param dependencies uses targets in declaration order
   */
  public record SpecVersion(
      String name,
      @Nullable String effectiveFrom,
      @Nullable String effectiveTo,
      List<SpecDependency> dependencies) {
    /**
     * Parses JSON.
     *
     * @param p parser at value start
     * @return parsed value
     * @throws IOException if JSON IO fails
     */
    static SpecVersion read(JsonParser p) throws IOException {
      JsonReading.expectStartObject(p, "SpecVersion");
      String name = null;
      String effectiveFrom = null;
      String effectiveTo = null;
      List<SpecDependency> dependencies = List.of();
      while (p.nextToken() != JsonToken.END_OBJECT) {
        String field = p.currentName();
        p.nextToken();
        switch (field) {
          case "name" -> name = JsonReading.readString(p);
          case "effective_from" -> effectiveFrom = JsonReading.readString(p);
          case "effective_to" -> effectiveTo = JsonReading.readString(p);
          case "dependencies" -> dependencies = JsonReading.readList(p, SpecDependency::read);
          default -> JsonReading.unknownField(field, "SpecVersion");
        }
      }
      if (name == null) {
        JsonReading.missingRequired("name", "SpecVersion");
      }
      return new SpecVersion(name, effectiveFrom, effectiveTo, dependencies);
    }
  }

  /**
   * Parses JSON.
   *
   * @param p parser at value start
   * @return parsed value
   * @throws IOException if JSON IO fails
   */
  static Repository read(JsonParser p) throws IOException {
    JsonReading.expectStartObject(p, "Repository");
    String repository = null;
    List<SpecVersion> specs = null;
    while (p.nextToken() != JsonToken.END_OBJECT) {
      String field = p.currentName();
      p.nextToken();
      switch (field) {
        case "repository" -> repository = JsonReading.readString(p);
        case "specs" -> specs = JsonReading.readList(p, SpecVersion::read);
        default -> JsonReading.unknownField(field, "Repository");
      }
    }
    if (specs == null) {
      JsonReading.missingRequired("specs", "Repository");
    }
    return new Repository(repository, specs);
  }
}

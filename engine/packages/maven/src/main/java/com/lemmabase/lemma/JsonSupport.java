package com.lemmabase.lemma;

import com.fasterxml.jackson.core.JsonParser;
import java.io.IOException;
import java.util.List;

/** JSON parse helpers for typed SDK responses. */
final class JsonSupport {
  /** Prevents instantiation. */
  private JsonSupport() {}

  /**
   * Parses a JSON array of engine errors.
   *
   * @param json JSON array text
   */
  static List<EngineError> parseEngineErrors(String json) {
    try (JsonParser p = JsonReading.parserFor(json)) {
      return JsonReading.readList(p, EngineError::read);
    } catch (IOException e) {
      throw new LemmaBugError("BUG: failed to parse EngineError JSON: " + e.getMessage());
    }
  }

  /**
   * Parses a run {@link Response}.
   *
   * @param json response JSON object
   */
  static Response parseResponse(String json) {
    try (JsonParser p = JsonReading.parserFor(json)) {
      return Response.read(p);
    } catch (IOException e) {
      throw new LemmaBugError("BUG: failed to parse Response JSON: " + e.getMessage());
    }
  }

  /**
   * Parses a {@link Show} result.
   *
   * @param json show JSON object
   */
  static Show parseShow(String json) {
    try (JsonParser p = JsonReading.parserFor(json)) {
      return Show.read(p, json);
    } catch (IOException e) {
      throw new LemmaBugError("BUG: failed to parse Show JSON: " + e.getMessage());
    }
  }

  /**
   * Parses a JSON array of {@link Repository}.
   *
   * @param json JSON array text
   */
  static List<Repository> parseRepositories(String json) {
    try (JsonParser p = JsonReading.parserFor(json)) {
      return JsonReading.readList(p, Repository::read);
    } catch (IOException e) {
      throw new LemmaBugError("BUG: failed to parse specs JSON: " + e.getMessage());
    }
  }

  /**
   * Parses a {@link RepositoryInstallResult}.
   *
   * @param json install result JSON object
   */
  static RepositoryInstallResult parseInstallResult(String json) {
    try (JsonParser p = JsonReading.parserFor(json)) {
      return RepositoryInstallResult.read(p);
    } catch (IOException e) {
      throw new LemmaBugError("BUG: failed to parse RepositoryInstallResult JSON: " + e.getMessage());
    }
  }

  /**
   * Parses {@link ResourceLimits}.
   *
   * @param json limits JSON object
   */
  static ResourceLimits parseLimits(String json) {
    try (JsonParser p = JsonReading.parserFor(json)) {
      return ResourceLimits.read(p);
    } catch (IOException e) {
      throw new LemmaBugError("BUG: failed to parse ResourceLimits JSON: " + e.getMessage());
    }
  }

  /**
   * Parses a JSON array of {@link Recommendation}.
   *
   * @param json JSON array text
   */
  static List<Recommendation> parseRecommendations(String json) {
    try (JsonParser p = JsonReading.parserFor(json)) {
      return JsonReading.readList(p, Recommendation::read);
    } catch (IOException e) {
      throw new LemmaBugError("BUG: failed to parse Recommendation JSON: " + e.getMessage());
    }
  }

  /**
   * Serializes limits for JNI.
   *
   * @param limits limits to serialize
   */
  static String limitsToJson(ResourceLimits limits) {
    return limits.toJson();
  }
}

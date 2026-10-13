package com.lemmabase.lemma;

import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import org.jspecify.annotations.Nullable;

/**
 * Named arguments for {@link Engine#evaluate(EvaluateRequest)}. Defaults: repository null,
 * effective null, rules null (all rules), explain false.
 */
public class EvaluateRequest {
  private final String spec;
  private final @Nullable String repository;
  private final @Nullable String effective;
  private final Map<String, ?> data;
  private final @Nullable List<String> rules;
  private final boolean explain;

  EvaluateRequest(
      String spec,
      @Nullable String repository,
      @Nullable String effective,
      Map<String, ?> data,
      @Nullable List<String> rules,
      boolean explain) {
    this.spec = Objects.requireNonNull(spec, "spec");
    this.repository = repository;
    this.effective = effective;
    // Not Map.copyOf: callers may pass a null value (e.g. an explicit override probe) and
    // must get back an attributed LemmaException from the JNI tree walk, not a bare
    // NullPointerException from the immutable-map constructor.
    this.data =
        Collections.unmodifiableMap(new LinkedHashMap<>(Objects.requireNonNull(data, "data")));
    this.rules = rules == null ? null : List.copyOf(rules);
    this.explain = explain;
  }

  /**
   * Creates an evaluate request for one spec with empty data.
   *
   * @param spec spec name
   * @return new request
   */
  public static EvaluateRequest of(String spec) {
    return new EvaluateRequest(spec, null, null, Map.of(), null, false);
  }

  /**
   * Sets the repository handle.
   *
   * @param repository repository handle; {@code null} for default
   * @return copy with updated repository
   */
  public EvaluateRequest repository(@Nullable String repository) {
    return new EvaluateRequest(spec, repository, effective, data, rules, explain);
  }

  /**
   * Sets the effective date.
   *
   * @param effective effective date; {@code null} for latest
   * @return copy with updated effective date
   */
  public EvaluateRequest effective(@Nullable String effective) {
    return new EvaluateRequest(spec, repository, effective, data, rules, explain);
  }

  /**
   * Sets input data bindings.
   *
   * @param data data map
   * @return copy with updated data
   */
  public EvaluateRequest data(Map<String, ?> data) {
    return new EvaluateRequest(spec, repository, effective, data, rules, explain);
  }

  /**
   * Sets the rule subset to evaluate.
   *
   * @param rules rule names; {@code null} for all rules
   * @return copy with updated rules
   */
  public EvaluateRequest rules(@Nullable List<String> rules) {
    return new EvaluateRequest(spec, repository, effective, data, rules, explain);
  }

  /**
   * Sets explain mode.
   *
   * @param explain whether to include explanations
   * @return copy with updated explain flag
   */
  public EvaluateRequest explain(boolean explain) {
    return new EvaluateRequest(spec, repository, effective, data, rules, explain);
  }

  /**
   * Spec name.
   *
   * @return spec name
   */
  public String spec() {
    return spec;
  }

  /**
   * Repository handle.
   *
   * @return repository handle or null
   */
  public @Nullable String repository() {
    return repository;
  }

  /**
   * Effective date.
   *
   * @return effective date or null
   */
  public @Nullable String effective() {
    return effective;
  }

  /**
   * Input data bindings.
   *
   * @return data map
   */
  public Map<String, ?> data() {
    return data;
  }

  /**
   * Rule subset; {@code null} means all rules.
   *
   * @return rule names or null
   */
  public @Nullable List<String> rules() {
    return rules;
  }

  /**
   * Whether explain mode is enabled.
   *
   * @return explain flag
   */
  public boolean explain() {
    return explain;
  }
}

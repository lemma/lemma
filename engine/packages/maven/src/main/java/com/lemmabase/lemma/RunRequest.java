package com.lemmabase.lemma;

import java.util.List;
import java.util.Map;
import org.jspecify.annotations.Nullable;

/**
 * Deprecated alias of {@link EvaluateRequest}.
 *
 * @deprecated Use {@link EvaluateRequest}
 */
@Deprecated
public final class RunRequest extends EvaluateRequest {
  private RunRequest(
      String spec,
      @Nullable String repository,
      @Nullable String effective,
      Map<String, ?> data,
      @Nullable List<String> rules,
      boolean explain) {
    super(spec, repository, effective, data, rules, explain);
  }

  /**
   * Creates a request for one spec with empty data.
   *
   * @param spec spec name
   * @return new request
   * @deprecated Use {@link EvaluateRequest#of(String)}
   */
  @Deprecated
  public static RunRequest of(String spec) {
    return new RunRequest(spec, null, null, Map.of(), null, false);
  }

  /**
   * Sets the repository handle.
   *
   * @param repository repository handle; {@code null} for default
   * @return copy with updated repository
   * @deprecated Use {@link EvaluateRequest#repository(String)}
   */
  @Override
  @Deprecated
  public RunRequest repository(@Nullable String repository) {
    return new RunRequest(spec(), repository, effective(), data(), rules(), explain());
  }

  /**
   * Sets the effective date.
   *
   * @param effective effective date; {@code null} for latest
   * @return copy with updated effective date
   * @deprecated Use {@link EvaluateRequest#effective(String)}
   */
  @Override
  @Deprecated
  public RunRequest effective(@Nullable String effective) {
    return new RunRequest(spec(), repository(), effective, data(), rules(), explain());
  }

  /**
   * Sets input data bindings.
   *
   * @param data data map
   * @return copy with updated data
   * @deprecated Use {@link EvaluateRequest#data(Map)}
   */
  @Override
  @Deprecated
  public RunRequest data(Map<String, ?> data) {
    return new RunRequest(spec(), repository(), effective(), data, rules(), explain());
  }

  /**
   * Sets the rule subset to evaluate.
   *
   * @param rules rule names; {@code null} for all rules
   * @return copy with updated rules
   * @deprecated Use {@link EvaluateRequest#rules(List)}
   */
  @Override
  @Deprecated
  public RunRequest rules(@Nullable List<String> rules) {
    return new RunRequest(spec(), repository(), effective(), data(), rules, explain());
  }

  /**
   * Sets explain mode.
   *
   * @param explain whether to include explanations
   * @return copy with updated explain flag
   * @deprecated Use {@link EvaluateRequest#explain(boolean)}
   */
  @Override
  @Deprecated
  public RunRequest explain(boolean explain) {
    return new RunRequest(spec(), repository(), effective(), data(), rules(), explain);
  }
}

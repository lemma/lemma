using System.Text.Json;

namespace Lemmabase.Lemma.Engine;

public sealed class EngineError
{
    public required string Kind { get; init; }
    public required string Message { get; init; }
    public string? RelatedData { get; init; }
    public string? Spec { get; init; }
    public string? RelatedSpec { get; init; }
    public EngineErrorSource? Source { get; init; }
    public string? Suggestion { get; init; }
    public string? Repository { get; init; }
    public string? RegistryKind { get; init; }
    public string? RequestKind { get; init; }
    public string? LimitName { get; init; }
    public string? LimitValue { get; init; }
    public string? ActualValue { get; init; }
}

public sealed class EngineErrorSource
{
    public required string Attribute { get; init; }
    public required int Line { get; init; }
    public required int Column { get; init; }
    public required int Length { get; init; }
}

public sealed class Response
{
    public required string Spec { get; init; }
    public required string Effective { get; init; }
    public string? SpecEffectiveFrom { get; init; }
    public string? SpecEffectiveTo { get; init; }
    public required Dictionary<string, RuleResult> Results { get; init; }
}

/// <summary>Show document. Nested type graphs stay as <see cref="JsonElement"/>.</summary>
public sealed class Show
{
    public string? Repository { get; init; }
    public required string Spec { get; init; }
    public string? Commentary { get; init; }
    public string? EffectiveFrom { get; init; }
    public string? EffectiveTo { get; init; }
    public JsonElement? Versions { get; init; }
    public required int StartLine { get; init; }
    public JsonElement? SourceType { get; init; }
    public required JsonElement Data { get; init; }
    public required JsonElement Rules { get; init; }
    public required JsonElement Meta { get; init; }
}

public sealed class RepositoryInstallResult
{
    public required string Source { get; init; }
    public required string Id { get; init; }
}

public sealed class Recommendation
{
    public required string Message { get; init; }
    public string? Repository { get; init; }
    public required string Spec { get; init; }
    public string? EffectiveFrom { get; init; }
    public required EngineErrorSource Source { get; init; }
}

public sealed class ResourceLimits
{
    public required long MaxSourceSizeBytes { get; init; }
    public required long MaxExpressionDepth { get; init; }
    public required long MaxExpressionCount { get; init; }
    public required long MaxDataValueBytes { get; init; }
    public required long MaxLoadedBytes { get; init; }
    public required long MaxSources { get; init; }
    public required long MaxNormalizedExpressionNodes { get; init; }
    public required long MaxSpecDependencyDepth { get; init; }
    public required long MaxDagSpecs { get; init; }
    public required long MaxNormalFormDepth { get; init; }

    public static Builder CreateBuilder() => new();

    public sealed class Builder
    {
        private readonly Dictionary<string, long> _overrides = new();

        public Builder MaxSourceSizeBytes(long value)
        {
            _overrides["max_source_size_bytes"] = value;
            return this;
        }

        public Builder MaxExpressionDepth(long value)
        {
            _overrides["max_expression_depth"] = value;
            return this;
        }

        public Builder MaxExpressionCount(long value)
        {
            _overrides["max_expression_count"] = value;
            return this;
        }

        public Builder MaxDataValueBytes(long value)
        {
            _overrides["max_data_value_bytes"] = value;
            return this;
        }

        public Builder MaxLoadedBytes(long value)
        {
            _overrides["max_loaded_bytes"] = value;
            return this;
        }

        public Builder MaxSources(long value)
        {
            _overrides["max_sources"] = value;
            return this;
        }

        public Builder MaxNormalizedExpressionNodes(long value)
        {
            _overrides["max_normalized_expression_nodes"] = value;
            return this;
        }

        public Builder MaxSpecDependencyDepth(long value)
        {
            _overrides["max_spec_dependency_depth"] = value;
            return this;
        }

        public Builder MaxDagSpecs(long value)
        {
            _overrides["max_dag_specs"] = value;
            return this;
        }

        public Builder MaxNormalFormDepth(long value)
        {
            _overrides["max_normal_form_depth"] = value;
            return this;
        }

        internal string ToJson()
        {
            return JsonSerializer.Serialize(_overrides);
        }
    }
}

public sealed class ListedSpec
{
    public required string Name { get; init; }
    public string? EffectiveFrom { get; init; }
    public string? EffectiveTo { get; init; }
}

public sealed class ResolvedRepository
{
    public string? Repository { get; init; }
    public required List<ListedSpec> Specs { get; init; }
}

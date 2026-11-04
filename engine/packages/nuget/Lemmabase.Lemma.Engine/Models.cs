using System.Text.Json;
using System.Text.Json.Serialization;

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

/// <summary>
/// Show: the engine's Draft 2020-12 JSON Schema for a spec's evaluation-data inputs, read into
/// typed members. <see cref="Json"/> is the document itself, for JSON Schema tooling. Lemma types,
/// values and rule expressions stay as <see cref="JsonElement"/>.
/// </summary>
public sealed class Show : IJsonOnDeserialized
{
    private const string Dialect = "https://json-schema.org/draft/2020-12/schema";

    private string? _json;

    [JsonInclude]
    [JsonPropertyName("$schema")]
    private string? SchemaDialect { get; init; }

    [JsonInclude]
    [JsonPropertyName("type")]
    private string? SchemaType { get; init; }

    [JsonInclude]
    [JsonPropertyName("additionalProperties")]
    private bool? SchemaAdditionalProperties { get; init; }

    /// <summary>Interned repository name (<c>x-lemma-repository</c>); null for the unnamed workspace.</summary>
    [JsonPropertyName("x-lemma-repository")]
    public string? Repository { get; init; }

    /// <summary>Spec name (<c>title</c>).</summary>
    [JsonPropertyName("title")]
    public required string Spec { get; init; }

    /// <summary>Spec commentary (<c>description</c>).</summary>
    [JsonPropertyName("description")]
    public string? Commentary { get; init; }

    [JsonPropertyName("x-lemma-effective-from")]
    public string? EffectiveFrom { get; init; }

    [JsonPropertyName("x-lemma-effective-to")]
    public string? EffectiveTo { get; init; }

    [JsonPropertyName("x-lemma-start-line")]
    public required int StartLine { get; init; }

    [JsonPropertyName("x-lemma-source-type")]
    public JsonElement? SourceType { get; init; }

    /// <summary>Local data declarations by name (<c>x-lemma-types</c>).</summary>
    [JsonPropertyName("x-lemma-types")]
    public required IReadOnlyDictionary<string, JsonElement> Types { get; init; }

    /// <summary>Inputs rules need, by evaluation-data key (<c>properties</c>).</summary>
    [JsonPropertyName("properties")]
    public required IReadOnlyDictionary<string, ShowData> Data { get; init; }

    /// <summary>Inputs a rule needs that have no committed value, sorted.</summary>
    [JsonPropertyName("required")]
    public IReadOnlyList<string> Required { get; init; } = [];

    /// <summary>Instance-list row schemas by <c>$defs</c> name.</summary>
    [JsonPropertyName("$defs")]
    public IReadOnlyDictionary<string, ShowRow> Rows { get; init; } = new Dictionary<string, ShowRow>();

    /// <summary>
    /// Per <c>$defs</c> row schema name: row field to the <see cref="Rules"/> keys that need it
    /// through an instance list (<c>x-lemma-row-field-needed-by-rules</c>). Fields no rule needs are absent.
    /// </summary>
    [JsonPropertyName("x-lemma-row-field-needed-by-rules")]
    public IReadOnlyDictionary<string, IReadOnlyDictionary<string, IReadOnlyList<string>>> RowFieldNeededByRules { get; init; } =
        new Dictionary<string, IReadOnlyDictionary<string, IReadOnlyList<string>>>();

    /// <summary>This spec's rule graph, local plus reachable imports (<c>x-lemma-rules</c>).</summary>
    [JsonPropertyName("x-lemma-rules")]
    public required IReadOnlyDictionary<string, ShowRule> Rules { get; init; }

    [JsonPropertyName("x-lemma-meta")]
    public required IReadOnlyDictionary<string, JsonElement> Meta { get; init; }

    /// <summary>The Show document.</summary>
    [JsonIgnore]
    public string Json => _json ?? throw new LemmaBugException("BUG: Show.Json is set by Show.Parse");

    internal static Show Parse(string json)
    {
        var show = LemmaJson.Deserialize<Show>(json);
        show._json = json;
        return show;
    }

    void IJsonOnDeserialized.OnDeserialized()
    {
        if (SchemaDialect != Dialect)
        {
            throw new LemmaBugException($"BUG: Show.$schema must be {Dialect}, got {SchemaDialect}");
        }
        if (SchemaType != "object")
        {
            throw new LemmaBugException($"BUG: Show.type must be object, got {SchemaType}");
        }
        if (SchemaAdditionalProperties != true)
        {
            throw new LemmaBugException("BUG: Show.additionalProperties must be true");
        }
    }
}

/// <summary>
/// One hop on a Show data or rule path: a uses local name, or (with <see cref="Row"/>) the row
/// written at that 1-based position in the data list named <see cref="Uses"/>.
/// </summary>
public sealed class PathSegment
{
    public required string Uses { get; init; }
    public int? Row { get; init; }

    /// <summary>Target repository name; null for the unnamed workspace.</summary>
    public string? Repository { get; init; }

    /// <summary>Resolved target spec name.</summary>
    public required string Spec { get; init; }
}

/// <summary>One evaluation-data input: a property of the Show document or of a row schema.</summary>
public sealed class ShowData : IJsonOnDeserialized
{
    /// <summary>JSON type: <c>boolean</c>, <c>string</c> or <c>array</c>; null when <see cref="OneOf"/> states the shape.</summary>
    [JsonPropertyName("type")]
    public string? Type { get; init; }

    [JsonPropertyName("oneOf")]
    public JsonElement? OneOf { get; init; }

    /// <summary>Literal grammar of number, measure and ratio inputs (ECMA-262).</summary>
    [JsonPropertyName("pattern")]
    public string? Pattern { get; init; }

    /// <summary>Text options.</summary>
    [JsonPropertyName("enum")]
    public IReadOnlyList<string>? Enum { get; init; }

    [JsonPropertyName("minLength")]
    public long? MinLength { get; init; }

    [JsonPropertyName("maxLength")]
    public long? MaxLength { get; init; }

    /// <summary>Type help.</summary>
    [JsonPropertyName("description")]
    public string? Description { get; init; }

    /// <summary>Evaluation-data JSON of <see cref="Value"/>.</summary>
    [JsonPropertyName("default")]
    public JsonElement? Default { get; init; }

    /// <summary>Evaluation-data JSON of <see cref="Suggestion"/>.</summary>
    [JsonPropertyName("examples")]
    public JsonElement? Examples { get; init; }

    [JsonPropertyName("items")]
    public JsonElement? Items { get; init; }

    [JsonPropertyName("maxItems")]
    public long? MaxItems { get; init; }

    /// <summary>Lemma type without what the standard keywords state (<c>x-lemma-type</c>).</summary>
    [JsonPropertyName("x-lemma-type")]
    public required JsonElement LemmaType { get; init; }

    /// <summary>Import hops to this slot; empty means this spec.</summary>
    [JsonPropertyName("x-lemma-path")]
    public required IReadOnlyList<PathSegment> Path { get; init; }

    /// <summary><see cref="Show.Rules"/> keys that need this slot; empty means reuse only. Always empty on row schemas.</summary>
    [JsonPropertyName("x-lemma-needed-by-rules")]
    public required IReadOnlyList<string> NeededByRules { get; init; }

    /// <summary>Spec literal or literal <c>with</c> binding (<c>x-lemma-value</c>).</summary>
    [JsonPropertyName("x-lemma-value")]
    public JsonElement? Value { get; init; }

    [JsonPropertyName("x-lemma-suggestion")]
    public JsonElement? Suggestion { get; init; }

    void IJsonOnDeserialized.OnDeserialized()
    {
        if (Type is null && OneOf is null)
        {
            throw new LemmaBugException("BUG: ShowProperty must have type or oneOf");
        }
    }
}

/// <summary>Row object schema of an instance list: one <c>$defs</c> entry per row spec.</summary>
public sealed class ShowRow : IJsonOnDeserialized
{
    [JsonInclude]
    [JsonPropertyName("type")]
    private string? SchemaType { get; init; }

    [JsonInclude]
    [JsonPropertyName("additionalProperties")]
    private bool? SchemaAdditionalProperties { get; init; }

    public required string Title { get; init; }

    /// <summary>Row fields by key relative to the row spec (<c>properties</c>).</summary>
    [JsonPropertyName("properties")]
    public required IReadOnlyDictionary<string, ShowData> Data { get; init; }

    /// <summary>Row fields a row rule needs that have no committed value, sorted.</summary>
    public IReadOnlyList<string> Required { get; init; } = [];

    void IJsonOnDeserialized.OnDeserialized()
    {
        if (SchemaType != "object")
        {
            throw new LemmaBugException($"BUG: ShowRowSchema.type must be object, got {SchemaType}");
        }
        if (SchemaAdditionalProperties != true)
        {
            throw new LemmaBugException("BUG: ShowRowSchema.additionalProperties must be true");
        }
    }
}

/// <summary>Rule on Show: result type, path, authored branches and the rules it depends on.</summary>
public sealed class ShowRule
{
    /// <summary>Result type (a Lemma type).</summary>
    public required JsonElement Type { get; init; }

    /// <summary>Import hops to this rule; empty means this spec.</summary>
    public required IReadOnlyList<PathSegment> Path { get; init; }

    /// <summary>Default arm, then unless arms.</summary>
    public required IReadOnlyList<ShowBranch> Branches { get; init; }

    /// <summary><see cref="Show.Rules"/> keys this rule depends on.</summary>
    public required IReadOnlyList<string> DependsOnRules { get; init; }
}

/// <summary>One arm of a rule's flat last-match table. The default arm has no condition.</summary>
public sealed class ShowBranch
{
    /// <summary>Show expression of the arm's condition.</summary>
    public JsonElement? Condition { get; init; }

    /// <summary>Show expression of the arm's result.</summary>
    public required JsonElement Result { get; init; }
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
    public required long MaxListRows { get; init; }
    public required long MaxListDepth { get; init; }
    public required long MaxListNodes { get; init; }

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

        public Builder MaxListRows(long value)
        {
            _overrides["max_list_rows"] = value;
            return this;
        }

        public Builder MaxListDepth(long value)
        {
            _overrides["max_list_depth"] = value;
            return this;
        }

        public Builder MaxListNodes(long value)
        {
            _overrides["max_list_nodes"] = value;
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

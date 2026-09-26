using System.Globalization;
using Lemmabase.Lemma.Engine.Native;

namespace Lemmabase.Lemma.Engine;

/// <summary>Named arguments for <see cref="Engine.Run"/>.</summary>
public sealed class RunRequest
{
    private RunRequest(
        string spec,
        string? repository,
        string? effective,
        IReadOnlyDictionary<string, object?> data,
        IReadOnlyList<string>? rules,
        bool explain
    )
    {
        Spec = spec;
        Repository = repository;
        Effective = effective;
        Data = data;
        Rules = rules;
        Explain = explain;
    }

    public string Spec { get; }
    public string? Repository { get; }
    public string? Effective { get; }
    public IReadOnlyDictionary<string, object?> Data { get; }
    public IReadOnlyList<string>? Rules { get; }
    public bool Explain { get; }

    public static RunRequest Of(string spec)
    {
        ArgumentNullException.ThrowIfNull(spec);
        return new RunRequest(spec, null, null, new Dictionary<string, object?>(), null, false);
    }

    public RunRequest WithRepository(string? repository) =>
        new(Spec, repository, Effective, Data, Rules, Explain);

    public RunRequest WithEffective(string? effective) =>
        new(Spec, Repository, effective, Data, Rules, Explain);

    public RunRequest WithData(IReadOnlyDictionary<string, object?> data)
    {
        ArgumentNullException.ThrowIfNull(data);
        return new(Spec, Repository, Effective, data, Rules, Explain);
    }

    public RunRequest WithRules(IReadOnlyList<string>? rules) =>
        new(Spec, Repository, Effective, Data, rules, Explain);

    public RunRequest WithExplain(bool explain) =>
        new(Spec, Repository, Effective, Data, Rules, explain);

    internal Dictionary<string, string> ToEngineStrings()
    {
        var result = new Dictionary<string, string>();
        foreach (var (key, value) in Data)
        {
            result[key] = Coerce(key, value);
        }
        return result;
    }

    private static string Coerce(string key, object? value)
    {
        if (value is null)
        {
            throw Request($"data '{key}' must not be null", key);
        }

        switch (value)
        {
            case string text:
                return text;
            case decimal number:
                return number.ToString(CultureInfo.InvariantCulture);
            case byte or sbyte or short or ushort or int or uint or long:
                return Convert.ToInt64(value, CultureInfo.InvariantCulture).ToString(CultureInfo.InvariantCulture);
            case ulong ulongValue:
                return ulongValue.ToString(CultureInfo.InvariantCulture);
            case float or double:
                throw Request(
                    "decimal values must be passed as decimal (or string) to preserve exactness; float/double are rejected",
                    key
                );
            case bool boolean:
                return boolean ? "true" : "false";
            default:
                throw Request(
                    $"data '{key}' has unsupported type {value.GetType().FullName}; use string, decimal, integer, or bool",
                    key
                );
        }
    }

    private static LemmaException Request(string message, string? relatedData)
    {
        var json = LemmaDotnet.RequestErrorJson(message, relatedData);
        return new LemmaException(message, json);
    }
}

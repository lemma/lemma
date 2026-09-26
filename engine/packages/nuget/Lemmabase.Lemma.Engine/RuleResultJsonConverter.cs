using System.Globalization;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace Lemmabase.Lemma.Engine;

internal sealed class RuleResultJsonConverter : JsonConverter<RuleResult>
{
    public override RuleResult Read(ref Utf8JsonReader reader, Type typeToConvert, JsonSerializerOptions options)
    {
        using var document = JsonDocument.ParseValue(ref reader);
        var root = document.RootElement;
        if (root.ValueKind != JsonValueKind.Object)
        {
            throw new LemmaBugException("BUG: RuleResult JSON must be an object");
        }

        var ruleType = RequireString(root, "rule_type");
        JsonElement? explanation = null;
        if (root.TryGetProperty("explanation", out var explanationElement)
            && explanationElement.ValueKind != JsonValueKind.Null)
        {
            explanation = explanationElement.Clone();
        }

        var vetoed = RequireBool(root, "vetoed");
        if (vetoed)
        {
            string? reason = null;
            if (root.TryGetProperty("veto_reason", out var reasonElement)
                && reasonElement.ValueKind != JsonValueKind.Null)
            {
                reason = reasonElement.GetString();
            }

            if (root.TryGetProperty("missing_data", out var missingElement)
                && missingElement.ValueKind == JsonValueKind.Array
                && missingElement.GetArrayLength() > 0)
            {
                var missing = new List<string>();
                foreach (var item in missingElement.EnumerateArray())
                {
                    if (item.ValueKind != JsonValueKind.String)
                    {
                        throw new LemmaBugException("BUG: missing_data entries must be strings");
                    }
                    missing.Add(item.GetString() ?? throw new LemmaBugException("BUG: missing_data null string"));
                }
                return new RuleResult.MissingData(missing, ruleType, explanation);
            }

            return new RuleResult.Veto(reason, ruleType, explanation);
        }

        if (root.TryGetProperty("number", out var numberElement) && numberElement.ValueKind != JsonValueKind.Null)
        {
            return new RuleResult.Number(
                OptionalDisplay(root),
                ReadDecimal(numberElement, "number"),
                ruleType,
                explanation
            );
        }

        if (root.TryGetProperty("text", out var textElement) && textElement.ValueKind != JsonValueKind.Null)
        {
            return new RuleResult.Text(
                OptionalDisplay(root),
                textElement.GetString() ?? throw new LemmaBugException("BUG: text null"),
                ruleType,
                explanation
            );
        }

        if (root.TryGetProperty("boolean", out var booleanElement) && booleanElement.ValueKind != JsonValueKind.Null)
        {
            return new RuleResult.BooleanValue(
                OptionalDisplay(root),
                booleanElement.GetBoolean(),
                ruleType,
                explanation
            );
        }

        if (root.TryGetProperty("date", out var dateElement) && dateElement.ValueKind != JsonValueKind.Null)
        {
            return new RuleResult.Date(
                OptionalDisplay(root),
                dateElement.GetString() ?? throw new LemmaBugException("BUG: date null"),
                ruleType,
                explanation
            );
        }

        if (root.TryGetProperty("time", out var timeElement) && timeElement.ValueKind != JsonValueKind.Null)
        {
            return new RuleResult.Time(
                OptionalDisplay(root),
                timeElement.GetString() ?? throw new LemmaBugException("BUG: time null"),
                ruleType,
                explanation
            );
        }

        if (root.TryGetProperty("measure", out var measureElement) && measureElement.ValueKind != JsonValueKind.Null)
        {
            return new RuleResult.Measure(
                OptionalDisplay(root),
                ReadUnitMap(measureElement, "measure"),
                ruleType,
                explanation
            );
        }

        if (root.TryGetProperty("ratio", out var ratioElement) && ratioElement.ValueKind != JsonValueKind.Null)
        {
            return new RuleResult.Ratio(
                OptionalDisplay(root),
                ReadUnitMap(ratioElement, "ratio"),
                ruleType,
                explanation
            );
        }

        if (root.TryGetProperty("range", out var rangeElement) && rangeElement.ValueKind != JsonValueKind.Null)
        {
            if (!rangeElement.TryGetProperty("from", out var from) || !rangeElement.TryGetProperty("to", out var to))
            {
                throw new LemmaBugException("BUG: range missing from/to");
            }
            return new RuleResult.Range(OptionalDisplay(root), from.Clone(), to.Clone(), ruleType, explanation);
        }

        if (root.TryGetProperty("calendar", out var calendarElement) && calendarElement.ValueKind != JsonValueKind.Null)
        {
            if (!calendarElement.TryGetProperty("value", out var valueElement)
                || !calendarElement.TryGetProperty("unit", out var unitElement))
            {
                throw new LemmaBugException("BUG: calendar missing value/unit");
            }
            return new RuleResult.Calendar(
                OptionalDisplay(root),
                ReadDecimal(valueElement, "calendar.value"),
                unitElement.GetString() ?? throw new LemmaBugException("BUG: calendar.unit null"),
                ruleType,
                explanation
            );
        }

        throw new LemmaBugException($"BUG: RuleResult has no typed value payload: {root}");
    }

    public override void Write(Utf8JsonWriter writer, RuleResult value, JsonSerializerOptions options)
    {
        throw new NotSupportedException("RuleResult serialization is not supported");
    }

    private static string OptionalDisplay(JsonElement root)
    {
        if (root.TryGetProperty("result", out var result) && result.ValueKind == JsonValueKind.String)
        {
            return result.GetString() ?? string.Empty;
        }
        return string.Empty;
    }

    private static string RequireString(JsonElement root, string name)
    {
        if (!root.TryGetProperty(name, out var element) || element.ValueKind != JsonValueKind.String)
        {
            throw new LemmaBugException($"BUG: RuleResult missing string '{name}'");
        }
        return element.GetString() ?? throw new LemmaBugException($"BUG: RuleResult '{name}' null");
    }

    private static bool RequireBool(JsonElement root, string name)
    {
        if (!root.TryGetProperty(name, out var element) || element.ValueKind is not (JsonValueKind.True or JsonValueKind.False))
        {
            throw new LemmaBugException($"BUG: RuleResult missing bool '{name}'");
        }
        return element.GetBoolean();
    }

    private static decimal ReadDecimal(JsonElement element, string field)
    {
        if (element.ValueKind == JsonValueKind.Number && element.TryGetDecimal(out var number))
        {
            return number;
        }
        if (element.ValueKind == JsonValueKind.String)
        {
            var text = element.GetString()
                ?? throw new LemmaBugException($"BUG: {field} null string");
            if (decimal.TryParse(text, NumberStyles.Number, CultureInfo.InvariantCulture, out var parsed))
            {
                return parsed;
            }
        }
        throw new LemmaBugException($"BUG: {field} is not a decimal");
    }

    private static IReadOnlyDictionary<string, decimal> ReadUnitMap(JsonElement element, string field)
    {
        if (element.ValueKind != JsonValueKind.Object)
        {
            throw new LemmaBugException($"BUG: {field} must be an object");
        }
        var map = new Dictionary<string, decimal>();
        foreach (var property in element.EnumerateObject())
        {
            map[property.Name] = ReadDecimal(property.Value, $"{field}.{property.Name}");
        }
        return map;
    }
}

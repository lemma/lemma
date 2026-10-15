using System.Text.Json;
using System.Text.Json.Serialization;

namespace Lemmabase.Lemma.Engine;

internal static class LemmaJson
{
    internal static readonly JsonSerializerOptions Options = CreateOptions();

    private static JsonSerializerOptions CreateOptions()
    {
        var options = new JsonSerializerOptions
        {
            PropertyNamingPolicy = JsonNamingPolicy.SnakeCaseLower,
            UnmappedMemberHandling = JsonUnmappedMemberHandling.Disallow,
            NumberHandling = JsonNumberHandling.AllowReadingFromString,
        };
        options.Converters.Add(new RuleResultJsonConverter());
        return options;
    }

    internal static T Deserialize<T>(string json)
    {
        try
        {
            var value = JsonSerializer.Deserialize<T>(json, Options);
            if (value is null)
            {
                throw new LemmaBugException($"BUG: JSON deserialized to null for {typeof(T).Name}");
            }
            return value;
        }
        catch (LemmaException)
        {
            throw;
        }
        catch (LemmaBugException)
        {
            throw;
        }
        catch (Exception ex)
        {
            throw new LemmaBugException($"BUG: failed to deserialize {typeof(T).Name}: {ex.Message}", ex);
        }
    }

    internal static List<EngineError> DeserializeEngineErrors(string errorsJson)
    {
        return Deserialize<List<EngineError>>(errorsJson);
    }
}

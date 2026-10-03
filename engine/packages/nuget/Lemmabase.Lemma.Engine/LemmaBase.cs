using System.Net.Http.Headers;
using System.Text.Json;
using Lemmabase.Lemma.Engine.Native;

namespace Lemmabase.Lemma.Engine;

/// <summary>Host-side LemmaBase HTTP loop over UniFFI install verbs.</summary>
internal static class LemmaBase
{
    private static readonly HttpClient DefaultClient = CreateDefaultClient();

    private static HttpClient CreateDefaultClient()
    {
        var client = new HttpClient();
        client.Timeout = TimeSpan.FromSeconds(30);
        return client;
    }

    internal static RepositoryInstallResult Install(string repository)
    {
        return Install(repository, DefaultClient);
    }

    internal static RepositoryInstallResult Install(string repository, HttpClient client)
    {
        ArgumentNullException.ThrowIfNull(repository);
        ArgumentNullException.ThrowIfNull(client);

        NativeLibraryLoader.EnsureLoaded();
        using var engine = NativeEngine.Create();
        return InstallWithEngine(engine, repository, client);
    }

    internal static RepositoryInstallResult InstallWithEngine(
        NativeEngine engine,
        string repository,
        HttpClient client
    )
    {
        InstallStart start;
        try
        {
            start = engine.InstallStart(repository);
        }
        catch (LemmaFfiException.Engine engineError)
        {
            throw new LemmaException(engineError.message, engineError.errorsJson);
        }
        catch (LemmaFfiException.Bug bug)
        {
            throw new LemmaBugException(bug.message);
        }
        catch (PanicException panic)
        {
            throw new LemmaBugException($"BUG: Rust panic crossed UniFFI boundary: {panic.Message}", panic);
        }

        using (start.Session)
        {
            var stepJson = start.StepJson;
            while (true)
            {
                using var document = JsonDocument.Parse(stepJson);
                var root = document.RootElement;
                if (root.TryGetProperty("finished", out var finished))
                {
                    return ParseFinished(finished);
                }

                if (!root.TryGetProperty("fetch", out var fetch))
                {
                    throw new LemmaBugException($"BUG: unknown install step tag: {stepJson}");
                }

                var url = fetch.GetProperty("url").GetString()
                    ?? throw new LemmaBugException("BUG: fetch step missing url");
                try
                {
                    using var request = new HttpRequestMessage(HttpMethod.Get, url);
                    if (fetch.TryGetProperty("headers", out var headers)
                        && headers.ValueKind == JsonValueKind.Array)
                    {
                        foreach (var header in headers.EnumerateArray())
                        {
                            var name = header.GetProperty("name").GetString()
                                ?? throw new LemmaBugException("BUG: header missing name");
                            var value = header.GetProperty("value").GetString()
                                ?? throw new LemmaBugException("BUG: header missing value");
                            request.Headers.TryAddWithoutValidation(name, value);
                        }
                    }

                    using var response = client.Send(request);
                    var body = response.Content.ReadAsStringAsync().GetAwaiter().GetResult();
                    var headersJson = HeadersJson(response.Headers, response.Content.Headers);
                    stepJson = start.Session.Respond((ushort)(int)response.StatusCode, headersJson, body);
                }
                catch (LemmaFfiException.Engine engineError)
                {
                    throw new LemmaException(engineError.message, engineError.errorsJson);
                }
                catch (LemmaFfiException.Bug bug)
                {
                    throw new LemmaBugException(bug.message);
                }
                catch (HttpRequestException ex)
                {
                    var message = ex.Message;
                    stepJson = start.Session.Fail(message);
                }
                catch (TaskCanceledException ex)
                {
                    stepJson = start.Session.Fail(ex.Message);
                }
            }
        }
    }

    private static RepositoryInstallResult ParseFinished(JsonElement finished)
    {
        if (finished.TryGetProperty("ok", out var ok))
        {
            return LemmaJson.Deserialize<RepositoryInstallResult>(ok.GetRawText());
        }

        if (finished.TryGetProperty("err", out var err))
        {
            throw new LemmaException("LemmaBase install failed", err.GetRawText());
        }

        throw new LemmaBugException("BUG: finished step missing ok or err");
    }

    private static string HeadersJson(HttpHeaders responseHeaders, HttpHeaders contentHeaders)
    {
        var list = new List<object>();
        foreach (var header in responseHeaders)
        {
            foreach (var value in header.Value)
            {
                list.Add(new { name = header.Key, value });
            }
        }
        foreach (var header in contentHeaders)
        {
            foreach (var value in header.Value)
            {
                list.Add(new { name = header.Key, value });
            }
        }
        return JsonSerializer.Serialize(list);
    }
}

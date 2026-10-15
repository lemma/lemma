using Lemmabase.Lemma.Engine.Native;

namespace Lemmabase.Lemma.Engine;

/// <summary>
/// Lemma rules engine. Thread-safe via an internal lock. Dispose when finished.
/// </summary>
public sealed class Engine : IDisposable
{
    private readonly object _gate = new();
    private NativeEngine? _native;
    private bool _disposed;

    private Engine(NativeEngine native)
    {
        _native = native;
    }

    ~Engine()
    {
        Dispose(false);
    }

    public static Engine Create()
    {
        NativeLibraryLoader.EnsureLoaded();
        return new Engine(NativeEngine.Create());
    }

    public static Engine Create(ResourceLimits.Builder limits)
    {
        ArgumentNullException.ThrowIfNull(limits);
        NativeLibraryLoader.EnsureLoaded();
        try
        {
            return new Engine(NativeEngine.CreateWithLimits(limits.ToJson()));
        }
        catch (LemmaFfiException.Engine engineError)
        {
            throw new LemmaException(engineError.message, engineError.errorsJson);
        }
        catch (LemmaFfiException.Bug bug)
        {
            throw new LemmaBugException(bug.message);
        }
    }

    public static Engine FromSnapshot(byte[] bytes)
    {
        ArgumentNullException.ThrowIfNull(bytes);
        NativeLibraryLoader.EnsureLoaded();
        return Call(() => new Engine(NativeEngine.FromSnapshot(bytes)));
    }

    public void Load(string code)
    {
        ArgumentNullException.ThrowIfNull(code);
        WithNative(native => native.Load(code));
    }

    public void Load(IReadOnlyDictionary<string, string> sources)
    {
        ArgumentNullException.ThrowIfNull(sources);
        var labels = sources.Keys.ToArray();
        var codes = labels.Select(label => sources[label]).ToArray();
        WithNative(native => native.LoadLabeled(labels, codes));
    }

    public void Load(FileInfo file)
    {
        ArgumentNullException.ThrowIfNull(file);
        string code;
        try
        {
            code = File.ReadAllText(file.FullName);
        }
        catch (IOException ex)
        {
            var message = $"failed to read Lemma source from '{file.FullName}': {ex.Message}";
            throw new LemmaException(message, LemmaDotnet.RequestErrorJson(message, null));
        }
        Load(code);
    }

    public RepositoryInstallResult Install(string repository)
    {
        ArgumentNullException.ThrowIfNull(repository);
        lock (_gate)
        {
            var native = RequireNative();
            return LemmaBase.InstallWithEngine(native, repository, LemmaBaseHttp.Client);
        }
    }

    public RepositoryInstallResult Install(string repository, HttpClient client)
    {
        ArgumentNullException.ThrowIfNull(repository);
        ArgumentNullException.ThrowIfNull(client);
        lock (_gate)
        {
            var native = RequireNative();
            return LemmaBase.InstallWithEngine(native, repository, client);
        }
    }

    public IReadOnlyList<ResolvedRepository> List()
    {
        var json = WithNative(native => native.List());
        return LemmaJson.Deserialize<List<ResolvedRepository>>(json);
    }

    public Show Show(string? repository, string spec, string? effective = null)
    {
        ArgumentNullException.ThrowIfNull(spec);
        var json = WithNative(native => native.Show(repository, spec, effective));
        return LemmaJson.Deserialize<Show>(json);
    }

    public string Source(string? repository, string? spec = null, string? effective = null)
    {
        return WithNative(native => native.Source(repository, spec, effective));
    }

    public Response Run(RunRequest request)
    {
        ArgumentNullException.ThrowIfNull(request);
        var data = request.ToEngineStrings();
        string[]? rules = request.Rules?.ToArray();
        var json = WithNative(native =>
            native.Run(request.Repository, request.Spec, request.Effective, data, rules, request.Explain)
        );
        return LemmaJson.Deserialize<Response>(json);
    }

    public void Remove(string? repository, string spec, string? effective = null)
    {
        ArgumentNullException.ThrowIfNull(spec);
        WithNative(native => native.Remove(repository, spec, effective));
    }

    public void Update(string? repository, string code, string? attribute = null)
    {
        ArgumentNullException.ThrowIfNull(code);
        WithNative(native => native.Update(repository, code, attribute));
    }

    public ResourceLimits Limits()
    {
        var json = WithNative(native => native.Limits());
        return LemmaJson.Deserialize<ResourceLimits>(json);
    }

    public byte[] Snapshot()
    {
        return WithNative(native => native.Snapshot());
    }

    public IReadOnlyList<Recommendation> Quality()
    {
        var json = WithNative(native => native.Quality());
        return LemmaJson.Deserialize<List<Recommendation>>(json);
    }

    /// <summary>Formats Lemma source. Named <c>Format</c> on <see cref="Engine"/> to avoid CS0542.</summary>
    public static string Format(string code)
    {
        ArgumentNullException.ThrowIfNull(code);
        NativeLibraryLoader.EnsureLoaded();
        return Call(() => LemmaDotnet.FormatSource(code));
    }

    public void Dispose()
    {
        Dispose(true);
        GC.SuppressFinalize(this);
    }

    private void Dispose(bool disposing)
    {
        lock (_gate)
        {
            if (_disposed)
            {
                return;
            }
            _disposed = true;
            _native?.Dispose();
            _native = null;
        }
    }

    private void WithNative(Action<NativeEngine> action)
    {
        lock (_gate)
        {
            var native = RequireNative();
            Call(() =>
            {
                action(native);
                return 0;
            });
        }
    }

    private T WithNative<T>(Func<NativeEngine, T> func)
    {
        lock (_gate)
        {
            var native = RequireNative();
            return Call(() => func(native));
        }
    }

    private NativeEngine RequireNative()
    {
        if (_disposed || _native is null)
        {
            throw new LemmaBugException("BUG: Engine used after Dispose");
        }
        return _native;
    }

    private static T Call<T>(Func<T> func)
    {
        try
        {
            return func();
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
        catch (ObjectDisposedException ex)
        {
            throw new LemmaBugException("BUG: Engine used after Dispose", ex);
        }
    }
}

internal static class LemmaBaseHttp
{
    internal static readonly HttpClient Client = Create();

    private static HttpClient Create()
    {
        var client = new HttpClient();
        client.Timeout = TimeSpan.FromSeconds(30);
        return client;
    }
}

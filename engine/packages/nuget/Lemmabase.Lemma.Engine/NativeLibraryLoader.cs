using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Text;
using Lemmabase.Lemma.Engine.Native;

namespace Lemmabase.Lemma.Engine;

internal static class NativeLibraryLoader
{
    private static readonly object Gate = new();
    private static bool _resolverInstalled;
    private static string? _resolvedPath;

    internal static void EnsureLoaded()
    {
        lock (Gate)
        {
            if (_resolverInstalled)
            {
                return;
            }

            RefuseMuslOnLinux();
            _resolvedPath = ResolveLibraryPath();
            NativeLibrary.SetDllImportResolver(
                typeof(NativeEngine).Assembly,
                (libraryName, _assembly, _searchPath) =>
                {
                    if (libraryName is not "lemma_dotnet")
                    {
                        return IntPtr.Zero;
                    }

                    var path = _resolvedPath
                        ?? throw new LemmaNativeException("BUG: lemma_dotnet path unresolved after resolver install");
                    return NativeLibrary.Load(path);
                }
            );
            _resolverInstalled = true;
        }
    }

    internal static string ResolveLibraryPathForTests()
    {
        RefuseMuslOnLinux();
        return ResolveLibraryPath();
    }

    private static void RefuseMuslOnLinux()
    {
        if (!OperatingSystem.IsLinux())
        {
            return;
        }

        // Prefer process linkage over filesystem presence: some glibc hosts also
        // ship an unused musl loader under /lib.
        if (File.Exists("/etc/alpine-release") || ProcessLinksMusl())
        {
            throw new LemmaNativeException(
                "Lemmabase.Lemma.Engine does not support musl/Alpine Linux; use a glibc-based distribution"
            );
        }
    }

    private static bool ProcessLinksMusl()
    {
        try
        {
            foreach (var line in File.ReadLines("/proc/self/maps"))
            {
                if (line.Contains("ld-musl-", StringComparison.Ordinal)
                    || line.Contains("libc.musl-", StringComparison.Ordinal))
                {
                    return true;
                }
            }
        }
        catch (IOException)
        {
            // /proc may be unavailable; fall through to no musl signal.
        }
        catch (UnauthorizedAccessException)
        {
        }

        return false;
    }

    private static string ResolveLibraryPath()
    {
        var envOverride = Environment.GetEnvironmentVariable("LEMMA_DOTNET_LIBRARY");
        if (!string.IsNullOrWhiteSpace(envOverride))
        {
            if (!File.Exists(envOverride))
            {
                throw new LemmaNativeException(
                    $"LEMMA_DOTNET_LIBRARY set to '{envOverride}' but it is not a regular file"
                );
            }
            return Path.GetFullPath(envOverride);
        }

        var triple = RustTargetTriple();
        var libName = LibraryFileName();
        var resourceSuffix = $".natives.{triple.Replace('-', '_')}.{libName}";

        var resourceName = typeof(NativeLibraryLoader).Assembly
            .GetManifestResourceNames()
            .FirstOrDefault(name =>
                name.EndsWith(resourceSuffix, StringComparison.Ordinal)
                || name.Equals($"natives/{triple}/{libName}", StringComparison.Ordinal)
            );
        if (resourceName is not null)
        {
            using var resource = typeof(NativeLibraryLoader).Assembly.GetManifestResourceStream(resourceName)
                ?? throw new LemmaNativeException($"BUG: resource '{resourceName}' disappeared");
            return ExtractEmbedded(resource, triple, libName);
        }

        var dev = DiscoverDevLibrary(libName);
        if (dev is not null)
        {
            return dev;
        }

        throw new LemmaNativeException(
            "native library not found. Checked: LEMMA_DOTNET_LIBRARY env, embedded natives resource, cargo target directories"
        );
    }

    private static string ExtractEmbedded(Stream resource, string triple, string libName)
    {
        using var memory = new MemoryStream();
        resource.CopyTo(memory);
        var bytes = memory.ToArray();
        var hash = Convert.ToHexString(SHA256.HashData(bytes)).ToLowerInvariant();
        var version = ReadEngineVersion();
        var cacheRoot = CacheRoot();
        var cacheDir = Path.Combine(cacheRoot, "lemma-dotnet", $"{version}-{triple}", hash);
        Directory.CreateDirectory(cacheDir);
        var cachedLib = Path.Combine(cacheDir, libName);
        if (File.Exists(cachedLib))
        {
            return cachedLib;
        }

        var temp = Path.Combine(cacheDir, $".{libName}.{Guid.NewGuid():N}.tmp");
        File.WriteAllBytes(temp, bytes);
        try
        {
            File.Move(temp, cachedLib);
        }
        catch (IOException) when (File.Exists(cachedLib))
        {
            File.Delete(temp);
        }

        return cachedLib;
    }

    private static string ReadEngineVersion()
    {
        using var stream = typeof(NativeLibraryLoader).Assembly.GetManifestResourceStream(
            "Lemmabase.Lemma.Engine.engine.version"
        );
        if (stream is null)
        {
            throw new LemmaNativeException("engine.version resource missing; cannot determine version for cache key");
        }

        using var reader = new StreamReader(stream, Encoding.UTF8);
        var version = reader.ReadToEnd().Trim();
        if (string.IsNullOrWhiteSpace(version))
        {
            throw new LemmaNativeException("engine.version blank; cannot determine version for cache key");
        }
        return version;
    }

    private static string CacheRoot()
    {
        var overrideDir = Environment.GetEnvironmentVariable("LEMMA_DOTNET_CACHE_DIR");
        if (!string.IsNullOrWhiteSpace(overrideDir))
        {
            return overrideDir;
        }

        var home = Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);
        if (string.IsNullOrWhiteSpace(home))
        {
            throw new LemmaNativeException(
                "user home is not set; override with LEMMA_DOTNET_CACHE_DIR"
            );
        }
        return Path.Combine(home, ".cache");
    }

    private static string? DiscoverDevLibrary(string libName)
    {
        var cargoTarget = Environment.GetEnvironmentVariable("CARGO_TARGET_DIR");
        var candidates = new List<string>();
        if (!string.IsNullOrWhiteSpace(cargoTarget))
        {
            candidates.Add(Path.Combine(cargoTarget, "debug", libName));
            candidates.Add(Path.Combine(cargoTarget, "release", libName));
        }

        var cwd = Directory.GetCurrentDirectory();
        candidates.Add(Path.Combine(cwd, "target", "debug", libName));
        candidates.Add(Path.Combine(cwd, "target", "release", libName));
        candidates.Add(Path.Combine(cwd, "..", "..", "..", "..", "target", "debug", libName));
        candidates.Add(Path.Combine(cwd, "..", "..", "..", "..", "target", "release", libName));

        // Walk up from the assembly location looking for workspace target/.
        var dir = Path.GetDirectoryName(typeof(NativeLibraryLoader).Assembly.Location);
        for (var i = 0; i < 8 && dir is not null; i++)
        {
            candidates.Add(Path.Combine(dir, "target", "debug", libName));
            candidates.Add(Path.Combine(dir, "target", "release", libName));
            dir = Path.GetDirectoryName(dir);
        }

        foreach (var candidate in candidates)
        {
            var full = Path.GetFullPath(candidate);
            if (File.Exists(full))
            {
                return full;
            }
        }

        return null;
    }

    private static string LibraryFileName()
    {
        if (OperatingSystem.IsMacOS())
        {
            return "liblemma_dotnet.dylib";
        }
        if (OperatingSystem.IsWindows())
        {
            return "lemma_dotnet.dll";
        }
        return "liblemma_dotnet.so";
    }

    private static string RustTargetTriple()
    {
        var arch = RuntimeInformation.ProcessArchitecture switch
        {
            Architecture.X64 => "x86_64",
            Architecture.Arm64 => "aarch64",
            _ => throw new LemmaNativeException(
                $"unsupported CPU architecture for lemma_dotnet: {RuntimeInformation.ProcessArchitecture}"
            ),
        };

        if (OperatingSystem.IsMacOS())
        {
            return $"{arch}-apple-darwin";
        }
        if (OperatingSystem.IsWindows())
        {
            return $"{arch}-pc-windows-msvc";
        }
        if (OperatingSystem.IsLinux())
        {
            return $"{arch}-unknown-linux-gnu";
        }

        throw new LemmaNativeException($"unsupported OS for lemma_dotnet: {RuntimeInformation.OSDescription}");
    }
}

using System.Reflection;
using Xunit;

namespace Lemmabase.Lemma.Engine.Tests;

public sealed class NativeLibraryCacheTests
{
    [Fact]
    public void ResolveLibraryPathFindsDevOrEnvLibrary()
    {
        var path = NativeLibraryLoader.ResolveLibraryPathForTests();
        Assert.True(File.Exists(path), path);
        Assert.Contains("lemma_dotnet", path);
    }

    [Fact]
    public void MuslRefuseSucceedsOnGlibcLinux()
    {
        if (!OperatingSystem.IsLinux())
        {
            return;
        }

        // Invoke private refuse check via the public resolve path; glibc hosts must not throw.
        _ = NativeLibraryLoader.ResolveLibraryPathForTests();
    }
}

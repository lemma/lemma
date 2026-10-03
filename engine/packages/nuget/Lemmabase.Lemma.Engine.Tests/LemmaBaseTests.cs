using System.Net;
using System.Text;
using Xunit;

namespace Lemmabase.Lemma.Engine.Tests;

public sealed class LemmaBaseTests
{
    private static readonly string FixturesDir = Path.GetFullPath(
        Path.Combine(AppContext.BaseDirectory, "..", "..", "..", "..", "..", "..", "tests", "registry_fixtures")
    );

    [Fact]
    public void InstallSucceeds()
    {
        var body = File.ReadAllText(Path.Combine(FixturesDir, "@iso", "countries.lemma"));
        using var client = new FixtureHttpClient("https://lemmabase.com/@iso/countries.lemma", HttpStatusCode.OK, body);
        using var engine = Engine.Create();
        var result = engine.Install("@iso/countries", client);
        Assert.Equal("@iso/countries", result.Id);
        Assert.Contains("spec alpha2", result.Source);
    }

    [Fact]
    public void InstallNotFoundThrowsLemmaException()
    {
        using var client = new FixtureHttpClient(
            "https://lemmabase.com/@iso/nonexistent.lemma",
            HttpStatusCode.NotFound,
            "not found"
        );
        using var engine = Engine.Create();
        var thrown = Assert.Throws<LemmaException>(() => engine.Install("@iso/nonexistent", client));
        Assert.NotEmpty(thrown.Errors);
        Assert.Contains(thrown.Errors, e => e.RegistryKind == "not_found");
    }

    [Fact]
    public void InstallTransportFailThrowsLemmaException()
    {
        using var client = new FailingHttpClient();
        using var engine = Engine.Create();
        var thrown = Assert.Throws<LemmaException>(() => engine.Install("@iso/countries", client));
        Assert.NotEmpty(thrown.Errors);
        Assert.Contains(thrown.Errors, e => e.RegistryKind == "network_error");
    }

    [Fact]
    public void InstallEmptyIdThrowsLemmaException()
    {
        using var client = new FixtureHttpClient("https://lemmabase.com/unused.lemma", HttpStatusCode.OK, "unused");
        using var engine = Engine.Create();
        var thrown = Assert.Throws<LemmaException>(() => engine.Install("   ", client));
        Assert.NotEmpty(thrown.Errors);
    }

    private sealed class FixtureHttpClient : HttpClient
    {
        public FixtureHttpClient(string expectedUrl, HttpStatusCode status, string body)
            : base(new FixtureHandler(expectedUrl, status, body))
        {
        }

        private sealed class FixtureHandler : HttpMessageHandler
        {
            private readonly string _expectedUrl;
            private readonly HttpStatusCode _status;
            private readonly string _body;

            public FixtureHandler(string expectedUrl, HttpStatusCode status, string body)
            {
                _expectedUrl = expectedUrl;
                _status = status;
                _body = body;
            }

            protected override HttpResponseMessage Send(
                HttpRequestMessage request,
                CancellationToken cancellationToken
            )
            {
                Assert.Equal(_expectedUrl, request.RequestUri?.ToString());
                return new HttpResponseMessage(_status)
                {
                    Content = new StringContent(_body, Encoding.UTF8, "text/plain"),
                };
            }

            protected override Task<HttpResponseMessage> SendAsync(
                HttpRequestMessage request,
                CancellationToken cancellationToken
            )
            {
                return Task.FromResult(Send(request, cancellationToken));
            }
        }
    }

    private sealed class FailingHttpClient : HttpClient
    {
        public FailingHttpClient()
            : base(new FailingHandler())
        {
        }

        private sealed class FailingHandler : HttpMessageHandler
        {
            protected override HttpResponseMessage Send(
                HttpRequestMessage request,
                CancellationToken cancellationToken
            )
            {
                throw new HttpRequestException("connection refused");
            }

            protected override Task<HttpResponseMessage> SendAsync(
                HttpRequestMessage request,
                CancellationToken cancellationToken
            )
            {
                throw new HttpRequestException("connection refused");
            }
        }
    }
}

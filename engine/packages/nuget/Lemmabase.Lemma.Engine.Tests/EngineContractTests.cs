using Xunit;

namespace Lemmabase.Lemma.Engine.Tests;

public sealed class EngineContractTests
{
    [Fact]
    public void InvalidLoadThrowsLemmaException()
    {
        using var engine = Engine.Create();
        var thrown = Assert.Throws<LemmaException>(() => engine.Load("this is not lemma"));
        Assert.NotEmpty(thrown.Errors);
    }

    [Fact]
    public void VetoRemainsInRuleResult()
    {
        using var engine = Engine.Create();
        engine.Load(
            """
            spec deny
            rule outcome: veto "not allowed"
            """
        );
        var response = engine.Run(RunRequest.Of("deny"));
        var outcome = Assert.IsType<RuleResult.Veto>(response.Results["outcome"]);
        Assert.Equal("not allowed", outcome.VetoReason);
    }

    [Fact]
    public void RejectsDoubleRunDataValues()
    {
        using var engine = Engine.Create();
        engine.Load(
            """
            spec pricing
            data amount: number
            rule doubled: amount * 2
            """
        );
        var thrown = Assert.Throws<LemmaException>(() =>
            engine.Run(RunRequest.Of("pricing").WithData(new Dictionary<string, object?> { ["amount"] = 1.5 }))
        );
        Assert.Contains("decimal values must be passed as decimal", thrown.Message);
        Assert.NotEmpty(thrown.Errors);
        Assert.Equal("request", thrown.Errors[0].Kind);
    }

    [Fact]
    public void EmptyRulesListFails()
    {
        using var engine = Engine.Create();
        engine.Load(
            """
            spec sample
            rule value: 1
            """
        );
        var thrown = Assert.Throws<LemmaException>(() =>
            engine.Run(RunRequest.Of("sample").WithRules(Array.Empty<string>()))
        );
        Assert.Contains(thrown.Errors, e => e.Message.Contains("rules must not be empty"));
    }

    [Fact]
    public void SnapshotRoundTripRestoresListAndRun()
    {
        using var source = Engine.Create();
        source.Load(
            """
            spec snap_demo
            data x: number
            rule y: x + 1
            """
        );
        var bytes = source.Snapshot();
        Assert.NotEmpty(bytes);
        using var restored = Engine.FromSnapshot(bytes);
        Assert.Equal(
            source.List().Select(r => r.Repository).ToArray(),
            restored.List().Select(r => r.Repository).ToArray()
        );
        var runSource = source.Run(
            RunRequest.Of("snap_demo").WithData(new Dictionary<string, object?> { ["x"] = "41" })
        );
        var runRestored = restored.Run(
            RunRequest.Of("snap_demo").WithData(new Dictionary<string, object?> { ["x"] = "41" })
        );
        var ySource = Assert.IsType<RuleResult.Number>(runSource.Results["y"]);
        var yRestored = Assert.IsType<RuleResult.Number>(runRestored.Results["y"]);
        Assert.Equal(ySource.NumberValue, yRestored.NumberValue);

        bytes[0] = 0x58;
        Assert.Throws<LemmaException>(() => Engine.FromSnapshot(bytes));
    }

    [Fact]
    public void LimitsBuilderAppliesSparseOverrides()
    {
        using var engine = Engine.Create(ResourceLimits.CreateBuilder().MaxSources(12));
        Assert.Equal(12, engine.Limits().MaxSources);
    }

    [Fact]
    public void FormatReturnsNormalizedSource()
    {
        var formatted = Engine.Format(
            """
            spec demo
            rule x:1
            """
        );
        Assert.Contains("spec demo", formatted);
        Assert.Contains("rule x", formatted);
    }

    [Fact]
    public void QualityReportsMissingHelp()
    {
        using var engine = Engine.Create();
        engine.Load(
            """"
            spec pricing 2026-01-01
            """
            Bulk pricing.
            """

            data qty: number
            rule total: qty
            """"
        );
        var recs = engine.Quality();
        Assert.NotEmpty(recs);
        var hit = Assert.Single(recs, r => r.Message.Contains("no `-> help`"));
        Assert.Equal("pricing", hit.Spec);
        Assert.Equal("2026-01-01", hit.EffectiveFrom);
        Assert.True(hit.Source.Line > 0);
    }

    [Fact]
    public void UseAfterDisposeThrowsLemmaBugException()
    {
        var engine = Engine.Create();
        engine.Dispose();
        Assert.Throws<LemmaBugException>(() => engine.Load("spec x\nrule y: 1"));
    }

    [Fact]
    public void InstallEmptyRepositoryThrowsLemmaException()
    {
        using var engine = Engine.Create();
        var thrown = Assert.Throws<LemmaException>(() => engine.Install("   "));
        Assert.NotEmpty(thrown.Errors);
        Assert.Contains(thrown.Errors, err => err.Kind == "registry");
    }

    [Fact]
    public void UpdateReplacesSpecSlice()
    {
        using var engine = Engine.Create();
        engine.Load(
            """
            spec pricing
            data quantity: 1
            rule total: quantity * 10
            """
        );
        engine.Update(
            null,
            """
            spec pricing
            data quantity: 1
            rule total: quantity * 20
            """
        );
        var response = engine.Run(RunRequest.Of("pricing"));
        var total = Assert.IsType<RuleResult.Number>(response.Results["total"]);
        Assert.Equal(20m, total.NumberValue);
    }
}

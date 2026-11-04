using System.Runtime.CompilerServices;
using System.Text.Json;
using Xunit;

namespace Lemmabase.Lemma.Engine.Tests;

public sealed class ShowTests
{
    private static string Fixture(string name, [CallerFilePath] string testFile = "")
    {
        var testDir =
            Path.GetDirectoryName(testFile)
            ?? throw new InvalidOperationException($"BUG: test file path has no directory: {testFile}");
        var path = Path.GetFullPath(Path.Combine(testDir, "..", "..", "..", "tests", "fixtures", "api", name));
        return File.ReadAllText(path);
    }

    [Fact]
    public void ShowMinimalFixtureParses()
    {
        var json = Fixture("show_minimal.json");
        var show = Show.Parse(json);
        Assert.Equal("sample", show.Spec);
        Assert.Null(show.Repository);
        Assert.Equal("2024-01-01", show.EffectiveFrom);
        Assert.Equal(2, show.StartLine);
        Assert.Equal(["amount"], show.Required);
        Assert.Empty(show.Rows);
        Assert.Empty(show.RowFieldNeededByRules);
        Assert.Equal(json, show.Json);

        var amount = show.Data["amount"];
        Assert.Equal("string", amount.Type);
        Assert.Equal("A dimensionless number.", amount.Description);
        Assert.Empty(amount.Path);
        Assert.Equal(["ok"], amount.NeededByRules);
        Assert.Equal("number", amount.LemmaType.GetProperty("kind").GetString());
        Assert.Equal("1", amount.Suggestion!.Value.GetProperty("number").GetString());

        Assert.Equal(["amount", "money"], show.Types.Keys.Order());
        Assert.Equal("t", show.Meta["title"].GetProperty("text").GetString());

        var ok = show.Rules["ok"];
        Assert.Empty(ok.Path);
        Assert.Empty(ok.DependsOnRules);
        var branch = Assert.Single(ok.Branches);
        Assert.Null(branch.Condition);
        Assert.Equal("data", branch.Result.GetProperty("type").GetString());
    }

    [Fact]
    public void ShowListFixtureParsesRowsAndRowFieldConsumers()
    {
        var show = Show.Parse(Fixture("show_list.json"));
        Assert.Equal("coffee_order", show.Spec);

        var menu = show.Data["menu"];
        Assert.Equal("array", menu.Type);
        Assert.Equal(1024, menu.MaxItems);
        Assert.Equal("#/$defs/LemmaRow_menu__item", menu.Items!.Value.GetProperty("$ref").GetString());
        Assert.Contains("total", menu.NeededByRules);
        Assert.Equal(JsonValueKind.Array, menu.Default!.Value.ValueKind);

        var row = show.Rows["LemmaRow_menu__item"];
        Assert.Equal("menu_item row", row.Title);
        Assert.Equal("string", row.Data["drink"].Type);
        Assert.Empty(row.Data["drink"].NeededByRules);
        Assert.Null(row.Data["price"].Type);
        Assert.Equal(JsonValueKind.Array, row.Data["price"].OneOf!.Value.ValueKind);

        var consumers = show.RowFieldNeededByRules["LemmaRow_menu__item"];
        Assert.Equal(["extended", "selected", "selected_price"], consumers["drink"]);
        Assert.Equal(["any_large", "large", "size_product"], consumers["size"]);
    }

    [Fact]
    public void EngineShowReadsRowFieldConsumers()
    {
        using var engine = Engine.Create();
        engine.Load(
            """
            spec line
            data qty: number
            data label: text

            spec order
            uses line
            data lines: line list
            rule total: lines -> sum qty
            """
        );
        var show = engine.Show(null, "order");
        Assert.Equal("order", show.Spec);
        var rowName = Assert.Single(show.Rows.Keys);
        Assert.Equal(["total"], show.RowFieldNeededByRules[rowName]["qty"]);
        Assert.False(show.RowFieldNeededByRules[rowName].ContainsKey("label"));
        Assert.Empty(show.Rules["total"].DependsOnRules);
    }

    [Fact]
    public void UnknownFieldOnShowIsABug()
    {
        var json = """
            {
              "$schema": "https://json-schema.org/draft/2020-12/schema",
              "title": "sample",
              "type": "object",
              "additionalProperties": true,
              "properties": {},
              "x-lemma-start-line": 1,
              "x-lemma-meta": {},
              "x-lemma-types": {},
              "x-lemma-rules": {},
              "not_a_real_field": true
            }
            """;
        var thrown = Assert.Throws<LemmaBugException>(() => Show.Parse(json));
        Assert.Contains("not_a_real_field", thrown.Message);
    }

    [Fact]
    public void ShowWithAnotherDialectIsABug()
    {
        var json = """
            {
              "$schema": "http://json-schema.org/draft-07/schema#",
              "title": "sample",
              "type": "object",
              "additionalProperties": true,
              "properties": {},
              "x-lemma-start-line": 1,
              "x-lemma-meta": {},
              "x-lemma-types": {},
              "x-lemma-rules": {}
            }
            """;
        var thrown = Assert.Throws<LemmaBugException>(() => Show.Parse(json));
        Assert.Contains("Show.$schema", thrown.Message);
    }

    [Fact]
    public void ShowPropertyWithoutTypeOrOneOfIsABug()
    {
        var json = """
            {
              "$schema": "https://json-schema.org/draft/2020-12/schema",
              "title": "sample",
              "type": "object",
              "additionalProperties": true,
              "properties": {
                "amount": {
                  "x-lemma-type": {},
                  "x-lemma-path": [],
                  "x-lemma-needed-by-rules": []
                }
              },
              "x-lemma-start-line": 1,
              "x-lemma-meta": {},
              "x-lemma-types": {},
              "x-lemma-rules": {}
            }
            """;
        var thrown = Assert.Throws<LemmaBugException>(() => Show.Parse(json));
        Assert.Contains("type or oneOf", thrown.Message);
    }
}

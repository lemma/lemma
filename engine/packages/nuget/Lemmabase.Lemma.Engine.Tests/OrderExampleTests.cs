using Xunit;

namespace Lemmabase.Lemma.Engine.Tests;

public sealed class OrderExampleTests
{
    [Fact]
    public void OrderWithDecimalData()
    {
        using var engine = Engine.Create();
        engine.Load(
            """
            spec order
            data quantity: number
            data unit_price: number
            rule total: quantity * unit_price
            """
        );

        Response response = engine.Run(
            RunRequest.Of("order").WithData(
                new Dictionary<string, object?>
                {
                    ["quantity"] = 3,
                    ["unit_price"] = 19.99m,
                }
            )
        );

        decimal total = ((RuleResult.Number)response.Results["total"]).NumberValue;
        Assert.Equal(59.97m, total);
    }
}

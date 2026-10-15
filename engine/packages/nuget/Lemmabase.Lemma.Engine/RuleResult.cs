using System.Text.Json;

namespace Lemmabase.Lemma.Engine;

/// <summary>
/// One rule evaluation result. Nested explanation trees stay as <see cref="JsonElement"/>.
/// </summary>
public abstract class RuleResult
{
    private protected RuleResult(string ruleType, JsonElement? explanation)
    {
        RuleType = ruleType;
        Explanation = explanation;
    }

    public string RuleType { get; }
    public JsonElement? Explanation { get; }

    public sealed class Veto : RuleResult
    {
        public Veto(string? vetoReason, string ruleType, JsonElement? explanation)
            : base(ruleType, explanation)
        {
            VetoReason = vetoReason;
        }

        public string? VetoReason { get; }
    }

    public sealed class MissingData : RuleResult
    {
        public MissingData(IReadOnlyList<string> missing, string ruleType, JsonElement? explanation)
            : base(ruleType, explanation)
        {
            Missing = missing;
        }

        public IReadOnlyList<string> Missing { get; }
    }

    public sealed class Number : RuleResult
    {
        public Number(string result, decimal numberValue, string ruleType, JsonElement? explanation)
            : base(ruleType, explanation)
        {
            Result = result;
            NumberValue = numberValue;
        }

        public string Result { get; }
        public decimal NumberValue { get; }
    }

    public sealed class Text : RuleResult
    {
        public Text(string result, string textValue, string ruleType, JsonElement? explanation)
            : base(ruleType, explanation)
        {
            Result = result;
            TextValue = textValue;
        }

        public string Result { get; }
        public string TextValue { get; }
    }

    public sealed class BooleanValue : RuleResult
    {
        public BooleanValue(string result, bool value, string ruleType, JsonElement? explanation)
            : base(ruleType, explanation)
        {
            Result = result;
            Value = value;
        }

        public string Result { get; }
        public bool Value { get; }
    }

    public sealed class Date : RuleResult
    {
        public Date(string result, string dateValue, string ruleType, JsonElement? explanation)
            : base(ruleType, explanation)
        {
            Result = result;
            DateValue = dateValue;
        }

        public string Result { get; }
        public string DateValue { get; }
    }

    public sealed class Time : RuleResult
    {
        public Time(string result, string timeValue, string ruleType, JsonElement? explanation)
            : base(ruleType, explanation)
        {
            Result = result;
            TimeValue = timeValue;
        }

        public string Result { get; }
        public string TimeValue { get; }
    }

    public sealed class Measure : RuleResult
    {
        public Measure(
            string result,
            IReadOnlyDictionary<string, decimal> units,
            string ruleType,
            JsonElement? explanation
        )
            : base(ruleType, explanation)
        {
            Result = result;
            Units = units;
        }

        public string Result { get; }
        public IReadOnlyDictionary<string, decimal> Units { get; }
    }

    public sealed class Ratio : RuleResult
    {
        public Ratio(
            string result,
            IReadOnlyDictionary<string, decimal> units,
            string ruleType,
            JsonElement? explanation
        )
            : base(ruleType, explanation)
        {
            Result = result;
            Units = units;
        }

        public string Result { get; }
        public IReadOnlyDictionary<string, decimal> Units { get; }
    }

    public sealed class Range : RuleResult
    {
        public Range(string result, JsonElement from, JsonElement to, string ruleType, JsonElement? explanation)
            : base(ruleType, explanation)
        {
            Result = result;
            From = from;
            To = to;
        }

        public string Result { get; }
        public JsonElement From { get; }
        public JsonElement To { get; }
    }

    public sealed class Calendar : RuleResult
    {
        public Calendar(string result, decimal value, string unit, string ruleType, JsonElement? explanation)
            : base(ruleType, explanation)
        {
            Result = result;
            Value = value;
            Unit = unit;
        }

        public string Result { get; }
        public decimal Value { get; }
        public string Unit { get; }
    }
}

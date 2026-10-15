namespace Lemmabase.Lemma.Engine;

/// <summary>Planning or request failure carrying structured <see cref="EngineError"/> entries.</summary>
public sealed class LemmaException : Exception
{
    public LemmaException(string message, IReadOnlyList<EngineError> errors)
        : base(message)
    {
        Errors = errors;
    }

    public LemmaException(string message, string errorsJson)
        : this(message, LemmaJson.DeserializeEngineErrors(errorsJson))
    {
    }

    public IReadOnlyList<EngineError> Errors { get; }
}

/// <summary>Bug or impossible state across the native boundary.</summary>
public sealed class LemmaBugException : Exception
{
    public LemmaBugException(string message)
        : base(message)
    {
    }

    public LemmaBugException(string message, Exception inner)
        : base(message, inner)
    {
    }
}

/// <summary>Native library resolution or load failure.</summary>
public sealed class LemmaNativeException : Exception
{
    public LemmaNativeException(string message)
        : base(message)
    {
    }

    public LemmaNativeException(string message, Exception inner)
        : base(message, inner)
    {
    }
}

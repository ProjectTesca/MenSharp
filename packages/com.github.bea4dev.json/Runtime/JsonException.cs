// JSON for MenSharp: what `Json.Parse<T>` and `Json.Stringify<T>` throw.

using System;

namespace Bea4dev.Json
{
    /// The text was not JSON, or its shape did not fit the type: the message
    /// names the path (`$.players[2].score`) and what was found there.
    public class JsonException : Exception
    {
        public JsonException(string message) : base(message)
        {
        }
    }
}

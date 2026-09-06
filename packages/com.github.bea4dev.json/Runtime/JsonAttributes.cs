// JSON for MenSharp: the attributes that shape how a field maps to JSON.
//
// `Json.Parse<T>` and `Json.Stringify<T>` walk the public fields and
// auto-properties of `T` with MenSharp's static reflection; these attributes
// are what that walk reads off each member.

using System;

namespace Bea4dev.Json
{
    /// The JSON key a field or property is read from and written to, when
    /// it is not the member's own name: `[JsonName("player_name")]`.
    [AttributeUsage(AttributeTargets.Field | AttributeTargets.Property)]
    public sealed class JsonNameAttribute : Attribute
    {
        public readonly string Name;

        public JsonNameAttribute(string name)
        {
            Name = name;
        }
    }

    /// Leaves a public field or property out of JSON entirely.
    [AttributeUsage(AttributeTargets.Field | AttributeTargets.Property)]
    public sealed class JsonIgnoreAttribute : Attribute
    {
    }

    /// Includes a non-public field or property in JSON, which is otherwise
    /// skipped; `[JsonName]` on it does the same.
    [AttributeUsage(AttributeTargets.Field | AttributeTargets.Property)]
    public sealed class JsonIncludeAttribute : Attribute
    {
    }

    /// A key that must be present: `Json.Parse<T>` fails when the JSON has
    /// no value for it, where an ordinary member keeps its default.
    [AttributeUsage(AttributeTargets.Field | AttributeTargets.Property)]
    public sealed class JsonRequiredAttribute : Attribute
    {
    }
}

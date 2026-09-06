// JSON for MenSharp: typed (de)serialization on top of the SDK's VRCJson.
//
//     public class Config { public string title; public int[] scores; public Player[] players; }
//
//     Config config = Json.Parse<Config>(text);
//     string text = Json.Stringify(config);
//
// The SDK parses JSON natively into `DataToken` trees (`VRCJson`); what it
// does not do is bind a tree to a class. That binding is generic code here,
// specialised per type by the MenSharp compiler: `JsonReader.Read<T>` asks
// `MenSharp.Reflection` what `T` is — a number, a string, an array, a
// `List<E>`, an `E?`, an enum, or a class/struct/record of the project's
// own — and for the last case walks its fields, each with its own static
// type, recursing. Nothing is looked up by name at run time, and a field
// of a type this package cannot map is a compile error naming it.
//
// Supported: bool, int, long, float, double, string, enums (as numbers),
// `T?` of those, one-dimensional arrays, `List<T>`, and classes, structs
// and records with a parameterless constructor whose public fields and
// auto-properties (and any member marked `[JsonName]`/`[JsonInclude]`)
// are themselves supported. JSON `null` reads as `null`/`default`; a key
// missing from an object leaves the member at its default unless it is
// `[JsonRequired]`; keys the type has no member for are ignored.

using VRC.SDK3.Data;

namespace Bea4dev.Json
{
    public static class Json
    {
        /// `text` as a `T`, or a `JsonException` naming what did not fit.
        public static T Parse<T>(string text)
        {
            T value;
            string error;
            if (!TryParse(text, out value, out error))
            {
                throw new JsonException(error);
            }
            return value;
        }

        /// `text` as a `T`; `false`, with `error` naming what did not fit,
        /// when it could not be read.
        public static bool TryParse<T>(string text, out T value, out string error)
        {
            DataToken root;
            if (!VRCJson.TryDeserializeFromJson(text, out root))
            {
                value = default(T);
                error = "not JSON: " + root.ToString();
                return false;
            }
            var reader = new JsonReader();
            value = reader.Read<T>(root, "$");
            error = reader.Error;
            return error == null;
        }

        /// `value` as JSON text, on one line.
        public static string Stringify<T>(T value)
        {
            return Stringify(value, false);
        }

        /// `value` as JSON text, indented when `pretty`.
        public static string Stringify<T>(T value, bool pretty)
        {
            string text;
            string error;
            if (!TryStringify(value, pretty, out text, out error))
            {
                throw new JsonException(error);
            }
            return text;
        }

        /// `value` as JSON text; `false`, with `error`, when something in it
        /// could not be written.
        public static bool TryStringify<T>(T value, bool pretty, out string text, out string error)
        {
            var writer = new JsonWriter();
            DataToken token = writer.Write<T>(value, "$");
            if (writer.Error != null)
            {
                text = null;
                error = writer.Error;
                return false;
            }
            DataToken result;
            if (!VRCJson.TrySerializeToJson(token, pretty ? JsonExportType.Beautify : JsonExportType.Minify, out result))
            {
                text = null;
                error = "could not serialize: " + result.ToString();
                return false;
            }
            text = result.String;
            error = null;
            return true;
        }
    }
}

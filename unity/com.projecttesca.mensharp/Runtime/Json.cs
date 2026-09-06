// MenSharp: typed JSON on top of the SDK's VRCJson — `MenSharp.Json`.
//
//     public class Config { public string title; public int[] scores; public Player[] players; }
//
//     Result<Config, JsonError> parsed = Json.Parse<Config>(text);
//     string text = Json.Stringify(config);
//
// The SDK parses JSON natively into `DataToken` trees (`VRCJson`); what it
// does not do is bind a tree to a class. That binding is generic code here,
// specialised per type by the compiler: `JsonReader.Read<T>` asks
// `MenSharp.Reflection` what `T` is — a number, a string, an array, a
// `List<E>`, an `E?`, an enum, or a class/struct/record of the project's
// own — and for the last case walks its fields, each with its own static
// type, recursing. Nothing is looked up by name at run time, and a field
// of a type this cannot map is a compile error naming it.
//
// Supported: bool, int, long, float, double, string, enums (as numbers),
// `T?` of those, one-dimensional arrays, `List<T>`, and classes, structs
// and records with a parameterless constructor whose public fields and
// auto-properties (and any member marked `[JsonName]`/`[JsonInclude]`)
// are themselves supported. JSON `null` reads as `null`/`default`; a key
// missing from an object leaves the member at its default unless it is
// `[JsonRequired]`; keys the type has no member for are ignored.
//
// Only present when the VRChat SDK is referenced (it needs VRCJson). This
// file is compiled by MenSharp as part of its corlib and, verbatim, by
// Unity as part of the runtime assembly.

using System;
using System.Collections.Generic;
using MenSharp.Reflection;
using VRC.SDK3.Data;

namespace MenSharp.Json
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

    /// What did not fit: where in the JSON (`$.players[2].score`) and what
    /// was found there.
    public sealed class JsonError
    {
        public readonly string Path;
        public readonly string Message;

        public JsonError(string path, string message)
        {
            Path = path;
            Message = message;
        }

        public override string ToString()
        {
            return Path + ": " + Message;
        }
    }

    public static class Json
    {
        /// `text` as a `T`, or the first thing that did not fit.
        public static Result<T, JsonError> Parse<T>(string text)
        {
            DataToken root;
            if (!VRCJson.TryDeserializeFromJson(text, out root))
            {
                return new Result<T, JsonError>.Err(new JsonError("$", "not JSON: " + root.ToString()));
            }
            var reader = new JsonReader();
            T value = reader.Read<T>(root, "$");
            if (reader.Error != null)
            {
                return new Result<T, JsonError>.Err(reader.Error);
            }
            return new Result<T, JsonError>.Ok(value);
        }

        /// `text` as a `T`; `false`, with `error` naming what did not fit,
        /// when it could not be read.
        public static bool TryParse<T>(string text, out T value, out string error)
        {
            Result<T, JsonError> parsed = Parse<T>(text);
            if (parsed is Result<T, JsonError>.Ok ok)
            {
                value = ok.Value;
                error = null;
                return true;
            }
            value = default(T);
            error = ((Result<T, JsonError>.Err)parsed).Error.ToString();
            return false;
        }

        /// `value` as JSON text, on one line; indented when `pretty`.
        public static string Stringify<T>(T value, bool pretty = false)
        {
            string text;
            string error;
            if (!TryStringify(value, pretty, out text, out error))
            {
                throw new InvalidOperationException("Json.Stringify: " + error);
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
                error = writer.Error.ToString();
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

    // ------------------------------------------------------------- reading

    /// A `DataToken` tree read into a `T`. `Read<T>` is one generic method
    /// that the compiler specialises per `T`: its arms are questions about
    /// `T` answered at compile time, so each specialisation is only the arm
    /// that applies, and the last arm — a `T` no arm covers — is a compile
    /// error naming the type. Errors while reading are collected, not
    /// thrown: the first one wins and names the JSON path.
    public sealed class JsonReader
    {
        /// The first thing that did not fit, or `null`.
        public JsonError Error;

        public T Read<T>(DataToken token, string path)
        {
            if (typeof(T) == typeof(int))
            {
                if (!token.IsNumber) return Mismatch<T>(token, path, "a number");
                return (T)(object)(int)token.Number;
            }
            else if (typeof(T) == typeof(long))
            {
                if (!token.IsNumber) return Mismatch<T>(token, path, "a number");
                return (T)(object)(long)token.Number;
            }
            else if (typeof(T) == typeof(float))
            {
                if (!token.IsNumber) return Mismatch<T>(token, path, "a number");
                return (T)(object)(float)token.Number;
            }
            else if (typeof(T) == typeof(double))
            {
                if (!token.IsNumber) return Mismatch<T>(token, path, "a number");
                return (T)(object)token.Number;
            }
            else if (typeof(T) == typeof(bool))
            {
                if (token.TokenType != TokenType.Boolean) return Mismatch<T>(token, path, "true or false");
                return (T)(object)token.Boolean;
            }
            else if (typeof(T) == typeof(string))
            {
                if (token.TokenType == TokenType.Null) return default(T);
                if (token.TokenType != TokenType.String) return Mismatch<T>(token, path, "a string");
                return (T)(object)token.String;
            }
            else if (Reflect.IsEnum<T>())
            {
                if (!token.IsNumber) return Mismatch<T>(token, path, "a number (enum)");
                return (T)(object)(int)token.Number;
            }
            else if (Reflect.IsNullable<T>())
            {
                if (token.TokenType == TokenType.Null) return default(T);
                var inner = new JsonElementReader(this, token, path);
                Reflect.VisitElementType<T, JsonElementReader>(inner);
                return (T)inner.Result;
            }
            else if (Reflect.IsArray<T>())
            {
                if (token.TokenType == TokenType.Null) return default(T);
                if (token.TokenType != TokenType.DataList) return Mismatch<T>(token, path, "an array");
                var elements = new JsonArrayReader(this, token.DataList, path);
                Reflect.VisitElementType<T, JsonArrayReader>(elements);
                return (T)elements.Result;
            }
            else if (Reflect.IsList<T>())
            {
                if (token.TokenType == TokenType.Null) return default(T);
                if (token.TokenType != TokenType.DataList) return Mismatch<T>(token, path, "an array");
                var elements = new JsonListReader(this, token.DataList, path);
                Reflect.VisitElementType<T, JsonListReader>(elements);
                return (T)elements.Result;
            }
            else if (Reflect.IsObject<T>())
            {
                if (token.TokenType == TokenType.Null) return default(T);
                if (token.TokenType != TokenType.DataDictionary) return Mismatch<T>(token, path, "an object");
                T value = Reflect.New<T>();
                Reflect.VisitFields(ref value, new JsonFieldReader(this, token.DataDictionary, path));
                return value;
            }
            else
            {
                Reflect.Unsupported<T>("Json.Parse");
                return default(T);
            }
        }

        public void Fail(string path, string message)
        {
            if (Error == null)
            {
                Error = new JsonError(path, message);
            }
        }

        private T Mismatch<T>(DataToken token, string path, string wanted)
        {
            Fail(path, "expected " + wanted + ", found " + Describe(token));
            return default(T);
        }

        private static string Describe(DataToken token)
        {
            TokenType kind = token.TokenType;
            if (kind == TokenType.Null) return "null";
            if (kind == TokenType.Boolean) return token.Boolean ? "true" : "false";
            if (kind == TokenType.String) return "\"" + token.String + "\"";
            if (kind == TokenType.DataList) return "an array";
            if (kind == TokenType.DataDictionary) return "an object";
            if (token.IsNumber) return token.Number.ToString();
            return "an unsupported value";
        }
    }

    /// `E?`: reads the `E` and hands it back boxed, which is what an `E?`
    /// is at run time.
    public sealed class JsonElementReader : ITypeVisitor
    {
        private readonly JsonReader reader;
        private readonly DataToken token;
        private readonly string path;
        public object Result;

        public JsonElementReader(JsonReader reader, DataToken token, string path)
        {
            this.reader = reader;
            this.token = token;
            this.path = path;
        }

        public void Visit<E>()
        {
            Result = reader.Read<E>(token, path);
        }
    }

    /// `E[]` from a `DataList`.
    public sealed class JsonArrayReader : ITypeVisitor
    {
        private readonly JsonReader reader;
        private readonly DataList list;
        private readonly string path;
        public object Result;

        public JsonArrayReader(JsonReader reader, DataList list, string path)
        {
            this.reader = reader;
            this.list = list;
            this.path = path;
        }

        public void Visit<E>()
        {
            int count = list.Count;
            E[] array = new E[count];
            for (int i = 0; i < count; i++)
            {
                DataToken item;
                list.TryGetValue(i, out item);
                array[i] = reader.Read<E>(item, path + "[" + i + "]");
            }
            Result = array;
        }
    }

    /// `List<E>` from a `DataList`.
    public sealed class JsonListReader : ITypeVisitor
    {
        private readonly JsonReader reader;
        private readonly DataList list;
        private readonly string path;
        public object Result;

        public JsonListReader(JsonReader reader, DataList list, string path)
        {
            this.reader = reader;
            this.list = list;
            this.path = path;
        }

        public void Visit<E>()
        {
            int count = list.Count;
            var result = new List<E>(count);
            for (int i = 0; i < count; i++)
            {
                DataToken item;
                list.TryGetValue(i, out item);
                result.Add(reader.Read<E>(item, path + "[" + i + "]"));
            }
            Result = result;
        }
    }

    /// One field of an object: looks its key up in the `DataDictionary` and
    /// reads the value with the field's own type.
    public sealed class JsonFieldReader : IFieldVisitor
    {
        private readonly JsonReader reader;
        private readonly DataDictionary dictionary;
        private readonly string path;

        public JsonFieldReader(JsonReader reader, DataDictionary dictionary, string path)
        {
            this.reader = reader;
            this.dictionary = dictionary;
            this.path = path;
        }

        public void Visit<F>(FieldInfo field, ref F value)
        {
            string key = JsonFields.KeyOf(field);
            if (key == null)
            {
                return;
            }
            DataToken token;
            if (dictionary.TryGetValue(key, out token))
            {
                value = reader.Read<F>(token, path + "." + key);
            }
            else if (field.Has<JsonRequiredAttribute>())
            {
                reader.Fail(path, "missing required key \"" + key + "\"");
            }
        }
    }

    public static class JsonFields
    {
        /// The JSON key of a member, or `null` when it takes no part: a
        /// non-public member without `[JsonName]`/`[JsonInclude]`, or one
        /// marked `[JsonIgnore]`.
        public static string KeyOf(FieldInfo field)
        {
            if (field.Has<JsonIgnoreAttribute>())
            {
                return null;
            }
            var alias = field.Attribute<JsonNameAttribute>();
            if (alias != null)
            {
                return alias.Name;
            }
            if (field.IsPublic || field.Has<JsonIncludeAttribute>())
            {
                return field.Name;
            }
            return null;
        }
    }

    // ------------------------------------------------------------- writing

    /// A `T` written out as a `DataToken` tree, the mirror of `JsonReader`.
    public sealed class JsonWriter
    {
        /// The first thing that could not be written, or `null`.
        public JsonError Error;

        public DataToken Write<T>(T value, string path)
        {
            if (typeof(T) == typeof(int))
            {
                return new DataToken((int)(object)value);
            }
            else if (typeof(T) == typeof(long))
            {
                return new DataToken((long)(object)value);
            }
            else if (typeof(T) == typeof(float))
            {
                return new DataToken((float)(object)value);
            }
            else if (typeof(T) == typeof(double))
            {
                return new DataToken((double)(object)value);
            }
            else if (typeof(T) == typeof(bool))
            {
                return new DataToken((bool)(object)value);
            }
            else if (typeof(T) == typeof(string))
            {
                string text = (string)(object)value;
                if (text == null) return new DataToken();
                return new DataToken(text);
            }
            else if (Reflect.IsEnum<T>())
            {
                return new DataToken((int)(object)value);
            }
            else if (Reflect.IsNullable<T>())
            {
                object boxed = value;
                if (boxed == null) return new DataToken();
                var inner = new JsonElementWriter(this, boxed, path);
                Reflect.VisitElementType<T, JsonElementWriter>(inner);
                return inner.Result;
            }
            else if (Reflect.IsArray<T>())
            {
                object boxed = value;
                if (boxed == null) return new DataToken();
                var elements = new JsonArrayWriter(this, boxed, path);
                Reflect.VisitElementType<T, JsonArrayWriter>(elements);
                return elements.Result;
            }
            else if (Reflect.IsList<T>())
            {
                object boxed = value;
                if (boxed == null) return new DataToken();
                var elements = new JsonListWriter(this, boxed, path);
                Reflect.VisitElementType<T, JsonListWriter>(elements);
                return elements.Result;
            }
            else if (Reflect.IsObject<T>())
            {
                object boxed = value;
                if (boxed == null) return new DataToken();
                var dictionary = new DataDictionary();
                Reflect.VisitFields(ref value, new JsonFieldWriter(this, dictionary, path));
                return new DataToken(dictionary);
            }
            else
            {
                Reflect.Unsupported<T>("Json.Stringify");
                return new DataToken();
            }
        }

        public void Fail(string path, string message)
        {
            if (Error == null)
            {
                Error = new JsonError(path, message);
            }
        }
    }

    /// `E?` with a value: written as the `E`.
    public sealed class JsonElementWriter : ITypeVisitor
    {
        private readonly JsonWriter writer;
        private readonly object subject;
        private readonly string path;
        public DataToken Result;

        public JsonElementWriter(JsonWriter writer, object subject, string path)
        {
            this.writer = writer;
            this.subject = subject;
            this.path = path;
        }

        public void Visit<E>()
        {
            Result = writer.Write<E>((E)subject, path);
        }
    }

    /// `E[]` as a `DataList`.
    public sealed class JsonArrayWriter : ITypeVisitor
    {
        private readonly JsonWriter writer;
        private readonly object subject;
        private readonly string path;
        public DataToken Result;

        public JsonArrayWriter(JsonWriter writer, object subject, string path)
        {
            this.writer = writer;
            this.subject = subject;
            this.path = path;
        }

        public void Visit<E>()
        {
            E[] array = (E[])subject;
            var list = new DataList();
            for (int i = 0; i < array.Length; i++)
            {
                list.Add(writer.Write<E>(array[i], path + "[" + i + "]"));
            }
            Result = new DataToken(list);
        }
    }

    /// `List<E>` as a `DataList`.
    public sealed class JsonListWriter : ITypeVisitor
    {
        private readonly JsonWriter writer;
        private readonly object subject;
        private readonly string path;
        public DataToken Result;

        public JsonListWriter(JsonWriter writer, object subject, string path)
        {
            this.writer = writer;
            this.subject = subject;
            this.path = path;
        }

        public void Visit<E>()
        {
            List<E> items = (List<E>)subject;
            var list = new DataList();
            for (int i = 0; i < items.Count; i++)
            {
                list.Add(writer.Write<E>(items[i], path + "[" + i + "]"));
            }
            Result = new DataToken(list);
        }
    }

    /// One field of an object, written under its JSON key.
    public sealed class JsonFieldWriter : IFieldVisitor
    {
        private readonly JsonWriter writer;
        private readonly DataDictionary dictionary;
        private readonly string path;

        public JsonFieldWriter(JsonWriter writer, DataDictionary dictionary, string path)
        {
            this.writer = writer;
            this.dictionary = dictionary;
            this.path = path;
        }

        public void Visit<F>(FieldInfo field, ref F value)
        {
            string key = JsonFields.KeyOf(field);
            if (key == null)
            {
                return;
            }
            dictionary.SetValue(key, writer.Write<F>(value, path + "." + key));
        }
    }
}

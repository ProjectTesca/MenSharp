// JSON for MenSharp: a `DataToken` tree read into a `T`.
//
// `Read<T>` is one generic method that the compiler specialises per `T`.
// Its arms are questions about `T` that MenSharp answers at compile time,
// so each specialisation is only the arm that applies — the arms for the
// other kinds of `T` are not compiled at all, which is what lets the
// object arm say `Reflect.New<T>()` while the `int` arm says `token.Number`.
// The last arm is a compile error: a `T` no arm covers is reported when
// the program is built, naming the type.
//
// Errors while reading are collected, not thrown: the first one wins and
// names the JSON path; reading continues so that a partially filled object
// is still returned to `TryParse`.

using System.Collections.Generic;
using MenSharp.Reflection;
using VRC.SDK3.Data;

namespace Bea4dev.Json
{
    public sealed class JsonReader
    {
        /// The first thing that did not fit, or `null`.
        public string Error;

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
                var inner = new ElementReader(this, token, path);
                Reflect.VisitElementType<T, ElementReader>(inner);
                return (T)inner.Result;
            }
            else if (Reflect.IsArray<T>())
            {
                if (token.TokenType == TokenType.Null) return default(T);
                if (token.TokenType != TokenType.DataList) return Mismatch<T>(token, path, "an array");
                var elements = new ArrayReader(this, token.DataList, path);
                Reflect.VisitElementType<T, ArrayReader>(elements);
                return (T)elements.Result;
            }
            else if (Reflect.IsList<T>())
            {
                if (token.TokenType == TokenType.Null) return default(T);
                if (token.TokenType != TokenType.DataList) return Mismatch<T>(token, path, "an array");
                var elements = new ListReader(this, token.DataList, path);
                Reflect.VisitElementType<T, ListReader>(elements);
                return (T)elements.Result;
            }
            else if (Reflect.IsObject<T>())
            {
                if (token.TokenType == TokenType.Null) return default(T);
                if (token.TokenType != TokenType.DataDictionary) return Mismatch<T>(token, path, "an object");
                T value = Reflect.New<T>();
                Reflect.VisitFields(ref value, new FieldReader(this, token.DataDictionary, path));
                return value;
            }
            else
            {
                Reflect.Unsupported<T>("Json.Parse");
                return default(T);
            }
        }

        public void Fail(string message)
        {
            if (Error == null)
            {
                Error = message;
            }
        }

        private T Mismatch<T>(DataToken token, string path, string wanted)
        {
            Fail(path + ": expected " + wanted + ", found " + Describe(token));
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
    internal sealed class ElementReader : ITypeVisitor
    {
        private readonly JsonReader reader;
        private readonly DataToken token;
        private readonly string path;
        public object Result;

        public ElementReader(JsonReader reader, DataToken token, string path)
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
    internal sealed class ArrayReader : ITypeVisitor
    {
        private readonly JsonReader reader;
        private readonly DataList list;
        private readonly string path;
        public object Result;

        public ArrayReader(JsonReader reader, DataList list, string path)
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
    internal sealed class ListReader : ITypeVisitor
    {
        private readonly JsonReader reader;
        private readonly DataList list;
        private readonly string path;
        public object Result;

        public ListReader(JsonReader reader, DataList list, string path)
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
    internal sealed class FieldReader : IFieldVisitor
    {
        private readonly JsonReader reader;
        private readonly DataDictionary dictionary;
        private readonly string path;

        public FieldReader(JsonReader reader, DataDictionary dictionary, string path)
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
                reader.Fail(path + ": missing required key \"" + key + "\"");
            }
        }
    }

    internal static class JsonFields
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
}

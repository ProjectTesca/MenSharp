// JSON for MenSharp: a `T` written out as a `DataToken` tree, the mirror
// of `JsonReader`. Same shape: one generic `Write<T>` whose arms the
// compiler settles per `T`.

using System.Collections.Generic;
using MenSharp.Reflection;
using VRC.SDK3.Data;

namespace Bea4dev.Json
{
    public sealed class JsonWriter
    {
        /// The first thing that could not be written, or `null`.
        public string Error;

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
                var inner = new ElementWriter(this, boxed, path);
                Reflect.VisitElementType<T, ElementWriter>(inner);
                return inner.Result;
            }
            else if (Reflect.IsArray<T>())
            {
                object boxed = value;
                if (boxed == null) return new DataToken();
                var elements = new ArrayWriter(this, boxed, path);
                Reflect.VisitElementType<T, ArrayWriter>(elements);
                return elements.Result;
            }
            else if (Reflect.IsList<T>())
            {
                object boxed = value;
                if (boxed == null) return new DataToken();
                var elements = new ListWriter(this, boxed, path);
                Reflect.VisitElementType<T, ListWriter>(elements);
                return elements.Result;
            }
            else if (Reflect.IsObject<T>())
            {
                object boxed = value;
                if (boxed == null) return new DataToken();
                var dictionary = new DataDictionary();
                Reflect.VisitFields(ref value, new FieldWriter(this, dictionary, path));
                return new DataToken(dictionary);
            }
            else
            {
                Reflect.Unsupported<T>("Json.Stringify");
                return new DataToken();
            }
        }

        public void Fail(string message)
        {
            if (Error == null)
            {
                Error = message;
            }
        }
    }

    /// `E?` with a value: written as the `E`.
    internal sealed class ElementWriter : ITypeVisitor
    {
        private readonly JsonWriter writer;
        private readonly object subject;
        private readonly string path;
        public DataToken Result;

        public ElementWriter(JsonWriter writer, object subject, string path)
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
    internal sealed class ArrayWriter : ITypeVisitor
    {
        private readonly JsonWriter writer;
        private readonly object subject;
        private readonly string path;
        public DataToken Result;

        public ArrayWriter(JsonWriter writer, object subject, string path)
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
    internal sealed class ListWriter : ITypeVisitor
    {
        private readonly JsonWriter writer;
        private readonly object subject;
        private readonly string path;
        public DataToken Result;

        public ListWriter(JsonWriter writer, object subject, string path)
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
    internal sealed class FieldWriter : IFieldVisitor
    {
        private readonly JsonWriter writer;
        private readonly DataDictionary dictionary;
        private readonly string path;

        public FieldWriter(JsonWriter writer, DataDictionary dictionary, string path)
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

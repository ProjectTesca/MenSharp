// How a `Dictionary<K, V>` field survives Unity's serialization, which has
// no idea what a dictionary is: the entries are kept beside the behaviour in
// a form Unity does serialize — text for keys and values that have a text
// form (numbers, strings, enums), object references for the rest — and put
// back into the field when the behaviour is deserialized (see
// MenSharpBehaviour's ISerializationCallbackReceiver). The compiled program
// never sees any of this: it is handed the entries (see MenSharpProxy) and
// builds its own hash table on first use.
using System;
using System.Collections;
using System.Collections.Generic;
using System.Globalization;
using System.Reflection;
using UnityEngine;

namespace MenSharp
{
    [Serializable]
    public class MenSharpDictionaryStore
    {
        /// The field the entries belong to.
        public string field;
        public string[] keys;
        public string[] values;
        /// The object form of each key / value — set where the type is a
        /// UnityEngine.Object, in which case the text is empty.
        public UnityEngine.Object[] keyObjects;
        public UnityEngine.Object[] valueObjects;
    }

    public static class MenSharpDictionarySerialization
    {
        private static readonly Dictionary<Type, FieldInfo[]> FieldCache = new Dictionary<Type, FieldInfo[]>();

        public static bool IsDictionaryType(Type type)
        {
            return type.IsGenericType && type.GetGenericTypeDefinition() == typeof(Dictionary<,>);
        }

        /// A key or value type the inspector can show and store: what has a
        /// text form, or is a Unity object.
        public static bool IsSupported(Type type)
        {
            return type.IsPrimitive
                || type.IsEnum
                || type == typeof(string)
                || type == typeof(decimal)
                || typeof(UnityEngine.Object).IsAssignableFrom(type);
        }

        public static bool IsSupportedDictionary(Type type)
        {
            if (!IsDictionaryType(type))
            {
                return false;
            }
            Type[] arguments = type.GetGenericArguments();
            return IsSupported(arguments[0]) && IsSupported(arguments[1]);
        }

        /// The dictionary fields the inspector serializes: public ones and
        /// `[SerializeField]` ones, minus `[NonSerialized]` — the rule every
        /// other field follows — declared below MenSharpBehaviour.
        public static FieldInfo[] DictionaryFields(Type type)
        {
            lock (FieldCache)
            {
                if (FieldCache.TryGetValue(type, out FieldInfo[] cached))
                {
                    return cached;
                }
            }
            var fields = new List<FieldInfo>();
            for (Type current = type;
                current != null && current != typeof(MenSharpBehaviour) && current != typeof(MonoBehaviour);
                current = current.BaseType)
            {
                foreach (FieldInfo field in current.GetFields(
                    BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance | BindingFlags.DeclaredOnly))
                {
                    if (!IsDictionaryType(field.FieldType)
                        || field.IsDefined(typeof(NonSerializedAttribute), false)
                        || (!field.IsPublic && !field.IsDefined(typeof(SerializeField), false)))
                    {
                        continue;
                    }
                    fields.Add(field);
                }
            }
            FieldInfo[] result = fields.ToArray();
            lock (FieldCache)
            {
                FieldCache[type] = result;
            }
            return result;
        }

        /// The fields' entries, as stores. A null dictionary is an empty one.
        public static MenSharpDictionaryStore[] Save(object owner)
        {
            var stores = new List<MenSharpDictionaryStore>();
            foreach (FieldInfo field in DictionaryFields(owner.GetType()))
            {
                if (!IsSupportedDictionary(field.FieldType))
                {
                    continue;
                }
                var dictionary = field.GetValue(owner) as IDictionary;
                int count = dictionary == null ? 0 : dictionary.Count;
                var store = new MenSharpDictionaryStore
                {
                    field = field.Name,
                    keys = new string[count],
                    values = new string[count],
                    keyObjects = new UnityEngine.Object[count],
                    valueObjects = new UnityEngine.Object[count],
                };
                int index = 0;
                if (dictionary != null)
                {
                    foreach (DictionaryEntry entry in dictionary)
                    {
                        store.keys[index] = ToText(entry.Key, out store.keyObjects[index]);
                        store.values[index] = ToText(entry.Value, out store.valueObjects[index]);
                        index++;
                    }
                }
                stores.Add(store);
            }
            return stores.ToArray();
        }

        /// Puts the stores' entries back into the fields. Every supported
        /// dictionary field ends up non-null, so the inspector has something
        /// to add to.
        public static void Load(object owner, MenSharpDictionaryStore[] stores)
        {
            foreach (FieldInfo field in DictionaryFields(owner.GetType()))
            {
                if (!IsSupportedDictionary(field.FieldType))
                {
                    continue;
                }
                MenSharpDictionaryStore store = null;
                foreach (MenSharpDictionaryStore candidate in stores ?? Array.Empty<MenSharpDictionaryStore>())
                {
                    if (candidate != null && candidate.field == field.Name)
                    {
                        store = candidate;
                        break;
                    }
                }
                var dictionary = (IDictionary)Activator.CreateInstance(field.FieldType);
                Type[] arguments = field.FieldType.GetGenericArguments();
                int count = store?.keys == null ? 0 : store.keys.Length;
                for (int index = 0; index < count; index++)
                {
                    object key = FromText(store.keys[index], Element(store.keyObjects, index), arguments[0]);
                    object value = FromText(
                        store.values == null || index >= store.values.Length ? null : store.values[index],
                        Element(store.valueObjects, index),
                        arguments[1]);
                    if (key == null || dictionary.Contains(key))
                    {
                        continue;
                    }
                    dictionary.Add(key, value);
                }
                field.SetValue(owner, dictionary);
            }
        }

        private static UnityEngine.Object Element(UnityEngine.Object[] objects, int index)
        {
            return objects != null && index < objects.Length ? objects[index] : null;
        }

        public static string ToText(object value, out UnityEngine.Object asObject)
        {
            asObject = null;
            if (value == null)
            {
                return "";
            }
            if (value is UnityEngine.Object unityObject)
            {
                asObject = unityObject;
                return "";
            }
            if (value.GetType().IsEnum)
            {
                return Enum.GetName(value.GetType(), value)
                    ?? Convert.ToInt64(value, CultureInfo.InvariantCulture).ToString(CultureInfo.InvariantCulture);
            }
            return Convert.ToString(value, CultureInfo.InvariantCulture);
        }

        public static object FromText(string text, UnityEngine.Object asObject, Type type)
        {
            if (typeof(UnityEngine.Object).IsAssignableFrom(type))
            {
                return asObject;
            }
            if (type == typeof(string))
            {
                return text ?? "";
            }
            if (string.IsNullOrEmpty(text))
            {
                return type.IsValueType ? Activator.CreateInstance(type) : null;
            }
            try
            {
                if (type.IsEnum)
                {
                    try
                    {
                        return Enum.Parse(type, text);
                    }
                    catch (Exception)
                    {
                        return Enum.ToObject(type, long.Parse(text, CultureInfo.InvariantCulture));
                    }
                }
                if (type == typeof(char))
                {
                    return text[0];
                }
                return Convert.ChangeType(text, type, CultureInfo.InvariantCulture);
            }
            catch (Exception)
            {
                return type.IsValueType ? Activator.CreateInstance(type) : null;
            }
        }
    }
}

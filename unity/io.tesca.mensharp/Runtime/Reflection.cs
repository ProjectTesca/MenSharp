// MenSharp static reflection, as Unity's C# compiler sees it.
//
// `MenSharp.Reflection` is answered by the MenSharp compiler: every call to
// `Reflect` is lowered for the type argument at hand, while the program is
// built. These declarations exist so that a script using them compiles in
// Unity too — and, because a MenSharpBehaviour's proxy never runs, mostly
// only have to exist. They are nevertheless real: this file answers the
// same questions with .NET reflection, so that editor-side code (tests, an
// inspector) can call the same API and get the same answers for every type
// MenSharp accepts.

using System;
using System.Collections.Generic;
using System.Reflection;

namespace MenSharp.Reflection
{
    /// One field (or auto-property) of a type, as `Reflect.VisitFields`
    /// hands it to a visitor: its name, its accessibility, and the
    /// attributes written on it.
    public sealed class FieldInfo
    {
        private readonly string name;
        private readonly bool isPublic;
        private readonly object[] attributes;

        public FieldInfo(string name, bool isPublic, object[] attributes)
        {
            this.name = name;
            this.isPublic = isPublic;
            this.attributes = attributes;
        }

        public string Name => name;

        public bool IsPublic => isPublic;

        public int AttributeCount => attributes.Length;

        /// The first attribute of type `A` (or a subtype) on the member, or
        /// `null`.
        public A Attribute<A>() where A : class
        {
            foreach (object attribute in attributes)
            {
                if (attribute is A found)
                {
                    return found;
                }
            }
            return null;
        }

        public bool Has<A>() where A : class => Attribute<A>() != null;
    }

    /// What `Reflect.VisitFields` calls, once per field, with `TField`
    /// bound to that field's type.
    public interface IFieldVisitor
    {
        void Visit<TField>(FieldInfo field, ref TField value);
    }

    /// What `Reflect.VisitElementType` calls, with `TType` bound to the
    /// element type it found.
    public interface ITypeVisitor
    {
        void Visit<TType>();
    }

    /// Compile-time questions about a type argument. Under MenSharp each
    /// call is lowered to its answer; here they are answered by reflection.
    public static class Reflect
    {
        /// Calls `visitor.Visit<F>(field, ref value)` for every instance
        /// field and auto-property of `T`, base class first.
        public static void VisitFields<T, V>(ref T target, V visitor) where V : IFieldVisitor
        {
            object boxed = target;
            foreach (MemberInfo member in StorageOf(typeof(T)))
            {
                Type fieldType = member is System.Reflection.FieldInfo f ? f.FieldType : ((PropertyInfo)member).PropertyType;
                object[] attributes = member.GetCustomAttributes(true);
                var info = new FieldInfo(member.Name, IsPublic(member), attributes);
                MethodInfo visit = typeof(V).GetMethod("Visit").MakeGenericMethod(fieldType);
                object[] arguments = { info, Read(member, boxed) };
                visit.Invoke(visitor, arguments);
                Write(member, boxed, arguments[1]);
            }
            target = (T)boxed;
        }

        /// A new `T` through its parameterless constructor, or a struct at
        /// its defaults.
        public static T New<T>()
        {
            return Activator.CreateInstance<T>();
        }

        /// Whether `T` is a class, struct or record of the project's own —
        /// a type `VisitFields` can walk. Engine and BCL types are not.
        public static bool IsObject<T>()
        {
            Type type = typeof(T);
            if (type.IsPrimitive || type.IsEnum || type.IsArray || type == typeof(string))
            {
                return false;
            }
            string assembly = type.Assembly.GetName().Name;
            return !assembly.StartsWith("System", StringComparison.Ordinal)
                && !assembly.StartsWith("mscorlib", StringComparison.Ordinal)
                && !assembly.StartsWith("netstandard", StringComparison.Ordinal)
                && !assembly.StartsWith("UnityEngine", StringComparison.Ordinal)
                && !assembly.StartsWith("VRC", StringComparison.Ordinal);
        }

        public static bool IsArray<T>() => typeof(T).IsArray && typeof(T).GetArrayRank() == 1;

        public static bool IsList<T>() =>
            typeof(T).IsGenericType && typeof(T).GetGenericTypeDefinition() == typeof(List<>);

        public static bool IsNullable<T>() => Nullable.GetUnderlyingType(typeof(T)) != null;

        public static bool IsEnum<T>() => typeof(T).IsEnum;

        public static bool Is<T, U>() => typeof(T) == typeof(U);

        /// Under MenSharp, a compile error naming `T`; here, the same at
        /// run time.
        public static void Unsupported<T>(string what)
        {
            throw new NotSupportedException($"{what} does not support {typeof(T)}");
        }

        /// Calls `visitor.Visit<E>()` with `E` the element type of `T`: an
        /// array's element, a `List<E>`'s `E`, or an `E?`'s `E`.
        public static void VisitElementType<T, V>(V visitor) where V : ITypeVisitor
        {
            Type type = typeof(T);
            Type element;
            if (type.IsArray)
            {
                element = type.GetElementType();
            }
            else if (Nullable.GetUnderlyingType(type) != null)
            {
                element = Nullable.GetUnderlyingType(type);
            }
            else if (IsList<T>())
            {
                element = type.GetGenericArguments()[0];
            }
            else
            {
                throw new NotSupportedException($"{type} has no element type");
            }
            typeof(V).GetMethod("Visit").MakeGenericMethod(element).Invoke(visitor, null);
        }

        // ---------------------------------------------------------------

        private const BindingFlags Instance =
            BindingFlags.Instance | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly;

        /// Fields and auto-properties, base class first, in declaration
        /// order — the slots MenSharp lays an object out in.
        private static IEnumerable<MemberInfo> StorageOf(Type type)
        {
            if (type.BaseType != null && type.BaseType != typeof(object) && type.BaseType != typeof(ValueType))
            {
                foreach (MemberInfo inherited in StorageOf(type.BaseType))
                {
                    yield return inherited;
                }
            }
            var declared = new List<MemberInfo>();
            foreach (System.Reflection.FieldInfo field in type.GetFields(Instance))
            {
                // an auto-property's backing field is the property's slot
                if (!field.Name.EndsWith("k__BackingField", StringComparison.Ordinal))
                {
                    declared.Add(field);
                }
            }
            foreach (PropertyInfo property in type.GetProperties(Instance))
            {
                if (type.GetField($"<{property.Name}>k__BackingField", Instance) != null)
                {
                    declared.Add(property);
                }
            }
            declared.Sort((a, b) => a.MetadataToken.CompareTo(b.MetadataToken));
            foreach (MemberInfo member in declared)
            {
                yield return member;
            }
        }

        private static bool IsPublic(MemberInfo member)
        {
            return member is System.Reflection.FieldInfo field
                ? field.IsPublic
                : ((PropertyInfo)member).GetMethod?.IsPublic ?? false;
        }

        private static object Read(MemberInfo member, object target)
        {
            return member is System.Reflection.FieldInfo field
                ? field.GetValue(target)
                : ((PropertyInfo)member).GetValue(target);
        }

        private static void Write(MemberInfo member, object target, object value)
        {
            if (member is System.Reflection.FieldInfo field)
            {
                field.SetValue(target, value);
            }
            else
            {
                var property = (PropertyInfo)member;
                MethodInfo setter = property.SetMethod
                    ?? throw new NotSupportedException($"{property.Name} has no setter");
                setter.Invoke(target, new[] { value });
            }
        }
    }
}

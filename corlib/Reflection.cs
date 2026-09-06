// MenSharp static reflection: `MenSharp.Reflection`.
//
// Udon has no `System.Reflection` — nothing that lists the fields of a type
// at run time — and an M# object is an `object[]` with no metadata at all.
// What M# does have is a compiler that knows every field of every type it
// compiles, and monomorphizes generic code per type argument. So reflection
// here is *static*: every question is asked about a type argument and
// answered while the program is compiled. The API is shaped so that there is
// nothing dynamic to ask — no `Type` values, no lookups by name — which is
// what lets every use compile down to straight-line code, or to an error
// naming the type when the answer does not exist.
//
// The methods of `Reflect` are intrinsics: their bodies below are
// placeholders, and the compiler lowers each call in place (see the code
// generator's `reflect` module). The visitor interfaces are ordinary
// interfaces, and a visitor is an ordinary class or struct; the generic
// `Visit<TField>` is what the compiler instantiates once per field, with
// `TField` bound to that field's static type.
//
// The same declarations exist in the Unity package's runtime assembly so
// that a script using them compiles under Unity's C# compiler too; there
// they run on real reflection, which agrees with what the compiler does
// here for every type M# accepts.

using System;

namespace System
{
    /// The base of every attribute class. A source twin of the BCL's, so
    /// that an attribute class written in M# (`class JsonName : Attribute`)
    /// is an ordinary M# class — with a layout, and constructible — rather
    /// than something derived from an engine type Udon cannot build. It
    /// holds nothing: an attribute is its own fields.
    public abstract class Attribute
    {
    }
}

namespace MenSharp.Reflection
{
    /// One field (or auto-property) of an M# type, as `Reflect.VisitFields`
    /// hands it to a visitor. What it knows was decided at compile time:
    /// the name, the accessibility, and the attributes written on it.
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

        /// The declared name of the field or property.
        public string Name
        {
            get { return name; }
        }

        /// Whether the member is `public`.
        public bool IsPublic
        {
            get { return isPublic; }
        }

        /// The number of attributes written on the member that the
        /// compiler could construct (attribute classes of the compilation's
        /// own; engine attributes such as `[SerializeField]` are not).
        public int AttributeCount
        {
            get { return attributes.Length; }
        }

        /// The first attribute of type `A` (or a subtype) written on the
        /// member, constructed with the arguments it was written with, or
        /// `null` when there is none.
        public A Attribute<A>() where A : class
        {
            for (int i = 0; i < attributes.Length; i++)
            {
                if (attributes[i] is A found)
                {
                    return found;
                }
            }
            return default(A);
        }

        /// Whether an attribute of type `A` is written on the member.
        public bool Has<A>() where A : class
        {
            return Attribute<A>() != null;
        }
    }

    /// What `Reflect.VisitFields` calls, once per field, with `TField`
    /// bound to that field's type. `value` is the field itself: assigning
    /// to it writes the field.
    public interface IFieldVisitor
    {
        void Visit<TField>(FieldInfo field, ref TField value);
    }

    /// What `Reflect.VisitElementType` calls, with `TType` bound to the type
    /// it found (an array's element type, a `List<T>`'s `T`, a `T?`'s `T`).
    public interface ITypeVisitor
    {
        void Visit<TType>();
    }

    /// Compile-time questions about a type argument. Every method here is an
    /// intrinsic: the compiler answers it for the `T` at hand and lowers the
    /// call to the answer — a constant, an object, or a sequence of calls.
    public static class Reflect
    {
        /// Calls `visitor.Visit<F>(field, ref value)` for every instance
        /// field and auto-property of `T`, base class first, in declaration
        /// order. `T` must be a class, struct or record of the compilation's
        /// own; `V` a concrete class or struct implementing `IFieldVisitor`.
        public static void VisitFields<T, V>(ref T target, V visitor) where V : IFieldVisitor
        {
        }

        /// A new `T`: a class or record through its parameterless
        /// constructor, a struct with its fields at their defaults.
        public static T New<T>()
        {
            return default(T);
        }

        /// Whether `T` is a class, struct or record of the compilation's own
        /// — a type `VisitFields` can walk.
        public static bool IsObject<T>()
        {
            return false;
        }

        /// Whether `T` is a one-dimensional array.
        public static bool IsArray<T>()
        {
            return false;
        }

        /// Whether `T` is `System.Collections.Generic.List<E>`.
        public static bool IsList<T>()
        {
            return false;
        }

        /// Whether `T` is `E?` for a value type `E`.
        public static bool IsNullable<T>()
        {
            return false;
        }

        /// Whether `T` is an enum.
        public static bool IsEnum<T>()
        {
            return false;
        }

        /// Whether `T` and `U` are the same type. `typeof(T) == typeof(U)`
        /// folds to the same answer.
        public static bool Is<T, U>()
        {
            return false;
        }

        /// A compile error naming `T`: "`what` does not support `T`". For
        /// the last arm of generic code that has covered every type it can
        /// handle — so that handing it a type it cannot is caught when the
        /// program is built, not when the world is running.
        public static void Unsupported<T>(string what)
        {
        }

        /// Calls `visitor.Visit<E>()` with `E` the element type of `T`: what
        /// an array holds, a `List<E>`'s `E`, or the `E` of an `E?`. Any
        /// other `T` is a compile error.
        public static void VisitElementType<T, V>(V visitor) where V : ITypeVisitor
        {
        }
    }
}

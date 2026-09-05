// MenSharp mini-corlib: how a `T` is ordered and compared for equality —
// what .NET does through `Comparer<T>.Default` and
// `EqualityComparer<T>.Default`, and what LINQ's `OrderBy`, `Min`, `Max`,
// `Distinct`, `Contains`, `GroupBy` and `List<T>.Sort()` are built on.
//
// Udon has no `IComparable` extern: each type that orders itself exposes
// its own `CompareTo` (`SystemInt32.__CompareTo__SystemInt32__SystemInt32`,
// `SystemString.__CompareTo__SystemString__SystemInt32`, ...). So
// `Compare<T>` is lowered by the compiler for the `T` at hand — see the
// code generator's `comparers` module — rather than written here.

using System;

namespace System
{
    /// Source twin of .NET's: a class or struct that orders its own values.
    /// Implement it and `OrderBy`, `Min`, `Max` and `List<T>.Sort()` use it.
    public interface IComparable<T>
    {
        int CompareTo(T other);
    }
}

namespace MenSharp.Internal
{
    public static class Comparers
    {
        /// `Comparer<T>.Default.Compare(a, b)`. Lowered by the compiler for
        /// the `T` at hand: the type's own `CompareTo` extern for a number,
        /// a string, a char, ...; the class's `IComparable<T>.CompareTo` for
        /// a source type that implements it; a compile error for anything
        /// else. `null` orders before everything, as in .NET.
        public static int Compare<T>(T a, T b)
        {
            return 0;
        }

        /// `EqualityComparer<T>.Default.Equals(a, b)`: the value's own
        /// `Equals`, with `null` equal to `null` only.
        public static bool Equal<T>(T a, T b)
        {
            object left = a;
            object right = b;
            if (left == null)
            {
                return right == null;
            }
            if (right == null)
            {
                return false;
            }
            return left.Equals(right);
        }

        /// `EqualityComparer<T>.Default.GetHashCode(value)`: 0 for `null`.
        public static int Hash<T>(T value)
        {
            object boxed = value;
            if (boxed == null)
            {
                return 0;
            }
            return boxed.GetHashCode();
        }
    }
}

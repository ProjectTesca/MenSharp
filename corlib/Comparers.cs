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

namespace System.Collections.Generic
{
    /// Source twin of .NET's: an object that orders two values. Passed to
    /// `List<T>.Sort`, `BinarySearch` and LINQ's `OrderBy` family.
    public interface IComparer<T>
    {
        int Compare(T x, T y);
    }

    /// Source twin of .NET's: an object that decides equality and hashes
    /// consistently with it. Passed to `Dictionary`, `HashSet` and LINQ's
    /// `Distinct` family.
    public interface IEqualityComparer<T>
    {
        bool Equals(T x, T y);
        int GetHashCode(T obj);
    }

    /// Source twin of .NET's `Comparer<T>`: `Default` orders by the type's
    /// own ordering, `Create` wraps a `Comparison<T>`.
    public abstract class Comparer<T> : IComparer<T>
    {
        public static Comparer<T> Default
        {
            get { return new MenSharp.Internal.DefaultComparer<T>(); }
        }

        public static Comparer<T> Create(Comparison<T> comparison)
        {
            if (comparison == null) { throw new ArgumentNullException("comparison"); }
            return new MenSharp.Internal.ComparisonComparer<T>(comparison);
        }

        public abstract int Compare(T x, T y);
    }

    /// Source twin of .NET's `EqualityComparer<T>`: `Default` is the value's
    /// own `Equals`/`GetHashCode`, with `null` equal to `null` only.
    public abstract class EqualityComparer<T> : IEqualityComparer<T>
    {
        public static EqualityComparer<T> Default
        {
            get { return new MenSharp.Internal.DefaultEqualityComparer<T>(); }
        }

        public abstract bool Equals(T x, T y);
        public abstract int GetHashCode(T obj);
    }
}

namespace System
{
    /// Source twin of .NET's `StringComparer`. The BCL class is not
    /// whitelisted, but `string.Compare`/`string.Equals` with a
    /// `StringComparison` are, so each of the six comparers delegates to
    /// those; the hash of an ignore-case comparer is the hash of the
    /// upper-cased string, so it agrees with its `Equals`.
    public abstract class StringComparer : System.Collections.Generic.IComparer<string>, System.Collections.Generic.IEqualityComparer<string>
    {
        public static StringComparer Ordinal
        {
            get { return new MenSharp.Internal.ComparisonStringComparer(StringComparison.Ordinal); }
        }

        public static StringComparer OrdinalIgnoreCase
        {
            get { return new MenSharp.Internal.ComparisonStringComparer(StringComparison.OrdinalIgnoreCase); }
        }

        public static StringComparer InvariantCulture
        {
            get { return new MenSharp.Internal.ComparisonStringComparer(StringComparison.InvariantCulture); }
        }

        public static StringComparer InvariantCultureIgnoreCase
        {
            get { return new MenSharp.Internal.ComparisonStringComparer(StringComparison.InvariantCultureIgnoreCase); }
        }

        public static StringComparer CurrentCulture
        {
            get { return new MenSharp.Internal.ComparisonStringComparer(StringComparison.CurrentCulture); }
        }

        public static StringComparer CurrentCultureIgnoreCase
        {
            get { return new MenSharp.Internal.ComparisonStringComparer(StringComparison.CurrentCultureIgnoreCase); }
        }

        public abstract int Compare(string x, string y);
        public abstract bool Equals(string x, string y);
        public abstract int GetHashCode(string obj);
    }
}

namespace MenSharp.Internal
{
    /// `Comparer<T>.Default`.
    public sealed class DefaultComparer<T> : System.Collections.Generic.Comparer<T>
    {
        public override int Compare(T x, T y)
        {
            return Comparers.Compare(x, y);
        }
    }

    /// `Comparer<T>.Create(comparison)`.
    public sealed class ComparisonComparer<T> : System.Collections.Generic.Comparer<T>
    {
        private readonly Comparison<T> comparison;

        public ComparisonComparer(Comparison<T> comparison)
        {
            this.comparison = comparison;
        }

        public override int Compare(T x, T y)
        {
            return comparison(x, y);
        }
    }

    /// `EqualityComparer<T>.Default`.
    public sealed class DefaultEqualityComparer<T> : System.Collections.Generic.EqualityComparer<T>
    {
        public override bool Equals(T x, T y)
        {
            return Comparers.Equal(x, y);
        }

        public override int GetHashCode(T obj)
        {
            return Comparers.Hash(obj);
        }
    }

    /// The six `StringComparer`s: one `StringComparison` each.
    public sealed class ComparisonStringComparer : StringComparer
    {
        private readonly StringComparison comparison;

        public ComparisonStringComparer(StringComparison comparison)
        {
            this.comparison = comparison;
        }

        public override int Compare(string x, string y)
        {
            return string.Compare(x, y, comparison);
        }

        public override bool Equals(string x, string y)
        {
            return string.Equals(x, y, comparison);
        }

        public override int GetHashCode(string obj)
        {
            if (obj == null) { return 0; }
            if (comparison == StringComparison.CurrentCultureIgnoreCase)
            {
                return obj.ToUpper().GetHashCode();
            }
            if (comparison == StringComparison.OrdinalIgnoreCase || comparison == StringComparison.InvariantCultureIgnoreCase)
            {
                return obj.ToUpperInvariant().GetHashCode();
            }
            return obj.GetHashCode();
        }
    }
}

// MenSharp mini-corlib: System.Linq.Enumerable.
//
// LINQ to Objects, written over the sequences of Iterators.cs: every
// deferred operator (`Where`, `Select`, `Take`, `OrderBy`, ...) is an
// iterator method, so a chain is as lazy and as single-pass as .NET's, and
// every eager one (`Sum`, `First`, `ToArray`, ...) is a loop. A user's
// script declares `using System.Linq;` and writes what it would in C#; the
// same file compiles under Unity against the real System.Core.
//
// Ordering and equality of a `T` go through `MenSharp.Internal.Comparers`
// (Comparers.cs): a type that has no ordering is a compile error at the
// `OrderBy`/`Min`/`Max` that needs one, not a runtime surprise.
//
// Not here: the overloads taking an `IComparer<T>`/`IEqualityComparer<T>`,
// the `Nullable` numeric overloads, `Cast`/`OfType` (Udon cannot test the
// type of a box against a source class), `ToHashSet`/`ToLookup`, and the
// `Join`/`GroupJoin` family. A missing one is an ordinary "no such method"
// error. `ArgumentNullException` for a null source is not checked: a null
// sequence fails at its first `GetEnumerator` like any null receiver.

using System;
using System.Collections.Generic;
using MenSharp.Internal;

namespace System.Linq
{
    /// A sorted sequence that `ThenBy` can refine.
    public interface IOrderedEnumerable<TElement> : IEnumerable<TElement>
    {
        /// The same ordering with one more, less significant, key. Written
        /// by `ThenBy`/`ThenByDescending`.
        IOrderedEnumerable<TElement> __Then(SortLevel<TElement> level);
    }

    /// One group of `GroupBy`: a key and the elements that share it.
    public interface IGrouping<TKey, TElement> : IEnumerable<TElement>
    {
        TKey Key { get; }
    }

    public static class Enumerable
    {
        // ------------------------------------------------------- filtering

        public static IEnumerable<TSource> Where<TSource>(this IEnumerable<TSource> source, Func<TSource, bool> predicate)
        {
            foreach (TSource item in source)
            {
                if (predicate(item)) { yield return item; }
            }
        }

        public static IEnumerable<TSource> Where<TSource>(this IEnumerable<TSource> source, Func<TSource, int, bool> predicate)
        {
            int index = 0;
            foreach (TSource item in source)
            {
                if (predicate(item, index)) { yield return item; }
                index++;
            }
        }

        // ------------------------------------------------------ projection

        public static IEnumerable<TResult> Select<TSource, TResult>(this IEnumerable<TSource> source, Func<TSource, TResult> selector)
        {
            foreach (TSource item in source)
            {
                yield return selector(item);
            }
        }

        public static IEnumerable<TResult> Select<TSource, TResult>(this IEnumerable<TSource> source, Func<TSource, int, TResult> selector)
        {
            int index = 0;
            foreach (TSource item in source)
            {
                yield return selector(item, index);
                index++;
            }
        }

        public static IEnumerable<TResult> SelectMany<TSource, TResult>(this IEnumerable<TSource> source, Func<TSource, IEnumerable<TResult>> selector)
        {
            foreach (TSource item in source)
            {
                foreach (TResult inner in selector(item))
                {
                    yield return inner;
                }
            }
        }

        public static IEnumerable<TResult> SelectMany<TSource, TCollection, TResult>(this IEnumerable<TSource> source, Func<TSource, IEnumerable<TCollection>> collectionSelector, Func<TSource, TCollection, TResult> resultSelector)
        {
            foreach (TSource item in source)
            {
                foreach (TCollection inner in collectionSelector(item))
                {
                    yield return resultSelector(item, inner);
                }
            }
        }

        // ---------------------------------------------------- partitioning

        public static IEnumerable<TSource> Take<TSource>(this IEnumerable<TSource> source, int count)
        {
            if (count <= 0) { yield break; }
            int taken = 0;
            foreach (TSource item in source)
            {
                yield return item;
                taken++;
                if (taken >= count) { yield break; }
            }
        }

        public static IEnumerable<TSource> Skip<TSource>(this IEnumerable<TSource> source, int count)
        {
            int skipped = 0;
            foreach (TSource item in source)
            {
                if (skipped < count) { skipped++; continue; }
                yield return item;
            }
        }

        public static IEnumerable<TSource> TakeWhile<TSource>(this IEnumerable<TSource> source, Func<TSource, bool> predicate)
        {
            foreach (TSource item in source)
            {
                if (!predicate(item)) { yield break; }
                yield return item;
            }
        }

        public static IEnumerable<TSource> TakeWhile<TSource>(this IEnumerable<TSource> source, Func<TSource, int, bool> predicate)
        {
            int index = 0;
            foreach (TSource item in source)
            {
                if (!predicate(item, index)) { yield break; }
                yield return item;
                index++;
            }
        }

        public static IEnumerable<TSource> SkipWhile<TSource>(this IEnumerable<TSource> source, Func<TSource, bool> predicate)
        {
            bool yielding = false;
            foreach (TSource item in source)
            {
                if (!yielding && !predicate(item)) { yielding = true; }
                if (yielding) { yield return item; }
            }
        }

        public static IEnumerable<TSource> SkipWhile<TSource>(this IEnumerable<TSource> source, Func<TSource, int, bool> predicate)
        {
            bool yielding = false;
            int index = 0;
            foreach (TSource item in source)
            {
                if (!yielding && !predicate(item, index)) { yielding = true; }
                if (yielding) { yield return item; }
                index++;
            }
        }

        // --------------------------------------------------------- joining

        public static IEnumerable<TSource> Concat<TSource>(this IEnumerable<TSource> first, IEnumerable<TSource> second)
        {
            foreach (TSource item in first) { yield return item; }
            foreach (TSource item in second) { yield return item; }
        }

        public static IEnumerable<TSource> Append<TSource>(this IEnumerable<TSource> source, TSource element)
        {
            foreach (TSource item in source) { yield return item; }
            yield return element;
        }

        public static IEnumerable<TSource> Prepend<TSource>(this IEnumerable<TSource> source, TSource element)
        {
            yield return element;
            foreach (TSource item in source) { yield return item; }
        }

        public static IEnumerable<TResult> Zip<TFirst, TSecond, TResult>(this IEnumerable<TFirst> first, IEnumerable<TSecond> second, Func<TFirst, TSecond, TResult> resultSelector)
        {
            IEnumerator<TFirst> left = first.GetEnumerator();
            try
            {
                IEnumerator<TSecond> right = second.GetEnumerator();
                try
                {
                    while (left.MoveNext() && right.MoveNext())
                    {
                        yield return resultSelector(left.Current, right.Current);
                    }
                }
                finally
                {
                    right.Dispose();
                }
            }
            finally
            {
                left.Dispose();
            }
        }

        public static IEnumerable<TSource> DefaultIfEmpty<TSource>(this IEnumerable<TSource> source)
        {
            return DefaultIfEmpty(source, default(TSource));
        }

        public static IEnumerable<TSource> DefaultIfEmpty<TSource>(this IEnumerable<TSource> source, TSource defaultValue)
        {
            bool any = false;
            foreach (TSource item in source)
            {
                any = true;
                yield return item;
            }
            if (!any) { yield return defaultValue; }
        }

        // ------------------------------------------------------------ sets

        public static IEnumerable<TSource> Distinct<TSource>(this IEnumerable<TSource> source)
        {
            return Distinct(source, null);
        }

        public static IEnumerable<TSource> Distinct<TSource>(this IEnumerable<TSource> source, IEqualityComparer<TSource> comparer)
        {
            HashSet<TSource> seen = new HashSet<TSource>(comparer);
            foreach (TSource item in source)
            {
                if (seen.Add(item)) { yield return item; }
            }
        }

        public static IEnumerable<TSource> Union<TSource>(this IEnumerable<TSource> first, IEnumerable<TSource> second)
        {
            return Union(first, second, null);
        }

        public static IEnumerable<TSource> Union<TSource>(this IEnumerable<TSource> first, IEnumerable<TSource> second, IEqualityComparer<TSource> comparer)
        {
            HashSet<TSource> seen = new HashSet<TSource>(comparer);
            foreach (TSource item in first)
            {
                if (seen.Add(item)) { yield return item; }
            }
            foreach (TSource item in second)
            {
                if (seen.Add(item)) { yield return item; }
            }
        }

        public static IEnumerable<TSource> Intersect<TSource>(this IEnumerable<TSource> first, IEnumerable<TSource> second)
        {
            return Intersect(first, second, null);
        }

        public static IEnumerable<TSource> Intersect<TSource>(this IEnumerable<TSource> first, IEnumerable<TSource> second, IEqualityComparer<TSource> comparer)
        {
            HashSet<TSource> candidates = new HashSet<TSource>(comparer);
            foreach (TSource item in second) { candidates.Add(item); }
            foreach (TSource item in first)
            {
                if (candidates.Remove(item)) { yield return item; }
            }
        }

        public static IEnumerable<TSource> Except<TSource>(this IEnumerable<TSource> first, IEnumerable<TSource> second)
        {
            return Except(first, second, null);
        }

        public static IEnumerable<TSource> Except<TSource>(this IEnumerable<TSource> first, IEnumerable<TSource> second, IEqualityComparer<TSource> comparer)
        {
            HashSet<TSource> excluded = new HashSet<TSource>(comparer);
            foreach (TSource item in second) { excluded.Add(item); }
            foreach (TSource item in first)
            {
                if (excluded.Add(item)) { yield return item; }
            }
        }

        // -------------------------------------------------------- ordering

        public static IOrderedEnumerable<TSource> OrderBy<TSource, TKey>(this IEnumerable<TSource> source, Func<TSource, TKey> keySelector)
        {
            return new OrderedEnumerable<TSource>(source, new KeyLevel<TSource, TKey>(keySelector, null, false));
        }

        public static IOrderedEnumerable<TSource> OrderBy<TSource, TKey>(this IEnumerable<TSource> source, Func<TSource, TKey> keySelector, IComparer<TKey> comparer)
        {
            return new OrderedEnumerable<TSource>(source, new KeyLevel<TSource, TKey>(keySelector, comparer, false));
        }

        public static IOrderedEnumerable<TSource> OrderByDescending<TSource, TKey>(this IEnumerable<TSource> source, Func<TSource, TKey> keySelector)
        {
            return new OrderedEnumerable<TSource>(source, new KeyLevel<TSource, TKey>(keySelector, null, true));
        }

        public static IOrderedEnumerable<TSource> OrderByDescending<TSource, TKey>(this IEnumerable<TSource> source, Func<TSource, TKey> keySelector, IComparer<TKey> comparer)
        {
            return new OrderedEnumerable<TSource>(source, new KeyLevel<TSource, TKey>(keySelector, comparer, true));
        }

        public static IOrderedEnumerable<TSource> ThenBy<TSource, TKey>(this IOrderedEnumerable<TSource> source, Func<TSource, TKey> keySelector)
        {
            return source.__Then(new KeyLevel<TSource, TKey>(keySelector, null, false));
        }

        public static IOrderedEnumerable<TSource> ThenBy<TSource, TKey>(this IOrderedEnumerable<TSource> source, Func<TSource, TKey> keySelector, IComparer<TKey> comparer)
        {
            return source.__Then(new KeyLevel<TSource, TKey>(keySelector, comparer, false));
        }

        public static IOrderedEnumerable<TSource> ThenByDescending<TSource, TKey>(this IOrderedEnumerable<TSource> source, Func<TSource, TKey> keySelector)
        {
            return source.__Then(new KeyLevel<TSource, TKey>(keySelector, null, true));
        }

        public static IOrderedEnumerable<TSource> ThenByDescending<TSource, TKey>(this IOrderedEnumerable<TSource> source, Func<TSource, TKey> keySelector, IComparer<TKey> comparer)
        {
            return source.__Then(new KeyLevel<TSource, TKey>(keySelector, comparer, true));
        }

        public static IEnumerable<TSource> Reverse<TSource>(this IEnumerable<TSource> source)
        {
            TSource[] items = ToArray(source);
            for (int i = items.Length - 1; i >= 0; i--)
            {
                yield return items[i];
            }
        }

        // -------------------------------------------------------- grouping

        public static IEnumerable<IGrouping<TKey, TSource>> GroupBy<TSource, TKey>(this IEnumerable<TSource> source, Func<TSource, TKey> keySelector)
        {
            return GroupBy(source, keySelector, (IEqualityComparer<TKey>)null);
        }

        public static IEnumerable<IGrouping<TKey, TSource>> GroupBy<TSource, TKey>(this IEnumerable<TSource> source, Func<TSource, TKey> keySelector, IEqualityComparer<TKey> comparer)
        {
            List<Grouping<TKey, TSource>> groups = Group(source, keySelector, comparer);
            foreach (Grouping<TKey, TSource> group in groups)
            {
                yield return group;
            }
        }

        public static IEnumerable<IGrouping<TKey, TElement>> GroupBy<TSource, TKey, TElement>(this IEnumerable<TSource> source, Func<TSource, TKey> keySelector, Func<TSource, TElement> elementSelector)
        {
            return GroupBy(source, keySelector, elementSelector, null);
        }

        public static IEnumerable<IGrouping<TKey, TElement>> GroupBy<TSource, TKey, TElement>(this IEnumerable<TSource> source, Func<TSource, TKey> keySelector, Func<TSource, TElement> elementSelector, IEqualityComparer<TKey> comparer)
        {
            List<Grouping<TKey, TElement>> groups = Group(source, keySelector, elementSelector, comparer);
            foreach (Grouping<TKey, TElement> group in groups)
            {
                yield return group;
            }
        }

        public static IEnumerable<TResult> GroupBy<TSource, TKey, TResult>(this IEnumerable<TSource> source, Func<TSource, TKey> keySelector, Func<TKey, IEnumerable<TSource>, TResult> resultSelector)
        {
            List<Grouping<TKey, TSource>> groups = Group(source, keySelector, null);
            foreach (Grouping<TKey, TSource> group in groups)
            {
                yield return resultSelector(group.Key, group);
            }
        }

        // the groups, in order of each key's first appearance, as .NET's
        private static List<Grouping<TKey, TSource>> Group<TSource, TKey>(IEnumerable<TSource> source, Func<TSource, TKey> keySelector, IEqualityComparer<TKey> comparer)
        {
            List<Grouping<TKey, TSource>> groups = new List<Grouping<TKey, TSource>>();
            foreach (TSource item in source)
            {
                FindGroup(groups, keySelector(item), comparer).Add(item);
            }
            return groups;
        }

        private static List<Grouping<TKey, TElement>> Group<TSource, TKey, TElement>(IEnumerable<TSource> source, Func<TSource, TKey> keySelector, Func<TSource, TElement> elementSelector, IEqualityComparer<TKey> comparer)
        {
            List<Grouping<TKey, TElement>> groups = new List<Grouping<TKey, TElement>>();
            foreach (TSource item in source)
            {
                FindGroup(groups, keySelector(item), comparer).Add(elementSelector(item));
            }
            return groups;
        }

        private static Grouping<TKey, TElement> FindGroup<TKey, TElement>(List<Grouping<TKey, TElement>> groups, TKey key, IEqualityComparer<TKey> comparer)
        {
            int hash = comparer == null ? Comparers.Hash(key) : comparer.GetHashCode(key);
            for (int i = 0; i < groups.Count; i++)
            {
                Grouping<TKey, TElement> group = groups[i];
                if (group.__hash == hash && (comparer == null ? Comparers.Equal(group.Key, key) : comparer.Equals(group.Key, key)))
                {
                    return group;
                }
            }
            Grouping<TKey, TElement> made = new Grouping<TKey, TElement>(key, hash);
            groups.Add(made);
            return made;
        }

        // ------------------------------------------------------ generation

        public static IEnumerable<int> Range(int start, int count)
        {
            if (count < 0) { throw new ArgumentOutOfRangeException("count"); }
            for (int i = 0; i < count; i++)
            {
                yield return start + i;
            }
        }

        public static IEnumerable<TResult> Repeat<TResult>(TResult element, int count)
        {
            if (count < 0) { throw new ArgumentOutOfRangeException("count"); }
            for (int i = 0; i < count; i++)
            {
                yield return element;
            }
        }

        public static IEnumerable<TResult> Empty<TResult>()
        {
            return new ArrayEnumerable<TResult>(new TResult[0]);
        }

        // ------------------------------------------------------ quantifiers

        public static bool Any<TSource>(this IEnumerable<TSource> source)
        {
            IEnumerator<TSource> enumerator = source.GetEnumerator();
            try
            {
                return enumerator.MoveNext();
            }
            finally
            {
                enumerator.Dispose();
            }
        }

        public static bool Any<TSource>(this IEnumerable<TSource> source, Func<TSource, bool> predicate)
        {
            foreach (TSource item in source)
            {
                if (predicate(item)) { return true; }
            }
            return false;
        }

        public static bool All<TSource>(this IEnumerable<TSource> source, Func<TSource, bool> predicate)
        {
            foreach (TSource item in source)
            {
                if (!predicate(item)) { return false; }
            }
            return true;
        }

        public static bool Contains<TSource>(this IEnumerable<TSource> source, TSource value)
        {
            foreach (TSource item in source)
            {
                if (Comparers.Equal(item, value)) { return true; }
            }
            return false;
        }

        public static bool Contains<TSource>(this IEnumerable<TSource> source, TSource value, IEqualityComparer<TSource> comparer)
        {
            if (comparer == null) { return Contains(source, value); }
            foreach (TSource item in source)
            {
                if (comparer.Equals(item, value)) { return true; }
            }
            return false;
        }

        public static bool SequenceEqual<TSource>(this IEnumerable<TSource> first, IEnumerable<TSource> second)
        {
            return SequenceEqual(first, second, null);
        }

        public static bool SequenceEqual<TSource>(this IEnumerable<TSource> first, IEnumerable<TSource> second, IEqualityComparer<TSource> comparer)
        {
            IEnumerator<TSource> left = first.GetEnumerator();
            try
            {
                IEnumerator<TSource> right = second.GetEnumerator();
                try
                {
                    while (left.MoveNext())
                    {
                        if (!right.MoveNext()) { return false; }
                        bool same = comparer == null
                            ? Comparers.Equal(left.Current, right.Current)
                            : comparer.Equals(left.Current, right.Current);
                        if (!same) { return false; }
                    }
                    return !right.MoveNext();
                }
                finally
                {
                    right.Dispose();
                }
            }
            finally
            {
                left.Dispose();
            }
        }

        // -------------------------------------------------------- elements

        public static TSource First<TSource>(this IEnumerable<TSource> source)
        {
            foreach (TSource item in source)
            {
                return item;
            }
            throw new InvalidOperationException("Sequence contains no elements");
        }

        public static TSource First<TSource>(this IEnumerable<TSource> source, Func<TSource, bool> predicate)
        {
            foreach (TSource item in source)
            {
                if (predicate(item)) { return item; }
            }
            throw new InvalidOperationException("Sequence contains no matching element");
        }

        public static TSource FirstOrDefault<TSource>(this IEnumerable<TSource> source)
        {
            foreach (TSource item in source)
            {
                return item;
            }
            return default(TSource);
        }

        public static TSource FirstOrDefault<TSource>(this IEnumerable<TSource> source, Func<TSource, bool> predicate)
        {
            foreach (TSource item in source)
            {
                if (predicate(item)) { return item; }
            }
            return default(TSource);
        }

        public static TSource Last<TSource>(this IEnumerable<TSource> source)
        {
            bool found = false;
            TSource last = default(TSource);
            foreach (TSource item in source)
            {
                last = item;
                found = true;
            }
            if (!found) { throw new InvalidOperationException("Sequence contains no elements"); }
            return last;
        }

        public static TSource Last<TSource>(this IEnumerable<TSource> source, Func<TSource, bool> predicate)
        {
            bool found = false;
            TSource last = default(TSource);
            foreach (TSource item in source)
            {
                if (predicate(item))
                {
                    last = item;
                    found = true;
                }
            }
            if (!found) { throw new InvalidOperationException("Sequence contains no matching element"); }
            return last;
        }

        public static TSource LastOrDefault<TSource>(this IEnumerable<TSource> source)
        {
            TSource last = default(TSource);
            foreach (TSource item in source)
            {
                last = item;
            }
            return last;
        }

        public static TSource LastOrDefault<TSource>(this IEnumerable<TSource> source, Func<TSource, bool> predicate)
        {
            TSource last = default(TSource);
            foreach (TSource item in source)
            {
                if (predicate(item)) { last = item; }
            }
            return last;
        }

        public static TSource Single<TSource>(this IEnumerable<TSource> source)
        {
            bool found = false;
            TSource single = default(TSource);
            foreach (TSource item in source)
            {
                if (found) { throw new InvalidOperationException("Sequence contains more than one element"); }
                single = item;
                found = true;
            }
            if (!found) { throw new InvalidOperationException("Sequence contains no elements"); }
            return single;
        }

        public static TSource Single<TSource>(this IEnumerable<TSource> source, Func<TSource, bool> predicate)
        {
            bool found = false;
            TSource single = default(TSource);
            foreach (TSource item in source)
            {
                if (!predicate(item)) { continue; }
                if (found) { throw new InvalidOperationException("Sequence contains more than one matching element"); }
                single = item;
                found = true;
            }
            if (!found) { throw new InvalidOperationException("Sequence contains no matching element"); }
            return single;
        }

        public static TSource SingleOrDefault<TSource>(this IEnumerable<TSource> source)
        {
            bool found = false;
            TSource single = default(TSource);
            foreach (TSource item in source)
            {
                if (found) { throw new InvalidOperationException("Sequence contains more than one element"); }
                single = item;
                found = true;
            }
            return single;
        }

        public static TSource SingleOrDefault<TSource>(this IEnumerable<TSource> source, Func<TSource, bool> predicate)
        {
            bool found = false;
            TSource single = default(TSource);
            foreach (TSource item in source)
            {
                if (!predicate(item)) { continue; }
                if (found) { throw new InvalidOperationException("Sequence contains more than one matching element"); }
                single = item;
                found = true;
            }
            return single;
        }

        public static TSource ElementAt<TSource>(this IEnumerable<TSource> source, int index)
        {
            if (index >= 0)
            {
                foreach (TSource item in source)
                {
                    if (index == 0) { return item; }
                    index--;
                }
            }
            throw new ArgumentOutOfRangeException("index");
        }

        public static TSource ElementAtOrDefault<TSource>(this IEnumerable<TSource> source, int index)
        {
            if (index >= 0)
            {
                foreach (TSource item in source)
                {
                    if (index == 0) { return item; }
                    index--;
                }
            }
            return default(TSource);
        }

        // ------------------------------------------------------- counting

        public static int Count<TSource>(this IEnumerable<TSource> source)
        {
            int count = 0;
            foreach (TSource item in source) { count++; }
            return count;
        }

        public static int Count<TSource>(this IEnumerable<TSource> source, Func<TSource, bool> predicate)
        {
            int count = 0;
            foreach (TSource item in source)
            {
                if (predicate(item)) { count++; }
            }
            return count;
        }

        public static long LongCount<TSource>(this IEnumerable<TSource> source)
        {
            long count = 0;
            foreach (TSource item in source) { count++; }
            return count;
        }

        public static long LongCount<TSource>(this IEnumerable<TSource> source, Func<TSource, bool> predicate)
        {
            long count = 0;
            foreach (TSource item in source)
            {
                if (predicate(item)) { count++; }
            }
            return count;
        }

        // ----------------------------------------------------- aggregation

        public static TSource Aggregate<TSource>(this IEnumerable<TSource> source, Func<TSource, TSource, TSource> func)
        {
            bool found = false;
            TSource result = default(TSource);
            foreach (TSource item in source)
            {
                if (found) { result = func(result, item); }
                else { result = item; found = true; }
            }
            if (!found) { throw new InvalidOperationException("Sequence contains no elements"); }
            return result;
        }

        public static TAccumulate Aggregate<TSource, TAccumulate>(this IEnumerable<TSource> source, TAccumulate seed, Func<TAccumulate, TSource, TAccumulate> func)
        {
            TAccumulate result = seed;
            foreach (TSource item in source)
            {
                result = func(result, item);
            }
            return result;
        }

        public static TResult Aggregate<TSource, TAccumulate, TResult>(this IEnumerable<TSource> source, TAccumulate seed, Func<TAccumulate, TSource, TAccumulate> func, Func<TAccumulate, TResult> resultSelector)
        {
            TAccumulate result = seed;
            foreach (TSource item in source)
            {
                result = func(result, item);
            }
            return resultSelector(result);
        }

        // ------------------------------------------------------------- Sum

        public static int Sum(this IEnumerable<int> source)
        {
            int total = 0;
            foreach (int item in source) { total += item; }
            return total;
        }

        public static long Sum(this IEnumerable<long> source)
        {
            long total = 0;
            foreach (long item in source) { total += item; }
            return total;
        }

        public static float Sum(this IEnumerable<float> source)
        {
            double total = 0;
            foreach (float item in source) { total += item; }
            return (float)total;
        }

        public static double Sum(this IEnumerable<double> source)
        {
            double total = 0;
            foreach (double item in source) { total += item; }
            return total;
        }

        public static int Sum<TSource>(this IEnumerable<TSource> source, Func<TSource, int> selector)
        {
            int total = 0;
            foreach (TSource item in source) { total += selector(item); }
            return total;
        }

        public static long Sum<TSource>(this IEnumerable<TSource> source, Func<TSource, long> selector)
        {
            long total = 0;
            foreach (TSource item in source) { total += selector(item); }
            return total;
        }

        public static float Sum<TSource>(this IEnumerable<TSource> source, Func<TSource, float> selector)
        {
            double total = 0;
            foreach (TSource item in source) { total += selector(item); }
            return (float)total;
        }

        public static double Sum<TSource>(this IEnumerable<TSource> source, Func<TSource, double> selector)
        {
            double total = 0;
            foreach (TSource item in source) { total += selector(item); }
            return total;
        }

        // --------------------------------------------------------- Average

        public static double Average(this IEnumerable<int> source)
        {
            long total = 0;
            long count = 0;
            foreach (int item in source) { total += item; count++; }
            if (count == 0) { throw new InvalidOperationException("Sequence contains no elements"); }
            return (double)total / count;
        }

        public static double Average(this IEnumerable<long> source)
        {
            long total = 0;
            long count = 0;
            foreach (long item in source) { total += item; count++; }
            if (count == 0) { throw new InvalidOperationException("Sequence contains no elements"); }
            return (double)total / count;
        }

        public static float Average(this IEnumerable<float> source)
        {
            double total = 0;
            long count = 0;
            foreach (float item in source) { total += item; count++; }
            if (count == 0) { throw new InvalidOperationException("Sequence contains no elements"); }
            return (float)(total / count);
        }

        public static double Average(this IEnumerable<double> source)
        {
            double total = 0;
            long count = 0;
            foreach (double item in source) { total += item; count++; }
            if (count == 0) { throw new InvalidOperationException("Sequence contains no elements"); }
            return total / count;
        }

        public static double Average<TSource>(this IEnumerable<TSource> source, Func<TSource, int> selector)
        {
            long total = 0;
            long count = 0;
            foreach (TSource item in source) { total += selector(item); count++; }
            if (count == 0) { throw new InvalidOperationException("Sequence contains no elements"); }
            return (double)total / count;
        }

        public static double Average<TSource>(this IEnumerable<TSource> source, Func<TSource, long> selector)
        {
            long total = 0;
            long count = 0;
            foreach (TSource item in source) { total += selector(item); count++; }
            if (count == 0) { throw new InvalidOperationException("Sequence contains no elements"); }
            return (double)total / count;
        }

        public static float Average<TSource>(this IEnumerable<TSource> source, Func<TSource, float> selector)
        {
            double total = 0;
            long count = 0;
            foreach (TSource item in source) { total += selector(item); count++; }
            if (count == 0) { throw new InvalidOperationException("Sequence contains no elements"); }
            return (float)(total / count);
        }

        public static double Average<TSource>(this IEnumerable<TSource> source, Func<TSource, double> selector)
        {
            double total = 0;
            long count = 0;
            foreach (TSource item in source) { total += selector(item); count++; }
            if (count == 0) { throw new InvalidOperationException("Sequence contains no elements"); }
            return total / count;
        }

        // --------------------------------------------------------- Min/Max

        public static int Min(this IEnumerable<int> source)
        {
            bool found = false;
            int value = 0;
            foreach (int item in source)
            {
                if (!found || item < value) { value = item; found = true; }
            }
            if (!found) { throw new InvalidOperationException("Sequence contains no elements"); }
            return value;
        }

        public static long Min(this IEnumerable<long> source)
        {
            bool found = false;
            long value = 0;
            foreach (long item in source)
            {
                if (!found || item < value) { value = item; found = true; }
            }
            if (!found) { throw new InvalidOperationException("Sequence contains no elements"); }
            return value;
        }

        public static float Min(this IEnumerable<float> source)
        {
            bool found = false;
            float value = 0;
            foreach (float item in source)
            {
                // NaN is the smallest, as .NET orders it
                if (!found || item < value || float.IsNaN(item)) { value = item; found = true; }
            }
            if (!found) { throw new InvalidOperationException("Sequence contains no elements"); }
            return value;
        }

        public static double Min(this IEnumerable<double> source)
        {
            bool found = false;
            double value = 0;
            foreach (double item in source)
            {
                if (!found || item < value || double.IsNaN(item)) { value = item; found = true; }
            }
            if (!found) { throw new InvalidOperationException("Sequence contains no elements"); }
            return value;
        }

        public static int Max(this IEnumerable<int> source)
        {
            bool found = false;
            int value = 0;
            foreach (int item in source)
            {
                if (!found || item > value) { value = item; found = true; }
            }
            if (!found) { throw new InvalidOperationException("Sequence contains no elements"); }
            return value;
        }

        public static long Max(this IEnumerable<long> source)
        {
            bool found = false;
            long value = 0;
            foreach (long item in source)
            {
                if (!found || item > value) { value = item; found = true; }
            }
            if (!found) { throw new InvalidOperationException("Sequence contains no elements"); }
            return value;
        }

        public static float Max(this IEnumerable<float> source)
        {
            bool found = false;
            float value = 0;
            foreach (float item in source)
            {
                if (!found || item > value || float.IsNaN(value)) { value = item; found = true; }
            }
            if (!found) { throw new InvalidOperationException("Sequence contains no elements"); }
            return value;
        }

        public static double Max(this IEnumerable<double> source)
        {
            bool found = false;
            double value = 0;
            foreach (double item in source)
            {
                if (!found || item > value || double.IsNaN(value)) { value = item; found = true; }
            }
            if (!found) { throw new InvalidOperationException("Sequence contains no elements"); }
            return value;
        }

        public static int Min<TSource>(this IEnumerable<TSource> source, Func<TSource, int> selector)
        {
            return Min(Select(source, selector));
        }

        public static long Min<TSource>(this IEnumerable<TSource> source, Func<TSource, long> selector)
        {
            return Min(Select(source, selector));
        }

        public static float Min<TSource>(this IEnumerable<TSource> source, Func<TSource, float> selector)
        {
            return Min(Select(source, selector));
        }

        public static double Min<TSource>(this IEnumerable<TSource> source, Func<TSource, double> selector)
        {
            return Min(Select(source, selector));
        }

        public static int Max<TSource>(this IEnumerable<TSource> source, Func<TSource, int> selector)
        {
            return Max(Select(source, selector));
        }

        public static long Max<TSource>(this IEnumerable<TSource> source, Func<TSource, long> selector)
        {
            return Max(Select(source, selector));
        }

        public static float Max<TSource>(this IEnumerable<TSource> source, Func<TSource, float> selector)
        {
            return Max(Select(source, selector));
        }

        public static double Max<TSource>(this IEnumerable<TSource> source, Func<TSource, double> selector)
        {
            return Max(Select(source, selector));
        }

        /// The least element by its own ordering (`Comparer<T>.Default`).
        /// Nulls are skipped; a sequence of a reference type with nothing
        /// else gives null, one of a value type throws — as .NET's does.
        public static TSource Min<TSource>(this IEnumerable<TSource> source)
        {
            bool found = false;
            TSource value = default(TSource);
            foreach (TSource item in source)
            {
                object boxed = item;
                if (boxed == null) { continue; }
                if (!found || Comparers.Compare(item, value) < 0) { value = item; found = true; }
            }
            if (!found)
            {
                object none = value;
                if (none != null) { throw new InvalidOperationException("Sequence contains no elements"); }
            }
            return value;
        }

        public static TSource Max<TSource>(this IEnumerable<TSource> source)
        {
            bool found = false;
            TSource value = default(TSource);
            foreach (TSource item in source)
            {
                object boxed = item;
                if (boxed == null) { continue; }
                if (!found || Comparers.Compare(item, value) > 0) { value = item; found = true; }
            }
            if (!found)
            {
                object none = value;
                if (none != null) { throw new InvalidOperationException("Sequence contains no elements"); }
            }
            return value;
        }

        public static TResult Min<TSource, TResult>(this IEnumerable<TSource> source, Func<TSource, TResult> selector)
        {
            return Min(Select(source, selector));
        }

        public static TResult Max<TSource, TResult>(this IEnumerable<TSource> source, Func<TSource, TResult> selector)
        {
            return Max(Select(source, selector));
        }

        // ------------------------------------------------------ conversion

        public static TSource[] ToArray<TSource>(this IEnumerable<TSource> source)
        {
            List<TSource> list = new List<TSource>();
            foreach (TSource item in source) { list.Add(item); }
            return list.ToArray();
        }

        public static List<TSource> ToList<TSource>(this IEnumerable<TSource> source)
        {
            List<TSource> list = new List<TSource>();
            foreach (TSource item in source) { list.Add(item); }
            return list;
        }

        public static HashSet<TSource> ToHashSet<TSource>(this IEnumerable<TSource> source)
        {
            return new HashSet<TSource>(source);
        }

        public static HashSet<TSource> ToHashSet<TSource>(this IEnumerable<TSource> source, IEqualityComparer<TSource> comparer)
        {
            return new HashSet<TSource>(source, comparer);
        }

        public static Dictionary<TKey, TSource> ToDictionary<TSource, TKey>(this IEnumerable<TSource> source, Func<TSource, TKey> keySelector)
        {
            return ToDictionary(source, keySelector, (IEqualityComparer<TKey>)null);
        }

        public static Dictionary<TKey, TSource> ToDictionary<TSource, TKey>(this IEnumerable<TSource> source, Func<TSource, TKey> keySelector, IEqualityComparer<TKey> comparer)
        {
            Dictionary<TKey, TSource> dictionary = new Dictionary<TKey, TSource>(comparer);
            foreach (TSource item in source)
            {
                dictionary.Add(keySelector(item), item);
            }
            return dictionary;
        }

        public static Dictionary<TKey, TElement> ToDictionary<TSource, TKey, TElement>(this IEnumerable<TSource> source, Func<TSource, TKey> keySelector, Func<TSource, TElement> elementSelector)
        {
            return ToDictionary(source, keySelector, elementSelector, null);
        }

        public static Dictionary<TKey, TElement> ToDictionary<TSource, TKey, TElement>(this IEnumerable<TSource> source, Func<TSource, TKey> keySelector, Func<TSource, TElement> elementSelector, IEqualityComparer<TKey> comparer)
        {
            Dictionary<TKey, TElement> dictionary = new Dictionary<TKey, TElement>(comparer);
            foreach (TSource item in source)
            {
                dictionary.Add(keySelector(item), elementSelector(item));
            }
            return dictionary;
        }

        /// The sequence itself, typed as one: `list.AsEnumerable().Select(...)`.
        public static IEnumerable<TSource> AsEnumerable<TSource>(this IEnumerable<TSource> source)
        {
            return source;
        }
    }
}

namespace MenSharp.Internal
{
    /// One key of an `OrderBy`/`ThenBy` chain: computes the keys of the
    /// elements once and orders positions by them.
    public abstract class SortLevel<TElement>
    {
        public abstract Comparison<int> Prepare(TElement[] items);
    }

    public sealed class KeyLevel<TElement, TKey> : SortLevel<TElement>
    {
        private readonly Func<TElement, TKey> keySelector;
        private readonly IComparer<TKey> comparer;   // null: the key's own order
        private readonly bool descending;

        public KeyLevel(Func<TElement, TKey> keySelector, IComparer<TKey> comparer, bool descending)
        {
            this.keySelector = keySelector;
            this.comparer = comparer;
            this.descending = descending;
        }

        public override Comparison<int> Prepare(TElement[] items)
        {
            TKey[] keys = new TKey[items.Length];
            for (int i = 0; i < items.Length; i++)
            {
                keys[i] = keySelector(items[i]);
            }
            if (comparer == null)
            {
                if (descending)
                {
                    return (i, j) => Comparers.Compare(keys[j], keys[i]);
                }
                return (i, j) => Comparers.Compare(keys[i], keys[j]);
            }
            IComparer<TKey> order = comparer;
            if (descending)
            {
                return (i, j) => order.Compare(keys[j], keys[i]);
            }
            return (i, j) => order.Compare(keys[i], keys[j]);
        }
    }

    /// What `OrderBy` returns: the source and the keys to sort it by, sorted
    /// afresh (stably, as .NET does) each time it is enumerated.
    public sealed class OrderedEnumerable<TElement> : System.Linq.IOrderedEnumerable<TElement>
    {
        private readonly System.Collections.Generic.IEnumerable<TElement> source;
        private readonly SortLevel<TElement>[] levels;

        public OrderedEnumerable(System.Collections.Generic.IEnumerable<TElement> source, SortLevel<TElement> level)
        {
            this.source = source;
            levels = new SortLevel<TElement>[] { level };
        }

        private OrderedEnumerable(System.Collections.Generic.IEnumerable<TElement> source, SortLevel<TElement>[] levels)
        {
            this.source = source;
            this.levels = levels;
        }

        public System.Linq.IOrderedEnumerable<TElement> __Then(SortLevel<TElement> level)
        {
            SortLevel<TElement>[] more = new SortLevel<TElement>[levels.Length + 1];
            for (int i = 0; i < levels.Length; i++)
            {
                more[i] = levels[i];
            }
            more[levels.Length] = level;
            return new OrderedEnumerable<TElement>(source, more);
        }

        public System.Collections.Generic.IEnumerator<TElement> GetEnumerator()
        {
            return new ArrayEnumerator<TElement>(Sorted());
        }

        private TElement[] Sorted()
        {
            TElement[] items = System.Linq.Enumerable.ToArray(source);
            int count = items.Length;
            Comparison<int>[] compare = new Comparison<int>[levels.Length];
            for (int level = 0; level < levels.Length; level++)
            {
                compare[level] = levels[level].Prepare(items);
            }

            // a bottom-up merge sort of the positions: stable, and without
            // the recursion .NET's quicksort would need
            int[] order = new int[count];
            for (int i = 0; i < count; i++) { order[i] = i; }
            int[] scratch = new int[count];
            for (int width = 1; width < count; width *= 2)
            {
                for (int low = 0; low < count; low += 2 * width)
                {
                    int middle = low + width;
                    if (middle > count) { middle = count; }
                    int high = low + 2 * width;
                    if (high > count) { high = count; }
                    int left = low;
                    int right = middle;
                    int output = low;
                    while (left < middle && right < high)
                    {
                        if (CompareAt(compare, order[left], order[right]) <= 0)
                        {
                            scratch[output] = order[left];
                            left++;
                        }
                        else
                        {
                            scratch[output] = order[right];
                            right++;
                        }
                        output++;
                    }
                    while (left < middle) { scratch[output] = order[left]; left++; output++; }
                    while (right < high) { scratch[output] = order[right]; right++; output++; }
                }
                int[] swap = order;
                order = scratch;
                scratch = swap;
            }

            TElement[] sorted = new TElement[count];
            for (int i = 0; i < count; i++)
            {
                sorted[i] = items[order[i]];
            }
            return sorted;
        }

        private static int CompareAt(Comparison<int>[] compare, int a, int b)
        {
            for (int level = 0; level < compare.Length; level++)
            {
                int result = compare[level](a, b);
                if (result != 0) { return result; }
            }
            return 0;
        }
    }

    /// What `GroupBy` yields.
    public sealed class Grouping<TKey, TElement> : System.Linq.IGrouping<TKey, TElement>
    {
        private readonly TKey key;
        internal readonly int __hash;
        private readonly List<TElement> elements;

        public Grouping(TKey key, int hash)
        {
            this.key = key;
            __hash = hash;
            elements = new List<TElement>();
        }

        public TKey Key
        {
            get { return key; }
        }

        public int Count
        {
            get { return elements.Count; }
        }

        internal void Add(TElement element)
        {
            elements.Add(element);
        }

        public System.Collections.Generic.IEnumerator<TElement> GetEnumerator()
        {
            return elements.GetEnumerator();
        }
    }
}

// MenSharp mini-corlib: the sorts behind `List<T>.Sort` and LINQ's
// `OrderBy` family.
//
// Udon pays for every instruction, so a sort written out in C# is slow
// however few comparisons it makes; the engine's own `Array.Sort` is one
// extern for the whole array. That is used whenever the order is the
// default one and `Comparers.Native<T>()` says the engine orders `T` the
// same way (numbers, strings, chars, source enums, ...). It is not stable,
// so unless ties are indistinguishable (`Comparers.TiesIdentical<T>()`),
// each run of equal elements is put back in its original order afterwards
// — by a second native sort, of their positions. Everything else (a
// `Comparison<T>`, an `IComparer<T>`, a class implementing `IComparable<T>`)
// is a bottom-up merge sort: stable, O(n log n) comparisons where the
// insertion sort before it made O(n²), and free of the recursion .NET's
// introsort would need.

using System;
using System.Collections.Generic;

namespace MenSharp.Internal
{
    public static class Sorting
    {
        /// Sorts `items[index, index + count)` stably by `Comparer<T>.Default`.
        public static void Sort<T>(T[] items, int index, int count)
        {
            if (count < 2) { return; }
            if (!Comparers.Native<T>())
            {
                Merge(items, index, count, null, new DefaultComparer<T>());
                return;
            }
            if (Comparers.TiesIdentical<T>())
            {
                System.Array.Sort((System.Array)items, index, count);
                return;
            }
            T[] keys = new T[count];
            System.Array.Copy(items, index, keys, 0, count);
            int[] positions = Positions(count);
            System.Array.Sort((System.Array)keys, (System.Array)positions);
            // each run of equal keys: its elements in their original order
            int start = 0;
            for (int i = 1; i <= count; i++)
            {
                if (i < count && Comparers.Compare(keys[start], keys[i]) == 0) { continue; }
                if (i - start > 1)
                {
                    System.Array.Sort((System.Array)positions, start, i - start);
                    for (int j = start; j < i; j++)
                    {
                        keys[j] = items[index + positions[j]];
                    }
                }
                start = i;
            }
            System.Array.Copy(keys, 0, items, index, count);
        }

        /// Sorts `items[index, index + count)` stably by `comparison`.
        public static void Sort<T>(T[] items, int index, int count, Comparison<T> comparison)
        {
            if (count < 2) { return; }
            Merge(items, index, count, comparison, null);
        }

        /// Sorts `items[index, index + count)` stably by `comparer`; `null`
        /// (or `Comparer<T>.Default`) is the default order, as in .NET.
        public static void Sort<T>(T[] items, int index, int count, IComparer<T> comparer)
        {
            if (comparer == null || comparer is DefaultComparer<T>)
            {
                Sort(items, index, count);
                return;
            }
            if (count < 2) { return; }
            Merge(items, index, count, null, comparer);
        }

        /// `order` (positions into `keys`; null for 0, 1, 2, ...) reordered
        /// stably by those keys, in their default order or its reverse, with
        /// the engine's sort; for a key `Comparers.Native` accepts.
        public static int[] Order<TKey>(TKey[] keys, int[] order, bool descending)
        {
            int count = keys.Length;
            TKey[] sorted = new TKey[count];
            if (order == null)
            {
                System.Array.Copy(keys, sorted, count);
            }
            else
            {
                for (int i = 0; i < count; i++)
                {
                    sorted[i] = keys[order[i]];
                }
            }
            int[] ranks = Positions(count);
            System.Array.Sort((System.Array)sorted, (System.Array)ranks);
            // ascending from the identity, the ranks are the answer
            int[] result = order == null && !descending ? ranks : new int[count];
            // runs of equal keys, from the last: each one's ranks put back in
            // ascending order, and the runs themselves reversed for a
            // descending sort
            int output = 0;
            int end = count;
            for (int start = count - 1; start >= 0; start--)
            {
                if (start > 0 && Comparers.Compare(sorted[start - 1], sorted[start]) == 0) { continue; }
                if (end - start > 1)
                {
                    System.Array.Sort((System.Array)ranks, start, end - start);
                }
                if (result != ranks)
                {
                    int at = descending ? output : start;
                    for (int i = start; i < end; i++)
                    {
                        result[at] = order == null ? ranks[i] : order[ranks[i]];
                        at++;
                    }
                }
                output += end - start;
                end = start;
            }
            return result;
        }

        private static int[] Positions(int count)
        {
            int[] positions = new int[count];
            for (int i = 0; i < count; i++) { positions[i] = i; }
            return positions;
        }

        // By `comparison` when it is not null, else by `comparer`: runs of 8
        // by insertion (fewer moves than merging pairs), then merged
        // bottom-up between two buffers. A pair of runs already in order is
        // copied whole, and so is the rest of a run the other ran out
        // before — both one extern.
        private static void Merge<T>(T[] items, int index, int count, Comparison<T> comparison, IComparer<T> comparer)
        {
            T[] from = new T[count];
            System.Array.Copy(items, index, from, 0, count);
            for (int low = 0; low < count; low += 8)
            {
                int high = low + 8;
                if (high > count) { high = count; }
                for (int i = low + 1; i < high; i++)
                {
                    T key = from[i];
                    int j = i - 1;
                    while (j >= low && (comparison != null ? comparison(from[j], key) : comparer.Compare(from[j], key)) > 0)
                    {
                        from[j + 1] = from[j];
                        j--;
                    }
                    from[j + 1] = key;
                }
            }

            T[] to = new T[count];
            for (int width = 8; width < count; width *= 2)
            {
                for (int low = 0; low < count; low += 2 * width)
                {
                    int middle = low + width;
                    if (middle >= count)
                    {
                        System.Array.Copy(from, low, to, low, count - low);
                        break;
                    }
                    int high = middle + width;
                    if (high > count) { high = count; }
                    if ((comparison != null ? comparison(from[middle - 1], from[middle]) : comparer.Compare(from[middle - 1], from[middle])) <= 0)
                    {
                        System.Array.Copy(from, low, to, low, high - low);
                        continue;
                    }
                    int left = low;
                    int right = middle;
                    int output = low;
                    while (left < middle && right < high)
                    {
                        // ties from the left: stable
                        if ((comparison != null ? comparison(from[left], from[right]) : comparer.Compare(from[left], from[right])) <= 0)
                        {
                            to[output] = from[left];
                            left++;
                        }
                        else
                        {
                            to[output] = from[right];
                            right++;
                        }
                        output++;
                    }
                    if (left < middle)
                    {
                        System.Array.Copy(from, left, to, output, middle - left);
                    }
                    else
                    {
                        System.Array.Copy(from, right, to, output, high - right);
                    }
                }
                T[] swap = from;
                from = to;
                to = swap;
            }
            System.Array.Copy(from, 0, items, index, count);
        }
    }
}

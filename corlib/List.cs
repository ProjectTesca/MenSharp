// MenSharp mini-corlib: System.Collections.Generic.List<T>.
//
// Udon does not whitelist the real List<T>, so M# ships this source port,
// compiled together with user code (and monomorphized per element type) like
// any other class. The algorithm mirrors dotnet/runtime's List<T> (MIT); the
// storage is a plain T[] that grows by doubling, and every operation below
// bottoms out in whitelisted externs (array ctor/Get/Set, Array.Copy).
//
// `foreach` binds to the enumerator pattern below (no interface, no
// dispatch); `IEnumerable<T>` is implemented explicitly beside it, exactly
// as .NET's List<T> does, so a list can also be passed where a sequence is
// wanted. `Sort()`, `Contains` and `IndexOf` order and compare through
// `MenSharp.Internal.Comparers` (see Comparers.cs). The indexer and
// RemoveAt throw ArgumentOutOfRangeException as .NET's do.

using System;

namespace System.Collections.Generic
{
    public class List<T> : IEnumerable<T>
    {
        private T[] items;
        private int size;

        public List()
        {
            items = new T[4];
            size = 0;
        }

        public List(int capacity)
        {
            if (capacity < 0) { throw new ArgumentOutOfRangeException("capacity"); }
            items = new T[capacity < 4 ? 4 : capacity];
            size = 0;
        }

        public List(IEnumerable<T> collection)
        {
            items = new T[4];
            size = 0;
            AddRange(collection);
        }

        public int Count => size;

        public void Add(T item)
        {
            if (size == items.Length)
            {
                var grown = new T[size * 2];
                System.Array.Copy(items, grown, size);
                items = grown;
            }
            items[size] = item;
            size++;
        }

        public T this[int index]
        {
            get
            {
                if (index < 0 || index >= size) { throw new ArgumentOutOfRangeException("index"); }
                return items[index];
            }
            set
            {
                if (index < 0 || index >= size) { throw new ArgumentOutOfRangeException("index"); }
                items[index] = value;
            }
        }

        public void AddRange(IEnumerable<T> collection)
        {
            foreach (T item in collection)
            {
                Add(item);
            }
        }

        public T[] ToArray()
        {
            T[] result = new T[size];
            System.Array.Copy(items, result, size);
            return result;
        }

        public int IndexOf(T item)
        {
            for (int i = 0; i < size; i++)
            {
                if (MenSharp.Internal.Comparers.Equal(items[i], item)) { return i; }
            }
            return -1;
        }

        public bool Contains(T item)
        {
            return IndexOf(item) >= 0;
        }

        /// Orders the elements by their own ordering (`Comparer<T>.Default`):
        /// numbers, strings, chars, ... and any class implementing
        /// `IComparable<T>`. Stable, unlike .NET's.
        public void Sort()
        {
            for (int i = 1; i < size; i++)
            {
                T key = items[i];
                int j = i - 1;
                while (j >= 0 && MenSharp.Internal.Comparers.Compare(items[j], key) > 0)
                {
                    items[j + 1] = items[j];
                    j--;
                }
                items[j + 1] = key;
            }
        }

        public void Clear()
        {
            size = 0;
        }

        public void RemoveAt(int index)
        {
            if (index < 0 || index >= size) { throw new ArgumentOutOfRangeException("index"); }
            int i = index;
            while (i < size - 1)
            {
                items[i] = items[i + 1];
                i++;
            }
            size--;
        }

        // ---- the delegate-taking members: `Predicate<T>`, `Action<T>`,
        // `Comparison<T>`, `Converter<T, TOut>` are delegates of the real
        // corlib, called like any M# delegate

        public T Find(Predicate<T> predicate)
        {
            for (int i = 0; i < size; i++)
            {
                if (predicate(items[i])) { return items[i]; }
            }
            return default(T);
        }

        public int FindIndex(Predicate<T> predicate)
        {
            for (int i = 0; i < size; i++)
            {
                if (predicate(items[i])) { return i; }
            }
            return -1;
        }

        public bool Exists(Predicate<T> predicate)
        {
            return FindIndex(predicate) >= 0;
        }

        public bool TrueForAll(Predicate<T> predicate)
        {
            for (int i = 0; i < size; i++)
            {
                if (!predicate(items[i])) { return false; }
            }
            return true;
        }

        public void ForEach(Action<T> action)
        {
            for (int i = 0; i < size; i++)
            {
                action(items[i]);
            }
        }

        public int RemoveAll(Predicate<T> predicate)
        {
            int kept = 0;
            for (int i = 0; i < size; i++)
            {
                if (!predicate(items[i]))
                {
                    items[kept] = items[i];
                    kept++;
                }
            }
            int removed = size - kept;
            size = kept;
            return removed;
        }

        // A stable insertion sort: fine for the list sizes a world script
        // keeps, and free of the recursion .NET's introsort would need.
        public void Sort(Comparison<T> comparison)
        {
            for (int i = 1; i < size; i++)
            {
                T key = items[i];
                int j = i - 1;
                while (j >= 0 && comparison(items[j], key) > 0)
                {
                    items[j + 1] = items[j];
                    j--;
                }
                items[j + 1] = key;
            }
        }

        public List<TOut> ConvertAll<TOut>(Converter<T, TOut> converter)
        {
            var result = new List<TOut>();
            for (int i = 0; i < size; i++)
            {
                result.Add(converter(items[i]));
            }
            return result;
        }

        // What `foreach` calls. The real one is a nested struct,
        // `List<T>.Enumerator`; until M# has value types (and nested types
        // that see the outer `T`) it is a class beside the list, so a
        // `foreach` costs one small allocation — the semantics (one pass over
        // the elements present when the loop started) are the same.
        public ListEnumerator<T> GetEnumerator()
        {
            return new ListEnumerator<T>(items, size);
        }

        // the same walk, reached through the interface: `foreach` never
        // needs this one, so a plain loop over a list costs no dispatch
        IEnumerator<T> IEnumerable<T>.GetEnumerator()
        {
            return new ListEnumerator<T>(items, size);
        }
    }

    public sealed class ListEnumerator<T> : IEnumerator<T>
    {
        private T[] items;
        private int count;
        private int index;
        private T current;

        public ListEnumerator(T[] items, int count)
        {
            this.items = items;
            this.count = count;
            index = 0;
        }

        public bool MoveNext()
        {
            if (index < count)
            {
                current = items[index];
                index++;
                return true;
            }
            return false;
        }

        public T Current => current;

        public void Dispose() { }
    }
}

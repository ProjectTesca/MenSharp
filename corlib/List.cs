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
// wanted. `Sort()`, `BinarySearch`, `Contains` and `IndexOf` order and
// compare through `MenSharp.Internal.Comparers` (see Comparers.cs). The
// indexer, `Insert`, `RemoveAt` and the range members throw
// ArgumentOutOfRangeException as .NET's do.
//
// Not here: the overloads taking an `IComparer<T>` / `IEqualityComparer<T>`
// (M# has no comparer objects; `Sort(Comparison<T>)` covers custom orders)
// and `AsReadOnly` (no `ReadOnlyCollection<T>`).

using System;

namespace System.Collections.Generic
{
    public class List<T> : IEnumerable<T>
    {
        private T[] items;
        private int size;

        public List()
        {
            items = new T[0];
            size = 0;
        }

        public List(int capacity)
        {
            if (capacity < 0) { throw new ArgumentOutOfRangeException("capacity"); }
            items = new T[capacity];
            size = 0;
        }

        public List(IEnumerable<T> collection)
        {
            items = new T[0];
            size = 0;
            AddRange(collection);
        }

        public int Count => size;

        /// The elements the storage holds before the next growth. Setting it
        /// reallocates; below `Count` is an error, as in .NET.
        public int Capacity
        {
            get { return items.Length; }
            set
            {
                if (value < size) { throw new ArgumentOutOfRangeException("value"); }
                if (value == items.Length) { return; }
                var resized = new T[value];
                System.Array.Copy(items, resized, size);
                items = resized;
            }
        }

        // doubling, from 4; `required` wins when one insertion needs more
        private void Grow(int required)
        {
            int capacity = items.Length == 0 ? 4 : items.Length * 2;
            if (capacity < required) { capacity = required; }
            var grown = new T[capacity];
            System.Array.Copy(items, grown, size);
            items = grown;
        }

        /// Grows the storage to hold at least `capacity` elements; returns
        /// the new capacity.
        public int EnsureCapacity(int capacity)
        {
            if (capacity < 0) { throw new ArgumentOutOfRangeException("capacity"); }
            if (items.Length < capacity) { Grow(capacity); }
            return items.Length;
        }

        /// Gives back the slack when less than a tenth of the storage is used.
        public void TrimExcess()
        {
            int threshold = items.Length / 10 * 9;
            if (size < threshold) { Capacity = size; }
        }

        public void Add(T item)
        {
            if (size == items.Length) { Grow(size + 1); }
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

        public void Insert(int index, T item)
        {
            if (index < 0 || index > size) { throw new ArgumentOutOfRangeException("index"); }
            if (size == items.Length) { Grow(size + 1); }
            if (index < size)
            {
                System.Array.Copy(items, index, items, index + 1, size - index);
            }
            items[index] = item;
            size++;
        }

        public void InsertRange(int index, IEnumerable<T> collection)
        {
            if (index < 0 || index > size) { throw new ArgumentOutOfRangeException("index"); }
            // materialized first: `collection` may be this very list
            var incoming = new List<T>(collection);
            int added = incoming.size;
            if (added == 0) { return; }
            if (size + added > items.Length) { Grow(size + added); }
            if (index < size)
            {
                System.Array.Copy(items, index, items, index + added, size - index);
            }
            System.Array.Copy(incoming.items, 0, items, index, added);
            size += added;
        }

        public T[] ToArray()
        {
            T[] result = new T[size];
            System.Array.Copy(items, result, size);
            return result;
        }

        public void CopyTo(T[] array)
        {
            System.Array.Copy(items, 0, array, 0, size);
        }

        public void CopyTo(T[] array, int arrayIndex)
        {
            System.Array.Copy(items, 0, array, arrayIndex, size);
        }

        public void CopyTo(int index, T[] array, int arrayIndex, int count)
        {
            if (index < 0 || count < 0 || index + count > size) { throw new ArgumentOutOfRangeException("index"); }
            System.Array.Copy(items, index, array, arrayIndex, count);
        }

        /// A copy of `count` elements from `index` on, as a new list.
        public List<T> GetRange(int index, int count)
        {
            if (index < 0 || count < 0 || index + count > size) { throw new ArgumentOutOfRangeException("index"); }
            var result = new List<T>(count);
            System.Array.Copy(items, index, result.items, 0, count);
            result.size = count;
            return result;
        }

        // ---- searching: equality is `EqualityComparer<T>.Default`'s, i.e.
        // the element's own `Equals`, with `null` equal to `null` only

        public int IndexOf(T item)
        {
            return IndexOf(item, 0, size);
        }

        public int IndexOf(T item, int index)
        {
            if (index < 0 || index > size) { throw new ArgumentOutOfRangeException("index"); }
            return IndexOf(item, index, size - index);
        }

        public int IndexOf(T item, int index, int count)
        {
            if (index < 0 || count < 0 || index + count > size) { throw new ArgumentOutOfRangeException("index"); }
            int end = index + count;
            for (int i = index; i < end; i++)
            {
                if (MenSharp.Internal.Comparers.Equal(items[i], item)) { return i; }
            }
            return -1;
        }

        public int LastIndexOf(T item)
        {
            for (int i = size - 1; i >= 0; i--)
            {
                if (MenSharp.Internal.Comparers.Equal(items[i], item)) { return i; }
            }
            return -1;
        }

        public bool Contains(T item)
        {
            return IndexOf(item) >= 0;
        }

        /// Where `item` is in a list sorted by `Sort()`, or the bitwise
        /// complement of where it would go, as .NET's. (`-low - 1` is that
        /// complement: Udon has no `~` extern for ints.)
        public int BinarySearch(T item)
        {
            int low = 0;
            int high = size - 1;
            while (low <= high)
            {
                int middle = low + (high - low) / 2;
                int order = MenSharp.Internal.Comparers.Compare(items[middle], item);
                if (order == 0) { return middle; }
                if (order < 0) { low = middle + 1; }
                else { high = middle - 1; }
            }
            return -low - 1;
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

        public void Reverse()
        {
            Reverse(0, size);
        }

        public void Reverse(int index, int count)
        {
            if (index < 0 || count < 0 || index + count > size) { throw new ArgumentOutOfRangeException("index"); }
            int i = index;
            int j = index + count - 1;
            while (i < j)
            {
                T swapped = items[i];
                items[i] = items[j];
                items[j] = swapped;
                i++;
                j--;
            }
        }

        // ---- removal: the vacated slots are cleared so a removed element
        // is not kept alive by the storage, as .NET does

        public void Clear()
        {
            for (int i = 0; i < size; i++) { items[i] = default(T); }
            size = 0;
        }

        public bool Remove(T item)
        {
            int index = IndexOf(item);
            if (index < 0) { return false; }
            RemoveAt(index);
            return true;
        }

        public void RemoveAt(int index)
        {
            if (index < 0 || index >= size) { throw new ArgumentOutOfRangeException("index"); }
            size--;
            if (index < size)
            {
                System.Array.Copy(items, index + 1, items, index, size - index);
            }
            items[size] = default(T);
        }

        public void RemoveRange(int index, int count)
        {
            if (index < 0 || count < 0 || index + count > size) { throw new ArgumentOutOfRangeException("index"); }
            if (count == 0) { return; }
            int remaining = size - index - count;
            if (remaining > 0)
            {
                System.Array.Copy(items, index + count, items, index, remaining);
            }
            int end = size;
            size -= count;
            for (int i = size; i < end; i++) { items[i] = default(T); }
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

        public T FindLast(Predicate<T> predicate)
        {
            for (int i = size - 1; i >= 0; i--)
            {
                if (predicate(items[i])) { return items[i]; }
            }
            return default(T);
        }

        public List<T> FindAll(Predicate<T> predicate)
        {
            var result = new List<T>();
            for (int i = 0; i < size; i++)
            {
                if (predicate(items[i])) { result.Add(items[i]); }
            }
            return result;
        }

        public int FindIndex(Predicate<T> predicate)
        {
            return FindIndex(0, size, predicate);
        }

        public int FindIndex(int startIndex, Predicate<T> predicate)
        {
            if (startIndex < 0 || startIndex > size) { throw new ArgumentOutOfRangeException("startIndex"); }
            return FindIndex(startIndex, size - startIndex, predicate);
        }

        public int FindIndex(int startIndex, int count, Predicate<T> predicate)
        {
            if (startIndex < 0 || count < 0 || startIndex + count > size) { throw new ArgumentOutOfRangeException("startIndex"); }
            int end = startIndex + count;
            for (int i = startIndex; i < end; i++)
            {
                if (predicate(items[i])) { return i; }
            }
            return -1;
        }

        public int FindLastIndex(Predicate<T> predicate)
        {
            for (int i = size - 1; i >= 0; i--)
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
            for (int i = kept; i < size; i++) { items[i] = default(T); }
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
            var result = new List<TOut>(size);
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

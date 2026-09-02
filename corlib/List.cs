// MenSharp mini-corlib: System.Collections.Generic.List<T>.
//
// Udon does not whitelist the real List<T>, so M# ships this source port,
// compiled together with user code (and monomorphized per element type) like
// any other class. The algorithm mirrors dotnet/runtime's List<T> (MIT); the
// storage is a plain T[] that grows by doubling, and every operation below
// bottoms out in whitelisted externs (array ctor/Get/Set, Array.Copy).
//
// Deliberately minimal for now: no IEnumerable (foreach binds to the
// enumerator pattern below, which needs no interface), no Sort/Contains
// (both need comparers, which need interfaces on the object model). The
// indexer and RemoveAt throw ArgumentOutOfRangeException as .NET's do.

using System;

namespace System.Collections.Generic
{
    public class List<T>
    {
        private T[] items;
        private int size;

        public List()
        {
            items = new T[4];
            size = 0;
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

        // What `foreach` calls. The real one is a nested struct,
        // `List<T>.Enumerator`; until M# has value types (and nested types
        // that see the outer `T`) it is a class beside the list, so a
        // `foreach` costs one small allocation — the semantics (one pass over
        // the elements present when the loop started) are the same.
        public ListEnumerator<T> GetEnumerator()
        {
            return new ListEnumerator<T>(items, size);
        }
    }

    public class ListEnumerator<T>
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
    }
}

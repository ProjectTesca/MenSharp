// MenSharp mini-corlib: System.Collections.Generic.List<T>.
//
// Udon does not whitelist the real List<T>, so M# ships this source port,
// compiled together with user code (and monomorphized per element type) like
// any other class. The algorithm mirrors dotnet/runtime's List<T> (MIT); the
// storage is a plain T[] that grows by doubling, and every operation below
// bottoms out in whitelisted externs (array ctor/Get/Set, Array.Copy).
//
// Deliberately minimal for now: no IEnumerable, no exceptions (bounds are the
// underlying array's problem until M# exceptions land), no Sort/Contains
// (both need comparers, which need interfaces on the object model).

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
            get { return items[index]; }
            set { items[index] = value; }
        }

        public void Clear()
        {
            size = 0;
        }

        public void RemoveAt(int index)
        {
            int i = index;
            while (i < size - 1)
            {
                items[i] = items[i + 1];
                i++;
            }
            size--;
        }
    }
}

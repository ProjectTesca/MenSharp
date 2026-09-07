// MenSharp mini-corlib: System.Collections.Generic.HashSet<T>.
//
// Udon does not whitelist the real HashSet<T>, so this is a source port
// compiled with user code and monomorphized per element type, like List<T>
// and Dictionary<,>. The layout is Dictionary's without the values: a
// bucket array of chain heads over parallel entry arrays (hash, next,
// element) with a free list for removed entries. Hashing and equality call
// `GetHashCode()`/`Equals()` on the element's own static type, so a struct
// (field-wise) or a class overriding them is honoured, and everything else
// goes through the whitelisted `object` externs — see Dictionary.cs.
//
// `null` is a member like any other in .NET; it hashes nowhere, so it lives
// in a flag beside the table and enumerates last. The set operators
// (`UnionWith`, `IntersectWith`, ...) accept any sequence, including the
// set itself. An `IEqualityComparer<T>` (the source twin from Comparers.cs)
// replaces the element's own `Equals`/`GetHashCode`; the sets the operators
// build internally share it.

using System;

namespace System.Collections.Generic
{
    public class HashSet<T> : IEnumerable<T>
    {
        // buckets[b] holds (entry index + 1); 0 marks an empty bucket
        private int[] buckets;
        private int[] hashes;   // -1 marks a removed entry
        private int[] next;
        private T[] slots;
        private int count;      // entries ever used (high-water mark)
        private int freeList;   // head of the removed-entry chain, -1 if none
        private int freeCount;
        private bool hasNull;
        private IEqualityComparer<T> comparer;   // null: the element's own

        public HashSet()
        {
            Initialize(4);
        }

        public HashSet(int capacity)
        {
            if (capacity < 4) { capacity = 4; }
            Initialize(capacity);
        }

        public HashSet(IEnumerable<T> collection)
        {
            Initialize(4);
            UnionWith(collection);
        }

        public HashSet(IEqualityComparer<T> comparer)
        {
            this.comparer = comparer;
            Initialize(4);
        }

        public HashSet(int capacity, IEqualityComparer<T> comparer)
        {
            if (capacity < 4) { capacity = 4; }
            this.comparer = comparer;
            Initialize(capacity);
        }

        public HashSet(IEnumerable<T> collection, IEqualityComparer<T> comparer)
        {
            this.comparer = comparer;
            Initialize(4);
            UnionWith(collection);
        }

        /// The comparer the elements go through: the one given, else the default.
        public IEqualityComparer<T> Comparer
        {
            get
            {
                if (comparer == null) { return EqualityComparer<T>.Default; }
                return comparer;
            }
        }

        private void Initialize(int capacity)
        {
            buckets = new int[capacity];
            hashes = new int[capacity];
            next = new int[capacity];
            slots = new T[capacity];
            count = 0;
            freeList = -1;
            freeCount = 0;
            hasNull = false;
        }

        public int Count
        {
            get { return count - freeCount + (hasNull ? 1 : 0); }
        }

        private static bool IsNull(T item)
        {
            object boxed = item;
            return boxed == null;
        }

        private int Hash(T item)
        {
            if (comparer == null) { return item.GetHashCode() & 0x7FFFFFFF; }
            return comparer.GetHashCode(item) & 0x7FFFFFFF;
        }

        private bool ItemEquals(T a, T b)
        {
            if (comparer == null) { return a.Equals(b); }
            return comparer.Equals(a, b);
        }

        // the entry holding a non-null `item`, or -1
        private int FindEntry(T item)
        {
            int hash = Hash(item);
            int i = buckets[hash % buckets.Length] - 1;
            while (i >= 0)
            {
                if (hashes[i] == hash && ItemEquals(slots[i], item)) { return i; }
                i = next[i];
            }
            return -1;
        }

        public bool Contains(T item)
        {
            if (IsNull(item)) { return hasNull; }
            return FindEntry(item) >= 0;
        }

        /// True when `item` was not there yet.
        public bool Add(T item)
        {
            if (IsNull(item))
            {
                if (hasNull) { return false; }
                hasNull = true;
                return true;
            }
            int hash = Hash(item);
            int bucket = hash % buckets.Length;
            int i = buckets[bucket] - 1;
            while (i >= 0)
            {
                if (hashes[i] == hash && ItemEquals(slots[i], item)) { return false; }
                i = next[i];
            }

            int index;
            if (freeCount > 0)
            {
                index = freeList;
                freeList = next[index];
                freeCount--;
            }
            else
            {
                if (count == slots.Length)
                {
                    Resize();
                    bucket = hash % buckets.Length;
                }
                index = count;
                count++;
            }
            hashes[index] = hash;
            slots[index] = item;
            next[index] = buckets[bucket] - 1;
            buckets[bucket] = index + 1;
            return true;
        }

        private void Resize()
        {
            int size = slots.Length * 2;
            var newHashes = new int[size];
            var newNext = new int[size];
            var newSlots = new T[size];
            System.Array.Copy(hashes, newHashes, count);
            System.Array.Copy(slots, newSlots, count);
            var newBuckets = new int[size];
            for (int i = 0; i < count; i++)
            {
                if (newHashes[i] >= 0)
                {
                    int bucket = newHashes[i] % size;
                    newNext[i] = newBuckets[bucket] - 1;
                    newBuckets[bucket] = i + 1;
                }
            }
            buckets = newBuckets;
            hashes = newHashes;
            next = newNext;
            slots = newSlots;
        }

        /// True when `item` was there, and is no longer.
        public bool Remove(T item)
        {
            if (IsNull(item))
            {
                if (!hasNull) { return false; }
                hasNull = false;
                return true;
            }
            int hash = Hash(item);
            int bucket = hash % buckets.Length;
            int last = -1;
            int i = buckets[bucket] - 1;
            while (i >= 0)
            {
                if (hashes[i] == hash && ItemEquals(slots[i], item))
                {
                    RemoveEntry(i, bucket, last);
                    return true;
                }
                last = i;
                i = next[i];
            }
            return false;
        }

        // unlinks entry `i` (whose chain predecessor is `last`, or -1 when it
        // heads `bucket`) and puts it on the free list
        private void RemoveEntry(int i, int bucket, int last)
        {
            if (last < 0) { buckets[bucket] = next[i] + 1; }
            else { next[last] = next[i]; }
            hashes[i] = -1;
            slots[i] = default(T);
            next[i] = freeList;
            freeList = i;
            freeCount++;
        }

        // removes entry `i` wherever it sits in its chain
        private void RemoveEntryAt(int i)
        {
            int bucket = hashes[i] % buckets.Length;
            int last = -1;
            int j = buckets[bucket] - 1;
            while (j != i)
            {
                last = j;
                j = next[j];
            }
            RemoveEntry(i, bucket, last);
        }

        /// Removes every element the predicate accepts; returns how many.
        public int RemoveWhere(Predicate<T> match)
        {
            int removed = 0;
            for (int i = 0; i < count; i++)
            {
                if (hashes[i] >= 0 && match(slots[i]))
                {
                    RemoveEntryAt(i);
                    removed++;
                }
            }
            if (hasNull && match(default(T)))
            {
                hasNull = false;
                removed++;
            }
            return removed;
        }

        public void Clear()
        {
            Initialize(slots.Length);
        }

        /// The stored element equal to `equalValue`, when there is one.
        public bool TryGetValue(T equalValue, out T actualValue)
        {
            if (IsNull(equalValue))
            {
                actualValue = default(T);
                return hasNull;
            }
            int i = FindEntry(equalValue);
            if (i >= 0)
            {
                actualValue = slots[i];
                return true;
            }
            actualValue = default(T);
            return false;
        }

        public void CopyTo(T[] array)
        {
            CopyTo(array, 0);
        }

        public void CopyTo(T[] array, int arrayIndex)
        {
            int at = arrayIndex;
            for (int i = 0; i < count; i++)
            {
                if (hashes[i] >= 0)
                {
                    array[at] = slots[i];
                    at++;
                }
            }
            if (hasNull) { array[at] = default(T); }
        }

        // ---- set operations. `other` may be this set: the ones that read
        // while writing snapshot it into a second set first

        private bool IsThis(IEnumerable<T> other)
        {
            object them = other;
            object me = this;
            return them == me;
        }

        // `other` as a set: itself when it already is this set, else a copy
        private HashSet<T> AsSet(IEnumerable<T> other)
        {
            if (IsThis(other)) { return this; }
            return new HashSet<T>(other, comparer);
        }

        public void UnionWith(IEnumerable<T> other)
        {
            if (IsThis(other)) { return; }
            foreach (T item in other) { Add(item); }
        }

        public void IntersectWith(IEnumerable<T> other)
        {
            if (IsThis(other)) { return; }
            var keep = new HashSet<T>(other, comparer);
            for (int i = 0; i < count; i++)
            {
                if (hashes[i] >= 0 && !keep.Contains(slots[i])) { RemoveEntryAt(i); }
            }
            if (hasNull && !keep.hasNull) { hasNull = false; }
        }

        public void ExceptWith(IEnumerable<T> other)
        {
            if (IsThis(other))
            {
                Clear();
                return;
            }
            foreach (T item in other) { Remove(item); }
        }

        /// Keeps what is in exactly one of the two.
        public void SymmetricExceptWith(IEnumerable<T> other)
        {
            if (IsThis(other))
            {
                Clear();
                return;
            }
            // an element `other` repeats must not be removed on its second
            // appearance after being added on its first
            var added = new HashSet<T>(comparer);
            foreach (T item in other)
            {
                if (added.Contains(item)) { continue; }
                if (!Remove(item))
                {
                    Add(item);
                    added.Add(item);
                }
            }
        }

        public bool IsSubsetOf(IEnumerable<T> other)
        {
            var them = AsSet(other);
            return ContainedIn(them);
        }

        public bool IsProperSubsetOf(IEnumerable<T> other)
        {
            var them = AsSet(other);
            return Count < them.Count && ContainedIn(them);
        }

        public bool IsSupersetOf(IEnumerable<T> other)
        {
            if (IsThis(other)) { return true; }
            foreach (T item in other)
            {
                if (!Contains(item)) { return false; }
            }
            return true;
        }

        public bool IsProperSupersetOf(IEnumerable<T> other)
        {
            var them = AsSet(other);
            return them.Count < Count && them.ContainedIn(this);
        }

        /// True when the two share at least one element.
        public bool Overlaps(IEnumerable<T> other)
        {
            if (IsThis(other)) { return Count > 0; }
            foreach (T item in other)
            {
                if (Contains(item)) { return true; }
            }
            return false;
        }

        public bool SetEquals(IEnumerable<T> other)
        {
            var them = AsSet(other);
            return Count == them.Count && ContainedIn(them);
        }

        // every element here is in `them`
        private bool ContainedIn(HashSet<T> them)
        {
            for (int i = 0; i < count; i++)
            {
                if (hashes[i] >= 0 && !them.Contains(slots[i])) { return false; }
            }
            return !hasNull || them.hasNull;
        }

        public HashSetEnumerator<T> GetEnumerator()
        {
            return new HashSetEnumerator<T>(hashes, slots, count, hasNull);
        }

        IEnumerator<T> IEnumerable<T>.GetEnumerator()
        {
            return new HashSetEnumerator<T>(hashes, slots, count, hasNull);
        }
    }

    /// Walks the live entries in slot order, then the `null` member if any.
    public sealed class HashSetEnumerator<T> : IEnumerator<T>
    {
        private int[] hashes;
        private T[] slots;
        private int count;
        private bool hasNull;
        private int index;
        private T current;

        public HashSetEnumerator(int[] hashes, T[] slots, int count, bool hasNull)
        {
            this.hashes = hashes;
            this.slots = slots;
            this.count = count;
            this.hasNull = hasNull;
            index = 0;
        }

        public bool MoveNext()
        {
            while (index < count)
            {
                int i = index;
                index++;
                if (hashes[i] >= 0)
                {
                    current = slots[i];
                    return true;
                }
            }
            if (index == count && hasNull)
            {
                index++;
                current = default(T);
                return true;
            }
            return false;
        }

        public T Current => current;

        public void Dispose() { }
    }
}

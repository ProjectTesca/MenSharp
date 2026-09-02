// MenSharp mini-corlib: System.Collections.Generic.Dictionary<TKey, TValue>.
//
// Udon whitelists neither the real Dictionary<,> nor KeyValuePair<,>, so
// both are source ports compiled with user code and monomorphized per key
// and value type, like List<T>. The layout mirrors dotnet/runtime's
// Dictionary (MIT): a bucket array of chain heads over parallel entry
// arrays (hash, next, key, value) with a free list for removed entries.
// Hashing and equality go through `object.GetHashCode()`/`object.Equals()`,
// which are whitelisted externs and dispatch virtually on the boxed key —
// so strings, ints, enums and engine structs hash by value and classes of
// your own by identity, exactly as the real one does without a comparer.
//
// Without exceptions (M# has none yet) the failure cases are defined
// instead of thrown: `this[missing]` reads `default(TValue)` (use
// TryGetValue), `Add` on a present key overwrites. A null key is not
// checked and fails inside the hashing extern, as it would throw in C#.
// The nested types (`Dictionary<,>.Enumerator`, `KeyCollection`, ...) are
// top-level classes here until M# has nested types that see the outer
// type parameters; `foreach` binds to them by the enumerator pattern.

namespace System.Collections.Generic
{
    public class KeyValuePair<TKey, TValue>
    {
        private TKey key;
        private TValue value;

        public KeyValuePair(TKey key, TValue value)
        {
            this.key = key;
            this.value = value;
        }

        public TKey Key => key;
        public TValue Value => value;
    }

    public class Dictionary<TKey, TValue>
    {
        // buckets[b] holds (entry index + 1); 0 marks an empty bucket
        private int[] buckets;
        private int[] hashes;   // -1 marks a removed entry
        private int[] next;
        private TKey[] keys;
        private TValue[] values;
        private int count;      // entries ever used (high-water mark)
        private int freeList;   // head of the removed-entry chain, -1 if none
        private int freeCount;

        public Dictionary()
        {
            Initialize(4);
        }

        public Dictionary(int capacity)
        {
            if (capacity < 4) { capacity = 4; }
            Initialize(capacity);
        }

        private void Initialize(int capacity)
        {
            buckets = new int[capacity];
            hashes = new int[capacity];
            next = new int[capacity];
            keys = new TKey[capacity];
            values = new TValue[capacity];
            count = 0;
            freeList = -1;
            freeCount = 0;
        }

        public int Count => count - freeCount;

        private static int Hash(TKey key)
        {
            object boxed = key;
            return boxed.GetHashCode() & 0x7FFFFFFF;
        }

        private static bool KeyEquals(TKey a, TKey b)
        {
            object boxed = a;
            return boxed.Equals(b);
        }

        private int FindEntry(TKey key)
        {
            int hash = Hash(key);
            int i = buckets[hash % buckets.Length] - 1;
            while (i >= 0)
            {
                if (hashes[i] == hash && KeyEquals(keys[i], key)) { return i; }
                i = next[i];
            }
            return -1;
        }

        public TValue this[TKey key]
        {
            get
            {
                int i = FindEntry(key);
                if (i >= 0) { return values[i]; }
                return default(TValue);
            }
            set { Insert(key, value); }
        }

        public void Add(TKey key, TValue value)
        {
            Insert(key, value);
        }

        private void Insert(TKey key, TValue value)
        {
            int hash = Hash(key);
            int bucket = hash % buckets.Length;
            int i = buckets[bucket] - 1;
            while (i >= 0)
            {
                if (hashes[i] == hash && KeyEquals(keys[i], key))
                {
                    values[i] = value;
                    return;
                }
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
                if (count == keys.Length)
                {
                    Resize();
                    bucket = hash % buckets.Length;
                }
                index = count;
                count++;
            }
            hashes[index] = hash;
            keys[index] = key;
            values[index] = value;
            next[index] = buckets[bucket] - 1;
            buckets[bucket] = index + 1;
        }

        private void Resize()
        {
            int size = keys.Length * 2;
            var newHashes = new int[size];
            var newNext = new int[size];
            var newKeys = new TKey[size];
            var newValues = new TValue[size];
            System.Array.Copy(hashes, newHashes, count);
            System.Array.Copy(keys, newKeys, count);
            System.Array.Copy(values, newValues, count);
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
            keys = newKeys;
            values = newValues;
        }

        public bool ContainsKey(TKey key)
        {
            return FindEntry(key) >= 0;
        }

        public bool ContainsValue(TValue value)
        {
            for (int i = 0; i < count; i++)
            {
                if (hashes[i] >= 0)
                {
                    object boxed = values[i];
                    if (boxed == null) { if (value == null) { return true; } }
                    else if (boxed.Equals(value)) { return true; }
                }
            }
            return false;
        }

        public bool TryGetValue(TKey key, out TValue value)
        {
            int i = FindEntry(key);
            if (i >= 0)
            {
                value = values[i];
                return true;
            }
            value = default(TValue);
            return false;
        }

        public bool Remove(TKey key)
        {
            int hash = Hash(key);
            int bucket = hash % buckets.Length;
            int last = -1;
            int i = buckets[bucket] - 1;
            while (i >= 0)
            {
                if (hashes[i] == hash && KeyEquals(keys[i], key))
                {
                    if (last < 0) { buckets[bucket] = next[i] + 1; }
                    else { next[last] = next[i]; }
                    hashes[i] = -1;
                    keys[i] = default(TKey);
                    values[i] = default(TValue);
                    next[i] = freeList;
                    freeList = i;
                    freeCount++;
                    return true;
                }
                last = i;
                i = next[i];
            }
            return false;
        }

        public void Clear()
        {
            Initialize(keys.Length);
        }

        public DictionaryKeyCollection<TKey, TValue> Keys => new DictionaryKeyCollection<TKey, TValue>(this);
        public DictionaryValueCollection<TKey, TValue> Values => new DictionaryValueCollection<TKey, TValue>(this);

        public DictionaryEnumerator<TKey, TValue> GetEnumerator()
        {
            return new DictionaryEnumerator<TKey, TValue>(hashes, keys, values, count);
        }

        // for the collections' enumerators: the live entry arrays
        public DictionaryKeyEnumerator<TKey, TValue> KeyEnumerator()
        {
            return new DictionaryKeyEnumerator<TKey, TValue>(hashes, keys, count);
        }

        public DictionaryValueEnumerator<TKey, TValue> ValueEnumerator()
        {
            return new DictionaryValueEnumerator<TKey, TValue>(hashes, values, count);
        }
    }

    public class DictionaryEnumerator<TKey, TValue>
    {
        private int[] hashes;
        private TKey[] keys;
        private TValue[] values;
        private int count;
        private int index;
        private KeyValuePair<TKey, TValue> current;

        public DictionaryEnumerator(int[] hashes, TKey[] keys, TValue[] values, int count)
        {
            this.hashes = hashes;
            this.keys = keys;
            this.values = values;
            this.count = count;
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
                    current = new KeyValuePair<TKey, TValue>(keys[i], values[i]);
                    return true;
                }
            }
            return false;
        }

        public KeyValuePair<TKey, TValue> Current => current;
    }

    public class DictionaryKeyCollection<TKey, TValue>
    {
        private Dictionary<TKey, TValue> dictionary;

        public DictionaryKeyCollection(Dictionary<TKey, TValue> dictionary)
        {
            this.dictionary = dictionary;
        }

        public int Count => dictionary.Count;

        public DictionaryKeyEnumerator<TKey, TValue> GetEnumerator()
        {
            return dictionary.KeyEnumerator();
        }
    }

    public class DictionaryKeyEnumerator<TKey, TValue>
    {
        private int[] hashes;
        private TKey[] keys;
        private int count;
        private int index;
        private TKey current;

        public DictionaryKeyEnumerator(int[] hashes, TKey[] keys, int count)
        {
            this.hashes = hashes;
            this.keys = keys;
            this.count = count;
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
                    current = keys[i];
                    return true;
                }
            }
            return false;
        }

        public TKey Current => current;
    }

    public class DictionaryValueCollection<TKey, TValue>
    {
        private Dictionary<TKey, TValue> dictionary;

        public DictionaryValueCollection(Dictionary<TKey, TValue> dictionary)
        {
            this.dictionary = dictionary;
        }

        public int Count => dictionary.Count;

        public DictionaryValueEnumerator<TKey, TValue> GetEnumerator()
        {
            return dictionary.ValueEnumerator();
        }
    }

    public class DictionaryValueEnumerator<TKey, TValue>
    {
        private int[] hashes;
        private TValue[] values;
        private int count;
        private int index;
        private TValue current;

        public DictionaryValueEnumerator(int[] hashes, TValue[] values, int count)
        {
            this.hashes = hashes;
            this.values = values;
            this.count = count;
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
                    current = values[i];
                    return true;
                }
            }
            return false;
        }

        public TValue Current => current;
    }
}

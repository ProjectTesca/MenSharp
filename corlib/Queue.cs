// MenSharp mini-corlib: System.Collections.Generic.Queue<T>.
//
// A source port compiled with user code and monomorphized per element type,
// like List<T>: a ring over a plain T[] (`head` is the oldest element,
// `tail` where the next goes) that doubles when full, mirroring
// dotnet/runtime's Queue<T> (MIT). `Dequeue`/`Peek` on an empty queue throw
// InvalidOperationException as .NET's do; `foreach` walks oldest to newest.

using System;

namespace System.Collections.Generic
{
    public class Queue<T> : IEnumerable<T>
    {
        private T[] items;
        private int head;
        private int tail;
        private int size;

        public Queue()
        {
            items = new T[0];
        }

        public Queue(int capacity)
        {
            if (capacity < 0) { throw new ArgumentOutOfRangeException("capacity"); }
            items = new T[capacity];
        }

        public Queue(IEnumerable<T> collection)
        {
            items = new T[4];
            foreach (T item in collection) { Enqueue(item); }
        }

        public int Count => size;

        public void Enqueue(T item)
        {
            if (size == items.Length) { Grow(size + 1); }
            items[tail] = item;
            tail = (tail + 1) % items.Length;
            size++;
        }

        public T Dequeue()
        {
            if (size == 0) { throw new InvalidOperationException("Queue empty."); }
            T item = items[head];
            items[head] = default(T);
            head = (head + 1) % items.Length;
            size--;
            return item;
        }

        public bool TryDequeue(out T result)
        {
            if (size == 0)
            {
                result = default(T);
                return false;
            }
            result = Dequeue();
            return true;
        }

        public T Peek()
        {
            if (size == 0) { throw new InvalidOperationException("Queue empty."); }
            return items[head];
        }

        public bool TryPeek(out T result)
        {
            if (size == 0)
            {
                result = default(T);
                return false;
            }
            result = items[head];
            return true;
        }

        public void Clear()
        {
            for (int i = 0; i < size; i++) { items[(head + i) % items.Length] = default(T); }
            head = 0;
            tail = 0;
            size = 0;
        }

        public bool Contains(T item)
        {
            for (int i = 0; i < size; i++)
            {
                if (MenSharp.Internal.Comparers.Equal(items[(head + i) % items.Length], item)) { return true; }
            }
            return false;
        }

        /// The elements oldest first.
        public T[] ToArray()
        {
            var result = new T[size];
            CopyTo(result, 0);
            return result;
        }

        public void CopyTo(T[] array, int arrayIndex)
        {
            if (size == 0) { return; }
            if (head < tail)
            {
                System.Array.Copy(items, head, array, arrayIndex, size);
            }
            else
            {
                int firstPart = items.Length - head;
                System.Array.Copy(items, head, array, arrayIndex, firstPart);
                System.Array.Copy(items, 0, array, arrayIndex + firstPart, tail);
            }
        }

        // doubling, from 4; `required` wins when one insertion needs more
        private void Grow(int required)
        {
            int capacity = items.Length == 0 ? 4 : items.Length * 2;
            if (capacity < required) { capacity = required; }
            SetCapacity(capacity);
        }

        // reallocates with the elements unwrapped to the front
        private void SetCapacity(int capacity)
        {
            var resized = new T[capacity];
            CopyTo(resized, 0);
            items = resized;
            head = 0;
            tail = size == capacity ? 0 : size;
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
            if (size < threshold) { SetCapacity(size); }
        }

        public QueueEnumerator<T> GetEnumerator()
        {
            return new QueueEnumerator<T>(items, head, size);
        }

        IEnumerator<T> IEnumerable<T>.GetEnumerator()
        {
            return new QueueEnumerator<T>(items, head, size);
        }
    }

    public sealed class QueueEnumerator<T> : IEnumerator<T>
    {
        private T[] items;
        private int head;
        private int count;
        private int index;
        private T current;

        public QueueEnumerator(T[] items, int head, int count)
        {
            this.items = items;
            this.head = head;
            this.count = count;
            index = 0;
        }

        public bool MoveNext()
        {
            if (index < count)
            {
                current = items[(head + index) % items.Length];
                index++;
                return true;
            }
            return false;
        }

        public T Current => current;

        public void Dispose() { }
    }
}

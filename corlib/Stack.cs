// MenSharp mini-corlib: System.Collections.Generic.Stack<T>.
//
// A source port compiled with user code and monomorphized per element type,
// like List<T>: a plain T[] filled from the bottom that doubles when full,
// mirroring dotnet/runtime's Stack<T> (MIT). `Pop`/`Peek` on an empty stack
// throw InvalidOperationException as .NET's do; `foreach`, `ToArray` and
// `CopyTo` go top first, as .NET's do.

using System;

namespace System.Collections.Generic
{
    public class Stack<T> : IEnumerable<T>
    {
        private T[] items;
        private int size;

        public Stack()
        {
            items = new T[0];
        }

        public Stack(int capacity)
        {
            if (capacity < 0) { throw new ArgumentOutOfRangeException("capacity"); }
            items = new T[capacity];
        }

        public Stack(IEnumerable<T> collection)
        {
            items = new T[4];
            foreach (T item in collection) { Push(item); }
        }

        public int Count => size;

        public void Push(T item)
        {
            if (size == items.Length) { Grow(size + 1); }
            items[size] = item;
            size++;
        }

        public T Pop()
        {
            if (size == 0) { throw new InvalidOperationException("Stack empty."); }
            size--;
            T item = items[size];
            items[size] = default(T);
            return item;
        }

        public bool TryPop(out T result)
        {
            if (size == 0)
            {
                result = default(T);
                return false;
            }
            result = Pop();
            return true;
        }

        public T Peek()
        {
            if (size == 0) { throw new InvalidOperationException("Stack empty."); }
            return items[size - 1];
        }

        public bool TryPeek(out T result)
        {
            if (size == 0)
            {
                result = default(T);
                return false;
            }
            result = items[size - 1];
            return true;
        }

        public void Clear()
        {
            for (int i = 0; i < size; i++) { items[i] = default(T); }
            size = 0;
        }

        public bool Contains(T item)
        {
            for (int i = 0; i < size; i++)
            {
                if (MenSharp.Internal.Comparers.Equal(items[i], item)) { return true; }
            }
            return false;
        }

        /// The elements top first.
        public T[] ToArray()
        {
            var result = new T[size];
            CopyTo(result, 0);
            return result;
        }

        public void CopyTo(T[] array, int arrayIndex)
        {
            for (int i = 0; i < size; i++)
            {
                array[arrayIndex + i] = items[size - 1 - i];
            }
        }

        // doubling, from 4; `required` wins when one insertion needs more
        private void Grow(int required)
        {
            int capacity = items.Length == 0 ? 4 : items.Length * 2;
            if (capacity < required) { capacity = required; }
            SetCapacity(capacity);
        }

        private void SetCapacity(int capacity)
        {
            var resized = new T[capacity];
            System.Array.Copy(items, resized, size);
            items = resized;
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

        public StackEnumerator<T> GetEnumerator()
        {
            return new StackEnumerator<T>(items, size);
        }

        IEnumerator<T> IEnumerable<T>.GetEnumerator()
        {
            return new StackEnumerator<T>(items, size);
        }
    }

    /// Walks from the top down.
    public sealed class StackEnumerator<T> : IEnumerator<T>
    {
        private T[] items;
        private int index;
        private T current;

        public StackEnumerator(T[] items, int count)
        {
            this.items = items;
            index = count;
        }

        public bool MoveNext()
        {
            if (index > 0)
            {
                index--;
                current = items[index];
                return true;
            }
            return false;
        }

        public T Current => current;

        public void Dispose() { }
    }
}

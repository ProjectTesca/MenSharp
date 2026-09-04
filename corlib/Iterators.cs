// MenSharp mini-corlib: iterators (`yield return`).
//
// An iterator method suspends the same way an `async` one does (see
// Tasks.cs and the code generator's `tasks` module): at `yield return` the
// method's variables are copied out, the value and the continuation are
// handed to the Iterator<T> object it made on entry, and the method returns.
// `MoveNext` runs that continuation, which puts the variables back and
// continues after the `yield` — until the next one, or the end.
//
// The interfaces below are source twins of .NET's, so an iterator method is
// declared exactly as in C# (`IEnumerable<int> Evens()`), `foreach` binds
// to them by the usual pattern, and the same file compiles under Unity.
// They are the members `foreach` needs and nothing more.

using System;

namespace System.Collections.Generic
{
    public interface IEnumerable<T>
    {
        IEnumerator<T> GetEnumerator();
    }

    public interface IEnumerator<T>
    {
        bool MoveNext();
        T Current { get; }
    }
}

namespace MenSharp.Internal
{
    /// A `T[]` seen as a sequence: what the compiler wraps an array in when
    /// one is passed where `IEnumerable<T>` is wanted. `foreach` over an
    /// array never comes here — it walks the array by index.
    public class ArrayEnumerable<T> : System.Collections.Generic.IEnumerable<T>
    {
        private readonly T[] items;

        public ArrayEnumerable(T[] items) { this.items = items; }

        public System.Collections.Generic.IEnumerator<T> GetEnumerator()
        {
            return new ArrayEnumerator<T>(items);
        }
    }

    public class ArrayEnumerator<T> : System.Collections.Generic.IEnumerator<T>
    {
        private readonly T[] items;
        private int index;
        private T current;

        public ArrayEnumerator(T[] items) { this.items = items; index = 0; }

        public bool MoveNext()
        {
            if (items == null || index >= items.Length) { return false; }
            current = items[index];
            index++;
            return true;
        }

        public T Current { get { return current; } }
    }

    public static class Iterators
    {
        /// The iterator whose body is running its next step: the body reads
        /// it back when it resumes, so a copy made by a second `foreach`
        /// (`GetEnumerator` again) drives the same code with its own state.
        public static object Stepping;
    }

    /// What an iterator method returns: both the enumerable and its
    /// enumerator, as C#'s compiler-generated class is.
    public class Iterator<T> : System.Collections.Generic.IEnumerable<T>, System.Collections.Generic.IEnumerator<T>
    {
        // 0 between steps (or not started), 1 yielded a value, 2 finished
        private int state;
        private T current;
        private Action resume;
        private Action entry;
        private bool enumerating;

        public Iterator() { }

        /// Written by the compiler on entry: the continuation that starts
        /// the body from the beginning.
        public void __Start(Action body)
        {
            entry = body;
            resume = body;
        }

        public System.Collections.Generic.IEnumerator<T> GetEnumerator()
        {
            if (!enumerating)
            {
                enumerating = true;
                return this;
            }
            // enumerated again: a fresh run of the same body and arguments
            Iterator<T> copy = new Iterator<T>();
            copy.__Start(entry);
            copy.enumerating = true;
            return copy;
        }

        public T Current
        {
            get { return current; }
        }

        public bool MoveNext()
        {
            if (state == 2)
            {
                return false;
            }
            Action step = resume;
            if (step == null)
            {
                state = 2;
                return false;
            }
            resume = null;
            state = 0;
            Iterators.Stepping = this;
            try
            {
                step();
            }
            catch (Exception)
            {
                // an exception out of the body ends the iteration, as in C#
                state = 2;
                current = default(T);
                throw;
            }
            return state == 1;
        }

        /// Written by the compiler at `yield return value`, with the
        /// continuation that resumes after it.
        public void __Yield(T value, Action next)
        {
            current = value;
            resume = next;
            state = 1;
        }

        /// Written by the compiler at `yield break` and at the end of the body.
        public void __Finish()
        {
            state = 2;
            current = default(T);
            resume = null;
        }

        public void Reset()
        {
            throw new NotSupportedException("an iterator cannot be reset: enumerate it again instead");
        }
    }
}

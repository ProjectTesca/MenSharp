// MenSharp mini-corlib: what the compiler's delegates need at run time.
//
// A delegate is an object[]: element 0 the code address of its thunk (a
// boxed uint), the rest its payload — the receiver of a method, the `this`
// and the captured variables of a lambda. A *multicast* delegate is an
// object[] whose element 0 is the address of its shape's multicast thunk
// and whose element 1 is the object[] of the single delegates it calls in
// turn. The compiler lowers `a + b`, `a - b`, `a == b` on delegates to the
// helpers below, handing them that shape's multicast address; see the code
// generator's `delegates` module.
namespace MenSharp.Internal
{
    public static class Delegates
    {
        // A delegate is an object[]: [0] the code address of its thunk, [1]
        // the UdonBehaviour whose program made it, [2] the address of its
        // shape's remote entry in that program, [3..] its payload — for a
        // multicast delegate, [3] the list of delegates. Kept in step with
        // the compiler's `delegates` module.

        /// `Delegate.Combine`: the invocation list of `a` followed by `b`'s.
        public static object[] Combine(object[] a, object[] b, object multicast, object owner, object remote)
        {
            if (a == null)
            {
                return b;
            }
            if (b == null)
            {
                return a;
            }
            object[] first = ListOf(a, multicast);
            object[] second = ListOf(b, multicast);
            object[] list = new object[first.Length + second.Length];
            for (int i = 0; i < first.Length; i++)
            {
                list[i] = first[i];
            }
            for (int i = 0; i < second.Length; i++)
            {
                list[first.Length + i] = second[i];
            }
            return new object[] { multicast, owner, remote, list };
        }

        /// `Delegate.Remove`: `a` without the last occurrence of `b`'s
        /// invocation list; `a` itself when it holds none.
        public static object[] Remove(object[] a, object[] b, object multicast, object owner, object remote)
        {
            if (a == null || b == null)
            {
                return a;
            }
            object[] source = ListOf(a, multicast);
            object[] removed = ListOf(b, multicast);
            if (removed.Length > source.Length)
            {
                return a;
            }
            int found = -1;
            for (int start = source.Length - removed.Length; start >= 0; start--)
            {
                bool matches = true;
                for (int i = 0; i < removed.Length; i++)
                {
                    if (!AreEqual((object[])source[start + i], (object[])removed[i], multicast))
                    {
                        matches = false;
                        break;
                    }
                }
                if (matches)
                {
                    found = start;
                    break;
                }
            }
            if (found < 0)
            {
                return a;
            }
            int remaining = source.Length - removed.Length;
            if (remaining == 0)
            {
                return null;
            }
            object[] list = new object[remaining];
            int next = 0;
            for (int i = 0; i < source.Length; i++)
            {
                if (i < found || i >= found + removed.Length)
                {
                    list[next] = source[i];
                    next++;
                }
            }
            if (remaining == 1)
            {
                return (object[])list[0];
            }
            return new object[] { multicast, owner, remote, list };
        }

        /// `Delegate.Equals`: the same target and the same payload — for a
        /// method group the same receiver, for a lambda the same closure.
        public static bool AreEqual(object[] a, object[] b, object multicast)
        {
            if (a == null || b == null)
            {
                return a == null && b == null;
            }
            if (a.Length != b.Length)
            {
                return false;
            }
            if ((uint)a[0] != (uint)b[0])
            {
                return false;
            }
            // the same program's, of the same shape
            if (!object.ReferenceEquals(a[1], b[1]) || (uint)a[2] != (uint)b[2])
            {
                return false;
            }
            if ((uint)a[0] == (uint)multicast)
            {
                object[] first = (object[])a[3];
                object[] second = (object[])b[3];
                if (first.Length != second.Length)
                {
                    return false;
                }
                for (int i = 0; i < first.Length; i++)
                {
                    if (!AreEqual((object[])first[i], (object[])second[i], multicast))
                    {
                        return false;
                    }
                }
                return true;
            }
            for (int i = 3; i < a.Length; i++)
            {
                if (!object.ReferenceEquals(a[i], b[i]))
                {
                    return false;
                }
            }
            return true;
        }

        /// The invocation list of a delegate: its own list when it is a
        /// multicast one, else itself alone.
        private static object[] ListOf(object[] d, object multicast)
        {
            if ((uint)d[0] == (uint)multicast)
            {
                return (object[])d[3];
            }
            return new object[] { d };
        }
    }
}

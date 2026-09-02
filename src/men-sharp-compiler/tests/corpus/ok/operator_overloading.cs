// User-defined operators on structs and classes: arithmetic, unary,
// comparison pairs, increment, compound assignment, `==` with null.
namespace Corpus
{
    public struct V
    {
        public int x, y;
        public V(int x, int y) { this.x = x; this.y = y; }
        public static V operator +(V a, V b) => new V(a.x + b.x, a.y + b.y);
        public static V operator -(V a) => new V(-a.x, -a.y);
        public static V operator *(V a, int k) => new V(a.x * k, a.y * k);
        public static V operator *(int k, V a) => a * k;
        public static bool operator ==(V a, V b) => a.x == b.x && a.y == b.y;
        public static bool operator !=(V a, V b) => !(a == b);
        public static bool operator <(V a, V b) => a.x < b.x;
        public static bool operator >(V a, V b) => a.x > b.x;
        public static V operator ++(V a) => new V(a.x + 1, a.y + 1);
        public static bool operator !(V a) => a.x == 0 && a.y == 0;
        public override bool Equals(object o) => o is V v && v == this;
        public override int GetHashCode() => x * 31 + y;
    }

    public class Money
    {
        public int cents;
        public Money(int c) { cents = c; }
        public static Money operator +(Money a, Money b) => new Money(a.cents + b.cents);
        public static bool operator ==(Money a, Money b)
        {
            if ((object)a == null) return (object)b == null;
            if ((object)b == null) return false;
            return a.cents == b.cents;
        }
        public static bool operator !=(Money a, Money b) => !(a == b);
        public override bool Equals(object o) => o is Money m && m == this;
        public override int GetHashCode() => cents;
    }

    public class Program
    {
        public static int total;

        public static void Main()
        {
            var a = new V(1, 2);
            var b = a + new V(3, 4) * 2;
            b += -a;
            b++;
            ++b;
            var c = b++;
            total = b.x + c.y + (a == new V(1, 2) ? 1 : 0) + (a != b ? 1 : 0) + (a < b ? 1 : 0) + (!a ? 1 : 0);
            Money m = null;
            Money n = new Money(5) + new Money(1);
            total += (m == null ? 1 : 0) + (n != null ? 1 : 0) + (n == new Money(6) ? 1 : 0);
        }
    }
}

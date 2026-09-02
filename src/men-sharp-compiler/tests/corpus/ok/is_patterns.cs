// `is` patterns: constants, relational, `and`/`or`/`not`, `var`, discard,
// property patterns (typed, untyped, nested) with designations.
namespace Corpus
{
    public enum Color { Red, Green, Blue }
    public abstract class Shape { public int Id; public abstract int Area(); }
    public class Circle : Shape
    {
        public int Radius; public Color Tint; public Circle Inner;
        public override int Area() => 3 * Radius * Radius;
    }
    public class Square : Shape { public int Side; public override int Area() => Side * Side; }
    public struct P { public int x; public int y; }

    public class Program
    {
        public static int total;

        static int B(bool b) => b ? 1 : 0;

        public static void Main()
        {
            int n = 7; string s = "abc"; object o = 5; object nothing = null; Color c = Color.Green;
            total = B(n is 7) + B(n is not 8) + B(s is "abc") + B(nothing is null) + B(o is not null)
                + B(c is Color.Green) + B(o is 5) + B(n is > 5) + B(n is >= 7 and < 10)
                + B(n is < 3 or > 6) + B(n is not (> 0 and < 5)) + B(o is > 3)
                + B(n is var anything && anything == 7) + B(n is (7));
            Shape sh = new Circle { Id = 1, Radius = 2, Tint = Color.Blue, Inner = new Circle { Radius = 1 } };
            P p = new P { x = 3, y = -1 };
            total += B(sh is Circle { Radius: 2 }) + B(sh is { Id: 1 })
                + B(sh is Circle { Tint: Color.Blue, Inner: { Radius: 1 } } found && found.Radius == 2)
                + B(sh is not Square) + B(p is { x: > 0, y: < 0 })
                + B(sh is Circle { Inner: not null } c2 && c2.Area() == 12)
                + B(sh is Square { Side: 2 } or Circle { Radius: 2 })
                + B(o is int i && i == 5);
        }
    }
}

// `switch` statements with patterns and `when` guards, and switch expressions.
namespace Corpus
{
    public enum Color { Red, Green, Blue }
    public abstract class Shape { public abstract int Area(); }
    public class Circle : Shape { public int Radius; public override int Area() => 3 * Radius * Radius; }
    public class Square : Shape { public int Side; public override int Area() => Side * Side; }

    public class Program
    {
        public static int total;
        public static string text;

        static int Classify(Shape s)
        {
            switch (s)
            {
                case null: return -1;
                case Circle { Radius: > 5 } big: return 100 + big.Radius;
                case Circle c when c.Radius == 2: return 20;
                case Circle c: return 10 + c.Radius;
                case Square { Side: 0 }: return 0;
                default: return 1;
            }
        }

        static string Describe(object o) => o switch
        {
            null => "null",
            int i when i < 0 => "negative",
            int i => "int" + i,
            string s => "str:" + s,
            Circle { Radius: var rad } => "circle" + rad,
            _ => "other",
        };

        static int Old(Color c)
        {
            switch (c)
            {
                case Color.Red: return 1;
                case Color.Blue: return 3;
                default: return 2;
            }
        }

        public static void Main()
        {
            total = Classify(null) + Classify(new Circle { Radius = 7 }) + Classify(new Square { Side = 4 }) + Old(Color.Green);
            int n = 5;
            text = Describe(4) + Describe("x") + Describe(new Circle { Radius = 9 })
                + (n switch { < 3 => "small", >= 3 and < 10 => "medium", _ => "large" });
        }
    }
}

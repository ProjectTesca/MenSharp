// Records: positional and nominal, class and struct, with inheritance,
// `with`, deconstruction, value equality and patterns — csc and M# must
// both accept every line.
using System;
using System.Collections.Generic;

namespace Corpus
{
    public record Point(int X, int Y);

    public record Named(string Name, Point At)
    {
        public int Extra = 3;
        public int Twice => X2 * 2;
        public int X2 { get; set; } = 5;
        public string Describe() => Name + "@" + At.X;
    }

    public record Empty();

    public record Plain
    {
        public int A { get; init; }
    }

    public abstract record Shape(string Tag);

    public record Circle(string Tag, float R) : Shape(Tag);

    public record Square(string Tag, float Side) : Shape(Tag)
    {
        public float Area => Side * Side;
    }

    public record Some<T>(T Value);

    public record struct Cell(int Row, int Column);

    public readonly record struct Ratio(int Numerator, int Denominator);

    public record WithDefaults(int Count = 0, string Label = "none");

    public class Program
    {
        static string Show(Shape shape) => shape switch
        {
            Circle c => "circle " + c.R,
            Square s => "square " + s.Area,
            _ => shape.Tag,
        };

        public static void Main()
        {
            var p = new Point(1, 2);
            var q = p with { Y = 9 };
            var (x, y) = q;
            p.Deconstruct(out int px, out int py);
            bool same = p == new Point(1, 2) && p != q && p.Equals(q) == false;
            int hash = p.GetHashCode();
            string text = p.ToString() + q + same + hash + x + y + px + py;

            var named = new Named("bea", p) { X2 = 6 };
            var renamed = named with { Name = "tesca", Extra = 1 };
            text += named.Describe() + renamed.Twice + named.At.X;

            var plain = new Plain { A = 4 };
            var plain2 = plain with { A = 5 };
            text += "" + plain.A + plain2.A + new Empty();

            Shape s1 = new Circle("c", 1f);
            Shape s2 = new Circle("c", 1f);
            text += (s1 == s2) + Show(s1) + Show(new Square("s", 2f)) + s1.Tag;
            if (s1 is Circle(var tag, var radius))
            {
                text += tag + radius;
            }

            var some = new Some<int>(7);
            var other = some with { Value = 8 };
            text += "" + some.Value + other.Value + (some == new Some<int>(7));

            var cell = new Cell(1, 2);
            cell.Row = 5;
            var cell2 = cell with { Column = 7 };
            var (row, column) = cell2;
            text += "" + (cell == new Cell(5, 2)) + row + column + cell.ToString();

            var ratio = new Ratio(1, 2);
            var half = ratio with { Denominator = 4 };
            text += "" + ratio.Numerator + half.Denominator + (ratio != half);

            var defaults = new WithDefaults();
            var counted = new WithDefaults(3);
            text += defaults.Label + counted.Count;

            var keys = new Dictionary<Point, string>();
            keys[new Point(1, 2)] = "a";
            text += keys[p] + keys.ContainsKey(q);

            if (q is Point(1, var second) && q is { X: 1 })
            {
                text += second;
            }
            Console.WriteLine(text);
        }
    }
}

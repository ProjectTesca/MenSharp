// MenSharp verification: records — positional properties, value equality,
// ToString, `with`, Deconstruct, inheritance. (Unity 2022.3 is C# 9: no
// `record struct`, so only record classes appear here.)
//
// Setup: a Cube "VerifyRecords" with this component. Play, click.
//
// Expected:
//   [verify-record] 1 tostring: Point { X = 1, Y = 2 } / Circle { Tag = c, R = 2.5 } / Some { Value = 7 } / Cell { Row = 1, Column = 2 }
//   [verify-record] 2 equality: True False True True True
//   [verify-record] 3 with: Point { X = 1, Y = 9 } Point { X = 1, Y = 2 } / 1,9 / Cell { Row = 5, Column = 7 }
//   [verify-record] 4 dictionary: a True circle 2.5

using System;
using System.Collections.Generic;
using MenSharp;
using UnityEngine;

namespace RecordVerify
{
    public record Point(int X, int Y);
    public abstract record Shape(string Tag);
    public record Circle(string Tag, float R) : Shape(Tag);
    public record Square(string Tag, float Side) : Shape(Tag);
    public record Some<T>(T Value);
    public record Cell(int Row, int Column);

    public class VerifyRecords : MenSharpBehaviour
    {
        private static string Show(Shape shape) => shape switch
        {
            Circle c => "circle " + c.R,
            Square s => "square " + s.Side,
            _ => shape.Tag,
        };

        public void Interact()
        {
            var p = new Point(1, 2);
            Debug.Log("[verify-record] 1 tostring: " + p + " / " + new Circle("c", 2.5f) + " / "
                + new Some<int>(7) + " / " + new Cell(1, 2));

            Shape s1 = new Circle("c", 1f);
            Shape s2 = new Circle("c", 1f);
            object boxed = p;
            Debug.Log("[verify-record] 2 equality: " + (p == new Point(1, 2)) + " " + p.Equals(new Point(1, 3))
                + " " + (p.GetHashCode() == new Point(1, 2).GetHashCode()) + " " + (s1 == s2)
                + " " + boxed.Equals(new Point(1, 2)));

            var q = p with { Y = 9 };
            var (x, y) = q;
            var cell = new Cell(1, 2);
            var cell2 = cell with { Row = 5, Column = 7 };
            Debug.Log("[verify-record] 3 with: " + q + " " + p + " / " + x + "," + y + " / " + cell2);

            var keys = new Dictionary<Point, string>();
            keys[new Point(1, 2)] = "a";
            Debug.Log("[verify-record] 4 dictionary: " + keys[p] + " " + keys.ContainsKey(q with { Y = 2 })
                + " " + Show(new Circle("c", 2.5f)));
        }
    }
}

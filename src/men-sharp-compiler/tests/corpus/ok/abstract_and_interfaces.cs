// Abstract members and interfaces: dispatch through base and interface types,
// explicit implementations, interface inheritance, generic constraints.
namespace Corpus
{
    public interface IDescribable { string Describe(); }
    public interface IShape : IDescribable
    {
        int Area();
        string Name { get; }
    }

    public abstract class Shape : IShape
    {
        public abstract int Area();
        public abstract string Name { get; }
        public virtual int Twice() => Area() * 2;
        public string Describe() => Name + Area();
    }

    public class Circle : Shape
    {
        private readonly int r;
        public Circle(int r) { this.r = r; }
        public override int Area() => 3 * r * r;
        public override string Name => "circle";
    }

    public sealed class Square : Shape
    {
        private readonly int s;
        public Square(int s) { this.s = s; }
        public override int Area() => s * s;
        public override string Name => "square";
        public override int Twice() => Area() * 2 + 1;
    }

    public struct Unit : IShape
    {
        public int Area() => 1;
        public string Name => "unit";
        string IDescribable.Describe() => "u";
    }

    public class AbstractAndInterfaces
    {
        static int Total<T>(T shape) where T : IShape => shape.Area();

        public int Run()
        {
            Shape[] shapes = new Shape[] { new Circle(2), new Square(3) };
            IShape any = shapes[0];
            IDescribable described = new Unit();
            int total = 0;
            foreach (Shape shape in shapes) { total += shape.Area() + shape.Twice() + shape.Name.Length; }
            return total + any.Area() + described.Describe().Length + Total(new Unit()) + Total(shapes[1]);
        }
    }
}

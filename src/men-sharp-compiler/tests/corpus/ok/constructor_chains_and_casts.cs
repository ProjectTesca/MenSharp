// Constructor chaining, static constructors, explicit implementations next to
// public members, object members through `object`, and runtime type tests.
namespace Corpus
{
    public interface IA { int F(); }
    public interface IB { int F(); }

    public class Base
    {
        public int v = 5;
        public int w;
        public Base() { w = F(); }
        public Base(int x) { w = x; }
        public virtual int F() => 1;
        public override string ToString() => "Base" + v;
    }

    public class Derived : Base, IA, IB
    {
        public int own = 7;
        public Derived() { }
        public Derived(int x) : base(x) { own = x * 2; }
        public Derived(int x, int y) : this(x) { own += y; }
        public override int F() => 2;
        int IA.F() => 10;
        int IB.F() => 20;
        public override bool Equals(object o) => o is Derived d && d.own == own;
        public override int GetHashCode() => own;
    }

    public class NoConstructor : Base { }

    public static class Cfg
    {
        public static int seed;
        public static int twice = Other.Value * 2;
        static Cfg() { seed = 42 + twice; }
    }

    public static class Other
    {
        public static int Value = Compute();
        static int Compute() => 10;
    }

    public struct Point { public int x; }

    public class Program
    {
        public static int total;
        public static string text;

        public static void Main()
        {
            var d = new Derived(3, 4);
            IA a = d;
            IB b = d;
            total = a.F() + b.F() + d.F() + new NoConstructor().v + Cfg.seed;
            object o = d;
            total += o.Equals(new Derived(3, 4)) ? 1 : 0;
            total += o.GetHashCode();
            text = "d=" + o + ";" + new Point { x = 1 } + ";" + (o is Derived ? "yes" : "no");
            if (o is Derived found) { total += found.own; }
            var asBase = o as Base;
            var cast = (Base)o;
            object boxed = 5;
            total += (asBase == null ? 0 : 1) + cast.w + (boxed is int ? 1 : 0);
        }
    }
}

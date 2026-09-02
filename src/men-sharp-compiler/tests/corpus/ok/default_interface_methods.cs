// Default interface methods (C# 8): bodies in interfaces, calls through
// `this`, default properties, re-implementation in a derived interface,
// structs and generic constraints.
namespace Corpus
{
    public interface IGreeter
    {
        string Name { get; }
        string Greet() => "Hi " + Name;
        int Twice(int x) => Helper(x) * 2;
        int Helper(int x) => x + 1;
        string Title => "Mx";
    }

    public interface IPolite : IGreeter
    {
        string IGreeter.Greet() => "Good day " + Name;
    }

    public class Plain : IGreeter { public string Name => "plain"; }

    public class Custom : IGreeter
    {
        public string Name => "custom";
        public string Greet() => "Yo " + Name;
        public int Helper(int x) => x + 10;
    }

    public class Polite : IPolite { public string Name => "polite"; }

    public struct Unit : IGreeter { public int n; public string Name => "unit" + n; }

    public class Program
    {
        public static string text;
        public static int total;

        public static void Main()
        {
            IGreeter a = new Plain();
            IGreeter b = new Custom();
            IGreeter c = new Polite();
            IGreeter d = new Unit { n = 3 };
            text = a.Greet() + b.Greet() + c.Greet() + d.Greet() + a.Title + Via(new Unit { n = 7 });
            total = a.Twice(1) + b.Twice(1);
        }

        static string Via<T>(T g) where T : IGreeter => g.Greet();
    }
}

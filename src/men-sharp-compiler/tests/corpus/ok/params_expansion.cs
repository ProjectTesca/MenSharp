namespace Corpus
{
    public class ParamsExpansion
    {
        private static int Sum(params int[] values)
        {
            int total = 0;
            foreach (var v in values) { total += v; }
            return total;
        }

        public int Run(int a, int b, int c)
        {
            return Sum(a, b, c);
        }
    }
}

namespace Corpus
{
    public class ParamsForms
    {
        static int Count(params int[] values) => values.Length;
        static int Count(int single) => single * 100;
        static string Line(string head, params string[] rest) => head + rest.Length;
        static int Widest<T>(params T[] items) => items.Length;

        public int Run()
        {
            return Count() + Count(7) + Count(1, 2) + Count(new int[] { 1, 2, 3 })
                + Line("h").Length + Line("h", "a", "b").Length
                + Widest(1.5, 2.5) + string.Join(",", "a", "b").Length;
        }
    }
}

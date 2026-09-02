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

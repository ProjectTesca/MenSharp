namespace Corpus
{
    public class BestCommonType
    {
        public double Run(bool flag)
        {
            var xs = new[] { 1, 2.5, 3 };
            double picked = flag ? 1 : 2.5;
            var strings = new[] { "a", "b" };
            int total = strings.Length + xs.Length;
            return picked + total;
        }
    }
}

namespace Corpus
{
    public class IsPatterns
    {
        public int Run(object value)
        {
            if (value is string s)
            {
                return s.Length;
            }
            if (value is int n && n > 0)
            {
                return n;
            }
            return 0;
        }
    }
}

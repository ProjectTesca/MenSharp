using System.Collections.Generic;

namespace Corpus
{
    public class StringInterpolation
    {
        public string Run(List<int> xs)
        {
            int count = xs.Count;
            string s = $"count={count}, first={(count > 0 ? xs[0] : -1)}";
            return s;
        }
    }
}

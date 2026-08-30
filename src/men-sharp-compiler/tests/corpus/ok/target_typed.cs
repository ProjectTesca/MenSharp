using System.Collections.Generic;

namespace Corpus
{
    public class TargetTyped
    {
        public int Run()
        {
            List<int> xs = new();
            xs.Add(3);
            Dictionary<string, List<int>> map = new();
            map["a"] = xs;
            int fallback = default;
            string missing = default;
            return xs.Count + fallback + (missing == null ? 1 : 0);
        }
    }
}

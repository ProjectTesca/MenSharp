// corpus: reference-assemblies-only — LINQ over the reference assembly's List<T>
using System.Collections.Generic;
using System.Linq;

namespace Corpus
{
    public class LinqChain
    {
        public int Run()
        {
            var names = new List<string> { "ab", "abc", "abcd" };
            var lengths = names.Where(x => x.Length > 2).Select(x => x.Length).ToArray();
            int total = lengths.Sum();
            var first = names.First();
            int firstLength = first.Length;
            var ordered = names.OrderBy(x => x.Length).ToList();
            return total + firstLength + ordered.Count;
        }
    }
}

using System.Collections.Generic;

namespace Corpus
{
    public class ForEachPatterns
    {
        public int Run(int[] numbers, List<string> names, IEnumerable<double> reals)
        {
            int total = 0;
            foreach (var n in numbers) { total += n; }
            foreach (var name in names) { total += name.Length; }
            double sum = 0;
            foreach (var r in reals) { sum += r; }
            return total + (int)sum;
        }
    }
}

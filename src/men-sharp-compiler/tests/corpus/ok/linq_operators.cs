// LINQ over the mini-corlib's own IEnumerable<T>: every operator M# ships,
// with the signatures .NET's Enumerable has — csc compiles this against the
// real System.Linq, M# against corlib/Linq.cs, and both must accept it.
using System;
using System.Collections.Generic;
using System.Linq;

namespace Corpus
{
    public class Score : IComparable<Score>
    {
        public string Player;
        public int Points;
        public float Time;
        public int CompareTo(Score other) { return Points.CompareTo(other.Points); }
    }

    public class LinqOperators
    {
        public string Run(List<Score> scores, int[] numbers, string text)
        {
            IEnumerable<int> odd = numbers.Where(n => n % 2 == 1).Select(n => n * 2);
            IEnumerable<string> names = scores.Select((s, i) => i + s.Player).Where((s, i) => i > 0);
            IEnumerable<char> letters = scores.SelectMany(s => s.Player.ToCharArray());
            IEnumerable<string> pairs = scores.SelectMany(s => s.Player.ToCharArray(), (s, c) => s.Player + c);
            IEnumerable<int> window = numbers.Skip(1).Take(3).TakeWhile(n => n > 0).SkipWhile(n => n < 0)
                .TakeWhile((n, i) => i < 5).SkipWhile((n, i) => i < 0);
            IEnumerable<int> joined = numbers.Concat(odd).Append(1).Prepend(0).Distinct().Union(odd).Intersect(odd).Except(odd)
                .Reverse().DefaultIfEmpty().DefaultIfEmpty(7);
            IEnumerable<string> zipped = numbers.Zip(scores, (n, s) => n + s.Player);
            IOrderedEnumerable<Score> ordered = scores.OrderBy(s => s.Points).ThenByDescending(s => s.Player).ThenBy(s => s.Time);
            IOrderedEnumerable<Score> reversed = scores.OrderByDescending(s => s).ThenByDescending(s => s.Time);
            IEnumerable<IGrouping<int, Score>> groups = scores.GroupBy(s => s.Points);
            IEnumerable<IGrouping<int, string>> players = scores.GroupBy(s => s.Points, s => s.Player);
            IEnumerable<string> summaries = scores.GroupBy(s => s.Points, (key, group) => key + ":" + group.Count());
            IEnumerable<int> range = Enumerable.Range(0, 3);
            IEnumerable<string> repeated = Enumerable.Repeat("x", 2);
            IEnumerable<Score> none = Enumerable.Empty<Score>();
            bool any = numbers.Any() && numbers.Any(n => n > 1) && numbers.All(n => n > 0) && numbers.Contains(1)
                && numbers.SequenceEqual(range);
            int first = numbers.First() + numbers.First(n => n > 1) + numbers.FirstOrDefault() + numbers.FirstOrDefault(n => n > 1)
                + numbers.Last() + numbers.Last(n => n > 1) + numbers.LastOrDefault() + numbers.LastOrDefault(n => n > 1)
                + numbers.Single() + numbers.Single(n => n > 1) + numbers.SingleOrDefault() + numbers.SingleOrDefault(n => n > 1)
                + numbers.ElementAt(0) + numbers.ElementAtOrDefault(9);
            int count = numbers.Count() + numbers.Count(n => n > 1);
            long longCount = numbers.LongCount() + numbers.LongCount(n => n > 1);
            int folded = numbers.Aggregate((a, b) => a + b) + numbers.Aggregate(0, (a, b) => a + b)
                + numbers.Aggregate("", (a, b) => a + b, a => a.Length);
            int sumInt = numbers.Sum() + scores.Sum(s => s.Points);
            long sumLong = numbers.Select(n => (long)n).Sum() + scores.Sum(s => (long)s.Points);
            float sumFloat = scores.Select(s => s.Time).Sum() + scores.Sum(s => s.Time);
            double sumDouble = numbers.Select(n => (double)n).Sum() + scores.Sum(s => (double)s.Points);
            double average = numbers.Average() + scores.Average(s => s.Points) + numbers.Select(n => (long)n).Average()
                + scores.Select(s => (double)s.Time).Average() + scores.Average(s => (long)s.Points) + scores.Average(s => (double)s.Points);
            float averageFloat = scores.Select(s => s.Time).Average() + scores.Average(s => s.Time);
            int minMax = numbers.Min() + numbers.Max() + scores.Min(s => s.Points) + scores.Max(s => s.Points);
            long minMaxLong = numbers.Select(n => (long)n).Min() + numbers.Select(n => (long)n).Max()
                + scores.Min(s => (long)s.Points) + scores.Max(s => (long)s.Points);
            float minMaxFloat = scores.Select(s => s.Time).Min() + scores.Select(s => s.Time).Max()
                + scores.Min(s => s.Time) + scores.Max(s => s.Time);
            double minMaxDouble = numbers.Select(n => (double)n).Min() + numbers.Select(n => (double)n).Max()
                + scores.Min(s => (double)s.Points) + scores.Max(s => (double)s.Points);
            Score best = scores.Max();
            Score worst = scores.Min();
            string longest = scores.Max(s => s.Player);
            string shortest = scores.Min(s => s.Player);
            int[] array = odd.ToArray();
            List<int> list = odd.ToList();
            Dictionary<string, Score> byName = scores.ToDictionary(s => s.Player);
            Dictionary<string, int> pointsByName = scores.ToDictionary(s => s.Player, s => s.Points);
            IEnumerable<Score> plain = scores.AsEnumerable();
            int inText = text.Count(c => c == 'a') + text.Reverse().Count() + text.Select(c => (int)c).Sum();
            string result = "";
            foreach (IGrouping<int, Score> group in groups)
            {
                result += group.Key + "=" + group.Count() + ";";
            }
            return result + any + first + count + longCount + folded + sumInt + sumLong + sumFloat + sumDouble + average + averageFloat
                + minMax + minMaxLong + minMaxFloat + minMaxDouble + best.Player + worst.Player + longest + shortest + array.Length
                + list.Count + byName.Count + pointsByName.Count + plain.Count() + inText + names.Count() + letters.Count() + pairs.Count()
                + window.Count() + joined.Count() + zipped.Count() + ordered.Count() + reversed.Count() + players.Count() + summaries.Count()
                + range.Count() + repeated.Count() + none.Count();
        }
    }
}

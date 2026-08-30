using System;
using System.Collections.Generic;

namespace Corpus
{
    public static class ListExtensions
    {
        public static T SecondOrFirst<T>(this List<T> values)
        {
            return values.Count > 1 ? values[1] : values[0];
        }

        public static TResult Pipe<T, TResult>(this T value, Func<T, TResult> f)
        {
            return f(value);
        }
    }

    public class ExtensionUse
    {
        public int Run()
        {
            var xs = new List<int> { 1, 2, 3 };
            int second = xs.SecondOrFirst();
            string text = second.Pipe(x => x.ToString());
            return text.Length;
        }
    }
}

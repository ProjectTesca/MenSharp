using System;

namespace Corpus
{
    public class LambdaInference
    {
        private static T Apply<T>(T value, Func<T, T> f) { return f(value); }
        private static TResult Map<T, TResult>(T value, Func<T, TResult> f) { return f(value); }

        public string Run()
        {
            int doubled = Apply(21, x => x * 2);
            string text = Map(doubled, x => x.ToString());
            bool flag = Map(text, t => t.Length > 1);
            return flag ? text : "small";
        }
    }
}

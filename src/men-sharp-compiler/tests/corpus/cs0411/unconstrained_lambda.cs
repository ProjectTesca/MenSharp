using System;

namespace Corpus
{
    public class UnconstrainedLambda
    {
        private static T Identity<T>(Func<T, T> f) { return f(default); }

        public void Run()
        {
            var v = Identity(x => x);
        }
    }
}

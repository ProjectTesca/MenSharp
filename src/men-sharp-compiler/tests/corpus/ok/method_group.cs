using System;

namespace Corpus
{
    public class MethodGroups
    {
        private static int Twice(int x) { return x * 2; }

        public int Run()
        {
            Func<int, int> f = Twice;
            return f(21);
        }
    }
}

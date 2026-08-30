using System;

namespace Corpus
{
    public class NaturalDelegates
    {
        public int Run()
        {
            var addOne = (int x) => x + 1;
            Func<int, int> twice = x => x * 2;
            Action<string> log = s => { };
            log("hi");
            return twice(addOne(3));
        }
    }
}

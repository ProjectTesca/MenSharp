using System;

namespace Corpus
{
    public class ActionOnly
    {
        private static void ForEach<T>(Action<T> action) { }

        public void Run()
        {
            ForEach(x => { });
        }
    }
}

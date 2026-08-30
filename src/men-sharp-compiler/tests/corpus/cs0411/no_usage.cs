namespace Corpus
{
    public class NoUsage
    {
        private static T Make<T>() { return default; }

        public void Run()
        {
            var v = Make();
        }
    }
}

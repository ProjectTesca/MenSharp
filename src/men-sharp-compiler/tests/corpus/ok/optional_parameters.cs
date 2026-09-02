// Optional parameters: omitted arguments take the declared default, named
// arguments may skip over them, and an overload needing no defaults wins.
using System;

namespace Corpus
{
    public enum Mode { Fast, Safe }

    public class OptionalParameters
    {
        static int Add(int a, int b = 2, int c = -3) => a + b + c;
        static string Tag(string text, string prefix = "#", Mode mode = Mode.Safe) => prefix + text;
        static int Pick(int a) => a;
        static int Pick(int a, int b = 0) => a + b;

        public int Run()
        {
            string[] parts = "a,b".Split(',');
            return Add(1) + Add(1, c: 5) + Tag("x").Length + Pick(1) + parts.Length
                + Math.Round(3.14159, digits: 2).GetHashCode();
        }
    }
}

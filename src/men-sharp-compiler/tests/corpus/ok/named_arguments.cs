using System;

namespace Corpus
{
    public class NamedArguments
    {
        public int Run(int value)
        {
            return Math.Clamp(value: value, min: 0, max: 10);
        }
    }
}

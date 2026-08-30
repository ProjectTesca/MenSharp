namespace Corpus
{
    public class OutVar
    {
        public int Run(string input)
        {
            if (int.TryParse(input, out var parsed))
            {
                return parsed * 2;
            }
            int.TryParse("7", out int seven);
            return seven;
        }
    }
}

namespace Corpus
{
    public class SwitchExpression
    {
        public string Run(int code)
        {
            var label = code switch
            {
                0 => "zero",
                1 => "one",
                _ => "many",
            };
            double size = code switch
            {
                0 => 0,
                _ => 1.5,
            };
            return label + size.ToString();
        }
    }
}

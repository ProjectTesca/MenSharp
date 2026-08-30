namespace Corpus
{
    public class NumericPromotion
    {
        public double Run()
        {
            int i = 3;
            long l = i + 4L;
            float f = i * 0.5f;
            double d = l + f;
            byte b = 200;
            int promoted = b + 55;
            bool cmp = d > promoted;
            return cmp ? d : promoted;
        }
    }
}

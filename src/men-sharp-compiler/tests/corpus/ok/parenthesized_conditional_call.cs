// A call with arguments inside a parenthesised conditional: `(F(1, 1) ? a : b)`
// must not be read as the type `F` declaring a broken designation `(1`.
public class Program
{
    static bool F(int a, int b) => a == b;
    static bool H(int a) => a > 0;
    static bool G() => true;

    public static int Main()
    {
        int r = 1 + (F(1, 1) ? 1 : 0) + (H(1) ? 10 : 0) + (G() ? 100 : 0);
        bool t = (F(1, 1) ? true : false);
        return r + (t ? 1 : 0);
    }
}

// A library in an assembly definition of its own — auto-referenced, so
// Assembly-CSharp (and the MenSharp behaviours in it) can use it. MenSharp
// must read it as a library source: it is a source of a Player assembly
// some Player assembly references.
public static class Toolbox
{
    public static int Twice(int value)
    {
        return value * 2;
    }
}

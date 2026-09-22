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

// NDMF and Modular Avatar's shape: a generic class with a static constructor,
// in a library M# only reads for its declarations. M# never uses it — and it
// used to fail the whole compilation just by existing (issue). A static
// constructor in a library is another compiler's body, never this one's to run.
public class ToolboxCache<T>
{
    public static int Count;

    static ToolboxCache()
    {
        Count = 1;
    }
}

public static class ToolboxSeed
{
    public static int Value;

    static ToolboxSeed()
    {
        Value = 12345;
    }
}

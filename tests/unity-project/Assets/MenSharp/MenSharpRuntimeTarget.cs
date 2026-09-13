using MenSharp;

public class MenSharpRuntimeTarget : MenSharpBehaviour
{
    // written from another behaviour via SetProgramVariable("received", int[])
    public int[] received;

    public int Add(int left, int right)
    {
        return left + right;
    }
}

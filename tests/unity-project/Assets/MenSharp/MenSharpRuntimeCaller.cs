using MenSharp;
using VRC.Udon;

public class MenSharpRuntimeCaller : MenSharpBehaviour
{
    public MenSharpRuntimeTarget target;
    public bool done;
    public int result;

    // SetProgramVariable<T> is C# sugar over the non-generic (string, object)
    // extern; passing an int[] used to fail because M# tried to name int[] as a
    // System.Type. Set on another behaviour, read back by the test.
    public UdonBehaviour receiver;

    public void RunRemote()
    {
        result = target.Add(20, 22);
        done = true;
    }

    public void PushArray()
    {
        int[] values = { 7, 8, 9 };
        receiver.SetProgramVariable("received", values);
    }
}

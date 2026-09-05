using MenSharp;

public class MenSharpRuntimeCaller : MenSharpBehaviour
{
    public MenSharpRuntimeTarget target;
    public bool done;
    public int result;

    public void RunRemote()
    {
        result = target.Add(20, 22);
        done = true;
    }
}

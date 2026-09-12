using System;
using System.Collections.Generic;
using System.Linq;
using MenSharp;

// A deliberately small black-box test program. The Unity test runner invokes
// its exported events through the SDK's real UdonBehaviour and reads these
// public variables back from the Udon heap.
public class MenSharpRuntimeSmoke : MenSharpBehaviour
{
    public bool syncDone;
    public int syncResult;
    public string syncText;

    public bool asyncDone;
    public int asyncResult;

    // one count for every instance of this behaviour
    public static int shared;
    public int mine;

    public void RunShared()
    {
        shared++;
        mine = shared;
    }

    // a delegate of one instance, invoked by another: the invoker hands it
    // back to the program that made it
    public static event Action<int> OnPublished;
    public int toPublish;
    public int heard;

    public void Subscribe()
    {
        OnPublished += value => { heard = value * 10; };
    }

    public void Publish()
    {
        OnPublished?.Invoke(toPublish);
    }

    public void RunSync()
    {
        var values = new List<int> { 1, 2, 3, 4 };
        syncResult = values.Where(value => value % 2 == 0).Sum();
        syncText = $"sum={syncResult}";
        syncDone = true;
    }

    public async void RunAsync()
    {
        await Scheduler.NextFrame();
        asyncResult = 42;
        asyncDone = true;
    }
}

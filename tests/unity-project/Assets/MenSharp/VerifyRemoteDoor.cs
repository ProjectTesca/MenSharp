// MenSharp verification: the behaviour VerifyRemoteAwait awaits.
//
// Setup: a Cube "VerifyRemoteDoor" with this component, dragged into the
// VerifyRemoteAwait component's `door` field.

using System;
using System.Threading.Tasks;
using MenSharp;
using UnityEngine;

public class VerifyRemoteDoor : MenSharpBehaviour
{
    public string Log = "";
    public int Opened;

    public async Task<int> Open(int by)
    {
        Log += "start" + by + ";";
        await Scheduler.Delay(1f);
        Opened += by;
        Log += "opened;";
        return by * 2;
    }

    public async Task Stick()
    {
        await Scheduler.NextFrame();
        throw new InvalidOperationException("stuck");
    }

    public Task<int> Ready(int n)
    {
        return Task.FromResult(n);
    }
}

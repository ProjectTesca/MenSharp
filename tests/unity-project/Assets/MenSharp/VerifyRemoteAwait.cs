// MenSharp verification: awaiting another behaviour's task.
//
// Setup: a Cube "VerifyRemoteAwait" with this component, and a second Cube
// "VerifyRemoteDoor" with VerifyRemoteDoor on it dragged into `door`.
// Play, click, wait about four seconds.
//
// Expected:
//   [verify-remote] 1 called: door log start21; and we are still waiting True
//   [verify-remote] 2 ready: an already finished remote task gives 9
//   [verify-remote] 3 opened: got 42, door log start21;opened;, door counted 21
//   [verify-remote] 4 faulted: caught RemoteTaskException, text mentions stuck True
//   [verify-remote] 5 both: two doors at once gave 2 and 4
//   [verify-remote] 6 end: run 1
//   [verify-remote] 7 report: trace=start;ready9;opened42;caught;both;  door task done True  door log start1;start2;opened;opened;
//
// Line 7 prints whatever actually happened, so a run that stops early still
// says where: `door task done` False means the door never finished its own
// work, True means it finished and the continuation did not come home.

using System;
using System.Threading.Tasks;
using MenSharp;
using UnityEngine;

public class VerifyRemoteAwait : MenSharpBehaviour
{
    public VerifyRemoteDoor door;
    private int runs;
    private string trace = "";
    private Task<int> opening;

    public void Interact()
    {
        runs++;
        door.Log = "";
        door.Opened = 0;
        trace = "";
        Watch(runs);
        Report();
    }

    private async void Watch(int run)
    {
        trace += "start;";
        opening = door.Open(21);
        Debug.Log($"[verify-remote] 1 called: door log {door.Log} and we are still waiting {!opening.IsCompleted}");

        int nine = await door.Ready(9);
        trace += "ready" + nine + ";";
        Debug.Log($"[verify-remote] 2 ready: an already finished remote task gives {nine}");

        int opened = await opening;
        trace += "opened" + opened + ";";
        Debug.Log($"[verify-remote] 3 opened: got {opened}, door log {door.Log}, door counted {door.Opened}");

        string caught = "not caught";
        bool mentions = false;
        try { await door.Stick(); }
        catch (RemoteTaskException e)
        {
            caught = "caught RemoteTaskException";
            mentions = e.Message.Contains("stuck");
        }
        trace += "caught;";
        Debug.Log($"[verify-remote] 4 faulted: {caught}, text mentions stuck {mentions}");

        door.Log = "";
        Task<int> a = door.Open(1);
        Task<int> b = door.Open(2);
        await Task.WhenAll(a, b);
        trace += "both;";
        Debug.Log($"[verify-remote] 5 both: two doors at once gave {a.Result} and {b.Result}");

        Debug.Log($"[verify-remote] 6 end: run {run}");
    }

    // Runs whatever happened above, so a run that stops early still reports
    // where it stopped. Its own delay uses this behaviour's scheduler, so
    // seeing this line at all proves the local one works.
    private async void Report()
    {
        await Scheduler.Delay(4f);
        Debug.Log($"[verify-remote] 7 report: trace={trace}  door task done {opening.IsCompleted}  door log {door.Log}");
    }
}

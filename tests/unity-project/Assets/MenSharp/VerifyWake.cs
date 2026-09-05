// MenSharp verification: the smallest possible wait, from a plain event.
//
// Setup: a Cube "VerifyWake" with this component. Play, click ONCE and wait
// three seconds. Do not click again.
//
// Expected:
//   [verify-wake] 0 click
//   [verify-wake] 1 one second later
//   [verify-wake] 2 next frame
//   [verify-wake] 3 two seconds later
//
// If nothing after line 0 appears until a second click, a behaviour is not
// being woken at all. If some appear and others do not, only some waits are
// being swept.

using System.Threading.Tasks;
using MenSharp;
using UnityEngine;

public class VerifyWake : MenSharpBehaviour
{
    public void Interact()
    {
        Debug.Log("[verify-wake] 0 click");
        OneSecond();
        NextFrame();
        TwoSeconds();
    }

    private async void OneSecond()
    {
        await Scheduler.Delay(1f);
        Debug.Log("[verify-wake] 1 one second later");
    }

    private async void NextFrame()
    {
        await Scheduler.NextFrame();
        Debug.Log("[verify-wake] 2 next frame");
    }

    private async void TwoSeconds()
    {
        await Scheduler.Delay(2f);
        Debug.Log("[verify-wake] 3 two seconds later");
    }
}

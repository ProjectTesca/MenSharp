// MenSharp verification: [NetworkCallable] — custom events with arguments
// over the network (SDK 3.7+), sent with SendCustomNetworkEvent to this
// behaviour and to another one.
//
// Setup: a Cube "VerifyNetwork" with this component; drag the "Target"
// object (VerifyTarget) into `target`. Both behaviours are Manual sync (a
// network event needs a sync mode). Play, click.
//
// Expected (ClientSim delivers network events to yourself):
//   [verify-net] 1 sent Hit(3, "me") to All
//   [verify-net] Hit ran: damage=3, by=me, hits=3
//   [verify-net] 2 sent Aim((1,2,3), 4) to Owner
//   [verify-net] Aim ran: at=(1.00, 2.00, 3.00), times=4
//   [verify-net] 3 sent Pinged(7) to the target
//   [verify] target: Pinged(7) ran                       (logged by VerifyTarget)

using MenSharp;
using UnityEngine;
using VRC.SDK3.UdonNetworkCalling;
using VRC.Udon.Common.Interfaces;

[UdonBehaviourSyncMode(BehaviourSyncMode.Manual)]
public class VerifyNetwork : MenSharpBehaviour
{
    public VerifyTarget target;
    public int hits;

    [NetworkCallable]
    public void Hit(int damage, string by)
    {
        hits += damage;
        Debug.Log($"[verify-net] Hit ran: damage={damage}, by={by}, hits={hits}");
    }

    [NetworkCallable(5)]
    public void Aim(Vector3 at, int times)
    {
        Debug.Log($"[verify-net] Aim ran: at={at}, times={times}");
    }

    public void Interact()
    {
        SendCustomNetworkEvent(NetworkEventTarget.All, nameof(Hit), 3, "me");
        Debug.Log("[verify-net] 1 sent Hit(3, \"me\") to All");
        SendCustomNetworkEvent(NetworkEventTarget.Owner, nameof(Aim), new Vector3(1, 2, 3), 4);
        Debug.Log("[verify-net] 2 sent Aim((1,2,3), 4) to Owner");
        target.SendCustomNetworkEvent(NetworkEventTarget.All, nameof(VerifyTarget.Pinged), 7);
        Debug.Log("[verify-net] 3 sent Pinged(7) to the target");
    }
}

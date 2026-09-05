// MenSharp verification: GetComponent<T>() for behaviour types — MenSharp's
// own and UdonSharp's — found by their program identity, not by engine type.
//
// Setup: a Cube "VerifyGetComponent" with THREE components: this one, a
// VerifyTarget (MenSharp) and a UCounter (UdonSharp, from
// Assets/MenSharpTestUSharp; it needs its program compiled, see UCounter.cs).
// Optionally a child GameObject with another VerifyTarget. Play, click.
//
// Expected:
//   [verify-gc] 1 GetComponent<VerifyTarget>() found = True, label = target
//   [verify-gc] 2 GetComponents<VerifyTarget>().Length = 1
//   [verify-gc] 3 GetComponent<UCounter>() found = True, then Bump(1) ran on the UdonSharp side
//   [verify-gc] 4 TryGetComponent<UCounter> = True
//   [verify-gc] 5 GetComponentInChildren<VerifyTarget>(true).Length = 1 (2 with the child)
//   [verify-gc] 6 GetComponent<VerifyCrash>() = null   (expect: True — not on this object)
//   [verify-gc] 7 target.poked after Poke() through the found reference = 1 (or more)

using MenSharp;
using UnityEngine;

public class VerifyGetComponent : MenSharpBehaviour
{
    public void Interact()
    {
        VerifyTarget target = GetComponent<VerifyTarget>();
        Debug.Log($"[verify-gc] 1 GetComponent<VerifyTarget>() found = {target != null}, label = {(target != null ? target.label : "-")}");
        VerifyTarget[] targets = gameObject.GetComponents<VerifyTarget>();
        Debug.Log($"[verify-gc] 2 GetComponents<VerifyTarget>().Length = {targets.Length}");
        UCounter counter = GetComponent<UCounter>();
        Debug.Log($"[verify-gc] 3 GetComponent<UCounter>() found = {counter != null}");
        if (counter != null) { counter.Bump(1); }
        bool tried = TryGetComponent<UCounter>(out UCounter viaTry);
        Debug.Log($"[verify-gc] 4 TryGetComponent<UCounter> = {tried && viaTry != null}");
        VerifyTarget[] inChildren = gameObject.GetComponentsInChildren<VerifyTarget>(true);
        Debug.Log($"[verify-gc] 5 GetComponentsInChildren<VerifyTarget>(true).Length = {inChildren.Length}");
        VerifyCrash absent = GetComponent<VerifyCrash>();
        Debug.Log($"[verify-gc] 6 GetComponent<VerifyCrash>() = null   (expect: True)   {absent == null}");
        if (target != null)
        {
            target.Poke();
            Debug.Log($"[verify-gc] 7 target.poked after Poke() through the found reference = {target.poked}");
        }
    }
}

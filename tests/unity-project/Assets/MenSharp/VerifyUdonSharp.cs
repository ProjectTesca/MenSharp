// MenSharp verification: talking to an *UdonSharp* behaviour, typed.
//
// UCounter (Assets/MenSharpTestUSharp/UCounter.cs) is compiled by UdonSharp.
// This script reads its fields, writes them, calls its methods with arguments
// and results, and uses its properties — all through the names UdonSharp
// exported them under, so nothing on the UdonSharp side changes.
//
// Setup: a GameObject "UCounter" with the UCounter component (UdonSharp),
// and a Cube "VerifyUdonSharp" with this component; drag the UCounter object
// into the `counter` field. Play, click the cube.
//
// Expected, in order:
//   [verify-u#] 1 counter.label = u#, count = 0, mode = Slow
//   [verify-u#] Bump(3) ran, count=3                 (logged by UdonSharp's code)
//   [verify-u#] 2 after Bump(3), counter.count = 3
//   [verify-u#] 3 counter.Add(20, 22) = 42
//   [verify-u#] 4 counter.Take(1, out rest) = True, rest = 2
//   [verify-u#] Level setter ran, level=9            (logged by UdonSharp's code)
//   [verify-u#] 5 counter.Level = 9, counter.Doubled = 6
//   [verify-u#] 6 counter.Describe() = u#:3:Fast
//   [verify-u#] Interact on the UdonSharp side: count=3, level=9, mode=1
//   [verify-u#] 7 UCounterMath.Triple(4) = 12

using MenSharp;
using UnityEngine;

public class VerifyUdonSharp : MenSharpBehaviour
{
    public UCounter counter;

    public void Interact()
    {
        counter.count = 0;
        counter.mode = CounterMode.Slow;
        Debug.Log($"[verify-u#] 1 counter.label = {counter.label}, count = {counter.count}, mode = {counter.mode}");
        counter.Bump(3);
        Debug.Log($"[verify-u#] 2 after Bump(3), counter.count = {counter.count}");
        Debug.Log($"[verify-u#] 3 counter.Add(20, 22) = {counter.Add(20, 22)}");
        bool ok = counter.Take(1, out int rest);
        Debug.Log($"[verify-u#] 4 counter.Take(1, out rest) = {ok}, rest = {rest}");
        counter.Level = 9;
        Debug.Log($"[verify-u#] 5 counter.Level = {counter.Level}, counter.Doubled = {counter.Doubled}");
        counter.mode = CounterMode.Fast;
        Debug.Log($"[verify-u#] 6 counter.Describe() = {counter.Describe()}");
        counter.Interact();   // the built-in event, raised on the other program
        Debug.Log($"[verify-u#] 7 UCounterMath.Triple(4) = {UCounterMath.Triple(4)}   (expect: 12; a U# helper class compiled into this program)");
    }
}

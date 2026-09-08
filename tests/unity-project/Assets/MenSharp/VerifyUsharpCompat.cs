// MenSharp verification: two things UdonSharp code does all the time.
//
//   - integer constant expressions narrow like literals: `sbyte s = 3 - 5;`
//   - `foreach (Transform child in transform)` walks the children by index
//
// Setup: a Cube "VerifyUsharpCompat" with this component. Play, click.
//
// Expected:
//   [verify-usharp] 1 constants: -2 6 16 250
//   [verify-usharp] 2 children: 0 of 0

using MenSharp;
using UnityEngine;

public class VerifyUsharpCompat : MenSharpBehaviour
{
    public void Interact()
    {
        sbyte a = 3 - 5;
        byte b = 20 / 3;
        short c = -(-(1 << 4));
        byte d = (0xFF & 250) | 0;
        Debug.Log($"[verify-usharp] 1 constants: {a} {b} {c} {d}");

        int seen = 0;
        foreach (Transform child in transform)
        {
            if (child != null)
            {
                seen++;
            }
        }
        Debug.Log($"[verify-usharp] 2 children: {seen} of {transform.childCount}");
    }
}

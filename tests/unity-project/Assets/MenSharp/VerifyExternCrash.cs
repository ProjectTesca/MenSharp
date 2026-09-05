// MenSharp verification: an exception thrown *inside* an engine/.NET call
// cannot be caught (the VM halts the behaviour), but the editor must say
// where it happened.
//
// Setup: make a GameObject "VerifyExternCrash" (a Cube, for the collider
// Interact needs) and add the VerifyExternCrash component. In play mode,
// click it.
//
// Expected in the Console:
//   [verify-extern-crash] 1 before the call
//   the VM's own report (An exception occurred during Udon execution ... FormatException ...)
//   MenSharp: the Udon VM halted inside `SystemInt32.Parse`, called from VerifyExternCrash.Interact
//       at Assets/MenSharp/VerifyExternCrash.cs:<the int.Parse line>:<col>
// and *no* "[verify-extern-crash] 2 ..." line. Note the try/catch around the
// call does not fire: that is Udon's rule for extern exceptions.

using System;
using MenSharp;
using UnityEngine;

public class VerifyExternCrash : MenSharpBehaviour
{
    public void Interact()
    {
        Debug.Log("[verify-extern-crash] 1 before the call");
        try
        {
            int parsed = int.Parse("not a number");   // FormatException inside the extern: VM halts here
            Debug.Log($"[verify-extern-crash] 2 THIS MUST NOT APPEAR: {parsed}");
        }
        catch (Exception e)
        {
            Debug.Log($"[verify-extern-crash] 2 THIS MUST NOT APPEAR EITHER: caught {e}");
        }
    }
}

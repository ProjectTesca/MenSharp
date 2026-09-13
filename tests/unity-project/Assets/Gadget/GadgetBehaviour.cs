// A behaviour living outside Assets/MenSharp, in a folder marked with a
// `.mensharp` file. It exists to prove the marker path end to end: MenSharp
// compiles it, and its program lands under this folder's own Programs. Kept to
// plain C# so UdonSharp's scanner never trips over it even for the instant
// before isolation registers the folder.

using MenSharp;
using UnityEngine;

public class GadgetBehaviour : MenSharpBehaviour
{
    public int doubled;

    public void Double()
    {
        doubled = 21 * 2;
        Debug.Log("[gadget] doubled: " + doubled);
    }
}

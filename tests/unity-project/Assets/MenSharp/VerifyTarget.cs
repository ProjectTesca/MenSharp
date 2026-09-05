// MenSharp verification: the *other* behaviour.
//
// Put this at Assets/MenSharp/VerifyTarget.cs and attach it to its own
// GameObject (call it "Target"). Verify.cs talks to it across the program
// boundary; everything it prints is the far side of a cross-behaviour call.

using MenSharp;
using UnityEngine;
using VRC.SDK3.UdonNetworkCalling;

[UdonBehaviourSyncMode(BehaviourSyncMode.Manual)]
public class VerifyTarget : MenSharpBehaviour
{
    // 22. a network callable on the far side: VerifyNetwork sends Pinged(7)
    [NetworkCallable]
    public void Pinged(int n)
    {
        Debug.Log($"[verify] target: Pinged({n}) ran");
    }

    // read and written from the other behaviour by name
    public int poked;
    public string label = "target";

    // 11. serialized private field: shows in the Inspector and its value
    //     arrives in the program without being public
    [SerializeField] private int secret = 5;

    // 12. external writes to `dial` (SetProgramVariable from Verify, or
    //     network sync) run the Dial setter instead of landing silently
    [FieldChangeCallback(nameof(Dial))] public int dial;
    public int dialChanges;

    public int Dial
    {
        get { return dial; }
        set
        {
            dialChanges = dialChanges + 1;
            Debug.Log($"[verify] 12b Dial setter ran: {dial} -> {value} (change #{dialChanges})");
            dial = value;
        }
    }

    public void Start()
    {
        Debug.Log($"[verify] 11 [SerializeField] secret = {secret}   (expect: 5, or the Inspector value)");
    }

    // raised from the other behaviour as a custom event
    public void Poke()
    {
        poked += 1;
        Debug.Log($"[verify] target: Poke() ran, poked={poked}");
    }

    // 21. calls with arguments and results across the program boundary:
    //     UdonSharp's protocol (parameter variables + event + result variable)
    public void Slide(int amount)
    {
        poked += amount;
        Debug.Log($"[verify] target: Slide({amount}) ran, poked={poked}");
    }

    public int Add(int a, int b)
    {
        return a + b;
    }

    public bool Take(int amount, out int rest)
    {
        rest = poked - amount;
        return rest >= 0;
    }

    public string Describe(string prefix, int times)
    {
        string s = prefix;
        for (int i = 1; i < times; i++) { s = s + prefix; }
        return s;
    }

    // a property with a body: reached through its accessor events
    private int level;
    public int Level
    {
        get { return level; }
        set
        {
            level = value;
            Debug.Log($"[verify] target: Level setter ran, level={level}");
        }
    }
}

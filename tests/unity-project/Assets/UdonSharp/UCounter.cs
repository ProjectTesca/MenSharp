// MenSharp verification: an *UdonSharp* behaviour that an M# behaviour
// (Assets/MenSharp/VerifyUdonSharp.cs) talks to. UdonSharp compiles this one;
// MenSharp only reads its declarations.
//
// Setup: this folder has its own assembly definition (MenSharpTestUSharp),
// referenced from Assets/MenSharp/MenSharp.Scripts.asmdef, so the M# script
// can name the class. Two things UdonSharp needs on its side:
//   1. UdonSharp only compiles scripts of Assembly-CSharp and of assemblies
//      that have a *U# Assembly Definition* asset: select
//      MenSharpTestUSharp.asmdef in the Project window, right-click,
//      Create > U# Assembly Definition (this creates MenSharpTestUSharp.asset
//      pointing at the asmdef). Without it UdonSharp never compiles UCounter.
//   2. a program asset per script — UCounter.asset beside this file (a script
//      created via Create > U# Script gets one automatically).
// Then make a GameObject "UCounter", add this component, and follow the setup
// in VerifyUdonSharp.cs. UdonSharp logs "Compile of N scripts finished" once
// it has compiled.

using UdonSharp;
using UnityEngine;

public enum CounterMode
{
    Slow,
    Fast,
}



// a helper beside the behaviour: UdonSharp inlines it into UCounter where
// used; MenSharp compiles it into the M# program that calls it
public static class UCounterMath
{
    public static int Triple(int x)
    {
        return x * 3;
    }
}

public class UCounter : UdonSharpBehaviour
{
    public int count;
    public CounterMode mode = CounterMode.Slow;
    public string label = "u#";

    private int level;

    public int Level
    {
        get { return level; }
        set
        {
            level = value;
            Debug.Log($"[verify-u#] Level setter ran, level={level}");
        }
    }

    public int Doubled
    {
        get { return count * 2; }
    }

    public void Bump(int by)
    {
        count += by;
        Debug.Log($"[verify-u#] Bump({by}) ran, count={count}");
    }

    public int Add(int a, int b)
    {
        return a + b;
    }

    public bool Take(int amount, out int rest)
    {
        rest = count - amount;
        return rest >= 0;
    }

    public string Describe()
    {
        return label + ":" + count + ":" + mode;
    }

    public override void Interact()
    {
        Debug.Log($"[verify-u#] Interact on the UdonSharp side: count={count}, level={level}, mode={mode}");
    }
}

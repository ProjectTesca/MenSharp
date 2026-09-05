// MenSharp verification: the array-initializer shorthand `= { 1, 2 }`.
//
// Setup: a Cube "VerifyArrays" with this component. Play, click.
//
// Expected:
//   [verify-arrays] 1 local: {10,20,30} length=3, sum=60, empty length=0
//   [verify-arrays] 2 behaviour field: names={a,b}, levels sum=6
//   [verify-arrays] 3 class field: {1,2,3} length=3, empty length=0
//   [verify-arrays] 4 conversions: floats={1, 2.5}, longs sum=3, strings={x,y}
//   [verify-arrays] 5 same as new[]: True, writes stick: {10,99,30}

using MenSharp;
using UnityEngine;

public class ArHolder
{
    public int[] numbers = { 1, 2, 3 };
    public int[] empty = { };
}

public class VerifyArrays : MenSharpBehaviour
{
    public string[] names = { "a", "b" };
    public int[] levels = { 1, 2, 3 };

    private string Join(int[] values)
    {
        string text = "";
        for (int i = 0; i < values.Length; i++)
        {
            if (i > 0) text += ",";
            text += values[i];
        }
        return "{" + text + "}";
    }

    public void Interact()
    {
        int[] local = { 10, 20, 30 };
        int[] empty = { };
        int sum = 0;
        foreach (int value in local) sum += value;
        Debug.Log($"[verify-arrays] 1 local: {Join(local)} length={local.Length}, sum={sum}, empty length={empty.Length}");

        int levelSum = 0;
        foreach (int level in levels) levelSum += level;
        Debug.Log($"[verify-arrays] 2 behaviour field: names={{{names[0]},{names[1]}}}, levels sum={levelSum}");

        var holder = new ArHolder();
        Debug.Log($"[verify-arrays] 3 class field: {Join(holder.numbers)} length={holder.numbers.Length}, empty length={holder.empty.Length}");

        float[] floats = { 1, 2.5f };
        long[] longs = { 1, 2 };
        string[] strings = { "x", "y" };
        long longSum = longs[0] + longs[1];
        Debug.Log($"[verify-arrays] 4 conversions: floats={{{floats[0]}, {floats[1]}}}, longs sum={longSum}, strings={{{strings[0]},{strings[1]}}}");

        int[] written = new int[] { 10, 20, 30 };
        bool same = written[0] == local[0] && written[2] == local[2] && written.Length == local.Length;
        local[1] = 99;
        Debug.Log($"[verify-arrays] 5 same as new[]: {same}, writes stick: {Join(local)}");
    }
}

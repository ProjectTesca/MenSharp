// MenSharp verification: target-typed `new()` and `??=`.
//
// Setup: a Cube "VerifyModern" with this component. Play, click.
//
// Expected:
//   [verify-modern] 1 new(): a.Count=1, b.Count=5, field=1, made=7, list={1,2,3}
//   [verify-modern] 2 new() struct: p.X=9, maybe.X=2, map["k"]=5
//   [verify-modern] 3 ??=: made once ('made'), a.Count=7, name=anon, count=5
//   [verify-modern] 4 ??= places: array[0].Count=1, dict k=1, field kept=3

using System.Collections.Generic;
using MenSharp;
using UnityEngine;

public class MdCounter
{
    public int Count;
    public MdCounter() { Count = 1; }
    public MdCounter(int start) { Count = start; }
}

public struct MdPoint
{
    public int X;
    public MdPoint(int x) { X = x; }
}

public class VerifyModern : MenSharpBehaviour
{
    private MdCounter field = new();
    private MdCounter kept = new(3);
    private string log = "";

    private MdCounter Made()
    {
        log += "made";
        return new MdCounter(7);
    }

    private MdCounter Make() => new(7);

    public void Interact()
    {
        MdCounter a = new();
        MdCounter b = new(5);
        List<int> values = new() { 1, 2, 3 };
        Debug.Log($"[verify-modern] 1 new(): a.Count={a.Count}, b.Count={b.Count}, field={field.Count}, made={Make().Count}, list={{{values[0]},{values[1]},{values[2]}}}");

        MdPoint p = new(9);
        MdPoint? maybe = new(2);
        Dictionary<string, int> map = new();
        map["k"] = 5;
        Debug.Log($"[verify-modern] 2 new() struct: p.X={p.X}, maybe.X={maybe.Value.X}, map k={map["k"]}");

        MdCounter target = null;
        target ??= Made();
        int firstCount = target.Count;
        target ??= Made();          // must not run a second time
        string name = null;
        name ??= "anon";
        name ??= "other";
        int? count = null;
        count ??= 5;
        count ??= 9;
        Debug.Log($"[verify-modern] 3 ??=: made once ('{log}'), a.Count={firstCount}, name={name}, count={count.Value}");

        MdCounter[] boxes = new MdCounter[1];
        boxes[0] ??= new MdCounter();
        var boxMap = new Dictionary<string, MdCounter>();
        boxMap["k"] = null;
        boxMap["k"] ??= new MdCounter();
        kept ??= new MdCounter(99);
        Debug.Log($"[verify-modern] 4 ??= places: array[0].Count={boxes[0].Count}, dict k={boxMap["k"].Count}, field kept={kept.Count}");
    }
}

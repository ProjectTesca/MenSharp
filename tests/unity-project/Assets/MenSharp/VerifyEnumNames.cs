// MenSharp verification: enum values print their names.
//
// Setup: a Cube "VerifyEnumNames" with this component. Play, click.
//
// Expected:
//   [verify-enum] 1 names: High High Low Mid 9
//   [verify-enum] 2 interpolated: state=High next=Mid generic High;3;
//   [verify-enum] 3 flags: Front, Side / None / Both / Both, Side / 8
//   [verify-enum] 4 sorted: Low,Mid,High max High boxed 6

using System;
using System.Collections.Generic;
using System.Linq;
using MenSharp;
using UnityEngine;

public class VerifyEnumNames : MenSharpBehaviour
{
    public enum Rank { Low, Mid = 5, High, Peak = 5 }

    [Flags]
    public enum Doors { None = 0, Front = 1, Back = 2, Side = 4, Both = 3 }

    private string Show<T>(T item) { return item + ";"; }

    private string Join<T>(IEnumerable<T> items)
    {
        string text = "";
        foreach (T item in items) { text += item + ","; }
        return text.TrimEnd(',');
    }

    public void Interact()
    {
        Rank r = Rank.High;
        Debug.Log($"[verify-enum] 1 names: {r} {r.ToString()} {Rank.Low} {Rank.Peak} {(Rank)9}");
        Debug.Log("[verify-enum] 2 interpolated: " + $"state={r} next={Rank.Mid}" + " generic " + Show(r) + Show(3));
        Doors d = Doors.Front | Doors.Side;
        Debug.Log($"[verify-enum] 3 flags: {d} / {Doors.None} / {(Doors)3} / {(Doors)7} / {(Doors)8}");
        Rank[] ranks = new Rank[] { Rank.High, Rank.Low, Rank.Mid };
        object boxed = r;
        Debug.Log($"[verify-enum] 4 sorted: {Join(ranks.OrderBy(x => x))} max {ranks.Max()} boxed {boxed}");
    }
}

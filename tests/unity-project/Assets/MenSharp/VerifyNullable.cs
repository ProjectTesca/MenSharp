// MenSharp verification: Nullable<T> — int?, Vector3?, struct?.
//
// Setup: a Cube "VerifyNullable" with this component. Play, click.
//
// Expected:
//   [verify-nullable] 1 members: a.HasValue=False, b.HasValue=True, b.Value=5, a??100=100, a.GetValueOrDefault(10)=10
//   [verify-nullable] 2 lifted: a+b null=True, b+1=6, a==null=True, b==5=True, a<3=False, b>3=True, -b=-5
//   [verify-nullable] 3 mutation: b++ -> 6, b+=10 -> 16, (int)b=16, long? wide==16: True
//   [verify-nullable] 4 strings: '' '16' '|' 'x=16' 'x='
//   [verify-nullable] 5 boxing: is int v -> 16, (int?)boxed -> 16, switch: N S, patterns: WZ
//   [verify-nullable] 6 struct?: p==null True, q.Value+(3,4)=(4,6), (p??q).Value.X=1, Vector3? v=(1.0, 2.0, 3.0)
//   [verify-nullable] 7 ?.: none?.Count null=True, some?.Count=3, some.Next?.Count ?? 7 = 7
//   [verify-nullable] 8 arrays/default: def==null True, arr[0]==null True, arr[1]=9
//   [verify-nullable] 9 a.Value threw InvalidOperationException; null==null True; null!=b True

using System;
using MenSharp;
using UnityEngine;

public struct NlPoint
{
    public int X;
    public int Y;
    public NlPoint(int x, int y) { X = x; Y = y; }
    public static NlPoint operator +(NlPoint a, NlPoint b) { return new NlPoint(a.X + b.X, a.Y + b.Y); }
    public override string ToString() { return "(" + X + "," + Y + ")"; }
}

public class NlBox
{
    public int Count = 3;
    public NlBox Next;
}

public class VerifyNullable : MenSharpBehaviour
{
    public int? stored;

    private static int? Twice(int? v) { return v * 2; }

    public void Interact()
    {
        int? a = null;
        int? b = 5;
        Debug.Log($"[verify-nullable] 1 members: a.HasValue={a.HasValue}, b.HasValue={b.HasValue}, b.Value={b.Value}, a??100={a ?? 100}, a.GetValueOrDefault(10)={a.GetValueOrDefault(10)}");

        int? c = a + b;
        int? d = b + 1;
        int? e = -b;
        Debug.Log($"[verify-nullable] 2 lifted: a+b null={c == null}, b+1={d}, a==null={a == null}, b==5={b == 5}, a<3={a < 3}, b>3={b > 3}, -b={e}");

        b++;
        int afterIncrement = b.Value;
        b += 10;
        long? wide = b;
        Debug.Log($"[verify-nullable] 3 mutation: b++ -> {afterIncrement}, b+=10 -> {b}, (int)b={(int)b}, long? wide==16: {wide == 16}");

        string s1 = a.ToString();
        string s2 = b.ToString();
        string s3 = "|" + a;
        string s4 = "x=" + Twice(8);
        string s5 = "x=" + Twice(a);
        Debug.Log($"[verify-nullable] 4 strings: '{s1}' '{s2}' '{s3}' '{s4}' '{s5}'");

        object boxed = b;
        int fromPattern = 0;
        if (boxed is int v) fromPattern = v;
        int? back = (int?)boxed;
        string sw = "";
        switch (a) { case null: sw += "N"; break; case 1: sw += "1"; break; }
        switch (b) { case null: sw += "N"; break; case 16: sw += " S"; break; default: sw += " D"; break; }
        string pat = "";
        if (b is int w && w == 16) pat += "W";
        if (a is null) pat += "Z";
        Debug.Log($"[verify-nullable] 5 boxing: is int v -> {fromPattern}, (int?)boxed -> {back}, switch: {sw}, patterns: {pat}");

        NlPoint? p = null;
        NlPoint? q = new NlPoint(1, 2);
        NlPoint sum = q.Value + new NlPoint(3, 4);
        NlPoint? r = p ?? q;
        Vector3? vec = null;
        vec = new Vector3(1, 2, 3);
        Debug.Log($"[verify-nullable] 6 struct?: p==null {p == null}, q.Value+(3,4)={sum}, (p??q).Value.X={r.Value.X}, Vector3? v={vec}");

        NlBox none = null;
        NlBox some = new NlBox();
        int? n1 = none?.Count;
        int? n2 = some?.Count;
        Debug.Log($"[verify-nullable] 7 ?.: none?.Count null={n1 == null}, some?.Count={n2}, some.Next?.Count ?? 7 = {some.Next?.Count ?? 7}");

        int? def = default;
        int?[] arr = new int?[2];
        arr[1] = 9;
        stored = arr[1];
        Debug.Log($"[verify-nullable] 8 arrays/default: def==null {def == null}, arr[0]==null {arr[0] == null}, arr[1]={stored}");

        string threw = "did not throw";
        try { int unused = a.Value; threw = "returned " + unused; }
        catch (InvalidOperationException) { threw = "threw InvalidOperationException"; }
        int? x = null;
        int? y = null;
        Debug.Log($"[verify-nullable] 9 a.Value {threw}; null==null {x == y}; null!=b {x != b}");

        int? pick = b.HasValue ? 1 : null;
        Debug.Log(pick);
    }
}

// MenSharp verification: `Deconstruct` on your own types.
//
// Setup: a Cube "VerifyDeconstruct" with this component. Play, click.
//
// Expected:
//   [verify-deconstruct] 1 class: (x, y)=3/4, (px, py)=3/4, struct (w, h)=2/5
//   [verify-deconstruct] 2 patterns: is (3, var found) -> 4, switch -> p7, switch expression -> unit
//   [verify-deconstruct] 3 dictionary: ann=30, bob=41, total=71
//   [verify-deconstruct] 4 nested: outer (1, 2), inner sum=3, named pair a=3 b=4
//
// List patterns ([1, ..], [var head, .. var rest]) are C# 11: Unity compiles
// Assets scripts as C# 9, so they cannot be written here. The compiler tests
// cover them (`list_patterns_match_an_array_by_shape`).

using System.Collections.Generic;
using MenSharp;
using UnityEngine;

public class DcPoint
{
    public int X;
    public int Y;
    public DcPoint(int x, int y) { X = x; Y = y; }
    public void Deconstruct(out int x, out int y) { x = X; y = Y; }
}

public struct DcSize
{
    public int Width;
    public int Height;
    public DcSize(int width, int height) { Width = width; Height = height; }
    public void Deconstruct(out int width, out int height)
    {
        width = Width;
        height = Height;
    }
}

public class VerifyDeconstruct : MenSharpBehaviour
{
    public void Interact()
    {
        var point = new DcPoint(3, 4);
        var (x, y) = point;
        int px;
        int py;
        (px, py) = point;
        var (width, height) = new DcSize(2, 5);
        Debug.Log($"[verify-deconstruct] 1 class: (x, y)={x}/{y}, (px, py)={px}/{py}, struct (w, h)={width}/{height}");

        int found = point is (3, var got) ? got : -1;
        object shape = point;
        string branch;
        switch (shape)
        {
            case DcPoint(0, 0): branch = "origin"; break;
            case DcPoint(var a, var b): branch = "p" + (a + b); break;
            default: branch = "?"; break;
        }
        string kind = new DcSize(1, 1) switch
        {
            (1, 1) => "unit",
            _ => "other",
        };
        Debug.Log($"[verify-deconstruct] 2 patterns: is (3, var found) -> {found}, switch -> {branch}, switch expression -> {kind}");

        var ages = new Dictionary<string, int>();
        ages["ann"] = 30;
        ages["bob"] = 41;
        string names = "";
        int total = 0;
        foreach (var (name, age) in ages)
        {
            names += name + "=" + age + ", ";
            total += age;
        }
        Debug.Log($"[verify-deconstruct] 3 dictionary: {names}total={total}");

        var inner = new DcPoint(1, 2);
        var (outerX, outerY) = inner;
        var pair = new DcSize(3, 4);
        var (namedA, namedB) = pair;
        Debug.Log($"[verify-deconstruct] 4 nested: outer ({outerX}, {outerY}), inner sum={outerX + outerY}, named pair a={namedA} b={namedB}");
    }
}

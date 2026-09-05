// MenSharp verification: tuples, deconstruction and positional patterns.
//
// Setup: a Cube "VerifyTuple" with this component. Play, click.
//
// Expected:
//   [verify-tuple] 1 values: pair=(1, a), named x=2 y=3, Item1=2, sum=9, printed (4, b)
//   [verify-tuple] 2 copies: copy.x=100 leaves the original at 2
//   [verify-tuple] 3 apart: var (a,b)=1/a, (c,d)=1/a, (int e,string f)=1/a, discard=a, nested 1+2 n
//   [verify-tuple] 4 equality: (1,2)==(1,2) True, (1,2)!=(1,3) True, dictionary key found: cell
//   [verify-tuple] 5 patterns: is (0, var h) -> 5, switch -> up5, switch expression -> left
//   [verify-tuple] 6 loops: foreach var (n, s) -> 7z, whole.Item1 -> 7
//   [verify-tuple] 7 boxed: printed (1, a), equal True, different False, cast back 1/2, bad cast caught

using System.Collections.Generic;
using MenSharp;
using UnityEngine;

public class VerifyTuple : MenSharpBehaviour
{
    private (int, string) Pair() { return (1, "a"); }
    private (int x, int y) Point(int x, int y) { return (x, y); }
    private int Sum((int a, int b) values) { return values.a + values.b; }

    public void Interact()
    {
        (int, string) pair = Pair();
        var named = Point(2, 3);
        Debug.Log($"[verify-tuple] 1 values: pair={pair}, named x={named.x} y={named.y}, Item1={named.Item1}, sum={Sum((4, 5))}, printed {(4, "b")}");

        var copy = named;
        copy.x = 100;
        Debug.Log($"[verify-tuple] 2 copies: copy.x={copy.x} leaves the original at {named.x}");

        var (a, b) = Pair();
        int c;
        string d;
        (c, d) = Pair();
        (int e, string f) = Pair();
        var (_, only) = Pair();
        ((int, int), string) nested = ((1, 2), "n");
        var ((p, q), r) = nested;
        Debug.Log($"[verify-tuple] 3 apart: var (a,b)={a}/{b}, (c,d)={c}/{d}, (int e,string f)={e}/{f}, discard={only}, nested {p}+{q} {r}");

        bool same = Point(1, 2) == (1, 2);
        bool differs = Point(1, 2) != (1, 3);
        var map = new Dictionary<(int, int), string>();
        map[(1, 2)] = "cell";
        string found = map.ContainsKey((1, 2)) ? map[(1, 2)] : "missing";
        Debug.Log($"[verify-tuple] 4 equality: (1,2)==(1,2) {same}, (1,2)!=(1,3) {differs}, dictionary key found: {found}");

        var shape = Point(0, 5);
        int height = shape is (0, var h) ? h : -1;
        string branch;
        switch (shape)
        {
            case (0, 0): branch = "origin"; break;
            case (0, var up): branch = "up" + up; break;
            default: branch = "?"; break;
        }
        string kind = shape switch
        {
            (0, 0) => "origin",
            (var x, _) when x > 0 => "right",
            _ => "left",
        };
        Debug.Log($"[verify-tuple] 5 patterns: is (0, var h) -> {height}, switch -> {branch}, switch expression -> {kind}");

        var list = new List<(int, string)>();
        list.Add((7, "z"));
        int total = 0;
        string letters = "";
        foreach (var (number, letter) in list)
        {
            total += number;
            letters += letter;
        }
        int whole = 0;
        foreach (var item in list) whole += item.Item1;
        Debug.Log($"[verify-tuple] 6 loops: foreach var (n, s) -> {total}{letters}, whole.Item1 -> {whole}");

        // a tuple keeps its shape inside an object
        object boxed = (1, "a");
        string printed = boxed.ToString();
        bool equal = ((object)(1, 2)).Equals((1, 2));
        bool different = ((object)(1, 2)).Equals((1, 3));
        object boxedPair = (1, 2);
        (int, int) backAgain = ((int, int))boxedPair;
        string bad;
        try
        {
            object text = "text";
            (int, int) wrong = ((int, int))text;
            bad = "not caught: " + wrong.Item1;
        }
        catch (System.InvalidCastException)
        {
            bad = "caught";
        }
        Debug.Log($"[verify-tuple] 7 boxed: printed {printed}, equal {equal}, different {different}, cast back {backAgain.Item1}/{backAgain.Item2}, bad cast {bad}");
    }
}

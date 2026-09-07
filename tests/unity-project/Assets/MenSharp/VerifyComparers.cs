// MenSharp verification: comparers — IComparer<T> / IEqualityComparer<T>,
// Comparer<T> / EqualityComparer<T>, StringComparer — through List<T>,
// Dictionary, HashSet and LINQ.
//
// Setup: a Cube "VerifyComparers" with this component. Play, click.
//
// Expected:
//   [verify-comparers] 1 sort: Fig,fig,kiwi,pear,apple by case apple,Fig,fig,kiwi,pear created pear,kiwi,fig,apple,Fig search 3 -1
//   [verify-comparers] 2 dictionary: count 2 ann 31 BOB True 41 keys Ann,bob ordinal 2 comparer True False
//   [verify-comparers] 3 set: 2,3,4 again False contains True False lower False True 2 equals True
//   [verify-comparers] 4 order: A,a,b,B,C,cc then cc,b,a,C,B,A then C,b,B,A,a,cc
//   [verify-comparers] 5 sets: distinct b,A,cc,C union x,Y,z intersect b,C except b,C mod 5,6
//   [verify-comparers] 6 queries: contains True False sequence True False tohashset 4 groups b:2,A:2,cc:1,C:1
//   [verify-comparers] 7 defaults: -1 0 True False hash True culture True True ordinal True True null 0

using System;
using System.Collections.Generic;
using System.Linq;
using MenSharp;
using UnityEngine;

public class ByLength : IComparer<string>
{
    public int Compare(string x, string y)
    {
        if (x.Length != y.Length) { return x.Length.CompareTo(y.Length); }
        return string.Compare(x, y, StringComparison.Ordinal);
    }
}

public class ModTen : IEqualityComparer<int>
{
    public bool Equals(int x, int y) { return x % 10 == y % 10; }
    public int GetHashCode(int obj) { return obj % 10; }
}

public class VerifyComparers : MenSharpBehaviour
{
    private string Join<T>(IEnumerable<T> items)
    {
        string text = "";
        foreach (T item in items) { text += item + ","; }
        return text.TrimEnd(',');
    }

    public void Interact()
    {
        var words = new List<string> { "pear", "Fig", "apple", "kiwi", "fig" };
        words.Sort(new ByLength());
        string byLength = Join(words);
        words.Sort(StringComparer.OrdinalIgnoreCase);
        string byCase = Join(words);
        words.Sort(Comparer<string>.Create((a, b) => string.Compare(b, a, StringComparison.Ordinal)));
        string created = Join(words);
        words.Sort(new ByLength());
        Debug.Log($"[verify-comparers] 1 sort: {byLength} by case {byCase} created {created} search {words.BinarySearch("pear", new ByLength())} {words.BinarySearch("zz", new ByLength())}");

        var ages = new Dictionary<string, int>(StringComparer.OrdinalIgnoreCase);
        ages["Ann"] = 30;
        ages["ANN"] = 31;
        ages.Add("bob", 41);
        int found;
        bool got = ages.TryGetValue("BOB", out found);
        var ordinal = new Dictionary<string, int>(StringComparer.Ordinal);
        ordinal["a"] = 1;
        ordinal["A"] = 2;
        Debug.Log($"[verify-comparers] 2 dictionary: count {ages.Count} ann {ages["ann"]} BOB {got} {found} keys {Join(ages.Keys)} ordinal {ordinal.Count} comparer {ages.Comparer.Equals("x", "X")} {ordinal.Comparer.Equals("x", "X")}");

        var tens = new HashSet<int>(new int[] { 1, 11, 2, 22, 3 }, new ModTen());
        bool again = tens.Add(21);
        tens.UnionWith(new int[] { 4, 14 });
        tens.IntersectWith(new int[] { 12, 13, 14, 15 });
        var lower = new HashSet<string>(StringComparer.InvariantCultureIgnoreCase) { "A", "b" };
        Debug.Log($"[verify-comparers] 3 set: {Join(tens)} again {again} contains {tens.Contains(33)} {tens.Contains(5)} lower {lower.Add("a")} {lower.Contains("B")} {lower.Count} equals {lower.SetEquals(new string[] { "a", "B" })}");

        string[] mixed = new string[] { "b", "A", "a", "B", "cc", "C" };
        Debug.Log($"[verify-comparers] 4 order: {Join(mixed.OrderBy(s => s, StringComparer.OrdinalIgnoreCase))} then {Join(mixed.OrderByDescending(s => s, new ByLength()).ThenBy(s => s, StringComparer.Ordinal))} then {Join(mixed.OrderBy(s => s.Length).ThenByDescending(s => s, StringComparer.OrdinalIgnoreCase))}");

        Debug.Log($"[verify-comparers] 5 sets: distinct {Join(mixed.Distinct(StringComparer.OrdinalIgnoreCase))} union {Join(new string[] { "x", "Y" }.Union(new string[] { "X", "z" }, StringComparer.OrdinalIgnoreCase))} intersect {Join(mixed.Intersect(new string[] { "c", "b" }, StringComparer.OrdinalIgnoreCase))} except {Join(mixed.Except(new string[] { "a", "cc" }, StringComparer.OrdinalIgnoreCase))} mod {Join(new int[] { 5, 15, 6 }.Distinct(new ModTen()))}");

        string groups = "";
        foreach (var group in mixed.GroupBy(s => s, StringComparer.OrdinalIgnoreCase)) { groups += group.Key + ":" + group.Count() + ","; }
        Debug.Log($"[verify-comparers] 6 queries: contains {mixed.Contains("CC", StringComparer.OrdinalIgnoreCase)} {mixed.Contains("CC", StringComparer.Ordinal)} sequence {new string[] { "a", "B" }.SequenceEqual(new string[] { "A", "b" }, StringComparer.OrdinalIgnoreCase)} {new string[] { "a", "B" }.SequenceEqual(new string[] { "A", "b" }, EqualityComparer<string>.Default)} tohashset {mixed.ToHashSet(StringComparer.OrdinalIgnoreCase).Count} groups {groups.TrimEnd(',')}");

        var byDefault = Comparer<int>.Default;
        var equality = EqualityComparer<string>.Default;
        var culture = StringComparer.CurrentCultureIgnoreCase;
        Debug.Log($"[verify-comparers] 7 defaults: {byDefault.Compare(3, 5)} {byDefault.Compare(5, 5)} {equality.Equals("a", "a")} {equality.Equals("a", null)} hash {equality.GetHashCode("hi") == "hi".GetHashCode()} culture {culture.Equals("Ab", "aB")} {culture.GetHashCode("Ab") == culture.GetHashCode("aB")} ordinal {StringComparer.Ordinal.Compare("a", "B") > 0} {StringComparer.OrdinalIgnoreCase.Compare("a", "B") < 0} null {StringComparer.Ordinal.GetHashCode(null)}");
    }
}

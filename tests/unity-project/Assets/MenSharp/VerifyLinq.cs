// MenSharp verification: LINQ.
//
// Setup: a Cube "VerifyLinq" with this component. Play, click.
//
// Expected:
//   [verify-linq] 1 filter: 50,30,10,90 sum 28 count 3 any True all True min 1 max 9 avg 466
//   [verify-linq] 2 elements: 5 8 2 0 8 window 8,1,9 while 5,3,8 / 1,9,2
//   [verify-linq] 3 sets: 7,2,9,1,8,3,5 distinct 1,2,3 union 1,2,3,4 intersect 2,3 except 1,3 range 1,2,3,4
//   [verify-linq] 4 order: kiwi,pear,apple,fig then kiwi,pear,fig,apple words apple,Banana,fig,pear
//   [verify-linq] 5 objects: total 80 weight 2.25 avg 266 names apple,fig dict 30 groups 30:2:apple,fig|20:1:pear
//   [verify-linq] 6 comparable: max 2.1 min 1.9 sorted 1.9,2.0,2.1 list 1.9,2.0,2.1 nulls null,1.0,3.0 empty True
//   [verify-linq] 7 lazy: built;s1;w1;s2;w2;got20;closed; again 20 aggregate 28 72 6
//   [verify-linq] 8 strings: hello has 2 l, reversed c,b,a chars a,b,c,d zip a5,b3 error Sequence contains no elements

using System;
using System.Collections.Generic;
using System.Linq;
using MenSharp;
using UnityEngine;

public class VerifyLinq : MenSharpBehaviour
{
    public class Item
    {
        public string Name;
        public int Price;
        public float Weight;
        public Item(string name, int price, float weight) { Name = name; Price = price; Weight = weight; }
    }

    public class Version : IComparable<Version>
    {
        public int Major;
        public int Minor;
        public Version(int major, int minor) { Major = major; Minor = minor; }
        public int CompareTo(Version other)
        {
            if (Major != other.Major) { return Major.CompareTo(other.Major); }
            return Minor.CompareTo(other.Minor);
        }
        public override string ToString() { return Major + "." + Minor; }
    }

    private string trace = "";

    private string Join<T>(IEnumerable<T> items)
    {
        string text = "";
        foreach (T item in items) { text += item + ","; }
        return text.TrimEnd(',');
    }

    private IEnumerable<int> Source()
    {
        try
        {
            for (int i = 1; i <= 5; i++)
            {
                trace += "s" + i + ";";
                yield return i;
            }
        }
        finally
        {
            trace += "closed;";
        }
    }

    public void Interact()
    {
        int[] numbers = new int[] { 5, 3, 8, 1, 9, 2 };
        Debug.Log($"[verify-linq] 1 filter: {Join(numbers.Where(n => n % 2 == 1).Select(n => n * 10))} sum {numbers.Sum()} count {numbers.Count(n => n > 4)} any {numbers.Any(n => n > 8)} all {numbers.All(n => n > 0)} min {numbers.Min()} max {numbers.Max()} avg {(int)(numbers.Average() * 100)}");
        Debug.Log($"[verify-linq] 2 elements: {numbers.First()} {numbers.First(n => n > 5)} {numbers.Last()} {numbers.FirstOrDefault(n => n > 100)} {numbers.ElementAt(2)} window {Join(numbers.Skip(2).Take(3))} while {Join(numbers.TakeWhile(n => n > 2))} / {Join(numbers.SkipWhile(n => n > 2))}");
        Debug.Log($"[verify-linq] 3 sets: {Join(numbers.Concat(new int[] { 7 }).Reverse())} distinct {Join(new int[] { 1, 2, 2, 3, 1 }.Distinct())} union {Join(new int[] { 1, 2, 3 }.Union(new int[] { 3, 4 }))} intersect {Join(new int[] { 1, 2, 3 }.Intersect(new int[] { 2, 3, 5 }))} except {Join(new int[] { 1, 2, 3 }.Except(new int[] { 2 }))} range {Join(Enumerable.Range(1, 4))}");

        List<Item> items = new List<Item>();
        items.Add(new Item("apple", 30, 1.5f));
        items.Add(new Item("pear", 20, 0.5f));
        items.Add(new Item("fig", 30, 0.25f));
        items.Add(new Item("kiwi", 10, 0.75f));
        string[] words = new string[] { "pear", "apple", "fig", "Banana" };
        Debug.Log($"[verify-linq] 4 order: {Join(items.OrderBy(x => x.Price).Select(x => x.Name))} then {Join(items.OrderBy(x => x.Price).ThenByDescending(x => x.Name).Select(x => x.Name))} words {Join(words.OrderBy(s => s))}");

        items.RemoveAt(3);
        string groups = "";
        foreach (IGrouping<int, Item> group in items.GroupBy(x => x.Price))
        {
            groups += group.Key + ":" + group.Count() + ":" + Join(group.Select(x => x.Name)) + "|";
        }
        Debug.Log($"[verify-linq] 5 objects: total {items.Sum(x => x.Price)} weight {items.Sum(x => x.Weight)} avg {(int)(items.Average(x => x.Price) * 10)} names {Join(items.Where(x => x.Price == 30).Select(x => x.Name))} dict {items.ToDictionary(x => x.Name)["fig"].Price} groups {groups.TrimEnd('|')}");

        Version[] versions = new Version[] { new Version(2, 1), new Version(1, 9), new Version(2, 0) };
        List<Version> list = new List<Version>(versions);
        list.Sort();
        Version[] withNull = new Version[] { new Version(3, 0), null, new Version(1, 0) };
        Debug.Log($"[verify-linq] 6 comparable: max {versions.Max()} min {versions.Min()} sorted {Join(versions.OrderBy(v => v))} list {Join(list)} nulls {Join(withNull.OrderBy(v => v).Select(v => v == null ? "null" : v.ToString()))} empty {new Version[0].Max() == null}");

        trace = "";
        IEnumerable<int> query = Source().Where(n => { trace += "w" + n + ";"; return n % 2 == 0; }).Select(n => n * 10);
        trace += "built;";
        foreach (int value in query.Take(1)) { trace += "got" + value + ";"; }
        string lazy = trace;
        int again = query.First();
        Debug.Log($"[verify-linq] 7 lazy: {lazy} again {again} aggregate {numbers.Aggregate((a, b) => a + b)} {numbers.Aggregate(100, (a, b) => a - b)} {numbers.Aggregate("", (s, n) => s + n, s => s.Length)}");

        string error = "";
        try
        {
            numbers.Where(n => n > 100).First();
        }
        catch (InvalidOperationException e)
        {
            error = e.Message;
        }
        Debug.Log($"[verify-linq] 8 strings: hello has {"hello".Count(c => c == 'l')} l, reversed {Join("abc".Reverse())} chars {Join(new string[] { "ab", "cd" }.SelectMany(s => s.ToCharArray()))} zip {Join(numbers.Zip(new string[] { "a", "b" }, (n, s) => s + n))} error {error}");
    }
}

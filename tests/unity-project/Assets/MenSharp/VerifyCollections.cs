// MenSharp verification: the collections beyond List<T>'s basics —
// List<T>'s insert/range/search members, HashSet<T>, Queue<T>, Stack<T>.
//
// Setup: a Cube "VerifyCollections" with this component. Play, click.
//
// Expected:
//   [verify-collections] 1 list: 5,10,11,12,20,30,40 range 11,12,20 removed 5,10,12,20,30,40 True False
//   [verify-collections] 2 list: reversed 40,12,20,30,10,5 last 5 all 40,20 index 2 -1 0 sorted 5,10,12,20,30,40 search 3 -5 copy 0,5,10,12,20,30,40,0
//   [verify-collections] 3 set: 3,5,2,4 count 4 add True False contains True False
//   [verify-collections] 4 set: union 1,2,3,4,5 intersect 2,3,4,5 except 2,4,5 symmetric 2,7,5 subset True proper False equals True overlaps True
//   [verify-collections] 5 set: names a,null count 2 null True removed 1 tohashset 2 distinct 1,2,3
//   [verify-collections] 6 queue: first 1 order 2,3,4 peek 2 count 3 ring 3100 4350 25 empty Queue empty.
//   [verify-collections] 7 stack: top 3 order 4,2,1 array 4,2,1 pop 4 peek 2 count 2 empty Stack empty.

using System;
using System.Collections.Generic;
using System.Linq;
using MenSharp;
using UnityEngine;

public class VerifyCollections : MenSharpBehaviour
{
    private string Join<T>(IEnumerable<T> items)
    {
        string text = "";
        foreach (T item in items) { text += (item == null ? "null" : item.ToString()) + ","; }
        return text.TrimEnd(',');
    }

    public void Interact()
    {
        var list = new List<int> { 10, 20, 30 };
        list.Insert(0, 5);
        list.Insert(4, 40);
        list.InsertRange(2, new int[] { 11, 12 });
        string inserted = Join(list);
        var range = list.GetRange(2, 3);
        bool gone = list.Remove(11);
        bool absent = list.Remove(99);
        Debug.Log($"[verify-collections] 1 list: {inserted} range {Join(range)} removed {Join(list)} {gone} {absent}");

        list.Reverse();
        list.Reverse(1, 3);
        string reversed = Join(list);
        int last = list.FindLast(n => n < 15);
        string all = Join(list.FindAll(n => n % 20 == 0));
        string index = list.IndexOf(20) + " " + list.IndexOf(20, 3) + " " + list.LastIndexOf(40);
        list.Sort();
        string search = list.BinarySearch(20) + " " + list.BinarySearch(21);
        var target = new int[8];
        list.CopyTo(target, 1);
        Debug.Log($"[verify-collections] 2 list: reversed {reversed} last {last} all {all} index {index} sorted {Join(list)} search {search} copy {Join(target)}");

        var set = new HashSet<int> { 3, 1, 2 };
        bool fresh = set.Add(4);
        bool dup = set.Add(3);
        set.Remove(1);
        set.Add(5);
        Debug.Log($"[verify-collections] 3 set: {Join(set)} count {set.Count} add {fresh} {dup} contains {set.Contains(2)} {set.Contains(1)}");

        var a = new HashSet<int>(new int[] { 1, 2, 3, 4 });
        a.UnionWith(new int[] { 3, 4, 5, 5 });
        string union = Join(a);
        a.IntersectWith(new int[] { 2, 3, 4, 5, 6 });
        string intersect = Join(a);
        a.ExceptWith(new int[] { 3 });
        string except = Join(a);
        a.SymmetricExceptWith(new int[] { 4, 7, 7 });
        string symmetric = Join(a);
        var small = new HashSet<int>(new int[] { 1, 2 });
        var big = new HashSet<int>(new int[] { 1, 2, 3 });
        Debug.Log($"[verify-collections] 4 set: union {union} intersect {intersect} except {except} symmetric {symmetric} subset {small.IsSubsetOf(big)} proper {big.IsProperSubsetOf(big)} equals {small.SetEquals(new int[] { 2, 1, 1 })} overlaps {small.Overlaps(new int[] { 2, 9 })}");

        var words = new HashSet<string>();
        words.Add("a");
        words.Add(null);
        words.Add(null);
        string names = Join(words);
        bool hasNull = words.Contains(null);
        int removed = words.RemoveWhere(w => w == null);
        var fromLinq = new int[] { 5, 5, 6 }.ToHashSet();
        Debug.Log($"[verify-collections] 5 set: names {names} count {words.Count + removed} null {hasNull} removed {removed} tohashset {fromLinq.Count} distinct {Join(new int[] { 1, 2, 2, 3, 1 }.Distinct())}");

        var queue = new Queue<int>();
        queue.Enqueue(1);
        queue.Enqueue(2);
        queue.Enqueue(3);
        int first = queue.Dequeue();
        queue.Enqueue(4);
        var ring = new Queue<int>(2);
        int total = 0;
        for (int i = 0; i < 50; i++)
        {
            ring.Enqueue(i);
            ring.Enqueue(i + 100);
            total += ring.Dequeue();
        }
        var empty = new Queue<string>();
        string queueError = "";
        try { empty.Dequeue(); } catch (InvalidOperationException e) { queueError = e.Message; }
        Debug.Log($"[verify-collections] 6 queue: first {first} order {Join(queue)} peek {queue.Peek()} count {queue.Count} ring {total} {ring.Sum()} {ring.Peek()} empty {queueError}");

        var stack = new Stack<int>();
        stack.Push(1);
        stack.Push(2);
        stack.Push(3);
        int top = stack.Pop();
        stack.Push(4);
        string order = Join(stack);
        string array = Join(stack.ToArray());
        int popped;
        stack.TryPop(out popped);
        var drained = new Stack<int>();
        string stackError = "";
        try { drained.Pop(); } catch (InvalidOperationException e) { stackError = e.Message; }
        Debug.Log($"[verify-collections] 7 stack: top {top} order {order} array {array} pop {popped} peek {stack.Peek()} count {stack.Count} empty {stackError}");
    }
}

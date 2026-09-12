// MenSharp verification: delegates, lambdas, closures, events.
//
// Setup: a Cube "VerifyDelegates" with this component. Play, click.
//
// Expected:
//   [verify-delegates] 1 lambda: f(2)=3, f.Invoke(3)=4, DlOp(4)=12
//   [verify-delegates] 2 method group: Twice(5)=10, counter.Bump x2 -> 11, virtual Twice(11)=22
//   [verify-delegates] 3 closure: captured 10 -> 12; per-iteration makers 3,4,7; nested 7
//   [verify-delegates] 4 recursion through a delegate: fact(5)=120
//   [verify-delegates] 5 generics: Best(3,9)=9, longer=yyy, holder=9
//   [verify-delegates] 6 List<T>: sorted=1245, find=4, removed=2, converted=<1><5>
//   [verify-delegates] 7 multicast: log=ababaaba0a, equal=1111
//   [verify-delegates] 8 events: hits=3, seen=3, quiet=1
//   [verify-delegates] 9 null delegate threw NullReferenceException; ?. skipped: 7
//   [verify-delegates] 10 behaviour closure: clicks=2, log=hit1hit2

using System;
using System.Collections.Generic;
using MenSharp;
using UnityEngine;

public delegate int DlOp(int a);
public delegate void DlHandler(int a);

public class VerifyDelegates : MenSharpBehaviour
{
    public int clicks;
    public string log = "";
    private Action onClick;

    private int Twice(int x) { return x * 2; }

    public void Interact()
    {
        // 1. lambdas and delegate types
        Func<int, int> f = x => x + 1;
        DlOp o = y => y * 3;
        Debug.Log($"[verify-delegates] 1 lambda: f(2)={f(2)}, f.Invoke(3)={f.Invoke(3)}, DlOp(4)={o(4)}");

        // 2. method groups: the behaviour's own, an object's, a virtual one
        Func<int, int> twice = Twice;
        var counter = new DlCounter();
        Action<int> add = counter.Bump;
        add(5);
        add(6);
        Func<int, int> virt = counter.Twice;
        Debug.Log($"[verify-delegates] 2 method group: Twice(5)={twice(5)}, counter.Bump x2 -> {counter.Count}, virtual Twice(11)={virt(counter.Count)}");

        // 3. closures
        int captured = 10;
        Action bump = () => { captured++; };
        bump();
        bump();
        Func<int>[] makers = new Func<int>[3];
        for (int i = 0; i < 3; i++)
        {
            int square = i * i;
            makers[i] = () => square + i;
        }
        int outer = 1;
        Func<Func<int>> make = () => { int inner = 2; return () => outer + inner; };
        Func<int> made = make();
        outer = 5;
        Debug.Log($"[verify-delegates] 3 closure: captured 10 -> {captured}; per-iteration makers {makers[0]()},{makers[1]()},{makers[2]()}; nested {made()}");

        // 4. recursion through a delegate
        Func<int, int> fact = null;
        fact = n => n <= 1 ? 1 : n * fact(n - 1);
        Debug.Log($"[verify-delegates] 4 recursion through a delegate: fact(5)={fact(5)}");

        // 5. generics
        var holder = new DlHolder<int>(1, v => v * 3);
        holder.Advance();
        holder.Advance();
        Func<string, string, string> longer = (a, b) => a.Length >= b.Length ? a : b;
        Debug.Log($"[verify-delegates] 5 generics: Best(3,9)={Best(3, 9, (a, b) => a < b ? b : a)}, longer={longer("x", "yyy")}, holder={holder.Value}");

        // 6. List<T> with delegates
        var list = new List<int>();
        list.Add(5); list.Add(1); list.Add(4); list.Add(2);
        list.Sort((a, b) => a - b);
        string sorted = "";
        list.ForEach(v => { sorted += v; });
        int found = list.Find(v => v > 3);
        int removed = list.RemoveAll(v => v % 2 == 0);
        var converted = list.ConvertAll(v => "<" + v + ">");
        Debug.Log($"[verify-delegates] 6 List<T>: sorted={sorted}, find={found}, removed={removed}, converted={converted[0]}{converted[1]}");

        // 7. multicast and equality
        string mlog = "";
        Action a = () => { mlog += "a"; };
        Action b = () => { mlog += "b"; };
        Action c = a + b; c();
        c += a; c();
        c -= a; c();
        c = c - b; c();
        c -= a; if (c == null) mlog += "0";
        Action none = null; none += a; none();
        int equal = 0;
        if (a == a) equal += 1;
        if (a != b) equal += 10;
        if ((a + b) == (a + b)) equal += 100;
        if ((a + b) != (b + a)) equal += 1000;
        Debug.Log($"[verify-delegates] 7 multicast: log={mlog}, equal={equal}");

        // 8. events
        var door = new DlDoor();
        int hits = 0;
        int seen = 0;
        door.Opened += () => { hits++; };
        DlHandler h = t => { seen += t; };
        door.Any += h;
        door.Open(); door.Open();
        door.Any -= h;
        door.Open();
        var quiet = new DlDoor();
        quiet.Open();
        Debug.Log($"[verify-delegates] 8 events: hits={hits}, seen={seen}, quiet={quiet.Times}");

        // 9. null delegates and ?.
        Action nothing = null;
        string threw = "did not throw";
        try { nothing(); } catch (NullReferenceException) { threw = "threw NullReferenceException"; }
        DlNode missing = null;
        int skipped = 0;
        if (missing?.Name == null) skipped += 1;
        var node = new DlNode();
        if (node?.Name == "n") skipped += 2;
        if (node.Next?.Next?.Name == null) skipped += 4;
        Debug.Log($"[verify-delegates] 9 null delegate {threw}; ?. skipped: {skipped}");

        // 10. a closure over the behaviour's own fields, kept across events
        if (onClick == null)
        {
            string prefix = "hit";
            onClick = () => { clicks++; log += prefix + clicks; };
        }
        onClick();
        onClick();
        Debug.Log($"[verify-delegates] 10 behaviour closure: clicks={clicks}, log={log}");
        clicks = 0;
        log = "";
    }

    private static T Best<T>(T a, T b, Func<T, T, T> pick) { return pick(a, b); }
}

public class DlCounter
{
    public int Count;
    public void Bump(int by) { Count += by; }
    public virtual int Twice(int x) { return x * 2; }
}

public class DlHolder<T>
{
    public Func<T, T> Step;
    public T Value;
    public DlHolder(T start, Func<T, T> step) { Value = start; Step = step; }
    public void Advance() { Value = Step(Value); }
}

public class DlDoor
{
    public event Action Opened;
    // (an instance event: a static delegate is one program's code, so a
    // static delegate field is a compile error)
    public event DlHandler Any;
    public int Times;
    public void Open() { Times++; Opened?.Invoke(); Any?.Invoke(Times); }
}

public class DlNode
{
    public string Name = "n";
    public DlNode Next;
}

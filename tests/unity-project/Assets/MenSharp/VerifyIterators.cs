// MenSharp verification: iterators (yield return) and cancellation.
//
// Setup: a Cube "VerifyIterators" with this component. Play, click, wait
// about two seconds.
//
// Expected:
//   [verify-iter] 1 lazy: made;start;make0;got0;make1;got2;end;
//   [verify-iter] 2 again: sum=4 (body ran 3 times), local=10,11, words=xa,xb,stopa, repeat=rr
//   [verify-iter] 3 tree: 1234, counter=9
//   [verify-iter] 4 broken: caught mid, after=False
//   [verify-iter] 5 cancel: early canceled=True, blinks before stop=3
//   [verify-iter] 6 stopped: stopped at 3, token-less blink still going True
//   [verify-iter] 7 timed: wait stopped, guarded faulted=True, elapsed ~2 (2 exactly, not 2.000248)

using System;
using System.Collections.Generic;
using System.Threading;
using System.Threading.Tasks;
using MenSharp;
using UnityEngine;

public class VerifyIterators : MenSharpBehaviour
{
    private string log;
    private int blinks;
    private int freeBlinks;

    private class Tree
    {
        public int Value;
        public Tree Left;
        public Tree Right;
        public Tree(int value, Tree left, Tree right) { Value = value; Left = left; Right = right; }

        public IEnumerable<int> InOrder()
        {
            if (Left != null) { foreach (int v in Left.InOrder()) { yield return v; } }
            yield return Value;
            if (Right != null) { foreach (int v in Right.InOrder()) { yield return v; } }
        }
    }

    private IEnumerable<int> Evens(int count)
    {
        log += "start;";
        for (int i = 0; i < count; i++)
        {
            log += "make" + i + ";";
            yield return i * 2;
        }
        log += "end;";
    }

    private IEnumerable<string> Words(string prefix)
    {
        yield return prefix + "a";
        if (prefix == "stop") { yield break; }
        yield return prefix + "b";
    }

    private IEnumerable<T> Repeat<T>(T item, int times)
    {
        for (int i = 0; i < times; i++) { yield return item; }
    }

    private IEnumerator<int> Counter()
    {
        int n = 1;
        while (true) { yield return n; n *= 3; }
    }

    private IEnumerable<int> Broken()
    {
        yield return 1;
        throw new InvalidOperationException("mid");
    }

    public async void Interact()
    {
        log = "";
        IEnumerable<int> evens = Evens(2);
        log += "made;";
        foreach (int e in evens) { log += "got" + e + ";"; }
        Debug.Log($"[verify-iter] 1 lazy: {log}");

        int sum = 0;
        int runs = 0;
        foreach (int e in evens) { sum += e; }
        foreach (int e in evens) { sum += e; }
        runs = (log.Length - log.Replace("start;", "").Length) / "start;".Length;
        IEnumerable<int> local(int a) { yield return a; yield return a + 1; }
        string locals = "";
        foreach (int v in local(10)) { locals += v + ","; }
        string words = "";
        foreach (string w in Words("x")) { words += w + ","; }
        foreach (string w in Words("stop")) { words += w + ","; }
        string repeat = "";
        foreach (string s in Repeat("r", 2)) { repeat += s; }
        Debug.Log($"[verify-iter] 2 again: sum={sum} (body ran {runs} times), local={locals.TrimEnd(',')}, words={words.TrimEnd(',')}, repeat={repeat}");

        Tree tree = new Tree(2, new Tree(1, null, null), new Tree(4, new Tree(3, null, null), null));
        string walked = "";
        foreach (int v in tree.InOrder()) { walked += v; }
        IEnumerator<int> counter = Counter();
        counter.MoveNext();
        counter.MoveNext();
        counter.MoveNext();
        Debug.Log($"[verify-iter] 3 tree: {walked}, counter={counter.Current}");

        IEnumerator<int> broken = Broken().GetEnumerator();
        broken.MoveNext();
        string caught = "not caught";
        try { broken.MoveNext(); } catch (InvalidOperationException e) { caught = "caught " + e.Message; }
        Debug.Log($"[verify-iter] 4 broken: {caught}, after={broken.MoveNext()}");

        float started = Time.time;
        blinks = 0;
        freeBlinks = 0;
        var blinking = new CancellationTokenSource();
        Blink(blinking.Token);
        FreeBlink();
        var already = new CancellationTokenSource();
        already.Cancel();
        Task early = Guarded(already.Token);
        string earlyResult = "not canceled";
        try { await early; } catch (TaskCanceledException) { earlyResult = "early canceled=" + early.IsCanceled; }
        await Scheduler.Delay(1.2f);
        Debug.Log($"[verify-iter] 5 cancel: {earlyResult}, blinks before stop={blinks}");

        int freeBefore = freeBlinks;
        blinking.Cancel();
        await Scheduler.Delay(1.2f);
        Debug.Log($"[verify-iter] 6 stopped: {stopped}, token-less blink still going {freeBlinks > freeBefore}");

        var timed = new CancellationTokenSource();
        float timedStart = Time.time;
        timed.CancelAfter(2000);
        Task guarded = Guarded(timed.Token);
        string wait = "wait not stopped";
        try { await Scheduler.WaitUntil(() => false, timed.Token); } catch (OperationCanceledException) { wait = "wait stopped"; }
        Debug.Log($"[verify-iter] 7 timed: {wait}, guarded faulted={guarded.IsFaulted}, elapsed ~{Time.time - timedStart:0}");
    }

    private string stopped = "";

    private async void Blink(CancellationToken token)
    {
        try
        {
            while (true)
            {
                blinks++;
                await Scheduler.Delay(0.5f, token);
            }
        }
        catch (OperationCanceledException)
        {
            stopped = "stopped at " + blinks;
        }
    }

    private async void FreeBlink()
    {
        while (freeBlinks < 100)
        {
            freeBlinks++;
            await Scheduler.Delay(0.5f);
        }
    }

    private async Task Guarded(CancellationToken token)
    {
        await Scheduler.Delay(5f, token);
        token.ThrowIfCancellationRequested();
    }
}

// MenSharp verification: async/await.
//
// Setup: a Cube "VerifyAsync" with this component. Play, click once, wait
// about three seconds. Clicking again while it runs starts a second, separate
// run (its lines say "run 2").
//
// Expected (run 1, in this order over ~3 seconds):
//   [verify-async] 1 start: sync part done, IsCompleted=False
//   [verify-async] 2 after 1s: elapsed ~1.0, frames advanced True
//   [verify-async] 3 helpers: doubled=84, both=x+y, first=x, delay(ms) ok, yield ok
//   [verify-async] 4 exceptions: caught later, faulted=True, finally ran, async void fine
//   [verify-async] 5 timing: same-tick order a,b | next frame later True, after 0.5s ~0.5
//   [verify-async] 6 source: got mail (from Ring), lambda=6, local=hi!, counter=2
//   [verify-async] 7 done: total ~3s, run 1

using System;
using System.Threading.Tasks;
using MenSharp;
using UnityEngine;

public class VerifyAsync : MenSharpBehaviour
{
    private int runs;
    private TaskCompletionSource<string> mailbox;

    public async void Interact()
    {
        runs++;
        int run = runs;
        float started = Time.time;
        int startFrame = Time.frameCount;

        Task<int> pending = Doubled(42);
        Debug.Log($"[verify-async] 1 start: sync part done, IsCompleted={pending.IsCompleted}");

        await Scheduler.Delay(1f);
        Debug.Log($"[verify-async] 2 after 1s: elapsed ~{Time.time - started:0.0}, frames advanced {Time.frameCount > startFrame}");

        Task<string> x = Word("x", 0.2f);
        Task<string> y = Word("y", 0.4f);
        Task first = await Task.WhenAny(x, y);
        await Task.WhenAll(x, y);
        string both = x.Result + "+" + y.Result;
        await Task.Delay(100);
        string yielded = "";
        await Scheduler.Yield();
        yielded = "yield ok";
        Debug.Log($"[verify-async] 3 helpers: doubled={await pending}, both={both}, first={(first == x ? "x" : "y")}, delay(ms) ok, {yielded}");

        string caught = "none";
        try { await Fails(); } catch (InvalidOperationException e) { caught = "caught " + e.Message; }
        Task faulted = Fails();
        await Scheduler.NextFrame();
        string finallyRan = "finally missing";
        try { await Scheduler.NextFrame(); } finally { finallyRan = "finally ran"; }
        FireAndForget();
        Debug.Log($"[verify-async] 4 exceptions: {caught}, faulted={faulted.IsFaulted}, {finallyRan}, async void fine");

        string order = "";
        Task a = Scheduler.Yield();
        Task b = Scheduler.Yield();
        await a;
        order += "a";
        await b;
        order += ",b";
        int frameBefore = Time.frameCount;
        await Task.CompletedTask.OnNextFrame();
        bool later = Time.frameCount > frameBefore;
        float beforeAfter = Time.time;
        await Task.FromResult(1).After(0.5f);
        Debug.Log($"[verify-async] 5 timing: same-tick order {order} | next frame later {later}, after 0.5s ~{Time.time - beforeAfter:0.0}");

        mailbox = new TaskCompletionSource<string>();
        Scheduler.Run(async () => { await Scheduler.Delay(0.2f); Ring(); });
        string mail = await mailbox.Task;
        Func<int, Task<int>> twice = async n => { await Scheduler.NextFrame(); return n * 2; };
        async Task<string> Local(string s) { await Scheduler.NextFrame(); return s + "!"; }
        int counter = 0;
        await Scheduler.Run(async () => { await Scheduler.NextFrame(); counter++; counter++; });
        await Scheduler.WaitUntil(() => counter >= 2);
        Debug.Log($"[verify-async] 6 source: got {mail} (from Ring), lambda={await twice(3)}, local={await Local("hi")}, counter={counter}");

        Debug.Log($"[verify-async] 7 done: total ~{Time.time - started:0}s, run {run}");
    }

    private async Task<int> Doubled(int n)
    {
        await Scheduler.NextFrame();
        return n * 2;
    }

    private async Task<string> Word(string word, float delay)
    {
        await Scheduler.Delay(delay);
        return word;
    }

    private async Task Fails()
    {
        await Scheduler.NextFrame();
        throw new InvalidOperationException("later");
    }

    private async void FireAndForget()
    {
        await Scheduler.NextFrame();
    }

    private void Ring()
    {
        mailbox.TrySetResult("mail");
    }
}

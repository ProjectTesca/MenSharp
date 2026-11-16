---
icon: lucide/timer
description: async/await、Task、Scheduler
---

# async / await

`Scheduler` と `Task` で待機、並列、キャンセルができます。`Scheduler.Delay` の単位は秒、`Task.Delay` の単位はミリ秒です。

```cs title="Ex5Async.cs"
using System.Threading;
using System.Threading.Tasks;
using MenSharp;
using UnityEngine;

public class Ex5Async : MenSharpBehaviour
{
    private CancellationTokenSource running;

    public async void Start()
    {
        await Scheduler.Delay(1f);          // 秒
        await Scheduler.NextFrame();
        await Scheduler.DelayFrames(10);
        await Task.Delay(500);              // ミリ秒（.NET と同じ）

        await Scheduler.WaitUntil(() => Time.time > 5f);

        // 同時に 2 つ
        await Task.WhenAll(Step("a", 0.5f), Step("b", 1f));

        // 投げっぱなし: 例外はイベントを殺さずログに出る
        Scheduler.Run(() => Step("background", 2f));

        // キャンセル
        running = new CancellationTokenSource();
        running.CancelAfter(3000);
        await Loop(running.Token);
    }

    public void Interact()
    {
        if (running != null)
        {
            running.Cancel();
        }
    }

    private async Task Step(string name, float seconds)
    {
        await Scheduler.Delay(seconds);
        Debug.Log(name);
    }

    private async Task<int> Count(int to)
    {
        int total = 0;
        for (int i = 0; i < to; i++)
        {
            total = total + i;
            await Scheduler.NextFrame();    // 処理をフレームに分散
        }
        return total;
    }

    private async Task Loop(CancellationToken token)
    {
        try
        {
            while (true)
            {
                await Scheduler.Delay(0.25f, token);
                Debug.Log(await Count(3));
            }
        }
        catch (System.OperationCanceledException)
        {
            Debug.Log("stopped");
        }
    }
}
```

このほかに、次の API があります。

- `Scheduler.WaitWhile` / `Scheduler.Yield()`
- `Task.CompletedTask` / `Task.FromResult` / `Task.WhenAny`
- `TaskCompletionSource<T>`（`TrySetResult` / `TrySetException`）
- `task.After(seconds)` / `task.OnNextFrame()` / `task.AfterFrames(n)`

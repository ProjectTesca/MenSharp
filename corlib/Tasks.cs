// MenSharp mini-corlib: Task, Task<T> and the scheduler behind async/await.
//
// Udon runs one event at a time and has no threads, so a task here is a
// promise: an object that is running, completed or faulted, holding the
// continuations to run once it is done. An `async` method makes its task on
// entry and returns it at the first `await` that has to wait; `await` on a
// task that is not done yet hands the task an `Action` that resumes the
// method where it left off — the compiler builds that action from a copy of
// the method's variables and the address to jump back to (see the code
// generator's `tasks` module). Everything else is ordinary M# code below.
//
// Continuations never run inside the code that completes a task. They are
// queued, and the queue is drained when the current event has finished
// (the compiler calls `Scheduler.__Drain` at the end of every event) — so a
// method that resumes never finds another activation of itself half-way
// through. `Task.Delay` and friends ask Udon for a delayed event
// (`_mensharpResume`) and resume from there.
//
// The names are .NET's, so a script using them is valid Unity C# as well;
// what runs on Udon is this file. Timing helpers with no .NET counterpart
// (`OnNextFrame`, `After`, `Scheduler.*`) are declared as extension methods
// and a stub class in the Unity package, so they compile there too.

using System;
using System.Threading.Tasks;

namespace System.Runtime.CompilerServices
{
    public class TaskAwaiter
    {
        private readonly System.Threading.Tasks.Task task;

        public TaskAwaiter(System.Threading.Tasks.Task task) { this.task = task; }

        public bool IsCompleted { get { return task.IsCompleted; } }

        public void OnCompleted(Action continuation) { task.__AddContinuation(continuation); }

        public void GetResult() { task.__Rethrow(); }
    }

    public class TaskAwaiter<TResult>
    {
        private readonly System.Threading.Tasks.Task<TResult> task;

        public TaskAwaiter(System.Threading.Tasks.Task<TResult> task) { this.task = task; }

        public bool IsCompleted { get { return task.IsCompleted; } }

        public void OnCompleted(Action continuation) { task.__AddContinuation(continuation); }

        public TResult GetResult() { task.__Rethrow(); return task.__result; }
    }
}

namespace System.Threading.Tasks
{
    public class Task
    {
        // 0 running, 1 completed, 2 faulted
        internal int __state;
        internal Exception __exception;
        internal Action[] __continuations;
        internal int __continuationCount;
        /// The behaviour whose program made this task. A task can be handed
        /// to another behaviour, but only its own program may run code for
        /// it, so everything that would do so checks this first.
        internal object __owner;
        /// What `__exception` prints as, recorded where the exception's own
        /// type is known. A faulted task read from another program can only
        /// report the text: the exception object itself carries a type id
        /// that means nothing outside the program that raised it.
        internal string __exceptionText;

        public Task() { __owner = MenSharp.Internal.Programs.SelfBehaviour(); }

        /// Was this task made by this program? Only then may its
        /// continuations be run here, or its exception rethrown as itself.
        internal bool __IsMine() { return MenSharp.Internal.Programs.IsSelf(__owner); }

        public bool IsCompleted { get { return __state != 0; } }
        public bool IsCompletedSuccessfully { get { return __state == 1; } }
        public bool IsFaulted { get { return __state == 2; } }
        public bool IsCanceled { get { return __state == 2 && __exception is OperationCanceledException; } }
        public Exception Exception { get { return __exception; } }

        public System.Runtime.CompilerServices.TaskAwaiter GetAwaiter()
        {
            return new System.Runtime.CompilerServices.TaskAwaiter(this);
        }

        // ------------------------------------------------ timing

        /// A task that completes on the frame after this one does: what to
        /// await when the continuation should not run in the same event.
        public Task OnNextFrame() { return __Then(3, 0f, 1); }

        /// A task that completes `seconds` after this one does.
        public Task After(float seconds) { return __Then(2, seconds, 0); }

        /// A task that completes `frames` frames after this one does.
        public Task AfterFrames(int frames) { return __Then(3, 0f, frames); }

        internal Task __Then(int mode, float seconds, int frames)
        {
            Task wrapper = new Task();
            Task source = this;
            __AddContinuation(() => MenSharp.Scheduler.__ScheduleCompletion(wrapper, source, mode, seconds, frames));
            return wrapper;
        }

        internal void __CompleteFrom(Task source)
        {
            if (source.__state == 2) { __Fail(source.__exception); }
            else { __Complete(); }
        }

        // ------------------------------------------------ factories

        public static Task CompletedTask
        {
            get
            {
                Task task = new Task();
                task.__state = 1;
                return task;
            }
        }

        public static Task<TResult> FromResult<TResult>(TResult result)
        {
            Task<TResult> task = new Task<TResult>();
            task.__result = result;
            task.__state = 1;
            return task;
        }

        public static Task FromException(Exception exception)
        {
            Task task = new Task();
            task.__exception = exception;
            task.__state = 2;
            return task;
        }

        /// Completes after `millisecondsDelay` milliseconds of scene time.
        public static Task Delay(int millisecondsDelay)
        {
            return MenSharp.Scheduler.Delay(millisecondsDelay / 1000f);
        }

        /// ... or faults with TaskCanceledException when `token` is
        /// cancelled first.
        public static Task Delay(int millisecondsDelay, System.Threading.CancellationToken token)
        {
            return MenSharp.Scheduler.Delay(millisecondsDelay / 1000f, token);
        }

        /// Completes once every task has; faults with the first fault met.
        public static async Task WhenAll(params Task[] tasks)
        {
            for (int i = 0; i < tasks.Length; i++)
            {
                await tasks[i];
            }
        }

        /// Completes with the first of the tasks to complete.
        public static Task<Task> WhenAny(params Task[] tasks)
        {
            TaskCompletionSource<Task> source = new TaskCompletionSource<Task>();
            for (int i = 0; i < tasks.Length; i++)
            {
                Task task = tasks[i];
                task.__AddContinuation(() => source.TrySetResult(task));
            }
            return source.Task;
        }

        // ------------------------------------------------ completion

        internal void __AddContinuation(Action continuation)
        {
            if (__state != 0)
            {
                // already done: this program runs it, in its own queue
                MenSharp.Scheduler.__Enqueue(continuation);
                return;
            }
            if (!__IsMine())
            {
                // the other program will fire this from its own queue, and
                // a code address of ours means nothing there — so give it
                // one that sends ours back to us instead
                continuation = MenSharp.Internal.Programs.RemoteContinuation(continuation);
            }
            if (__continuations == null)
            {
                __continuations = new Action[2];
            }
            else if (__continuationCount == __continuations.Length)
            {
                Action[] bigger = new Action[__continuations.Length * 2];
                for (int i = 0; i < __continuationCount; i++)
                {
                    bigger[i] = __continuations[i];
                }
                __continuations = bigger;
            }
            __continuations[__continuationCount] = continuation;
            __continuationCount++;
        }

        internal void __Complete()
        {
            if (__state != 0) { return; }
            __state = 1;
            __Fire();
        }

        internal void __Fail(Exception exception)
        {
            if (__state != 0) { return; }
            __state = 2;
            __exception = exception;
            __exceptionText = exception == null ? "" : exception.ToString();
            __Fire();
        }

        internal void __Fire()
        {
            for (int i = 0; i < __continuationCount; i++)
            {
                MenSharp.Scheduler.__Enqueue(__continuations[i]);
            }
            __continuations = null;
            __continuationCount = 0;
        }

        internal void __Rethrow()
        {
            if (__state != 2) { return; }
            if (!__IsMine())
            {
                // the exception object belongs to the other program: its
                // type cannot be tested here, so only the text crosses
                throw new MenSharp.RemoteTaskException(__exceptionText);
            }
            throw __exception;
        }
    }

    public class Task<TResult> : Task
    {
        internal TResult __result;

        public Task() { }

        /// The result, once the task has completed; faults rethrow.
        public TResult Result
        {
            get
            {
                if (__state == 0)
                {
                    throw new InvalidOperationException("the task has not completed yet: await it instead of reading Result");
                }
                __Rethrow();
                return __result;
            }
        }

        public new System.Runtime.CompilerServices.TaskAwaiter<TResult> GetAwaiter()
        {
            return new System.Runtime.CompilerServices.TaskAwaiter<TResult>(this);
        }

        public new Task<TResult> OnNextFrame() { return __ThenTyped(3, 0f, 1); }
        public new Task<TResult> After(float seconds) { return __ThenTyped(2, seconds, 0); }
        public new Task<TResult> AfterFrames(int frames) { return __ThenTyped(3, 0f, frames); }

        private Task<TResult> __ThenTyped(int mode, float seconds, int frames)
        {
            Task<TResult> wrapper = new Task<TResult>();
            Task<TResult> source = this;
            __AddContinuation(() => MenSharp.Scheduler.__ScheduleCompletionTyped(wrapper, source, mode, seconds, frames));
            return wrapper;
        }

        internal void __SetResult(TResult result)
        {
            if (__state != 0) { return; }
            __result = result;
            __state = 1;
            __Fire();
        }

        internal void __CompleteFromTyped(Task<TResult> source)
        {
            if (source.__state == 2) { __Fail(source.__exception); }
            else { __SetResult(source.__result); }
        }
    }

    /// A task completed by hand: `SetResult` from an event handler, a
    /// network callback, a button — whatever an `async` method is waiting for.
    public class TaskCompletionSource<TResult>
    {
        private readonly Task<TResult> task = new Task<TResult>();

        public TaskCompletionSource() { }

        public Task<TResult> Task { get { return task; } }

        public bool TrySetResult(TResult result)
        {
            if (task.IsCompleted) { return false; }
            task.__SetResult(result);
            return true;
        }

        public void SetResult(TResult result)
        {
            if (!TrySetResult(result))
            {
                throw new InvalidOperationException("the task has already completed");
            }
        }

        public bool TrySetException(Exception exception)
        {
            if (task.IsCompleted) { return false; }
            task.__Fail(exception);
            return true;
        }

        public void SetException(Exception exception)
        {
            if (!TrySetException(exception))
            {
                throw new InvalidOperationException("the task has already completed");
            }
        }
    }
}

namespace System.Threading
{
    /// Cancels what holds its token: the `StopCoroutine` of async code. An
    /// awaitable given the token faults with TaskCanceledException the
    /// moment `Cancel` is called, so the method awaiting it leaves through
    /// its `catch (OperationCanceledException)` — or, without one, ends.
    public class CancellationTokenSource
    {
        private bool cancelled;
        private Action[] callbacks = new Action[2];
        private int count;

        public CancellationTokenSource() { }

        public CancellationToken Token { get { return new CancellationToken(this); } }

        public bool IsCancellationRequested { get { return cancelled; } }

        public void Cancel()
        {
            if (cancelled) { return; }
            cancelled = true;
            for (int i = 0; i < count; i++)
            {
                Action callback = callbacks[i];
                callbacks[i] = null;
                callback();
            }
            count = 0;
        }

        /// Cancels after `millisecondsDelay` milliseconds of scene time.
        public void CancelAfter(int millisecondsDelay)
        {
            CancellationTokenSource source = this;
            MenSharp.Scheduler.Delay(millisecondsDelay / 1000f).__AddContinuation(() => source.Cancel());
        }

        /// Forgets a registered callback: what an awaitable does once it
        /// has completed on its own, so a long-lived token does not keep
        /// every delay it was ever given.
        internal void __Unregister(Action callback)
        {
            for (int i = 0; i < count; i++)
            {
                if (callbacks[i] == callback)
                {
                    for (int j = i + 1; j < count; j++)
                    {
                        callbacks[j - 1] = callbacks[j];
                    }
                    count--;
                    callbacks[count] = null;
                    return;
                }
            }
        }

        internal void __Register(Action callback)
        {
            if (cancelled)
            {
                callback();
                return;
            }
            if (count == callbacks.Length)
            {
                Action[] bigger = new Action[callbacks.Length * 2];
                for (int i = 0; i < count; i++)
                {
                    bigger[i] = callbacks[i];
                }
                callbacks = bigger;
            }
            callbacks[count] = callback;
            count++;
        }
    }

    public struct CancellationToken
    {
        private CancellationTokenSource source;

        public CancellationToken(CancellationTokenSource source) { this.source = source; }

        /// A token that is never cancelled.
        public static CancellationToken None { get { return new CancellationToken(); } }

        public bool CanBeCanceled { get { return source != null; } }

        public bool IsCancellationRequested { get { return source != null && source.IsCancellationRequested; } }

        public void ThrowIfCancellationRequested()
        {
            if (IsCancellationRequested)
            {
                throw new OperationCanceledException();
            }
        }

        /// Runs `callback` when the token is cancelled — at once, if it
        /// already is.
        public void Register(Action callback)
        {
            if (source != null)
            {
                source.__Register(callback);
            }
        }

        internal void __Unregister(Action callback)
        {
            if (source != null)
            {
                source.__Unregister(callback);
            }
        }
    }
}

namespace MenSharp
{
    using System.Threading;

    /// Where continuations wait, and the awaitables that put them there.
    public static class Scheduler
    {
        // continuations ready to run: drained at the end of every event
        private static Action[] ready = new Action[8];
        private static int readyHead;
        private static int readyCount;
        // continuations waiting for a moment in time / a frame number
        private static Action[] timedActions = new Action[4];
        private static float[] timedDue = new float[4];
        private static int timedCount;
        private static Action[] framedActions = new Action[4];
        private static int[] framedDue = new int[4];
        private static int framedCount;

        // ------------------------------------------------ awaitables

        /// Completes after `seconds` of scene time.
        public static Task Delay(float seconds)
        {
            Task task = new Task();
            __Schedule(() => task.__Complete(), 2, seconds, 0);
            return task;
        }

        /// Completes after `frames` frames (at least one).
        public static Task DelayFrames(int frames)
        {
            Task task = new Task();
            __Schedule(() => task.__Complete(), 3, 0f, frames);
            return task;
        }

        /// Completes on the next frame.
        public static Task NextFrame() { return DelayFrames(1); }

        // the same, faulting with TaskCanceledException as soon as the
        // token is cancelled
        public static Task Delay(float seconds, CancellationToken token)
        {
            return __Cancellable(Delay(seconds), token);
        }

        public static Task DelayFrames(int frames, CancellationToken token)
        {
            return __Cancellable(DelayFrames(frames), token);
        }

        public static Task NextFrame(CancellationToken token)
        {
            return __Cancellable(DelayFrames(1), token);
        }

        private static Task __Cancellable(Task task, CancellationToken token)
        {
            if (token.CanBeCanceled)
            {
                Action cancel = () => task.__Fail(new TaskCanceledException());
                token.Register(cancel);
                task.__AddContinuation(() => token.__Unregister(cancel));
            }
            return task;
        }

        /// Completes later in the current event, after every continuation
        /// queued before it: how a long computation lets others run.
        public static Task Yield()
        {
            Task task = new Task();
            __Enqueue(() => task.__Complete());
            return task;
        }

        /// Completes on the first frame `condition` is true.
        public static async Task WaitUntil(Func<bool> condition)
        {
            while (!condition())
            {
                await NextFrame();
            }
        }

        /// Completes on the first frame `condition` is false.
        public static async Task WaitWhile(Func<bool> condition)
        {
            while (condition())
            {
                await NextFrame();
            }
        }

        public static async Task WaitUntil(Func<bool> condition, CancellationToken token)
        {
            while (!condition())
            {
                await NextFrame(token);
            }
        }

        public static async Task WaitWhile(Func<bool> condition, CancellationToken token)
        {
            while (condition())
            {
                await NextFrame(token);
            }
        }

        /// Starts `body` and forgets it: a fault is logged when it happens
        /// instead of ending the event, since nothing awaits the task.
        public static Task Run(Func<Task> body)
        {
            Task task = body();
            task.__AddContinuation(() => __Report(task));
            return task;
        }

        private static void __Report(Task task)
        {
            if (task.IsFaulted)
            {
                Internal.Programs.LogError("Unobserved exception in a task started by Scheduler.Run: " + task.Exception);
            }
        }

        // ------------------------------------------------ the queues

        internal static void __Enqueue(Action continuation)
        {
            if (readyCount == ready.Length)
            {
                Action[] bigger = new Action[ready.Length * 2];
                int kept = 0;
                for (int i = readyHead; i < readyCount; i++)
                {
                    bigger[kept] = ready[i];
                    kept++;
                }
                ready = bigger;
                readyHead = 0;
                readyCount = kept;
            }
            ready[readyCount] = continuation;
            readyCount++;
        }

        /// mode 0: this event; 2: after `seconds`; 3: after `frames`.
        internal static void __Schedule(Action continuation, int mode, float seconds, int frames)
        {
            if (mode == 2)
            {
                if (timedCount == timedActions.Length)
                {
                    Action[] biggerActions = new Action[timedActions.Length * 2];
                    float[] biggerDue = new float[timedActions.Length * 2];
                    for (int i = 0; i < timedCount; i++)
                    {
                        biggerActions[i] = timedActions[i];
                        biggerDue[i] = timedDue[i];
                    }
                    timedActions = biggerActions;
                    timedDue = biggerDue;
                }
                timedActions[timedCount] = continuation;
                timedDue[timedCount] = Internal.Programs.Now() + seconds;
                timedCount++;
                Internal.Programs.ScheduleResume(seconds);
            }
            else if (mode == 3)
            {
                if (frames < 1) { frames = 1; }
                if (framedCount == framedActions.Length)
                {
                    Action[] biggerActions = new Action[framedActions.Length * 2];
                    int[] biggerDue = new int[framedActions.Length * 2];
                    for (int i = 0; i < framedCount; i++)
                    {
                        biggerActions[i] = framedActions[i];
                        biggerDue[i] = framedDue[i];
                    }
                    framedActions = biggerActions;
                    framedDue = biggerDue;
                }
                framedActions[framedCount] = continuation;
                framedDue[framedCount] = Internal.Programs.FrameCount() + frames;
                framedCount++;
                Internal.Programs.ScheduleResumeFrames(frames);
            }
            else
            {
                __Enqueue(continuation);
            }
        }

        internal static void __ScheduleCompletion(Task wrapper, Task source, int mode, float seconds, int frames)
        {
            __Schedule(() => wrapper.__CompleteFrom(source), mode, seconds, frames);
        }

        internal static void __ScheduleCompletionTyped<TResult>(Task<TResult> wrapper, Task<TResult> source, int mode, float seconds, int frames)
        {
            __Schedule(() => wrapper.__CompleteFromTyped(source), mode, seconds, frames);
        }

        /// Runs every continuation queued so far, and those they queue in
        /// turn. Called by the compiler at the end of every event.
        public static void __Drain()
        {
            while (readyHead < readyCount)
            {
                Action next = ready[readyHead];
                ready[readyHead] = null;
                readyHead++;
                if (Internal.Programs.IsRemoteContinuation(next))
                {
                    Internal.Programs.SendResume(next);
                }
                else
                {
                    next();
                }
            }
            readyHead = 0;
            readyCount = 0;
        }

        /// The `_mensharpResume` event: queues every continuation whose
        /// time or frame has come.
        public static void __OnResume()
        {
            // another behaviour may have sent one of our continuations home
            Action incoming = Internal.Programs.TakeIncomingResume();
            if (incoming != null)
            {
                __Enqueue(incoming);
            }
            float now = Internal.Programs.Now();
            int frame = Internal.Programs.FrameCount();
            int i = 0;
            while (i < timedCount)
            {
                if (timedDue[i] <= now)
                {
                    __Enqueue(timedActions[i]);
                    for (int j = i + 1; j < timedCount; j++)
                    {
                        timedActions[j - 1] = timedActions[j];
                        timedDue[j - 1] = timedDue[j];
                    }
                    timedCount--;
                    timedActions[timedCount] = null;
                }
                else
                {
                    i++;
                }
            }
            i = 0;
            while (i < framedCount)
            {
                if (framedDue[i] <= frame)
                {
                    __Enqueue(framedActions[i]);
                    for (int j = i + 1; j < framedCount; j++)
                    {
                        framedActions[j - 1] = framedActions[j];
                        framedDue[j - 1] = framedDue[j];
                    }
                    framedCount--;
                    framedActions[framedCount] = null;
                }
                else
                {
                    i++;
                }
            }
        }
    }
}

// The Unity-side twin of MenSharp's scheduler: the awaitables and timing
// helpers that .NET's Task does not have.
//
// These exist so that an M# script using them is valid Unity C# (IDE
// completion, drag-and-drop onto GameObjects). What executes on Udon is the
// compiler's own implementation in its mini-corlib; this component is
// stripped before play, so the bodies here never run.

using System;
using System.Threading;
using System.Threading.Tasks;

namespace MenSharp
{
    /// Awaitables that resume an `async` method later: after some seconds,
    /// after some frames, or once every continuation queued before it ran.
    public static class Scheduler
    {
        /// Completes after `seconds` of scene time.
        public static Task Delay(float seconds) => Task.CompletedTask;

        /// Completes after `frames` frames (at least one).
        public static Task DelayFrames(int frames) => Task.CompletedTask;

        /// Completes on the next frame.
        public static Task NextFrame() => Task.CompletedTask;

        // the same, faulting with TaskCanceledException as soon as the
        // token is cancelled
        public static Task Delay(float seconds, CancellationToken token) => Task.CompletedTask;

        public static Task DelayFrames(int frames, CancellationToken token) => Task.CompletedTask;

        public static Task NextFrame(CancellationToken token) => Task.CompletedTask;

        /// Completes later in the current event, after every continuation
        /// queued before it: how a long computation lets others run.
        public static Task Yield() => Task.CompletedTask;

        /// Completes on the first frame `condition` is true.
        public static Task WaitUntil(Func<bool> condition) => Task.CompletedTask;

        /// Completes on the first frame `condition` is false.
        public static Task WaitWhile(Func<bool> condition) => Task.CompletedTask;

        public static Task WaitUntil(Func<bool> condition, CancellationToken token) => Task.CompletedTask;

        public static Task WaitWhile(Func<bool> condition, CancellationToken token) => Task.CompletedTask;

        /// Starts `body` and forgets it: a fault is logged when it happens
        /// instead of ending the event, since nothing awaits the task.
        public static Task Run(Func<Task> body) => body();
    }

    /// What awaiting a faulted task of *another* behaviour throws. The
    /// exception that behaviour raised cannot cross a program boundary: its
    /// type is a number that means something only inside the program that
    /// made it, so what arrives is the text it printed as.
    public class RemoteTaskException : Exception
    {
        public RemoteTaskException() : base("A task from another behaviour faulted.") { }
        public RemoteTaskException(string message) : base(message) { }
    }

    /// When a task's continuation runs: on M# every task resumes what awaits
    /// it in the same event by default; these make a task that completes
    /// on a later frame or after a delay instead.
    public static class TaskTimingExtensions
    {
        /// A task that completes on the frame after this one does.
        public static Task OnNextFrame(this Task task) => task;

        public static Task<T> OnNextFrame<T>(this Task<T> task) => task;

        /// A task that completes `seconds` after this one does.
        public static Task After(this Task task, float seconds) => task;

        public static Task<T> After<T>(this Task<T> task, float seconds) => task;

        /// A task that completes `frames` frames after this one does.
        public static Task AfterFrames(this Task task, int frames) => task;

        public static Task<T> AfterFrames<T>(this Task<T> task, int frames) => task;
    }
}

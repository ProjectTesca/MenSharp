// MenSharp: `Result<T, E>` and `Option<T>` — outcomes as values.
//
// Udon halts a behaviour for good on an unhandled exception, so the things
// that fail as a matter of course — a download, a parse, a lookup — should
// not throw. They return one of these instead: a closed union of the two
// outcomes, checked for completeness in a `switch` (`[Union]`), and
// carrying the helpers that make the common cases one line:
//
//     Result<Config, HttpError> loaded = await Http.GetJson<Config>(url);
//     loaded.Switch(config => Apply(config), error => label.text = error.Message);
//     int size = loaded.Match(config => config.size, error => 1);
//     if (loaded.TryGet(out var config, out var error)) { ... }
//
// Producing one is as short as consuming one: `using static MenSharp.Result;`
// makes `return Ok(value);` and `return Err(error);` valid in any method
// that returns a `Result<T, E>` — `Ok` builds an `OkValue<T>` that does not
// know `E` yet, and the implicit conversion on `Result<T, E>` supplies it
// from the return type, as the LanguageExt / ErrorOr libraries do.
//
// This file is compiled by MenSharp as part of its corlib and, verbatim,
// by Unity as part of the runtime assembly, so the two agree.

using System;

namespace MenSharp
{
    /// Either a value (`Ok`) or an error (`Err`); nothing else. A `switch`
    /// over it that forgets a case is a compile error.
    [Union]
    public abstract class Result<T, E>
    {
        protected Result()
        {
        }

        public sealed class Ok : Result<T, E>
        {
            public new readonly T Value;

            public Ok(T value)
            {
                Value = value;
            }

            public void Deconstruct(out T value)
            {
                value = Value;
            }
        }

        public sealed class Err : Result<T, E>
        {
            public new readonly E Error;

            public Err(E error)
            {
                Error = error;
            }

            public void Deconstruct(out E error)
            {
                error = Error;
            }
        }

        /// `return Ok(value);` — the `E` comes from the type returned into.
        public static implicit operator Result<T, E>(OkValue<T> ok)
        {
            return new Ok(ok.Value);
        }

        /// `return Err(error);` — the `T` comes from the type returned into.
        public static implicit operator Result<T, E>(ErrValue<E> err)
        {
            return new Err(err.Error);
        }

        public bool IsOk
        {
            get { return this is Ok; }
        }

        public bool IsErr
        {
            get { return this is Err; }
        }

        /// The value; an `InvalidOperationException` on an `Err`.
        public T Value
        {
            get
            {
                if (this is Ok ok)
                {
                    return ok.Value;
                }
                throw new InvalidOperationException("Result.Value on an Err: " + ToString());
            }
        }

        /// The error; an `InvalidOperationException` on an `Ok`.
        public E Error
        {
            get
            {
                if (this is Err err)
                {
                    return err.Error;
                }
                throw new InvalidOperationException("Result.Error on an Ok: " + ToString());
            }
        }

        /// Both outcomes at once: `true` with the value, `false` with the
        /// error (the other `out` is its default).
        public bool TryGet(out T value, out E error)
        {
            if (this is Ok ok)
            {
                value = ok.Value;
                error = default(E);
                return true;
            }
            value = default(T);
            error = ((Err)this).Error;
            return false;
        }

        /// One of two functions, by outcome, giving the answer.
        public R Match<R>(Func<T, R> ok, Func<E, R> err)
        {
            if (this is Ok o)
            {
                return ok(o.Value);
            }
            return err(((Err)this).Error);
        }

        /// One of two actions, by outcome.
        public void Switch(Action<T> ok, Action<E> err)
        {
            if (this is Ok o)
            {
                ok(o.Value);
            }
            else
            {
                err(((Err)this).Error);
            }
        }

        /// The value transformed; an error passes through.
        public Result<U, E> Map<U>(Func<T, U> map)
        {
            if (this is Ok ok)
            {
                return new Result<U, E>.Ok(map(ok.Value));
            }
            return new Result<U, E>.Err(((Err)this).Error);
        }

        /// The error transformed; a value passes through.
        public Result<T, F> MapError<F>(Func<E, F> map)
        {
            if (this is Ok ok)
            {
                return new Result<T, F>.Ok(ok.Value);
            }
            return new Result<T, F>.Err(map(((Err)this).Error));
        }

        /// The next step, taken only on a value: the way to chain
        /// operations that can each fail.
        public Result<U, E> AndThen<U>(Func<T, Result<U, E>> next)
        {
            if (this is Ok ok)
            {
                return next(ok.Value);
            }
            return new Result<U, E>.Err(((Err)this).Error);
        }

        /// The value, or `fallback` on an error.
        public T UnwrapOr(T fallback)
        {
            if (this is Ok ok)
            {
                return ok.Value;
            }
            return fallback;
        }

        /// The value; an `InvalidOperationException` on an `Err`. For the
        /// places where an error really is a bug.
        public T Unwrap()
        {
            return Value;
        }

        public override string ToString()
        {
            if (this is Ok ok)
            {
                return "Ok(" + ok.Value + ")";
            }
            return "Err(" + ((Err)this).Error + ")";
        }
    }

    /// What `Result.Ok(value)` gives: a value that becomes a `Result<T, E>`
    /// for whichever `E` it is assigned, returned or passed into.
    public sealed class OkValue<T>
    {
        public readonly T Value;

        public OkValue(T value)
        {
            Value = value;
        }
    }

    /// What `Result.Err(error)` gives: see `OkValue`.
    public sealed class ErrValue<E>
    {
        public readonly E Error;

        public ErrValue(E error)
        {
            Error = error;
        }
    }

    /// The constructors: `Result.Ok(v)` / `Result.Err(e)`, or bare `Ok(v)`
    /// / `Err(e)` after `using static MenSharp.Result;`.
    public static class Result
    {
        public static OkValue<T> Ok<T>(T value)
        {
            return new OkValue<T>(value);
        }

        public static ErrValue<E> Err<E>(E error)
        {
            return new ErrValue<E>(error);
        }
    }

    /// A value that may be absent: `Some` or `None`, and nothing else.
    [Union]
    public abstract class Option<T>
    {
        protected Option()
        {
        }

        public sealed class Some : Option<T>
        {
            public readonly T Value;

            public Some(T value)
            {
                Value = value;
            }

            public void Deconstruct(out T value)
            {
                value = Value;
            }
        }

        public sealed class None : Option<T>
        {
        }

        public static implicit operator Option<T>(SomeValue<T> some)
        {
            return new Some(some.Value);
        }

        public static implicit operator Option<T>(NoneValue none)
        {
            return new None();
        }

        public bool IsSome
        {
            get { return this is Some; }
        }

        public bool IsNone
        {
            get { return this is None; }
        }

        public bool TryGet(out T value)
        {
            if (this is Some some)
            {
                value = some.Value;
                return true;
            }
            value = default(T);
            return false;
        }

        public R Match<R>(Func<T, R> some, Func<R> none)
        {
            if (this is Some s)
            {
                return some(s.Value);
            }
            return none();
        }

        public void Switch(Action<T> some, Action none)
        {
            if (this is Some s)
            {
                some(s.Value);
            }
            else
            {
                none();
            }
        }

        public Option<U> Map<U>(Func<T, U> map)
        {
            if (this is Some some)
            {
                return new Option<U>.Some(map(some.Value));
            }
            return new Option<U>.None();
        }

        public T UnwrapOr(T fallback)
        {
            if (this is Some some)
            {
                return some.Value;
            }
            return fallback;
        }

        /// The value as a `Result`, with `error` standing in for `None`.
        public Result<T, E> OkOr<E>(E error)
        {
            if (this is Some some)
            {
                return new Result<T, E>.Ok(some.Value);
            }
            return new Result<T, E>.Err(error);
        }

        public override string ToString()
        {
            if (this is Some some)
            {
                return "Some(" + some.Value + ")";
            }
            return "None";
        }
    }

    public sealed class SomeValue<T>
    {
        public readonly T Value;

        public SomeValue(T value)
        {
            Value = value;
        }
    }

    public sealed class NoneValue
    {
    }

    /// `Option.Some(v)` / `Option.None`, or bare `Some(v)` / `None` after
    /// `using static MenSharp.Option;`.
    public static class Option
    {
        public static SomeValue<T> Some<T>(T value)
        {
            return new SomeValue<T>(value);
        }

        public static NoneValue None
        {
            get { return new NoneValue(); }
        }
    }
}

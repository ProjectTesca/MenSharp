// MenSharp verification: the std's Result<T, E> and Option<T> — a closed
// union with Match/Switch/TryGet, built with `return Ok(x)` / `Err(e)`
// through `using static`, and `Result<int, string>.Ok` as a nested type.
//
// Setup: a Cube "VerifyResult" with this component. Play, click.
//
// Expected:
//   [verify-result] 1 switch: ok 42 / err nan x
//   [verify-result] 2 match: 84 / -1
//   [verify-result] 3 tryget: got 42 / failed nan x
//   [verify-result] 4 combinators: Ok(43) / Err(5) / 7 / 9
//   [verify-result] 5 patterns: case ok 42 / case err nan x / True True
//   [verify-result] 6 option: Some(3) / None / 5 / Err(neg) / Some(20)

using MenSharp;
using UnityEngine;
using static MenSharp.Option;
using static MenSharp.Result;

public class VerifyResult : MenSharpBehaviour
{
    private static Result<int, string> Parse(string text)
    {
        int value;
        if (!int.TryParse(text, out value)) return Err("nan " + text);
        return Ok(value);
    }

    private static Option<int> Positive(int value)
    {
        if (value > 0) return Some(value);
        return None;
    }

    private static string Describe(Result<int, string> result)
    {
        switch (result)
        {
            case Result<int, string>.Ok(var value): return "case ok " + value;
            case Result<int, string>.Err(var error): return "case err " + error;
        }
        // MenSharp knows the switch is exhaustive; Unity's compiler does not
        throw new System.InvalidOperationException("unreachable");
    }

    public void Interact()
    {
        Result<int, string> good = Parse("42");
        Result<int, string> bad = Parse("x");

        string switched = "";
        good.Switch(v => switched += "ok " + v, e => switched += "err " + e);
        bad.Switch(v => switched += "ok " + v, e => switched += " / err " + e);
        Debug.Log("[verify-result] 1 switch: " + switched);

        Debug.Log("[verify-result] 2 match: " + good.Match(v => v * 2, e => -1) + " / " + bad.Match(v => v * 2, e => -1));

        int value;
        string error;
        string got = good.TryGet(out value, out error) ? "got " + value : "failed " + error;
        string failed = bad.TryGet(out value, out error) ? "got " + value : "failed " + error;
        Debug.Log("[verify-result] 3 tryget: " + got + " / " + failed);

        Debug.Log("[verify-result] 4 combinators: " + good.Map(v => v + 1) + " / " + bad.MapError(e => e.Length)
            + " / " + good.AndThen(v => Parse("7")).UnwrapOr(0) + " / " + bad.UnwrapOr(9));

        Debug.Log("[verify-result] 5 patterns: " + Describe(good) + " / " + Describe(bad) + " / " + good.IsOk + " " + bad.IsErr);

        Debug.Log("[verify-result] 6 option: " + Positive(3) + " / " + Positive(-3) + " / " + Positive(5).UnwrapOr(0)
            + " / " + Positive(-1).OkOr("neg") + " / " + Positive(2).Map(v => v * 10));
    }
}

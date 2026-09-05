// MenSharp verification: try/finally around a yield return.
//
// Setup: a Cube "VerifyIteratorCleanup" with this component. Play, click.
//
// Expected:
//   [verify-cleanup] 1 to the end: open;1;mid;2;close;
//   [verify-cleanup] 2 break: open;1;close;
//   [verify-cleanup] 3 return: got1 after open;close;
//   [verify-cleanup] 4 exception: open;close;caught boom;
//   [verify-cleanup] 5 nested: inner;outer;
//   [verify-cleanup] 6 yield break: e1;efin;
//   [verify-cleanup] 7 never started: nothing ran, log empty True

using System;
using System.Collections.Generic;
using MenSharp;
using UnityEngine;

public class VerifyIteratorCleanup : MenSharpBehaviour
{
    private string log;

    private IEnumerable<int> Guarded()
    {
        log += "open;";
        try
        {
            yield return 1;
            log += "mid;";
            yield return 2;
        }
        finally { log += "close;"; }
    }

    private IEnumerable<int> Nested()
    {
        try
        {
            try { yield return 1; }
            finally { log += "inner;"; }
        }
        finally { log += "outer;"; }
    }

    private IEnumerable<int> Early()
    {
        try { yield return 1; yield break; }
        finally { log += "efin;"; }
    }

    private string FirstOf()
    {
        foreach (int n in Guarded()) { return "got" + n; }
        return "none";
    }

    public void Interact()
    {
        log = "";
        foreach (int n in Guarded()) { log += n + ";"; }
        Debug.Log($"[verify-cleanup] 1 to the end: {log}");

        log = "";
        foreach (int n in Guarded()) { log += n + ";"; break; }
        Debug.Log($"[verify-cleanup] 2 break: {log}");

        log = "";
        string first = FirstOf();
        Debug.Log($"[verify-cleanup] 3 return: {first} after {log}");

        log = "";
        try
        {
            foreach (int n in Guarded()) { throw new InvalidOperationException("boom"); }
        }
        catch (InvalidOperationException e) { log += "caught " + e.Message + ";"; }
        Debug.Log($"[verify-cleanup] 4 exception: {log}");

        log = "";
        foreach (int n in Nested()) { break; }
        Debug.Log($"[verify-cleanup] 5 nested: {log}");

        log = "";
        foreach (int n in Early()) { log += "e" + n + ";"; }
        Debug.Log($"[verify-cleanup] 6 yield break: {log}");

        log = "";
        IEnumerable<int> unused = Guarded();
        Debug.Log($"[verify-cleanup] 7 never started: nothing ran, log empty {log == ""}");
    }
}

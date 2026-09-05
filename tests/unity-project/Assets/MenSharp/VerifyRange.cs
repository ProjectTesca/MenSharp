// MenSharp verification: `a[^1]` (from the end) and `a[1..3]` (slices).
//
// Setup: a Cube "VerifyRange" with this component. Play, click.
//
// Expected:
//   [verify-range] 1 from the end: [^1]=5, [^5]=1, [^n]=4, after [^1]=50 -> 50
//   [verify-range] 2 slices: [1..4]={2,3,4}, [3..]={4,50}, [..2]={1,2}, [..]=5, [2..2]=0
//   [verify-range] 3 from-end slices: [^2..^1]={4}, [^2..]={4,50}
//   [verify-range] 4 strings: word[^1]=o, [1..3]=el, [^2..]=lo, [..1]=h
//   [verify-range] 5 copies: slice[0]=99 leaves the array at 2; [3..1] threw ArgumentOutOfRangeException

using System;
using MenSharp;
using UnityEngine;

public class VerifyRange : MenSharpBehaviour
{
    public string word = "hello";

    private string Join(int[] values)
    {
        string text = "";
        for (int i = 0; i < values.Length; i++)
        {
            if (i > 0) text += ",";
            text += values[i];
        }
        return "{" + text + "}";
    }

    public void Interact()
    {
        int[] numbers = { 1, 2, 3, 4, 5 };
        int n = 2;
        int last = numbers[^1];
        int first = numbers[^5];
        int nth = numbers[^n];
        numbers[^1] = 50;
        Debug.Log($"[verify-range] 1 from the end: [^1]={last}, [^5]={first}, [^n]={nth}, after [^1]=50 -> {numbers[4]}");

        int[] middle = numbers[1..4];
        int[] tail = numbers[3..];
        int[] head = numbers[..2];
        int[] all = numbers[..];
        int[] none = numbers[2..2];
        Debug.Log($"[verify-range] 2 slices: [1..4]={Join(middle)}, [3..]={Join(tail)}, [..2]={Join(head)}, [..]={all.Length}, [2..2]={none.Length}");

        int[] fromEnd = numbers[^2..^1];
        int[] lastTwo = numbers[^2..];
        Debug.Log($"[verify-range] 3 from-end slices: [^2..^1]={Join(fromEnd)}, [^2..]={Join(lastTwo)}");

        char letter = word[^1];
        string inner = word[1..3];
        string end = word[^2..];
        string start = word[..1];
        Debug.Log($"[verify-range] 4 strings: word[^1]={letter}, [1..3]={inner}, [^2..]={end}, [..1]={start}");

        middle[0] = 99;
        string threw = "did not throw";
        try
        {
            int[] bad = numbers[3..1];
            threw = "no throw, length " + bad.Length;
        }
        catch (ArgumentOutOfRangeException)
        {
            threw = "threw ArgumentOutOfRangeException";
        }
        Debug.Log($"[verify-range] 5 copies: slice[0]=99 leaves the array at {numbers[1]}; [3..1] {threw}");
    }
}

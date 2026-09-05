// MenSharp verification: IEnumerable<T> across every kind of sequence.
//
// Setup: a Cube "VerifySequences" with this component. Play, click.
//
// Expected:
//   [verify-seq] 1 list: 1,2,3, total 6, direct foreach 6
//   [verify-seq] 2 dictionary: keys ann,bob, values total 70, pairs ann=30;bob=40;
//   [verify-seq] 3 array: total 15, words p,q, via variable 15, by index 15
//   [verify-seq] 4 iterator: 1,4,9,16, total 14, lazy True
//   [verify-seq] 5 string: hey and o,k

using System.Collections.Generic;
using MenSharp;
using UnityEngine;

public class VerifySequences : MenSharpBehaviour
{
    private int made;

    private string Join<T>(IEnumerable<T> items)
    {
        string text = "";
        foreach (T item in items) { text += item + ","; }
        return text;
    }

    private int Total(IEnumerable<int> numbers)
    {
        int sum = 0;
        foreach (int n in numbers) { sum += n; }
        return sum;
    }

    private IEnumerable<int> Squares(int count)
    {
        for (int i = 1; i <= count; i++) { made++; yield return i * i; }
    }

    public void Interact()
    {
        List<int> numbers = new List<int>();
        numbers.Add(1);
        numbers.Add(2);
        numbers.Add(3);
        int direct = 0;
        foreach (int n in numbers) { direct += n; }
        Debug.Log($"[verify-seq] 1 list: {Join<int>(numbers)} total {Total(numbers)}, direct foreach {direct}");

        Dictionary<string, int> ages = new Dictionary<string, int>();
        ages["ann"] = 30;
        ages["bob"] = 40;
        string pairs = "";
        foreach (KeyValuePair<string, int> pair in ages) { pairs += pair.Key + "=" + pair.Value + ";"; }
        Debug.Log($"[verify-seq] 2 dictionary: keys {Join<string>(ages.Keys).TrimEnd(',')}, values total {Total(ages.Values)}, pairs {pairs}");

        int[] fixedNumbers = new int[] { 4, 5, 6 };
        IEnumerable<int> sequence = fixedNumbers;
        int byIndex = 0;
        foreach (int n in fixedNumbers) { byIndex += n; }
        Debug.Log($"[verify-seq] 3 array: total {Total(fixedNumbers)}, words {Join<string>(new string[] { "p", "q" }).TrimEnd(',')}, via variable {Total(sequence)}, by index {byIndex}");

        made = 0;
        IEnumerable<int> squares = Squares(4);
        bool lazy = made == 0;
        Debug.Log($"[verify-seq] 4 iterator: {Join<int>(squares).TrimEnd(',')}, total {Total(Squares(3))}, lazy {lazy}");

        IEnumerable<char> letters = "hey";
        string spelled = "";
        foreach (char c in letters) { spelled += c; }
        Debug.Log($"[verify-seq] 5 string: {spelled} and {Join<char>("ok").TrimEnd(',')}");
    }
}

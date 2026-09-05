// MenSharp verification: string indexing — text[i].
//
// Setup: a Cube "VerifyString" with this component. Play, click.
//
// Expected:
//   [verify-string] 1 read: word[0]=h, word[4]=o, last=o, At("abc",1)=b
//   [verify-string] 2 compare: word[1]=='e' True, letters=5, built='hello' matches True
//   [verify-string] 3 mixed: "abc"[2]=c, Substring(1,3)[1]=l, ToCharArray()[0]=h, field[0]=h
//   [verify-string] 4 bounds: word[9] threw IndexOutOfRangeException, word[-1] threw too

using System;
using MenSharp;
using UnityEngine;

public class VerifyString : MenSharpBehaviour
{
    public string word = "hello";

    private char At(string text, int index) { return text[index]; }

    public void Interact()
    {
        char first = word[0];
        char fifth = word[4];
        char last = word[word.Length - 1];
        Debug.Log($"[verify-string] 1 read: word[0]={first}, word[4]={fifth}, last={last}, At(\"abc\",1)={At("abc", 1)}");

        int letters = 0;
        for (int i = 0; i < word.Length; i++)
        {
            if (char.IsLetter(word[i])) letters++;
        }
        string built = "";
        foreach (char c in word) built += c;
        Debug.Log($"[verify-string] 2 compare: word[1]=='e' {word[1] == 'e'}, letters={letters}, built='{built}' matches {built == word}");

        char literal = "abc"[2];
        char inner = word.Substring(1, 3)[1];
        char[] all = word.ToCharArray();
        Debug.Log($"[verify-string] 3 mixed: \"abc\"[2]={literal}, Substring(1,3)[1]={inner}, ToCharArray()[0]={all[0]}, field[0]={word[0]}");

        string high = "";
        string low = "";
        try { high = word[9].ToString(); }
        catch (IndexOutOfRangeException) { high = "threw IndexOutOfRangeException"; }
        try { low = word[-1].ToString(); }
        catch (IndexOutOfRangeException) { low = "threw too"; }
        Debug.Log($"[verify-string] 4 bounds: word[9] {high}, word[-1] {low}");
    }
}

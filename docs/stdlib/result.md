---
icon: lucide/git-branch
description: Result<T, E> と Option<T> の分岐
---

# `Result<T, E>` と `Option<T>`

`Result<T, E>` は成功値とエラーを一つの値で返します。`Ok` / `Err` は `using static MenSharp.Result` で裸の名前として使えます。型引数は戻り値の型から決まります。

次の例には、網羅的な `switch`、`Switch` / `Match`、`TryGet`、`Map` / `AndThen`、そして `Option<T>` が含まれます。`switch` の case を書き漏らすとコンパイルエラーになります。

```cs title="ExResult.cs"
using MenSharp;
using UnityEngine;
using static MenSharp.Result;   // Ok(...) / Err(...) を裸の名前で

public class ExResult : MenSharpBehaviour
{
    public string input;

    private Result<int, string> ParseCount(string text)
    {
        int value;
        if (!int.TryParse(text, out value))
        {
            return Err("not a number: " + text);   // E は戻り値型から決まる
        }
        if (value < 0)
        {
            return Err("negative: " + value);
        }
        return Ok(value);                          // T も同様
    }

    public void Start()
    {
        Result<int, string> parsed = ParseCount(input);

        // 1. 網羅的 switch — case を忘れるとコンパイルエラー
        switch (parsed)
        {
            case Result<int, string>.Ok(var count):
                Debug.Log("count = " + count);
                break;
            case Result<int, string>.Err(var error):
                Debug.LogWarning(error);
                break;
        }

        // 2. 二択のアクション / 二択の答え
        parsed.Switch(count => Debug.Log(count), error => Debug.LogWarning(error));
        int safe = parsed.Match(count => count, error => 0);

        // 3. out 形式
        int value;
        string why;
        if (parsed.TryGet(out value, out why))
        {
            Debug.Log("ok " + value);
        }

        // 4. コンビネータ
        Result<int, string> doubled = parsed.Map(count => count * 2);
        Result<int, string> chained = parsed.AndThen(count => ParseCount(count.ToString()));
        Debug.Log(doubled.UnwrapOr(0) + " " + chained.IsOk + " " + safe);

        // Option<T>
        Option<int> maybe = Find(3);
        Debug.Log(maybe.Match(v => "found " + v, () => "none"));
        Result<int, string> required = maybe.OkOr("not there");
        Debug.Log(required.ToString());
    }

    private Option<int> Find(int wanted)
    {
        int[] values = new int[] { 1, 2, 3 };
        foreach (int value in values)
        {
            if (value == wanted)
            {
                return Option.Some(value);
            }
        }
        return Option.None;
    }
}
```

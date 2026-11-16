---
icon: lucide/globe
description: MenSharp.Net.Http で文字列と JSON を取得する
---

# HTTP

`MenSharp.Net.Http` と async/await でリクエストを送ります。本文は `Http.GetString`、JSON は `Http.GetJson<T>` です。どちらも [`Result<T, HttpError>`](result.md) を返します。URL は SDK の要求どおり、Inspector の `VRCUrl` で設定します。

```cs title="ExHttp.cs"
using MenSharp;
using MenSharp.Net;
using UnityEngine;
using VRC.SDKBase;

public class Motd
{
    public string message;
}

public class ExHttp : MenSharpBehaviour
{
    public VRCUrl url;          // SDK の要求どおり Inspector で設定

    public async void Interact()
    {
        // 生テキスト
        Result<string, HttpError> text = await Http.GetString(url);
        text.Switch(
            body => Debug.Log("got " + body.Length + " chars"),
            error => Debug.LogWarning(error.ToString()));   // "400: Invalid URL"

        // そのまま型へ
        Result<Motd, HttpError> loaded = await Http.GetJson<Motd>(url);
        switch (loaded)
        {
            case Result<Motd, HttpError>.Ok(var motd):
                Debug.Log(motd.message);
                break;
            case Result<Motd, HttpError>.Err(var error):
                Debug.LogWarning(error.Code + " " + error.Message);
                break;
        }
    }
}
```

`switch` で分岐を書き漏らすとコンパイルエラーになります。詳しくは [`Result<T, E>` と `Option<T>`](result.md) を参照してください。

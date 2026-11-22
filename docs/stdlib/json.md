---
icon: lucide/braces
description: MenSharp.Json で JSON を読み書きする
---

# JSON

`MenSharp.Json` の `Json.Parse<T>` と `Json.TryParse` で JSON を型に変換します。フィールド名は `JsonName`、シリアライズから外すフィールドは `JsonIgnore`、必須フィールドは `JsonRequired` です。失敗時の `JsonError` はパスとメッセージを持ちます。

```cs title="ExJson.cs"
using MenSharp;
using MenSharp.Json;
using UnityEngine;

public class Player
{
    public string name;
    [JsonName("level")] public int Level { get; set; }
    [JsonIgnore] public float cachedScore;
}
public class Config
{
    [JsonRequired] public string title;
    public float size;
    public float[] color;
    public System.Collections.Generic.List<Player> players;
    public int? maxPlayers;
}
public class ExJson : MenSharpBehaviour
{
    public void Start()
    {
        string text = "{\"title\":\"hello\",\"size\":2.5,\"color\":[1,0,0]," +
                    "\"players\":[{\"name\":\"a\",\"level\":3}],\"maxPlayers\":16}";

        Result<Config, JsonError> parsed = Json.Parse<Config>(text);
        switch (parsed)
        {
            case Result<Config, JsonError>.Ok(var config):
                Debug.Log(config.title + " " + config.players[0].Level + " " + config.maxPlayers);
                break;
            case Result<Config, JsonError>.Err(var error):
                Debug.LogWarning(error.Path + ": " + error.Message);   // "$.players[0].level: ..."
                break;
        }
        // out 形式
        Config value;
        string why;
        if (Json.TryParse<Config>(text, out value, out why))
        {
            Debug.Log(value.size);
        }
    }
}
```

成功と失敗の分岐は [`Result<T, E>`](result.md) と同じです。

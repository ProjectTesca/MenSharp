<div align="center">
<h1>MenSharpコンパイラー(M#)</h1>
<p>C# → UdonAssemblyへコンパイルする世界最速のC#コンパイラ</p>

<a href="https://discord.gg/uA5gGmFpRe" data-size="large">
  <img alt="Discord" src="https://img.shields.io/discord/1546759195267825755.svg?label=Discord&logo=Discord&colorB=7289da&style=for-the-badge">
</a>

<p><a href="./README.md">English version here</a></p>

</div>

## MenSharp(M#)とは？

M#はC#からUdonAssemblyへのコンパイラでありながら、現在（2026/09/09 13:00）最速のC#コンパイラでもあります。

C#公式のコンパイラであるRoslynよりも最大8倍高速に動作します。ただし、現在は.NETバックエンドを搭載していません。

このプロジェクトはAIを活用していますが**バイブコーディングではありません**。

もし、PRを送る場合はその内容をすべて理解したうえで送っていただく必要があります。

詳しくは[CONTRIBUTING.md](./CONTRIBUTING.md)を参照してください。


## サポートする表記

サポートする記法はC#の**ほぼ全て**です。

現状でサポートしていないのは、Thread, lock, unsafe, FileIO, VRC SDKを経由しないネットワークアクセス等のごく一部の機能のみです。

```cs
public class Test : MenSharpBehaviour
{
    [UdonSynced]
    private int value;

    public async void Interact() {
        var list = new List<string> { "Hello", "world!" };

        // async/awaitやtry/catchもサポート！
        string result;
        try {
            result = await AsyncFunction();
        } catch (Exception exception) {
            Debug.LogWarning($"{exception}");
            return;
        }

        list.Add(result);

        // LINQもサポート！
        var linq = list.Select(value => value.ToLowerInvariant());

        foreach (var value in linq) {
            Debug.Log(value);
        }
    }
}
```

## ドキュメント

(docs/doc.md)[./docs/doc.md]

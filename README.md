<div align="center">
<h1>MenSharp Compiler (M#)</h1>
<p>The world's fastest C# compiler, compiling C# into Udon Assembly</p>

<a href="https://discord.gg/uA5gGmFpRe" data-size="large">
  <img alt="Discord" src="https://img.shields.io/discord/1546759195267825755.svg?label=Discord&logo=Discord&colorB=7289da&style=for-the-badge">
</a>

<p><a href="./README.ja.md">日本語版はこちら</a></p>

</div>

## What is MenSharp (M#)?

M# is a compiler from C# to Udon Assembly, and at the same time it is currently (2026/09/09 13:00) the fastest C# compiler.

It runs up to 8 times faster than Roslyn, the official C# compiler. Note that it does not have a .NET backend at the moment.

This project makes use of AI, but it is **NOT vibe coding**.

If you send a PR, you are required to fully understand its contents before doing so.

See [CONTRIBUTING.md](./CONTRIBUTING.md) for details.


## Supported syntax

**Almost all** of C#'s syntax is supported.

The only things not supported right now are a small set of features such as Thread, lock, unsafe, file I/O, and network access that does not go through the VRC SDK.

```cs
public class Test : MenSharpBehaviour
{
    [UdonSynced]
    private int value;

    public async void Interact() {
        var list = new List<string> { "Hello", "world!" };

        // async/await and try/catch are supported too!
        string result;
        try {
            result = await AsyncFunction();
        } catch (Exception exception) {
            Debug.LogWarning($"{exception}");
            return;
        }

        list.Add(result);

        // LINQ is supported too!
        var linq = list.Select(value => value.ToLowerInvariant());

        foreach (var value in linq) {
            Debug.Log(value);
        }
    }
}
```

## Documentation

(docs/doc.md)[./docs/doc.md]

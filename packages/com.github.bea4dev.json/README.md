# JSON

Typed JSON for MenSharp: `Json.Parse<T>` reads JSON text into your own
classes, `Json.Stringify<T>` writes them back. The parsing itself is the
VRChat SDK's native `VRCJson`; what this package adds is the binding from a
`DataToken` tree to a `T`, generated per type by the MenSharp compiler
through `MenSharp.Reflection` — no reflection at run time, no lookups by
name, and a field of a type the package cannot map is a compile error that
names the type.

```csharp
using Bea4dev.Json;

public class Player { public string name; public int level; }

public class Config
{
    [JsonRequired] public string title;
    public float ratio;
    public Player[] players;
    public List<int> scores;
    public int? limit;
    [JsonName("player_name")] public string playerName;
    [JsonIgnore] public int cached;
}

Config config = Json.Parse<Config>(text);      // throws JsonException on a mismatch
string text = Json.Stringify(config);           // one line; Stringify(config, true) indents

Config parsed;
string error;                                   // "$.players[2].level: expected a number, found \"seven\""
if (Json.TryParse(text, out parsed, out error)) { ... }
```

## What maps

| C# | JSON |
|---|---|
| `bool`, `int`, `long`, `float`, `double` | `true`/`false`, numbers |
| `string` | string, or `null` |
| enums (your own) | their number |
| `T?` of the above | the value, or `null` |
| `T[]`, `List<T>` | array |
| a class, struct or record with a parameterless constructor | object: every public field and auto-property, base class first |

Reading is lenient where JSON usually is: a key missing from an object leaves
the member at its default (unless it is `[JsonRequired]`), and keys the type
has no member for are ignored. A value of the wrong shape is an error naming
its path.

Attributes on a field or property:

- `[JsonName("key")]` — read from and write to `key` instead of the member's name.
- `[JsonIgnore]` — leave a public member out.
- `[JsonInclude]` — take a non-public member in (a `[JsonName]` on it does the same).
- `[JsonRequired]` — fail when the key is missing.

Not (yet) mapped: `Dictionary<string, T>`, enums by name, positional records
(they have no parameterless constructor), engine types such as `Vector3`.
Naming one of these in a type you parse is a compile error, not a surprise
at run time.

## Requirements

- MenSharp (`com.projecttesca.mensharp`) with `MenSharp.Reflection`.
- VRChat Worlds SDK 3.3.0 or later (`VRCJson`).

## Layout

`Runtime/` holds the sources; MenSharp compiles them into whichever of your
behaviours uses them, so the package ships no programs of its own. The
`VerifyJson` fixture in the MenSharp repository's Unity test project is the
package's test: it runs `Parse`/`Stringify` in the SDK's Udon VM and compares
the log with the fixture's header.

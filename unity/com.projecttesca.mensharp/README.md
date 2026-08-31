# MenSharp (M#)

A UdonSharp-compatible C# compiler for VRChat Udon, written in Rust.
Classes, generics, `List<T>`, string interpolation and virtual dispatch —
compiled to Udon assembly and validated against the SDK's extern whitelist
at compile time.

## Getting started

1. Install this package with ALCOM or the VRChat Creator Companion.
2. In your project, create `Assets/MenSharp/` and put your `.cs` sources there
   (running **MenSharp > Compile All** once creates the folder for you).
3. Write behaviours — any class inheriting `MenSharp.MenSharpBehaviour`
   becomes one Udon program, no configuration needed:

   ```csharp
   using MenSharp;

   public class Door : MenSharpBehaviour
   {
       public int openCount;                  // a public variable on the UdonBehaviour

       public void Interact()                 // Udon events: Start → _start,
       {                                      // Interact → _interact, other
           openCount += 1;                    // names become custom events
           transform.Rotate(0f, 90f, 0f);     // `this`: the GameObject this
       }                                      // program is attached to
   }
   ```

4. Save — compilation runs automatically (or **MenSharp > Compile All**,
   Ctrl+Shift+M). Diagnostics appear in the Console; one program asset per
   behaviour appears under `Assets/MenSharp/Programs/`.
5. Add the behaviour to a GameObject like any component (drag the script or
   Add Component). An UdonBehaviour with the compiled program is paired
   automatically; inspector-edited field values are carried into Udon when
   entering play mode or building.

Program assets keep their GUID across recompiles, so scenes never break.
The component you added is a *proxy*: at play/build time its values transfer
to the paired UdonBehaviour and the proxy itself is stripped, so code never
runs twice. Its inspector shows which program is actually wired up, and the
UdonBehaviours on a GameObject are kept in sync with the components on it —
swap or delete a behaviour and the program that went with it goes too.

One `.cs` file may declare several behaviours, but dragging the file onto a
GameObject only ever adds the class named after the file. The inspector lists
the others with an **Add** button next to each.

## Notes

- Your sources are plain C#, so Unity compiles them too — that is what gives
  you IDE completion and it is expected; the Udon program is produced by the
  bundled MenSharp compiler, not by Unity.
- `gameObject` and `transform` refer to what the behaviour is attached to,
  as in any Unity component. They are read-only, and only your own — Udon
  resolves them against the UdonBehaviour that owns the program, so there is
  no way to ask another object for its `transform` through them (give the
  behaviour a `public GameObject` field and assign it in the inspector).
- Behaviours inherit from one another. The leaf class is the whole instance:
  it exports its bases' public variables and events too, and a virtual method
  called from a base always lands on the leaf's override. Because the public
  variables share one inspector namespace, a name may not be declared twice
  in a hierarchy.
## Networking

```csharp
using MenSharp;

[UdonBehaviourSyncMode(BehaviourSyncMode.Manual)]
public class Counter : MenSharpBehaviour
{
    [UdonSynced] public int total;
    [UdonSynced(UdonSyncMode.Linear)] public float dial;

    public void Interact()
    {
        if (!Networking.IsOwner(gameObject))
        {
            Networking.SetOwner(Networking.LocalPlayer, gameObject);
        }
        total += 1;
        RequestSerialization();
    }

    public void OnDeserialization() { Debug.Log(total); }
}
```

`[UdonSynced]` becomes a `.sync` directive on the variable, the class
attribute sets the paired UdonBehaviour's sync mode, and `OnPreSerialization` /
`OnDeserialization` / `OnPostSerialization` are Udon events like `Start`.
`RequestSerialization()` and `SendCustomEvent(name)` are externs on the
behaviour itself. `VRC.SDKBase.Networking` is available for ownership.

Method names Udon knows (`Start`, `Interact`, `OnPlayerJoined`, ...) become
Udon events; every other public method becomes a custom event under its own
name, which is what `SendCustomEvent` raises.

## Notes (continued)

- Not supported yet. Each is a compile error rather than a program that runs
  and does the wrong thing:
  - **one behaviour referring to another** — `public Door door;` and
    `door.Open()`, which need Udon custom events;
  - `GetComponent<T>`, `Instantiate`/`Destroy`, and reading the arguments of
    the VRChat events that take them (`OnPlayerJoined(VRCPlayerApi player)` —
    the parameterless form works);
  - `[SerializeField]`, `[FieldChangeCallback]`, `[RecursiveMethod]` (purely
    cosmetic attributes like `[Header]` are ignored without complaint);
  - recursion, exceptions, `ref`/`out` arguments, user-defined structs, and
    `switch` over enum values (enums otherwise work).

## Links

- Source, issues: https://github.com/ProjectTesca/MenSharp

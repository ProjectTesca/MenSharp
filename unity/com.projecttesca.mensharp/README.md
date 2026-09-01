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
   using UnityEngine;

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

## Events

Method names Udon knows (`Start`, `Update`, `Interact`, `OnPlayerJoined`,
`OnDeserialization`, ...) become Udon events — the full set comes from the
SDK's own event list. Every other parameterless public method becomes a custom
event under its own name, which is what `SendCustomEvent` raises.

A behaviour with an `Interact` event shows **Interaction Text** and
**Proximity** in its inspector, exactly as a visible UdonBehaviour would (the
GameObject still needs a Collider to be clickable).

Events that carry arguments work by declaring the documented parameters:

```csharp
public void OnPlayerJoined(VRCPlayerApi player)
{
    Debug.Log($"{player.displayName} joined");
}

public void OnPlayerTriggerEnter(VRCPlayerApi player) { ... }
public void MidiNoteOn(int channel, int number, int velocity) { ... }
```

Declare them exactly as documented or with no parameters at all (to ignore the
arguments); anything in between is a compile error, because Unity fires the
event by name regardless and the mismatched parameters would silently stay
unset.

## Networking

```csharp
using MenSharp;
using UnityEngine;
using VRC.SDKBase;

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

`[UdonSynced]` becomes a `.sync` directive on the variable and the class
attribute sets the paired UdonBehaviour's sync mode; the inspector shows both,
so you can see what is synced without reading the generated assembly.
`RequestSerialization()` and `SendCustomEvent(name)` are externs on the
behaviour itself.

Which fields the inspector shows (and the program exports) follows Unity's own
serialization rule: `public` opts in, `[SerializeField]` opts a private field
in, `[NonSerialized]` opts a public field out.

To *react* to a variable changing — arriving over the network, or written by
another program — route it through a property:

```csharp
[UdonSynced] [FieldChangeCallback(nameof(Level))]
private int _level;

public int Level
{
    get => _level;
    set { _level = value; UpdateDisplay(); }
}
```

External writes to `_level` run the `Level` setter with the written value (the
field still holds the old one, so the setter can compare). Writes from your
own code go to the field directly, as in UdonSharp.

## Talking to another behaviour

```csharp
public class Switch : MenSharpBehaviour
{
    public Door door;           // drag the other GameObject in
    public Door[] doors;

    public void Interact()
    {
        door.openCount = 0;     // SetProgramVariable
        door.Interact();        // SendCustomEvent
        Debug.Log(door.openCount);  // GetProgramVariable
    }
}
```

Two behaviours are two Udon programs with no memory in common, so all of this
goes through Udon's by-name access. That is why the member has to be `public`,
and why a custom event carries no arguments and returns nothing: calling a
method that takes or returns something is a compile error naming the
alternative — write a public variable first, then call a method that takes
nothing.

## Components and cloning

```csharp
Rigidbody body = gameObject.GetComponent<Rigidbody>();
GameObject clone = Instantiate(prefab);
Instantiate(prefab, position, rotation);   // written in terms of the above
Destroy(clone, 3f);

if (Physics.Raycast(ray, out RaycastHit hit))   // `out`/`ref` arguments work
{                                               // on engine methods — Udon
    Debug.Log(hit.point);                       // externs take every parameter
}                                               // by heap address anyway
```

Udon has no generics, so `GetComponent<T>` is one extern that takes
`typeof(T)` as an ordinary value; the same goes for `GetComponentInChildren<T>`
and friends. `typeof(...)` works for any type Udon knows.

VRChat replaces Unity's `Instantiate` with its own, which only clones a
GameObject and takes nothing else — the position/rotation/parent overloads are
written in terms of it. VRChat's own rules still apply: the original has to be
in the scene (not a project prefab), and the clone is local to your client.

## Inheritance

Behaviours inherit from one another. The leaf class is the whole instance: it
exports its bases' public variables and events too, and a virtual method called
from a base always lands on the leaf's override. Because the public variables
share one inspector namespace, a name may not be declared twice in a hierarchy.

Adding both a behaviour and one deriving from it to the same GameObject is
legal but rarely intended — they are two programs, each with its own copy of
the variables, and both run. The inspector says so when it happens.

## Notes

- Your sources are plain C#, so Unity compiles them too — that is what gives
  you IDE completion and it is expected; the Udon program is produced by the
  bundled MenSharp compiler, not by Unity.
- `gameObject` and `transform` refer to what the behaviour is attached to, as
  in any Unity component. They are read-only, and only your own — Udon resolves
  them against the UdonBehaviour that owns the program, so there is no way to
  ask another object for its `transform` through them (give the behaviour a
  `public GameObject` field and assign it in the inspector).
- One `.cs` file may declare several behaviours, but dragging the file onto a
  GameObject only ever adds the class named after the file. The inspector lists
  the others with an **Add** button next to each.
- **MenSharp > Report Scene Wiring** prints every GameObject with a MenSharp
  component or program on it, and which is paired to which.

## Recursion

Just write it — no `[RecursiveMethod]`, no configuration (the UdonSharp
attribute is accepted and ignored):

```csharp
private int Fib(int n)
{
    if (n < 2) { return n; }
    return Fib(n - 1) + Fib(n - 2);
}
```

The compiler finds every call that could re-enter a function — direct,
mutual, or through virtual dispatch — and saves that function's variables
around exactly those calls. Code that never recurses compiles exactly as
before, so there is no cost for not using it.

One loop no compiler can see from inside a single program: your event calls
another program (`SendCustomEvent`, or writing its variables) and that
program synchronously calls back into the *same* method that is still
running. UdonSharp silently corrupts the method's variables in that case;
MenSharp logs an error naming the method and aborts the event instead.

## Enums and switch

Your own enums, engine enums (`KeyCode`, `VideoError`, ...), engine constants
(`int.MaxValue`, `Mathf.PI`) and `switch` all work:

```csharp
public enum DoorState { Closed, Open, Locked = 10 }

switch (state)
{
    case DoorState.Open:
        ...
        break;
    case KeyCode.Space:      // engine enums too, in their own switch
    default:
        break;
}
```

`switch` takes constant case labels and `default`; pattern matching (`case > 0`,
`case string s`, `when` guards) is a compile error for now.

## Not supported yet

Each of these is a compile error rather than a program that runs and does the
wrong thing:

- exceptions and user-defined structs;
- pattern matching in `switch` beyond constant labels;
- `ref`/`out` parameters on methods you define yourself (passing `ref`/`out`
  *to engine methods* like `Physics.Raycast` works).

## Links

- Source, issues: https://github.com/ProjectTesca/MenSharp

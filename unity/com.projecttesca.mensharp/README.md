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
A compile only rewrites the program assets whose program actually changed
(the Console line says how many were updated and how many left alone), so
saving one file costs the compiler run plus one asset, not all of them.
**MenSharp > Rebuild All Programs** rewrites every asset regardless — for
after an SDK update, or a program asset that looks wrong.
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

A custom event can carry arguments over the network (SDK 3.7 and later):

```csharp
using VRC.SDK3.UdonNetworkCalling;
using VRC.Udon.Common.Interfaces;

[NetworkCallable]                       // or [NetworkCallable(5)]: at most
public void Hit(int damage, string by)  // five per second
{
    ...
}

SendCustomNetworkEvent(NetworkEventTarget.All, nameof(Hit), 3, "me");
other.SendCustomNetworkEvent(NetworkEventTarget.Owner, nameof(Other.Ping), 7);
```

The rules are UdonSharp's: public, an instance method, not virtual or an
override, not generic, not a built-in event, up to eight parameters of engine
or .NET types (numbers, strings, vectors, players, arrays of those). The
compiler records, per event, which variables the arguments arrive in and their
types; the program asset hands that to the SDK, which serializes the arguments
by it. Both behaviours need a sync mode other than none.

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
goes through Udon's by-name access — which is why the member has to be
`public`. A call with arguments or a result works too:

```csharp
door.Slide(2);                         // arguments go into the callee's
int n = door.Count();                  // parameter variables, the event runs,
bool ok = door.Take(1, out int rest);  // results and `out`/`ref` come back
door.Level = n;                        // a property with a body: its
Debug.Log(door.Level);                 // accessors are events of their own
```

Under the hood this is UdonSharp's protocol, with UdonSharp's names: the
arguments are written into variables called `__0_amount__param`, the event
`__0_Slide` runs the body, and the result is read from `__0___0_Count__ret`
(a method with parameters is mangled so overloads stay apart; one without
keeps its name). Every public method and every public property with a body
of your behaviours is exported this way, so an UdonSharp script — which only
knows strings — reaches an M# program the same way it reaches its own.

Fields, auto-properties and parameterless methods are addressed by their
plain names, as before. What stays out of reach is an indexer of another
behaviour, and `gameObject`/`transform` through another behaviour (Udon
resolves those against the program that owns the heap).

### Talking to an UdonSharp behaviour

An UdonSharp asset in the project can be used as is: UdonSharp keeps
compiling it, and an M# behaviour talks to it typed.

```csharp
public class Switch : MenSharpBehaviour
{
    public UCounter counter;        // an UdonSharpBehaviour subclass — drag it in

    public void Interact()
    {
        counter.count = 0;          // its public fields, by name
        counter.Bump(3);            // its methods, with arguments and results
        int n = counter.Add(20, 22);
        counter.Level = n;          // its properties, through their accessors
        counter.Interact();         // its built-in events
    }
}
```

Which sources are MenSharp's is decided by a rule Unity already enforces:
the scripts of every assembly definition that references
`ProjectTesca.MenSharp.Runtime` — the reference any script using
`MenSharpBehaviour` needs to compile at all — plus `Assets/MenSharp` itself.
Every other `.cs` in the project (under `Assets`, and under packages other
than VRChat's, `Editor` folders excluded) is read as a *library* — the way
Kotlin reads the Java on its classpath. The compiler follows UdonSharp's own
criteria for what it finds there:

- a class deriving from `UdonSharpBehaviour` is a program UdonSharp compiled.
  M# never compiles it; it talks to it by name, as above. Its public surface is
  what you can use — fields, properties, methods, and the members
  `UdonSharpBehaviour` itself provides (`SendCustomEvent`,
  `RequestSerialization`, `SendCustomNetworkEvent`).
- everything else — a static helper class, an enum, a struct — is ordinary
  code, compiled into *your* program when you use it, exactly as UdonSharp
  inlines a helper into each behaviour that calls it. `UCounterMath.Triple(4)`
  from a U# asset just works, static or not.
- a class deriving from an engine class (a plain `MonoBehaviour`) is neither:
  Udon can neither create nor hold one, so using it as a type is an error.

A library's errors are its own: nothing is reported for files you did not
write. What M# code *uses* is checked at the use — a helper that leans on
something M# does not support is an error there, naming the helper and the
reason. A library type under a name your own code declares is dropped; yours
wins. `#if` is evaluated: your files see `COMPILER_MENSHARP`, library files
see `COMPILER_UDONSHARP` as well (so their `#if !COMPILER_UDONSHARP &&
UNITY_EDITOR` editor blocks vanish, as they do for UdonSharp), and
`--define NAME` adds your own.

UdonSharp's own rules still apply on its side: a script needs its UdonSharp
program asset (one created through *Create > U# Script* has it), and a script
inside an assembly definition is only compiled by UdonSharp when a *U#
Assembly Definition* asset points at that asmdef (*Create > U# Assembly
Definition* with the asmdef selected). Until UdonSharp has compiled the
program, calls into it do nothing; MenSharp warns about such a reference when
it transfers values.

For Unity to compile `public UCounter counter` in your M# source, your sources
must be able to *see* the UdonSharp class. They can: `Assets/MenSharp` has no
assembly definition, so your sources are part of Assembly-CSharp, which sees
every assembly in the project — an asset dropped into `Assets` (Assembly-CSharp
too) and one with an assembly definition alike. UdonSharp is kept from reading
your sources (its own compiler pass is C# 7.3) by an entry MenSharp adds to
UdonSharp's scanning blacklist. If you prefer your sources in an assembly of
their own, **MenSharp > Create Assembly Definition for Assets/MenSharp** writes
one; then an UdonSharp asset without an asmdef can no longer be named from
them, only reached by string. A `MenSharpBehaviour` placed anywhere else —
outside `Assets/MenSharp`, in a folder without an assembly definition — is an
error naming the two places it belongs, since nothing there would check it
and UdonSharp's own compiler would read it.

## Distributing a package

**MenSharp > Create Package…** writes the skeleton of a VPM package under
`Packages/`: a `package.json` depending on MenSharp, and a `Runtime/` folder
with an assembly definition already referencing the MenSharp runtime. Write
behaviours in `Runtime/`; they compile on save like the ones in
`Assets/MenSharp`, and their programs land in `Runtime/Programs/` inside the
package, beside the prefabs you build with them. Ship the folder. On the
consumer's side the assembly reference is what marks the sources as
MenSharp's — there is nothing to configure — and their MenSharp recompiles the
programs in place, GUIDs intact, so the prefabs keep working.

## Components and cloning

```csharp
Rigidbody body = gameObject.GetComponent<Rigidbody>();
if (TryGetComponent<BoxCollider>(out var box)) { ... }
GameObject clone = Instantiate(prefab);
Instantiate(prefab, position, rotation);   // written in terms of the above
Destroy(clone, 3f);

if (Physics.Raycast(ray, out RaycastHit hit))   // `out`/`ref` arguments work
{                                               // on engine methods — Udon
    Debug.Log(hit.point);                       // externs take every parameter
}                                               // by heap address anyway
```

Engine structs behave as in C#: `new Vector3 { x = 1f }`, `new Vector3()`,
`default`, and writing a field (`v.x = 5f`) all work — the SDK spells a
struct field setter differently from a property setter, and the compiler
picks the right one.

Udon has no generics, so `GetComponent<T>` is one extern that takes
`typeof(T)` as an ordinary value; the same goes for `GetComponentInChildren<T>`
and friends. `typeof(...)` works for any type Udon knows.

`GetComponent<Door>()` with one of *your* behaviours — or an UdonSharp one —
works too, and so do `GetComponents`, `GetComponentInChildren`,
`GetComponentInParent` and `TryGetComponent`, on the behaviour itself, on a
`GameObject` or on any component. Udon cannot name a program as a type, so
the search asks every UdonBehaviour on the object for its identity (the
program id MenSharp puts in heap slot 0, or UdonSharp's `__refl_typeid`) and
returns the ones that are a `Door` — subclasses included, as with any
`GetComponent`. The cost is one `GetComponents(typeof(UdonBehaviour))` and one
variable read per behaviour on the object.

VRChat replaces Unity's `Instantiate` with its own, which only clones a
GameObject and takes nothing else — the position/rotation/parent overloads are
written in terms of it. VRChat's own rules still apply: the original has to be
in the scene (not a project prefab), and the clone is local to your client.

## Inheritance, abstract classes and interfaces

Classes of your own inherit, override (`virtual`/`abstract`/`override`, methods
and properties and indexers alike), and implement interfaces — explicit
implementations (`int IShape.Area()`) and interfaces extending interfaces
included. A call through a base or interface type lands on the runtime type's
implementation:

```csharp
public interface IShape { int Area(); string Name { get; } }
public abstract class Shape : IShape
{
    public abstract int Area();
    public abstract string Name { get; }
    public virtual int Twice() => Area() * 2;
}
public struct Unit : IShape { ... }

IShape s = new Circle(2);   // dispatches on the object's type id
Total<T>(T shape) where T : IShape => shape.Area();   // a struct T binds statically
```

Every type is known at compile time, so a dispatch is a comparison of type ids
against the implementations that exist in the program — there is no runtime
lookup. A `sealed` class or a struct behind a constrained type parameter skips
even that and calls the implementation directly. A concrete type that leaves an
abstract or interface member unimplemented, or `new` of an abstract class, is
the C# error it is (CS0534/CS0535/CS0144). When a class has both a public
member and an explicit implementation of the same name, a call through the
interface reaches the explicit one, as in C#.

An interface member with a body is a default implementation (C# 8): a type
that does not implement it gets the interface's version, reached through the
interface type only (as in C#, `plain.Greet()` on the class is an error when
the class never declared it). A derived interface may re-implement it
(`string IGreeter.Greet() => ...`), and the most derived one wins; two
unrelated interfaces doing so is the C# error (CS8705). On a struct the
default body runs on a boxed copy, exactly as .NET does.

Constructors chain as in C#: field initializers, then `: base(...)` /
`: this(...)` (or the implicit `base()`), then the body — so a base class's
fields are initialized whichever subclass is constructed, and a virtual call
from a base constructor lands on the subclass's override. A class whose base
has no parameterless constructor must write `: base(...)` (CS7036). Static
constructors run once at startup, after the static field initializers, before
the first event.

`Equals`, `GetHashCode` and `ToString` go to your override whatever the static
type of the receiver — `object`, a base class, an interface — and so does
string concatenation (`"got " + shape`). A struct without them gets field-wise
equality and its type name, as in .NET.

Operators you declare (`public static V operator +(V a, V b)`, unary `-` and
`!`, `==`/`!=`, `<`/`>`, `++`/`--`) are chosen by the usual overload rules and
used by the plain operator syntax, by compound assignment (`v += w`) and by
`x++`. `==` on a class with its own operator calls that operator, `null`
included, as C# does; a comparison operator without its partner is the C#
error (CS0216). Conversion operators (`implicit operator`) are not supported
yet.

Casts, `is` and `as` test the runtime type, and `is` takes the C# patterns:

```csharp
if (shape is Circle c) { ... }                    // type pattern, binds on success
if (shape is Circle { Radius: > 3 } big) { ... }  // property pattern with a relational one
if (n is >= 0 and < 10 || o is not null) { ... }  // relational, and/or/not, constants
if (mood is Mood.Happy or Mood.Locked) { ... }
var square = shape as Square;                     // null when it is not one
var circle = (Circle)shape;                       // InvalidCastException when it is not one
```

Constant, relational, `and`/`or`/`not`, `var`, `_` and property patterns
(nested, with or without a type) all work, on values and on `object`
(`o is > 3` is false for a string, as in C#). The same patterns work as
`case` labels, with `when` guards, and in switch expressions:

```csharp
switch (shape)
{
    case Circle { Radius: > 5 } big: ...; break;
    case Circle c when c.Radius == 2: ...; break;
    case null: ...; break;
    default: ...; break;
}
string size = n switch { < 3 => "small", < 10 => "medium", _ => "large" };
```

A switch expression no arm matches throws `SwitchExpressionException`, as in
C#. Positional patterns (`is (0, var y)`, `Point(var x, var y)` through a
`Deconstruct`) and list patterns (`[1, .., var last]`) work too.

## Unions: closed hierarchies and exhaustive switch

C# has no tagged union, but an abstract class (or an interface) with a fixed
set of subclasses is the usual stand-in. Mark it `[Union]` and MenSharp
treats the set as closed: every `switch` over it — statement or expression —
must handle every concrete type below it, or have a `default`/`_` arm, and a
missing one is a compile error naming the type:

```csharp
using MenSharp;

[Union] public abstract class Shape { }
public sealed class Circle : Shape { public float R; }
public sealed class Rect : Shape
{
    public float W, H;
    public void Deconstruct(out float w, out float h) { w = W; h = H; }
}

float Area(Shape s) => s switch
{
    Circle c => c.R * c.R * 3.14f,
    Rect(var w, var h) => w * h,
};
// add `public sealed class Triangle : Shape` and every switch over Shape
// that does not handle it stops compiling: missing: ["Triangle"]
```

`[Union]` goes on an abstract class or an interface, generic ones included
(`[Union] abstract class Option<T>` with `Some<T>` and `None<T>`; a switch
over `Option<int>` has to handle `Some<int>` and `None<int>`). A concrete
class that is not `sealed` is a case of its own and its subclasses are more
cases — a `Foo f` arm takes them all. What counts as handling a case: a type
pattern the case converts to (`Shape s`, `object o`), `var`, `_`, `not`/
`and`/`or` of those, and a property or positional pattern whose parts all
always match (`Circle { R: var r }`, `Rect(var w, var h)`). An arm with a
`when` guard, a `Circle { R: > 0 }`, or a relational pattern does not count:
it may not match.

Switch *expressions* over an enum and over `bool` are held to the same
standard, as the C# compiler's warning CS8509 would have it, but as an
error: `door switch { DoorState.Open => 1, DoorState.Closed => 0 }` with a
third member is rejected, naming it. Switch statements over an enum are not
checked (leaving members out is normal there).

One thing to know: Unity's own compiler cannot see that a `[Union]` is
closed, so it still warns (CS8509) on a switch expression over one that has
no `_` arm. A `_ => throw new InvalidOperationException()` arm silences it
at the cost of MenSharp's check; a `#pragma warning disable CS8509` in that
file keeps the check. Switch statements draw no warning either way.

## Exceptions

`throw`, `try`/`catch`/`finally`, `catch (T e) when (...)`, `throw;` and
`x ?? throw new ...` work as in C#, across calls and recursion. `System.Exception`
and the usual family (`InvalidOperationException`, `ArgumentException`,
`ArgumentNullException`, `ArgumentOutOfRangeException`, `IndexOutOfRangeException`,
`NullReferenceException`, `InvalidCastException`, `DivideByZeroException`,
`KeyNotFoundException`, `NotSupportedException`, `NotImplementedException`,
`FormatException`, ...) come from the mini-corlib, since Udon exposes none of
the real ones; derive your own from them as usual.

```csharp
class DoorLockedException : InvalidOperationException
{
    public int Code;
    public DoorLockedException(int code) : base("door " + code + " is locked") { Code = code; }
}

try { Open(3); }
catch (DoorLockedException e) when (e.Code == 3) { Debug.Log(e.Message); }
finally { busy = false; }
```

What the compiler can see coming throws the C# exception instead of crashing
the VM: an index outside an array (`IndexOutOfRangeException`), a member or
call on a null object of your own classes (`NullReferenceException`), integer
`/` and `%` by zero (`DivideByZeroException`), a failed cast, a switch
expression with no matching arm, `List` and `Dictionary` misuse
(`ArgumentOutOfRangeException`, `KeyNotFoundException`, `ArgumentException`).
Every exception knows where it was thrown and what it unwound through:
`e.StackTrace` lists the frames from the throw to the `catch`, with file, line
and column, and `e.ToString()` prints type, message and trace the way .NET
does. An exception nothing catches is reported to the console the same way,
prefixed `Unhandled exception:`, and halts the behaviour:

```text
Unhandled exception: Game.DoorLockedException: door 3 is locked
   at Game.Door.Open in Assets/MenSharp/Door.cs:42:13
   at Game.Verify.Interact in Assets/MenSharp/Verify.cs:310:9
```

The trace costs nothing until an exception actually unwinds: each frame line
is a string constant, appended only on the exception path. `throw;` keeps the
trace and goes on adding to it; `throw e;` starts it over, as in C#.

What an engine or .NET call throws *inside itself* — `GetComponent` on a
destroyed object, `int.Parse("x")`, a Unity API given null — cannot be
caught: the Udon VM stops the behaviour before any of your code runs again.
That is Udon's rule, not a C# one, so check such inputs before the call. You
do get told where it happened: in the editor (play mode included) a second
console entry follows the VM's report, naming the call and its position —
`MenSharp: the Udon VM halted inside `SystemInt32.Parse`, called from
Game.Door.Open at Assets/MenSharp/Door.cs:57:21` — with the program asset as
its context. This works because every program carries an address → source
table in its sidecar and its own id in heap slot 0, which the VM's report
prints. The table also knows where each function starts and where M#'s own
stop after `Unhandled exception:` is, so a halt in compiler-generated code
is named as such, and the VM's report about an unhandled M# exception gets
no second explanation — the trace above it is the whole story.

The cost is small: a `try` costs nothing to enter, a `throw` is a jump, and
each call is followed by one flag test. The runtime checks above add a
comparison or two where they apply.

A failed cast throws `InvalidCastException` (see *Exceptions* below), at the
cast, instead of reading the wrong object's fields later. `is`/`as` and checked casts work with your own classes, structs
and interfaces (by type id) and with engine and .NET types (`c is Collider`,
`hit.collider as BoxCollider`, `o is int` — through `Type.IsInstanceOfType`,
so subclasses and interfaces count as in C#).

Behaviours inherit from one another. The leaf class is the whole instance: it
exports its bases' public variables and events too, and a virtual method called
from a base always lands on the leaf's override. Because the public variables
share one inspector namespace, a name may not be declared twice in a hierarchy.
A behaviour cannot be reached *through an interface*, though: another
behaviour is another Udon program, only callable by event name, so call it
through a variable of its own type.

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

## Strings

String interpolation carries format specifiers and alignment, as in C#:

```csharp
Debug.Log($"{elapsed:0.0}s | {count:D5} | {total:N0} | [{name,-12}] [{score,4}]");
```

The format goes to the value's own `ToString(string)`, which Udon exposes for
the numeric types and most engine structs (`Vector3`, `Quaternion`, `Color`,
`Rect`, ...), so the standard forms (`F2`, `N0`, `D5`, `X`, `P1`) and custom
patterns (`0.0`, `#.##`, `000`) all mean what .NET says they mean. A type
without that overload — a class of your own, `string`, `bool` — is a compile
error naming the type, rather than a string that quietly ignores the format.
The alignment pads with `PadLeft`/`PadRight`: positive is right-aligned,
negative left-aligned, and it has to be a number written out (a named
constant is not accepted there yet).

One caveat that is .NET's, not MenSharp's: the culture-dependent forms
(`N`, `C`, `P`) render differently per culture — invariant culture writes
`50.0 %` where `en-US` writes `50.0%`. Use them for display, not for text
another program parses.

## async/await

`async`/`await` work as in C#, with `Task` and `Task<T>` — and they are how
you write what would be a coroutine elsewhere:

```csharp
public async void Interact()
{
    door.SetActive(true);
    await Scheduler.Delay(2f);            // seconds of scene time
    await Scheduler.NextFrame();          // one frame
    int hits = await CountHits(5);        // another async method
    string word = await pressed.Task;     // a TaskCompletionSource<string>
    door.SetActive(false);
}

private async Task<int> CountHits(int frames)
{
    int hits = 0;
    for (int i = 0; i < frames; i++)
    {
        await Scheduler.NextFrame();
        if (Physics.Raycast(transform.position, transform.forward)) { hits++; }
    }
    return hits;
}
```

What is there: `async` methods, lambdas and local functions returning `void`,
`Task` or `Task<T>`; `await` on any of them, on `Task.Delay(ms)`,
`Task.CompletedTask`, `Task.FromResult(x)`, `Task.WhenAll(...)`,
`Task.WhenAny(...)`, a `TaskCompletionSource<T>` (complete it from any event —
a button, `OnDeserialization`, a network event), and on an awaitable of your
own (a class with `GetAwaiter()` giving `IsCompleted`, `OnCompleted(Action)`
and `GetResult()`, as C# defines it). Exceptions travel through tasks: one
thrown in an async method faults its task and is rethrown where the task is
awaited, `try`/`catch`/`finally` work across an `await`, and an exception out
of an `async void` method is an unhandled one (logged, and the event ends), as
in C#. Two activations of the same method can be suspended at once; each
keeps its own variables.

`MenSharp.Scheduler` holds the awaitables Udon has and .NET does not:
`Delay(seconds)`, `DelayFrames(n)`, `NextFrame()`, `Yield()` (runs the other
continuations queued so far, then continues — how a long computation shares
an event), `WaitUntil(() => ready)`, `WaitWhile(...)`, and `Run(async () =>
{ ... })` for a task nothing awaits (a fault in it is logged rather than
ending the event). A long job split with `await Scheduler.NextFrame()` inside
its loop is a cooperative thread.

When a continuation runs: as soon as the event that completed the task has
finished its own work — never in the middle of it — and in the same frame.
To move it, ask for a task that completes later: `await
task.OnNextFrame()`, `await task.After(0.5f)`, `await task.AfterFrames(3)`,
each on `Task` and `Task<T>`. They are ordinary tasks, so `var later =
Load().OnNextFrame();` makes a task whose awaiters all resume on the next
frame.

How it works on the VM: a suspended method's variables are copied into an
`object[]` together with the address to resume at, and the pair is the
`Action` the task holds; resuming copies them back and jumps. Timed
continuations use the program's own `SendCustomEventDelayedSeconds`/`Frames`
(the event is `_mensharpResume`; the name is reserved). Nothing is hoisted or
rewritten, so anything that works in a method works in an async one.

Stopping one — the `StopCoroutine` of async code — is a
`CancellationTokenSource`, as in .NET: give its token to what waits, and
`Cancel()` makes that awaitable fault with `TaskCanceledException` at once,
so the method leaves through `catch (OperationCanceledException)`:

```csharp
private CancellationTokenSource blinking;

public void StartBlinking()
{
    blinking = new CancellationTokenSource();
    Blink(blinking.Token);
}

public void StopBlinking() { blinking.Cancel(); }

private async void Blink(CancellationToken token)
{
    try
    {
        while (true)
        {
            lamp.SetActive(!lamp.activeSelf);
            await Scheduler.Delay(0.5f, token);
        }
    }
    catch (OperationCanceledException) { lamp.SetActive(false); }
}
```

`Scheduler.Delay`, `DelayFrames`, `NextFrame`, `WaitUntil`, `WaitWhile` and
`Task.Delay` take a token; `token.IsCancellationRequested`,
`token.ThrowIfCancellationRequested()`, `token.Register(action)`,
`source.CancelAfter(milliseconds)`, `CancellationToken.None` and `task.IsCanceled`
work as in .NET.

A task crosses a behaviour boundary, so one behaviour can await another's
work directly:

```csharp
public class Door : MenSharpBehaviour
{
    public async Task<int> Open(int by)
    {
        await Scheduler.Delay(1f);
        return by * 2;
    }
}

public class Switch : MenSharpBehaviour
{
    public Door door;
    public async void Interact()
    {
        int opened = await door.Open(21);   // resumes here when the door is done
    }
}
```

The task travels as data, and every part of it that would need the *owning*
program's code — running a continuation, rethrowing its exception as itself —
checks who owns it first. So a continuation you register on another
behaviour's task is handed back to you rather than run over there, and it
arrives at the end of the frame rather than in the middle of that behaviour's
event: nothing is ever re-entered. Tasks kept in variables, `Task.WhenAll`
over a mix of local and remote ones, and an already-finished remote task all
behave like any other.

Two things do not cross. An exception cannot: its type is a number that means
something only inside the program that raised it, so awaiting a faulted remote
task throws `MenSharp.RemoteTaskException` carrying the text the original
printed as. And a Task-typed *field* is not an exported variable, so read one
through a method instead. Task-typed fields are not inspector variables
either.

## Iterators

`yield return` works as in C#, in a method or local function returning
`IEnumerable<T>` or `IEnumerator<T>`:

```csharp
private IEnumerable<Transform> Descendants(Transform root)
{
    foreach (Transform child in root)      // hmm — Transform is not enumerable on Udon; see below
    {
        yield return child;
        foreach (Transform grandchild in Descendants(child)) { yield return grandchild; }
    }
}
```

The body runs lazily, one step per `MoveNext` — `foreach` over the result
runs it up to each `yield return` in turn — and a second `foreach` over the
same enumerable starts it over. `yield break` ends it early, an exception
out of the body ends it and reaches the loop, and generic iterators
(`IEnumerable<T> Repeat<T>(T item, int n)`) and recursive ones (the tree walk
above) work. It suspends the same way an `async` method does, so the same
things hold: nothing is hoisted or rewritten, and any code that works in a
method works in an iterator.

An iterator can clean up after itself with `try`/`finally`, and the
`finally` runs however the loop is left:

```csharp
private IEnumerable<int> Reading()
{
    handle = Open();
    try
    {
        while (handle.HasMore) { yield return handle.Next(); }
    }
    finally { handle.Close(); }   // runs on break, return, or an exception too
}
```

That works because `foreach` disposes its enumerator on every way out, as C#
does, and disposing a suspended iterator resumes it one last time just to run
the `finally` blocks it is sitting inside. Enumerators with nothing to clean
up cost nothing: a `foreach` over an array, a string, a `List<T>` or a
`Dictionary` compiles to exactly what it did before.

Where C# says no, so does this: `yield return` inside a `try` that has a
`catch` (CS1626), inside a `catch` (CS1631), and `yield` of either kind
inside a `finally` (CS1625). `yield break` inside a `try`/`catch` is fine.

## Sequences: IEnumerable&lt;T&gt;

`IEnumerable<T>` is a real interface here, so one loop takes anything:

```csharp
private int Total(IEnumerable<int> numbers)
{
    int sum = 0;
    foreach (int n in numbers) { sum += n; }
    return sum;
}

Total(list);                       // List<int>
Total(new int[] { 1, 2, 3 });      // an array
Total(Squares(4));                 // an iterator
Total(ages.Values);                // a Dictionary's values
```

`List<T>`, `Dictionary<K, V>` (and its `Keys` and `Values`), every iterator,
arrays and `string` (as `IEnumerable<char>`) all satisfy it. Arrays and
strings are wrapped when they cross into a sequence, exactly where the
conversion happens; `foreach` over an array or a string directly still walks
it by index with nothing allocated, and `foreach` over a `List<T>` still
binds to its concrete enumerator, so the common loops cost no dispatch.

It is the compiler's own `IEnumerable<T>`, with `GetEnumerator` on it and
nothing else — and the LINQ operators below as extension methods on it.
(`foreach (Transform child in transform)` needs the engine's non-generic
enumerator, which Udon does not expose — walk
`transform.childCount`/`GetChild(i)` instead.)

## LINQ

`using System.Linq;` and the query operators work on every sequence — arrays,
`List<T>`, `Dictionary<K, V>`, strings, iterators and each other's results:

```csharp
using System.Linq;

int[] scores = new int[] { 40, 85, 62, 91 };
int passed = scores.Count(s => s >= 60);
int best = scores.Max();
string top = scores.Where(s => s >= 60).OrderByDescending(s => s)
    .Select(s => s.ToString()).Aggregate((a, b) => a + "," + b);

List<Player> players = ...;
Player leader = players.OrderByDescending(p => p.Score).ThenBy(p => p.Name).First();
Dictionary<string, Player> byName = players.ToDictionary(p => p.Name);
foreach (IGrouping<int, Player> team in players.GroupBy(p => p.Team))
{
    Debug.Log($"team {team.Key}: {team.Count()} players, {team.Sum(p => p.Score)} points");
}
```

What is there, with the signatures of .NET's `Enumerable`:

- **Filtering and projection:** `Where`, `Select` (both also with an index),
  `SelectMany` (both forms), `Distinct`, `Union`, `Intersect`, `Except`.
- **Partitioning and joining:** `Take`, `Skip`, `TakeWhile`, `SkipWhile`,
  `Concat`, `Append`, `Prepend`, `Zip` (with a result selector),
  `DefaultIfEmpty`, `Reverse`.
- **Ordering:** `OrderBy`, `OrderByDescending`, `ThenBy`, `ThenByDescending`
  — a stable sort, like .NET's, by however many keys you chain.
- **Grouping:** `GroupBy` (key, key + element selector, key + result
  selector); each group is an `IGrouping<K, T>` with a `Key`.
- **Elements:** `First`, `Last`, `Single`, `ElementAt` and their `OrDefault`
  forms, `Any`, `All`, `Contains`, `SequenceEqual`, `Count`, `LongCount`.
- **Aggregation:** `Sum`, `Average`, `Min`, `Max` for `int`, `long`, `float`
  and `double` (directly and through a selector), `Min`/`Max` of anything
  orderable, and `Aggregate` (all three forms).
- **Conversion and generation:** `ToArray`, `ToList`, `ToDictionary` (with and
  without an element selector), `AsEnumerable`, `Enumerable.Range`, `Repeat`,
  `Empty`.

The deferred operators are as lazy as .NET's: nothing runs until the result
is enumerated, each element flows through the whole chain before the next is
read, and leaving a `foreach` early closes the chain (an iterator's
`finally` blocks run). Enumerating the same query again runs it again.

**Ordering and equality.** `OrderBy`, `Min`, `Max` and `List<T>.Sort()` order
values by the type's own `CompareTo`: numbers, `string`, `char`, `bool`,
enums, and any class or struct of yours that implements `IComparable<T>`.
Ordering something that has none is a compile error naming the type — order
by a key instead (`OrderBy(x => x.Name)`), or implement the interface.
`Distinct`, `Contains`, `GroupBy` and `ToDictionary` compare with the value's
`Equals`/`GetHashCode`, as `Dictionary<K, V>` does. `null` orders before
everything and is skipped by `Min`/`Max`, as in .NET.

`List<T>` gained the members these lean on: `Sort()`, `Contains`, `IndexOf`,
`ToArray`, `AddRange`, and the `List(IEnumerable<T>)` and `List(int)`
constructors.

Not there: the overloads that take an `IComparer<T>`/`IEqualityComparer<T>`,
the `Nullable` numeric ones, `Cast`/`OfType` (Udon cannot test a box against
one of your classes), `ToHashSet`/`ToLookup`, and `Join`/`GroupJoin`. Each is
an ordinary "no such method" error. A null source is not checked for
`ArgumentNullException`; it fails at its first `GetEnumerator` like any null
receiver.

A word on cost: every deferred operator is an iterator, so each element
costs a resumed continuation per stage of the chain (a few hundred Udon
instructions), and `OrderBy` computes every key once and sorts positions.
For a hot per-frame loop over a large array, a plain `for` is still cheaper;
for the everyday "find, filter, sort, sum" of a world script, LINQ is fine.

## Delegates, lambdas and events

Delegates are ordinary values: `Func<>`/`Action<>`/`Predicate<>` and the
other delegate types of .NET, or `delegate` types of your own. Lambdas,
method groups, closures over locals and `this`, multicast `+=`/`-=`,
field-like events and `?.Invoke()` all work as in C#:

```csharp
public delegate int Op(int a);

public event Action Opened;                 // field-like events
private Func<int, int> scale = x => x * 2;  // a lambda in a field

public void Interact()
{
    Op triple = a => a * 3;
    Func<int, int> twice = Twice;           // a method group, yours or an object's
    int captured = 0;
    Action bump = () => { captured++; };    // a closure: the variable is shared
    bump(); bump();                         // captured == 2

    Opened += () => Debug.Log("opened");
    Opened += OnOpened;
    Opened -= OnOpened;
    Opened?.Invoke();                       // nothing happens while nobody listens

    var list = new List<int> { 3, 1, 2 };
    list.Sort((a, b) => a - b);
    list.RemoveAll(v => v > 2);
    list.ForEach(v => Debug.Log(v));
}
```

What a delegate is on the VM: an `object[]` holding the code address of a
small entry stub and what that stub hands the target — the receiver of a
method, or the `this` and the captured variables of a lambda. Calling one
jumps to that address through the same indirect jump every `return` uses,
so there is no lookup table and no string dispatch. A captured variable
lives in a one-element `object[]` shared by the lambda and the method that
declared it, so writes on either side are seen by the other, and a lambda
made inside a loop body keeps that iteration's variable, as C# promises.
Recursion through a delegate (`fact = n => n * fact(n - 1)`) is handled by
the same frame saving as any other recursion.

Two delegates are equal when they call the same method on the same object,
or are the same closure (the same lambda made in the same activation), and
`-=` removes by that equality — which is why `Opened -= OnOpened` works and
`Opened -= () => ...` removes nothing, exactly as in C#.

What does not cross a program boundary: a delegate is code addresses of
the program that made it, so a delegate-typed field, parameter or return of
*another* behaviour is an error, and delegate-typed fields are not inspector
variables. What is not there yet, each as an error: anonymous methods
(`delegate (int x) { ... }` — write a lambda), a method
of the *engine* as a delegate (`Func<string, int> f = int.Parse;` — wrap it,
`s => int.Parse(s)`), conversions between delegate types of different
shapes (`Func<object> f = funcOfString;`), and events with `add`/`remove`
accessors.

## Local functions

A method may declare functions of its own, anywhere a statement goes. They
work as in C#: called from above their own declaration, recursive and
mutually recursive, sharing the variables of the method around them, with
`ref`/`out`, optional and named arguments — and usable as a delegate.

```csharp
public void Interact()
{
    int total = 0;
    void Add(int by) { total += by; }       // shares `total`, does not copy it
    Add(2);
    Add(3);                                 // total == 5

    int Fact(int n) => n <= 1 ? 1 : n * Fact(n - 1);
    Debug.Log(Fact(5));                     // 120

    bool Even(int n) => n == 0 || Odd(n - 1);
    bool Odd(int n) => n != 0 && Even(n - 1);

    static int Pure(int x) => x + 1;        // `static`: uses nothing around it
    Func<int, int> f = Fact;                // as a delegate, closure and all
}
```

A local function is a function of its own on the VM, not a delegate: the
call is direct, with the boxes of the variables it shares handed over in
front of its arguments. Nothing is allocated to call one — a delegate
object appears only where you make one (`Func<int, int> f = Fact;`).
A caller passes on what its callee shares too, so a lambda that calls a
local function which uses `total` reaches the same `total`.

Local functions are visible throughout their block, above their own
declaration included; variables are not. So a local function uses the
variables written *above* it (C# says CS0841 otherwise), and calling one
before such a variable's declaration has run is an error too — C# rejects
that as a use before assignment.

## Tuples

`(int, string)`, named elements, deconstruction and positional patterns work
as in C#:

```csharp
(int x, int y) Point(int x, int y) => (x, y);

var p = Point(2, 3);
int sum = p.x + p.y;                  // or p.Item1 + p.Item2
var (a, b) = Point(4, 5);             // deconstruction, in every spelling
(int e, string f) = Pair();            // declaring types inline
foreach (var (id, name) in pairs) { ... }

if (p is (0, var height)) { ... }     // positional patterns
string kind = p switch
{
    (0, 0) => "origin",
    (var x, _) when x > 0 => "right",
    _ => "left",
};

var seen = new Dictionary<(int, int), string>();
seen[(1, 2)] = "cell";                // hashed and compared element-wise
Debug.Log($"{p}");                    // (2, 3), as C# prints it
```

A class or struct of your own comes apart the same way once it offers a
`Deconstruct`, and so do the pairs a `Dictionary` yields:

```csharp
public void Deconstruct(out int x, out int y) { x = X; y = Y; }

var (x, y) = point;
if (point is (0, var height)) { ... }
switch (shape) { case Point(var a, var b): ...; }
foreach (var (name, age) in ages) { ... }
```

List patterns match an array by shape:

```csharp
string kind = values switch
{
    [] => "empty",
    [var only] => "one",
    [1, 2] => "one two",
    [var first, .., var last] => "ends",
    _ => "other",
};
if (numbers is [1, ..]) { ... }
if (numbers is [var head, .. var rest]) { ... }   // `rest` is a copy
```

**Unity's own C# version comes first.** Every script under `Assets` is
compiled twice: by M# into an Udon program, and by Unity into the proxy
component the inspector shows. Unity 2022.3 fixes that second compile at
**C# 9**, so a file using anything newer fails there — and a file Unity
cannot compile has no class, which is why the editor then says *"Can't add
script component … the script class cannot be found"*. Of what M# supports,
only **list and slice patterns** (C# 11) are past that line. Adding an
`Assets/csc.rsp` holding `-langversion:latest` is the usual way to lift it,
at the cost of a compile Unity does not officially support.

A tuple is an `object[]` on the VM, with a value type's copy-on-assignment:
`var copy = p; copy.x = 9;` leaves `p` alone. It carries the id of its shape
the way a struct does, so it stays itself inside an `object`: printing,
`Equals` and `GetHashCode` still work element by element, and casting one
back (`((int, int))boxed`) checks the shape and throws
`InvalidCastException` when it does not fit. Unity does not serialize
tuples, so a tuple field is never an inspector variable.

## Nullable value types

`int?`, `float?`, `Vector3?`, `MyStruct?` — `Nullable<T>` — work as in C#:
`HasValue`, `Value` (which throws `InvalidOperationException` on a null),
`GetValueOrDefault()`, `??`, `?.` producing one, lifted operators, patterns
and `switch`, boxing and unboxing:

```csharp
int? last = null;
if (last == null) { ... }                 // or !last.HasValue, or `last is null`
last = 5;
int? next = last + 1;                     // 6; null if `last` were null
if (last > 3) { ... }                     // false when null
int value = last ?? 0;                    // the value, or the fallback
last++;
Vector3? target = null;
target = hit.point;
if (target is Vector3 point) { ... }
int? count = door?.OpenCount;             // null when `door` is
int? pick = found ? index : null;         // a conditional takes the target type (C# 9)
switch (last) { case null: ...; case 6: ...; }
```

On the VM a `T?` is what .NET boxes it to: the value itself, or null, in an
`object` slot — so a `T?` costs no allocation, `x.HasValue` is one null
test, and `x.Value` one copy. Nullable-typed fields are not inspector
variables (Unity does not serialize them either). `string?` and other
annotations on reference types are just the type; no flow analysis is done
for them yet.

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

`switch` takes constant case labels, `default`, and every kind of pattern
(`case > 0`, `case string s`, `when` guards — the `is` patterns under
*Inheritance, abstract classes and interfaces*). A
switch *expression* over an enum must handle every named member or have a
`_` arm; a forgotten member is a compile error (see *Unions*).

An enum value prints as its name, as in C#: `"state: " + state`,
`$"{state}"`, `state.ToString()` and a `T` that is an enum inside a generic
method all give `Open`. A value no member has prints as its number, and a
`[Flags]` enum is decomposed the way .NET does it (`Front, Side`). The one
difference from C#: an enum stored in an `object` has lost its type (on Udon
it is a boxed `int`), so `object o = state; o.ToString()` prints the number.

## foreach, List<T> and Dictionary<K, V>

`foreach` walks arrays, strings (by `char`), `List<T>`, `Dictionary<K, V>`
(and its `Keys`/`Values`), and anything with a `GetEnumerator()` whose result
has `MoveNext()`/`Current` — the same pattern C# itself binds to, so a class
of your own is enumerable without implementing any interface:

```csharp
var names = new List<string> { "a", "b" };   // collection initializer = Add calls
names.Add("c");
foreach (var name in names) { ... }
foreach (char c in "text") { ... }

var ages = new Dictionary<string, int> { { "ann", 30 }, { "bob", 41 } };
var seed = new Dictionary<string, int> { ["cy"] = 52 };   // index initializer
ages["dee"] = 63;
if (ages.TryGetValue("bob", out var age)) { ... }
foreach (var pair in ages) { Debug.Log($"{pair.Key}: {pair.Value}"); }
foreach (var key in ages.Keys) { ... }
```

`int[] steps = { 1, 2, 3 };` — the brace shorthand — works wherever C# takes
it: a local, a field, `= { }` for an empty array, with each element converted
to the element type (`long[] ids = { 1, 2 };`). It is exactly
`= new int[] { 1, 2, 3 }`, and, like any initializer the compiler cannot bake
into the heap, it runs when the program starts — so on a *behaviour* field it
overwrites what the inspector holds. Leave such a field bare (`public int[]
steps;`) when the inspector is meant to fill it.

A jagged array (`int[][]`) is an array of arrays, as in C#: `new int[3][]`
makes the rows null until you fill them, `grid[1][0] = 30` writes through
both levels, and `foreach (int[] row in grid)` walks the rows. Unity does not
serialize an array of arrays, so a jagged field is never an inspector
variable — fill it in `Start()`.

`a[^1]` counts back from the end and `a[1..3]` takes a slice, on arrays and
strings:

```csharp
int[] numbers = { 1, 2, 3, 4, 5 };
int last = numbers[^1];          // 5
numbers[^1] = 50;                // writes too
int[] middle = numbers[1..4];    // a new array: 2, 3, 4
int[] tail = numbers[3..];       // to the end
string end = word[^2..];         // and on strings
```

A slice **copies** — Udon has no `Span<T>` — so `middle[0] = 9` leaves
`numbers` alone, and slicing in a loop allocates each time. A range that runs
backwards or past the end throws `ArgumentOutOfRangeException`, as in C#.

`text[i]` reads a character, as in C#, with the same
`IndexOutOfRangeException` on a bad index. Udon exposes no `String.get_Chars`,
so each read is a one-character `ToCharArray(i, 1)` — cheap, but an
allocation all the same, so walk a string with `foreach (char c in text)` or
take one `text.ToCharArray()` before a loop rather than indexing it inside
one. Writing through a string (`text[i] = c`) is an error here, as it is in
C#: a `string` is immutable.

`List<T>` also has the delegate-taking members — `Find`, `FindIndex`,
`Exists`, `TrueForAll`, `ForEach`, `RemoveAll`, `Sort(Comparison<T>)`,
`ConvertAll` — see *Delegates* below.

Both collections are source ports compiled with your code (Udon exposes
neither the real ones nor `KeyValuePair`), so they cost no externs beyond
array access, `GetHashCode` and `Equals` — which dispatch on the boxed key,
so strings, numbers, enums and engine structs hash by value and your own
classes by identity, as in .NET. `dictionary[missingKey]` throws
`KeyNotFoundException` and `Add` on a present key `ArgumentException`, and
`list[i]` outside the count `ArgumentOutOfRangeException`, as in .NET.

## Engine collections: DataList, DataDictionary and JSON

Indexers on types from the SDK and Unity work, and so do the conversion
operators their metadata declares — which is what makes `DataToken` bearable:

```csharp
var list = new DataList();
list.Add(1);                       // int -> DataToken, implicitly
list.Add("two");
DataToken first = list[0];
list[0] = 9;

var map = new DataDictionary();
map["count"] = 3;                  // the key converts too
DataToken count = map["count"];

if (VRCJson.TryDeserializeFromJson(json, out DataToken parsed))
{
    double n = parsed.DataDictionary["n"].Double;
}

Vector3 v = new Vector3(1, 2, 3);
float y = v[1];                    // engine structs index too
v[1] = 9f;
```

A conversion operator *written in your own code* (`implicit operator`) is
still an error; these are the ones the engine and SDK already ship.

## Structs

Your own structs are values, as in C#: assignment, argument passing, returning
a field, storing into a field or element, and boxing all copy; writing a
member of a variable, an array element or a field changes it in place.
Writing a member of a struct that a property, indexer or method returned is
the C# error it is (CS1612), not a silently lost write.

```csharp
public struct Point { public int x, y; public void Move(int dx) { x += dx; } }

Point a = new Point { x = 1 };
Point b = a; b.x = 9;      // a.x is still 1
points[0].Move(1);          // in place
var byKey = new Dictionary<Point, string>();   // Equals/GetHashCode are field-wise
```

Udon has no user types, so a struct is an `object[]` copied at exactly those
points — each copy is an allocation. Structs without their own
`Equals`/`GetHashCode` get field-wise ones, so they work as dictionary keys;
`==` on a struct still needs an operator, which is not supported yet.

## Not supported yet

Each of these is a compile error rather than a program that runs and does the
wrong thing:

- conversion operators (`implicit operator` / `explicit operator`);
- static abstract/virtual interface members (C# 11 generic math);
- static constructors of generic classes;
- anonymous methods (`delegate { ... }`), engine methods as delegates,
  delegate variance, events with `add`/`remove` accessors — see *Delegates,
  lambdas and events*;
- generic local functions (`T Pick<T>(T a) { ... }`) — a generic method of the
  type does the same job. `static` local functions that use a variable of the
  method around them (CS8421), and local functions that use a variable written
  below them (CS0841), are errors here as they are in C#;
- multi-dimensional arrays (`int[,]`) — Udon has no type for one; a jagged
  array (`int[][]`) does the same job and works;
- `Index` and `Range` as values (`Index i = ^1;`): `^i` and `i..j` work where
  they are written, on arrays and strings, and nowhere else — Udon has no
  such types to pass around, and no `Span<T>` for a slice that does not copy;
- `ulong` literals (`1UL`), and integer literals too big for `long`;
- nested array braces (`{ { 1, 2 }, { 3, 4 } }`) — jagged and rectangular
  arrays are not there yet;
- awaiting a task across a program boundary — see *async/await*.

Target-typed `new()` (`List<int> values = new();`, `Counter c = new(5);`) and
the null-coalescing assignment (`name ??= "anon";`, which evaluates its right
side only when the left is null) work as in C#.

`ref`/`out` work everywhere: on engine methods (`Physics.Raycast(ray, out hit)`,
`int.TryParse`) and on methods you define yourself. Named arguments
(`Mathf.Clamp(value: v, min: 0f, max: 1f)`) bind by name and are evaluated in
the order written, optional parameters (`void Log(string text, int level =
1)`, engine methods' too) take their defaults at the call site, and `params`
(`Sum(1, 2, 3)`, `string.Join(",", a, b)`) gathers the trailing arguments
into one array — all as in C#.

## Links

- Source, issues: https://github.com/ProjectTesca/MenSharp

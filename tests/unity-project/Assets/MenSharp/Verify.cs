// MenSharp verification: everything added since the last time this was run on
// a real VM, in one behaviour.
//
// Setup (5 minutes):
//   1. Put this and VerifyTarget.cs in Assets/MenSharp/.
//   2. Make a GameObject "Verify" — a Cube is ideal: it comes with a Box
//      Collider (which Interact needs, and which GetComponent looks for) and
//      you can see where it is. Put it near the spawn, about eye height. Then
//      add the Verify component. Do *not* add a Rigidbody: it would fall.
//   3. Make a GameObject "Target" and add the VerifyTarget component.
//   4. Make a GameObject "Clone Me" (a cube will do), and *disable* it —
//      VRChat can only clone objects already in the scene.
//   5. On Verify, assign: target = Target, prefab = Clone Me.
//   6. Enter play mode (ClientSim) and click the Verify object.
//
// Read the Console top to bottom. Every line starts with [verify].

using System;
using System.Collections.Generic;
using MenSharp;
using UnityEngine;
using VRC.SDKBase;

public interface ICreature
{
    string IsAlive();
}
public interface IAnimal : ICreature
{
    void Speak();
}

class Dog : IAnimal
{
    public string IsAlive()
    {
        return "生きてるワン！";
    }

    public void Speak()
    {
        Debug.Log("ワン!");
    }
}

struct Cat : IAnimal
{
    public string IsAlive()
    {
        return "生きてるにゃー！";
    }

    public void Speak()
    {
        Debug.Log("にゃー！");
    }
}

public enum Mood { Sad, Happy, Locked = 10 }

// 15. class/struct/interface semantics: constructor chains, static
//     constructors, explicit implementations, object members, casts
class Base
{
    public int v = 5;
    public int w;
    public int fromVirtual;
    public Base() { w = 1; fromVirtual = F(); }
    public Base(int x) { w = x; }
    public virtual int F() { return 1; }
    public override string ToString() { return "Base" + v; }
}

class Kid : Base
{
    public int own;
    public Kid(int x) : base(x) { own = x * 2; }
    public Kid(int x, int y) : this(x) { own += y; }
    public override int F() { return 2; }
    public override string ToString() { return "Kid" + own; }
    public override bool Equals(object o) { var k = o as Kid; return k != null && k.own == own; }
    public override int GetHashCode() { return own; }
}

class PlainKid : Base
{
    public override int F() { return 2; }
}

static class Cfg
{
    public static int seed;
    public static int twice = Other.Value * 2;
    static Cfg() { seed = 42 + twice; }
}

static class Other
{
    public static int Value = Compute();
    static int Compute() { return 10; }
}

public interface IA { int F(); }
public interface IB { int F(); }

class Both : IA, IB
{
    int IA.F() { return 1; }
    int IB.F() { return 2; }
    public int F() { return 3; }
}

struct Pt { public int x; }

// 17. default interface methods
public interface IGreeter
{
    string Name { get; }
    string Greet() { return "Hi " + Name; }
    string Title => "Mx";
}
public interface IPolite : IGreeter
{
    string IGreeter.Greet() { return "Good day " + Name; }
}
class PlainGreeter : IGreeter { public string Name => "plain"; }
class LoudGreeter : IGreeter { public string Name => "loud"; public string Greet() { return "YO " + Name; } }
class PoliteGreeter : IPolite { public string Name => "polite"; }
struct UnitGreeter : IGreeter { public int n; public string Name => "unit" + n; }

// 20. exceptions
class DoorLockedException : InvalidOperationException
{
    public int Code;
    public DoorLockedException(int code) : base("door " + code + " is locked") { Code = code; }
}

// 16. user-defined operators
struct V2
{
    public int x; public int y;
    public V2(int x, int y) { this.x = x; this.y = y; }
    public static V2 operator +(V2 a, V2 b) { return new V2(a.x + b.x, a.y + b.y); }
    public static V2 operator -(V2 a) { return new V2(-a.x, -a.y); }
    public static V2 operator *(V2 a, int k) { return new V2(a.x * k, a.y * k); }
    public static bool operator ==(V2 a, V2 b) { return a.x == b.x && a.y == b.y; }
    public static bool operator !=(V2 a, V2 b) { return !(a == b); }
    public static V2 operator ++(V2 a) { return new V2(a.x + 1, a.y + 1); }
    public override bool Equals(object o) { return o is V2 v && v == this; }
    public override int GetHashCode() { return x * 31 + y; }
    public override string ToString() { return "(" + x + "," + y + ")"; }
}

[UdonBehaviourSyncMode(BehaviourSyncMode.Manual)]
public class Verify : MenSharpBehaviour
{
    public VerifyTarget target;
    public GameObject prefab;

    [UdonSynced] public int syncedCount;
    [UdonSynced(UdonSyncMode.Linear)] public float syncedDial;
    public int localCount;

    private GameObject clone;

    public void Start()
    {
        Debug.Log("[verify] ---- Start ----");

        // 1. self references
        Debug.Log($"[verify] 1a gameObject.name = {gameObject.name}   (expect: Verify)");
        Debug.Log($"[verify] 1b transform.name  = {transform.name}    (expect: Verify)");

        // 2. typeof, whose System.Type the importer has to build
        Debug.Log($"[verify] 2  typeof(BoxCollider) = {typeof(BoxCollider)}   (expect: UnityEngine.BoxCollider)");

        // 3. GetComponent<T> — a generic extern taking that type as a value.
        //    The collider Interact needs is also what this looks for, so the
        //    test needs no component it would not have anyway.
        if (!TryGetComponent<BoxCollider>(out var box))
        {
            Debug.LogError("[verify] 3  GetComponent<BoxCollider>() was null — this object needs a Box Collider (a Cube has one)");
        }
        else
        {
            Debug.Log($"[verify] 3  GetComponent<BoxCollider>() = {box.name}   (expect: Verify)");
        }

        // 4. the other behaviour, reached by name across programs
        if (target == null)
        {
            Debug.LogError("[verify] 4  target is not assigned");
        }
        else
        {
            Debug.Log($"[verify] 4a target.label = {target.label}   (expect: target)");
            target.poked = 10;
            Debug.Log($"[verify] 4b wrote target.poked = 10, read back {target.poked}");
            target.Poke();
            Debug.Log($"[verify] 4c after Poke(), target.poked = {target.poked}   (expect: 11)");

            // 21. calls with arguments and results across programs
            target.Slide(4);
            Debug.Log($"[verify] 21a after Slide(4), target.poked = {target.poked}   (expect: 15)");
            int added = target.Add(20, 22);
            Debug.Log($"[verify] 21b target.Add(20, 22) = {added}   (expect: 42)");
            bool ok = target.Take(5, out int rest);
            Debug.Log($"[verify] 21c target.Take(5, out rest) = {ok}, rest = {rest}   (expect: True, 10)");
            bool notOk = target.Take(100, out int deficit);
            Debug.Log($"[verify] 21d target.Take(100, out deficit) = {notOk}, deficit = {deficit}   (expect: False, -85)");
            Debug.Log($"[verify] 21e target.Describe(\"ab\", 3) = {target.Describe("ab", 3)}   (expect: ababab)");
            target.Level = 7;
            Debug.Log($"[verify] 21f target.Level = {target.Level} after writing 7 (setter should have logged)   (expect: 7)");
        }

        // 9. `out` arguments — the extern writes straight into the variable
        if (int.TryParse("123", out int parsed))
        {
            Debug.Log($"[verify] 9a int.TryParse(\"123\", out parsed) → {parsed}   (expect: 123)");
        }
        else
        {
            Debug.LogError("[verify] 9a int.TryParse(\"123\") returned false");
        }

        // 12. writing another behaviour's [FieldChangeCallback] field goes
        //     through SetProgramVariable, so the runtime runs its Dial setter
        if (target != null)
        {
            target.dial = 42;
            Debug.Log($"[verify] 12a wrote target.dial = 42; read back {target.dial}, setter ran {target.dialChanges} time(s)   (expect: 42, 1)");
        }

        // 13. recursion — no attribute, no configuration
        Debug.Log($"[verify] 13 Fib(12) = {Fib(12)}   (expect: 144)");

        // 14. enums: our own compile to ints; engine enums are boxed values
        //     built by the importer (KeyCode.Space = 32)
        Mood mood = Mood.Happy;
        int moodReport = 0;
        switch (mood)
        {
            case Mood.Sad:
                moodReport = 1;
                break;
            case Mood.Happy:
                moodReport = 2;
                break;
            default:
                moodReport = 3;
                break;
        }
        Debug.Log($"[verify] 14a switch over own enum picked {moodReport}   (expect: 2)");
        KeyCode key = KeyCode.Space;
        Debug.Log($"[verify] 14b KeyCode.Space as int = {(int)key}, equal check = {key == KeyCode.Space}   (expect: 32, True)");

        // aimed down at our own cube from 1m above its centre, so it hits the
        // top face — expect distance 0.5 on a default cube
        Ray ray = new Ray(transform.position + Vector3.up, Vector3.down);
        if (Physics.Raycast(ray, out RaycastHit hit, 10f))
        {
            Debug.Log($"[verify] 9b Physics.Raycast hit {hit.collider.name} at distance {hit.distance}   (expect: Verify, 0.5)");
        }
        else
        {
            Debug.LogError("[verify] 9b Physics.Raycast hit nothing — expected our own Box Collider below");
        }

        var names = new List<string>
        {
            "a",
            "b"
        };
        foreach (var n in names) Debug.Log(n);
        foreach (char c in "ok") Debug.Log(c);

        Vector3 d = default; Debug.Log(d.x);
        var v = new Vector3 { x = 1f }; Debug.Log(v.x);

        var e = new Dictionary<string, int> { { "a", 1 } };
        e["b"] = 2;
        foreach (var p in e) Debug.Log($"{p.Key}={p.Value}");

        var dog = new Dog();
        var cat = new Cat();
        SpeakStatic(dog);
        SpeakStatic(cat);
        SpeakDyn(dog);
        SpeakDyn(cat);
        dog.Speak();
        cat.Speak();

        CheckAliveStatic(dog);
        CheckAliveStatic(cat);
        CheckAliveDyn(dog);
        CheckAliveDyn(cat);

        // 15. class/struct/interface semantics
        var kid = new Kid(3, 1);
        Debug.Log($"[verify] 15a ctor chain (this→base): v={kid.v} w={kid.w} own={kid.own}   (expect: 5, 3, 7)");
        var plain = new PlainKid();
        Debug.Log($"[verify] 15b implicit ctor → base(): v={plain.v} w={plain.w} virtual-from-base-ctor={plain.fromVirtual}   (expect: 5, 1, 2)");
        Debug.Log($"[verify] 15c static ctor after field initializers: Cfg.seed={Cfg.seed} Cfg.twice={Cfg.twice}   (expect: 62, 20)");
        var both = new Both();
        IA ia = both;
        IB ib = both;
        Debug.Log($"[verify] 15d explicit implementations: IA={ia.F()} IB={ib.F()} public={both.F()}   (expect: 1, 2, 3)");
        object o = kid;
        Debug.Log($"[verify] 15e object members: ToString={o} Equals={o.Equals(new Kid(3, 1))} hash={o.GetHashCode()} concat={"<" + kid + ">"}   (expect: Kid7, True, 7, <Kid7>)");
        Base asBase = plain;
        Debug.Log($"[verify] 15f inherited ToString via base type: {asBase}   (expect: Base5)");
        object boxedPt = new Pt { x = 1 };
        Debug.Log($"[verify] 15g struct via object: ToString={boxedPt} Equals={boxedPt.Equals(new Pt { x = 1 })} notEqual={boxedPt.Equals(new Pt { x = 2 })}   (expect: Pt, True, False)");
        object boxedInt = 5;
        Debug.Log($"[verify] 15h boxed int via object: ToString={boxedInt} is int={boxedInt is int} is string={boxedInt is string}   (expect: 5, True, False)");
        Base kidAsBase = kid;
        var backToKid = kidAsBase as Kid;
        var notPlain = kidAsBase as PlainKid;
        Debug.Log($"[verify] 15i as: {(backToKid == null ? "null" : "Kid")} {(notPlain == null ? "null" : "PlainKid")}   (expect: Kid, null)");
        Debug.Log($"[verify] 15j is: {kidAsBase is Kid} {kidAsBase is PlainKid} {(kidAsBase is Kid ? 1 : 0)}   (expect: True, False, 1)");
        if (kidAsBase is Kid bound)
        {
            Debug.Log($"[verify] 15k is-pattern bound: own={bound.own}   (expect: 7)");
        }
        Kid checkedCast = (Kid)kidAsBase;
        Debug.Log($"[verify] 15l checked cast that succeeds: own={checkedCast.own}   (expect: 7)");
        IAnimal animal = cat;
        Debug.Log($"[verify] 15m struct through interface: is Cat={animal is Cat} is Dog={animal is Dog}   (expect: True, False)");
        // engine class hierarchies: Type.IsInstanceOfType on the real objects
        object component = GetComponent<BoxCollider>();
        Debug.Log($"[verify] 15n engine is: BoxCollider={component is BoxCollider} Collider={component is Collider} Component={component is Component} SphereCollider={component is SphereCollider} Rigidbody={component is Rigidbody}   (expect: True, True, True, False, False)");
        var asCollider = component as Collider;
        var asRigidbody = component as Rigidbody;
        Debug.Log($"[verify] 15o engine as: Collider={(asCollider == null ? "null" : asCollider.name)} Rigidbody={(asRigidbody == null ? "null" : "??")}   (expect: Verify, null)");
        if (component is Collider bound2)
        {
            Debug.Log($"[verify] 15p engine is-pattern: enabled={bound2.enabled}   (expect: True)");
        }
        Collider downcast = (Collider)component;
        Debug.Log($"[verify] 15q engine checked cast: {downcast.name}   (expect: Verify)");
        Debug.Log("[verify] 15r a *failed* cast halts the behaviour: click the VerifyCrash object to see it");

        // 16. user-defined operators, next to Unity's own (Vector3 + Vector3)
        var v1 = new V2(1, 2);
        var v2 = new V2(10, 20);
        var sum = v1 + v2 * 2;
        var neg = -v1;
        var inc = v1; inc++; inc += v2;
        Debug.Log($"[verify] 16a operators: sum={sum} neg={neg} inc={inc}   (expect: (21,42), (-1,-2), (12,23))");
        Debug.Log($"[verify] 16b comparisons: ==:{v1 == new V2(1, 2)} !=:{v1 != v2} Equals:{v1.Equals(new V2(1, 2))}   (expect: True, True, True)");
        Vector3 engine = Vector3.one + Vector3.up * 2f;
        Debug.Log($"[verify] 16c engine operators still work: {engine}   (expect: (1.0, 3.0, 1.0))");

        // 17. default interface methods
        IGreeter g1 = new PlainGreeter();
        IGreeter g2 = new LoudGreeter();
        IGreeter g3 = new PoliteGreeter();
        IGreeter g4 = new UnitGreeter { n = 3 };
        Debug.Log($"[verify] 17a default bodies: {g1.Greet()} | {g2.Greet()} | {g3.Greet()} | {g4.Greet()} | {g1.Title}   (expect: Hi plain | YO loud | Good day polite | Hi unit3 | Mx)");
        Debug.Log($"[verify] 17b static dispatch to a default body: {GreetStatic(new UnitGreeter { n = 7 })}   (expect: Hi unit7)");

        // 18. `is` patterns beyond types
        object boxed = 5;
        bool c1 = localCount is 0, c2 = mood is Mood.Happy or Mood.Locked, c3 = boxed is 5, c4 = boxed is not null, c5 = prefab is null;
        Debug.Log($"[verify] 18a constants: {c1} {c2} {c3} {c4} {c5}   (expect: True, True, True, True, False)");
        bool r1 = syncedDial is >= 0f and < 1f, r2 = boxed is > 3, r3 = gameObject.name is "Verify";
        Debug.Log($"[verify] 18b relational: {r1} {r2} {r3}   (expect: True, True, True)");
        bool p1 = kid is Kid { own: 7 };
        bool p2 = kid is Base { v: > 3 } bigKid && bigKid.w == 3;
        bool p3 = kid is Kid { own: < 0 };
        bool p4 = transform is { childCount: >= 0 };
        Debug.Log($"[verify] 18c property patterns: {p1} {p2} {p3} {p4}   (expect: True, True, False, True)");

        // 19. switch with patterns, `when`, and switch expressions
        string s1 = DescribeSwitch(kid);
        string s2 = DescribeSwitch(null);
        string s3 = DescribeSwitch(new PlainKid());
        string s4 = boxed switch { int i when i > 3 => "big int", int i => "int" + i, _ => "other" };
        Debug.Log($"[verify] 19a switch patterns: {s1} | {s2} | {s3} | {s4}   (expect: kid7 | null | base5 | big int)");

        // 20. exceptions
        string e1 = "";
        try
        {
            OpenDoor(3);
            e1 = "opened";
        }
        catch (DoorLockedException exception) when (exception.Code == 3)
        {
            e1 = "caught " + exception.Message;
        }
        finally
        {
            e1 += " (finally ran)";
        }
        Debug.Log($"[verify] 20a throw/catch/when/finally: {e1}   (expect: caught door 3 is locked (finally ran))");
        string e2 = "";
        int[] small = new int[2];
        Kid nobody = null;
        int zero = small[0];
        try
        {
            small[5] = 1;
        }
        catch (IndexOutOfRangeException)
        {
            e2 += "index ";
        }
        try
        {
            e2 += nobody.own;
        }
        catch (NullReferenceException)
        {
            e2 += "null ";
        }
        try
        {
            e2 += 10 / zero;
        }
        catch (DivideByZeroException)
        {
            e2 += "divide ";
        }
        try
        {
            object str = "s"; Kid k = (Kid)str;
        }
        catch (InvalidCastException)
        {
            e2 += "cast ";
        }
        try
        {
            var dictionary = new Dictionary<string, int>(); e2 += dictionary["missing"];
        }
        catch (KeyNotFoundException)
        {
            e2 += "key";
        }
        Debug.Log($"[verify] 20b runtime checks: {e2}   (expect: index null divide cast key)");
        try
        {
            OpenDoor(3);
        }
        catch (Exception exception)
        {
            Debug.Log($"[verify] 20c ToString with stack trace:\n{exception}   (expect: DoorLockedException: door 3 is locked, then `at Verify.OpenDoor in Assets/MenSharp/Verify.cs:<line>:<col>` and `at Verify.Start ...`)");
        }
        Debug.Log("[verify] 20d an *uncaught* exception halts the behaviour: click the VerifyCrash object to see it");
    }

    static void OpenDoor(int code)
    {
        if (code == 3) { throw new DoorLockedException(code); }
    }

    static string DescribeSwitch(Base b)
    {
        switch (b)
        {
            case null: return "null";
            case Kid { own: > 5 } k when k.v == 5: return "kid" + k.own;
            case Kid k: return "smallkid" + k.own;
            case { v: 5 }: return "base" + b.v;
            default: return "?";
        }
    }

    public static string GreetStatic<T>(T greeter) where T : IGreeter
    {
        return greeter.Greet();
    }

    public static void CheckAliveStatic<T>(T creature) where T : ICreature
    {
        Debug.Log(creature.IsAlive());
    }

    public static void CheckAliveDyn(ICreature creature)
    {
        Debug.Log(creature.IsAlive());
    }

    public static void SpeakStatic<T>(T animal) where T : IAnimal
    {
        animal.Speak();
    }

    public static void SpeakDyn(IAnimal animal)
    {
        animal.Speak();
    }

    public void Interact()
    {
        Debug.Log("[verify] ---- Interact ----");

        // 5. cloning, and destroying what was cloned
        if (prefab == null)
        {
            Debug.LogError("[verify] 5  prefab is not assigned");
        }
        else if (clone == null)
        {
            Debug.Log($"[verify] 5a cloning {prefab.name}");
            clone = Instantiate(prefab);
            clone.SetActive(true);
            Debug.Log($"[verify] 5b got {clone.name}, active={clone.activeSelf} — click again to Destroy it");
        }
        else
        {
            Debug.Log($"[verify] 5c destroying {clone.name}");
            Destroy(clone);
            clone = null;
        }

        // 6. networking. Alone in the world this only proves the calls run;
        //    with a second player, syncedCount should match on both screens.
        if (!Networking.IsOwner(gameObject))
        {
            Networking.SetOwner(Networking.LocalPlayer, gameObject);
            Debug.Log("[verify] 6a took ownership");
        }
        syncedCount += 1;
        syncedDial += 0.25f;
        localCount += 1;
        RequestSerialization();
        Debug.Log($"[verify] 6b synced={syncedCount} dial={syncedDial} local={localCount}");

        // 7. an event raised on ourselves by name
        SendCustomEvent("Echo");
    }

    public void Echo()
    {
        Debug.Log("[verify] 7  SendCustomEvent(\"Echo\") arrived");
    }

    private int Fib(int n)
    {
        if (n < 2) { return n; }
        return Fib(n - 1) + Fib(n - 2);
    }

    public void OnDeserialization()
    {
        Debug.Log($"[verify] 8  OnDeserialization: synced={syncedCount}   (only with another player)");
    }

    // 10. an event argument — the runtime writes it into a named heap slot
    //     before raising the event. Fires once at start in ClientSim, when
    //     the local (test) player joins.
    public void OnPlayerJoined(VRCPlayerApi player)
    {
        if (player == null)
        {
            Debug.LogError("[verify] 10 OnPlayerJoined fired but player is null");
        }
        else
        {
            Debug.Log($"[verify] 10 OnPlayerJoined: {player.displayName} (local={player.isLocal})");
        }
    }
}

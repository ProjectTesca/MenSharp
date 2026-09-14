using System;
using System.Collections.Generic;
using System.Linq;
using MenSharp;
using UnityEngine;
using VRC.SDK3.Image;

// A deliberately small black-box test program. The Unity test runner invokes
// its exported events through the SDK's real UdonBehaviour and reads these
// public variables back from the Udon heap.
public class MenSharpRuntimeSmoke : MenSharpBehaviour
{
    public bool syncDone;
    public int syncResult;
    public string syncText;

    public bool asyncDone;
    public int asyncResult;

    // one count for every instance of this behaviour
    public static int shared;
    public int mine;

    // a `const string` read from an event — came up null once the SDK's
    // inspector had listed it as a public variable (issue)
    // a number cast to an engine enum has to arrive at an extern as the
    // boxed enum, not as the Int32 it was (issue: the Texture2D constructor
    // halted the behaviour)
    public int formatValue = 4; // TextureFormat.RGBA32
    public string formatName;
    public void MakeTexture()
    {
        TextureFormat format = (TextureFormat)formatValue;
        var texture = new Texture2D(2, 2, format, false);
        formatName = texture.format.ToString();
    }

    // operators on engine/corlib enums: computed on the int, boxed back —
    // and the result has to be what an extern accepts
    public int splitCount;
    public int flagsValue;
    public bool hasFlag;
    public string keyName;
    public void EnumOps()
    {
        StringSplitOptions options = StringSplitOptions.None | StringSplitOptions.RemoveEmptyEntries;
        splitCount = "a,,b".Split(new[] { ',' }, options).Length;
        flagsValue = (int)(options & ~StringSplitOptions.None);
        hasFlag = (options & StringSplitOptions.RemoveEmptyEntries) != 0;
        KeyCode key = KeyCode.A + 1;
        Input.GetKey(key);
        keyName = key.ToString();
    }

    // `float x = 0;` on a field the proxy does not transfer (private): the
    // slot must hold a Single, or the multiplication halts the VM (issue)
    private float scale = 0;
    public float scaled;
    public void Scale()
    {
        scaled = scale * 0.92f + 1f;
    }

    // a public array field with an initializer, set in the inspector: the
    // transferred value must survive, not be wiped by `= { }` at startup
    // (issue: intArray.Length came out 0). And `private float = 1` on a
    // field the proxy does not transfer must reach the VM as a Single.
    public int[] items = { };
    public int itemsLength;
    private float one = 1;
    public float oneProduct;
    public void CountItems() { itemsLength = items.Length; }
    public void ScaleOne() { oneProduct = one * 0.92f; }

    // a private field Udon cannot construct: the importer runs the C#
    // initializer on the proxy and bakes the VRCUrl into the heap default
    private VRC.SDKBase.VRCUrl url = new VRC.SDKBase.VRCUrl("https://example.com/baked");
    public string urlText;
    public void ReadUrl() { urlText = url.Get(); }

    const string greeting = "Hello";
    public string greeted;
    public void Greet()
    {
        greeted = $"Start: {greeting}";
    }

    public void RunShared()
    {
        shared++;
        mine = shared;
    }

    // a static of a generic class is one per closed type: Count<int> and
    // Count<string> must not share (issue), and each is shared across every
    // instance the same way `shared` is
    public int genericInt;
    public int genericString;
    public int genericMine;

    public int genericSeededInt;
    public int genericSeededString;

    public void GenericStatics()
    {
        GenericCache<int>.Count++;
        GenericCache<string>.Count += 10;
        genericInt = GenericCache<int>.Count;
        genericString = GenericCache<string>.Count;
        // each closed type takes its own constant initializer (7)
        GenericCache<int>.Seeded += 1;
        genericSeededInt = GenericCache<int>.Seeded;    // 8
        genericSeededString = GenericCache<string>.Seeded; // 7, untouched
    }

    public void BumpGeneric()
    {
        GenericCache<int>.Count++;
        genericMine = GenericCache<int>.Count;
    }

    // `is null || …` must be `(x is null) || …`, not `x is (null || …)`
    public bool arrayFull;
    public bool arrayEmpty;

    public void NullOrEmpty()
    {
        int[] a = { 1, 2, 3 };
        arrayFull = a is null || a.Length == 0;   // false
        int[] b = { };
        arrayEmpty = b is null || b.Length == 0;  // true
    }

    // a switch-expression arm guard: `2 when ok => 10` must keep its `=>`
    // (the guard once swallowed `ok => 10` as a lambda)
    public int guardedFalse;
    public int guardedTrue;

    public void GuardedSwitch()
    {
        int value = 2;
        bool ok = false;
        guardedFalse = value switch { 2 when ok => 10, _ => 20 };
        ok = true;
        guardedTrue = value switch { 2 when ok => 10, _ => 20 };
    }

    // overload tie-breaks C# makes and M# once called ambiguous: `F1(null)`
    // is the string overload, `F2(short)` the int one
    public string overloadNull;
    public string overloadShort;
    string F1(object x) => "object";
    string F1(string x) => "string";
    string F2(int x) => "int";
    string F2(long x) => "long";

    public void Overloads()
    {
        overloadNull = F1(null);
        short x = 1;
        overloadShort = F2(x);
    }

    // `ref` is an alias: `R1(ref x1, ref x1)` sees one variable (8, not 7),
    // and `R2(ref this.refField)` reading `this.refField` sees its own write
    // (20, not 13)
    public int refAliased;
    public int refViaField;
    public int refField;
    void R1(ref int a, ref int b) { a++; b += a; }
    void R2(ref int a) { a = 10; a += this.refField; }

    // ...and so are referents that are elements rather than heap symbols: a
    // shared static (8, and 20 reading itself), a class instance's field
    // (20), an array element (8), a captured local (105: bumped, +100,
    // bumped — a copy would be 103)
    public int refViaStatic;
    public int refStaticReadsItself;
    public int refViaClassField;
    public int refViaElement;
    public int refViaCaptured;
    void R3(ref int a) { a = 10; a += RefCounter.total; }
    void R4(ref int a, RefHolder h) { a = 10; a += h.v; }
    void R5(ref int v, System.Action act) { act(); v += 100; act(); }

    public void RefAliasing()
    {
        int x1 = 3;
        R1(ref x1, ref x1);
        refAliased = x1;
        refField = 3;
        R2(ref this.refField);
        refViaField = refField;

        RefCounter.total = 3;
        R1(ref RefCounter.total, ref RefCounter.total);
        refViaStatic = RefCounter.total;
        RefCounter.total = 3;
        R3(ref RefCounter.total);
        refStaticReadsItself = RefCounter.total;
        var h = new RefHolder { v = 3 };
        R4(ref h.v, h);
        refViaClassField = h.v;
        int[] items = { 3 };
        R1(ref items[0], ref items[0]);
        refViaElement = items[0];
        int c = 3;
        System.Action bump = () => c++;
        R5(ref c, bump);
        refViaCaptured = c;
    }

    // a user enum (byte-backed, even) set from the inspector: the proxy
    // transfer must hand the M# heap an Int32, not the C# enum (issue: the
    // VM halted reading `mode` as Int32). Arrays of one are object[] of Int32s.
    [SerializeField] private InspectorEnumMode mode;
    [SerializeField] private InspectorEnumMode[] modes;
    public bool modeIsSecond;
    public int modesSecondCount;

    public void CheckMode()
    {
        modeIsSecond = mode == InspectorEnumMode.Second;
        modesSecondCount = 0;
        foreach (InspectorEnumMode each in modes)
        {
            if (each == InspectorEnumMode.Second)
            {
                modesSecondCount++;
            }
        }
    }

    // IVRCImageDownload : IDisposable, and VRCSDK3.dll names IDisposable as
    // living in `netstandard` — an assembly the compiler is never given. The
    // inherited Dispose() must still resolve (by name, into the loaded
    // corlib) and map to the extern Udon exposes under the receiver's own
    // interface name. Compile-only: there is no download to run it on here.
    public void OnImageLoadSuccess(IVRCImageDownload result)
    {
        result.Dispose();
    }

    // VRCPlayerApi.TrackingDataType / .TrackingData are nested types: their
    // externs are named after the enclosing type
    // (`VRCSDKBaseVRCPlayerApi.__GetTrackingData__VRCSDKBaseVRCPlayerApiTrackingDataType__…`).
    // Compile-only: there is no local player to ask in the test scene.
    public Vector3 headPosition;

    public void TrackHead()
    {
        VRC.SDKBase.VRCPlayerApi player = VRC.SDKBase.Networking.LocalPlayer;
        if (player == null) return;
        VRC.SDKBase.VRCPlayerApi.TrackingData head =
            player.GetTrackingData(VRC.SDKBase.VRCPlayerApi.TrackingDataType.Head);
        headPosition = head.position;
    }

    // a delegate of one instance, invoked by another: the invoker hands it
    // back to the program that made it
    public static event Action<int> OnPublished;
    public int toPublish;
    public int heard;

    public void Subscribe()
    {
        OnPublished += value => { heard = value * 10; };
    }

    public void Publish()
    {
        OnPublished?.Invoke(toPublish);
    }

    public void RunSync()
    {
        var values = new List<int> { 1, 2, 3, 4 };
        syncResult = values.Where(value => value % 2 == 0).Sum();
        syncText = $"sum={syncResult}";
        syncDone = true;
    }

    public async void RunAsync()
    {
        await Scheduler.NextFrame();
        asyncResult = 42;
        asyncDone = true;
    }
}

// a generic class whose static field the smoke program keeps one of per closed
// type — lives beside the behaviour, since a MenSharpBehaviour cannot be generic
public static class GenericCache<T>
{
    public static int Count;
    // a constant-literal initializer: one per closed type (int and string
    // each start at 7), baked with no code
    public static int Seeded = 7;
}

// a static and a class whose members `RefAliasing` takes `ref` to
public static class RefCounter
{
    public static int total;
}

public class RefHolder
{
    public int v;
}

// a byte-backed enum: an Int32 on the M# heap whatever its C# underlying type
public enum InspectorEnumMode : byte
{
    First = 0,
    Second
}

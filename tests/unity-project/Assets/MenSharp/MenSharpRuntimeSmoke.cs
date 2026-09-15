using System;
using System.Collections.Generic;
using System.Linq;
using MenSharp;
using MenSharp.Json;
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

    // `a ?? throw …` keeps `a`'s type (issue: `.Length` on it gave 0, and
    // was an internal error as a `Debug.Log` argument); `?:` with a throw
    // arm likewise; the throw itself still throws
    public int throwLength;
    public int throwFromNullable;
    public int throwConditional;
    public string throwCaught;

    public void ThrowExpressions()
    {
        string a = "ok";
        throwLength = (a ?? throw new Exception()).Length;
        Debug.Log((a ?? throw new Exception()).Length);
        int? v = 5;
        throwFromNullable = v ?? throw new Exception();
        bool ok = throwLength == 2;
        throwConditional = (ok ? a : throw new Exception()).Length;
        string missing = null;
        try { throwLength = (missing ?? throw new Exception("absent")).Length; }
        catch (Exception e) { throwCaught = e.Message; }
    }

    // `_` is a discard: `out _` (issue: "name does not exist"), `out var _`
    // (issue: declared a variable `_`), `out int _`, `_ = F()`
    public bool discardParsed;
    public int discardGiven;
    public int discardEvaluated;
    public int discardLambda;
    void Give(out int v) { discardGiven++; v = 7; }
    int Evaluate() { discardEvaluated++; return discardEvaluated; }

    public void Discards()
    {
        var isInteger = int.TryParse("12345", out _);
        discardParsed = isInteger;
        var isInteger2 = int.TryParse("12345", out var _);
        discardParsed = discardParsed && isInteger2;
        Give(out _);
        Give(out var _);
        Give(out int _);
        _ = 1;
        _ = Evaluate();
        _ = Evaluate();
        // lambda discards: two `_` parameters are discards, a lone `_` is
        // a parameter named `_` — 1 + 2
        Func<int, int, int> pair = (_, _) => 1;
        Func<int, int> lone = _ => _ + 1;
        discardLambda = pair(5, 6) + lone(1);
    }

    // identifiers spelled with unicode escapes (issue: syntax errors): one
    // variable spelled three ways, a formatting character that is no part
    // of the name, an escaped keyword that is an identifier
    public int escapedA;
    public int escapedAb;
    public int escapedKeyword;

    public void EscapedIdentifiers()
    {
        int \u0061 = 1;
        \U00000061 = 2;
        escapedA = a;
        int a\u200Db = 3;
        escapedAb = ab;
        int \u0069nt = 4;
        escapedKeyword = @int;
    }

    // a class nested in the behaviour is an ordinary class (issue: `new
    // NestedData()` was "belongs to the behaviour itself")
    public class NestedData
    {
        public int Value;
        public int Doubled() { return Value * 2; }
    }

    public int nestedValue;
    public int nestedDoubled;

    public void NestedType()
    {
        var data = new NestedData();
        data.Value = 42;
        nestedValue = data.Value;
        nestedDoubled = data.Doubled();
    }

    // a `[JsonIgnore]`d field of a type JSON cannot read is left alone
    // (issue: a compile error naming Vector3); the same field without the
    // attribute is an exception when the JSON actually has it
    public int jsonIgnoredValue;
    public string jsonUnsupportedMessage;

    public void JsonIgnored()
    {
        jsonIgnoredValue = Json.Parse<JsonPartial>("{\"value\":42}").Unwrap().Value;
        try
        {
            Json.Parse<JsonUnsupported>("{\"value\":1,\"Position\":0}");
        }
        catch (Exception e)
        {
            jsonUnsupportedMessage = e.Message;
        }
    }

    // an engine enum read from JSON is the real boxed enum, not the Int32
    // the reader parsed (issue: the VM halted in the first extern given it)
    public bool jsonEnumEquals;
    public string jsonEnumBack;

    public void JsonEnum()
    {
        var parsed = Json.Parse<JsonComparison>("{\"mode\": 5}").Unwrap(); // OrdinalIgnoreCase
        jsonEnumEquals = "abc".Equals("ABC", parsed.Mode);                  // an extern unboxes it
        jsonEnumBack = Json.Stringify(parsed);                              // {"mode":5}
    }

    // a user enum (byte-backed, even) set from the inspector: the proxy
    // transfer must hand the M# heap an Int32, not the C# enum (issue: the
    // VM halted reading `mode` as Int32). Arrays of one are object[] of Int32s.
    // a `when` filter runs before an inner `finally` (issue: after it)
    public string exceptionOrder;

    bool ExceptionFilter()
    {
        exceptionOrder += "Filter,";
        return true;
    }

    public void ExceptionOrder()
    {
        exceptionOrder = "";
        try
        {
            try { throw new Exception(); }
            finally { exceptionOrder += "Finally,"; }
        }
        catch (Exception) when (ExceptionFilter())
        {
            exceptionOrder += "Catch";
        }
    }

    // `&`, `|`, `^` on bools evaluate both sides (issue: a compile error)
    public int boolCalls;
    public bool boolOrCall;
    public bool boolAnd;
    public bool boolXor;
    public bool boolCompound;

    bool BoolCheck() { boolCalls++; return true; }

    public void BoolOperators()
    {
        bool a = true;
        bool b = false;
        boolOrCall = a | BoolCheck();
        boolAnd = a & b;
        boolXor = a ^ b;
        a &= b;
        boolCompound = a;
    }

    // an enum keeps to its underlying type: a byte enum wraps at 255, a
    // ulong enum holds a member past long.MaxValue (issue: both were Int32)
    public bool enumByteWraps;
    public int enumByteCast;
    public bool enumULongBig;
    public string enumULongName;

    public void EnumUnderlying()
    {
        ByteEnum value = ByteEnum.Max;
        value++;
        enumByteWraps = value == ByteEnum.Zero;
        int outside = 300;                        // a constant would be CS0221 in C#
        enumByteCast = (int)(ByteEnum)outside;
        ULongEnum big = ULongEnum.Big;
        enumULongBig = big == ULongEnum.Big;
        enumULongName = big.ToString();
    }

    // a struct method on a value — a readonly field, an `in` parameter —
    // runs on a copy, as in C# (issue: the readonly field went 3 → 4);
    // on a variable — a plain field, an array element — in place
    public int copyReadonly;
    public int copyPlain;
    public int copyElement;
    public int copyIn;

    void BumpIn(in SmokeCounter c) { c.Increment(); }

    public void DefensiveCopies()
    {
        var holder = new SmokeHolder();
        holder.Counter.Increment();
        copyReadonly = holder.Counter.Value;    // 3
        holder.Plain.Increment();
        copyPlain = holder.Plain.Value;         // 4
        var items = new SmokeCounter[] { new SmokeCounter { Value = 3 } };
        items[0].Increment();
        copyElement = items[0].Value;           // 4
        var local = new SmokeCounter { Value = 3 };
        BumpIn(in local);
        copyIn = local.Value;                   // 3
    }

    // compound assignment and ++/-- on the small integral types compute in
    // int and cast back, wrapping as unchecked C# does (issue: "operator
    // `op_Addition` is not available on Udon" for `b += 2`, `sh++`, `c++`)
    public int smallByte;
    public int smallByteWrapped;
    public int smallShort;
    public int smallShortWrapped;
    public int smallSByte;
    public int smallUShort;
    public string smallChar;

    public void SmallIntegers()
    {
        byte b = 1;
        b += 2;
        smallByte = b;                    // 3
        byte w = 255;
        w += 2;
        smallByteWrapped = w;             // 1
        short sh = 1;
        sh++;
        smallShort = sh;                  // 2
        short top = 32767;
        top++;
        smallShortWrapped = top;          // -32768
        sbyte sb = 127;
        sb++;
        smallSByte = sb;                  // -128
        ushort us = 65535;
        us++;
        smallUShort = us;                 // 0
        char c = 'A';
        c++;
        smallChar = c.ToString();         // "B"
    }

    // a decimal literal is a Decimal on the heap (issue: `0.1m` was a Double,
    // `value is decimal` false and decimal addition halted the VM)
    public bool decimalIsDecimal;
    public bool decimalExact;
    public string decimalSum;

    public void Decimals()
    {
        object value = 0.1m;
        decimalIsDecimal = value is decimal;
        decimal a = 0.1m;
        decimal b = 0.2m;
        decimalExact = a + b == 0.3m;
        decimalSum = (a + b).ToString();
    }

    // a library in an auto-referenced assembly definition of its own is a
    // library source (issue: a reference check compared "Toolbox.dll" with
    // "Toolbox" and dropped every such library)
    public int toolboxTwice;

    public void UseToolbox()
    {
        toolboxTwice = Toolbox.Twice(21);
    }

    // an unassigned (or destroyed) object reference is Unity's "fake null":
    // the editor transfer must not touch it (issue: TextAsset.ToString threw
    // UnassignedReferenceException while summarizing), and the program sees
    // it as null
    [SerializeField] private TextAsset textAsset;
    [SerializeField] private string text;
    public string readText;
    public bool textAssetMissing;

    public void ReadText()
    {
        readText = text;
        textAssetMissing = textAsset == null;
    }

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

// what `EnumUnderlying` uses: the issue's enums
public enum ByteEnum : byte
{
    Zero = 0, Max = 255
}

public enum ULongEnum : ulong
{
    Big = 9223372036854775808UL
}

// what `DefensiveCopies` mutates: the issue's struct and holder
public struct SmokeCounter
{
    public int Value;
    public void Increment() { Value++; }
}

public class SmokeHolder
{
    public readonly SmokeCounter Counter = new SmokeCounter { Value = 3 };
    public SmokeCounter Plain = new SmokeCounter { Value = 3 };
}

// what `JsonIgnored` reads: the issue's class, and one that forgot the attribute
public class JsonPartial
{
    [JsonName("value")]
    public int Value { get; }

    [JsonIgnore]
    public Vector3 Position { get; }
}

public class JsonUnsupported
{
    [JsonName("value")]
    public int Value { get; }

    public Vector3 Position { get; }
}

// what `JsonEnum` reads: an engine enum property, as the issue wrote it
public class JsonComparison
{
    [JsonName("mode"), JsonRequired]
    public StringComparison Mode { get; }
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

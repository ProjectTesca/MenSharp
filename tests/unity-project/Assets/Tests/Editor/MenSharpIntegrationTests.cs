#if UNITY_EDITOR
using System;
using System.Collections;
using System.Collections.Generic;
using System.Linq;
using System.Reflection;
using NUnit.Framework;
using UnityEditor;
using UnityEditor.SceneManagement;
using UnityEngine;
using UnityEngine.TestTools;
using VRC.Udon;

public class MenSharpIntegrationTests
{
    private const string GeneratedScene = "Assets/Tests/Generated/MenSharpRuntime.unity";

    private static readonly string[] ExpectedPrograms =
    {
        "Door",
        "JsonVerify.VerifyJson",
        "MenSharp.Statics",
        "MenSharpRuntimeCaller",
        "MenSharpRuntimeSmoke",
        "MenSharpRuntimeTarget",
        "RecordVerify.VerifyRecords",
        "ShadowUrlBase",
        "Test1",
        "UnionVerify.VerifyUnion",
        "Verify",
        "VerifyArrays",
        "VerifyAsync",
        "VerifyCollections",
        "VerifyComparers",
        "VerifyCrash",
        "VerifyData",
        "VerifyDeconstruct",
        "VerifyDelegates",
        "VerifyEnumNames",
        "VerifyExternCrash",
        "VerifyFormat",
        "VerifyGetComponent",
        "VerifyHttp",
        "VerifyIteratorCleanup",
        "VerifyIterators",
        "VerifyJagged",
        "VerifyLinq",
        "VerifyLocalFunctions",
        "VerifyModern",
        "VerifyNetwork",
        "VerifyNullable",
        "VerifyRange",
        "VerifyRectangular",
        "VerifyReferences",
        "VerifyRemoteAwait",
        "VerifyRemoteDoor",
        "VerifyResult",
        "VerifySequences",
        "VerifyShadowedUrl",
        "VerifyString",
        "VerifyStringLoad",
        "VerifyStructKeys",
        "VerifyTarget",
        "VerifyTuple",
        "VerifyUdonSharp",
        "VerifyUsharpCompat",
        "VerifyVideo",
        "VerifyWake",
    };

    [TestCase("Single", "NaN")]
    [TestCase("Single", "1.25")]
    [TestCase("Single", "Infinity")]
    [TestCase("Single", "-Infinity")]
    [TestCase("Double", "NaN")]
    [TestCase("Double", "1.25")]
    [TestCase("Double", "Infinity")]
    [TestCase("Double", "-Infinity")]
    public void FloatingMetadataPreservesSpecialValuesAndBoxedTypes(string kind, string text)
    {
        // Verify the metadata spellings emitted by the compiler on Unity Mono.
        // Assert boxed types as well as values: Single slots must receive floats.
        var decode = typeof(MenSharpProgramAsset).GetMethod("Decode",
            BindingFlags.Static | BindingFlags.NonPublic);
        Assert.IsNotNull(decode);
        object actual = decode.Invoke(null, new object[] {
            new MenSharpHeapEntry { kind = kind, value = text }
        });

        if (kind == "Single")
        {
            Assert.IsInstanceOf<float>(actual);
            Assert.AreEqual(float.Parse(text, System.Globalization.CultureInfo.InvariantCulture), actual);
        }
        else
        {
            Assert.IsInstanceOf<double>(actual);
            Assert.AreEqual(double.Parse(text, System.Globalization.CultureInfo.InvariantCulture), actual);
        }
    }

    [Test]
    public void CompilerCreatesEveryManualFixtureAsAnSdkProgramAsset()
    {
        MenSharpCompiler.RebuildAll();
        AssetDatabase.Refresh(ImportAssetOptions.ForceSynchronousImport);

        string[] paths = AssetDatabase
            .FindAssets("t:MenSharpProgramAsset", new[] { "Assets/MenSharp/Programs" })
            .Select(AssetDatabase.GUIDToAssetPath)
            .OrderBy(path => path, StringComparer.Ordinal)
            .ToArray();
        string[] names = paths
            .Select(path => System.IO.Path.GetFileNameWithoutExtension(path))
            .ToArray();

        CollectionAssert.AreEqual(ExpectedPrograms, names);
        foreach (string path in paths)
        {
            var program = AssetDatabase.LoadAssetAtPath<MenSharpProgramAsset>(path);
            Assert.IsNotNull(program, path);
            Assert.IsNotNull(program.SerializedProgramAsset, path);
            Assert.IsFalse(string.IsNullOrEmpty(program.metaJson), path);
        }

        var guids = paths.ToDictionary(path => path, AssetDatabase.AssetPathToGUID);
        MenSharpCompiler.CompileAll();
        foreach (string path in paths)
        {
            Assert.AreEqual(guids[path], AssetDatabase.AssetPathToGUID(path), path);
        }
    }

    [UnityTest]
    public IEnumerator GeneratedProgramsExecuteInTheSdkUdonVm()
    {
        // The edit-mode runner holds assembly reloads back while tests run.
        // A reload still owed from before the run (a fresh project imports
        // UdonSharp's utility scripts on first load) would otherwise fire in
        // the middle of the play-mode transition below, and the editor
        // comes out of that in edit mode with Udon never initialised. Let it
        // happen here, where the runner knows how to resume the test.
        AssetDatabase.Refresh();
        if (MenSharpTestScene.ScriptReloadPending())
        {
            yield return new WaitForDomainReload();
        }

        MenSharpCompiler.CompileAll();
        AssetDatabase.Refresh(ImportAssetOptions.ForceSynchronousImport);
        BuildRuntimeScene();

        // what the SDK's UdonBehaviour inspector leaves behind for a symbol
        // it finds no value for (and what a scene saved by an earlier
        // compiler carries): a null public variable named after the const
        {
            UdonBehaviour udon = FindUdon("MenSharpRuntimeSmoke");
            udon.publicVariables.RemoveVariable("greeting");
            Assert.IsTrue(udon.publicVariables.TryAddVariable(
                new VRC.Udon.Common.UdonVariable<string>("greeting", null)));
        }

        yield return new EnterPlayMode();
        yield return null;
        yield return null;
        Assert.IsTrue(Application.isPlaying, "the editor did not enter play mode");

        UdonBehaviour smoke = FindUdon("MenSharpRuntimeSmoke");
        Assert.IsTrue(smoke.IsInitialized, "the SDK did not initialise the smoke UdonBehaviour");
        smoke.RunProgram("Greet");
        Assert.AreEqual("Start: Hello", smoke.GetProgramVariable("greeted"));

        smoke.RunProgram("MakeTexture");
        Assert.AreEqual("RGBA32", smoke.GetProgramVariable("formatName"));

        smoke.RunProgram("Scale");
        Assert.AreEqual(1f, smoke.GetProgramVariable("scaled"));

        // a private field whose initializer Udon cannot run (new VRCUrl):
        // the importer baked the value from the constructed proxy
        smoke.RunProgram("ReadUrl");
        Assert.AreEqual("https://example.com/baked", smoke.GetProgramVariable("urlText"));

        // a public array initializer must not overwrite the inspector value
        smoke.RunProgram("CountItems");
        Assert.AreEqual(1, smoke.GetProgramVariable("itemsLength"));

        // `private float = 1` reaches the VM as a Single
        smoke.RunProgram("ScaleOne");
        Assert.AreEqual(0.92f, smoke.GetProgramVariable("oneProduct"));

        smoke.RunProgram("EnumOps");
        Assert.AreEqual(2, smoke.GetProgramVariable("splitCount"));
        Assert.AreEqual(1, smoke.GetProgramVariable("flagsValue"));
        Assert.AreEqual(true, smoke.GetProgramVariable("hasFlag"));
        Assert.AreEqual("B", smoke.GetProgramVariable("keyName"));

        smoke.RunProgram("RunSync");
        Assert.AreEqual(true, smoke.GetProgramVariable("syncDone"));
        Assert.AreEqual(6, smoke.GetProgramVariable("syncResult"));
        Assert.AreEqual("sum=6", smoke.GetProgramVariable("syncText"));

        smoke.RunProgram("RunAsync");
        float deadline = Time.realtimeSinceStartup + 5f;
        while (!Equals(true, smoke.GetProgramVariable("asyncDone"))
               && Time.realtimeSinceStartup < deadline)
        {
            yield return null;
        }
        Assert.AreEqual(true, smoke.GetProgramVariable("asyncDone"), "async event timed out");
        Assert.AreEqual(42, smoke.GetProgramVariable("asyncResult"));

        UdonBehaviour caller = FindUdon("MenSharpRuntimeCaller");
        caller.RunProgram("RunRemote");
        Assert.AreEqual(true, caller.GetProgramVariable("done"));
        Assert.AreEqual(42, caller.GetProgramVariable("result"));

        // SetProgramVariable("received", int[]) — a generic method that erases
        // to the (string, object) extern; passing an array once failed to compile
        UdonBehaviour arrayTarget = FindUdon("MenSharpRuntimeTarget");
        caller.SetProgramVariable("receiver", arrayTarget);
        caller.RunProgram("PushArray");
        Assert.AreEqual(new int[] { 7, 8, 9 }, arrayTarget.GetProgramVariable("received"));

        // a static field is one for every instance (issue: each instance
        // counted from 0), through the holder the scene carries
        GameObject holderObject = GameObject.Find(MenSharpProxy.StaticsHolderName);
        Assert.IsNotNull(holderObject, "no statics holder in the scene");
        // the holder has no synced variables and never networks: it must sync
        // None, not the Continuous a fresh UdonBehaviour defaults to
        Assert.AreEqual(
            VRC.SDKBase.Networking.SyncType.None,
            holderObject.GetComponent<UdonBehaviour>().SyncMethod,
            "the statics holder should sync None");
        UdonBehaviour second = FindUdon("MenSharpRuntimeSmoke2");
        smoke.RunProgram("RunShared");
        second.RunProgram("RunShared");
        smoke.RunProgram("RunShared");
        Assert.AreEqual(3, smoke.GetProgramVariable("mine"));
        Assert.AreEqual(2, second.GetProgramVariable("mine"));

        // a static of a generic class: one per closed type (int vs string do
        // not share), and — like `shared` above — one across every instance
        // `values is null || values.Length == 0` binds as `(is null) || (…)`
        smoke.RunProgram("NullOrEmpty");
        Assert.AreEqual(false, smoke.GetProgramVariable("arrayFull"));
        Assert.AreEqual(true, smoke.GetProgramVariable("arrayEmpty"));

        // `2 when ok => 10`: the arm guard keeps its `=>`
        smoke.RunProgram("GuardedSwitch");
        Assert.AreEqual(20, smoke.GetProgramVariable("guardedFalse"));
        Assert.AreEqual(10, smoke.GetProgramVariable("guardedTrue"));

        // the better function member: F1(null) → string, F2(short) → int
        smoke.RunProgram("Overloads");
        Assert.AreEqual("string", smoke.GetProgramVariable("overloadNull"));
        Assert.AreEqual("int", smoke.GetProgramVariable("overloadShort"));

        // `ref` aliases: same variable twice (8), and through a field (20)
        smoke.RunProgram("RefAliasing");
        Assert.AreEqual(8, smoke.GetProgramVariable("refAliased"));
        Assert.AreEqual(20, smoke.GetProgramVariable("refViaField"));
        // ...and element referents: a static, a class field, an array
        // element, a captured local
        Assert.AreEqual(8, smoke.GetProgramVariable("refViaStatic"));
        Assert.AreEqual(20, smoke.GetProgramVariable("refStaticReadsItself"));
        Assert.AreEqual(20, smoke.GetProgramVariable("refViaClassField"));
        Assert.AreEqual(8, smoke.GetProgramVariable("refViaElement"));
        Assert.AreEqual(105, smoke.GetProgramVariable("refViaCaptured"));

        // `a ?? throw …` / `c ? a : throw …` keep the typed operand's type
        smoke.RunProgram("ThrowExpressions");
        Assert.AreEqual(2, smoke.GetProgramVariable("throwLength"));
        Assert.AreEqual(5, smoke.GetProgramVariable("throwFromNullable"));
        Assert.AreEqual(2, smoke.GetProgramVariable("throwConditional"));
        Assert.AreEqual("absent", smoke.GetProgramVariable("throwCaught"));

        // `_` discards: extern and source `out _`/`out var _`/`out int _`, `_ = F()`
        smoke.RunProgram("Discards");
        Assert.AreEqual(true, smoke.GetProgramVariable("discardParsed"));
        Assert.AreEqual(3, smoke.GetProgramVariable("discardGiven"));
        Assert.AreEqual(2, smoke.GetProgramVariable("discardEvaluated"));
        Assert.AreEqual(3, smoke.GetProgramVariable("discardLambda"));

        // identifiers spelled with unicode escapes
        smoke.RunProgram("EscapedIdentifiers");
        Assert.AreEqual(2, smoke.GetProgramVariable("escapedA"));
        Assert.AreEqual(3, smoke.GetProgramVariable("escapedAb"));
        Assert.AreEqual(4, smoke.GetProgramVariable("escapedKeyword"));

        // a class nested in the behaviour is instantiated and called like any other
        smoke.RunProgram("NestedType");
        Assert.AreEqual(42, smoke.GetProgramVariable("nestedValue"));
        Assert.AreEqual(84, smoke.GetProgramVariable("nestedDoubled"));

        // a VRCUrl[] field initializer is baked from the proxy
        smoke.RunProgram("UrlList");
        Assert.AreEqual("https://example.com/3", smoke.GetProgramVariable("urlThird"));

        // a LINQ field initializer needs the interface dispatchers it calls
        smoke.RunProgram("LinqInitializer");
        Assert.AreEqual(40, smoke.GetProgramVariable("linqThird"));

        // a ulong literal past long.MaxValue
        smoke.RunProgram("ULongLiteral");
        Assert.AreEqual("ulong value: 9223372036854775808", smoke.GetProgramVariable("ulongText"));
        Assert.AreEqual(9223372036854775808UL, smoke.GetProgramVariable("ulongValue"));

        // two-pass exception handling: filter before the inner finally
        smoke.RunProgram("ExceptionOrder");
        Assert.AreEqual("Filter,Finally,Catch", smoke.GetProgramVariable("exceptionOrder"));

        // non-short-circuit bool operators
        smoke.RunProgram("BoolOperators");
        Assert.AreEqual(1, smoke.GetProgramVariable("boolCalls"));
        Assert.AreEqual(true, smoke.GetProgramVariable("boolOrCall"));
        Assert.AreEqual(false, smoke.GetProgramVariable("boolAnd"));
        Assert.AreEqual(true, smoke.GetProgramVariable("boolXor"));
        Assert.AreEqual(false, smoke.GetProgramVariable("boolCompound"));

        // enums keep to their underlying type
        smoke.RunProgram("EnumUnderlying");
        Assert.AreEqual(true, smoke.GetProgramVariable("enumByteWraps"));
        Assert.AreEqual(44, smoke.GetProgramVariable("enumByteCast"));
        Assert.AreEqual(true, smoke.GetProgramVariable("enumULongBig"));
        Assert.AreEqual("Big", smoke.GetProgramVariable("enumULongName"));

        // struct methods on values work on a copy; on variables in place
        smoke.RunProgram("DefensiveCopies");
        Assert.AreEqual(3, smoke.GetProgramVariable("copyReadonly"));
        Assert.AreEqual(4, smoke.GetProgramVariable("copyPlain"));
        Assert.AreEqual(4, smoke.GetProgramVariable("copyElement"));
        Assert.AreEqual(3, smoke.GetProgramVariable("copyIn"));

        // small integral types: compound ops compute in int, cast back, wrap
        smoke.RunProgram("IntegerPromotion");
        var promoted = (object[])smoke.GetProgramVariable("promotedIntegers");
        Assert.AreEqual(25, promoted.Length);
        for (int i = 0; i < promoted.Length; i++)
        {
            Assert.IsInstanceOf<int>(promoted[i], "promotion index " + i);
            int expected = i < 5 ? 13 : i < 10 ? -13 : i < 15 ? -14 : i < 20 ? 0 : i == 24 ? 26 : 106496;
            Assert.AreEqual(expected, promoted[i], "promotion index " + i);
        }
        Assert.AreEqual(true, smoke.GetProgramVariable("signedUnsignedComparison"));
        Assert.AreEqual(13.5, smoke.GetProgramVariable("charFloating"));

        smoke.RunProgram("WideIntegers");
        CollectionAssert.AreEqual(new object[] { 0u, -2L, 5UL, 0UL },
            (object[])smoke.GetProgramVariable("wideIntegerResults"));
        Assert.AreEqual(1, smoke.GetProgramVariable("remainderZeroCaught"));
        Assert.AreEqual(1, smoke.GetProgramVariable("remainderOverflowCaught"));

        smoke.RunProgram("SmallIntegerConstants");
        var limits = (object[])smoke.GetProgramVariable("smallLimits");
        var limitsAfter = (object[])smoke.GetProgramVariable("smallLimitsAfter");
        object[] expectedLimits = { sbyte.MaxValue, byte.MaxValue, short.MaxValue, ushort.MaxValue };
        object[] expectedAfter = { sbyte.MinValue, (byte)0, short.MinValue, (ushort)0 };
        for (int i = 0; i < limits.Length; i++)
        {
            Assert.AreEqual(expectedLimits[i].GetType(), limits[i].GetType());
            Assert.AreEqual(expectedLimits[i], limits[i]);
            Assert.AreEqual(expectedAfter[i].GetType(), limitsAfter[i].GetType());
            Assert.AreEqual(expectedAfter[i], limitsAfter[i]);
        }

        smoke.RunProgram("SmallIntegers");
        Assert.AreEqual(3, smoke.GetProgramVariable("smallByte"));
        Assert.AreEqual(1, smoke.GetProgramVariable("smallByteWrapped"));
        Assert.AreEqual(2, smoke.GetProgramVariable("smallShort"));
        Assert.AreEqual(-32768, smoke.GetProgramVariable("smallShortWrapped"));
        Assert.AreEqual(-128, smoke.GetProgramVariable("smallSByte"));
        Assert.AreEqual(0, smoke.GetProgramVariable("smallUShort"));
        Assert.AreEqual("B", smoke.GetProgramVariable("smallChar"));

        // decimal literals are decimals, exactly
        smoke.RunProgram("Decimals");
        Assert.AreEqual(true, smoke.GetProgramVariable("decimalIsDecimal"));
        Assert.AreEqual(true, smoke.GetProgramVariable("decimalExact"));
        Assert.AreEqual("0.3", smoke.GetProgramVariable("decimalSum"));

        // a library in its own auto-referenced asmdef is read as a library
        smoke.RunProgram("UseToolbox");
        Assert.AreEqual(42, smoke.GetProgramVariable("toolboxTwice"));

        // the inspector string arrived; the fake-null TextAsset is null
        smoke.RunProgram("ReadText");
        Assert.AreEqual("hello", smoke.GetProgramVariable("readText"));
        Assert.AreEqual(true, smoke.GetProgramVariable("textAssetMissing"));

        // `[JsonIgnore]` on an unsupported type compiles; reading one throws
        smoke.RunProgram("JsonIgnored");
        Assert.AreEqual(42, smoke.GetProgramVariable("jsonIgnoredValue"));
        StringAssert.Contains("Vector3", (string)smoke.GetProgramVariable("jsonUnsupportedMessage"));

        // an engine enum parsed from JSON is usable by an extern
        smoke.RunProgram("JsonEnum");
        Assert.AreEqual(true, smoke.GetProgramVariable("jsonEnumEquals"));
        Assert.AreEqual("{\"mode\":5}", smoke.GetProgramVariable("jsonEnumBack"));

        // a user enum transferred from the inspector reaches the heap as Int32
        smoke.RunProgram("CheckMode");
        Assert.AreEqual(true, smoke.GetProgramVariable("modeIsSecond"));
        Assert.AreEqual(2, smoke.GetProgramVariable("modesSecondCount"));

        smoke.RunProgram("GenericStatics");
        Assert.AreEqual(1, smoke.GetProgramVariable("genericInt"));
        Assert.AreEqual(10, smoke.GetProgramVariable("genericString"));
        // constant-literal initializer, one per closed type: <int> seeded 7 → 8,
        // <string> its own 7
        Assert.AreEqual(8, smoke.GetProgramVariable("genericSeededInt"));
        Assert.AreEqual(7, smoke.GetProgramVariable("genericSeededString"));
        smoke.RunProgram("BumpGeneric");
        second.RunProgram("BumpGeneric");
        smoke.RunProgram("BumpGeneric");
        // GenericStatics already made GenericCache<int>.Count 1; three bumps → 4
        Assert.AreEqual(4, smoke.GetProgramVariable("genericMine"));
        Assert.AreEqual(3, second.GetProgramVariable("genericMine"));

        // a static event: subscribed in one instance, raised from another —
        // the handler runs in the instance that made it
        smoke.RunProgram("Subscribe");
        second.SetProgramVariable("toPublish", 4);
        second.RunProgram("Publish");
        Assert.AreEqual(40, smoke.GetProgramVariable("heard"));
        Assert.AreEqual(0, second.GetProgramVariable("heard"));

        // a field shadowed across base and derived: two distinct heap slots,
        // each proxy-baked from its own class's field — not both from the
        // most-derived one (issue: base read the derived URL)
        UdonBehaviour shadow = FindUdon("VerifyShadowedUrl");
        shadow.RunProgram("Run");
        Assert.AreEqual("https://example.com/base", shadow.GetProgramVariable("baseUrl"));
        Assert.AreEqual("https://example.com/derived", shadow.GetProgramVariable("derivedUrl"));

        yield return new ExitPlayMode();
        AssetDatabase.DeleteAsset(GeneratedScene);
    }

    [UnityTearDown]
    public IEnumerator LeavePlayModeAfterAFailure()
    {
        if (Application.isPlaying)
        {
            yield return new ExitPlayMode();
        }
        AssetDatabase.DeleteAsset(GeneratedScene);
    }

    private static void BuildRuntimeScene()
    {
        MenSharpTestScene.EnsureFolder("Assets/Tests/Generated");
        var scene = EditorSceneManager.NewScene(NewSceneSetup.EmptyScene, NewSceneMode.Single);

        GameObject smokeObject = MenSharpTestScene.AddProxy("MenSharpRuntimeSmoke", "MenSharpRuntimeSmoke", Vector3.zero);
        MenSharpTestScene.Assign(
            MenSharpTestScene.Proxy(smokeObject, "MenSharpRuntimeSmoke"),
            "items",
            new int[] { 42 });
        // a user enum and an array of it, as the inspector would set them
        {
            Type modeType = MenSharpTestScene.FindType("InspectorEnumMode");
            Component smokeProxy = MenSharpTestScene.Proxy(smokeObject, "MenSharpRuntimeSmoke");
            MenSharpTestScene.Assign(smokeProxy, "mode", Enum.ToObject(modeType, 1));
            Array modes = Array.CreateInstance(modeType, 3);
            modes.SetValue(Enum.ToObject(modeType, 0), 0);
            modes.SetValue(Enum.ToObject(modeType, 1), 1);
            modes.SetValue(Enum.ToObject(modeType, 1), 2);
            MenSharpTestScene.Assign(smokeProxy, "modes", modes);
        }
        // a string set in the inspector, next to an object reference that is
        // a "fake null" (a destroyed asset, as an unassigned field is after
        // deserialization): the transfer must survive it
        {
            Component smokeProxy = MenSharpTestScene.Proxy(smokeObject, "MenSharpRuntimeSmoke");
            MenSharpTestScene.Assign(smokeProxy, "text", "hello");
            var dead = new TextAsset("gone");
            UnityEngine.Object.DestroyImmediate(dead);
            MenSharpTestScene.Assign(smokeProxy, "textAsset", dead);
        }
        GameObject secondSmoke = MenSharpTestScene.AddProxy("MenSharpRuntimeSmoke2", "MenSharpRuntimeSmoke", Vector3.up * 4);
        GameObject targetObject = MenSharpTestScene.AddProxy("MenSharpRuntimeTarget", "MenSharpRuntimeTarget", Vector3.right * 4);
        GameObject callerObject = MenSharpTestScene.AddProxy("MenSharpRuntimeCaller", "MenSharpRuntimeCaller", Vector3.right * 8);
        MenSharpTestScene.Assign(
            MenSharpTestScene.Proxy(callerObject, "MenSharpRuntimeCaller"),
            "target",
            MenSharpTestScene.Proxy(targetObject, "MenSharpRuntimeTarget"));
        GameObject shadowObject = MenSharpTestScene.AddProxy("VerifyShadowedUrl", "VerifyShadowedUrl", Vector3.right * 12);

        var targets = new List<GameObject> { smokeObject, secondSmoke, targetObject, callerObject, shadowObject };
        MenSharpProxy.SyncThenTransfer(targets, false);
        foreach (GameObject target in targets)
        {
            Assert.IsNotNull(target.GetComponent<UdonBehaviour>(), target.name);
        }

        Assert.IsTrue(EditorSceneManager.SaveScene(scene, GeneratedScene));
    }

    private static UdonBehaviour FindUdon(string objectName) => MenSharpTestScene.FindUdon(objectName);
}
#endif

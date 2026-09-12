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

        yield return new EnterPlayMode();
        yield return null;
        yield return null;
        Assert.IsTrue(Application.isPlaying, "the editor did not enter play mode");

        UdonBehaviour smoke = FindUdon("MenSharpRuntimeSmoke");
        Assert.IsTrue(smoke.IsInitialized, "the SDK did not initialise the smoke UdonBehaviour");
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

        // a static field is one for every instance (issue: each instance
        // counted from 0), through the holder the scene carries
        Assert.IsNotNull(GameObject.Find(MenSharpProxy.StaticsHolderName), "no statics holder in the scene");
        UdonBehaviour second = FindUdon("MenSharpRuntimeSmoke2");
        smoke.RunProgram("RunShared");
        second.RunProgram("RunShared");
        smoke.RunProgram("RunShared");
        Assert.AreEqual(3, smoke.GetProgramVariable("mine"));
        Assert.AreEqual(2, second.GetProgramVariable("mine"));

        // a static event: subscribed in one instance, raised from another —
        // the handler runs in the instance that made it
        smoke.RunProgram("Subscribe");
        second.SetProgramVariable("toPublish", 4);
        second.RunProgram("Publish");
        Assert.AreEqual(40, smoke.GetProgramVariable("heard"));
        Assert.AreEqual(0, second.GetProgramVariable("heard"));

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
        GameObject secondSmoke = MenSharpTestScene.AddProxy("MenSharpRuntimeSmoke2", "MenSharpRuntimeSmoke", Vector3.up * 4);
        GameObject targetObject = MenSharpTestScene.AddProxy("MenSharpRuntimeTarget", "MenSharpRuntimeTarget", Vector3.right * 4);
        GameObject callerObject = MenSharpTestScene.AddProxy("MenSharpRuntimeCaller", "MenSharpRuntimeCaller", Vector3.right * 8);
        MenSharpTestScene.Assign(
            MenSharpTestScene.Proxy(callerObject, "MenSharpRuntimeCaller"),
            "target",
            MenSharpTestScene.Proxy(targetObject, "MenSharpRuntimeTarget"));

        var targets = new List<GameObject> { smokeObject, secondSmoke, targetObject, callerObject };
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

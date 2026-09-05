#if UNITY_EDITOR
using System;
using System.Collections;
using System.Collections.Generic;
using System.Linq;
using System.Reflection;
using NUnit.Framework;
using UnityEditor;
using UnityEditor.SceneManagement;
using UnityEditorInternal;
using UnityEngine;
using UnityEngine.TestTools;
using VRC.Udon;

public class MenSharpIntegrationTests
{
    private const string GeneratedScene = "Assets/Tests/Generated/MenSharpRuntime.unity";

    private static readonly string[] ExpectedPrograms =
    {
        "Door",
        "MenSharpRuntimeCaller",
        "MenSharpRuntimeSmoke",
        "MenSharpRuntimeTarget",
        "RecordVerify.VerifyRecords",
        "Test1",
        "UnionVerify.VerifyUnion",
        "Verify",
        "VerifyArrays",
        "VerifyAsync",
        "VerifyCrash",
        "VerifyData",
        "VerifyDeconstruct",
        "VerifyDelegates",
        "VerifyEnumNames",
        "VerifyExternCrash",
        "VerifyFormat",
        "VerifyGetComponent",
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
        "VerifyRemoteAwait",
        "VerifyRemoteDoor",
        "VerifySequences",
        "VerifyString",
        "VerifyTarget",
        "VerifyTuple",
        "VerifyUdonSharp",
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
        if (ScriptReloadPending())
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

    // the check the test framework's own WaitForDomainReload loops on; it
    // is internal to the editor, hence the reflection
    private static readonly MethodInfo ScriptReloadRequested = typeof(InternalEditorUtility)
        .GetMethod("IsScriptReloadRequested", BindingFlags.Static | BindingFlags.NonPublic | BindingFlags.Public);

    private static bool ScriptReloadPending()
    {
        if (EditorApplication.isCompiling)
        {
            return true;
        }
        return ScriptReloadRequested != null && (bool)ScriptReloadRequested.Invoke(null, null);
    }

    private static void BuildRuntimeScene()
    {
        EnsureFolder("Assets/Tests/Generated");
        var scene = EditorSceneManager.NewScene(NewSceneSetup.EmptyScene, NewSceneMode.Single);

        GameObject smokeObject = AddProxy("MenSharpRuntimeSmoke");
        GameObject targetObject = AddProxy("MenSharpRuntimeTarget");
        GameObject callerObject = AddProxy("MenSharpRuntimeCaller");

        var targetProxy = targetObject.GetComponent(FindType("MenSharpRuntimeTarget"));
        var callerProxy = callerObject.GetComponent(FindType("MenSharpRuntimeCaller"));
        FieldInfo targetField = callerProxy.GetType().GetField("target");
        Assert.IsNotNull(targetField);
        targetField.SetValue(callerProxy, targetProxy);

        var targets = new List<GameObject> { smokeObject, targetObject, callerObject };
        MenSharpProxy.SyncThenTransfer(targets, false);
        foreach (GameObject target in targets)
        {
            Assert.IsNotNull(target.GetComponent<UdonBehaviour>(), target.name);
        }

        Assert.IsTrue(EditorSceneManager.SaveScene(scene, GeneratedScene));
    }

    private static GameObject AddProxy(string typeName)
    {
        var target = new GameObject(typeName);
        Type type = FindType(typeName);
        Assert.IsTrue(typeof(MenSharp.MenSharpBehaviour).IsAssignableFrom(type), typeName);
        Assert.IsNotNull(target.AddComponent(type));
        return target;
    }

    private static Type FindType(string fullName)
    {
        Type found = AppDomain.CurrentDomain.GetAssemblies()
            .Select(assembly => assembly.GetType(fullName, false))
            .FirstOrDefault(type => type != null);
        Assert.IsNotNull(found, $"no loaded type named {fullName}");
        return found;
    }

    private static UdonBehaviour FindUdon(string objectName)
    {
        GameObject target = GameObject.Find(objectName);
        Assert.IsNotNull(target, objectName);
        UdonBehaviour udon = target.GetComponent<UdonBehaviour>();
        Assert.IsNotNull(udon, objectName);
        return udon;
    }

    private static void EnsureFolder(string path)
    {
        string current = "Assets";
        foreach (string segment in path.Split('/').Skip(1))
        {
            string next = current + "/" + segment;
            if (!AssetDatabase.IsValidFolder(next))
            {
                AssetDatabase.CreateFolder(current, segment);
            }
            current = next;
        }
    }
}
#endif

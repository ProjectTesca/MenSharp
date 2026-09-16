#if UNITY_EDITOR
using System.Collections;
using System.Collections.Generic;
using NUnit.Framework;
using UnityEditor;
using UnityEditor.SceneManagement;
using UnityEngine;
using UnityEngine.TestTools;
using VRC.Udon;

public class MenSharpCancellationTokenTests
{
    private const string GeneratedScene = "Assets/Tests/Generated/MenSharpCancellationToken.unity";

    [UnityTest]
    public IEnumerator DestroyCancellationTokenIsLazyAndCanceledBeforeOnDestroy()
    {
        AssetDatabase.Refresh();
        if (MenSharpTestScene.ScriptReloadPending())
        {
            yield return new WaitForDomainReload();
        }

        MenSharpCompiler.RebuildAll();
        AssetDatabase.Refresh(ImportAssetOptions.ForceSynchronousImport);
        BuildScene();

        yield return new EnterPlayMode();
        yield return null;
        yield return null;
        Assert.IsTrue(Application.isPlaying, "the editor did not enter play mode");

        UdonBehaviour cached = MenSharpTestScene.FindUdon("MenSharpCancellationCached");
        UdonBehaviour uncached = MenSharpTestScene.FindUdon("MenSharpCancellationUncached");
        Assert.IsTrue(cached.IsInitialized, "the cached behaviour was not initialised");
        Assert.IsTrue(uncached.IsInitialized, "the uncached behaviour was not initialised");

        // Start creates the source only for the behaviour that caches its token.
        cached.RunProgram("Start");
        uncached.RunProgram("Start");
        Assert.AreEqual(true, cached.GetProgramVariable("destroyTokenCached"));
        Assert.AreEqual(false, uncached.GetProgramVariable("destroyTokenCached"));

        // The generated pre-hook cancels before the user OnDestroy handler.
        cached.RunProgram("_onDestroy");
        Assert.AreEqual(true, cached.GetProgramVariable("destroyTokenCanceled"),
            "the cached token was not canceled before OnDestroy");
        uncached.RunProgram("_onDestroy");
        Assert.AreEqual(false, uncached.GetProgramVariable("destroyTokenCanceled"));

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

    private static void BuildScene()
    {
        MenSharpTestScene.EnsureFolder("Assets/Tests/Generated");
        var scene = EditorSceneManager.NewScene(NewSceneSetup.EmptyScene, NewSceneMode.Single);

        GameObject cached = MenSharpTestScene.AddProxy(
            "MenSharpCancellationCached", "MenSharpRuntimeSmoke", Vector3.zero);
        MenSharpTestScene.Assign(
            MenSharpTestScene.Proxy(cached, "MenSharpRuntimeSmoke"),
            "cacheDestroyToken",
            true);

        GameObject uncached = MenSharpTestScene.AddProxy(
            "MenSharpCancellationUncached", "MenSharpRuntimeSmoke", Vector3.up * 4);
        MenSharpTestScene.Assign(
            MenSharpTestScene.Proxy(uncached, "MenSharpRuntimeSmoke"),
            "cacheDestroyToken",
            false);

        var targets = new List<GameObject> { cached, uncached };
        MenSharpProxy.SyncThenTransfer(targets, false);
        foreach (GameObject target in targets)
        {
            Assert.IsNotNull(target.GetComponent<UdonBehaviour>(), target.name);
        }

        AssetDatabase.DeleteAsset(GeneratedScene);
        Assert.IsTrue(EditorSceneManager.SaveScene(scene, GeneratedScene));
    }
}
#endif

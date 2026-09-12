#if UNITY_EDITOR
using System;
using System.Linq;
using System.Reflection;
using NUnit.Framework;
using UnityEditor;
using UnityEditor.SceneManagement;
using UnityEngine;
using VRC.Udon;

/// The pairing under undo: a proxy and its backing UdonBehaviour must come
/// and go together, in the user's own undo step — never in a step of the
/// sweep's own, which Ctrl+Z would peel off one layer at a time.
public class MenSharpUndoTests
{
    private const string Proxy = "MenSharpRuntimeSmoke";

    private static readonly MethodInfo ScheduleSweep = typeof(MenSharpProxy)
        .GetMethod("ScheduleSweep", BindingFlags.Static | BindingFlags.NonPublic);
    private static readonly MethodInfo RunPendingSweep = typeof(MenSharpProxy)
        .GetMethod("RunPendingSweep", BindingFlags.Static | BindingFlags.NonPublic);

    private GameObject target;

    [SetUp]
    public void SetUp()
    {
        MenSharpCompiler.CompileAll();
        AssetDatabase.Refresh(ImportAssetOptions.ForceSynchronousImport);
        EditorSceneManager.NewScene(NewSceneSetup.EmptyScene, NewSceneMode.Single);
        Undo.ClearAll();
        target = ObjectFactory.CreateGameObject("Thing");
        Undo.IncrementCurrentGroup();
    }

    /// What the inspector's Add Component button does.
    [Test]
    public void UndoingAnAddedProxyRemovesItsBackingBehaviourWithIt()
    {
        ObjectFactory.AddComponent(target, MenSharpTestScene.FindType(Proxy));
        AssertPaired();

        Undo.PerformUndo();
        Sweep();
        AssertBare();

        Undo.PerformRedo();
        Sweep();
        AssertPaired();

        // and the step stays one step: no sweep of its own to peel off
        Undo.PerformUndo();
        Sweep();
        AssertBare();
    }

    /// A proxy added without ObjectFactory (a script dropped onto the
    /// GameObject) is paired by the sweep, inside the same undo step.
    [Test]
    public void ASweptPairingJoinsTheUndoStepThatAddedTheProxy()
    {
        Undo.AddComponent(target, MenSharpTestScene.FindType(Proxy));
        Assert.IsNull(target.GetComponent<UdonBehaviour>());
        Sweep();
        AssertPaired();

        Undo.PerformUndo();
        Sweep();
        AssertBare();

        Undo.PerformRedo();
        Sweep();
        AssertPaired();
    }

    [Test]
    public void RemovingAProxyRemovesItsBackingBehaviourInTheSameUndoStep()
    {
        ObjectFactory.AddComponent(target, MenSharpTestScene.FindType(Proxy));
        Undo.IncrementCurrentGroup();
        int udon = target.GetComponent<UdonBehaviour>().GetInstanceID();

        Undo.DestroyObjectImmediate(target.GetComponent(MenSharpTestScene.FindType(Proxy)));
        Sweep();
        AssertBare();

        Undo.PerformUndo();
        Sweep();
        AssertPaired();
        Assert.AreEqual(udon, target.GetComponent<UdonBehaviour>().GetInstanceID(),
            "the original UdonBehaviour (and its settings) should be back, not a fresh one");

        Undo.PerformRedo();
        Sweep();
        AssertBare();
    }

    /// The editor's own sweep after a hierarchy change or an undo, run now.
    private static void Sweep()
    {
        ScheduleSweep.Invoke(null, new object[] { false });
        RunPendingSweep.Invoke(null, null);
    }

    private void AssertPaired()
    {
        Component proxy = target.GetComponent(MenSharpTestScene.FindType(Proxy));
        Assert.IsNotNull(proxy, "proxy");
        Assert.AreEqual(HideFlags.None, proxy.hideFlags, "proxy hideFlags");
        UdonBehaviour[] udons = target.GetComponents<UdonBehaviour>();
        Assert.AreEqual(1, udons.Length, "one backing UdonBehaviour");
        Assert.AreEqual(MenSharpProxy.FindProgram(proxy.GetType()), udons[0].programSource);
        Assert.AreEqual(3, target.GetComponents<Component>().Count(c => c != null), "Transform + proxy + Udon");
    }

    private void AssertBare()
    {
        Assert.IsNotNull(target, "the GameObject itself stays");
        string[] left = target.GetComponents<Component>()
            .Select(c => c == null ? "<destroyed>" : c.GetType().Name)
            .ToArray();
        CollectionAssert.AreEqual(new[] { "Transform" }, left);
    }
}
#endif

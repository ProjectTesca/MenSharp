#if UNITY_EDITOR
using System.IO;
using MenSharp;
using NUnit.Framework;
using UnityEditor;
using UnityEditor.SceneManagement;
using UnityEngine;
using VRC.Udon;

/// The pair between a proxy and its backing UdonBehaviour has to survive a
/// trip through a unitypackage: in the receiving project the program asset
/// the UdonBehaviour carried is another object or missing, and the hidden
/// flag may be gone. The proxy remembers its UdonBehaviour, and a program
/// asset's GUID is derived from its class so the reference resolves again.
public class MenSharpPairingTests
{
    private const string Proxy = "MenSharpRuntimeSmoke";

    [SetUp]
    public void SetUp()
    {
        MenSharpCompiler.CompileAll();
        AssetDatabase.Refresh(ImportAssetOptions.ForceSynchronousImport);
        EditorSceneManager.NewScene(NewSceneSetup.EmptyScene, NewSceneMode.Single);
    }

    private static (MenSharpBehaviour proxy, UdonBehaviour udon) Pair(GameObject target)
    {
        var proxy = (MenSharpBehaviour)target.AddComponent(MenSharpTestScene.FindType(Proxy));
        var pairs = MenSharpProxy.SyncPairs(target, quiet: true, undoable: false);
        Assert.AreEqual(1, pairs.Count);
        Assert.AreSame(proxy, pairs[0].proxy);
        return (proxy, pairs[0].udon);
    }

    /// What an import from another project leaves: the UdonBehaviour is
    /// there, its Program Source is not, and it is not hidden either.
    [Test]
    public void AnImportedBackingBehaviourIsPairedAgainByReference()
    {
        GameObject target = new GameObject("Imported");
        (MenSharpBehaviour proxy, UdonBehaviour udon) = Pair(target);
        MenSharpProgramAsset program = MenSharpProxy.FindProgram(proxy.GetType());
        Assert.AreEqual(program, udon.programSource);

        udon.programSource = null;
        udon.hideFlags = HideFlags.None;

        Assert.AreSame(udon, MenSharpProxy.FindPaired(proxy));
        var pairs = MenSharpProxy.SyncPairs(target, quiet: true, undoable: false);
        Assert.AreEqual(1, pairs.Count);
        Assert.AreSame(udon, pairs[0].udon, "the remembered UdonBehaviour is reused, not replaced");
        Assert.AreEqual(program, udon.programSource);
        Assert.AreEqual(1, target.GetComponents<UdonBehaviour>().Length, "no second UdonBehaviour");
    }

    /// The reference also wins over a stale program of another class's.
    [Test]
    public void ABackingBehaviourCarryingAnotherProgramIsRepointed()
    {
        GameObject target = new GameObject("Swapped");
        (MenSharpBehaviour proxy, UdonBehaviour udon) = Pair(target);
        MenSharpProgramAsset other = MenSharpProxy.FindProgram(MenSharpTestScene.FindType("Door"));
        Assert.IsNotNull(other);
        udon.programSource = other;

        var pairs = MenSharpProxy.SyncPairs(target, quiet: true, undoable: false);
        Assert.AreSame(udon, pairs[0].udon);
        Assert.AreEqual(MenSharpProxy.FindProgram(proxy.GetType()), udon.programSource);
        Assert.AreEqual(1, target.GetComponents<UdonBehaviour>().Length);
    }

    /// A proxy pasted onto another GameObject remembers the original's
    /// UdonBehaviour; that one belongs to the original and is left alone.
    [Test]
    public void APastedProxyDoesNotClaimAnotherObjectsBackingBehaviour()
    {
        GameObject original = new GameObject("Original");
        (MenSharpBehaviour proxy, UdonBehaviour udon) = Pair(original);

        GameObject copy = new GameObject("Copy");
        var pasted = (MenSharpBehaviour)copy.AddComponent(MenSharpTestScene.FindType(Proxy));
        var serialized = new SerializedObject(pasted);
        serialized.FindProperty("menSharpBacking").objectReferenceValue = udon;
        serialized.ApplyModifiedPropertiesWithoutUndo();

        var pairs = MenSharpProxy.SyncPairs(copy, quiet: true, undoable: false);
        Assert.AreEqual(1, pairs.Count);
        Assert.AreNotSame(udon, pairs[0].udon);
        Assert.AreEqual(copy, pairs[0].udon.gameObject);
        Assert.AreSame(udon, MenSharpProxy.FindPaired(proxy));
        Assert.AreSame(pairs[0].udon, MenSharpProxy.FindPaired(pasted));
    }

    /// Two proxies of one class on one object, both remembering the same
    /// UdonBehaviour (a duplicated component): the second gets its own.
    [Test]
    public void ADuplicatedProxyGetsItsOwnBackingBehaviour()
    {
        GameObject target = new GameObject("Twice");
        (MenSharpBehaviour first, UdonBehaviour udon) = Pair(target);
        var second = (MenSharpBehaviour)target.AddComponent(MenSharpTestScene.FindType(Proxy));
        var serialized = new SerializedObject(second);
        serialized.FindProperty("menSharpBacking").objectReferenceValue = udon;
        serialized.ApplyModifiedPropertiesWithoutUndo();

        var pairs = MenSharpProxy.SyncPairs(target, quiet: true, undoable: false);
        Assert.AreEqual(2, pairs.Count);
        Assert.AreSame(udon, pairs[0].udon);
        Assert.AreNotSame(udon, pairs[1].udon);
        Assert.AreEqual(2, target.GetComponents<UdonBehaviour>().Length);
        Assert.AreSame(pairs[1].udon, MenSharpProxy.FindPaired(second));
    }

    /// A program asset created from scratch gets the GUID its class implies.
    [Test]
    public void ANewProgramAssetGetsTheGuidDerivedFromItsClass()
    {
        string uasm = Path.Combine("Library", "MenSharp", "Door.uasm");
        string meta = Path.Combine("Library", "MenSharp", "Door.meta.json");
        Assert.IsTrue(File.Exists(uasm), uasm);
        string folder = "Assets/MenSharp/Programs/GuidProbe";
        MenSharpTestScene.EnsureFolder(folder);
        string assetPath = folder + "/GuidProbe.Door.asset";
        AssetDatabase.DeleteAsset(assetPath);
        try
        {
            MenSharpProgramAsset asset = MenSharpImporter.CreateOrUpdate(uasm, meta, assetPath, false, out _);
            Assert.IsNotNull(asset);
            string expected = MenSharpImporter.StableGuid("GuidProbe.Door");
            Assert.AreEqual(expected, AssetDatabase.AssetPathToGUID(assetPath));
            Assert.AreEqual(asset, AssetDatabase.LoadAssetAtPath<MenSharpProgramAsset>(assetPath));
            Assert.IsNotNull(asset.SerializedProgramAsset, "the serialized program is made after the GUID is settled");
            // and an update keeps it
            MenSharpImporter.CreateOrUpdate(uasm, meta, assetPath, true, out _);
            Assert.AreEqual(expected, AssetDatabase.AssetPathToGUID(assetPath));
        }
        finally
        {
            var made = AssetDatabase.LoadAssetAtPath<MenSharpProgramAsset>(assetPath);
            string serializedPath = made != null && made.SerializedProgramAsset != null
                ? AssetDatabase.GetAssetPath(made.SerializedProgramAsset)
                : null;
            AssetDatabase.DeleteAsset(assetPath);
            if (!string.IsNullOrEmpty(serializedPath))
            {
                AssetDatabase.DeleteAsset(serializedPath);
            }
            AssetDatabase.DeleteAsset(folder);
            MenSharpSources.InvalidateProgramIndex();
        }
    }
}
#endif

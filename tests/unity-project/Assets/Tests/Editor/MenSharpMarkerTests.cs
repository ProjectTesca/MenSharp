#if UNITY_EDITOR
using System.IO;
using System.Linq;
using NUnit.Framework;
using UnityEditor;

/// A `.mensharp` file makes its folder — and every subfolder — MenSharp's,
/// wherever it lives, and each source's programs land under the nearest marked
/// folder to it. The probe files are written straight to disk and removed
/// before anything refreshes the AssetDatabase, so Unity never imports them;
/// MenSharpSources reads them off disk, which is exactly the path under test.
/// (A folder ending in `~` would hide them from Unity too — but MenSharp now
/// skips those folders like Unity does, which the last tests check.)
public class MenSharpMarkerTests
{
    private const string Outer = "Assets/MenSharpMarkerProbeOuter";
    private const string Inner = Outer + "/Gadget";
    private const string Hidden = Outer + "/Tests~";
    private const string HiddenInRoot = MenSharpSources.SourceRoot + "/MarkerProbe~";
    private const string HiddenLibrary = "Assets/MenSharpLibraryProbe~";
    private const string HiddenMarked = "Assets/MenSharpHiddenMarkedProbe~";

    [SetUp]
    public void SetUp()
    {
        Directory.CreateDirectory(Inner);
        File.WriteAllText(Outer + "/" + MenSharpMarker.MarkerFileName, "# test\n");
        File.WriteAllText(Outer + "/Outer.cs", "// dummy\n");
        File.WriteAllText(Inner + "/Inner.cs", "// dummy\n");
    }

    [TearDown]
    public void TearDown()
    {
        foreach (string folder in new[] { Outer, HiddenInRoot, HiddenLibrary, HiddenMarked })
        {
            if (Directory.Exists(folder))
            {
                Directory.Delete(folder, true);
            }
        }
        // a compile during a test may have registered the probe folder in
        // UdonSharp's blacklist; take it back out so the tracked settings
        // asset does not accumulate a prefix for a folder that no longer exists
        MenSharpUdonSharpIsolation.StopIgnoring(Outer);
        MenSharpUdonSharpIsolation.StopIgnoring(Inner);
    }

    [Test]
    public void AMarkedFolderIsAMenSharpSourceRootWhereverItLives()
    {
        Assert.IsTrue(MenSharpMarker.MarkedRoots().Contains(Outer));
        Assert.IsTrue(MenSharpSources.IsMenSharpSource(Outer + "/Outer.cs"));
    }

    [Test]
    public void SubfoldersOfAMarkedFolderAreIncludedToo()
    {
        Assert.IsTrue(MenSharpSources.IsMenSharpSource(Inner + "/Inner.cs"));
        Assert.AreEqual(Outer, MenSharpMarker.MarkedRootOf(Inner + "/Inner.cs"));
    }

    [Test]
    public void CollectClaimsMarkedSourcesWithProgramsUnderTheirRoot()
    {
        MenSharpSources.SourceSet set = MenSharpSources.Collect();
        // the source is listed as MenSharp
        Assert.IsTrue(
            set.MenSharp.Any(p => MenSharpSources.Normalize(p).EndsWith("/Outer.cs")),
            "Outer.cs should be a MenSharp source");
        // and never doubled up as a library
        Assert.IsFalse(
            set.Library.Any(p => MenSharpSources.Normalize(p).EndsWith("/Outer.cs")),
            "a marked source must not also be a library");
        string openable = set.MenSharp.First(p => MenSharpSources.Normalize(p).EndsWith("/Outer.cs"));
        Assert.AreEqual(Outer + "/Programs", set.ProgramsFolderOf[openable]);
    }

    [Test]
    public void ANestedMarkerOwnsItsOwnSubtree()
    {
        File.WriteAllText(Inner + "/" + MenSharpMarker.MarkerFileName, "# test\n");
        try
        {
            Assert.AreEqual(Inner, MenSharpMarker.MarkedRootOf(Inner + "/Inner.cs"));
            MenSharpSources.SourceSet set = MenSharpSources.Collect();
            string openable = set.MenSharp.First(p => MenSharpSources.Normalize(p).EndsWith("/Inner.cs"));
            Assert.AreEqual(Inner + "/Programs", set.ProgramsFolderOf[openable]);
        }
        finally
        {
            File.Delete(Inner + "/" + MenSharpMarker.MarkerFileName);
        }
    }

    /// `Tests~`, `Samples~`, `.hidden`: Unity imports nothing under them, so
    /// MenSharp compiles nothing from there either — not in a marked folder,
    /// not in Assets/MenSharp, not as a library — and a marker inside one
    /// marks nothing.
    [Test]
    public void FoldersUnityIgnoresAreNotCollected()
    {
        Directory.CreateDirectory(Hidden);
        File.WriteAllText(Hidden + "/InMarked.cs", "// dummy\n");
        Directory.CreateDirectory(HiddenInRoot);
        File.WriteAllText(HiddenInRoot + "/InRoot.cs", "// dummy\n");
        Directory.CreateDirectory(HiddenLibrary);
        File.WriteAllText(HiddenLibrary + "/InLibrary.cs", "// dummy\n");
        Directory.CreateDirectory(HiddenMarked);
        File.WriteAllText(HiddenMarked + "/" + MenSharpMarker.MarkerFileName, "# test\n");
        File.WriteAllText(HiddenMarked + "/UnderHiddenMarker.cs", "// dummy\n");

        MenSharpSources.SourceSet set = MenSharpSources.Collect();
        foreach (string name in new[] { "InMarked.cs", "InRoot.cs", "InLibrary.cs", "UnderHiddenMarker.cs" })
        {
            Assert.IsFalse(
                set.MenSharp.Any(p => MenSharpSources.Normalize(p).EndsWith("/" + name)),
                name + " is in a folder Unity ignores: no MenSharp source");
            Assert.IsFalse(
                set.Library.Any(p => MenSharpSources.Normalize(p).EndsWith("/" + name)),
                name + " is in a folder Unity ignores: no library source");
        }
        // the visible sources beside them are still collected
        Assert.IsTrue(set.MenSharp.Any(p => MenSharpSources.Normalize(p).EndsWith("/Outer.cs")));

        Assert.IsFalse(MenSharpSources.IsMenSharpSource(Hidden + "/InMarked.cs"));
        Assert.IsFalse(MenSharpSources.IsMenSharpSource(HiddenInRoot + "/InRoot.cs"));
        Assert.IsFalse(MenSharpSources.IsLibrarySource(HiddenLibrary + "/InLibrary.cs"));
        Assert.IsFalse(MenSharpMarker.MarkedRoots().Contains(HiddenMarked));
    }

    [Test]
    public void TheIgnoreRuleIsUnitys()
    {
        Assert.IsTrue(MenSharpSources.IsUnityIgnored("Assets/Foo/Tests~/A.cs"));
        Assert.IsTrue(MenSharpSources.IsUnityIgnored("Assets/Foo~"));
        Assert.IsTrue(MenSharpSources.IsUnityIgnored("Assets/.hidden/A.cs"));
        Assert.IsTrue(MenSharpSources.IsUnityIgnored("Assets/cvs/A.cs"));
        Assert.IsTrue(MenSharpSources.IsUnityIgnored("Packages/com.example.lib/Samples~/A.cs"));
        Assert.IsTrue(MenSharpSources.IsUnityIgnored("Assets\\Foo\\Tests~\\A.cs"));
        Assert.IsFalse(MenSharpSources.IsUnityIgnored("Assets/Foo/A~b/A.cs"));
        Assert.IsFalse(MenSharpSources.IsUnityIgnored("Assets/MenSharp/A.cs"));
        // the package folder itself is not judged: only what is inside it
        Assert.IsFalse(MenSharpSources.IsUnityIgnored("Packages/.local.pkg/Runtime/A.cs"));
    }
}

/// End to end: the committed Assets/Gadget fixture is a marked folder with a
/// real behaviour, and its program must compile into its own Programs — never
/// into Assets/MenSharp/Programs. A fixture of its own: it refreshes the
/// AssetDatabase, which must not happen while the probe files above exist.
public class MenSharpMarkerCompileTests
{
    [Test]
    public void ABehaviourInAMarkedFolderCompilesToThatFoldersPrograms()
    {
        MenSharpCompiler.CompileAll();
        AssetDatabase.Refresh(ImportAssetOptions.ForceSynchronousImport);
        var program = AssetDatabase.LoadAssetAtPath<MenSharpProgramAsset>(
            "Assets/Gadget/Programs/GadgetBehaviour.asset");
        Assert.IsNotNull(program, "GadgetBehaviour should compile into Assets/Gadget/Programs");
        Assert.IsNull(
            AssetDatabase.LoadAssetAtPath<MenSharpProgramAsset>(
                "Assets/MenSharp/Programs/GadgetBehaviour.asset"),
            "a marked folder's program must not land in Assets/MenSharp/Programs");
    }
}
#endif

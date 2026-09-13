#if UNITY_EDITOR
using System.IO;
using System.Linq;
using NUnit.Framework;
using UnityEditor;

/// A `.mensharp` file makes its folder — and every subfolder — MenSharp's,
/// wherever it lives, and each source's programs land under the nearest marked
/// folder to it. The probe folders end in `~` so Unity never imports the dummy
/// scripts (they would not compile); MenSharpSources reads them off disk all
/// the same, which is exactly the path under test.
public class MenSharpMarkerTests
{
    private const string Outer = "Assets/MenSharpMarkerProbeOuter~";
    private const string Inner = Outer + "/Gadget";

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
        if (Directory.Exists(Outer))
        {
            Directory.Delete(Outer, true);
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

    /// End to end: the committed Assets/Gadget fixture is a marked folder with
    /// a real behaviour, and its program must compile into its own Programs —
    /// never into Assets/MenSharp/Programs.
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

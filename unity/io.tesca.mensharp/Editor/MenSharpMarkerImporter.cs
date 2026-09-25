// What a marker file is to the editor. Registering an importer for the
// `.mensharp` extension is what makes the marker a real asset — imported, with
// a .meta and a GUID, so a unitypackage or a VPM package carries it — and
// gives the Project window and the Inspector something to say about it.
// Nothing is stored: the marker works by existing (see MenSharpMarker), and
// the object built here (a MenSharpMarkerAsset) lives only in Unity's import
// cache.
#if UNITY_EDITOR
using System.IO;
using UnityEditor;
using UnityEditor.AssetImporters;
using UnityEngine;

[ScriptedImporter(2, "mensharp")]
public sealed class MenSharpMarkerImporter : ScriptedImporter
{
    public override void OnImportAsset(AssetImportContext ctx)
    {
        var asset = ScriptableObject.CreateInstance<MenSharpMarkerAsset>();
        asset.name = Path.GetFileNameWithoutExtension(ctx.assetPath);
        ctx.AddObjectToAsset("marker", asset);
        ctx.SetMainObject(asset);
    }
}

[CustomEditor(typeof(MenSharpMarkerImporter))]
public sealed class MenSharpMarkerImporterEditor : ScriptedImporterEditor
{
    // the marker has no settings to apply or revert
    protected override bool needsApplyRevert => false;

    public override void OnInspectorGUI()
    {
        string folder = MenSharpSources.Normalize(Path.GetDirectoryName(((AssetImporter)target).assetPath));
        EditorGUILayout.HelpBox(
            $"{folder} is a MenSharp source folder: its C# — and every subfolder's — is compiled "
            + "by MenSharp to Udon, and UdonSharp's scanner skips it.\n\n"
            + "This file marks the folder by existing; its text is not read. Delete it, or press "
            + "Unmark, to hand the folder back to UdonSharp / plain C#.",
            MessageType.Info);
        if (GUILayout.Button("Unmark MenSharp Source Folder"))
        {
            // not from inside the inspector of the asset being deleted
            EditorApplication.delayCall += () => MenSharpMarker.Unmark(new[] { folder });
        }
    }
}
#endif

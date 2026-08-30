// MenSharp: turn a compiled `.uasm` + `.meta.json` pair into an Udon program
// asset.
//
// Usage:
//   1. Copy this file AND MenSharpProgramAsset.cs into Assets/Editor/
//      (two files — Unity requires the asset class in its own, same-named file)
//   2. Copy the compiler's output (e.g. greeter.uasm and greeter.meta.json)
//      somewhere inside Assets/
//   3. Menu: MenSharp > Import Udon Program, pick the .uasm file
//   4. Drop the created .asset onto an UdonBehaviour's "Program Source"

#if UNITY_EDITOR
using System.IO;
using System.Reflection;
using UnityEditor;
using UnityEngine;
using VRC.Udon.Editor.ProgramSources;

public static class MenSharpProgramImporter
{
    [MenuItem("MenSharp/Import Udon Program")]
    public static void Import()
    {
        string uasmPath = EditorUtility.OpenFilePanel("Pick a .uasm", Application.dataPath, "uasm");
        if (string.IsNullOrEmpty(uasmPath))
        {
            return;
        }
        string metaPath = Path.ChangeExtension(uasmPath, null) + ".meta.json";
        if (!File.Exists(metaPath))
        {
            Debug.LogError($"MenSharp: missing sidecar {metaPath}");
            return;
        }

        var programAsset = ScriptableObject.CreateInstance<MenSharpProgramAsset>();
        programAsset.metaJson = File.ReadAllText(metaPath);
        var assemblyField = typeof(UdonAssemblyProgramAsset).GetField(
            "udonAssembly", BindingFlags.Instance | BindingFlags.NonPublic | BindingFlags.Public);
        assemblyField.SetValue(programAsset, File.ReadAllText(uasmPath));

        // the asset must exist on disk before its serialized program sub-asset
        // can be created next to it
        string assetPath = ToAssetPath(Path.ChangeExtension(uasmPath, ".asset"));
        AssetDatabase.CreateAsset(programAsset, assetPath);

        programAsset.RefreshProgram();
        EditorUtility.SetDirty(programAsset);
        AssetDatabase.SaveAssets();

        var meta = JsonUtility.FromJson<MenSharpMeta>(programAsset.metaJson);
        Debug.Log($"MenSharp: imported {assetPath} ({meta.heap.Length} heap values, entry points: {string.Join(", ", meta.entryPoints)})");
    }

    private static string ToAssetPath(string absolute)
    {
        absolute = absolute.Replace('\\', '/');
        string dataPath = Application.dataPath.Replace('\\', '/');
        if (absolute.StartsWith(dataPath))
        {
            return "Assets" + absolute.Substring(dataPath.Length);
        }
        return "Assets/" + Path.GetFileName(absolute);
    }
}
#endif

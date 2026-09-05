// MenSharp: create or update program assets from compiled `.uasm` +
// `.meta.json` pairs.
//
// `CreateOrUpdate` is the path the compile button takes: when the asset
// already exists it is updated **in place**, keeping its GUID, so every
// UdonBehaviour in every scene that references it keeps working across
// recompiles. The manual menu item remains for importing a hand-picked file.

#if UNITY_EDITOR
using System.IO;
using System.Reflection;
using UnityEditor;
using UnityEngine;
using VRC.Udon.Editor.ProgramSources;

public static class MenSharpImporter
{
    public static MenSharpProgramAsset CreateOrUpdate(
        string uasmPath,
        string metaPath,
        string assetPath)
    {
        return CreateOrUpdate(uasmPath, metaPath, assetPath, true, out _);
    }

    /// `unchanged` comes back true when the asset already holds exactly this
    /// program and was left alone. Re-assembling and re-serializing a
    /// program is what the compile button spends its time on, and a
    /// behaviour whose source did not change compiles to the same text
    /// (the compiler's output is deterministic), so only the ones that
    /// differ are touched — unless `force` asks for all of them.
    public static MenSharpProgramAsset CreateOrUpdate(
        string uasmPath,
        string metaPath,
        string assetPath,
        bool force,
        out bool unchanged)
    {
        unchanged = false;
        string assembly = File.ReadAllText(uasmPath);
        string metaJson = File.ReadAllText(metaPath);

        var programAsset = AssetDatabase.LoadAssetAtPath<MenSharpProgramAsset>(assetPath);
        bool created = programAsset == null;
        var assemblyField = typeof(UdonAssemblyProgramAsset).GetField(
            "udonAssembly", BindingFlags.Instance | BindingFlags.NonPublic | BindingFlags.Public);
        if (!created
            && !force
            && programAsset.SerializedProgramAsset != null
            && programAsset.metaJson == metaJson
            && (string)assemblyField.GetValue(programAsset) == assembly)
        {
            unchanged = true;
            return programAsset;
        }
        if (created)
        {
            programAsset = ScriptableObject.CreateInstance<MenSharpProgramAsset>();
        }

        programAsset.metaJson = metaJson;
        assemblyField.SetValue(programAsset, assembly);

        if (created)
        {
            // the asset must exist on disk before its serialized program
            // sub-asset can be created next to it
            AssetDatabase.CreateAsset(programAsset, assetPath);
        }
        programAsset.RefreshProgram();
        EditorUtility.SetDirty(programAsset);
        return programAsset;
    }

    [MenuItem("MenSharp/Import Udon Program (manual)")]
    public static void ImportManually()
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
        string assetPath = ToAssetPath(Path.ChangeExtension(uasmPath, ".asset"));
        CreateOrUpdate(uasmPath, metaPath, assetPath);
        AssetDatabase.SaveAssets();
        Debug.Log($"MenSharp: imported {assetPath}");
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

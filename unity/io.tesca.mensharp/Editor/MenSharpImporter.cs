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
using System.Security.Cryptography;
using System.Text;
using System.Text.RegularExpressions;
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
        var watch = System.Diagnostics.Stopwatch.StartNew();
        string assembly = File.ReadAllText(uasmPath);
        string metaJson = File.ReadAllText(metaPath);
        // the binary program beside them, when the compiler wrote one
        string blobPath = Path.ChangeExtension(uasmPath, ".uprog");
        byte[] blob = File.Exists(blobPath) ? File.ReadAllBytes(blobPath) : null;

        var programAsset = AssetDatabase.LoadAssetAtPath<MenSharpProgramAsset>(assetPath);
        LoadMilliseconds += watch.ElapsedMilliseconds;
        watch.Restart();
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
        programAsset.programBlob = blob;
        assemblyField.SetValue(programAsset, assembly);

        if (created)
        {
            // the asset must exist on disk before its serialized program
            // sub-asset can be created next to it
            AssetDatabase.CreateAsset(programAsset, assetPath);
            programAsset = AdoptStableGuid(programAsset, assetPath);
        }
        CreateMilliseconds += watch.ElapsedMilliseconds;
        watch.Restart();
        programAsset.RefreshProgram();
        RefreshMilliseconds += watch.ElapsedMilliseconds;
        watch.Restart();
        EditorUtility.SetDirty(programAsset);
        DirtyMilliseconds += watch.ElapsedMilliseconds;
        return programAsset;
    }

    /// The GUID a class's program asset gets when it is first created: a
    /// function of the class path alone, so that the same class compiles
    /// to the same GUID in every project. A prefab exported from one
    /// project then finds its programs in another as soon as that project
    /// has compiled — its UdonBehaviours' Program Source resolves by
    /// itself instead of reading `None`. (An asset that already exists
    /// keeps whatever GUID it has: changing it would break the references
    /// to it.)
    public static string StableGuid(string classPath)
    {
        using (var md5 = MD5.Create())
        {
            byte[] hash = md5.ComputeHash(Encoding.UTF8.GetBytes("MenSharp/" + classPath));
            var text = new StringBuilder(32);
            foreach (byte value in hash)
            {
                text.Append(value.ToString("x2"));
            }
            return text.ToString();
        }
    }

    /// Rewrites a freshly created asset's `.meta` to the class's stable
    /// GUID and re-imports it. Nothing references the asset yet, so the
    /// change is safe; if some other asset already holds that GUID (one
    /// imported from elsewhere and moved), Unity's own GUID stays.
    private static MenSharpProgramAsset AdoptStableGuid(MenSharpProgramAsset asset, string assetPath)
    {
        string wanted = StableGuid(Path.GetFileNameWithoutExtension(assetPath));
        if (AssetDatabase.AssetPathToGUID(assetPath) == wanted)
        {
            return asset;
        }
        string holder = AssetDatabase.GUIDToAssetPath(wanted);
        if (!string.IsNullOrEmpty(holder) && holder != assetPath)
        {
            Debug.LogWarning(
                $"MenSharp: {assetPath} keeps a project-specific GUID — {holder} already has "
                + "the one derived from its class. Prefabs from other projects referencing this "
                + "program will need pairing again.");
            return asset;
        }
        string metaPath = assetPath + ".meta";
        if (!File.Exists(metaPath))
        {
            return asset;
        }
        string meta = File.ReadAllText(metaPath);
        string rewritten = Regex.Replace(
            meta, @"^guid: [0-9a-fA-F]{32}", "guid: " + wanted, RegexOptions.Multiline);
        if (rewritten == meta)
        {
            return asset;
        }
        File.WriteAllText(metaPath, rewritten);
        AssetDatabase.ImportAsset(assetPath, ImportAssetOptions.ForceSynchronousImport);
        var reloaded = AssetDatabase.LoadAssetAtPath<MenSharpProgramAsset>(assetPath);
        if (AssetDatabase.AssetPathToGUID(assetPath) != wanted)
        {
            Debug.LogWarning($"MenSharp: could not give {assetPath} its stable GUID.");
        }
        return reloaded != null ? reloaded : asset;
    }

    /// Where a compile's import time goes, summed over its programs.
    public static long LoadMilliseconds;
    public static long CreateMilliseconds;
    public static long RefreshMilliseconds;
    public static long DirtyMilliseconds;

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

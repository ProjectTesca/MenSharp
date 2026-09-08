// Keeping UdonSharp's compiler away from MenSharp's sources.
//
// UdonSharp runs every Assembly-CSharp script through Roslyn at C# 7.3 to
// find its own behaviours. MenSharp sources are C# 9+ (switch expressions,
// recursive patterns), so a MenSharp file in Assembly-CSharp breaks that
// pass for the whole project — every UdonSharp asset stops compiling.
//
// UdonSharp has the answer built in: a list of path prefixes its scanner
// skips (`scanningDirectoryBlacklist` in its settings asset). This registers
// Assets/MenSharp there, once, on load and before every compile. That is
// what lets MenSharp sources live in Assembly-CSharp — where they can see
// every UdonSharp asset dropped into Assets — without an assembly
// definition of their own.
#if UNITY_EDITOR
using System;
using System.IO;
using UnityEditor;
using UnityEngine;

[InitializeOnLoad]
public static class MenSharpUdonSharpIsolation
{
    private const string IgnoredPrefix = MenSharpSources.SourceRoot + "/";

    static MenSharpUdonSharpIsolation()
    {
        EditorApplication.delayCall += () => Ensure(false);
    }

    /// Makes sure UdonSharp's scanner skips Assets/MenSharp. Returns true
    /// when the setting was just added (UdonSharp will notice on its next
    /// compile, which a script reload triggers).
    public static bool Ensure(bool log)
    {
        string path = UdonSharp.Updater.UdonSharpLocator.SettingsPath;
        if (string.IsNullOrEmpty(path))
        {
            return false;
        }
        // the settings class is internal to UdonSharp: the asset is edited
        // through its serialized form, and created by its type name
        var settings = AssetDatabase.LoadAssetAtPath<ScriptableObject>(path);
        bool created = settings == null;
        if (created)
        {
            Type type = Type.GetType("UdonSharpEditor.UdonSharpSettings, UdonSharp.Editor");
            if (type == null)
            {
                return false;
            }
            settings = ScriptableObject.CreateInstance(type);
            string folder = Path.GetDirectoryName(path);
            if (!string.IsNullOrEmpty(folder) && !AssetDatabase.IsValidFolder(folder))
            {
                Directory.CreateDirectory(folder);
                AssetDatabase.Refresh();
            }
            AssetDatabase.CreateAsset(settings, path);
        }
        var serialized = new SerializedObject(settings);
        SerializedProperty blacklist = serialized.FindProperty("scanningDirectoryBlacklist");
        if (blacklist == null || !blacklist.isArray)
        {
            return false;
        }
        for (int index = 0; index < blacklist.arraySize; index++)
        {
            string entry = blacklist.GetArrayElementAtIndex(index).stringValue ?? "";
            if (entry.Replace('\\', '/').TrimEnd('/') == IgnoredPrefix.TrimEnd('/'))
            {
                return false;
            }
        }
        blacklist.InsertArrayElementAtIndex(blacklist.arraySize);
        blacklist.GetArrayElementAtIndex(blacklist.arraySize - 1).stringValue = IgnoredPrefix;
        serialized.ApplyModifiedPropertiesWithoutUndo();
        EditorUtility.SetDirty(settings);
        AssetDatabase.SaveAssets();
        if (log)
        {
            Debug.Log(
                $"MenSharp: registered {IgnoredPrefix} in UdonSharp's scanning blacklist, so "
                + "UdonSharp leaves MenSharp sources alone.");
        }
        return true;
    }
}
#endif

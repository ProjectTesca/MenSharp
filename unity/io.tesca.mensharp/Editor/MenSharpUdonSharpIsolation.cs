// Keeping UdonSharp's compiler away from MenSharp's sources.
//
// UdonSharp runs every Assembly-CSharp script through Roslyn at C# 7.3 to
// find its own behaviours. MenSharp sources are C# 9+ (switch expressions,
// recursive patterns), so a MenSharp file in Assembly-CSharp breaks that
// pass for the whole project — every UdonSharp asset stops compiling.
//
// UdonSharp has the answer built in: a list of path prefixes its scanner
// skips (`scanningDirectoryBlacklist` in its settings asset). This registers
// Assets/MenSharp and every folder marked with a `.mensharp` file there, on
// load and before every compile. That is what lets MenSharp sources live in
// Assembly-CSharp — where they can see every UdonSharp asset dropped into
// Assets — without an assembly definition of their own.
#if UNITY_EDITOR
using System;
using System.Collections.Generic;
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

    /// Every folder UdonSharp's scanner must skip: Assets/MenSharp and each
    /// marked folder, as prefixes ending in a slash.
    private static List<string> IgnoredPrefixes()
    {
        var prefixes = new List<string> { IgnoredPrefix };
        foreach (string root in MenSharpMarker.MarkedRoots())
        {
            prefixes.Add(root + "/");
        }
        return prefixes;
    }

    /// Makes sure UdonSharp's scanner skips every MenSharp folder. Returns
    /// true when a prefix was just added (UdonSharp will notice on its next
    /// compile, which a script reload triggers).
    public static bool Ensure(bool log)
    {
        SerializedObject serialized = OpenSettings(create: true);
        if (serialized == null)
        {
            return false;
        }
        SerializedProperty blacklist = serialized.FindProperty("scanningDirectoryBlacklist");
        if (blacklist == null || !blacklist.isArray)
        {
            return false;
        }
        // stale entries first: a marked folder that was deleted (or unmarked
        // by hand) leaves a prefix that would keep UdonSharp out of a path it
        // should scan again. An entry for a folder that no longer exists is
        // inert and safe to drop; Assets/MenSharp is ours, kept regardless
        bool pruned = false;
        for (int index = blacklist.arraySize - 1; index >= 0; index--)
        {
            string entry = (blacklist.GetArrayElementAtIndex(index).stringValue ?? "")
                .Replace('\\', '/').TrimEnd('/');
            if (entry.Length == 0
                || entry == IgnoredPrefix.TrimEnd('/')
                || Directory.Exists(entry))
            {
                continue;
            }
            blacklist.DeleteArrayElementAtIndex(index);
            pruned = true;
        }

        var added = new List<string>();
        foreach (string prefix in IgnoredPrefixes())
        {
            if (!Contains(blacklist, prefix))
            {
                blacklist.InsertArrayElementAtIndex(blacklist.arraySize);
                blacklist.GetArrayElementAtIndex(blacklist.arraySize - 1).stringValue = prefix;
                added.Add(prefix);
            }
        }
        if (added.Count == 0 && !pruned)
        {
            return false;
        }
        serialized.ApplyModifiedPropertiesWithoutUndo();
        EditorUtility.SetDirty(serialized.targetObject);
        AssetDatabase.SaveAssets();
        if (log && added.Count > 0)
        {
            Debug.Log(
                $"MenSharp: registered {string.Join(", ", added)} in UdonSharp's scanning "
                + "blacklist, so UdonSharp leaves MenSharp sources alone.");
        }
        return added.Count > 0;
    }

    /// Stops UdonSharp skipping a folder that is no longer a MenSharp source
    /// — for Unmark. Assets/MenSharp is never removed: it is a MenSharp folder
    /// whether or not it carries the marker.
    public static void StopIgnoring(string folder)
    {
        string prefix = MenSharpSources.Normalize(folder).TrimEnd('/');
        if (prefix == IgnoredPrefix.TrimEnd('/'))
        {
            return;
        }
        SerializedObject serialized = OpenSettings(create: false);
        if (serialized == null)
        {
            return;
        }
        SerializedProperty blacklist = serialized.FindProperty("scanningDirectoryBlacklist");
        if (blacklist == null || !blacklist.isArray)
        {
            return;
        }
        bool removed = false;
        for (int index = blacklist.arraySize - 1; index >= 0; index--)
        {
            string entry = (blacklist.GetArrayElementAtIndex(index).stringValue ?? "")
                .Replace('\\', '/').TrimEnd('/');
            if (entry == prefix)
            {
                blacklist.DeleteArrayElementAtIndex(index);
                removed = true;
            }
        }
        if (removed)
        {
            serialized.ApplyModifiedPropertiesWithoutUndo();
            EditorUtility.SetDirty(serialized.targetObject);
            AssetDatabase.SaveAssets();
        }
    }

    private static bool Contains(SerializedProperty blacklist, string prefix)
    {
        string wanted = prefix.Replace('\\', '/').TrimEnd('/');
        for (int index = 0; index < blacklist.arraySize; index++)
        {
            string entry = (blacklist.GetArrayElementAtIndex(index).stringValue ?? "")
                .Replace('\\', '/').TrimEnd('/');
            if (entry == wanted)
            {
                return true;
            }
        }
        return false;
    }

    /// The UdonSharp settings as a SerializedObject, creating the asset when
    /// asked and it does not exist yet; null when UdonSharp is not present.
    private static SerializedObject OpenSettings(bool create)
    {
        string path = UdonSharp.Updater.UdonSharpLocator.SettingsPath;
        if (string.IsNullOrEmpty(path))
        {
            return null;
        }
        // the settings class is internal to UdonSharp: the asset is edited
        // through its serialized form, and created by its type name
        var settings = AssetDatabase.LoadAssetAtPath<ScriptableObject>(path);
        if (settings == null)
        {
            if (!create)
            {
                return null;
            }
            Type type = Type.GetType("UdonSharpEditor.UdonSharpSettings, UdonSharp.Editor");
            if (type == null)
            {
                return null;
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
        return new SerializedObject(settings);
    }
}
#endif

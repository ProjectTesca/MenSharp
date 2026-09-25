// A marker file — any file with the `.mensharp` extension, `MenSharp.mensharp`
// by default — marks a folder, and everything under it, as MenSharp's: its C#
// is compiled to Udon programs, and UdonSharp's C# 7.3 scanner is kept out of
// it (see MenSharpUdonSharpIsolation). This is what lets MenSharp sources live
// anywhere in the project without an assembly definition, not only under
// Assets/MenSharp.
//
// The marker is an ordinary asset: Unity imports it, gives it a .meta and a
// GUID, and so it travels with its folder in a unitypackage or a VPM package
// — exactly as an .asmdef does. It used to be a dotfile (`.mensharp`), which
// Unity ignores (see "Hidden assets" in the manual), and so did every package
// export: an imported gimmick arrived unmarked. A dotfile still marks its
// folder, and is moved to the new name on load and before every compile.
//
// The behaviour is decided by the file's existence alone; its text is never
// read. What the editor shows for one is MenSharpMarkerImporter's.
#if UNITY_EDITOR
using System;
using System.Collections.Generic;
using System.IO;
using UnityEditor;
using UnityEngine;

public static class MenSharpMarker
{
    public const string MarkerExtension = ".mensharp";
    /// The name the menu writes. Any base name marks the folder just the same.
    public const string MarkerFileName = "MenSharp" + MarkerExtension;
    /// The dotfile of earlier versions: still honoured, migrated when met.
    public const string LegacyMarkerFileName = ".mensharp";

    /// Written into a fresh marker so anyone who opens it knows what it does.
    private const string MarkerContent =
        "# This folder's C# is compiled by MenSharp to Udon, and everything under it.\n"
        + "# UdonSharp's scanner skips it, so C# 9+ here does not break UdonSharp.\n"
        + "# Delete this file to hand the folder back to UdonSharp / plain C#.\n";

    [InitializeOnLoadMethod]
    private static void MigrateOnLoad()
    {
        EditorApplication.delayCall += () => MigrateLegacyMarkers(true);
    }

    /// Is this file name a marker's? `MenSharp.mensharp`, `anything.mensharp`
    /// and the legacy `.mensharp` alike.
    public static bool IsMarkerFile(string path)
    {
        string name = Path.GetFileName(path);
        return name.EndsWith(MarkerExtension, StringComparison.OrdinalIgnoreCase);
    }

    public static bool IsLegacyMarkerFile(string path)
    {
        return Path.GetFileName(path) == LegacyMarkerFileName;
    }

    /// Every marker in a folder, as normalized paths. Read off disk, so a
    /// marker counts the moment it is written, imported or not.
    private static List<string> MarkerFilesIn(string folder)
    {
        var markers = new List<string>();
        if (!Directory.Exists(folder))
        {
            return markers;
        }
        foreach (string file in Directory.GetFiles(folder, "*" + MarkerExtension))
        {
            if (IsMarkerFile(file))
            {
                markers.Add(MenSharpSources.Normalize(file));
            }
        }
        return markers;
    }

    /// Every folder under Assets that carries a marker, as asset paths with
    /// no trailing slash — except those Unity ignores (`Foo~`).
    public static List<string> MarkedRoots()
    {
        var roots = new List<string>();
        if (!Directory.Exists("Assets"))
        {
            return roots;
        }
        foreach (string file in Directory.GetFiles("Assets", "*" + MarkerExtension, SearchOption.AllDirectories))
        {
            if (!IsMarkerFile(file))
            {
                continue;
            }
            // a marker inside a folder Unity ignores (`Tests~`) marks nothing:
            // none of the scripts under it are compiled
            string folder = MenSharpSources.Normalize(Path.GetDirectoryName(file));
            if (!MenSharpSources.IsUnityIgnored(folder) && !roots.Contains(folder))
            {
                roots.Add(folder);
            }
        }
        return roots;
    }

    /// The nearest marked folder at or above this asset path, or null when no
    /// ancestor is marked. Nearest wins, so a marked gimmick folder inside a
    /// marked parent keeps its own Programs.
    public static string MarkedRootOf(string assetPath)
    {
        string path = MenSharpSources.Normalize(assetPath);
        if (!path.StartsWith("Assets/", StringComparison.Ordinal))
        {
            return null;
        }
        string directory = MenSharpSources.Normalize(Path.GetDirectoryName(path));
        while (directory == "Assets" || directory.StartsWith("Assets/", StringComparison.Ordinal))
        {
            if (HasMarker(directory))
            {
                return directory;
            }
            int slash = directory.LastIndexOf('/');
            if (slash < 0)
            {
                break;
            }
            directory = directory.Substring(0, slash);
        }
        return null;
    }

    public static bool HasMarker(string folder)
    {
        return MarkerFilesIn(MenSharpSources.Normalize(folder)).Count > 0;
    }

    /// Writes the marker into a folder if it carries none yet. The caller
    /// refreshes the AssetDatabase, which imports it.
    public static void EnsureMarker(string folder)
    {
        string normalized = MenSharpSources.Normalize(folder);
        if (!HasMarker(normalized))
        {
            File.WriteAllText(normalized + "/" + MarkerFileName, MarkerContent);
        }
    }

    /// Moves every legacy dotfile marker under Assets to the asset form:
    /// `MenSharp.mensharp` is written beside it (unless the folder already
    /// has a marker of the new kind) and the dotfile is deleted. Answers the
    /// folders that were migrated; with `refresh`, imports the new files and
    /// says so once. Disk only otherwise, so a test may run it with unimported
    /// probe files around.
    public static List<string> MigrateLegacyMarkers(bool refresh)
    {
        var migrated = new List<string>();
        if (!Directory.Exists("Assets"))
        {
            return migrated;
        }
        foreach (string file in Directory.GetFiles("Assets", LegacyMarkerFileName, SearchOption.AllDirectories))
        {
            if (!IsLegacyMarkerFile(file))
            {
                continue;
            }
            string folder = MenSharpSources.Normalize(Path.GetDirectoryName(file));
            bool hasNew = MarkerFilesIn(folder).Exists(marker => !IsLegacyMarkerFile(marker));
            if (!hasNew)
            {
                File.WriteAllText(folder + "/" + MarkerFileName, MarkerContent);
            }
            File.Delete(file);
            migrated.Add(folder);
        }
        if (migrated.Count > 0 && refresh)
        {
            AssetDatabase.Refresh();
            Debug.Log(
                $"MenSharp: the marker in {string.Join(", ", migrated)} is now {MarkerFileName} "
                + "(the .mensharp dotfile was never imported by Unity, so it was left out of every "
                + "exported package). Nothing else changed; the new file belongs in version control "
                + "like any asset.");
        }
        return migrated;
    }

    // ------------------------------------------------------------- mark/unmark

    /// Marks these folders, imports the markers and recompiles.
    public static void Mark(IEnumerable<string> folders)
    {
        var marked = new List<string>();
        foreach (string folder in folders)
        {
            if (!HasMarker(folder))
            {
                EnsureMarker(folder);
                marked.Add(MenSharpSources.Normalize(folder));
            }
        }
        if (marked.Count == 0)
        {
            return;
        }
        AssetDatabase.Refresh();
        MenSharpUdonSharpIsolation.Ensure(true);
        Debug.Log(
            $"MenSharp: {string.Join(", ", marked)} is now a MenSharp source folder — "
            + "its C# (and every subfolder's) compiles to Udon. Recompiling now.");
        // marking changes which files are MenSharp, not their timestamps, so
        // the change-detecting compile would see nothing new: force one
        MenSharpCompiler.CompileAll();
    }

    /// Removes every marker from these folders (the asset with its .meta, a
    /// legacy dotfile as a file), lets UdonSharp back in and recompiles.
    public static void Unmark(IEnumerable<string> folders)
    {
        var unmarked = new List<string>();
        foreach (string folder in folders)
        {
            string normalized = MenSharpSources.Normalize(folder);
            List<string> markers = MarkerFilesIn(normalized);
            if (markers.Count == 0)
            {
                continue;
            }
            foreach (string marker in markers)
            {
                if (IsLegacyMarkerFile(marker) || !AssetDatabase.DeleteAsset(marker))
                {
                    File.Delete(marker);
                }
            }
            MenSharpUdonSharpIsolation.StopIgnoring(normalized);
            unmarked.Add(normalized);
        }
        if (unmarked.Count == 0)
        {
            return;
        }
        AssetDatabase.Refresh();
        Debug.LogWarning(
            $"MenSharp: {string.Join(", ", unmarked)} is no longer a MenSharp source folder. "
            + "Any programs already compiled under it stay until you delete them; delete the "
            + "folder's Programs to remove them. Recompiling now.");
        MenSharpCompiler.CompileAll();
    }

    // ------------------------------------------------------------------ menu

    [MenuItem("Assets/MenSharp/Mark Folder as MenSharp Source", false, 2000)]
    private static void MarkSelected()
    {
        Mark(SelectedFolders());
    }

    [MenuItem("Assets/MenSharp/Mark Folder as MenSharp Source", true)]
    private static bool MarkSelectedValidate()
    {
        foreach (string folder in SelectedFolders())
        {
            if (!HasMarker(folder))
            {
                return true;
            }
        }
        return false;
    }

    [MenuItem("Assets/MenSharp/Unmark MenSharp Source Folder", false, 2001)]
    private static void UnmarkSelected()
    {
        Unmark(SelectedFolders());
    }

    [MenuItem("Assets/MenSharp/Unmark MenSharp Source Folder", true)]
    private static bool UnmarkSelectedValidate()
    {
        foreach (string folder in SelectedFolders())
        {
            if (HasMarker(folder))
            {
                return true;
            }
        }
        return false;
    }

    /// The folders currently selected in the Project window.
    private static List<string> SelectedFolders()
    {
        var folders = new List<string>();
        foreach (string guid in Selection.assetGUIDs)
        {
            string path = AssetDatabase.GUIDToAssetPath(guid);
            if (!string.IsNullOrEmpty(path) && AssetDatabase.IsValidFolder(path))
            {
                folders.Add(MenSharpSources.Normalize(path));
            }
        }
        return folders;
    }
}
#endif

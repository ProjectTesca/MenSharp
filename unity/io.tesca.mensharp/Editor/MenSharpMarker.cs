// A ".mensharp" file marks a folder — and everything under it — as MenSharp's:
// its C# is compiled to Udon programs, and UdonSharp's C# 7.3 scanner is kept
// out of it (see MenSharpUdonSharpIsolation). This is what lets MenSharp
// sources live anywhere in the project without an assembly definition, not
// only under Assets/MenSharp.
//
// The file is a dotfile on purpose: Unity ignores names starting with a dot,
// so it imports no asset and writes no .meta, and MenSharp reads it straight
// off disk. Drop one by hand in any editor, or use the Assets > MenSharp menu.
#if UNITY_EDITOR
using System;
using System.Collections.Generic;
using System.IO;
using UnityEditor;
using UnityEngine;

public static class MenSharpMarker
{
    public const string MarkerFileName = ".mensharp";

    /// Written into a fresh marker so anyone who opens it knows what it does.
    /// The behaviour is decided by the file's existence alone; the text is
    /// never read.
    private const string MarkerContent =
        "# This folder's C# is compiled by MenSharp to Udon, and everything under it.\n"
        + "# UdonSharp's scanner skips it, so C# 9+ here does not break UdonSharp.\n"
        + "# Delete this file to hand the folder back to UdonSharp / plain C#.\n";

    /// Every folder under Assets that carries the marker, as asset paths with
    /// no trailing slash. Read off disk: the marker is a dotfile Unity does
    /// not import.
    public static List<string> MarkedRoots()
    {
        var roots = new List<string>();
        if (!Directory.Exists("Assets"))
        {
            return roots;
        }
        foreach (string file in Directory.GetFiles("Assets", MarkerFileName, SearchOption.AllDirectories))
        {
            if (Path.GetFileName(file) == MarkerFileName)
            {
                roots.Add(MenSharpSources.Normalize(Path.GetDirectoryName(file)));
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
            if (File.Exists(directory + "/" + MarkerFileName))
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
        return File.Exists(MenSharpSources.Normalize(folder) + "/" + MarkerFileName);
    }

    /// Writes the marker into a folder if it is not there yet.
    public static void EnsureMarker(string folder)
    {
        string path = MenSharpSources.Normalize(folder) + "/" + MarkerFileName;
        if (!File.Exists(path))
        {
            File.WriteAllText(path, MarkerContent);
        }
    }

    // ------------------------------------------------------------------ menu

    [MenuItem("Assets/MenSharp/Mark Folder as MenSharp Source", false, 2000)]
    private static void MarkSelected()
    {
        var marked = new List<string>();
        foreach (string folder in SelectedFolders())
        {
            if (!HasMarker(folder))
            {
                EnsureMarker(folder);
                marked.Add(folder);
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
        var unmarked = new List<string>();
        foreach (string folder in SelectedFolders())
        {
            string path = MenSharpSources.Normalize(folder) + "/" + MarkerFileName;
            if (File.Exists(path))
            {
                File.Delete(path);
                MenSharpUdonSharpIsolation.StopIgnoring(folder);
                unmarked.Add(folder);
            }
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

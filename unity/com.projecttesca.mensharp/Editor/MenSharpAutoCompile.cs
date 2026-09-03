// MenSharp: compile on save.
//
// Whenever a .cs under Assets/MenSharp/ is (re)imported — which is what
// saving in an IDE triggers — the compiler runs automatically. Program assets
// update in place (stable GUIDs), so paired UdonBehaviours pick the new code
// up with no further action. Diagnostics land in the Console as always.
//
// Updating the *package* has to trigger a rebuild too: the compiler lives in
// `Compiler~/`, which Unity deliberately does not import, so replacing it
// fires no asset event. Without the check below, a newer compiler would sit
// there while the old programs stayed on disk — and the only symptom would be
// features that quietly do not appear.

#if UNITY_EDITOR
using System;
using System.Collections.Generic;
using System.IO;
using UnityEditor;
using UnityEngine;

public class MenSharpAutoCompile : AssetPostprocessor
{
    private static bool pending;

    [InitializeOnLoadMethod]
    private static void RecompileWhenTheCompilerIsNewer()
    {
        EditorApplication.delayCall += () =>
        {
            string compiler = MenSharpCompiler.CompilerPath();
            if (compiler == null || !File.Exists(compiler))
            {
                return;
            }
            var programs = new List<string>();
            foreach (string guid in AssetDatabase.FindAssets("t:MenSharpProgramAsset"))
            {
                programs.Add(AssetDatabase.GUIDToAssetPath(guid));
            }
            if (programs.Count == 0)
            {
                return;
            }
            DateTime built = File.GetLastWriteTimeUtc(compiler);
            foreach (string program in programs)
            {
                if (File.Exists(program) && File.GetLastWriteTimeUtc(program) < built)
                {
                    Debug.Log(
                        "MenSharp: the bundled compiler is newer than the compiled programs — "
                        + "rebuilding them.");
                    MenSharpCompiler.CompileAll();
                    return;
                }
            }
        };
    }

    private static void OnPostprocessAllAssets(
        string[] importedAssets,
        string[] deletedAssets,
        string[] movedAssets,
        string[] movedFromAssetPaths)
    {
        // a program asset arrived or left (a compile, a package install):
        // the class → program index is stale
        foreach (string path in importedAssets)
        {
            if (path.EndsWith(".asset") && path.Contains("/Programs/"))
            {
                MenSharpSources.InvalidateProgramIndex();
                break;
            }
        }
        if (pending)
        {
            return;
        }
        bool relevant = false;
        foreach (string path in importedAssets)
        {
            if (IsMenSharpSource(path))
            {
                relevant = true;
                break;
            }
        }
        if (!relevant)
        {
            foreach (string path in deletedAssets)
            {
                if (IsMenSharpSource(path))
                {
                    relevant = true;
                    break;
                }
            }
        }
        if (!relevant)
        {
            return;
        }

        pending = true;
        EditorApplication.delayCall += () =>
        {
            pending = false;
            MenSharpCompiler.CompileAll();
        };
    }

    /// A change worth a recompile: a MenSharp source, or a library source
    /// (an UdonSharp asset's script whose surface M# code may use).
    private static bool IsMenSharpSource(string path)
    {
        if (!path.EndsWith(".cs"))
        {
            return false;
        }
        return MenSharpSources.IsMenSharpSource(path) || MenSharpSources.IsLibrarySource(path);
    }
}
#endif

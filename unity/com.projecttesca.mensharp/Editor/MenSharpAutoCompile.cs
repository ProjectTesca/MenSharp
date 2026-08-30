// MenSharp: compile on save.
//
// Whenever a .cs under Assets/MenSharp/ is (re)imported — which is what
// saving in an IDE triggers — the compiler runs automatically. Program assets
// update in place (stable GUIDs), so paired UdonBehaviours pick the new code
// up with no further action. Diagnostics land in the Console as always.

#if UNITY_EDITOR
using UnityEditor;

public class MenSharpAutoCompile : AssetPostprocessor
{
    private static bool pending;

    private static void OnPostprocessAllAssets(
        string[] importedAssets,
        string[] deletedAssets,
        string[] movedAssets,
        string[] movedFromAssetPaths)
    {
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

    private static bool IsMenSharpSource(string path)
    {
        return path.StartsWith("Assets/MenSharp/")
            && path.EndsWith(".cs")
            && !path.StartsWith("Assets/MenSharp/Programs/");
    }
}
#endif

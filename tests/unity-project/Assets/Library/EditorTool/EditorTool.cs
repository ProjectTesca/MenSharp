// Editor-only by its assembly definition's platforms, not by an `Editor`
// folder: MenSharp must leave it alone — it uses the editor API and the
// patcher below, neither of which the compiler could read.
using UnityEditor;

public static class EditorTool
{
    public static bool Playing()
    {
        return EditorApplication.isPlaying && EditorPatcher.Deref(0) == 0;
    }
}

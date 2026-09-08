// MenSharp: "what is actually wired up in this scene?"
//
// The backing UdonBehaviours are hidden, which is right for everyday use and
// terrible when something runs that you did not expect. This prints the whole
// picture — every GameObject with a MenSharp component or a MenSharp program
// on it, and which is paired to which — so a surprise becomes a fact.

#if UNITY_EDITOR
using System.Collections.Generic;
using System.Text;
using MenSharp;
using UnityEditor;
using UnityEngine;
using UnityEngine.SceneManagement;
using VRC.Udon;

public static class MenSharpReport
{
    [MenuItem("MenSharp/Report Scene Wiring")]
    public static void Report()
    {
        var text = new StringBuilder("MenSharp: scene wiring\n");
        int objects = 0;

        for (int index = 0; index < SceneManager.sceneCount; index++)
        {
            Scene scene = SceneManager.GetSceneAt(index);
            if (!scene.isLoaded)
            {
                continue;
            }
            text.Append("scene ").Append(scene.name).Append('\n');
            foreach (GameObject target in MenSharpProxy.PairingTargets(scene))
            {
                objects++;
                text.Append("  ").Append(Path(target)).Append('\n');

                var components = new List<MenSharpBehaviour>(
                    target.GetComponents<MenSharpBehaviour>());
                if (components.Count == 0)
                {
                    text.Append("    (no MenSharp component)\n");
                }
                foreach (MenSharpBehaviour proxy in components)
                {
                    MenSharpProgramAsset program = MenSharpProxy.FindProgram(proxy.GetType());
                    text.Append("    component ").Append(proxy.GetType().FullName)
                        .Append(" -> ")
                        .Append(program == null ? "no compiled program" : program.name)
                        .Append(MenSharpProxy.FindPaired(proxy) == null ? " (not paired)" : "")
                        .Append('\n');
                }

                foreach (UdonBehaviour udon in target.GetComponents<UdonBehaviour>())
                {
                    string source = udon.programSource == null
                        ? "<none>"
                        : udon.programSource.name;
                    string kind = udon.programSource is MenSharpProgramAsset
                        ? "MenSharp"
                        : udon.programSource == null ? "empty" : "other";
                    bool hidden = (udon.hideFlags & HideFlags.HideInInspector) != 0;
                    text.Append("    UdonBehaviour program=").Append(source)
                        .Append(" [").Append(kind).Append(hidden ? ", hidden" : "").Append("]\n");
                }
            }
        }

        if (objects == 0)
        {
            text.Append("  nothing wired up in the open scenes\n");
        }
        Debug.Log(text.ToString());
    }

    private static string Path(GameObject target)
    {
        string path = target.name;
        Transform parent = target.transform.parent;
        while (parent != null)
        {
            path = parent.name + "/" + path;
            parent = parent.parent;
        }
        return path;
    }
}
#endif

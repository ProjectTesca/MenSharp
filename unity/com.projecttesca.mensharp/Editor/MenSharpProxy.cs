// MenSharp: the proxy wiring — what makes "drag the script onto a GameObject"
// work.
//
// The trick (the same one UdonSharp uses): a MenSharpBehaviour subclass is a
// real MonoBehaviour, so Unity gives us the component workflow and the
// inspector for free. The component itself never runs, though — it is a
// *proxy* for the real thing:
//
//   - when one is added to a GameObject, an UdonBehaviour with the class's
//     compiled program asset is paired next to it;
//   - when entering play mode or building, every proxy's public fields are
//     copied into the paired UdonBehaviour's public variables (this is how
//     inspector-edited values and scene references reach the Udon heap), and
//     the proxy component is then stripped so it can never double-execute.

#if UNITY_EDITOR
using System;
using System.Reflection;
using MenSharp;
using UnityEditor;
using UnityEditor.Build;
using UnityEditor.Build.Reporting;
using UnityEngine;
using UnityEngine.SceneManagement;
using VRC.Udon;
using VRC.Udon.Common;
using VRC.Udon.Common.Interfaces;

[InitializeOnLoad]
public static class MenSharpProxy
{
    private const string ProgramsFolder = "Assets/MenSharp/Programs";

    static MenSharpProxy()
    {
        ObjectFactory.componentWasAdded += component =>
        {
            if (component is MenSharpBehaviour proxy)
            {
                EnsurePaired(proxy);
            }
        };

        // values must be transferred while still in edit mode: Unity then
        // serializes the scene into the play-mode copy, carrying the updated
        // public-variable bytes along. Doing this later (in scene processing)
        // only mutates a lazily-deserialized table that a subsequent
        // (de)serialization pass silently discards.
        EditorApplication.playModeStateChanged += change =>
        {
            if (change == PlayModeStateChange.ExitingEditMode)
            {
                TransferAllInOpenScenes();
            }
        };

        // hideFlags only persist when the scene is saved, so re-hide the
        // backing UdonBehaviours whenever the editor (re)loads, a scene
        // opens, or the hierarchy changes (which also catches proxies added
        // by drag-and-drop, where componentWasAdded may not fire) — the
        // pairing is self-repairing
        EditorApplication.delayCall += HideAllPairedInOpenScenes;
        UnityEditor.SceneManagement.EditorSceneManager.sceneOpened +=
            (_, _) => HideAllPairedInOpenScenes();
        EditorApplication.hierarchyChanged += ScheduleSweep;
    }

    private static bool sweepPending;

    private static void ScheduleSweep()
    {
        if (sweepPending)
        {
            return;
        }
        sweepPending = true;
        EditorApplication.delayCall += () =>
        {
            sweepPending = false;
            HideAllPairedInOpenScenes();
        };
    }

    private static void HideAllPairedInOpenScenes()
    {
        if (EditorApplication.isPlayingOrWillChangePlaymode)
        {
            return;
        }
        for (int index = 0; index < UnityEngine.SceneManagement.SceneManager.sceneCount; index++)
        {
            Scene scene = UnityEngine.SceneManagement.SceneManager.GetSceneAt(index);
            if (!scene.isLoaded)
            {
                continue;
            }
            foreach (GameObject root in scene.GetRootGameObjects())
            {
                foreach (MenSharpBehaviour proxy in
                    root.GetComponentsInChildren<MenSharpBehaviour>(true))
                {
                    EnsurePaired(proxy, quiet: true);
                }
            }
        }
    }

    private static void TransferAllInOpenScenes()
    {
        for (int index = 0; index < UnityEngine.SceneManagement.SceneManager.sceneCount; index++)
        {
            Scene scene = UnityEngine.SceneManagement.SceneManager.GetSceneAt(index);
            if (!scene.isLoaded)
            {
                continue;
            }
            foreach (GameObject root in scene.GetRootGameObjects())
            {
                foreach (MenSharpBehaviour proxy in
                    root.GetComponentsInChildren<MenSharpBehaviour>(true))
                {
                    UdonBehaviour udon = EnsurePaired(proxy);
                    if (udon != null)
                    {
                        TransferValues(proxy, udon);
                    }
                }
            }
        }
    }

    /// The UdonBehaviour carrying this proxy's compiled program, created on
    /// the same GameObject when missing.
    public static UdonBehaviour EnsurePaired(MenSharpBehaviour proxy, bool quiet = false)
    {
        string className = proxy.GetType().Name;
        string assetPath = $"{ProgramsFolder}/{className}.asset";
        var program = AssetDatabase.LoadAssetAtPath<MenSharpProgramAsset>(assetPath);
        if (program == null)
        {
            if (!quiet)
            {
                Debug.LogWarning(
                    $"MenSharp: no compiled program for {className} yet — run MenSharp > "
                    + "Compile All (it will pair automatically afterwards).",
                    proxy);
            }
            return null;
        }

        foreach (UdonBehaviour existing in proxy.GetComponents<UdonBehaviour>())
        {
            if (existing.programSource == program)
            {
                HideBackingBehaviour(existing);
                return existing;
            }
        }
        var udon = Undo.AddComponent<UdonBehaviour>(proxy.gameObject);
        udon.programSource = program;
        HideBackingBehaviour(udon);
        EditorUtility.SetDirty(udon);
        return udon;
    }

    /// The paired UdonBehaviour is an implementation detail: hiding it leaves
    /// exactly one place to edit values — the proxy component — so nothing
    /// typed into the UdonBehaviour's own inspector can be silently
    /// overwritten by the transfer.
    private static void HideBackingBehaviour(UdonBehaviour udon)
    {
        if ((udon.hideFlags & HideFlags.HideInInspector) == 0)
        {
            udon.hideFlags |= HideFlags.HideInInspector;
            EditorUtility.SetDirty(udon);
            // the inspector does not notice hideFlags changes on its own
            EditorApplication.delayCall += () =>
            {
                UnityEditorInternal.InternalEditorUtility.RepaintAllViews();
            };
        }
    }

    /// Copies the proxy's public instance fields into the UdonBehaviour's
    /// public variable table — the values the Udon heap starts from.
    public static void TransferValues(MenSharpBehaviour proxy, UdonBehaviour udon)
    {
        IUdonVariableTable table = udon.publicVariables;
        var summary = new System.Text.StringBuilder();
        foreach (FieldInfo field in proxy
            .GetType()
            .GetFields(BindingFlags.Public | BindingFlags.Instance))
        {
            object value = field.GetValue(proxy);
            table.RemoveVariable(field.Name);
            Type variableType = typeof(UdonVariable<>).MakeGenericType(field.FieldType);
            var variable = (IUdonVariable)Activator.CreateInstance(
                variableType, field.Name, value);
            if (!table.TryAddVariable(variable))
            {
                Debug.LogWarning(
                    $"MenSharp: could not set public variable {field.Name} on {udon.name}",
                    udon);
                continue;
            }
            if (summary.Length > 0)
            {
                summary.Append(", ");
            }
            summary.Append(field.Name).Append('=').Append(value ?? "null");
        }

        // write the table back into its serialized byte form immediately, so
        // any later (de)serialization keeps the values
        if (udon is UnityEngine.ISerializationCallbackReceiver receiver)
        {
            receiver.OnBeforeSerialize();
        }
        EditorUtility.SetDirty(udon);
        Debug.Log(
            $"MenSharp: transferred to {proxy.GetType().Name} on "
            + $"{proxy.gameObject.name}: {summary}");
    }
}

/// Runs for every scene on play-mode entry and on world builds: pair,
/// transfer, strip.
public class MenSharpSceneProcessor : IProcessSceneWithReport
{
    public int callbackOrder => -10_000;

    public void OnProcessScene(Scene scene, BuildReport report)
    {
        foreach (GameObject root in scene.GetRootGameObjects())
        {
            foreach (MenSharpBehaviour proxy in
                root.GetComponentsInChildren<MenSharpBehaviour>(true))
            {
                UdonBehaviour udon = MenSharpProxy.EnsurePaired(proxy);
                if (udon != null)
                {
                    MenSharpProxy.TransferValues(proxy, udon);
                }
                UnityEngine.Object.DestroyImmediate(proxy);
            }
        }
    }
}
#endif

// MenSharp: the proxy wiring — what makes "drag the script onto a GameObject"
// work.
//
// The trick (the same one UdonSharp uses): a MenSharpBehaviour subclass is a
// real MonoBehaviour, so Unity gives us the component workflow and the
// inspector for free. The component itself never runs, though — it is a
// *proxy* for the real thing:
//
//   - the UdonBehaviours on a GameObject are kept in sync with the proxies on
//     it: one per proxy, carrying that proxy's compiled program, and none left
//     over (see SyncPairs — pairing has to be a synchronisation, not an
//     append, or replacing a component leaves the old program running from a
//     hidden component nobody can see);
//   - when entering play mode or building, every proxy's public fields are
//     copied into the paired UdonBehaviour's public variables (this is how
//     inspector-edited values and scene references reach the Udon heap), and
//     the proxy component is then stripped so it can never double-execute.

#if UNITY_EDITOR
using System;
using System.Collections.Generic;
using System.Reflection;
using MenSharp;
using UnityEditor;
using UnityEditor.Build;
using UnityEditor.Build.Reporting;
using UnityEngine;
using UnityEngine.SceneManagement;
using VRC.SDKBase;
using VRC.Udon;
using VRC.Udon.Common;
using VRC.Udon.Common.Interfaces;

[InitializeOnLoad]
public static class MenSharpProxy
{
    public const string ProgramsFolder = "Assets/MenSharp/Programs";

    private const string RevealPreference = "MenSharp.RevealBackingBehaviours";
    private const string RevealMenu = "MenSharp/Show Backing UdonBehaviours";

    static MenSharpProxy()
    {
        ObjectFactory.componentWasAdded += component =>
        {
            if (component is MenSharpBehaviour proxy)
            {
                SyncPairs(proxy.gameObject);
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

        // hideFlags only persist when the scene is saved, so re-run the sync
        // whenever the editor (re)loads, a scene opens, or the hierarchy
        // changes (which also catches proxies added by drag-and-drop, where
        // componentWasAdded may not fire) — the pairing is self-repairing
        EditorApplication.delayCall += SyncAllInOpenScenes;
        UnityEditor.SceneManagement.EditorSceneManager.sceneOpened +=
            (_, _) => SyncAllInOpenScenes();
        EditorApplication.hierarchyChanged += ScheduleSweep;
    }

    // ------------------------------------------------------------ visibility

    /// The backing UdonBehaviour is an implementation detail, so it is hidden:
    /// that leaves exactly one place to edit values — the proxy — and nothing
    /// typed into the UdonBehaviour can be silently overwritten by a transfer.
    /// Revealing them is for debugging, hence a preference rather than a
    /// per-object flag the sync would have to remember.
    public static bool Reveal
    {
        get => EditorPrefs.GetBool(RevealPreference, false);
        set
        {
            if (Reveal == value)
            {
                return;
            }
            EditorPrefs.SetBool(RevealPreference, value);
            Menu.SetChecked(RevealMenu, value);
            SyncAllInOpenScenes();
            UnityEditorInternal.InternalEditorUtility.RepaintAllViews();
        }
    }

    [MenuItem(RevealMenu)]
    private static void ToggleReveal() => Reveal = !Reveal;

    [MenuItem(RevealMenu, validate = true)]
    private static bool ToggleRevealValidate()
    {
        Menu.SetChecked(RevealMenu, Reveal);
        return true;
    }

    // ----------------------------------------------------------------- sweeps

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
            SyncAllInOpenScenes();
        };
    }

    private static void SyncAllInOpenScenes()
    {
        if (EditorApplication.isPlayingOrWillChangePlaymode)
        {
            return;
        }
        foreach (GameObject target in PairingTargetsInOpenScenes())
        {
            SyncPairs(target, quiet: true);
        }
    }

    private static void TransferAllInOpenScenes()
    {
        SyncThenTransfer(PairingTargetsInOpenScenes(), undoable: true);
    }

    /// Pair everything first, then copy values. The order matters now that a
    /// behaviour can reference another one: the value written for
    /// `public Door door` is the UdonBehaviour paired with that Door, so every
    /// pairing has to exist before the first transfer runs.
    public static void SyncThenTransfer(List<GameObject> targets, bool undoable)
    {
        var pairs = new List<(MenSharpBehaviour proxy, UdonBehaviour udon)>();
        foreach (GameObject target in targets)
        {
            pairs.AddRange(SyncPairs(target, quiet: false, undoable: undoable));
        }
        foreach ((MenSharpBehaviour proxy, UdonBehaviour udon) in pairs)
        {
            TransferValues(proxy, udon);
        }
    }

    /// Every GameObject in a scene that either carries a MenSharp proxy or one
    /// of our backing UdonBehaviours. The second half is the important one: it
    /// is how a leftover whose proxy is already gone gets found at all — look
    /// only where the proxies are and an orphan is invisible *and* immortal.
    public static List<GameObject> PairingTargets(Scene scene)
    {
        var targets = new List<GameObject>();
        var seen = new HashSet<int>();
        foreach (GameObject root in scene.GetRootGameObjects())
        {
            foreach (MenSharpBehaviour proxy in
                root.GetComponentsInChildren<MenSharpBehaviour>(true))
            {
                if (seen.Add(proxy.gameObject.GetInstanceID()))
                {
                    targets.Add(proxy.gameObject);
                }
            }
            foreach (UdonBehaviour udon in root.GetComponentsInChildren<UdonBehaviour>(true))
            {
                if (IsBackingBehaviour(udon) && seen.Add(udon.gameObject.GetInstanceID()))
                {
                    targets.Add(udon.gameObject);
                }
            }
        }
        return targets;
    }

    private static List<GameObject> PairingTargetsInOpenScenes()
    {
        var targets = new List<GameObject>();
        for (int index = 0; index < SceneManager.sceneCount; index++)
        {
            Scene scene = SceneManager.GetSceneAt(index);
            if (scene.isLoaded)
            {
                targets.AddRange(PairingTargets(scene));
            }
        }
        return targets;
    }

    // ---------------------------------------------------------------- pairing

    /// The compiled program for a behaviour class, or null when it has not
    /// been compiled yet.
    public static MenSharpProgramAsset FindProgram(Type behaviourType)
    {
        // wherever its assembly keeps its programs: Assets/MenSharp/Programs
        // for the project's own, a package's Programs folder for a package's
        return MenSharpSources.FindProgram(behaviourType);
    }

    /// The UdonBehaviour currently carrying this proxy's program, without
    /// creating or destroying anything — for inspectors and diagnostics.
    public static UdonBehaviour FindPaired(MenSharpBehaviour proxy)
    {
        MenSharpProgramAsset program = FindProgram(proxy.GetType());
        if (program == null)
        {
            return null;
        }
        foreach (UdonBehaviour udon in proxy.GetComponents<UdonBehaviour>())
        {
            if (udon != null && udon.programSource == program)
            {
                return udon;
            }
        }
        return null;
    }

    /// Is this an UdonBehaviour *we* placed? Only ones carrying a MenSharp
    /// program qualify, so hand-authored Udon (graphs, UdonSharp) is never
    /// touched — least of all removed.
    private static bool IsBackingBehaviour(UdonBehaviour udon)
    {
        if (udon == null)
        {
            return false;
        }
        if (udon.programSource is MenSharpProgramAsset)
        {
            return true;
        }
        // its program asset was deleted, so the type no longer identifies it.
        // Nothing else hides an UdonBehaviour, and one with no program does
        // nothing but linger — recognising it here is what lets the sweep
        // clear it away. (While `Reveal` is on ours are not hidden, so a
        // broken one is visible instead, which is just as good.)
        return udon.programSource == null
            && (udon.hideFlags & HideFlags.HideInInspector) != 0;
    }

    /// Makes this GameObject's backing UdonBehaviours match the MenSharp
    /// proxies on it: one per proxy carrying that proxy's program, created
    /// when missing and removed when its proxy is gone or now runs a
    /// different class. Returns the resulting pairs.
    ///
    /// This has to be a synchronisation rather than "add if absent": swapping
    /// one behaviour for another, or deleting the component, used to leave the
    /// old UdonBehaviour behind — hidden, and still executing its program.
    ///
    /// `undoable` is off while a scene is being processed for play or a build:
    /// that scene is a throwaway copy, and registering undo steps against it
    /// would outlive the copy itself.
    public static List<(MenSharpBehaviour proxy, UdonBehaviour udon)> SyncPairs(
        GameObject target, bool quiet = false, bool undoable = true)
    {
        var pairs = new List<(MenSharpBehaviour, UdonBehaviour)>();
        if (target == null)
        {
            return pairs;
        }

        var spare = new List<UdonBehaviour>();
        foreach (UdonBehaviour udon in target.GetComponents<UdonBehaviour>())
        {
            if (IsBackingBehaviour(udon))
            {
                spare.Add(udon);
            }
        }

        foreach (MenSharpBehaviour proxy in target.GetComponents<MenSharpBehaviour>())
        {
            MenSharpProgramAsset program = FindProgram(proxy.GetType());
            if (program == null)
            {
                if (!quiet)
                {
                    Debug.LogWarning(
                        $"MenSharp: no compiled program for {proxy.GetType().Name} yet — run "
                        + "MenSharp > Compile All (it will pair automatically afterwards).",
                        proxy);
                }
                continue;
            }

            UdonBehaviour paired = null;
            for (int index = 0; index < spare.Count; index++)
            {
                if (spare[index].programSource == program)
                {
                    paired = spare[index];
                    spare.RemoveAt(index);
                    break;
                }
            }
            if (paired == null)
            {
                // a spare whose program asset was deleted (a rename, or a
                // compiler update): reuse it, so its interaction settings and
                // serialized values survive the migration
                for (int index = 0; index < spare.Count; index++)
                {
                    if (spare[index].programSource == null)
                    {
                        paired = spare[index];
                        spare.RemoveAt(index);
                        if (undoable)
                        {
                            Undo.RecordObject(paired, "Pair MenSharp behaviour");
                        }
                        paired.programSource = program;
                        EditorUtility.SetDirty(paired);
                        break;
                    }
                }
            }
            if (paired == null)
            {
                paired = undoable
                    ? Undo.AddComponent<UdonBehaviour>(target)
                    : target.AddComponent<UdonBehaviour>();
                paired.programSource = program;
                EditorUtility.SetDirty(paired);
            }
            ApplyVisibility(paired);
            ApplySyncMode(paired, program);
            ApplySerializedProgram(paired, program);
            pairs.Add((proxy, paired));
        }

        // whatever is left over belongs to a proxy that no longer exists (or
        // no longer runs that program): it would keep executing, invisibly
        foreach (UdonBehaviour leftover in spare)
        {
            string program = leftover.programSource == null
                ? "<missing>"
                : leftover.programSource.name;
            Debug.Log(
                $"MenSharp: removed the leftover Udon program `{program}` from "
                + $"{target.name} — no MenSharp component on it uses that program any more.",
                target);
            if (undoable)
            {
                Undo.DestroyObjectImmediate(leftover);
            }
            else
            {
                UnityEngine.Object.DestroyImmediate(leftover);
            }
        }

        return pairs;
    }

    /// An UdonBehaviour caches, in the scene, which serialized program it
    /// loads — separately from the program *source* it points at. Recompiling
    /// can give the source a new serialized asset, and then the two disagree:
    /// the inspector shows the new program while the VM runs the old one, with
    /// nothing anywhere saying so. UdonSharp repairs the same field for the
    /// same reason.
    private static readonly FieldInfo SerializedProgramField = typeof(UdonBehaviour)
        .GetField("serializedProgramAsset", BindingFlags.NonPublic | BindingFlags.Instance);

    private static void ApplySerializedProgram(UdonBehaviour udon, MenSharpProgramAsset program)
    {
        if (SerializedProgramField == null)
        {
            return;
        }
        UnityEngine.Object wanted = program.SerializedProgramAsset;
        if (wanted == null)
        {
            return;
        }
        var current = SerializedProgramField.GetValue(udon) as UnityEngine.Object;
        if (current == wanted)
        {
            return;
        }
        SerializedProgramField.SetValue(udon, wanted);
        EditorUtility.SetDirty(udon);
    }

    /// `[UdonBehaviourSyncMode(...)]` is a setting on the component, not part
    /// of the program, so the compiler can only ask for it — this is where it
    /// takes effect.
    private static void ApplySyncMode(UdonBehaviour udon, MenSharpProgramAsset program)
    {
        string wanted = program.SyncMode;
        if (wanted == null)
        {
            return;
        }
        Networking.SyncType mode;
        switch (wanted)
        {
            case "manual": mode = Networking.SyncType.Manual; break;
            case "none": mode = Networking.SyncType.None; break;
            default: mode = Networking.SyncType.Continuous; break;
        }
        if (udon.SyncMethod == mode)
        {
            return;
        }
        udon.SyncMethod = mode;
        EditorUtility.SetDirty(udon);
    }

    private static void ApplyVisibility(UdonBehaviour udon)
    {
        HideFlags wanted = Reveal
            ? udon.hideFlags & ~HideFlags.HideInInspector
            : udon.hideFlags | HideFlags.HideInInspector;
        if (udon.hideFlags == wanted)
        {
            return;
        }
        udon.hideFlags = wanted;
        EditorUtility.SetDirty(udon);
        // the inspector does not notice hideFlags changes on its own
        EditorApplication.delayCall += () =>
        {
            UnityEditorInternal.InternalEditorUtility.RepaintAllViews();
        };
    }

    // --------------------------------------------------------------- transfer

    /// The UdonBehaviour behind a referenced proxy. A null here means the
    /// reference will do nothing at runtime, which is worth saying out loud.
    private static UdonBehaviour PairedOrWarn(
        MenSharpBehaviour other, MenSharpBehaviour from, string field)
    {
        if (other == null)
        {
            return null;
        }
        UdonBehaviour paired = FindPaired(other);
        if (paired == null)
        {
            Debug.LogWarning(
                $"MenSharp: {from.GetType().Name}.{field} points at {other.GetType().Name} on "
                + $"{other.gameObject.name}, which has no compiled program paired yet — the "
                + "reference will be empty. Compile, then re-enter play mode.",
                from);
        }
        return paired;
    }

    /// The UdonBehaviour UdonSharp keeps behind one of its proxies — what an
    /// M# program can actually talk to.
    private static UdonBehaviour BackingOrWarn(
        UdonSharp.UdonSharpBehaviour other, MenSharpBehaviour from, string field)
    {
        if (other == null)
        {
            return null;
        }
        UdonBehaviour backing = UdonSharpEditor.UdonSharpEditorUtility.GetBackingUdonBehaviour(other);
        if (backing == null)
        {
            Debug.LogWarning(
                $"MenSharp: {from.GetType().Name}.{field} points at the UdonSharp behaviour "
                + $"{other.GetType().Name} on {other.gameObject.name}, which has no backing "
                + "UdonBehaviour yet — the reference will be empty. Let UdonSharp compile it "
                + "(it needs a program asset), then re-enter play mode.",
                from);
            return null;
        }
        // UdonSharp compiles on its own schedule: a program asset it has not
        // compiled yet has no program, and every call into it would be a
        // silent no-op — say so instead
        var program = backing.programSource as UdonSharp.UdonSharpProgramAsset;
        if (backing.programSource == null || (program != null && program.SerializedProgramAsset == null))
        {
            Debug.LogWarning(
                $"MenSharp: {from.GetType().Name}.{field} points at the UdonSharp behaviour "
                + $"{other.GetType().Name} on {other.gameObject.name}, whose program UdonSharp "
                + "has not compiled yet — calls into it will do nothing. Run VRChat SDK > "
                + "Udon Sharp > Compile All UdonSharp Programs, then re-enter play mode.",
                from);
        }
        return backing;
    }

    /// The fields Unity serializes on the proxy — public ones plus
    /// `[SerializeField]`, minus `[NonSerialized]` — which is also exactly
    /// what the compiler exports. Walked class by class because GetFields
    /// never returns a base class's private fields.
    private static IEnumerable<FieldInfo> SerializedFields(Type type)
    {
        for (Type current = type;
            current != null
                && current != typeof(MenSharpBehaviour)
                && current != typeof(MonoBehaviour);
            current = current.BaseType)
        {
            foreach (FieldInfo field in current.GetFields(
                BindingFlags.Public | BindingFlags.NonPublic
                | BindingFlags.Instance | BindingFlags.DeclaredOnly))
            {
                if (field.IsDefined(typeof(NonSerializedAttribute), false))
                {
                    continue;
                }
                if (!field.IsPublic && !field.IsDefined(typeof(SerializeField), false))
                {
                    continue;
                }
                // what Unity never serializes, and the compiler never exports:
                // delegates (a program's own code addresses) and nullable
                // value types (a boxed value or null)
                if (typeof(Delegate).IsAssignableFrom(field.FieldType)
                    || Nullable.GetUnderlyingType(field.FieldType) != null)
                {
                    continue;
                }
                yield return field;
            }
        }
    }

    /// Copies the proxy's serialized instance fields into the UdonBehaviour's
    /// public variable table — the values the Udon heap starts from.
    public static void TransferValues(MenSharpBehaviour proxy, UdonBehaviour udon)
    {
        IUdonVariableTable table = udon.publicVariables;
        var summary = new System.Text.StringBuilder();
        foreach (FieldInfo field in SerializedFields(proxy.GetType()))
        {
            object value = field.GetValue(proxy);
            Type valueType = field.FieldType;
            // what you drag in is a proxy component; what the program can talk
            // to is the UdonBehaviour paired with it
            if (typeof(MenSharpBehaviour).IsAssignableFrom(valueType))
            {
                value = PairedOrWarn(value as MenSharpBehaviour, proxy, field.Name);
                valueType = typeof(UdonBehaviour);
            }
            else if (valueType.IsArray
                && typeof(MenSharpBehaviour).IsAssignableFrom(valueType.GetElementType()))
            {
                var source = (Array)value;
                var mapped = new UdonBehaviour[source == null ? 0 : source.Length];
                for (int index = 0; index < mapped.Length; index++)
                {
                    mapped[index] = PairedOrWarn(
                        source.GetValue(index) as MenSharpBehaviour, proxy, field.Name);
                }
                value = mapped;
                valueType = typeof(UdonBehaviour[]);
            }
            // an UdonSharp behaviour is a proxy too, over the UdonBehaviour
            // UdonSharp keeps behind it
            else if (typeof(UdonSharp.UdonSharpBehaviour).IsAssignableFrom(valueType))
            {
                value = BackingOrWarn(value as UdonSharp.UdonSharpBehaviour, proxy, field.Name);
                valueType = typeof(UdonBehaviour);
            }
            else if (valueType.IsArray
                && typeof(UdonSharp.UdonSharpBehaviour).IsAssignableFrom(valueType.GetElementType()))
            {
                var source = (Array)value;
                var mapped = new UdonBehaviour[source == null ? 0 : source.Length];
                for (int index = 0; index < mapped.Length; index++)
                {
                    mapped[index] = BackingOrWarn(
                        source.GetValue(index) as UdonSharp.UdonSharpBehaviour, proxy, field.Name);
                }
                value = mapped;
                valueType = typeof(UdonBehaviour[]);
            }
            table.RemoveVariable(field.Name);
            Type variableType = typeof(UdonVariable<>).MakeGenericType(valueType);
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

/// Runs for every scene on play-mode entry and on world builds: sync,
/// transfer, strip.
public class MenSharpSceneProcessor : IProcessSceneWithReport
{
    public int callbackOrder => -10_000;

    public void OnProcessScene(Scene scene, BuildReport report)
    {
        // the same target set the editor sweep uses, orphans included: a
        // leftover on a GameObject with no proxy would otherwise sail straight
        // into play mode and run
        MenSharpProxy.SyncThenTransfer(MenSharpProxy.PairingTargets(scene), undoable: false);

        foreach (GameObject root in scene.GetRootGameObjects())
        {
            foreach (MenSharpBehaviour proxy in
                root.GetComponentsInChildren<MenSharpBehaviour>(true))
            {
                UnityEngine.Object.DestroyImmediate(proxy);
            }
        }
    }
}
#endif

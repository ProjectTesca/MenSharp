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
//     inspector-edited values and scene references reach the Udon heap); a
//     build then strips the proxy component so it can never double-execute,
//     and play mode disables it and keeps it as the inspector's live view of
//     the program (ReadBack / WriteLive below).

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
        // Unity is still recording its own "Add <component>" undo step while
        // this runs, and that step's after-state takes in everything on the
        // GameObject at the time it closes — the backing UdonBehaviour
        // included, so one Ctrl+Z removes both and Ctrl+Y brings both back.
        // Registering an undo step of our own here would nest a second
        // record inside Unity's: undoing them in turn restored the proxy from
        // the inner record's before-state as a half-dead object — unpaired,
        // and with no Remove Component.
        ObjectFactory.componentWasAdded += component =>
        {
            if (component is MenSharpBehaviour proxy)
            {
                SyncPairs(proxy.gameObject, quiet: false, undoable: false);
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
        // componentWasAdded may not fire) — the pairing is self-repairing.
        // Repairs made on load are not undo steps: nothing the user did is
        // being undone. Those made after a hierarchy change join the step
        // that changed it (see ScheduleSweep).
        EditorApplication.delayCall += () => SyncAllInOpenScenes(undoable: false);
        UnityEditor.SceneManagement.EditorSceneManager.sceneOpened +=
            (_, _) => SyncAllInOpenScenes(undoable: false);
        EditorApplication.hierarchyChanged += () => ScheduleSweep(repair: false);
        Undo.undoRedoPerformed += () => ScheduleSweep(repair: true);
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
            SyncAllInOpenScenes(undoable: false);
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
    private static bool sweepRepairs;
    private static int sweepGroup;

    /// Runs the sync once the current editor event is over. The backing
    /// UdonBehaviours are derived from the proxies, so what the sweep does
    /// must never be an undo step of its own: Ctrl+Z would then remove only
    /// the UdonBehaviour, the next sweep would put it back, and the user's
    /// own change could never be undone. So either the sweep's changes are
    /// collapsed into the undo step that changed the hierarchy (a proxy
    /// dropped onto a GameObject, a component removed), or — after an undo
    /// or redo, where the scene is expected to be consistent already and
    /// anything left to fix is not the user's doing — they are made without
    /// an undo record at all (`repair`).
    private static void ScheduleSweep(bool repair)
    {
        if (sweepPending)
        {
            sweepRepairs |= repair;
            return;
        }
        sweepPending = true;
        sweepRepairs = repair;
        sweepGroup = Undo.GetCurrentGroup();
        EditorApplication.delayCall += RunPendingSweep;
    }

    /// The sweep itself; separate from the scheduling so tests can run it
    /// without waiting for the editor loop.
    private static void RunPendingSweep()
    {
        if (!sweepPending)
        {
            return;
        }
        sweepPending = false;
        if (sweepRepairs)
        {
            SyncAllInOpenScenes(undoable: false);
            return;
        }
        SyncAllInOpenScenes(undoable: true);
        Undo.CollapseUndoOperations(sweepGroup);
    }

    private static void SyncAllInOpenScenes(bool undoable)
    {
        if (EditorApplication.isPlayingOrWillChangePlaymode)
        {
            return;
        }
        foreach (GameObject target in PairingTargetsInOpenScenes())
        {
            SyncPairs(target, quiet: true, undoable: undoable);
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
        WireStatics(pairs, undoable);
    }

    // ---------------------------------------------------------------- statics

    /// The scene object carrying the array every behaviour's static fields
    /// live in — see the compiler's shared statics. Udon gives each
    /// UdonBehaviour a heap of its own, so a static field would otherwise be
    /// one per instance; instead every program of a compilation shares one
    /// array, kept by a program of no code (`MenSharp.Statics`) on this
    /// object, one per scene, made here on demand. Each backing UdonBehaviour
    /// is told where it is through `__mensharp_statics`; a behaviour
    /// instantiated at run time from a prefab finds the object by its name.
    public const string StaticsHolderName = "MenSharp.Statics";
    private const string StaticsReferenceVariable = "__mensharp_statics";

    /// The holder's UdonBehaviour is not a backing behaviour: it has no proxy,
    /// and the sweep must not clear it away as an orphan.
    private static bool IsStaticsHolder(UdonBehaviour udon)
    {
        return udon != null
            && udon.programSource is MenSharpProgramAsset program
            && program.name == StaticsHolderName;
    }

    private static void WireStatics(
        List<(MenSharpBehaviour proxy, UdonBehaviour udon)> pairs, bool undoable)
    {
        var holders = new Dictionary<Scene, UdonBehaviour>();
        foreach ((MenSharpBehaviour _, UdonBehaviour udon) in pairs)
        {
            if (udon == null)
            {
                continue;
            }
            Scene scene = udon.gameObject.scene;
            if (!holders.TryGetValue(scene, out UdonBehaviour holder))
            {
                holder = EnsureStaticsHolder(scene, undoable);
                holders[scene] = holder;
            }
            if (holder != null)
            {
                SetBehaviourVariable(udon, StaticsReferenceVariable, holder);
            }
        }
    }

    /// The scene's holder, made when the scene has none yet.
    private static UdonBehaviour EnsureStaticsHolder(Scene scene, bool undoable)
    {
        MenSharpProgramAsset program = MenSharpSources.FindProgram(StaticsHolderName);
        if (program == null)
        {
            Debug.LogWarning(
                "MenSharp: the statics holder program (MenSharp.Statics) has not been "
                + "compiled yet — static fields are one per behaviour instance until the "
                + "next compile.");
            return null;
        }
        foreach (GameObject root in scene.GetRootGameObjects())
        {
            if (root.name != StaticsHolderName)
            {
                continue;
            }
            UdonBehaviour existing = root.GetComponent<UdonBehaviour>();
            if (existing == null)
            {
                continue;
            }
            if (existing.programSource != program)
            {
                if (undoable)
                {
                    Undo.RecordObject(existing, "Pair MenSharp statics holder");
                }
                existing.programSource = program;
                EditorUtility.SetDirty(existing);
            }
            ApplyVisibility(existing);
            ApplySerializedProgram(existing, program);
            PinHolderToNoSync(existing, undoable);
            root.hideFlags = Reveal ? HideFlags.None : HideFlags.HideInHierarchy;
            return existing;
        }

        var holderObject = new GameObject(StaticsHolderName);
        SceneManager.MoveGameObjectToScene(holderObject, scene);
        if (undoable)
        {
            Undo.RegisterCreatedObjectUndo(holderObject, "Add MenSharp statics holder");
        }
        holderObject.hideFlags = Reveal ? HideFlags.None : HideFlags.HideInHierarchy;
        UdonBehaviour holder = holderObject.AddComponent<UdonBehaviour>();
        holder.programSource = program;
        ApplyVisibility(holder);
        ApplySerializedProgram(holder, program);
        PinHolderToNoSync(holder, undoable);
        EditorUtility.SetDirty(holderObject);
        return holder;
    }

    /// The holder carries no synced variables and never networks — Udon
    /// statics are one array per scene, read and written locally. A fresh
    /// UdonBehaviour defaults its SyncMethod to Unknown, which the inspector
    /// shows as Continuous and asks the network manager for a slot it never
    /// uses. Pin it to None. Existing holders are corrected on the next sweep.
    private static void PinHolderToNoSync(UdonBehaviour holder, bool undoable)
    {
        if (holder == null || holder.SyncMethod == Networking.SyncType.None)
        {
            return;
        }
        if (undoable)
        {
            Undo.RecordObject(holder, "Set MenSharp statics holder sync mode");
        }
        holder.SyncMethod = Networking.SyncType.None;
        EditorUtility.SetDirty(holder);
    }

    /// The type the compiled program declared a public variable with, from
    /// its symbol table (retrieved on first use); null when there is no
    /// program to ask or the program has no such variable.
    private static Type DeclaredVariableType(UdonBehaviour udon, string name, ref IUdonSymbolTable symbols)
    {
        if (symbols == null)
        {
            var program = udon.programSource as MenSharpProgramAsset;
            symbols = program?.SerializedProgramAsset?.RetrieveProgram()?.SymbolTable;
            if (symbols == null)
            {
                return null;
            }
        }
        return symbols.HasAddressForSymbol(name) ? symbols.GetSymbolType(name) : null;
    }

    /// Sets one behaviour-typed public variable, the way TransferValues sets
    /// a proxy's fields.
    private static void SetBehaviourVariable(UdonBehaviour udon, string name, UdonBehaviour value)
    {
        IUdonVariableTable table = udon.publicVariables;
        table.RemoveVariable(name);
        if (!table.TryAddVariable(new UdonVariable<UdonBehaviour>(name, value)))
        {
            Debug.LogWarning($"MenSharp: could not set public variable {name} on {udon.name}", udon);
            return;
        }
        if (udon is UnityEngine.ISerializationCallbackReceiver receiver)
        {
            receiver.OnBeforeSerialize();
        }
        EditorUtility.SetDirty(udon);
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
        // the one it remembers, unless that now carries some other program
        // (the sweep will sort that out; until then the program decides)
        UdonBehaviour remembered = RememberedBacking(proxy);
        if (remembered != null
            && (remembered.programSource == program || remembered.programSource == null))
        {
            return remembered;
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

    private const string BackingFieldName = "menSharpBacking";

    private static readonly FieldInfo BackingField = typeof(MenSharpBehaviour).GetField(
        BackingFieldName, BindingFlags.Instance | BindingFlags.NonPublic);

    /// The UdonBehaviour a proxy remembers being paired with, when it is
    /// still a component of the proxy's own GameObject. A reference to
    /// another object's UdonBehaviour is a pasted component's leftover and
    /// counts for nothing.
    private static UdonBehaviour RememberedBacking(MenSharpBehaviour proxy)
    {
        if (BackingField == null || proxy == null)
        {
            return null;
        }
        var udon = BackingField.GetValue(proxy) as UdonBehaviour;
        if (udon == null || udon.gameObject != proxy.gameObject || IsStaticsHolder(udon))
        {
            return null;
        }
        return udon;
    }

    /// Records the pair on the proxy — through its serialized property, so
    /// that a prefab instance keeps it as an override and the undo step is
    /// the caller's (`undoable`) or none at all.
    private static void RememberBacking(MenSharpBehaviour proxy, UdonBehaviour udon, bool undoable)
    {
        if (BackingField == null || (BackingField.GetValue(proxy) as UdonBehaviour) == udon)
        {
            return;
        }
        var serialized = new SerializedObject(proxy);
        SerializedProperty property = serialized.FindProperty(BackingFieldName);
        if (property == null)
        {
            return;
        }
        property.objectReferenceValue = udon;
        if (undoable)
        {
            serialized.ApplyModifiedProperties();
        }
        else
        {
            serialized.ApplyModifiedPropertiesWithoutUndo();
        }
    }

    /// Is this an UdonBehaviour *we* placed? Only ones carrying a MenSharp
    /// program qualify, so hand-authored Udon (graphs, UdonSharp) is never
    /// touched — least of all removed. (One a proxy remembers is ours too,
    /// whatever it carries: see `SyncPairs`.)
    private static bool IsBackingBehaviour(UdonBehaviour udon)
    {
        if (udon == null || IsStaticsHolder(udon))
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
    /// `undoable` is off while a scene is being processed for play or a build
    /// (that scene is a throwaway copy, and registering undo steps against it
    /// would outlive the copy itself), inside Unity's own add-component undo
    /// step (which already covers what this changes), and for repairs that
    /// undo nothing the user did — see ScheduleSweep.
    public static List<(MenSharpBehaviour proxy, UdonBehaviour udon)> SyncPairs(
        GameObject target, bool quiet = false, bool undoable = true)
    {
        var pairs = new List<(MenSharpBehaviour, UdonBehaviour)>();
        if (target == null)
        {
            return pairs;
        }

        // what the proxies remember is ours even when nothing else says so:
        // imported from another project, the program asset it carried is
        // gone and the hidden flag may be too (saved with Reveal on)
        MenSharpBehaviour[] proxies = target.GetComponents<MenSharpBehaviour>();
        var remembered = new HashSet<UdonBehaviour>();
        foreach (MenSharpBehaviour proxy in proxies)
        {
            UdonBehaviour udon = RememberedBacking(proxy);
            if (udon != null)
            {
                remembered.Add(udon);
            }
        }
        var spare = new List<UdonBehaviour>();
        foreach (UdonBehaviour udon in target.GetComponents<UdonBehaviour>())
        {
            if (IsBackingBehaviour(udon) || remembered.Contains(udon))
            {
                spare.Add(udon);
            }
        }

        foreach (MenSharpBehaviour proxy in proxies)
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
            // the remembered one first, whatever it carries now — the
            // component uGUI events and other scripts point at — re-pointed
            // at the current program. Still in `spare` means no earlier
            // proxy (a pasted copy remembering the same one) claimed it
            UdonBehaviour kept = RememberedBacking(proxy);
            if (kept != null && spare.Remove(kept))
            {
                paired = kept;
                if (paired.programSource != program)
                {
                    if (undoable)
                    {
                        Undo.RecordObject(paired, "Pair MenSharp behaviour");
                    }
                    paired.programSource = program;
                    EditorUtility.SetDirty(paired);
                }
            }
            for (int index = 0; paired == null && index < spare.Count; index++)
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
            RememberBacking(proxy, paired, undoable);
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
    /// An enum value's underlying bits as a long: a ulong past
    /// `long.MaxValue` keeps its bit pattern, which is what the M# heap
    /// holds for it.
    private static long EnumBits(object value)
    {
        Type underlying = Enum.GetUnderlyingType(value.GetType());
        if (underlying == typeof(ulong))
        {
            return unchecked((long)Convert.ToUInt64(value));
        }
        return Convert.ToInt64(value);
    }

    /// A value for the transfer summary that never asks a Unity object to
    /// describe itself: `TextAsset.ToString` reads the asset's text, which
    /// throws on an unassigned or destroyed reference.
    private static string Describe(object value)
    {
        if (value == null)
        {
            return "null";
        }
        if (value is UnityEngine.Object unityObject)
        {
            return unityObject == null ? "null" : unityObject.name;
        }
        if (value is Array array)
        {
            return $"{value.GetType().GetElementType()?.Name}[{array.Length}]";
        }
        return value.ToString();
    }

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
    private static bool IsTuple(Type type)
    {
        return type.IsGenericType
            && type.FullName != null
            && type.FullName.StartsWith("System.ValueTuple`", StringComparison.Ordinal);
    }

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
                // delegates (a program's own code addresses), tasks (which
                // hold continuations — code addresses too), nullable value
                // types (a boxed value or null), and jagged arrays
                if (typeof(Delegate).IsAssignableFrom(field.FieldType)
                    || typeof(System.Threading.Tasks.Task).IsAssignableFrom(field.FieldType)
                    || Nullable.GetUnderlyingType(field.FieldType) != null
                    || IsTuple(field.FieldType)
                    || (field.FieldType.IsArray
                        && field.FieldType.GetElementType() is Type element
                        && element.IsArray))
                {
                    continue;
                }
                yield return field;
            }
        }
    }

    // ------------------------------------------------------------ encoding

    /// A proxy field's value as the program stores it: behaviours as their
    /// UdonBehaviours, user enums as their integers, a `List<T>` as the
    /// `object[]` the compiled class is. `valueType` is the type the heap
    /// slot takes. Shared by the edit-mode transfer and the play-mode write.
    private static object EncodeField(
        FieldInfo field, object value, MenSharpBehaviour proxy, UdonBehaviour udon,
        ref IUdonSymbolTable declared, out Type valueType)
    {
        valueType = field.FieldType;
        Type fieldType = field.FieldType;
        if (fieldType.IsGenericType && fieldType.GetGenericTypeDefinition() == typeof(List<>))
        {
            valueType = typeof(object[]);
            return EncodeList(field, value as System.Collections.IList, proxy, udon, ref declared);
        }
        if (MenSharpDictionarySerialization.IsDictionaryType(fieldType))
        {
            valueType = typeof(object[]);
            return EncodeDictionary(field, value as System.Collections.IDictionary, proxy, udon);
        }
            // an unassigned or destroyed reference is Unity's "fake null": an
            // object whose native side is gone, which anything reading it
            // throws on (TextAsset.ToString did, summarizing below). The
            // program gets a real null
            if (value is UnityEngine.Object unityValue && unityValue == null)
            {
                value = null;
            }
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
            // an enum the user declared is an Int32 on the M# heap (its
            // arrays an object[] of Int32s) — the program's own symbol table
            // says so. Handing over the C# enum instead left a slot the VM
            // could not read as Int32 (`mode == Mode.Second` halted). An
            // engine enum is declared as the boxed enum and passes as is.
            else if (valueType.IsEnum
                || (valueType.IsArray && valueType.GetElementType().IsEnum))
            {
                Type wanted = DeclaredVariableType(udon, field.Name, ref declared);
                // an enum past what an Int32 holds (uint, long, ulong
                // underlying) is an Int64 on the M# heap — a ulong as its
                // bit pattern
                if (valueType.IsEnum && wanted == typeof(int))
                {
                    value = value == null ? 0 : unchecked((int)EnumBits(value));
                    valueType = typeof(int);
                }
                else if (valueType.IsEnum && wanted == typeof(long))
                {
                    value = value == null ? 0L : EnumBits(value);
                    valueType = typeof(long);
                }
                else if (valueType.IsArray
                    && (wanted == typeof(object[]) || wanted == typeof(int[]) || wanted == typeof(long[])))
                {
                    var source = (Array)value;
                    int length = source == null ? 0 : source.Length;
                    bool wide = Enum.GetUnderlyingType(valueType.GetElementType()) is Type underlying
                        && (underlying == typeof(uint) || underlying == typeof(long) || underlying == typeof(ulong));
                    if (wanted == typeof(int[]))
                    {
                        var mapped = new int[length];
                        for (int index = 0; index < length; index++)
                        {
                            mapped[index] = unchecked((int)EnumBits(source.GetValue(index)));
                        }
                        value = mapped;
                        valueType = typeof(int[]);
                    }
                    else if (wanted == typeof(long[]))
                    {
                        var mapped = new long[length];
                        for (int index = 0; index < length; index++)
                        {
                            mapped[index] = EnumBits(source.GetValue(index));
                        }
                        value = mapped;
                        valueType = typeof(long[]);
                    }
                    else
                    {
                        var mapped = new object[length];
                        for (int index = 0; index < length; index++)
                        {
                            long bits = EnumBits(source.GetValue(index));
                            mapped[index] = wide ? (object)bits : unchecked((int)bits);
                        }
                        value = mapped;
                        valueType = typeof(object[]);
                    }
                }
            }
        return value;
    }

    /// The compiled program's name for a C# type, as the layouts spell it:
    /// `System.Collections.Generic.List`1<System.Int32>`.
    private static string LayoutName(Type type)
    {
        if (type.IsArray)
        {
            return LayoutName(type.GetElementType()) + "[]";
        }
        if (type.IsGenericType)
        {
            var arguments = new List<string>();
            foreach (Type argument in type.GetGenericArguments())
            {
                arguments.Add(LayoutName(argument));
            }
            return type.GetGenericTypeDefinition().FullName + "<" + string.Join(", ", arguments) + ">";
        }
        return type.FullName;
    }

    private static MenSharpLayout LayoutFor(UdonBehaviour udon, Type type, MenSharpBehaviour from, string field)
    {
        var program = udon.programSource as MenSharpProgramAsset;
        MenSharpLayout layout = program?.LayoutOf(LayoutName(type));
        if (layout == null)
        {
            Debug.LogWarning(
                $"MenSharp: {from.GetType().Name}.{field} is a {type.Name} the program has no "
                + "layout for (compile again?); it is left null.",
                from);
        }
        return layout;
    }

    private static MenSharpLayoutSlot SlotNamed(MenSharpLayout layout, string name)
    {
        foreach (MenSharpLayoutSlot slot in layout.slots ?? Array.Empty<MenSharpLayoutSlot>())
        {
            if (slot.field == name)
            {
                return slot;
            }
        }
        return null;
    }

    /// The element type of the array a slot stores: what the heap type says,
    /// which for a user enum or a behaviour reference is not the C# type.
    private static Type StorageElementType(MenSharpLayoutSlot slot, Type element)
    {
        switch (slot.type)
        {
            case "SystemObjectArray": return typeof(object);
            case "SystemInt32Array": return typeof(int);
            case "SystemInt64Array": return typeof(long);
            case "VRCUdonCommonInterfacesIUdonEventReceiverArray":
            case "VRCUdonUdonBehaviourArray": return typeof(UdonBehaviour);
        }
        return element;
    }

    /// One element of a collection as the program stores it.
    private static object EncodeElement(object value, Type storage, MenSharpBehaviour from, string field)
    {
        if (value is UnityEngine.Object unityValue && unityValue == null)
        {
            return null;
        }
        if (value is MenSharpBehaviour other)
        {
            return PairedOrWarn(other, from, field);
        }
        if (value is UdonSharp.UdonSharpBehaviour sharp)
        {
            return BackingOrWarn(sharp, from, field);
        }
        if (value != null && value.GetType().IsEnum && !IsEngineType(value.GetType()))
        {
            long bits = EnumBits(value);
            return storage == typeof(long) ? (object)bits : unchecked((int)bits);
        }
        return value;
    }

    private static bool IsEngineType(Type type)
    {
        string assembly = type.Assembly.GetName().Name;
        return assembly.StartsWith("UnityEngine", StringComparison.Ordinal)
            || assembly.StartsWith("VRC", StringComparison.Ordinal)
            || assembly == "mscorlib";
    }

    /// `List<T>` → the compiled class: `[type id, ..., items: T[], size]` at
    /// the indices the layout names. An empty or null list is an empty one.
    private static object EncodeList(
        FieldInfo field, System.Collections.IList list, MenSharpBehaviour proxy, UdonBehaviour udon,
        ref IUdonSymbolTable declared)
    {
        MenSharpLayout layout = LayoutFor(udon, field.FieldType, proxy, field.Name);
        MenSharpLayoutSlot itemsSlot = layout == null ? null : SlotNamed(layout, "items");
        MenSharpLayoutSlot sizeSlot = layout == null ? null : SlotNamed(layout, "size");
        if (itemsSlot == null || sizeSlot == null)
        {
            return null;
        }
        Type element = field.FieldType.GetGenericArguments()[0];
        Type storage = StorageElementType(itemsSlot, element);
        int count = list == null ? 0 : list.Count;
        Array items = Array.CreateInstance(storage, count);
        for (int index = 0; index < count; index++)
        {
            items.SetValue(EncodeElement(list[index], storage, proxy, field.Name), index);
        }
        var cell = new object[layout.size];
        cell[0] = layout.typeId;
        cell[itemsSlot.index] = items;
        cell[sizeSlot.index] = count;
        return cell;
    }

    /// `Dictionary<K, V>` → the compiled class, entries only: `keys`,
    /// `values` and `count` at the indices the layout names, no hash table
    /// — the program builds that itself on first use (its hash codes are
    /// its own; see the corlib's EnsureBuckets). A null dictionary is empty.
    private static object EncodeDictionary(
        FieldInfo field, System.Collections.IDictionary dictionary, MenSharpBehaviour proxy, UdonBehaviour udon)
    {
        MenSharpLayout layout = LayoutFor(udon, field.FieldType, proxy, field.Name);
        MenSharpLayoutSlot keysSlot = layout == null ? null : SlotNamed(layout, "keys");
        MenSharpLayoutSlot valuesSlot = layout == null ? null : SlotNamed(layout, "values");
        MenSharpLayoutSlot countSlot = layout == null ? null : SlotNamed(layout, "count");
        if (keysSlot == null || valuesSlot == null || countSlot == null)
        {
            return null;
        }
        Type[] arguments = field.FieldType.GetGenericArguments();
        Type keyStorage = StorageElementType(keysSlot, arguments[0]);
        Type valueStorage = StorageElementType(valuesSlot, arguments[1]);
        int count = dictionary == null ? 0 : dictionary.Count;
        Array keys = Array.CreateInstance(keyStorage, count);
        Array values = Array.CreateInstance(valueStorage, count);
        int index = 0;
        if (dictionary != null)
        {
            foreach (System.Collections.DictionaryEntry entry in dictionary)
            {
                keys.SetValue(EncodeElement(entry.Key, keyStorage, proxy, field.Name), index);
                values.SetValue(EncodeElement(entry.Value, valueStorage, proxy, field.Name), index);
                index++;
            }
        }
        var cell = new object[layout.size];
        cell[0] = layout.typeId;
        cell[keysSlot.index] = keys;
        cell[valuesSlot.index] = values;
        cell[countSlot.index] = count;
        MenSharpLayoutSlot freeList = SlotNamed(layout, "freeList");
        if (freeList != null)
        {
            cell[freeList.index] = -1;
        }
        MenSharpLayoutSlot freeCount = SlotNamed(layout, "freeCount");
        if (freeCount != null)
        {
            cell[freeCount.index] = 0;
        }
        return cell;
    }

    // ------------------------------------------------------------ decoding

    /// The proxy on a GameObject that a running UdonBehaviour belongs to.
    public static MenSharpBehaviour ProxyOf(UdonBehaviour udon)
    {
        if (udon == null)
        {
            return null;
        }
        foreach (MenSharpBehaviour proxy in udon.GetComponents<MenSharpBehaviour>())
        {
            if (FindPaired(proxy) == udon)
            {
                return proxy;
            }
        }
        return null;
    }

    /// A heap value as the proxy field holds it: the inverse of EncodeField.
    private static object DecodeField(FieldInfo field, object heap, MenSharpBehaviour proxy, UdonBehaviour udon)
    {
        Type fieldType = field.FieldType;
        if (fieldType.IsGenericType && fieldType.GetGenericTypeDefinition() == typeof(List<>))
        {
            return DecodeList(field, heap as object[], proxy, udon);
        }
        if (MenSharpDictionarySerialization.IsDictionaryType(fieldType))
        {
            return DecodeDictionary(field, heap as object[], proxy, udon);
        }
        if (fieldType.IsArray)
        {
            if (!(heap is Array source))
            {
                return null;
            }
            Type element = fieldType.GetElementType();
            Array mapped = Array.CreateInstance(element, source.Length);
            for (int index = 0; index < source.Length; index++)
            {
                mapped.SetValue(DecodeElement(source.GetValue(index), element), index);
            }
            return mapped;
        }
        return DecodeElement(heap, fieldType);
    }

    private static object DecodeElement(object heap, Type wanted)
    {
        if (heap == null)
        {
            return wanted.IsValueType ? Activator.CreateInstance(wanted) : null;
        }
        if (typeof(MenSharpBehaviour).IsAssignableFrom(wanted))
        {
            return heap is UdonBehaviour udon ? ProxyOf(udon) : null;
        }
        if (typeof(UdonSharp.UdonSharpBehaviour).IsAssignableFrom(wanted))
        {
            return heap is UdonBehaviour udon
                ? UdonSharpEditor.UdonSharpEditorUtility.GetProxyBehaviour(udon)
                : null;
        }
        if (wanted.IsEnum)
        {
            return heap.GetType() == wanted ? heap : Enum.ToObject(wanted, heap);
        }
        if (wanted.IsInstanceOfType(heap))
        {
            return heap;
        }
        try
        {
            return Convert.ChangeType(heap, wanted);
        }
        catch (Exception)
        {
            return wanted.IsValueType ? Activator.CreateInstance(wanted) : null;
        }
    }

    private static object DecodeList(FieldInfo field, object[] cell, MenSharpBehaviour proxy, UdonBehaviour udon)
    {
        var list = (System.Collections.IList)Activator.CreateInstance(field.FieldType);
        if (cell == null)
        {
            return list;
        }
        var program = udon.programSource as MenSharpProgramAsset;
        MenSharpLayout layout = program?.LayoutOf(LayoutName(field.FieldType));
        MenSharpLayoutSlot itemsSlot = layout == null ? null : SlotNamed(layout, "items");
        MenSharpLayoutSlot sizeSlot = layout == null ? null : SlotNamed(layout, "size");
        if (itemsSlot == null || sizeSlot == null
            || itemsSlot.index >= cell.Length || sizeSlot.index >= cell.Length)
        {
            return list;
        }
        Type element = field.FieldType.GetGenericArguments()[0];
        var items = cell[itemsSlot.index] as Array;
        int size = cell[sizeSlot.index] is int n ? n : 0;
        for (int index = 0; items != null && index < size && index < items.Length; index++)
        {
            list.Add(DecodeElement(items.GetValue(index), element));
        }
        return list;
    }

    /// The live entries of a compiled dictionary: every slot below `count`
    /// whose hash is not the removed marker (-1) — or every one, when the
    /// table was never built.
    private static object DecodeDictionary(FieldInfo field, object[] cell, MenSharpBehaviour proxy, UdonBehaviour udon)
    {
        var dictionary = (System.Collections.IDictionary)Activator.CreateInstance(field.FieldType);
        if (cell == null)
        {
            return dictionary;
        }
        var program = udon.programSource as MenSharpProgramAsset;
        MenSharpLayout layout = program?.LayoutOf(LayoutName(field.FieldType));
        MenSharpLayoutSlot keysSlot = layout == null ? null : SlotNamed(layout, "keys");
        MenSharpLayoutSlot valuesSlot = layout == null ? null : SlotNamed(layout, "values");
        MenSharpLayoutSlot countSlot = layout == null ? null : SlotNamed(layout, "count");
        MenSharpLayoutSlot hashesSlot = layout == null ? null : SlotNamed(layout, "hashes");
        if (keysSlot == null || valuesSlot == null || countSlot == null
            || keysSlot.index >= cell.Length || valuesSlot.index >= cell.Length || countSlot.index >= cell.Length)
        {
            return dictionary;
        }
        Type[] arguments = field.FieldType.GetGenericArguments();
        var keys = cell[keysSlot.index] as Array;
        var values = cell[valuesSlot.index] as Array;
        var hashes = hashesSlot != null && hashesSlot.index < cell.Length ? cell[hashesSlot.index] as int[] : null;
        int count = cell[countSlot.index] is int n ? n : 0;
        for (int index = 0; keys != null && values != null && index < count
            && index < keys.Length && index < values.Length; index++)
        {
            if (hashes != null && index < hashes.Length && hashes[index] < 0)
            {
                continue;
            }
            object key = DecodeElement(keys.GetValue(index), arguments[0]);
            if (key == null || dictionary.Contains(key))
            {
                continue;
            }
            dictionary.Add(key, DecodeElement(values.GetValue(index), arguments[1]));
        }
        return dictionary;
    }

    // ------------------------------------------------------- play-mode sync

    /// Fields already reported as unreadable or unwritable this session, so
    /// a repainting inspector does not repeat itself.
    private static readonly HashSet<string> LiveReported = new HashSet<string>(StringComparer.Ordinal);

    /// Copies the running program's variables into the proxy's fields — what
    /// the inspector shows in play mode. False when the program is not
    /// running yet.
    public static bool ReadBack(MenSharpBehaviour proxy, UdonBehaviour udon)
    {
        if (udon == null || !udon.IsInitialized)
        {
            return false;
        }
        IUdonSymbolTable declared = null;
        foreach (FieldInfo field in SerializedFields(proxy.GetType()))
        {
            if (DeclaredVariableType(udon, field.Name, ref declared) == null)
            {
                continue;
            }
            try
            {
                object heap = udon.GetProgramVariable(field.Name);
                field.SetValue(proxy, DecodeField(field, heap, proxy, udon));
            }
            catch (Exception error)
            {
                if (LiveReported.Add(proxy.GetType().Name + "." + field.Name + "<"))
                {
                    Debug.LogWarning(
                        $"MenSharp: could not read {field.Name} back from the running program "
                        + $"({error.Message}); the inspector keeps its last value.",
                        proxy);
                }
            }
        }
        return true;
    }

    /// Writes the proxy's fields into the running program — what an edit in
    /// the play-mode inspector does.
    public static bool WriteLive(MenSharpBehaviour proxy, UdonBehaviour udon)
    {
        if (udon == null || !udon.IsInitialized)
        {
            return false;
        }
        IUdonSymbolTable declared = null;
        foreach (FieldInfo field in SerializedFields(proxy.GetType()))
        {
            if (DeclaredVariableType(udon, field.Name, ref declared) == null)
            {
                continue;
            }
            try
            {
                object value = EncodeField(field, field.GetValue(proxy), proxy, udon, ref declared, out Type _);
                udon.SetProgramVariable(field.Name, value);
            }
            catch (Exception error)
            {
                if (LiveReported.Add(proxy.GetType().Name + "." + field.Name + ">"))
                {
                    Debug.LogWarning(
                        $"MenSharp: could not write {field.Name} into the running program "
                        + $"({error.Message}).",
                        proxy);
                }
            }
        }
        return true;
    }

    /// Copies the proxy's serialized instance fields into the UdonBehaviour's
    /// public variable table — the values the Udon heap starts from. Every
    /// other entry in the table goes: the runtime writes each entry over the
    /// heap slot of the same name, whether the program exports it or not,
    /// and the SDK's UdonBehaviour inspector (shown by Reveal) adds a null
    /// entry for every exported symbol it finds no value for — which is how
    /// a `const string` on a scene saved by an earlier compiler came up null.
    public static void TransferValues(MenSharpBehaviour proxy, UdonBehaviour udon)
    {
        IUdonVariableTable table = udon.publicVariables;
        var summary = new System.Text.StringBuilder();
        var transferred = new HashSet<string>(StringComparer.Ordinal);
        // the compiled program's symbol table, fetched once and only if a
        // field needs it (see the enum case below)
        IUdonSymbolTable declared = null;
        foreach (FieldInfo field in SerializedFields(proxy.GetType()))
        {
            transferred.Add(field.Name);
            object value = EncodeField(field, field.GetValue(proxy), proxy, udon, ref declared, out Type valueType);
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
            summary.Append(field.Name).Append('=').Append(Describe(value));
        }

        foreach (string stale in new List<string>(table.VariableSymbols))
        {
            // the statics reference is wired after the transfer (WireStatics)
            if (!transferred.Contains(stale) && stale != StaticsReferenceVariable)
            {
                table.RemoveVariable(stale);
            }
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
    /// M# strips proxies and transfers values to Udon this early so anything
    /// that reads the finished UdonBehaviours can order itself after it, and
    /// anything that must run before it (e.g. resolving references M# will
    /// bake) can subtract from this value instead of copying the literal.
    public const int CallbackOrder = -10_000;

    public int callbackOrder => CallbackOrder;

    public void OnProcessScene(Scene scene, BuildReport report)
    {
        // the same target set the editor sweep uses, orphans included: a
        // leftover on a GameObject with no proxy would otherwise sail straight
        // into play mode and run
        MenSharpProxy.SyncThenTransfer(MenSharpProxy.PairingTargets(scene), undoable: false);

        // a build ships no proxy at all. Play mode keeps them, disabled — no
        // Unity message reaches a disabled component, so nothing of theirs
        // runs beside the program — as the inspector's window onto the
        // running program (see MenSharpBehaviourEditor): the same view as in
        // edit mode, over live values. Exactly what UdonSharp does.
        bool building = report != null || BuildPipeline.isBuildingPlayer;
        foreach (GameObject root in scene.GetRootGameObjects())
        {
            foreach (MenSharpBehaviour proxy in
                root.GetComponentsInChildren<MenSharpBehaviour>(true))
            {
                if (building)
                {
                    UnityEngine.Object.DestroyImmediate(proxy);
                }
                else
                {
                    proxy.enabled = false;
                }
            }
        }
    }
}
#endif

// MenSharp: the inspector for behaviour components.
//
// The backing UdonBehaviour is hidden so that values have exactly one place to
// be edited — but that also hid *which program actually runs*. This header
// puts it back: the compiled program, whether it exists, and the other
// behaviours declared in the same file (Unity's drag-and-drop only ever adds
// the class whose name matches the file name, so the rest are otherwise
// invisible).

#if UNITY_EDITOR
using System;
using System.Collections;
using System.Collections.Generic;
using System.Reflection;
using MenSharp;
using UnityEditor;
using UnityEngine;
using VRC.Udon;

[CustomEditor(typeof(MenSharpBehaviour), true)]
[CanEditMultipleObjects]
public class MenSharpBehaviourEditor : Editor
{
    /// Behaviour classes sharing a source file, by class. Cleared on every
    /// assembly reload, which is exactly when it could go stale.
    private static readonly Dictionary<Type, Type[]> SiblingCache = new();

    /// In play mode the fields are the running program's: repaint as it runs.
    public override bool RequiresConstantRepaint()
    {
        return EditorApplication.isPlaying;
    }

    public override void OnInspectorGUI()
    {
        // play mode: the proxy is disabled and kept as a window onto the
        // running program — the values shown are read from it before every
        // draw, and an edit is written straight back (UdonSharp's way)
        UdonBehaviour live = null;
        if (targets.Length == 1 && target is MenSharpBehaviour proxy)
        {
            if (EditorApplication.isPlaying)
            {
                UdonBehaviour paired = MenSharpProxy.FindPaired(proxy);
                if (MenSharpProxy.ReadBack(proxy, paired))
                {
                    live = paired;
                    EditorGUILayout.HelpBox(
                        "Play mode: these are the running program's values. An edit is "
                        + "written into the program right away.",
                        MessageType.Info);
                }
                else
                {
                    EditorGUILayout.HelpBox(
                        "Play mode: the program is not running yet; these are the values it "
                        + "will start from.",
                        MessageType.None);
                }
            }
            DrawProgramHeader(proxy);
        }
        else
        {
            EditorGUILayout.HelpBox(
                $"{targets.Length} MenSharp behaviours selected.", MessageType.None);
        }
        EditorGUILayout.Space();
        bool changed = DrawFields();
        if (targets.Length == 1 && target is MenSharpBehaviour owner)
        {
            changed |= DrawDictionaryFields(owner);
        }
        if (changed && live != null && target is MenSharpBehaviour edited)
        {
            MenSharpProxy.WriteLive(edited, live);
        }
    }

    /// Like DrawDefaultInspector, except a synced variable says so. Whether a
    /// variable reaches the other players is invisible everywhere else — the
    /// UdonBehaviour that carries the sync metadata is hidden, by design — so
    /// without this the only way to check `[UdonSynced]` took is to read the
    /// generated assembly.
    private bool DrawFields()
    {
        serializedObject.Update();
        SerializedProperty property = serializedObject.GetIterator();
        bool enterChildren = true;
        while (property.NextVisible(enterChildren))
        {
            enterChildren = false;
            if (property.propertyPath == "m_Script")
            {
                using (new EditorGUI.DisabledScope(true))
                {
                    EditorGUILayout.PropertyField(property, true);
                }
                continue;
            }
            string mode = SyncModeOfField(property);
            if (mode == null)
            {
                EditorGUILayout.PropertyField(property, true);
                continue;
            }
            // spelled the way the attribute is written, rather than an icon:
            // the label should read back as the source that produced it
            var label = new GUIContent(
                property.displayName + " (UdonSynced:" + mode + ")",
                "Network-synchronised, interpolation mode " + mode + ".");
            EditorGUILayout.PropertyField(property, label, true);
        }
        return serializedObject.ApplyModifiedProperties();
    }

    // ------------------------------------------------------- dictionaries

    /// Unity draws no editor for a Dictionary<K, V>, so this is one: a row
    /// per entry, a row to add one. The field is edited in place (the
    /// entries are serialized through MenSharpDictionaryStore), and in play
    /// mode a change is written into the program like any other.
    private readonly Dictionary<string, object> pendingKeys = new Dictionary<string, object>();
    private readonly Dictionary<string, object> pendingValues = new Dictionary<string, object>();
    private readonly HashSet<string> collapsed = new HashSet<string>();

    private bool DrawDictionaryFields(MenSharpBehaviour owner)
    {
        bool changed = false;
        foreach (FieldInfo field in MenSharpDictionarySerialization.DictionaryFields(owner.GetType()))
        {
            Type[] arguments = field.FieldType.GetGenericArguments();
            string title = ObjectNames.NicifyVariableName(field.Name);
            if (!MenSharpDictionarySerialization.IsSupportedDictionary(field.FieldType))
            {
                EditorGUILayout.LabelField(
                    title,
                    $"Dictionary<{arguments[0].Name}, {arguments[1].Name}>: not editable here");
                continue;
            }
            var dictionary = field.GetValue(owner) as IDictionary;
            if (dictionary == null)
            {
                dictionary = (IDictionary)Activator.CreateInstance(field.FieldType);
                field.SetValue(owner, dictionary);
            }
            bool open = !collapsed.Contains(field.Name);
            bool nowOpen = EditorGUILayout.Foldout(open, $"{title} ({dictionary.Count})", true);
            if (nowOpen != open)
            {
                if (nowOpen) { collapsed.Remove(field.Name); } else { collapsed.Add(field.Name); }
            }
            if (!nowOpen)
            {
                continue;
            }
            EditorGUI.indentLevel++;
            object removeKey = null;
            var updates = new List<KeyValuePair<object, object>>();
            foreach (DictionaryEntry entry in dictionary)
            {
                EditorGUILayout.BeginHorizontal();
                using (new EditorGUI.DisabledScope(true))
                {
                    DrawElement(arguments[0], entry.Key, GUILayout.MinWidth(60));
                }
                object edited = DrawElement(arguments[1], entry.Value, GUILayout.MinWidth(60));
                if (!Equals(edited, entry.Value))
                {
                    updates.Add(new KeyValuePair<object, object>(entry.Key, edited));
                }
                if (GUILayout.Button("−", GUILayout.Width(22)))
                {
                    removeKey = entry.Key;
                }
                EditorGUILayout.EndHorizontal();
            }
            // the row a new entry is typed into
            EditorGUILayout.BeginHorizontal();
            pendingKeys.TryGetValue(field.Name, out object pendingKey);
            pendingValues.TryGetValue(field.Name, out object pendingValue);
            pendingKey = DrawElement(arguments[0], pendingKey ?? DefaultOf(arguments[0]), GUILayout.MinWidth(60));
            pendingValue = DrawElement(arguments[1], pendingValue ?? DefaultOf(arguments[1]), GUILayout.MinWidth(60));
            pendingKeys[field.Name] = pendingKey;
            pendingValues[field.Name] = pendingValue;
            bool canAdd = pendingKey != null && !dictionary.Contains(pendingKey);
            using (new EditorGUI.DisabledScope(!canAdd))
            {
                if (GUILayout.Button("+", GUILayout.Width(22)))
                {
                    Undo.RecordObject(owner, "Add dictionary entry");
                    dictionary.Add(pendingKey, pendingValue);
                    pendingKeys.Remove(field.Name);
                    pendingValues.Remove(field.Name);
                    changed = true;
                }
            }
            EditorGUILayout.EndHorizontal();
            EditorGUI.indentLevel--;

            if (updates.Count > 0)
            {
                Undo.RecordObject(owner, "Edit dictionary entry");
                foreach (KeyValuePair<object, object> update in updates)
                {
                    dictionary[update.Key] = update.Value;
                }
                changed = true;
            }
            if (removeKey != null)
            {
                Undo.RecordObject(owner, "Remove dictionary entry");
                dictionary.Remove(removeKey);
                changed = true;
            }
        }
        if (changed)
        {
            EditorUtility.SetDirty(owner);
        }
        return changed;
    }

    private static object DefaultOf(Type type)
    {
        if (type == typeof(string))
        {
            return "";
        }
        return type.IsValueType ? Activator.CreateInstance(type) : null;
    }

    /// One key or value, in the editor its type gets.
    private static object DrawElement(Type type, object value, params GUILayoutOption[] options)
    {
        if (typeof(UnityEngine.Object).IsAssignableFrom(type))
        {
            return EditorGUILayout.ObjectField(value as UnityEngine.Object, type, true, options);
        }
        if (type.IsEnum)
        {
            var current = value as Enum ?? (Enum)Enum.ToObject(type, 0);
            return EditorGUILayout.EnumPopup(current, options);
        }
        if (type == typeof(string))
        {
            return EditorGUILayout.TextField(value as string ?? "", options);
        }
        if (type == typeof(bool))
        {
            return EditorGUILayout.Toggle(value is bool b && b, options);
        }
        if (type == typeof(int) || type == typeof(short) || type == typeof(byte)
            || type == typeof(sbyte) || type == typeof(ushort))
        {
            int edited = EditorGUILayout.IntField(Convert.ToInt32(value ?? 0), options);
            return Convert.ChangeType(edited, type);
        }
        if (type == typeof(uint) || type == typeof(long) || type == typeof(ulong))
        {
            long edited = EditorGUILayout.LongField(Convert.ToInt64(value ?? 0L), options);
            return Convert.ChangeType(edited, type);
        }
        if (type == typeof(float))
        {
            return EditorGUILayout.FloatField(Convert.ToSingle(value ?? 0f), options);
        }
        if (type == typeof(double) || type == typeof(decimal))
        {
            double edited = EditorGUILayout.DoubleField(Convert.ToDouble(value ?? 0d), options);
            return Convert.ChangeType(edited, type);
        }
        if (type == typeof(char))
        {
            string text = EditorGUILayout.TextField(value is char c ? c.ToString() : "", options);
            return text.Length > 0 ? text[0] : '\0';
        }
        EditorGUILayout.LabelField(value?.ToString() ?? "null", options);
        return value;
    }

    /// The sync mode `[UdonSynced]` asks for on the field behind this
    /// property, or null when it is not synced.
    private string SyncModeOfField(SerializedProperty property)
    {
        foreach (UnityEngine.Object each in targets)
        {
            var synced = FindField(each.GetType(), property.name)
                ?.GetCustomAttribute<UdonSyncedAttribute>();
            if (synced != null)
            {
                return synced.Mode.ToString();
            }
        }
        return null;
    }

    /// GetField, except it also finds a `[SerializeField]` private field
    /// declared on a base class — which the inspector does serialize and
    /// GetField never returns.
    private static FieldInfo FindField(Type type, string name)
    {
        for (Type current = type; current != null; current = current.BaseType)
        {
            FieldInfo field = current.GetField(
                name,
                BindingFlags.Public | BindingFlags.NonPublic
                | BindingFlags.Instance | BindingFlags.DeclaredOnly);
            if (field != null)
            {
                return field;
            }
        }
        return null;
    }

    private void DrawProgramHeader(MenSharpBehaviour proxy)
    {
        Type type = proxy.GetType();
        MenSharpProgramAsset program = MenSharpProxy.FindProgram(type);

        using (new EditorGUILayout.VerticalScope(EditorStyles.helpBox))
        {
            using (new EditorGUILayout.HorizontalScope())
            {
                EditorGUILayout.PrefixLabel("Udon Program");
                using (new EditorGUI.DisabledScope(true))
                {
                    EditorGUILayout.ObjectField(program, typeof(MenSharpProgramAsset), false);
                }
            }

            if (program == null)
            {
                EditorGUILayout.HelpBox(
                    $"{type.Name} has not been compiled yet, so nothing will run. "
                    + "Compile, and the component pairs itself automatically.",
                    MessageType.Warning);
                if (GUILayout.Button("Compile All"))
                {
                    MenSharpCompiler.CompileAll();
                }
            }
            else if (MenSharpProxy.FindPaired(proxy) == null)
            {
                EditorGUILayout.HelpBox(
                    "This component is not paired with an UdonBehaviour yet.",
                    MessageType.Warning);
                if (GUILayout.Button("Pair now"))
                {
                    MenSharpProxy.SyncPairs(proxy.gameObject);
                }
            }

            DrawSyncMode(program);
            DrawInteraction(proxy, program);
            DrawRelatedOnThisObject(proxy, type);
            DrawSiblings(proxy, type);
            DrawRevealToggle();
        }
    }

    /// The behaviour-wide sync mode, which lives on the (hidden) UdonBehaviour
    /// rather than in the program — and is the difference between
    /// `RequestSerialization()` mattering and doing nothing.
    private static void DrawSyncMode(MenSharpProgramAsset program)
    {
        string mode = program == null ? null : program.SyncMode;
        if (mode == null)
        {
            return;
        }
        EditorGUILayout.LabelField("Sync Mode", char.ToUpperInvariant(mode[0]) + mode.Substring(1));
        if (mode == "none")
        {
            EditorGUILayout.HelpBox(
                "Sync mode is None, so no variable on this behaviour is sent to anyone.",
                MessageType.Info);
        }
    }

    /// Interaction Text and Proximity, when the program exports `_interact`.
    /// They live on the (hidden) UdonBehaviour — this is the same section
    /// VRChat's own inspector draws on a visible one, surfaced here so hiding
    /// the pairing does not cost the setting.
    private static void DrawInteraction(MenSharpBehaviour proxy, MenSharpProgramAsset program)
    {
        if (program == null || Array.IndexOf(program.EntryPoints, "_interact") < 0)
        {
            return;
        }
        UdonBehaviour paired = MenSharpProxy.FindPaired(proxy);
        if (paired == null)
        {
            // the pairing warning above already says what to do
            return;
        }
        EditorGUI.BeginChangeCheck();
        string text = EditorGUILayout.TextField("Interaction Text", paired.interactText);
        float proximity = EditorGUILayout.Slider("Proximity", paired.proximity, 0f, 100f);
        if (EditorGUI.EndChangeCheck())
        {
            Undo.RecordObject(paired, "Edit Interaction");
            paired.interactText = text;
            paired.proximity = proximity;
            EditorUtility.SetDirty(paired);
        }
    }

    /// A behaviour and one that derives from it can both sit on one GameObject
    /// — C# has no objection, they are two components. On Udon they are also
    /// two *programs*, each with its own heap, so the base runs twice over and
    /// the public variables exist in duplicate. Legal, rarely intended, and
    /// impossible to see otherwise: say it out loud.
    private static void DrawRelatedOnThisObject(MenSharpBehaviour proxy, Type type)
    {
        var related = new List<string>();
        foreach (MenSharpBehaviour other in proxy.GetComponents<MenSharpBehaviour>())
        {
            Type otherType = other.GetType();
            if (otherType == type)
            {
                continue;
            }
            if (otherType.IsSubclassOf(type))
            {
                related.Add($"{otherType.Name} derives from this one");
            }
            else if (type.IsSubclassOf(otherType))
            {
                related.Add($"this one derives from {otherType.Name}");
            }
        }
        if (related.Count == 0)
        {
            return;
        }

        EditorGUILayout.Space(2);
        EditorGUILayout.HelpBox(
            "This GameObject also has a related behaviour on it ("
            + string.Join("; ", related)
            + "). Each component is its own Udon program with its own variables, so "
            + "both run and the inherited code runs once per component. Keep only the "
            + "one you want to run.",
            MessageType.Info);
    }

    /// Other behaviours declared in the same .cs file. Dragging the file onto
    /// a GameObject only ever adds the class named after it, so without this
    /// the others are reachable but undiscoverable.
    private static void DrawSiblings(MenSharpBehaviour proxy, Type type)
    {
        MenSharpProgramAsset own = MenSharpProxy.FindProgram(type);
        if (own != null && string.IsNullOrEmpty(own.SourcePath))
        {
            // the program predates the compiler recording its source file, so
            // the grouping below has nothing to group on. Say so: silence here
            // is indistinguishable from "this file declares nothing else".
            EditorGUILayout.Space(2);
            EditorGUILayout.HelpBox(
                "This program was built by an older MenSharp and does not record which "
                + "file it came from, so the other behaviours declared beside it cannot "
                + "be listed. Recompile to fix.",
                MessageType.Info);
            if (GUILayout.Button("Compile All"))
            {
                MenSharpCompiler.CompileAll();
            }
            return;
        }

        Type[] siblings = SiblingsOf(type);
        if (siblings.Length == 0)
        {
            return;
        }

        EditorGUILayout.Space(2);
        EditorGUILayout.LabelField(
            siblings.Length == 1
                ? "This file also declares one other behaviour:"
                : $"This file also declares {siblings.Length} other behaviours:",
            EditorStyles.miniLabel);
        foreach (Type sibling in siblings)
        {
            using (new EditorGUILayout.HorizontalScope())
            {
                // an exact match: GetComponent would answer yes for a base
                // class of a component that is already there
                bool present = false;
                foreach (MenSharpBehaviour other in
                    proxy.GetComponents<MenSharpBehaviour>())
                {
                    if (other.GetType() == sibling)
                    {
                        present = true;
                        break;
                    }
                }
                EditorGUILayout.LabelField(
                    present ? $"{sibling.Name} (on this object)" : sibling.Name);
                using (new EditorGUI.DisabledScope(present))
                {
                    if (GUILayout.Button("Add", GUILayout.Width(60f)))
                    {
                        Undo.AddComponent(proxy.gameObject, sibling);
                    }
                }
            }
        }
    }

    /// Which source file a behaviour came from is something Unity cannot
    /// answer — asking a MonoScript for its asset path only works for the
    /// class the file is named after, and the whole point here is the *other*
    /// classes. So the compiler records it in each program's sidecar, and the
    /// grouping happens over program assets instead.
    private static Type[] SiblingsOf(Type type)
    {
        if (SiblingCache.TryGetValue(type, out Type[] cached))
        {
            return cached;
        }

        MenSharpProgramAsset own = MenSharpProxy.FindProgram(type);
        string source = own == null ? null : own.SourcePath;
        if (string.IsNullOrEmpty(source))
        {
            // not compiled yet (or compiled by an older version): answer for
            // now, but do not cache — the assets appear without a domain
            // reload to clear this
            return Array.Empty<Type>();
        }

        var found = new List<Type>();
        foreach (Type other in type.Assembly.GetTypes())
        {
            if (other == type
                || other.IsAbstract
                || !typeof(MenSharpBehaviour).IsAssignableFrom(other))
            {
                continue;
            }
            MenSharpProgramAsset program = MenSharpProxy.FindProgram(other);
            if (program != null && program.SourcePath == source)
            {
                found.Add(other);
            }
        }
        found.Sort((left, right) => string.CompareOrdinal(left.Name, right.Name));

        Type[] siblings = found.ToArray();
        SiblingCache[type] = siblings;
        return siblings;
    }

    private static void DrawRevealToggle()
    {
        EditorGUILayout.Space(2);
        bool reveal = EditorGUILayout.ToggleLeft(
            "Show the backing UdonBehaviour", MenSharpProxy.Reveal);
        if (reveal != MenSharpProxy.Reveal)
        {
            MenSharpProxy.Reveal = reveal;
        }
        if (reveal)
        {
            EditorGUILayout.HelpBox(
                "The backing UdonBehaviour is for inspection only: its public variables "
                + "are overwritten from this component when you enter play mode or build.",
                MessageType.Info);
        }
    }
}
#endif

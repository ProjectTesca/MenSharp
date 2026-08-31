// MenSharp: the program asset type. MUST live in a file named
// `MenSharpProgramAsset.cs` — Unity binds ScriptableObject assets to their
// script by matching the class name against the file name.
//
// Copy into the Unity project at Assets/Editor/MenSharpProgramAsset.cs,
// together with MenSharpProgramImporter.cs.
//
// Why this type exists: `UdonAssemblyProgramAsset` re-assembles its text and
// overwrites the serialized program every time the inspector draws it — so
// the initial heap values (which `.uasm` text cannot express; they ride in
// `.meta.json`) must be re-applied after *every* assembly, not once at
// import. This subclass carries the meta JSON and hooks the refresh path —
// the same architecture UdonSharp uses for its program assets.

#if UNITY_EDITOR
using System;
using UnityEngine;
using VRC.Udon.Common.Interfaces;
using VRC.Udon.Editor.ProgramSources;

/// An Udon assembly program plus the MenSharp heap-initialisation sidecar,
/// re-applied on every (re)assembly.
public class MenSharpProgramAsset : UdonAssemblyProgramAsset
{
    [SerializeField]
    public string metaJson;

    protected override void RefreshProgramImpl()
    {
        base.RefreshProgramImpl();
        ApplyMenSharpMeta();
    }

    /// The .cs file this program came from, or null when unknown (an asset
    /// imported before the compiler started recording it).
    public string SourcePath
    {
        get
        {
            if (string.IsNullOrEmpty(metaJson))
            {
                return null;
            }
            var meta = JsonUtility.FromJson<MenSharpMeta>(metaJson);
            return string.IsNullOrEmpty(meta?.source) ? null : meta.source;
        }
    }

    /// The behaviour-wide sync mode the source asked for, or null.
    public string SyncMode
    {
        get
        {
            if (string.IsNullOrEmpty(metaJson))
            {
                return null;
            }
            var meta = JsonUtility.FromJson<MenSharpMeta>(metaJson);
            return string.IsNullOrEmpty(meta?.syncMode) ? null : meta.syncMode;
        }
    }

    public void ApplyMenSharpMeta()
    {
        IUdonProgram current = program;
        if (current == null)
        {
            // nothing to patch means the heap keeps its `null`s and the
            // sidecar's values never arrive — worth saying so
            Debug.LogWarning($"MenSharp: {name} has no assembled program to initialise", this);
            return;
        }
        if (string.IsNullOrEmpty(metaJson))
        {
            return;
        }

        // value-typed slots default to their type's default, not boxed null
        // (the disassembly view and several externs choke on nulls)
        foreach (string symbol in current.SymbolTable.GetSymbols())
        {
            uint address = current.SymbolTable.GetAddressFromSymbol(symbol);
            Type slotType = current.Heap.GetHeapVariableType(address);
            if (slotType != null
                && slotType.IsValueType
                && current.Heap.GetHeapVariable(address) == null)
            {
                current.Heap.SetHeapVariable(address, Activator.CreateInstance(slotType), slotType);
            }
        }

        var meta = JsonUtility.FromJson<MenSharpMeta>(metaJson);
        foreach (MenSharpHeapEntry entry in meta.heap)
        {
            if (!current.SymbolTable.HasAddressForSymbol(entry.name))
            {
                Debug.LogWarning($"MenSharp: no heap symbol named {entry.name}");
                continue;
            }
            uint address = current.SymbolTable.GetAddressFromSymbol(entry.name);
            object value = Decode(entry);
            if (value == null && entry.kind != "String")
            {
                Debug.LogWarning($"MenSharp: cannot decode {entry.name} ({entry.kind})");
                continue;
            }
            // the slot's declared type, not the value's: a System.Type value
            // reports itself as RuntimeType, which is not what the program
            // declared and not what the VM expects to find
            Type declared = current.Heap.GetHeapVariableType(address);
            current.Heap.SetHeapVariable(address, value, declared ?? value.GetType());
        }

        // persist the patched heap so the runtime loads the same state
        SerializedProgramAsset.StoreProgram(current);
    }

    private static object Decode(MenSharpHeapEntry entry)
    {
        switch (entry.kind)
        {
            case "Int32": return int.Parse(entry.value);
            case "UInt32": return uint.Parse(entry.value);
            case "Int64": return long.Parse(entry.value);
            case "Boolean": return bool.Parse(entry.value);
            case "Single": return float.Parse(entry.value, System.Globalization.CultureInfo.InvariantCulture);
            case "Double": return double.Parse(entry.value, System.Globalization.CultureInfo.InvariantCulture);
            case "Char": return (char)uint.Parse(entry.value);
            case "String": return entry.value;
            case "Type": return ResolveType(entry.value);
            default: return null;
        }
    }

    /// A `System.Type` by .NET full name. Udon passes a generic method's type
    /// argument as a value, so `GetComponent<Rigidbody>()` needs the real
    /// `typeof(Rigidbody)` on the heap — and only the editor can make one.
    ///
    /// Searched across the loaded assemblies rather than through
    /// `Type.GetType`, which only looks in mscorlib and the caller's own
    /// assembly and would never find UnityEngine's or VRChat's types.
    private static Type ResolveType(string fullName)
    {
        if (string.IsNullOrEmpty(fullName))
        {
            return null;
        }
        foreach (System.Reflection.Assembly assembly in
            AppDomain.CurrentDomain.GetAssemblies())
        {
            Type found = assembly.GetType(fullName, false);
            if (found != null)
            {
                return found;
            }
        }
        Debug.LogWarning($"MenSharp: no type named {fullName} is loaded");
        return null;
    }
}

[Serializable]
public class MenSharpMeta
{
    public MenSharpHeapEntry[] heap;
    public string[] entryPoints;
    /// The .cs file this program was compiled from, as the compiler was given
    /// it (so: project-relative). Unity cannot answer this — a .cs asset only
    /// ever maps to the class named after the file — so the compiler tells us.
    public string source;
    /// `continuous`, `manual` or `none` from `[UdonBehaviourSyncMode]`, or
    /// empty to leave the UdonBehaviour's own setting alone. This is a setting
    /// on the component rather than part of the program, so the pairing applies
    /// it — see MenSharpProxy.
    public string syncMode;
}

[Serializable]
public class MenSharpHeapEntry
{
    public string name;
    public string kind;
    public string value;
}
#endif

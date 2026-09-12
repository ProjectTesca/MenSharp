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
using System.Reflection;
using System.Text.RegularExpressions;
using UnityEngine;
using VRC.Udon.Common;
using VRC.Udon.Common.Interfaces;
using VRC.Udon.Editor;
using VRC.Udon.Editor.ProgramSources;
using VRC.Udon.EditorBindings;
using VRC.Udon.UAssembly.Assembler;
using VRC.Udon.UAssembly.Interfaces;

/// An Udon assembly program plus the MenSharp heap-initialisation sidecar,
/// re-applied on every (re)assembly.
public class MenSharpProgramAsset : UdonAssemblyProgramAsset
{
    [SerializeField]
    public string metaJson;

    /// The compiler's binary program (`.uprog`), from which the program is
    /// built without parsing the assembly text. Empty on assets imported by
    /// an older package; the text is the fallback either way.
    [SerializeField]
    public byte[] programBlob;

    /// Where a compile's program-asset time goes, accumulated across the
    /// programs of one compile (MenSharpCompiler resets and reports them).
    public static long AssembleMilliseconds;
    public static long MetaMilliseconds;

    protected override void RefreshProgramImpl()
    {
        var watch = System.Diagnostics.Stopwatch.StartNew();
        if (!BuildDirectly())
        {
            AssembleWithSizedHeap();
        }
        AssembleMilliseconds += watch.ElapsedMilliseconds;
        watch.Restart();
        ApplyMenSharpMeta();
        MetaMilliseconds += watch.ElapsedMilliseconds;
        // the base RefreshProgram stores `program` (with the network-calling
        // metadata) right after this returns; nothing to store here
    }

    /// The program from the text through [`MenSharpProgramBuilder`]: the
    /// SDK assembler's result, in a fraction of its time. False — with the
    /// program left for the SDK assembler — when the text is not the
    /// compiler's own shape or the SDK's type resolver is not reachable.
    private bool BuildDirectly()
    {
        IUAssemblyTypeResolver resolver = TypeResolver();
        if (resolver == null)
        {
            return false;
        }
        try
        {
            program = programBlob != null && programBlob.Length > 0
                ? MenSharpProgramBuilder.Build(programBlob, resolver)
                : MenSharpProgramBuilder.Build(udonAssembly, resolver);
            assemblyError = null;
        }
        catch (MenSharpProgramBuilder.FormatException e)
        {
            Debug.LogWarning($"MenSharp: {name}: {e.Message}; assembling with the SDK assembler instead", this);
            return false;
        }
        if (Environment.GetEnvironmentVariable("MENSHARP_VERIFY_PROGRAM_BUILDER") == "1")
        {
            IUdonProgram built = program;
            AssembleWithSizedHeap();
            var differences = MenSharpProgramBuilder.Differences(built, program);
            if (differences.Count > 0)
            {
                Debug.LogError($"MenSharp: {name}: program builder differs from the SDK assembler: "
                    + string.Join(", ", differences), this);
            }
            else
            {
                Debug.Log($"MenSharp: {name}: program builder verified against the SDK assembler", this);
            }
            program = built;
        }
        return true;
    }

    /// The `[NetworkCallable]` metadata from the sidecar, in the SDK's
    /// shape: stored beside the program, read by the runtime (and ClientSim)
    /// to serialize a network event's arguments into the named variables.
    protected override VRC.SDK3.UdonNetworkCalling.NetworkCallingEntrypointMetadata[] GetLastNetworkCallingMetadata()
    {
        if (string.IsNullOrEmpty(metaJson))
        {
            return null;
        }
        MenSharpMeta meta;
        try
        {
            meta = JsonUtility.FromJson<MenSharpMeta>(metaJson);
        }
        catch (Exception)
        {
            return null;
        }
        if (meta?.networkCallable == null || meta.networkCallable.Length == 0)
        {
            return null;
        }
        var entries = new System.Collections.Generic.List<VRC.SDK3.UdonNetworkCalling.NetworkCallingEntrypointMetadata>();
        foreach (MenSharpNetworkCallable callable in meta.networkCallable)
        {
            var parameters = new System.Collections.Generic.List<VRC.SDK3.UdonNetworkCalling.NetworkCallingParameterMetadata>();
            bool complete = true;
            foreach (MenSharpNetworkParameter parameter in callable.parameters ?? new MenSharpNetworkParameter[0])
            {
                Type type = ResolveType(parameter.type);
                if (type == null)
                {
                    Debug.LogError(
                        $"MenSharp: the network callable `{callable.@event}` has a parameter of type "
                        + $"{parameter.type}, which is not loaded; the event is left without metadata.",
                        this);
                    complete = false;
                    break;
                }
                parameters.Add(new VRC.SDK3.UdonNetworkCalling.NetworkCallingParameterMetadata(parameter.name, type));
            }
            if (!complete)
            {
                continue;
            }
            var attribute = callable.maxEventsPerSecond > 0
                ? new VRC.SDK3.UdonNetworkCalling.NetworkCallableAttribute(callable.maxEventsPerSecond)
                : new VRC.SDK3.UdonNetworkCalling.NetworkCallableAttribute();
            entries.Add(new VRC.SDK3.UdonNetworkCalling.NetworkCallingEntrypointMetadata(
                callable.@event, attribute, parameters.ToArray()));
        }
        return entries.ToArray();
    }

    // ----------------------------------------------------------- assembly

    // The SDK's shared assembler (`UdonEditorManager.Assemble`) builds every
    // program on a heap of 512 slots — enough for graphs, not for a compiled
    // program whose every temporary and monomorphized corlib class is a
    // slot. So the text is assembled here on a heap sized to its `.data`
    // section, the way UdonSharp does: its own UAssemblyAssembler with a
    // heap factory it controls, and the SDK's type resolvers borrowed from
    // an UdonEditorInterface (they are not otherwise reachable).

    private static UAssemblyAssembler assembler;
    private static SizedHeapFactory heapFactory;

    private sealed class SizedHeapFactory : IUdonHeapFactory
    {
        public uint HeapSize { get; set; }
        public IUdonHeap ConstructUdonHeap() => new UdonHeap(HeapSize);
        public IUdonHeap ConstructUdonHeap(uint heapSize) => new UdonHeap(HeapSize);
    }

    private static readonly Regex DataSymbol =
        new Regex(@"^\s*[^\s:]+:\s*%", RegexOptions.Multiline);
    private static readonly Regex ExternSignature =
        new Regex(@"^\s*EXTERN,\s*""([^""]+)""", RegexOptions.Multiline);

    /// How many heap slots the assembly needs: one per `name: %Type, value`
    /// line of its `.data` section, plus one per distinct EXTERN signature —
    /// the assembler interns each signature string as an anonymous heap
    /// variable of its own (UdonSharp counts the same way).
    public static uint HeapSlotsOf(string assembly)
    {
        if (string.IsNullOrEmpty(assembly))
        {
            return 0;
        }
        int start = assembly.IndexOf(".data_start", StringComparison.Ordinal);
        int end = assembly.IndexOf(".data_end", StringComparison.Ordinal);
        if (start < 0 || end < start)
        {
            return 0;
        }
        int symbols = DataSymbol.Matches(assembly.Substring(start, end - start)).Count;
        var externs = new System.Collections.Generic.HashSet<string>();
        foreach (Match match in ExternSignature.Matches(assembly.Substring(end)))
        {
            externs.Add(match.Groups[1].Value);
        }
        return (uint)(symbols + externs.Count);
    }

    private static IUAssemblyTypeResolver typeResolver;

    /// The SDK's type resolver (Udon type name → System.Type), borrowed from
    /// its editor interface; null while that is not ready.
    private static IUAssemblyTypeResolver TypeResolver()
    {
        if (typeResolver != null)
        {
            return typeResolver;
        }
        UdonEditorInterface editorInterface = SharedEditorInterface();
        if (editorInterface == null)
        {
            return null;
        }
        FieldInfo group = typeof(UdonEditorInterface).GetField(
            "_typeResolverGroup",
            BindingFlags.NonPublic | BindingFlags.Instance);
        if (!(group?.GetValue(editorInterface) is IUAssemblyTypeResolver resolver))
        {
            Debug.LogWarning(
                "MenSharp: cannot size the Udon heap on this SDK (UdonEditorInterface has no "
                + "_typeResolverGroup); programs over 512 heap slots will fail to assemble");
            return null;
        }
        typeResolver = resolver;
        return resolver;
    }

    private static UAssemblyAssembler SizedAssembler()
    {
        if (assembler != null)
        {
            return assembler;
        }
        IUAssemblyTypeResolver resolver = TypeResolver();
        if (resolver == null)
        {
            return null;
        }
        heapFactory = new SizedHeapFactory();
        assembler = new UAssemblyAssembler(heapFactory, resolver);
        return assembler;
    }

    /// The SDK's own `UdonEditorInterface` — the one `UdonEditorManager`
    /// assembles with — rather than a second one of our own. Its constructor
    /// registers every node-registry type it can find, and in the window
    /// right after a script reload that enumeration can meet a type twice
    /// and throw ("An item with the same key has already been added");
    /// the SDK built its copy when that was safe, so it is the one to use.
    /// Null when even that is not available yet: the caller assembles
    /// with the SDK's shared assembler for this round.
    private static UdonEditorInterface SharedEditorInterface()
    {
        try
        {
            FieldInfo field = typeof(UdonEditorManager).GetField(
                "_udonEditorInterface",
                BindingFlags.NonPublic | BindingFlags.Instance);
            if (field?.GetValue(UdonEditorManager.Instance) is Lazy<UdonEditorInterface> shared)
            {
                return shared.Value;
            }
            return new UdonEditorInterface();
        }
        catch (ArgumentException e)
        {
            Debug.LogWarning(
                "MenSharp: the Udon editor interface is not ready yet (" + e.Message
                + "); this program assembles with the SDK's shared assembler for now and is "
                + "sized properly on the next compile");
            return null;
        }
    }

    private void AssembleWithSizedHeap()
    {
        try
        {
            UAssemblyAssembler sized = SizedAssembler();
            if (sized == null)
            {
                // no sized assembler this round (the SDK moved its resolver
                // group, or its editor interface is not ready yet): the
                // shared assembler works up to 512 slots
                program = UdonEditorManager.Instance.Assemble(udonAssembly);
            }
            else
            {
                // a few spare slots: the assembler itself may declare some
                heapFactory.HeapSize = HeapSlotsOf(udonAssembly) + 8;
                program = sized.Assemble(udonAssembly);
            }
            assemblyError = null;
        }
        catch (Exception e)
        {
            program = null;
            assemblyError = e.Message;
            Debug.LogException(e);
        }
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

    /// The events this program exports (`_start`, `_interact`, ...).
    public string[] EntryPoints
    {
        get
        {
            if (string.IsNullOrEmpty(metaJson))
            {
                return Array.Empty<string>();
            }
            var meta = JsonUtility.FromJson<MenSharpMeta>(metaJson);
            return meta?.entryPoints ?? Array.Empty<string>();
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

        // the caller (RefreshProgram) stores the patched heap so the runtime
        // loads the same state
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
            case "Enum": return DecodeEnum(entry.value);
            case "EnumArray": return DecodeEnumArray(entry.value);
            default: return null;
        }
    }

    /// `Namespace.EnumType#value` → the real boxed enum value. An `Int32` in
    /// an enum-typed slot would throw the moment an extern unboxes it, so the
    /// value has to be built here, where the type exists.
    private static object DecodeEnum(string encoded)
    {
        int separator = encoded.LastIndexOf('#');
        if (separator < 0)
        {
            return null;
        }
        Type enumType = ResolveType(encoded.Substring(0, separator));
        if (enumType == null || !long.TryParse(encoded.Substring(separator + 1), out long value))
        {
            return null;
        }
        return Enum.ToObject(enumType, value);
    }

    /// `Namespace.EnumType#length` → the boxed values `0..length` of the
    /// enum, indexed by value: how a program turns a number into an enum,
    /// Udon having no `Enum.ToObject` of its own.
    private static object DecodeEnumArray(string encoded)
    {
        int separator = encoded.LastIndexOf('#');
        if (separator < 0)
        {
            return null;
        }
        Type enumType = ResolveType(encoded.Substring(0, separator));
        if (enumType == null || !int.TryParse(encoded.Substring(separator + 1), out int length))
        {
            return null;
        }
        Array values = Array.CreateInstance(enumType, length);
        for (int index = 0; index < length; index++)
        {
            values.SetValue(Enum.ToObject(enumType, index), index);
        }
        return values;
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
    /// The value in heap slot 0 (slot 1 holds the program name): what the
    /// Udon VM's halt report shows first in its heap dump, and so what the
    /// runtime log watcher uses to find this asset. 0 when absent.
    public long programId;
    /// Code address → source position, in address order. The watcher maps the
    /// report's program counter to the last entry at or before it.
    public MenSharpSourceMark[] lines;
    /// `[NetworkCallable]` events: what the SDK needs to carry their
    /// arguments over the network — the variable each argument arrives in
    /// and its type. Handed to the SDK when the program is stored.
    public MenSharpNetworkCallable[] networkCallable;
}

[Serializable]
public class MenSharpNetworkCallable
{
    public string @event;
    /// 0 for no limit given.
    public int maxEventsPerSecond;
    public MenSharpNetworkParameter[] parameters;
}

[Serializable]
public class MenSharpNetworkParameter
{
    public string name;
    /// .NET full name (`System.Int32`, `UnityEngine.Vector3`).
    public string type;
}

[Serializable]
public class MenSharpSourceMark
{
    public uint address;
    public string file;
    public int line;
    public int column;
    public string function;
    /// "" for a source position, "function" for the start of a function
    /// (no position until the next mark), "halt" for the compiler's own
    /// halt after an unhandled exception was reported.
    public string kind;
}

[Serializable]
public class MenSharpHeapEntry
{
    public string name;
    public string kind;
    public string value;
}
#endif

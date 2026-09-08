// MenSharp: builds the Udon program straight from the compiler's `.uasm`
// text, without the SDK's assembler.
//
// The SDK's UAssemblyAssembler is a general tool: a scanner, a parser and a
// visitor over every line, with a regex-heavy front end. On a compiled
// program it costs about 90 ms — more than the whole MenSharp compiler
// spends on the same behaviour. The text MenSharp writes is regular (see
// men-sharp-asm's `Program::to_uasm`): two sections, one statement per
// line, jumps already resolved to addresses. Reading it back takes a
// string split, and the program object is then assembled the way the SDK
// would: the same heap layout, symbol tables, sync metadata and byte code,
// checked against the SDK assembler by `MenSharpProgramAsset` when
// MENSHARP_VERIFY_PROGRAM_BUILDER is set.
//
// Layout, as the SDK assembler lays it out:
//   - one heap slot per `.data` symbol, in order; `null` stays null (the
//     symbol carries the declared type), `this` becomes an
//     UdonGameObjectComponentHeapReference the UdonBehaviour resolves at load;
//   - one anonymous string slot per distinct EXTERN signature, after the
//     symbols, in order of first use — the EXTERN operand is its address;
//   - byte code is big-endian: a 4-byte opcode, then a 4-byte operand for
//     PUSH, JUMP, JUMP_IF_FALSE, JUMP_INDIRECT and EXTERN;
//   - exported labels form the entry-point table, typed as the SDK types
//     them (no type); `.export`ed symbols are the symbol table's exports;
//   - `.sync name, mode` is sync metadata on `name` with a single property
//     named `this` carrying the interpolation method.
#if UNITY_EDITOR
using System;
using System.Collections.Generic;
using System.Globalization;
using VRC.Udon.Common;
using VRC.Udon.Common.Interfaces;
using VRC.Udon.UAssembly.Interfaces;

public static class MenSharpProgramBuilder
{
    private const string InstructionSet = "UDON";
    private const int InstructionSetVersion = 1;

    // VRC.Udon.VM.Common.OpCode
    private const uint OpNop = 0;
    private const uint OpPush = 1;
    private const uint OpPop = 2;
    private const uint OpJumpIfFalse = 4;
    private const uint OpJump = 5;
    private const uint OpExtern = 6;
    private const uint OpJumpIndirect = 8;
    private const uint OpCopy = 9;

    /// Udon type name → System.Type, resolved once per editor session: the
    /// resolver walks the SDK's registries, and every program asks for the
    /// same few dozen names.
    private static readonly Dictionary<string, Type> TypeCache = new Dictionary<string, Type>(StringComparer.Ordinal);

    private struct DataSymbol
    {
        public string Name;
        public string UdonType;
        public bool IsThis;
    }

    /// Thrown for text this builder does not understand; the caller falls
    /// back to the SDK assembler, which reports the problem in its own words.
    public sealed class FormatException : Exception
    {
        public FormatException(string message) : base(message) { }
    }

    /// Where a compile's build time goes, summed over its programs; the
    /// compiler's log line reports them.
    public static long ParseMilliseconds;
    public static long CodeMilliseconds;
    public static long HeapMilliseconds;
    public static long ProgramMilliseconds;

    /// The program from the compiler's binary blob (`.uprog`, see
    /// men-sharp-asm's `Program::to_blob`): the same layout as from the
    /// text, with nothing to parse.
    public static IUdonProgram Build(byte[] blob, IUAssemblyTypeResolver resolver)
    {
        if (blob == null || blob.Length < 8)
        {
            throw new FormatException("no program blob");
        }
        var watch = System.Diagnostics.Stopwatch.StartNew();
        var reader = new System.IO.BinaryReader(new System.IO.MemoryStream(blob, false), System.Text.Encoding.UTF8);
        if (reader.ReadByte() != (byte)'M' || reader.ReadByte() != (byte)'S'
            || reader.ReadByte() != (byte)'H' || reader.ReadByte() != (byte)'P')
        {
            throw new FormatException("not a MenSharp program blob");
        }
        uint version = reader.ReadUInt32();
        if (version != 1)
        {
            throw new FormatException("program blob version " + version + " is newer than this package");
        }
        string Str()
        {
            int length = (int)reader.ReadUInt32();
            return System.Text.Encoding.UTF8.GetString(reader.ReadBytes(length));
        }

        int symbolCount = (int)reader.ReadUInt32();
        var symbols = new List<DataSymbol>(symbolCount);
        var exportedSymbols = new List<string>();
        var syncs = new List<KeyValuePair<string, string>>();
        for (int i = 0; i < symbolCount; i++)
        {
            string name = Str();
            string udonType = Str();
            byte flags = reader.ReadByte();
            string sync = Str();
            symbols.Add(new DataSymbol { Name = name, UdonType = udonType, IsThis = (flags & 1) != 0 });
            if ((flags & 2) != 0)
            {
                exportedSymbols.Add(name);
            }
            if (sync.Length > 0)
            {
                syncs.Add(new KeyValuePair<string, string>(name, sync));
            }
        }
        int externCount = (int)reader.ReadUInt32();
        var externs = new List<string>(externCount);
        for (int i = 0; i < externCount; i++)
        {
            externs.Add(Str());
        }
        int entryCount = (int)reader.ReadUInt32();
        var labels = new List<IUdonSymbol>(entryCount);
        var exportedLabels = new List<string>(entryCount);
        for (int i = 0; i < entryCount; i++)
        {
            string name = Str();
            uint address = reader.ReadUInt32();
            labels.Add(new UdonSymbol(name, null, address));
            exportedLabels.Add(name);
        }
        int codeLength = (int)reader.ReadUInt32();
        byte[] code = reader.ReadBytes(codeLength);
        if (code.Length != codeLength)
        {
            throw new FormatException("truncated program blob");
        }
        ParseMilliseconds += watch.ElapsedMilliseconds;
        watch.Restart();

        IUdonProgram program = Assemble(symbols, exportedSymbols, syncs, externs, labels, exportedLabels, code, resolver);
        ProgramMilliseconds += watch.ElapsedMilliseconds;
        return program;
    }

    /// The heap, symbol tables and sync metadata around finished byte code.
    private static IUdonProgram Assemble(
        List<DataSymbol> symbols,
        List<string> exportedSymbols,
        List<KeyValuePair<string, string>> syncs,
        List<string> externs,
        List<IUdonSymbol> labels,
        List<string> exportedLabels,
        byte[] code,
        IUAssemblyTypeResolver resolver)
    {
        // a few spare slots, as the SDK path gave it: the assembler itself
        // may declare some
        var heap = new UdonHeap((uint)(symbols.Count + externs.Count + 8));
        var table = new List<IUdonSymbol>(symbols.Count);
        for (int i = 0; i < symbols.Count; i++)
        {
            DataSymbol symbol = symbols[i];
            Type type = Resolve(symbol.UdonType, resolver);
            uint address = (uint)i;
            if (symbol.IsThis)
            {
                heap.SetHeapVariable(address, new UdonGameObjectComponentHeapReference(type));
            }
            else
            {
                heap.SetHeapVariable(address, null, type);
            }
            table.Add(new UdonSymbol(symbol.Name, type, address));
        }
        for (int i = 0; i < externs.Count; i++)
        {
            heap.SetHeapVariable((uint)(symbols.Count + i), externs[i]);
        }

        var syncMetadata = new List<IUdonSyncMetadata>(syncs.Count);
        foreach (KeyValuePair<string, string> sync in syncs)
        {
            var method = (UdonSyncInterpolationMethod)Enum.Parse(typeof(UdonSyncInterpolationMethod), sync.Value, true);
            syncMetadata.Add(new UdonSyncMetadata(
                sync.Key,
                new List<IUdonSyncProperty> { new UdonSyncProperty("this", method) }));
        }

        return new UdonProgram(
            InstructionSet,
            InstructionSetVersion,
            code,
            heap,
            new UdonSymbolTable(labels, exportedLabels),
            new UdonSymbolTable(table, exportedSymbols),
            new UdonSyncMetadataTable(syncMetadata),
            0);
    }

    public static IUdonProgram Build(string assembly, IUAssemblyTypeResolver resolver)
    {
        if (assembly == null)
        {
            throw new FormatException("no assembly text");
        }
        var watch = System.Diagnostics.Stopwatch.StartNew();
        var symbols = new List<DataSymbol>();
        var exportedSymbols = new List<string>();
        var syncs = new List<KeyValuePair<string, string>>();
        var codeLines = new List<string>();
        var exportedLabels = new List<string>();

        // ---- split into the two sections; every line is one statement
        int section = 0; // 0 outside, 1 data, 2 code
        foreach (string raw in assembly.Split('\n'))
        {
            string line = raw.Trim();
            if (line.Length == 0)
            {
                continue;
            }
            switch (line)
            {
                case ".data_start": section = 1; continue;
                case ".data_end": section = 0; continue;
                case ".code_start": section = 2; continue;
                case ".code_end": section = 0; continue;
            }
            if (section == 1)
            {
                if (line.StartsWith(".export ", StringComparison.Ordinal))
                {
                    exportedSymbols.Add(line.Substring(8).Trim());
                }
                else if (line.StartsWith(".sync ", StringComparison.Ordinal))
                {
                    int comma = line.IndexOf(',');
                    if (comma < 0)
                    {
                        throw new FormatException("malformed sync: " + line);
                    }
                    syncs.Add(new KeyValuePair<string, string>(
                        line.Substring(6, comma - 6).Trim(), line.Substring(comma + 1).Trim()));
                }
                else
                {
                    // name: %Type, value
                    int colon = line.IndexOf(':');
                    int percent = line.IndexOf('%', colon < 0 ? 0 : colon);
                    int comma = percent < 0 ? -1 : line.IndexOf(',', percent);
                    if (colon < 0 || percent < 0 || comma < 0)
                    {
                        throw new FormatException("malformed data symbol: " + line);
                    }
                    string value = line.Substring(comma + 1).Trim();
                    bool isThis = value == "this";
                    if (!isThis && value != "null")
                    {
                        throw new FormatException("unexpected initial value: " + line);
                    }
                    symbols.Add(new DataSymbol
                    {
                        Name = line.Substring(0, colon).Trim(),
                        UdonType = line.Substring(percent + 1, comma - percent - 1).Trim(),
                        IsThis = isThis,
                    });
                }
            }
            else if (section == 2)
            {
                if (line.StartsWith(".export ", StringComparison.Ordinal))
                {
                    exportedLabels.Add(line.Substring(8).Trim());
                }
                else
                {
                    codeLines.Add(line);
                }
            }
            else
            {
                throw new FormatException("statement outside a section: " + line);
            }
        }

        ParseMilliseconds += watch.ElapsedMilliseconds;
        watch.Restart();
        // ---- heap layout: symbols first, then the extern strings
        var addressOf = new Dictionary<string, uint>(symbols.Count, StringComparer.Ordinal);
        for (int i = 0; i < symbols.Count; i++)
        {
            addressOf[symbols[i].Name] = (uint)i;
        }
        var externAddress = new Dictionary<string, uint>(StringComparer.Ordinal);
        var externs = new List<string>();
        foreach (string line in codeLines)
        {
            if (line.StartsWith("EXTERN,", StringComparison.Ordinal))
            {
                string signature = ExternSignature(line);
                if (!externAddress.ContainsKey(signature))
                {
                    externAddress[signature] = (uint)(symbols.Count + externs.Count);
                    externs.Add(signature);
                }
            }
        }

        // ---- byte code and labels
        var code = new List<byte>(codeLines.Count * 8);
        var labels = new List<IUdonSymbol>();
        foreach (string line in codeLines)
        {
            if (line.EndsWith(":", StringComparison.Ordinal))
            {
                // an entry point: the SDK gives label symbols no type
                labels.Add(new UdonSymbol(line.Substring(0, line.Length - 1).Trim(), null, (uint)code.Count));
                continue;
            }
            int comma = line.IndexOf(',');
            string mnemonic = comma < 0 ? line : line.Substring(0, comma).TrimEnd();
            string operand = comma < 0 ? null : line.Substring(comma + 1).Trim();
            switch (mnemonic)
            {
                case "NOP": Word(code, OpNop); break;
                case "POP": Word(code, OpPop); break;
                case "COPY": Word(code, OpCopy); break;
                case "PUSH": Word(code, OpPush); Word(code, SymbolAddress(addressOf, operand, line)); break;
                case "JUMP_INDIRECT": Word(code, OpJumpIndirect); Word(code, SymbolAddress(addressOf, operand, line)); break;
                case "JUMP": Word(code, OpJump); Word(code, Address(operand, line)); break;
                case "JUMP_IF_FALSE": Word(code, OpJumpIfFalse); Word(code, Address(operand, line)); break;
                case "EXTERN": Word(code, OpExtern); Word(code, externAddress[ExternSignature(line)]); break;
                default: throw new FormatException("unknown instruction: " + line);
            }
        }

        CodeMilliseconds += watch.ElapsedMilliseconds;
        watch.Restart();
        IUdonProgram program = Assemble(symbols, exportedSymbols, syncs, externs, labels, exportedLabels, code.ToArray(), resolver);
        ProgramMilliseconds += watch.ElapsedMilliseconds;
        return program;
    }

    private static Type Resolve(string udonType, IUAssemblyTypeResolver resolver)
    {
        if (TypeCache.TryGetValue(udonType, out Type cached))
        {
            return cached;
        }
        Type type = resolver.GetTypeFromTypeString(udonType);
        if (type == null)
        {
            throw new FormatException("Type referenced by '" + udonType + "' could not be resolved.");
        }
        TypeCache[udonType] = type;
        return type;
    }

    private static string ExternSignature(string line)
    {
        int open = line.IndexOf('"');
        int close = line.LastIndexOf('"');
        if (open < 0 || close <= open)
        {
            throw new FormatException("malformed extern: " + line);
        }
        return line.Substring(open + 1, close - open - 1);
    }

    private static uint SymbolAddress(Dictionary<string, uint> addressOf, string name, string line)
    {
        if (name == null || !addressOf.TryGetValue(name, out uint address))
        {
            throw new FormatException("unknown symbol in: " + line);
        }
        return address;
    }

    private static uint Address(string operand, string line)
    {
        if (operand != null
            && operand.StartsWith("0x", StringComparison.OrdinalIgnoreCase)
            && uint.TryParse(operand.Substring(2), NumberStyles.HexNumber, CultureInfo.InvariantCulture, out uint address))
        {
            return address;
        }
        throw new FormatException("malformed address in: " + line);
    }

    private static void Word(List<byte> code, uint value)
    {
        code.Add((byte)(value >> 24));
        code.Add((byte)(value >> 16));
        code.Add((byte)(value >> 8));
        code.Add((byte)value);
    }

    // ------------------------------------------------------------ verify

    /// Every way `built` could differ from what the SDK assembler produced
    /// for the same text; empty when they agree. Used with
    /// MENSHARP_VERIFY_PROGRAM_BUILDER=1 to prove the builder on a project.
    public static List<string> Differences(IUdonProgram built, IUdonProgram reference)
    {
        var differences = new List<string>();
        if (built.InstructionSetIdentifier != reference.InstructionSetIdentifier
            || built.InstructionSetVersion != reference.InstructionSetVersion)
        {
            differences.Add("instruction set");
        }
        byte[] a = built.ByteCode;
        byte[] b = reference.ByteCode;
        if (a.Length != b.Length)
        {
            differences.Add($"byte code length {a.Length} vs {b.Length}");
        }
        else
        {
            for (int i = 0; i < a.Length; i++)
            {
                if (a[i] != b[i])
                {
                    differences.Add($"byte code differs at 0x{i:X8}");
                    break;
                }
            }
        }
        CompareTables("symbols", built.SymbolTable, reference.SymbolTable, differences);
        CompareTables("entry points", built.EntryPoints, reference.EntryPoints, differences);
        foreach (string symbol in reference.SymbolTable.GetSymbols())
        {
            if (!built.SymbolTable.HasAddressForSymbol(symbol))
            {
                continue;
            }
            uint address = reference.SymbolTable.GetAddressFromSymbol(symbol);
            uint ours = built.SymbolTable.GetAddressFromSymbol(symbol);
            if (built.Heap.GetHeapVariableType(ours) != reference.Heap.GetHeapVariableType(address))
            {
                differences.Add($"heap type of {symbol}");
            }
            object x = built.Heap.GetHeapVariable(ours);
            object y = reference.Heap.GetHeapVariable(address);
            if ((x == null) != (y == null) || (x != null && x.GetType() != y.GetType()))
            {
                differences.Add($"heap value of {symbol}");
            }
        }
        if (built.Heap.GetHeapCapacity() < reference.Heap.GetHeapCapacity())
        {
            differences.Add($"heap capacity {built.Heap.GetHeapCapacity()} vs {reference.Heap.GetHeapCapacity()}");
        }
        var syncA = new List<string>();
        foreach (IUdonSyncMetadata metadata in built.SyncMetadataTable.GetAllSyncMetadata())
        {
            foreach (IUdonSyncProperty property in metadata.Properties)
            {
                syncA.Add(metadata.Name + "." + property.Name + "=" + property.InterpolationAlgorithm);
            }
        }
        var syncB = new List<string>();
        foreach (IUdonSyncMetadata metadata in reference.SyncMetadataTable.GetAllSyncMetadata())
        {
            foreach (IUdonSyncProperty property in metadata.Properties)
            {
                syncB.Add(metadata.Name + "." + property.Name + "=" + property.InterpolationAlgorithm);
            }
        }
        syncA.Sort(StringComparer.Ordinal);
        syncB.Sort(StringComparer.Ordinal);
        if (string.Join(";", syncA) != string.Join(";", syncB))
        {
            differences.Add("sync metadata");
        }
        return differences;
    }

    private static void CompareTables(string what, IUdonSymbolTable built, IUdonSymbolTable reference, List<string> differences)
    {
        var names = new List<string>(reference.GetSymbols());
        var ours = new List<string>(built.GetSymbols());
        names.Sort(StringComparer.Ordinal);
        ours.Sort(StringComparer.Ordinal);
        if (string.Join(";", names) != string.Join(";", ours))
        {
            differences.Add(what + ": names");
            return;
        }
        foreach (string name in names)
        {
            if (built.GetAddressFromSymbol(name) != reference.GetAddressFromSymbol(name))
            {
                differences.Add($"{what}: address of {name}");
            }
            if (built.GetSymbolType(name) != reference.GetSymbolType(name))
            {
                differences.Add($"{what}: type of {name}");
            }
        }
        var exportedA = new List<string>(built.GetExportedSymbols());
        var exportedB = new List<string>(reference.GetExportedSymbols());
        exportedA.Sort(StringComparer.Ordinal);
        exportedB.Sort(StringComparer.Ordinal);
        if (string.Join(";", exportedA) != string.Join(";", exportedB))
        {
            differences.Add(what + ": exports");
        }
    }
}
#endif

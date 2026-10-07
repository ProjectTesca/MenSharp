#if UNITY_EDITOR
using System;
using System.Reflection;
using NUnit.Framework;
using UnityEditor;
using UnityEngine;
using UnityEngine.TestTools;

/// A compiler diagnostic in the console opens the M# source it is about on a
/// double-click — not MenSharpCompiler.cs, where the entry was logged. The
/// console opens an entry's own file and line, so those are what is checked.
public class MenSharpConsoleTests
{
    /// What `men-sharp --error-format unity` writes for one error, the
    /// source excerpt marked with rich text.
    private const string Stderr =
        "expected `int`, found `double` (console probe)\n"
        + "       5 │         int a = <color=#ff6b6b><b>1.5 * 2</b></color>;\n"
        + "    \n"
        + "    <color=#ff6b6b><b>[TypeError]</b></color> expected `int`, found `double` (console probe)\n"
        + "      --> Assets/MenSharp/ConsoleProbe.cs:5:17\n"
        + "       5 │         int a = <color=#ff6b6b><b>1.5 * 2</b></color>;\n"
        + "    <color=#5ad1e6><b>[Hint]</b></color> convert explicitly with a cast\n"
        + "      Assets/MenSharp/Other.cs:9:3: declared here\n"
        + "\n";

    private static readonly Type LogEntries = typeof(Editor).Assembly.GetType("UnityEditor.LogEntries");
    private static readonly Type LogEntry = typeof(Editor).Assembly.GetType("UnityEditor.LogEntry");

    [Test]
    public void TheArrowLineIsThePosition()
    {
        Assert.IsTrue(MenSharpConsole.TryLocate(Stderr, out string file, out int line, out int column));
        Assert.AreEqual("Assets/MenSharp/ConsoleProbe.cs", file);
        Assert.AreEqual(5, line);
        Assert.AreEqual(17, column);

        // a drive letter's colon stays in the file name
        Assert.IsTrue(MenSharpConsole.TryLocate("  --> C:\\Project\\Assets\\A.cs:12:3", out file, out line, out column));
        Assert.AreEqual("C:\\Project\\Assets\\A.cs", file);
        Assert.AreEqual(12, line);
        Assert.AreEqual(3, column);

        Assert.IsFalse(MenSharpConsole.TryLocate("[MenSharp] compiler exited with 2", out _, out _, out _));
    }

    [Test]
    public void ADiagnosticEntryOpensTheSourceItIsAbout()
    {
        Assert.NotNull(LogEntries, "UnityEditor.LogEntries is gone");
        Assert.NotNull(LogEntry, "UnityEditor.LogEntry is gone");

        bool ignoring = LogAssert.ignoreFailingMessages;
        LogAssert.ignoreFailingMessages = true;
        try
        {
            MenSharpCompiler.LogDiagnostics(Stderr);
        }
        finally
        {
            LogAssert.ignoreFailingMessages = ignoring;
        }

        object entry = FindEntry("(console probe)");
        Assert.NotNull(entry, "the diagnostic is not in the console");
        Assert.AreEqual("Assets/MenSharp/ConsoleProbe.cs", Field<string>(entry, "file"));
        Assert.AreEqual(5, Field<int>(entry, "line"));
        Assert.AreEqual(17, Field<int>(entry, "column"));
        string message = Field<string>(entry, "message");
        StringAssert.StartsWith("[MenSharp] expected `int`, found `double` (console probe)", message);
        StringAssert.Contains("[Hint]", message);
        // the excerpt and hint stay in the one entry, no stack trace after them
        StringAssert.DoesNotContain("MenSharpCompiler", message);
    }

    /// The VM's halt report, turned into "halted inside ..., called from
    /// Foo at file:line:column", opens that call too.
    [Test]
    public void AHaltReportOpensTheCallThatThrew()
    {
        MenSharpCompiler.CompileAll();
        AssetDatabase.Refresh(ImportAssetOptions.ForceSynchronousImport);
        var asset = AssetDatabase.LoadAssetAtPath<MenSharpProgramAsset>(
            "Assets/MenSharp/Programs/MenSharpRuntimeSmoke.asset");
        Assert.NotNull(asset);
        MenSharpMeta meta = JsonUtility.FromJson<MenSharpMeta>(asset.metaJson);
        Assert.AreNotEqual(0, meta.programId);

        // a counter at a source mark; the watcher takes the last mark at or
        // before it, which is the last of any marks sharing that address
        MenSharpSourceMark mark = null;
        for (int index = 0; index < meta.lines.Length && mark == null; index++)
        {
            MenSharpSourceMark candidate = meta.lines[index];
            bool last = index + 1 == meta.lines.Length || meta.lines[index + 1].address != candidate.address;
            if (last && string.IsNullOrEmpty(candidate.kind) && !string.IsNullOrEmpty(candidate.file) && candidate.line > 0)
            {
                mark = candidate;
            }
        }
        Assert.NotNull(mark, "no source mark in the smoke program");

        string report =
            "An exception occurred during Udon execution, this UdonBehaviour will be halted.\n"
            + "VRC.Udon.VM.UdonVMException: An exception occurred in an UdonVM, execution will be halted. --->"
            + " VRC.Udon.VM.UdonVMException: An exception occurred during EXTERN to"
            + " 'UnityEngineTransform.__get_position__UnityEngineVector3'. --->"
            + " System.NullReferenceException: Object reference not set to an instance of an object\n"
            + $"Program Counter was at: {mark.address}\n"
            + "----------------------\n"
            + "Heap Dump:\n"
            + $"  0x00000000: {meta.programId}\n";
        MethodInfo onLog = typeof(MenSharpRuntimeLogWatcher)
            .GetMethod("OnLog", BindingFlags.NonPublic | BindingFlags.Static);
        bool ignoring = LogAssert.ignoreFailingMessages;
        LogAssert.ignoreFailingMessages = true;
        try
        {
            onLog.Invoke(null, new object[] { report, "", LogType.Error });
        }
        finally
        {
            LogAssert.ignoreFailingMessages = ignoring;
        }

        object entry = FindEntry($"called from {mark.function} at {mark.file}:{mark.line}:{mark.column}");
        Assert.NotNull(entry, "the watcher's report is not in the console");
        Assert.AreEqual(mark.file.Replace('\\', '/'), Field<string>(entry, "file"));
        Assert.AreEqual(mark.line, Field<int>(entry, "line"));
        Assert.AreEqual(mark.column, Field<int>(entry, "column"));
    }

    /// The newest console entry whose text contains `marker`.
    private static object FindEntry(string marker)
    {
        object entry = Activator.CreateInstance(LogEntry);
        MethodInfo get = LogEntries.GetMethod("GetEntryInternal", BindingFlags.Public | BindingFlags.Static);
        int count = (int)LogEntries.GetMethod("StartGettingEntries", BindingFlags.Public | BindingFlags.Static)
            .Invoke(null, Array.Empty<object>());
        try
        {
            for (int row = count - 1; row >= 0; row--)
            {
                get.Invoke(null, new[] { (object)row, entry });
                if (Field<string>(entry, "message")?.Contains(marker) == true)
                {
                    return entry;
                }
            }
            return null;
        }
        finally
        {
            LogEntries.GetMethod("EndGettingEntries", BindingFlags.Public | BindingFlags.Static)
                .Invoke(null, Array.Empty<object>());
        }
    }

    private static T Field<T>(object entry, string name)
    {
        return (T)LogEntry.GetField(name, BindingFlags.Public | BindingFlags.Instance).GetValue(entry);
    }
}
#endif

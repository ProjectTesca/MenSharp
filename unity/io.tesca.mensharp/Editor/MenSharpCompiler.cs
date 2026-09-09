// MenSharp: one-button compilation inside Unity.
//
// Workflow:
//   - M# sources live under Assets/MenSharp/ (plain .cs — Unity compiles them
//     too, which is what gives you IDE completion for free)
//   - Every class inheriting MenSharp.MenSharpBehaviour is an entry point:
//     no configuration, the compiler discovers them
//   - Menu: MenSharp > Compile All (or Ctrl+Shift+M) runs the bundled Rust
//     compiler; diagnostics land in the Console; one program asset per
//     behaviour is created/updated under Assets/MenSharp/Programs/ with a
//     stable GUID, so scene references survive recompiles.

#if UNITY_EDITOR
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Text;
using UnityEditor;
using UnityEngine;
using Debug = UnityEngine.Debug;

public static class MenSharpCompiler
{
    private const string SourceRoot = MenSharpSources.SourceRoot;
    private const string ProgramsFolder = MenSharpSources.DefaultProgramsFolder;
    private const string PackageName = MenSharpSources.PackageName;

    /// The signature of the sources and compiler the last successful compile
    /// saw — in the Library folder, so it survives domain reloads and editor
    /// restarts but never travels with the project.
    private const string LastSignaturePath = "Library/MenSharp/last-compile.txt";

    [MenuItem("MenSharp/Compile All %#m")]
    public static void CompileAll()
    {
        Compile(false, false);
    }

    /// Compile All, but every program asset is re-assembled and rewritten,
    /// changed or not — for after an SDK update, or a program asset that
    /// looks wrong.
    [MenuItem("MenSharp/Rebuild All Programs")]
    public static void RebuildAll()
    {
        Compile(true, false);
    }

    /// The automatic compile (on save, on load): does nothing when neither
    /// a source nor the compiler changed since the last successful compile.
    /// Saving a script makes Unity import it, then reload the domain, and
    /// report the import once more after the reload — one save, two
    /// notifications; this is what keeps them from becoming two compiles.
    public static void CompileIfChanged()
    {
        Compile(false, true);
    }

    private static void Compile(bool force, bool onlyIfChanged)
    {
        if (!Directory.Exists(SourceRoot))
        {
            Directory.CreateDirectory(SourceRoot);
            AssetDatabase.Refresh();
            Debug.Log(
                $"MenSharp: created {SourceRoot}. Put your .cs sources there (classes "
                + "inheriting MenSharpBehaviour become programs) and compile again.");
            return;
        }
        // UdonSharp must not read these sources (its Roslyn pass is C# 7.3)
        if (MenSharpUdonSharpIsolation.Ensure(true))
        {
            // UdonSharp picks the setting up on its next pass; nothing to wait for
        }

        // MenSharp sources are the scripts of every assembly referencing the
        // runtime, plus the source folder; everything else is a library (see
        // MenSharpSources)
        MenSharpSources.SourceSet set = MenSharpSources.Collect();
        if (set.MenSharp.Count == 0)
        {
            // An empty source set is normal just after installing the package:
            // creating Assets/MenSharp triggers another asset-postprocessor pass.
            // Keep that automatic pass quiet; an explicit menu command still
            // deserves feedback, but this is not a compilation failure.
            if (!onlyIfChanged)
            {
                Debug.LogWarning(
                    $"MenSharp: no .cs files under {SourceRoot} (or in an assembly "
                    + $"referencing {MenSharpSources.RuntimeAssembly}).");
            }
            return;
        }

        string binary = FindCompilerBinary();
        if (binary == null)
        {
            return;
        }

        string signature = SourceSignature(set, binary);
        if (onlyIfChanged && !force && signature == LastSignature())
        {
            return;
        }

        string outputDirectory = Path.Combine("Library", "MenSharp");
        Directory.CreateDirectory(outputDirectory);
        foreach (string stale in Directory.GetFiles(outputDirectory))
        {
            File.Delete(stale);
        }

        var stopwatch = Stopwatch.StartNew();
        bool compiled = RunCompiler(binary, outputDirectory, set.MenSharp.ToArray(), set.Library);
        // remembered whether it compiled or not: the errors were reported
        // once, and the next edit changes the signature anyway
        RememberSignature(signature);
        if (!compiled)
        {
            Debug.LogError("MenSharp: compilation failed.");
            return;
        }
        // the compiler process against the asset work that follows: the two
        // are tuned separately, so the log tells them apart
        long compilerMilliseconds = stopwatch.ElapsedMilliseconds;

        // one program asset per produced behaviour, beside the assembly that
        // declares it: a package ships its programs with its prefabs
        var produced = Directory.GetFiles(outputDirectory, "*.uasm");
        int updated = 0;
        int unchanged = 0;
        MenSharpProgramAsset.AssembleMilliseconds = 0;
        MenSharpProgramAsset.MetaMilliseconds = 0;
        MenSharpProgramBuilder.ParseMilliseconds = 0;
        MenSharpProgramBuilder.CodeMilliseconds = 0;
        MenSharpProgramBuilder.HeapMilliseconds = 0;
        MenSharpProgramBuilder.ProgramMilliseconds = 0;
        MenSharpImporter.LoadMilliseconds = 0;
        MenSharpImporter.CreateMilliseconds = 0;
        MenSharpImporter.RefreshMilliseconds = 0;
        MenSharpImporter.DirtyMilliseconds = 0;
        long importMilliseconds = 0;
        var current = new Dictionary<string, HashSet<string>>();
        foreach (string folder in set.ProgramsFolders)
        {
            current[folder] = new HashSet<string>();
        }
        current[ProgramsFolder] = current.TryGetValue(ProgramsFolder, out var own) ? own : new HashSet<string>();
        foreach (string uasmPath in produced)
        {
            // assets are named by the *full* class path ("Demo.Door.asset"):
            // two behaviours may share a short name across namespaces, and a
            // short-named asset would make them overwrite each other
            string classPath = Path.GetFileNameWithoutExtension(uasmPath); // "Demo.Door"
            string metaPath = Path.Combine(outputDirectory, classPath + ".meta.json");
            string folder = ProgramsFolderFor(metaPath, set);
            if (!EnsureAssetFolder(folder))
            {
                Debug.LogWarning(
                    $"MenSharp: cannot write programs into {folder} (an immutable package?); "
                    + $"{classPath} goes to {ProgramsFolder} instead.");
                folder = ProgramsFolder;
                EnsureAssetFolder(folder);
            }
            long before = stopwatch.ElapsedMilliseconds;
            MenSharpImporter.CreateOrUpdate(
                uasmPath, metaPath, $"{folder}/{classPath}.asset", force, out bool sameAsBefore);
            importMilliseconds += stopwatch.ElapsedMilliseconds - before;
            if (sameAsBefore)
            {
                unchanged++;
            }
            else
            {
                updated++;
            }
            if (!current.TryGetValue(folder, out HashSet<string> names))
            {
                names = current[folder] = new HashSet<string>();
            }
            names.Add(classPath);
        }
        foreach (KeyValuePair<string, HashSet<string>> entry in current)
        {
            DeleteProgramsWithoutABehaviour(entry.Key, entry.Value);
        }
        long beforeSave = stopwatch.ElapsedMilliseconds;
        AssetDatabase.SaveAssets();
        long saveMilliseconds = stopwatch.ElapsedMilliseconds - beforeSave;
        MenSharpSources.InvalidateProgramIndex();

        long totalMilliseconds = stopwatch.ElapsedMilliseconds;
        Debug.Log(
            $"MenSharp: compiled {produced.Length} behaviour(s) from {set.MenSharp.Count} "
            + $"file(s) (+{set.Library.Count} library file(s)) in {totalMilliseconds}ms "
            + $"(compiler {compilerMilliseconds}ms, program assets {totalMilliseconds - compilerMilliseconds}ms: "
            + $"{updated} updated, {unchanged} unchanged; build {MenSharpProgramAsset.AssembleMilliseconds}ms, "
            + $"heap init {MenSharpProgramAsset.MetaMilliseconds}ms, store "
            + $"{MenSharpImporter.RefreshMilliseconds - MenSharpProgramAsset.AssembleMilliseconds - MenSharpProgramAsset.MetaMilliseconds}ms, "
            + $"save {saveMilliseconds}ms).");
    }

    /// Every source path with its last-write time, plus the compiler's — the
    /// inputs of a compile, hashed. Equal signatures mean the same programs.
    private static string SourceSignature(MenSharpSources.SourceSet set, string binary)
    {
        var text = new StringBuilder();
        var paths = new List<string>(set.MenSharp);
        paths.AddRange(set.Library);
        paths.Sort(StringComparer.Ordinal);
        paths.Add(binary);
        foreach (string path in paths)
        {
            text.Append(path).Append('\t');
            try
            {
                text.Append(File.GetLastWriteTimeUtc(path).Ticks);
            }
            catch (Exception)
            {
                text.Append("missing");
            }
            text.Append('\n');
        }
        using (var sha = System.Security.Cryptography.SHA1.Create())
        {
            byte[] hash = sha.ComputeHash(Encoding.UTF8.GetBytes(text.ToString()));
            return BitConverter.ToString(hash).Replace("-", "");
        }
    }

    private static string LastSignature()
    {
        try
        {
            return File.Exists(LastSignaturePath) ? File.ReadAllText(LastSignaturePath).Trim() : null;
        }
        catch (Exception)
        {
            return null;
        }
    }

    private static void RememberSignature(string signature)
    {
        try
        {
            Directory.CreateDirectory(Path.GetDirectoryName(LastSignaturePath));
            File.WriteAllText(LastSignaturePath, signature);
        }
        catch (Exception)
        {
            // a signature that cannot be kept only costs a compile
        }
    }

    /// The programs folder of the assembly the program's source belongs to,
    /// read from the sidecar's `source` (the path the compiler was given).
    private static string ProgramsFolderFor(string metaPath, MenSharpSources.SourceSet set)
    {
        try
        {
            MenSharpMeta meta = JsonUtility.FromJson<MenSharpMeta>(File.ReadAllText(metaPath));
            if (meta != null && !string.IsNullOrEmpty(meta.source)
                && set.ProgramsFolderOf.TryGetValue(MenSharpSources.Normalize(meta.source), out string folder))
            {
                return folder;
            }
        }
        catch (Exception)
        {
            // an unreadable sidecar: the default folder
        }
        return ProgramsFolder;
    }

    /// Makes an asset folder exist, creating it (and the AssetDatabase's
    /// knowledge of it) when needed; false when the location is not writable.
    private static bool EnsureAssetFolder(string folder)
    {
        if (AssetDatabase.IsValidFolder(folder))
        {
            return true;
        }
        try
        {
            Directory.CreateDirectory(folder);
        }
        catch (Exception)
        {
            return false;
        }
        AssetDatabase.Refresh();
        return AssetDatabase.IsValidFolder(folder);
    }

    /// Program assets left over from behaviours the sources no longer declare.
    /// Keeping them would leave GameObjects running code that is not in the
    /// project any more — and the UdonBehaviour carrying it is hidden, so
    /// nobody would see why.
    private static void DeleteProgramsWithoutABehaviour(string folder, HashSet<string> current)
    {
        if (!Directory.Exists(folder))
        {
            return;
        }
        foreach (string file in Directory.GetFiles(folder, "*.asset"))
        {
            string name = Path.GetFileNameWithoutExtension(file);
            if (current.Contains(name))
            {
                continue;
            }
            string assetPath = $"{folder}/{Path.GetFileName(file)}";
            // only ours: anything else in this folder is the user's business
            if (AssetDatabase.LoadAssetAtPath<MenSharpProgramAsset>(assetPath) == null)
            {
                continue;
            }
            AssetDatabase.DeleteAsset(assetPath);
            Debug.Log(
                $"MenSharp: removed {name}.asset — no behaviour named {name} is declared "
                + "any more.");
        }
    }

    private static bool RunCompiler(
        string binary, string outputDirectory, string[] sources, List<string> foreign)
    {
        var arguments = new List<string>();
        foreach (string reference in ReferenceAssemblies())
        {
            arguments.Add("--reference");
            arguments.Add(reference);
        }
        foreach (string path in foreign)
        {
            arguments.Add("--udonsharp");
            arguments.Add(path);
        }
        arguments.Add("--emit-udon-all");
        arguments.Add("--out-dir");
        arguments.Add(outputDirectory);
        // diagnostics in the editor's language, in the one-line-plus-detail
        // format the console groups well (see LogDiagnostics)
        arguments.Add("--lang");
        arguments.Add(Application.systemLanguage == SystemLanguage.Japanese ? "ja" : "en");
        arguments.Add("--error-format");
        arguments.Add("unity");
        arguments.AddRange(sources);

        var info = new ProcessStartInfo
        {
            FileName = binary,
            Arguments = QuoteArguments(arguments),
            UseShellExecute = false,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            CreateNoWindow = true,
        };

        using var process = Process.Start(info);
        // both pipes drained at once: reading stdout to its end while stderr
        // sits unread deadlocks as soon as the diagnostics outgrow the pipe
        // buffer — the compiler blocks writing, this side blocks waiting for
        // an EOF that never comes, and the editor hangs on a compile with
        // many errors
        System.Threading.Tasks.Task<string> stderrTask = process.StandardError.ReadToEndAsync();
        string stdout = process.StandardOutput.ReadToEnd();
        string stderr = stderrTask.Result;
        process.WaitForExit();

        // a crash report, when there is one, is the tail of stderr from the
        // marker line on; whatever came before it is ordinary diagnostics
        int crash = stderr.IndexOf(CrashMarker, StringComparison.Ordinal);
        LogDiagnostics(crash < 0 ? stderr : stderr.Substring(0, crash));
        if (crash >= 0)
        {
            LogCrash(process.ExitCode, stderr.Substring(crash + CrashMarker.Length).Trim(), stdout);
        }
        else if (process.ExitCode != 0 && process.ExitCode != 1)
        {
            // 1 is the compiler declining the sources (the diagnostics say
            // why); anything else is a death it could not report itself — a
            // stack overflow, a signal, a missing runtime library
            LogCrash(process.ExitCode, stderr.Trim(), stdout);
        }
        else if (process.ExitCode != 0 && stderr.Trim().Length == 0)
        {
            Debug.LogError($"[MenSharp] compiler exited with {process.ExitCode}\n{stdout}");
        }
        return process.ExitCode == 0;
    }

    /// The first line of the compiler's crash report starts with this; keep
    /// it in sync with `src/men-sharp/src/crash.rs`.
    private const string CrashMarker = "[mensharp crash]";

    private const string IssuesUrl = "https://github.com/ProjectTesca/MenSharp/issues";

    /// One console entry for a compiler crash: an apology, where to report
    /// it, and the report itself (or, when the compiler died without one,
    /// the exit code and whatever it did print).
    private static void LogCrash(int exitCode, string report, string stdout)
    {
        var text = new System.Text.StringBuilder();
        text.Append("[MenSharp] OH NO! The MenSharp compiler has crashed! Sorry!\n");
        text.Append("This is a bug in MenSharp, not in your code. Please report it at ")
            .Append(IssuesUrl)
            .Append(" with the log below.\n\n");
        if (report.Length > 0)
        {
            text.Append(report).Append('\n');
        }
        else
        {
            text.Append("The compiler exited without a report, so the cause could not be recorded ")
                .Append("(a stack overflow or an out-of-memory condition, most likely).\n");
        }
        text.Append("exit code ").Append(exitCode).Append('\n');
        if (stdout.Trim().Length > 0)
        {
            text.Append("\nstandard output:\n").Append(stdout.Trim()).Append('\n');
        }
        Debug.LogError(text.ToString());
    }

    /// One console entry per diagnostic. The compiler's short format is a
    /// `file(line,column): error: message` line the console can jump from,
    /// followed by the source excerpt and hints indented — those lines
    /// belong to the entry above them, shown when it is expanded.
    private static void LogDiagnostics(string stderr)
    {
        var entry = new System.Text.StringBuilder();
        void Flush()
        {
            string text = entry.ToString().TrimEnd();
            if (text.Length > 0)
            {
                // a blank line at the end keeps the stack trace the console
                // appends from running into the diagnostic
                Debug.LogError($"[MenSharp] {text}\n");
            }
            entry.Clear();
        }
        foreach (string raw in stderr.Split('\n'))
        {
            string line = raw.TrimEnd();
            if (line.Length == 0)
            {
                // a blank line inside an entry stays (the details are laid
                // out with them); one between entries is trimmed at Flush
                if (entry.Length > 0)
                {
                    entry.Append('\n');
                }
                continue;
            }
            bool continuation = char.IsWhiteSpace(line[0]);
            if (!continuation)
            {
                Flush();
                entry.Append(line);
            }
            else
            {
                // the compiler indents a diagnostic's detail lines by four
                // spaces; those go. The `unity` format marks the offending
                // part inside the source line with rich text, so nothing
                // depends on leading spaces — which the console drops — or
                // on a monospaced font, which it does not use
                string detail = line.StartsWith("    ") ? line.Substring(4) : line;
                entry.Append('\n').Append(detail);
            }
        }
        Flush();
    }

    /// An optional assembly definition for Assets/MenSharp. Not needed:
    /// UdonSharp is kept away from these sources by its scanning blacklist
    /// (see MenSharpUdonSharpIsolation), and living in Assembly-CSharp is
    /// what lets them name the UdonSharp assets dropped into Assets — an
    /// assembly definition cannot reference Assembly-CSharp. For a project
    /// that wants the sources in an assembly of their own anyway (compile
    /// times, layering), this writes one, referencing the runtime.
    [MenuItem("MenSharp/Create Assembly Definition for Assets/MenSharp")]
    private static void CreateAssemblyDefinition()
    {
        string path = SourceRoot + "/MenSharp.Scripts.asmdef";
        if (File.Exists(path))
        {
            Debug.Log($"MenSharp: {path} already exists.");
            return;
        }
        Directory.CreateDirectory(SourceRoot);
        File.WriteAllText(path, @"{
    ""name"": ""MenSharp.Scripts"",
    ""rootNamespace"": """",
    ""references"": [
        ""ProjectTesca.MenSharp.Runtime"",
        ""UdonSharp.Runtime""
    ],
    ""includePlatforms"": [],
    ""excludePlatforms"": [],
    ""allowUnsafeCode"": false,
    ""overrideReferences"": false,
    ""precompiledReferences"": [],
    ""autoReferenced"": true,
    ""defineConstraints"": [],
    ""versionDefines"": [],
    ""noEngineReferences"": false
}
");
        AssetDatabase.Refresh();
    }

    private static string QuoteArguments(List<string> arguments)
    {
        var builder = new StringBuilder();
        foreach (string argument in arguments)
        {
            if (builder.Length > 0)
            {
                builder.Append(' ');
            }
            builder.Append('"');
            // our arguments are paths and class names; quotes never appear,
            // and Windows treats backslashes literally unless they precede one
            builder.Append(argument.Replace("\"", "\\\""));
            builder.Append('"');
        }
        return builder.ToString();
    }

    /// The reference dlls, taken from the very assemblies this editor session
    /// has loaded — no path guessing.
    private static IEnumerable<string> ReferenceAssemblies()
    {
        yield return typeof(object).Assembly.Location; // mscorlib
        yield return typeof(UnityEngine.Debug).Assembly.Location; // CoreModule
        yield return typeof(UnityEngine.Rigidbody).Assembly.Location; // PhysicsModule
        yield return typeof(VRC.Udon.UdonBehaviour).Assembly.Location; // VRC.Udon
        // IUdonEventReceiver — what a behaviour calls on itself
        yield return typeof(VRC.Udon.Common.Interfaces.IUdonEventReceiver).Assembly.Location;
        // Networking, VRCPlayerApi
        yield return typeof(VRC.SDKBase.Networking).Assembly.Location;
        // UdonSharpBehaviour: what an UdonSharp script's class derives from
        yield return typeof(UdonSharp.UdonSharpBehaviour).Assembly.Location;
        // [NetworkCallable], NetworkEventTarget's users, VRC components
        yield return typeof(VRC.SDK3.UdonNetworkCalling.NetworkCallableAttribute).Assembly.Location;
        // text in the world: TextMeshPro and uGUI. Looked up by name — this
        // assembly does not reference them, and a project without
        // TextMeshPro simply compiles without it.
        foreach (string name in new[] { "Unity.TextMeshPro", "UnityEngine.UI" })
        {
            string location = LoadedAssemblyLocation(name);
            if (location != null)
            {
                yield return location;
            }
        }
        // every engine module the editor has loaded (Animation, Audio,
        // ParticleSystem, ...): a script may name a type from any of them,
        // and the Udon whitelist already decides what is callable. Each is a
        // small assembly, parsed in parallel, so listing them all costs less
        // than one user asking why HumanBodyBones does not resolve.
        foreach (System.Reflection.Assembly assembly in AppDomain.CurrentDomain.GetAssemblies())
        {
            string name = assembly.GetName().Name;
            if (assembly.IsDynamic
                || !name.StartsWith("UnityEngine.", StringComparison.Ordinal)
                || !name.EndsWith("Module", StringComparison.Ordinal)
                || name == "UnityEngine.CoreModule"
                || name == "UnityEngine.PhysicsModule")
            {
                continue;
            }
            string location = assembly.Location;
            if (!string.IsNullOrEmpty(location))
            {
                yield return location;
            }
        }
    }

    /// The file of a loaded assembly, by its simple name; null when none is
    /// loaded (or it lives in memory only).
    private static string LoadedAssemblyLocation(string name)
    {
        foreach (System.Reflection.Assembly assembly in AppDomain.CurrentDomain.GetAssemblies())
        {
            if (assembly.GetName().Name != name || assembly.IsDynamic)
            {
                continue;
            }
            string location = assembly.Location;
            return string.IsNullOrEmpty(location) ? null : location;
        }
        return null;
    }

    /// Where the bundled compiler for this platform lives. Pure path
    /// arithmetic — no checks, no logging — so callers that only want to look
    /// at it (is it newer than what we built?) do not trip diagnostics.
    public static string CompilerPath()
    {
        string root = Path.GetFullPath($"Packages/{PackageName}/Compiler~");
        string name;
#if UNITY_EDITOR_WIN
        name = "men-sharp-windows-x64.exe";
#elif UNITY_EDITOR_OSX
        name = System.Runtime.InteropServices.RuntimeInformation.ProcessArchitecture
            == System.Runtime.InteropServices.Architecture.Arm64
            ? "men-sharp-macos-arm64"
            : "men-sharp-macos-x64";
#else
        name = "men-sharp-linux-x64";
#endif
        return Path.Combine(root, name);
    }

    private static string FindCompilerBinary()
    {
        string path = CompilerPath();
        if (!File.Exists(path))
        {
            Debug.LogError(
                $"MenSharp: compiler binary not found at {path}. Reinstall the package "
                + "(or run tools/install-local.sh when developing MenSharp itself).");
            return null;
        }
#if !UNITY_EDITOR_WIN
        // zip extraction loses the executable bit
        try
        {
            var chmod = Process.Start(new ProcessStartInfo
            {
                FileName = "/bin/chmod",
                ArgumentList = { "+x", path },
                UseShellExecute = false,
                CreateNoWindow = true,
            });
            chmod?.WaitForExit();
        }
        catch (Exception exception)
        {
            Debug.LogWarning($"MenSharp: chmod failed: {exception.Message}");
        }
#endif
        return path;
    }
}
#endif

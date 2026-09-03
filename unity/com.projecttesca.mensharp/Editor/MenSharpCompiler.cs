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

    [MenuItem("MenSharp/Compile All %#m")]
    public static void CompileAll()
    {
        if (!Directory.Exists(SourceRoot))
        {
            Directory.CreateDirectory(SourceRoot);
            CreateAssemblyDefinition();
            AssetDatabase.Refresh();
            Debug.Log(
                $"MenSharp: created {SourceRoot}. Put your .cs sources there (classes "
                + "inheriting MenSharpBehaviour become programs) and compile again.");
            return;
        }

        // MenSharp sources are the scripts of every assembly referencing the
        // runtime, plus the source folder; everything else is a library (see
        // MenSharpSources)
        MenSharpSources.SourceSet set = MenSharpSources.Collect();
        if (set.MenSharp.Count == 0)
        {
            Debug.LogError($"MenSharp: no .cs files under {SourceRoot} (or in an assembly referencing {MenSharpSources.RuntimeAssembly}).");
            return;
        }

        string binary = FindCompilerBinary();
        if (binary == null)
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
        if (!RunCompiler(binary, outputDirectory, set.MenSharp.ToArray(), set.Library))
        {
            Debug.LogError("MenSharp: compilation failed.");
            return;
        }

        // one program asset per produced behaviour, beside the assembly that
        // declares it: a package ships its programs with its prefabs
        var produced = Directory.GetFiles(outputDirectory, "*.uasm");
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
            MenSharpImporter.CreateOrUpdate(uasmPath, metaPath, $"{folder}/{classPath}.asset");
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
        AssetDatabase.SaveAssets();
        MenSharpSources.InvalidateProgramIndex();

        Debug.Log(
            $"MenSharp: compiled {produced.Length} behaviour(s) from {set.MenSharp.Count} "
            + $"file(s) (+{set.Library.Count} library file(s)) in {stopwatch.ElapsedMilliseconds}ms.");
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
        string stdout = process.StandardOutput.ReadToEnd();
        string stderr = process.StandardError.ReadToEnd();
        process.WaitForExit();

        foreach (string line in stderr.Split('\n'))
        {
            string trimmed = line.TrimEnd();
            if (trimmed.Length == 0)
            {
                continue;
            }
            Debug.LogError($"[MenSharp] {trimmed}");
        }
        if (process.ExitCode != 0 && stderr.Trim().Length == 0)
        {
            Debug.LogError($"[MenSharp] compiler exited with {process.ExitCode}\n{stdout}");
        }
        return process.ExitCode == 0;
    }

    /// An assembly definition keeps these sources out of Assembly-CSharp —
    /// which keeps them out of *UdonSharp's* compilation pass (U# compiles
    /// every Assembly-CSharp script and cannot resolve MenSharpBehaviour).
    /// Unity still compiles them normally, so IDE completion keeps working.
    ///
    /// Written once, when the source folder is first created. Deleting it is
    /// a choice this code respects: an assembly definition cannot reference
    /// Assembly-CSharp, so a project whose UdonSharp assets live there (no
    /// asmdef of their own) needs the M# sources there too, to name them.
    private static void CreateAssemblyDefinition()
    {
        string path = SourceRoot + "/MenSharp.Scripts.asmdef";
        if (File.Exists(path))
        {
            return;
        }
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

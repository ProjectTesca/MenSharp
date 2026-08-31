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
    private const string SourceRoot = "Assets/MenSharp";
    private const string ProgramsFolder = SourceRoot + "/Programs";
    private const string PackageName = "com.projecttesca.mensharp";

    [MenuItem("MenSharp/Compile All %#m")]
    public static void CompileAll()
    {
        if (!Directory.Exists(SourceRoot))
        {
            Directory.CreateDirectory(SourceRoot);
            EnsureAssemblyDefinition();
            AssetDatabase.Refresh();
            Debug.Log(
                $"MenSharp: created {SourceRoot}. Put your .cs sources there (classes "
                + "inheriting MenSharpBehaviour become programs) and compile again.");
            return;
        }
        EnsureAssemblyDefinition();

        var sources = Directory.GetFiles(SourceRoot, "*.cs", SearchOption.AllDirectories);
        if (sources.Length == 0)
        {
            Debug.LogError($"MenSharp: no .cs files under {SourceRoot}.");
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
        if (!RunCompiler(binary, outputDirectory, sources))
        {
            Debug.LogError("MenSharp: compilation failed.");
            return;
        }

        // one program asset per produced behaviour
        var produced = Directory.GetFiles(outputDirectory, "*.uasm");
        Directory.CreateDirectory(ProgramsFolder);
        var current = new HashSet<string>();
        foreach (string uasmPath in produced)
        {
            string classPath = Path.GetFileNameWithoutExtension(uasmPath); // "Demo.Door"
            string className = classPath.Substring(classPath.LastIndexOf('.') + 1);
            string metaPath = Path.Combine(
                outputDirectory, classPath + ".meta.json");
            MenSharpImporter.CreateOrUpdate(
                uasmPath, metaPath, $"{ProgramsFolder}/{className}.asset");
            current.Add(className);
        }
        DeleteProgramsWithoutABehaviour(current);
        AssetDatabase.SaveAssets();

        Debug.Log(
            $"MenSharp: compiled {produced.Length} behaviour(s) from {sources.Length} "
            + $"file(s) in {stopwatch.ElapsedMilliseconds}ms.");
    }

    /// Program assets left over from behaviours the sources no longer declare.
    /// Keeping them would leave GameObjects running code that is not in the
    /// project any more — and the UdonBehaviour carrying it is hidden, so
    /// nobody would see why.
    private static void DeleteProgramsWithoutABehaviour(HashSet<string> current)
    {
        foreach (string file in Directory.GetFiles(ProgramsFolder, "*.asset"))
        {
            string name = Path.GetFileNameWithoutExtension(file);
            if (current.Contains(name))
            {
                continue;
            }
            string assetPath = $"{ProgramsFolder}/{Path.GetFileName(file)}";
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

    private static bool RunCompiler(string binary, string outputDirectory, string[] sources)
    {
        var arguments = new List<string>();
        foreach (string reference in ReferenceAssemblies())
        {
            arguments.Add("--reference");
            arguments.Add(reference);
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
    private static void EnsureAssemblyDefinition()
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
        ""ProjectTesca.MenSharp.Runtime""
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

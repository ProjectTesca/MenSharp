// MenSharp: one-button compilation inside Unity.
//
// Workflow (v0.1):
//   - M# sources live under Assets/MenSharp/ (plain .cs — Unity compiles them
//     too, which is what gives you IDE completion for free)
//   - Assets/MenSharp/mensharp.json lists the entry classes:
//         { "entries": [ "Demo.Greeter" ] }
//   - Menu: MenSharp > Compile All (or Ctrl+Shift+M) runs the bundled Rust
//     compiler; diagnostics land in the Console; one program asset per entry
//     is created/updated under Assets/MenSharp/Programs/ with a stable GUID,
//     so scene references survive recompiles.

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
    private const string ConfigPath = SourceRoot + "/mensharp.json";
    private const string ProgramsFolder = SourceRoot + "/Programs";
    private const string PackageName = "com.projecttesca.mensharp";

    [Serializable]
    private class Config
    {
        public string[] entries;
    }

    [MenuItem("MenSharp/Compile All %#m")]
    public static void CompileAll()
    {
        if (!Directory.Exists(SourceRoot))
        {
            Directory.CreateDirectory(SourceRoot);
            File.WriteAllText(
                ConfigPath,
                "{\n  \"entries\": [\n  ]\n}\n");
            AssetDatabase.Refresh();
            Debug.Log(
                $"MenSharp: created {SourceRoot}. Put your .cs sources there and list entry "
                + $"classes in {ConfigPath}, then compile again.");
            return;
        }
        if (!File.Exists(ConfigPath))
        {
            Debug.LogError($"MenSharp: missing {ConfigPath} — list your entry classes there.");
            return;
        }
        var config = JsonUtility.FromJson<Config>(File.ReadAllText(ConfigPath));
        if (config?.entries == null || config.entries.Length == 0)
        {
            Debug.LogError($"MenSharp: {ConfigPath} lists no entries.");
            return;
        }

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

        var stopwatch = Stopwatch.StartNew();
        int failures = 0;
        foreach (string entry in config.entries)
        {
            string safeName = entry.Replace('.', '_');
            string outputBase = Path.Combine(outputDirectory, safeName);
            if (!RunCompiler(binary, entry, outputBase, sources))
            {
                failures++;
                continue;
            }

            string className = entry.Substring(entry.LastIndexOf('.') + 1);
            Directory.CreateDirectory(ProgramsFolder);
            MenSharpImporter.CreateOrUpdate(
                outputBase + ".uasm",
                outputBase + ".meta.json",
                $"{ProgramsFolder}/{className}.asset");
        }
        AssetDatabase.SaveAssets();

        if (failures == 0)
        {
            Debug.Log(
                $"MenSharp: compiled {config.entries.Length} program(s) from {sources.Length} "
                + $"file(s) in {stopwatch.ElapsedMilliseconds}ms.");
        }
        else
        {
            Debug.LogError($"MenSharp: {failures} program(s) failed to compile.");
        }
    }

    private static bool RunCompiler(
        string binary,
        string entry,
        string outputBase,
        string[] sources)
    {
        var arguments = new List<string>();
        foreach (string reference in ReferenceAssemblies())
        {
            arguments.Add("--reference");
            arguments.Add(reference);
        }
        arguments.Add("--emit-udon");
        arguments.Add(entry);
        arguments.Add("--out");
        arguments.Add(outputBase);
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
    }

    private static string FindCompilerBinary()
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
        string path = Path.Combine(root, name);
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

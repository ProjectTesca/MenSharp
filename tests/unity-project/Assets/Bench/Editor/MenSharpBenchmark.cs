// Compile-speed benchmark: UdonSharp and MenSharp over the same corpus, in
// the same editor session (tools/bench-unity.sh runs it in batchmode).
//
// Each compiler is run several times; the first run of each is cold (the
// compiler's own caches empty) and the rest are warm. MenSharp's log line
// splits its time into the compiler process and the program-asset work
// that follows, so both numbers are recorded.
#if UNITY_EDITOR
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Reflection;
using UdonSharp;
using UnityEditor;
using UnityEngine;
using Debug = UnityEngine.Debug;

public static class MenSharpBenchmark
{
    private const int Runs = 4;
    private const string UdonSharpFolder = "Assets/Bench/UdonSharp";

    public static void Run()
    {
        string output = Environment.GetEnvironmentVariable("MENSHARP_BENCH_RESULTS");
        if (string.IsNullOrEmpty(output))
        {
            output = Path.Combine("..", "..", "artifacts", "bench", "results.txt");
        }
        Directory.CreateDirectory(Path.GetDirectoryName(Path.GetFullPath(output)));

        var lines = new List<string>();
        var captured = new List<string>();
        Application.LogCallback capture = (message, stack, type) =>
        {
            if (message.StartsWith("[MenSharp]") || message.StartsWith("MenSharp") || message.StartsWith("[UdonSharp]"))
            {
                captured.Add(message);
            }
        };
        Application.logMessageReceived += capture;

        try
        {
            int created = EnsureUdonSharpProgramAssets();
            int udonSharpPrograms = UdonSharpProgramAsset.GetAllUdonSharpPrograms().Length;
            lines.Add($"udonsharp programs: {udonSharpPrograms} ({created} created for this run)");
            MethodInfo compileSync = FindUdonSharpCompileSync(lines);

            for (int run = 0; run < Runs; run++)
            {
                captured.Clear();
                var watch = Stopwatch.StartNew();
                if (compileSync != null)
                {
                    Invoke(compileSync);
                }
                else
                {
                    UdonSharpProgramAsset.CompileAllCsPrograms(true, false);
                }
                watch.Stop();
                bool errors = UdonSharpProgramAsset.AnyUdonSharpScriptHasError();
                lines.Add($"udonsharp run {run} ({(run == 0 ? "cold" : "warm")}): {watch.ElapsedMilliseconds} ms{(errors ? " WITH ERRORS" : "")}");
                foreach (string message in captured)
                {
                    lines.Add("    " + FirstLine(message));
                }
            }

            for (int run = 0; run < Runs; run++)
            {
                captured.Clear();
                var watch = Stopwatch.StartNew();
                MenSharpCompiler.RebuildAll();
                watch.Stop();
                lines.Add($"mensharp run {run} ({(run == 0 ? "cold" : "warm")}): {watch.ElapsedMilliseconds} ms");
                foreach (string message in captured)
                {
                    lines.Add("    " + FirstLine(message));
                }
            }
        }
        finally
        {
            Application.logMessageReceived -= capture;
        }

        File.WriteAllLines(output, lines);
        Debug.Log("[Bench] wrote " + Path.GetFullPath(output) + "\n" + string.Join("\n", lines));
    }

    /// UdonSharp compiles the scripts that have a program asset. The editor
    /// creates those interactively; in batchmode they are created here, one
    /// per behaviour script of the staged corpus, beside the script.
    private static int EnsureUdonSharpProgramAssets()
    {
        int created = 0;
        foreach (string guid in AssetDatabase.FindAssets("t:MonoScript", new[] { UdonSharpFolder }))
        {
            string path = AssetDatabase.GUIDToAssetPath(guid);
            var script = AssetDatabase.LoadAssetAtPath<MonoScript>(path);
            Type type = script != null ? script.GetClass() : null;
            if (type == null || !typeof(UdonSharpBehaviour).IsAssignableFrom(type) || type.IsAbstract)
            {
                continue;
            }
            string assetPath = Path.ChangeExtension(path, ".asset");
            if (AssetDatabase.LoadAssetAtPath<UdonSharpProgramAsset>(assetPath) != null)
            {
                continue;
            }
            var asset = ScriptableObject.CreateInstance<UdonSharpProgramAsset>();
            asset.sourceCsScript = script;
            AssetDatabase.CreateAsset(asset, assetPath);
            created++;
        }
        if (created > 0)
        {
            AssetDatabase.SaveAssets();
            AssetDatabase.Refresh();
            // internal in some UdonSharp builds; the cache otherwise refreshes on the next domain reload
            MethodInfo clear = typeof(UdonSharpProgramAsset).GetMethod(
                "ClearProgramAssetCache", BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Static);
            clear?.Invoke(null, null);
        }
        return created;
    }

    /// `UdonSharpCompilerV1.CompileSync`, the synchronous entry the editor's
    /// own build hooks use, found by reflection so this file does not depend
    /// on the compiler's internals. Null when the running UdonSharp has none.
    private static MethodInfo FindUdonSharpCompileSync(List<string> lines)
    {
        foreach (Type type in typeof(UdonSharpProgramAsset).Assembly.GetTypes())
        {
            if (type.Name != "UdonSharpCompilerV1")
            {
                continue;
            }
            foreach (MethodInfo method in type.GetMethods(BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Static))
            {
                if (method.Name == "CompileSync")
                {
                    lines.Add($"udonsharp entry: {type.FullName}.{method.Name}({method.GetParameters().Length} parameter(s))");
                    return method;
                }
            }
        }
        lines.Add("udonsharp entry: UdonSharpProgramAsset.CompileAllCsPrograms (CompileSync not found)");
        return null;
    }

    private static void Invoke(MethodInfo method)
    {
        ParameterInfo[] parameters = method.GetParameters();
        var arguments = new object[parameters.Length];
        for (int i = 0; i < parameters.Length; i++)
        {
            arguments[i] = parameters[i].HasDefaultValue ? parameters[i].DefaultValue : null;
        }
        method.Invoke(null, arguments);
    }

    private static string FirstLine(string message)
    {
        int newline = message.IndexOf('\n');
        return newline < 0 ? message : message.Substring(0, newline);
    }
}
#endif

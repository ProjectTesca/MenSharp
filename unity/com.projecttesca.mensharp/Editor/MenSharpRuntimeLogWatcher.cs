// Turns the Udon VM's halt report into a source position.
//
// When an extern (an engine or .NET call) throws, the VM catches it, logs
// "An exception occurred during Udon execution, this UdonBehaviour will be
// halted." with the exception, the program counter and a heap dump, and
// stops the behaviour — nothing of the program runs after that, so M#'s
// own exceptions cannot catch it. What can be done is to say *where*: the
// compiler puts the program's id in heap slot 0 (first in the dump) and
// writes an address → source table into the sidecar, so this watcher can
// name the file, line and column of the call that threw.
//
// Editor only: play mode (ClientSim) logs the report through Unity's log,
// which is what Application.logMessageReceived delivers.
#if UNITY_EDITOR
using System;
using System.Collections.Generic;
using System.Globalization;
using System.Text.RegularExpressions;
using UnityEditor;
using UnityEngine;

[InitializeOnLoad]
public static class MenSharpRuntimeLogWatcher
{
    private const string HaltMarker = "An exception occurred during Udon execution, this UdonBehaviour will be halted.";

    private static readonly Regex ProgramCounter = new Regex(@"Program Counter was at:?\s*(?<counter>\d+)", RegexOptions.Compiled);
    private static readonly Regex ProgramId = new Regex(@"Heap Dump:[\r\n\s]+0x[0-9A-Fa-f]+:\s*(?<id>-?\d+)", RegexOptions.Compiled);
    private static readonly Regex ExternName = new Regex(@"An exception occurred during EXTERN to '(?<name>[^']+)'", RegexOptions.Compiled);

    private static Dictionary<long, MenSharpProgramAsset> _programsById;

    static MenSharpRuntimeLogWatcher()
    {
        Application.logMessageReceived -= OnLog;
        Application.logMessageReceived += OnLog;
    }

    private static void OnLog(string condition, string stackTrace, LogType type)
    {
        if (type != LogType.Error && type != LogType.Exception)
        {
            return;
        }
        if (condition == null || condition.IndexOf(HaltMarker, StringComparison.Ordinal) < 0)
        {
            return;
        }

        Match counterMatch = ProgramCounter.Match(condition);
        Match idMatch = ProgramId.Match(condition);
        if (!counterMatch.Success || !idMatch.Success)
        {
            return;
        }
        if (!uint.TryParse(counterMatch.Groups["counter"].Value, NumberStyles.Integer, CultureInfo.InvariantCulture, out uint counter)
            || !long.TryParse(idMatch.Groups["id"].Value, NumberStyles.Integer, CultureInfo.InvariantCulture, out long programId))
        {
            return;
        }

        MenSharpProgramAsset asset = FindProgram(programId);
        if (asset == null || string.IsNullOrEmpty(asset.metaJson))
        {
            return;
        }
        MenSharpMeta meta;
        try
        {
            meta = JsonUtility.FromJson<MenSharpMeta>(asset.metaJson);
        }
        catch (Exception)
        {
            return;
        }
        if (meta?.lines == null || meta.lines.Length == 0)
        {
            return;
        }

        // the last entry at or before the counter: the code from there on
        // belongs to that source position
        MenSharpSourceMark mark = null;
        foreach (MenSharpSourceMark candidate in meta.lines)
        {
            if (candidate.address > counter)
            {
                break;
            }
            mark = candidate;
        }
        if (mark == null)
        {
            return;
        }
        if (mark.kind == "halt")
        {
            // M#'s own stop after "Unhandled exception: ..." was logged:
            // the VM's report is the expected consequence, nothing to add
            return;
        }

        string externName = ExternName.Match(condition).Groups["name"].Value;
        string message = ExceptionMessage(condition);
        string inside = string.IsNullOrEmpty(externName) ? "an engine call" : $"`{Readable(externName)}`";
        string where = mark.kind == "function" || string.IsNullOrEmpty(mark.file)
            ? $"in compiler-generated code of {mark.function} (before its first statement)"
            : $"called from {mark.function} at {mark.file}:{mark.line}:{mark.column}";
        Debug.LogError(
            $"MenSharp: the Udon VM halted inside {inside}, {where}"
            + (string.IsNullOrEmpty(message) ? "" : $"\n{message}")
            + "\nThe exception came from the engine call itself, which try/catch cannot reach on Udon — "
            + "check the call's inputs (null references, ranges) before making it.",
            asset);
    }

    /// The text between "Exception Message:" and the dashed separator.
    private static string ExceptionMessage(string report)
    {
        const string Start = "Exception Message:";
        const string End = "----------------------";
        int from = report.IndexOf(Start, StringComparison.Ordinal);
        if (from < 0)
        {
            return null;
        }
        from += Start.Length;
        int to = report.IndexOf(End, from, StringComparison.Ordinal);
        string text = to < 0 ? report.Substring(from) : report.Substring(from, to - from);
        return text.Trim('\r', '\n', ' ');
    }

    /// `UnityEngineTransform.__get_position__UnityEngineVector3` →
    /// `UnityEngineTransform.get_position`, close enough to read.
    private static string Readable(string externName)
    {
        int split = externName.IndexOf(".__", StringComparison.Ordinal);
        if (split < 0)
        {
            return externName;
        }
        string owner = externName.Substring(0, split);
        string rest = externName.Substring(split + 3);
        int end = rest.IndexOf("__", StringComparison.Ordinal);
        return end < 0 ? $"{owner}.{rest}" : $"{owner}.{rest.Substring(0, end)}";
    }

    private static MenSharpProgramAsset FindProgram(long programId)
    {
        if (_programsById != null && _programsById.TryGetValue(programId, out MenSharpProgramAsset cached) && cached != null)
        {
            return cached;
        }
        _programsById = new Dictionary<long, MenSharpProgramAsset>();
        foreach (string guid in AssetDatabase.FindAssets("t:MenSharpProgramAsset"))
        {
            var asset = AssetDatabase.LoadAssetAtPath<MenSharpProgramAsset>(AssetDatabase.GUIDToAssetPath(guid));
            if (asset == null || string.IsNullOrEmpty(asset.metaJson))
            {
                continue;
            }
            try
            {
                MenSharpMeta meta = JsonUtility.FromJson<MenSharpMeta>(asset.metaJson);
                if (meta != null && meta.programId != 0)
                {
                    _programsById[meta.programId] = asset;
                }
            }
            catch (Exception)
            {
                // a sidecar from an older compiler: no id, no lookup
            }
        }
        return _programsById.TryGetValue(programId, out MenSharpProgramAsset found) ? found : null;
    }
}
#endif

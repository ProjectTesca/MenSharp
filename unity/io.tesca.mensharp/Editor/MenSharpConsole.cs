// MenSharp: console entries that open the M# source they are about.
//
// The console jumps to the first frame of an entry's stack trace on a
// double-click, and for Debug.LogError that frame is the editor code doing
// the logging — so a compile error opened MenSharpCompiler.cs instead of the
// line it reports. Debug.LogPlayerBuildError (internal; Burst and UdonSharp
// log their compile errors through it too) records the file, line and column
// on the entry itself, with no stack trace, and the console opens that.
#if UNITY_EDITOR
using System;
using System.Globalization;
using System.Reflection;
using System.Text.RegularExpressions;
using UnityEngine;

public static class MenSharpConsole
{
    private static readonly MethodInfo LogWithLocation = typeof(Debug).GetMethod(
        "LogPlayerBuildError",
        BindingFlags.NonPublic | BindingFlags.Static,
        null,
        new[] { typeof(string), typeof(string), typeof(int), typeof(int) },
        null);

    /// The `  --> file:line:column` line of a diagnostic in the compiler's
    /// `unity` format. The file may hold colons of its own (a Windows drive),
    /// so the line and column are the last two numbers.
    private static readonly Regex Arrow = new Regex(
        @"^\s*-->\s*(?<file>.+):(?<line>\d+):(?<column>\d+)\s*$",
        RegexOptions.Compiled | RegexOptions.Multiline);

    /// The first `-->` position in a diagnostic: the primary span, ahead of
    /// any labels pointing elsewhere.
    public static bool TryLocate(string diagnostic, out string file, out int line, out int column)
    {
        file = null;
        line = 0;
        column = 0;
        Match match = Arrow.Match(diagnostic ?? "");
        if (!match.Success
            || !int.TryParse(match.Groups["line"].Value, NumberStyles.Integer, CultureInfo.InvariantCulture, out line)
            || !int.TryParse(match.Groups["column"].Value, NumberStyles.Integer, CultureInfo.InvariantCulture, out column))
        {
            return false;
        }
        file = match.Groups["file"].Value.Trim();
        return file.Length > 0;
    }

    /// An error the console opens at `file:line:column`. Without a usable
    /// position — or should a Unity version drop the internal method — it is
    /// an ordinary error, the text unchanged, logged against `context`.
    public static void ErrorAt(string message, string file, int line, int column, UnityEngine.Object context = null)
    {
        if (LogWithLocation != null && !string.IsNullOrEmpty(file) && line > 0)
        {
            try
            {
                LogWithLocation.Invoke(null, new object[] { message, file.Replace('\\', '/'), line, Math.Max(column, 0) });
                return;
            }
            catch (Exception)
            {
                // fall through to the plain entry
            }
        }
        Debug.LogError(message, context);
    }

    /// A compiler diagnostic, opened at the position its `-->` line names.
    public static void Diagnostic(string message)
    {
        if (TryLocate(message, out string file, out int line, out int column))
        {
            ErrorAt(message, file, line, column);
        }
        else
        {
            Debug.LogError(message);
        }
    }
}
#endif

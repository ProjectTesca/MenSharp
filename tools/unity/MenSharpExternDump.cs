// MenSharp: dump every Udon node definition (extern signatures included) to JSON.
//
// Usage:
//   1. Copy this file into the Unity project at Assets/Editor/MenSharpExternDump.cs
//   2. In the editor menu, run  MenSharp > Dump Udon Node Definitions
//   3. Pick a save location; commit the resulting JSON into the men-sharp repo.
//
// The dump includes everything UdonEditorManager knows about — extern method
// signatures, type nodes, graph flow nodes. Filtering happens on the Rust side,
// so this stays a dumb, complete snapshot of what the installed SDK exposes.

#if UNITY_EDITOR
using System;
using System.IO;
using System.Text;
using UnityEditor;
using UnityEngine;
using VRC.Udon.Editor;
using VRC.Udon.Graph;

public static class MenSharpExternDump
{
    [MenuItem("MenSharp/Dump Udon Node Definitions")]
    public static void Dump()
    {
        string path = EditorUtility.SaveFilePanel(
            "Save Udon node definitions", "", "udon_nodes.json", "json");
        if (string.IsNullOrEmpty(path))
        {
            return;
        }

        var sb = new StringBuilder(1 << 24);
        sb.Append("{\n");
        sb.Append("  \"unityVersion\": ").Append(Quote(Application.unityVersion)).Append(",\n");
        sb.Append("  \"nodes\": [\n");

        bool firstNode = true;
        int count = 0;
        foreach (UdonNodeDefinition definition in UdonEditorManager.Instance.GetNodeDefinitions())
        {
            if (!firstNode)
            {
                sb.Append(",\n");
            }
            firstNode = false;
            count++;

            sb.Append("    {\"fullName\": ").Append(Quote(definition.fullName));
            sb.Append(", \"relatedType\": ").Append(Quote(TypeName(definition.type)));
            sb.Append(", \"parameters\": [");

            bool firstParameter = true;
            foreach (UdonNodeParameter parameter in definition.parameters)
            {
                if (!firstParameter)
                {
                    sb.Append(", ");
                }
                firstParameter = false;

                sb.Append("{\"name\": ").Append(Quote(parameter.name));
                sb.Append(", \"type\": ").Append(Quote(TypeName(parameter.type)));
                sb.Append(", \"kind\": ").Append(Quote(parameter.parameterType.ToString()));
                sb.Append('}');
            }
            sb.Append("]}");
        }

        sb.Append("\n  ]\n}\n");
        File.WriteAllText(path, sb.ToString());
        Debug.Log($"MenSharp: wrote {count} node definitions to {path}");
    }

    private static string TypeName(Type type)
    {
        if (type == null)
        {
            return "null";
        }
        return type.FullName ?? type.Name;
    }

    private static string Quote(string value)
    {
        if (value == null)
        {
            return "null";
        }
        var sb = new StringBuilder(value.Length + 2);
        sb.Append('"');
        foreach (char c in value)
        {
            switch (c)
            {
                case '"': sb.Append("\\\""); break;
                case '\\': sb.Append("\\\\"); break;
                case '\n': sb.Append("\\n"); break;
                case '\r': sb.Append("\\r"); break;
                case '\t': sb.Append("\\t"); break;
                default:
                    if (c < 0x20)
                    {
                        sb.Append("\\u").Append(((int)c).ToString("x4"));
                    }
                    else
                    {
                        sb.Append(c);
                    }
                    break;
            }
        }
        sb.Append('"');
        return sb.ToString();
    }
}
#endif

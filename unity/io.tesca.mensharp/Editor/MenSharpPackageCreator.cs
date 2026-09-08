// MenSharp > Create Package…: the skeleton of a distributable MenSharp
// package, so an asset author never writes an asmdef or a package.json by
// hand. What makes the result a MenSharp package is nothing special — its
// assembly definition references ProjectTesca.MenSharp.Runtime, which any
// script using MenSharpBehaviour needs anyway — and that is the very thing
// the compiler looks for (see MenSharpSources). Its programs are written to
// Runtime/Programs inside the package, beside the prefabs that use them.
#if UNITY_EDITOR
using System;
using System.IO;
using System.Text.RegularExpressions;
using UnityEditor;
using PackageInfo = UnityEditor.PackageManager.PackageInfo;
using UnityEngine;

public class MenSharpPackageCreator : EditorWindow
{
    private static readonly Regex ValidName = new Regex(@"^[a-z0-9]+(\.[a-z0-9-]+)+$", RegexOptions.Compiled);

    private string _name = "com.example.mygimmick";
    private string _displayName = "My Gimmick";
    private string _author = "";

    [MenuItem("MenSharp/Create Package...")]
    public static void Open()
    {
        var window = GetWindow<MenSharpPackageCreator>(true, "Create MenSharp Package");
        window.minSize = new Vector2(420, 190);
        window.Show();
    }

    private void OnGUI()
    {
        EditorGUILayout.LabelField(
            "A package under Packages/ with an assembly definition wired to MenSharp. "
            + "Write behaviours in its Runtime folder; their programs are compiled into "
            + "Runtime/Programs, next to any prefabs you add. Distribute the folder as a VPM package.",
            EditorStyles.wordWrappedLabel);
        EditorGUILayout.Space();
        _name = EditorGUILayout.TextField("Package name", _name);
        _displayName = EditorGUILayout.TextField("Display name", _displayName);
        _author = EditorGUILayout.TextField("Author", _author);

        string problem = Problem();
        if (problem != null)
        {
            EditorGUILayout.HelpBox(problem, MessageType.Info);
        }
        using (new EditorGUI.DisabledScope(problem != null))
        {
            if (GUILayout.Button("Create"))
            {
                Create();
                Close();
            }
        }
    }

    private string Problem()
    {
        if (!ValidName.IsMatch(_name))
        {
            return "The package name is a reverse-domain id in lower case: com.yourname.gimmick";
        }
        if (Directory.Exists("Packages/" + _name))
        {
            return $"Packages/{_name} already exists.";
        }
        if (string.IsNullOrWhiteSpace(_displayName))
        {
            return "A display name is what the Creator Companion shows.";
        }
        return null;
    }

    private void Create()
    {
        string root = "Packages/" + _name;
        string runtime = root + "/Runtime";
        Directory.CreateDirectory(runtime);
        Directory.CreateDirectory(runtime + "/Programs");

        string menSharpVersion = PackageInfo.FindForAssembly(typeof(MenSharpPackageCreator).Assembly)?.version ?? "0.1.0";
        File.WriteAllText(root + "/package.json", "{\n"
            + $"  \"name\": \"{_name}\",\n"
            + $"  \"displayName\": \"{Escape(_displayName)}\",\n"
            + "  \"version\": \"0.1.0\",\n"
            + "  \"unity\": \"2022.3\",\n"
            + "  \"description\": \"\",\n"
            + $"  \"author\": {{ \"name\": \"{Escape(_author)}\" }},\n"
            + "  \"vpmDependencies\": {\n"
            + $"    \"{MenSharpSources.PackageName}\": \"^{menSharpVersion}\",\n"
            + "    \"com.vrchat.worlds\": \">=3.5.0\"\n"
            + "  }\n"
            + "}\n");

        // the assembly definition: the reference to the runtime is what makes
        // these sources MenSharp's, for Unity and for the compiler alike
        File.WriteAllText(runtime + "/" + _name + ".asmdef", "{\n"
            + $"    \"name\": \"{_name}\",\n"
            + "    \"rootNamespace\": \"\",\n"
            + "    \"references\": [\n"
            + $"        \"{MenSharpSources.RuntimeAssembly}\",\n"
            + "        \"UdonSharp.Runtime\"\n"
            + "    ],\n"
            + "    \"includePlatforms\": [],\n"
            + "    \"excludePlatforms\": [],\n"
            + "    \"allowUnsafeCode\": false,\n"
            + "    \"overrideReferences\": false,\n"
            + "    \"precompiledReferences\": [],\n"
            + "    \"autoReferenced\": true,\n"
            + "    \"defineConstraints\": [],\n"
            + "    \"versionDefines\": [],\n"
            + "    \"noEngineReferences\": false\n"
            + "}\n");

        File.WriteAllText(root + "/README.md",
            $"# {_displayName}\n\n"
            + "A MenSharp package. Behaviours live in `Runtime/`; MenSharp compiles them into "
            + "`Runtime/Programs/` on save (or MenSharp > Compile All). Ship the whole folder: "
            + "programs, prefabs and sources go together, and a consumer's MenSharp recompiles "
            + "the programs in place.\n");

        AssetDatabase.Refresh();
        Debug.Log(
            $"MenSharp: created {root}. Add behaviours under Runtime/ — they compile on save — "
            + "and prefabs beside them; the folder is your VPM package.",
            AssetDatabase.LoadAssetAtPath<UnityEngine.Object>(root + "/package.json"));
    }

    private static string Escape(string text)
    {
        return (text ?? "").Replace("\\", "\\\\").Replace("\"", "\\\"");
    }
}
#endif

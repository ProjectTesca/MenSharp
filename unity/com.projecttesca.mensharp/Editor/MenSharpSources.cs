// Which C# files are MenSharp's to compile, which are a library to read, and
// where each compiled program goes.
//
// The rule is one Unity already enforces: MenSharp sources are the scripts of
// every assembly definition that references ProjectTesca.MenSharp.Runtime —
// the reference `MenSharpBehaviour` needs to resolve at all, so an asset
// author has it whether or not they think about it — plus Assets/MenSharp,
// for the project that dropped its asmdef to live in Assembly-CSharp. A
// program asset is written beside the assembly that declares its behaviour
// (`<asmdef folder>/Programs/`), so a package ships its programs with its
// prefabs and a consumer's recompile updates them in place.
//
// Every other .cs in the project — Assets outside Editor folders, packages
// other than VRChat's — is a *library*: read for declarations and bodies,
// compiled into a program only where M# code uses it, its errors its own.
#if UNITY_EDITOR
using System;
using System.Collections.Generic;
using System.IO;
using UnityEditor;
using UnityEditor.Compilation;
using PackageInfo = UnityEditor.PackageManager.PackageInfo;
using UnityEngine;

public static class MenSharpSources
{
    public const string RuntimeAssembly = "ProjectTesca.MenSharp.Runtime";
    public const string SourceRoot = "Assets/MenSharp";
    public const string DefaultProgramsFolder = SourceRoot + "/Programs";
    public const string PackageName = "com.projecttesca.mensharp";

    /// The classified sources of one compilation.
    public sealed class SourceSet
    {
        /// MenSharp sources, as paths the compiler can open.
        public readonly List<string> MenSharp = new List<string>();
        /// Library sources (UdonSharp assets and everything else), likewise.
        public readonly List<string> Library = new List<string>();
        /// For each MenSharp source path: the asset folder its programs go to.
        public readonly Dictionary<string, string> ProgramsFolderOf = new Dictionary<string, string>();
        /// Every programs folder in play this compilation.
        public readonly HashSet<string> ProgramsFolders = new HashSet<string>();
    }

    public static SourceSet Collect()
    {
        var set = new SourceSet();
        var claimed = new HashSet<string>(StringComparer.Ordinal);

        void AddMenSharp(string assetPath, string programsFolder)
        {
            string path = Normalize(assetPath);
            if (!claimed.Add(path))
            {
                return;
            }
            string openable = Openable(path);
            set.MenSharp.Add(openable);
            set.ProgramsFolderOf[openable] = programsFolder;
            set.ProgramsFolders.Add(programsFolder);
        }

        // assemblies that reference the runtime: MenSharp's, wherever they live
        foreach (Assembly assembly in CompilationPipeline.GetAssemblies(AssembliesType.PlayerWithoutTestAssemblies))
        {
            if (!IsMenSharpAssembly(assembly))
            {
                continue;
            }
            string asmdef = CompilationPipeline.GetAssemblyDefinitionFilePathFromAssemblyName(assembly.name);
            if (string.IsNullOrEmpty(asmdef))
            {
                continue;
            }
            string folder = Normalize(Path.GetDirectoryName(asmdef)) + "/Programs";
            foreach (string source in assembly.sourceFiles)
            {
                AddMenSharp(source, folder);
            }
        }
        // the source folder itself, asmdef or not
        if (Directory.Exists(SourceRoot))
        {
            foreach (string source in Directory.GetFiles(SourceRoot, "*.cs", SearchOption.AllDirectories))
            {
                AddMenSharp(source, DefaultProgramsFolder);
            }
        }

        // everything else is a library
        var roots = new List<string> { "Assets" };
        if (Directory.Exists("Packages"))
        {
            foreach (string package in Directory.GetDirectories("Packages"))
            {
                string name = Path.GetFileName(package);
                if (name.StartsWith("com.vrchat.", StringComparison.Ordinal) || name == PackageName)
                {
                    continue;
                }
                roots.Add(package);
            }
        }
        foreach (string root in roots)
        {
            if (!Directory.Exists(root))
            {
                continue;
            }
            foreach (string file in Directory.GetFiles(root, "*.cs", SearchOption.AllDirectories))
            {
                string path = Normalize(file);
                if (claimed.Contains(path) || IsEditorPath(path))
                {
                    continue;
                }
                claimed.Add(path);
                set.Library.Add(path);
            }
        }
        set.MenSharp.Sort(StringComparer.Ordinal);
        set.Library.Sort(StringComparer.Ordinal);
        return set;
    }

    /// Does this assembly reference the MenSharp runtime — is it MenSharp's?
    /// Assembly-CSharp references every auto-referenced assembly, ours
    /// included, so it never counts; its MenSharp sources are the ones under
    /// Assets/MenSharp.
    public static bool IsMenSharpAssembly(Assembly assembly)
    {
        if (assembly.name == RuntimeAssembly
            || assembly.name.StartsWith("Assembly-CSharp", StringComparison.Ordinal))
        {
            return false;
        }
        foreach (Assembly reference in assembly.assemblyReferences)
        {
            if (reference.name == RuntimeAssembly)
            {
                return true;
            }
        }
        return false;
    }

    /// Is this .cs one of MenSharp's, by the rule above? For the
    /// auto-compile: a change to it (or to any library file) means a
    /// recompile.
    public static bool IsMenSharpSource(string assetPath)
    {
        string path = Normalize(assetPath);
        if (!path.EndsWith(".cs", StringComparison.Ordinal))
        {
            return false;
        }
        if (path.StartsWith(SourceRoot + "/", StringComparison.Ordinal))
        {
            return true;
        }
        string assemblyName = CompilationPipeline.GetAssemblyNameFromScriptPath(path);
        if (string.IsNullOrEmpty(assemblyName))
        {
            return false;
        }
        assemblyName = Path.GetFileNameWithoutExtension(assemblyName);
        foreach (Assembly assembly in CompilationPipeline.GetAssemblies(AssembliesType.PlayerWithoutTestAssemblies))
        {
            if (assembly.name == assemblyName)
            {
                return IsMenSharpAssembly(assembly);
            }
        }
        return false;
    }

    /// Is this .cs a library source — anything of the project's that is not
    /// MenSharp's, not in an Editor folder and not in a VRChat package?
    public static bool IsLibrarySource(string assetPath)
    {
        string path = Normalize(assetPath);
        if (!path.EndsWith(".cs", StringComparison.Ordinal) || IsEditorPath(path))
        {
            return false;
        }
        if (path.StartsWith("Packages/", StringComparison.Ordinal))
        {
            string package = path.Substring("Packages/".Length).Split('/')[0];
            if (package.StartsWith("com.vrchat.", StringComparison.Ordinal) || package == PackageName)
            {
                return false;
            }
            return true;
        }
        return path.StartsWith("Assets/", StringComparison.Ordinal);
    }

    // ------------------------------------------------------------- programs

    private static Dictionary<string, MenSharpProgramAsset> _programsByClass;

    /// Forget the program index; the next lookup rebuilds it.
    public static void InvalidateProgramIndex()
    {
        _programsByClass = null;
    }

    /// The compiled program for a behaviour class, wherever its assembly
    /// keeps its programs, or null when it has not been compiled yet.
    public static MenSharpProgramAsset FindProgram(Type behaviourType)
    {
        // assets are named by the full class path — two behaviours may share
        // a short name across namespaces ('+' is how reflection spells nesting)
        string fullPath = (behaviourType.FullName ?? behaviourType.Name).Replace('+', '.');
        MenSharpProgramAsset program = FindProgram(fullPath);
        if (program != null)
        {
            return program;
        }
        // an asset imported before full-path naming; the next compile renames it
        return FindProgram(behaviourType.Name);
    }

    public static MenSharpProgramAsset FindProgram(string classPath)
    {
        if (_programsByClass != null
            && _programsByClass.TryGetValue(classPath, out MenSharpProgramAsset cached)
            && cached != null)
        {
            return cached;
        }
        _programsByClass = new Dictionary<string, MenSharpProgramAsset>(StringComparer.Ordinal);
        foreach (string guid in AssetDatabase.FindAssets("t:MenSharpProgramAsset"))
        {
            string path = AssetDatabase.GUIDToAssetPath(guid);
            var asset = AssetDatabase.LoadAssetAtPath<MenSharpProgramAsset>(path);
            if (asset == null)
            {
                continue;
            }
            string name = Path.GetFileNameWithoutExtension(path);
            // Assets/MenSharp/Programs first: the project's own wins a tie
            if (!_programsByClass.ContainsKey(name)
                || Normalize(path).StartsWith(DefaultProgramsFolder + "/", StringComparison.Ordinal))
            {
                _programsByClass[name] = asset;
            }
        }
        return _programsByClass.TryGetValue(classPath, out MenSharpProgramAsset found) ? found : null;
    }

    // ---------------------------------------------------------------- paths

    public static string Normalize(string path)
    {
        return path.Replace('\\', '/');
    }

    /// A path the compiler (a separate process, run from the project root)
    /// can open: the asset path itself for Assets and local packages, the
    /// resolved location for a package Unity keeps elsewhere.
    public static string Openable(string assetPath)
    {
        if (File.Exists(assetPath))
        {
            return assetPath;
        }
        if (assetPath.StartsWith("Packages/", StringComparison.Ordinal))
        {
            PackageInfo package = PackageInfo.FindForAssetPath(assetPath);
            if (package != null && !string.IsNullOrEmpty(package.resolvedPath))
            {
                string prefix = "Packages/" + package.name + "/";
                if (assetPath.StartsWith(prefix, StringComparison.Ordinal))
                {
                    return Normalize(Path.Combine(package.resolvedPath, assetPath.Substring(prefix.Length)));
                }
            }
        }
        return assetPath;
    }

    public static bool IsEditorPath(string path)
    {
        foreach (string segment in Normalize(path).Split('/'))
        {
            if (segment == "Editor")
            {
                return true;
            }
        }
        return false;
    }
}
#endif

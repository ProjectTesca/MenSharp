#if UNITY_EDITOR
using System;
using System.Collections.Generic;
using System.Linq;
using System.Reflection;
using NUnit.Framework;
using UnityEditor;
using UnityEditorInternal;
using UnityEngine;
using VRC.Udon;

/// What the integration tests share: finding fixture types by name (they
/// live in Assembly-CSharp, which a test assembly cannot reference), putting
/// proxies on GameObjects, and the editor plumbing around play mode.
internal static class MenSharpTestScene
{
    /// The check the test framework's own WaitForDomainReload loops on; it
    /// is internal to the editor, hence the reflection.
    private static readonly MethodInfo ScriptReloadRequested = typeof(InternalEditorUtility)
        .GetMethod("IsScriptReloadRequested", BindingFlags.Static | BindingFlags.NonPublic | BindingFlags.Public);

    /// True when the edit-mode runner is holding an assembly reload back.
    /// A reload still owed from before the run (a fresh project imports
    /// UdonSharp's utility scripts on first load) would otherwise fire in
    /// the middle of a play-mode transition, and the editor comes out of
    /// that in edit mode with Udon never initialised.
    public static bool ScriptReloadPending()
    {
        if (EditorApplication.isCompiling)
        {
            return true;
        }
        return ScriptReloadRequested != null && (bool)ScriptReloadRequested.Invoke(null, null);
    }

    public static Type FindType(string fullName)
    {
        Type found = AppDomain.CurrentDomain.GetAssemblies()
            .Select(assembly => assembly.GetType(fullName, false))
            .FirstOrDefault(type => type != null);
        Assert.IsNotNull(found, $"no loaded type named {fullName}");
        return found;
    }

    /// A GameObject carrying a MenSharp proxy of the named type. A cube, so
    /// it has the Box Collider Interact needs and GetComponent looks for.
    public static GameObject AddProxy(string objectName, string typeName, Vector3 position)
    {
        GameObject target = GameObject.CreatePrimitive(PrimitiveType.Cube);
        target.name = objectName;
        target.transform.position = position;
        Type type = FindType(typeName);
        Assert.IsTrue(typeof(MenSharp.MenSharpBehaviour).IsAssignableFrom(type), typeName);
        Assert.IsNotNull(target.AddComponent(type), typeName);
        return target;
    }

    public static Component Proxy(GameObject target, string typeName)
    {
        Component proxy = target.GetComponent(FindType(typeName));
        Assert.IsNotNull(proxy, $"{target.name} has no {typeName}");
        return proxy;
    }

    /// Writes a proxy's serialized field — the value the transfer copies
    /// into the Udon heap when play mode starts.
    public static void Assign(Component proxy, string field, object value)
    {
        FieldInfo info = proxy.GetType().GetField(field);
        Assert.IsNotNull(info, $"{proxy.GetType().Name} has no field {field}");
        info.SetValue(proxy, value);
    }

    public static UdonBehaviour FindUdon(string objectName)
    {
        GameObject target = GameObject.Find(objectName);
        Assert.IsNotNull(target, objectName);
        UdonBehaviour udon = target.GetComponent<UdonBehaviour>();
        Assert.IsNotNull(udon, objectName);
        return udon;
    }

    public static UdonBehaviour FindUdon(string objectName, string programName)
    {
        GameObject target = GameObject.Find(objectName);
        Assert.IsNotNull(target, objectName);
        UdonBehaviour udon = target.GetComponents<UdonBehaviour>()
            .FirstOrDefault(candidate => candidate.programSource != null && candidate.programSource.name == programName);
        Assert.IsNotNull(udon, $"{objectName} has no UdonBehaviour running {programName}");
        return udon;
    }

    public static void EnsureFolder(string path)
    {
        string current = "Assets";
        foreach (string segment in path.Split('/').Skip(1))
        {
            string next = current + "/" + segment;
            if (!AssetDatabase.IsValidFolder(next))
            {
                AssetDatabase.CreateFolder(current, segment);
            }
            current = next;
        }
    }

    public static string ProjectRoot => System.IO.Path.GetDirectoryName(Application.dataPath);
}
#endif

#if UNITY_EDITOR
using System;
using System.Collections;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Reflection;
using System.Text;
using System.Text.RegularExpressions;
using NUnit.Framework;
using UdonSharp;
using UdonSharp.Compiler;
using UdonSharpEditor;
using UnityEditor;
using UnityEditor.SceneManagement;
using UnityEngine;
using UnityEngine.TestTools;
using VRC.Udon;

/// The manual verification corpus, run the way a person runs it in the
/// development world — every `Verify*` behaviour's Interact (and Start,
/// where it logs) raised on the SDK's real UdonBehaviour — with the console
/// read by the test instead of by eye: what a fixture logs is compared, line
/// by line, with Assets/Tests/Editor/Expected/<Fixture>.txt. Those files are
/// the "Expected:" blocks from the fixtures' own headers, pinned to what this
/// scene produces.
///
/// To re-pin after a deliberate change, run with MENSHARP_VERIFY_RECORD=1:
/// the files are rewritten from what the fixtures logged, for review in the
/// diff; MENSHARP_VERIFY_ONLY=VerifyLinq restricts a run to one fixture.
/// Every run also writes what it saw to artifacts/unity-tests/verify.
public class MenSharpVerifyFixtureTests
{
    private const string GeneratedScene = "Assets/Tests/Generated/MenSharpVerify.unity";
    private const string ExpectedFolder = "Assets/Tests/Editor/Expected";
    private const string ActualFolder = "artifacts/unity-tests/verify";
    private const string UdonSharpScript = "Assets/UdonSharp/UCounter.cs";
    private const string UdonSharpProgram = "Assets/UdonSharp/UCounter.asset";
    private const string RecordVariable = "MENSHARP_VERIFY_RECORD";
    private const string OnlyVariable = "MENSHARP_VERIFY_ONLY";
    private const float GameSpeed = 20f;
    /// The physics step: the cap cannot go below it, and a smaller physics
    /// step makes a frame that did real work spend seconds catching up.
    private const float MaxStep = 0.02f;
    private const float DefaultMaxStep = 1f / 3f;

    private sealed class Fixture
    {
        /// The type, and the name of its GameObject unless `Object` says otherwise.
        public string Type;
        public string Object;
        /// Udon events raised, in order; none for a behaviour that only answers others.
        public string[] Events = { "_interact" };
        /// How long the log may keep arriving after the events (async fixtures).
        public float Timeout = 0.1f;
        /// Why the fixture is placed in the scene but not run.
        public string Skip;
        /// Extra components, and references to the other objects (all exist by then).
        public Action<GameObject> Setup;
        /// Puts state back before the events, for a fixture whose Start already ran at play-mode entry.
        public Action Reset;

        public string Name => Type.Substring(Type.LastIndexOf('.') + 1);
        public string ObjectName => Object ?? Name;
    }

    private static readonly Fixture[] Fixtures =
    {
        new Fixture
        {
            Type = "Verify",
            Events = new[] { "_start", "_interact" },
            Setup = self =>
            {
                Component proxy = MenSharpTestScene.Proxy(self, "Verify");
                MenSharpTestScene.Assign(proxy, "target", MenSharpTestScene.Proxy(GameObject.Find("Target"), "VerifyTarget"));
                MenSharpTestScene.Assign(proxy, "prefab", Extra("Clone Me"));
            },
            // Start already ran once when play mode began: undo what it did
            // to the target so the setter count it reports is the same
            Reset = () =>
            {
                UdonBehaviour target = MenSharpTestScene.FindUdon("Target");
                target.SetProgramVariable("dial", 0);
                target.SetProgramVariable("dialChanges", 0);
            },
        },
        new Fixture { Type = "VerifyTarget", Object = "Target", Events = new[] { "_start" } },
        new Fixture { Type = "VerifyArrays" },
        new Fixture { Type = "VerifyAsync", Timeout = 6f },
        new Fixture { Type = "VerifyCollections" },
        new Fixture { Type = "VerifyComparers" },
        new Fixture { Type = "VerifyCrash" },
        new Fixture { Type = "VerifyData" },
        new Fixture { Type = "VerifyDeconstruct" },
        new Fixture { Type = "VerifyDelegates" },
        new Fixture { Type = "VerifyEnumNames" },
        new Fixture { Type = "VerifyExternCrash" },
        new Fixture { Type = "VerifyFormat" },
        new Fixture
        {
            Type = "VerifyGetComponent",
            Setup = self =>
            {
                self.AddComponent(MenSharpTestScene.FindType("VerifyTarget"));
                AddUdonSharp(self, "UCounter");
            },
        },
        new Fixture { Type = "VerifyIteratorCleanup" },
        new Fixture { Type = "VerifyIterators", Timeout = 7f },
        new Fixture { Type = "VerifyJagged" },
        new Fixture { Type = "VerifyLinq" },
        new Fixture { Type = "VerifyLocalFunctions" },
        new Fixture { Type = "VerifyModern" },
        new Fixture
        {
            Type = "VerifyNetwork",
            Skip = "network events need ClientSim; without it the SDK throws inside SendCustomNetworkEvent",
            Setup = self => MenSharpTestScene.Assign(
                MenSharpTestScene.Proxy(self, "VerifyNetwork"),
                "target",
                MenSharpTestScene.Proxy(GameObject.Find("Target"), "VerifyTarget")),
        },
        new Fixture { Type = "VerifyNullable" },
        new Fixture { Type = "VerifyRange" },
        new Fixture { Type = "RecordVerify.VerifyRecords" },
        new Fixture { Type = "VerifyRectangular" },
        new Fixture
        {
            Type = "VerifyRemoteAwait",
            Timeout = 6f,
            Setup = self => MenSharpTestScene.Assign(
                MenSharpTestScene.Proxy(self, "VerifyRemoteAwait"),
                "door",
                MenSharpTestScene.Proxy(GameObject.Find("VerifyRemoteDoor"), "VerifyRemoteDoor")),
        },
        new Fixture { Type = "VerifyRemoteDoor", Events = new string[0] },
        new Fixture { Type = "VerifySequences" },
        new Fixture { Type = "VerifyString" },
        new Fixture { Type = "VerifyStructKeys" },
        new Fixture { Type = "VerifyTuple" },
        new Fixture
        {
            Type = "VerifyUdonSharp",
            Setup = self => MenSharpTestScene.Assign(
                MenSharpTestScene.Proxy(self, "VerifyUdonSharp"),
                "counter",
                Extra("UCounter").GetComponent(MenSharpTestScene.FindType("UCounter"))),
        },
        new Fixture { Type = "UnionVerify.VerifyUnion" },
        new Fixture { Type = "VerifyWake", Timeout = 3f },
        new Fixture { Type = "JsonVerify.VerifyJson" },
        new Fixture { Type = "VerifyResult" },
        new Fixture
        {
            Type = "VerifyHttp",
            Setup = self => MenSharpTestScene.Assign(
                MenSharpTestScene.Proxy(self, "VerifyHttp"), "url", new VRC.SDKBase.VRCUrl("not a url")),
        },
        new Fixture
        {
            Type = "VerifyStringLoad",
            Setup = self => MenSharpTestScene.Assign(
                MenSharpTestScene.Proxy(self, "VerifyStringLoad"), "url", new VRC.SDKBase.VRCUrl("not a url")),
        },
    };

    /// Objects the fixtures refer to that are not fixtures themselves.
    private static readonly Dictionary<string, GameObject> Extras = new Dictionary<string, GameObject>();

    private static GameObject Extra(string name)
    {
        Assert.IsTrue(Extras.ContainsKey(name), $"no extra object {name}");
        return Extras[name];
    }

    [UnityTest]
    public IEnumerator ManualFixturesLogWhatTheirHeadersPromise()
    {
        AssetDatabase.Refresh();
        if (MenSharpTestScene.ScriptReloadPending())
        {
            yield return new WaitForDomainReload();
        }

        EnsureUdonSharpProgram();
        MenSharpCompiler.CompileAll();
        AssetDatabase.Refresh(ImportAssetOptions.ForceSynchronousImport);
        BuildScene();

        yield return new EnterPlayMode();
        yield return null;
        yield return null;
        Assert.IsTrue(Application.isPlaying, "the editor did not enter play mode");
        // a halting fixture logs errors on purpose; the comparison below is
        // what judges them. Set after entering play mode: the domain reload
        // on the way in resets it
        LogAssert.ignoreFailingMessages = true;
        // Every wait in the fixtures is in game time, and a headless
        // editor's frames are erratic: a fraction of a millisecond one
        // moment, several the next, with Time.deltaTime far below the real
        // gap. Running game time faster than the clock keeps the async
        // fixtures' seconds to a few hundred frames whatever the pacing
        // — with the step a single frame may take capped low, so that a
        // frame that did real work (an Interact, its logging) does not carry
        // the clock past the next second and reorder the waits behind it.
        // Not captureFramerate: that starves the editor loop the test runs on.
        Time.timeScale = GameSpeed;
        Time.maximumDeltaTime = MaxStep;

        bool record = Environment.GetEnvironmentVariable(RecordVariable) == "1";
        // one fixture by name, for re-pinning or debugging it alone
        string only = Environment.GetEnvironmentVariable(OnlyVariable);
        if (string.IsNullOrEmpty(only))
        {
            only = null;
        }
        // static, not a local: a closure made before the play-mode domain
        // reload does not survive it, and one made after must not capture
        // the (rebuilt) iterator's locals either
        Captured.Clear();
        Application.logMessageReceived += OnLog;
        var report = new StringBuilder();
        var skipped = new List<string>();
        var timings = new StringBuilder();
        try
        {
            foreach (Fixture fixture in Fixtures)
            {
                if (fixture.Events.Length == 0)
                {
                    continue;
                }
                if (fixture.Skip != null)
                {
                    skipped.Add($"{fixture.Name}: {fixture.Skip}");
                    continue;
                }
                if (only != null && only != fixture.Name)
                {
                    continue;
                }
                UdonBehaviour udon = MenSharpTestScene.FindUdon(fixture.ObjectName, fixture.Type);
                Assert.IsTrue(udon.IsInitialized, $"{fixture.Name} was not initialised by the SDK");
                string[] expected = record ? null : ReadExpected(fixture.Name);

                fixture.Reset?.Invoke();
                Captured.Clear();
                foreach (string eventName in fixture.Events)
                {
                    udon.RunProgram(eventName);
                }
                // let the log settle: everything expected has arrived, or
                // the fixture's own time is up. Game time, like the waits in
                // the fixtures: a headless editor's frames come slowly and
                // unevenly, and Time.time is what Scheduler.Delay counts
                float deadline = Time.time + fixture.Timeout;
                int startFrame = Time.frameCount;
                float startedAt = Time.realtimeSinceStartup;
                float gameStartedAt = Time.time;
                do
                {
                    // a headless editor only runs the player loop when
                    // something asks it to; without this a frame is a second
                    EditorApplication.QueuePlayerLoopUpdate();
                    yield return null;
                }
                while (Time.time < deadline
                       && (expected == null || Captured.Count < expected.Length));
                EditorApplication.QueuePlayerLoopUpdate();
                yield return null;
                EditorApplication.QueuePlayerLoopUpdate();
                yield return null;
                timings.AppendLine($"  {fixture.Name}: {Captured.Count} line(s), "
                    + $"{Time.frameCount - startFrame} frame(s), {Time.realtimeSinceStartup - startedAt:0.0}s real, "
                    + $"{Time.time - gameStartedAt:0.0}s game time");

                string[] actual = Captured.ToArray();
                WriteLines(Path.Combine(MenSharpTestScene.ProjectRoot, "..", "..", ActualFolder), fixture.Name, actual);
                if (record)
                {
                    WriteLines(Path.Combine(MenSharpTestScene.ProjectRoot, ExpectedFolder), fixture.Name, actual);
                    continue;
                }
                string difference = Compare(fixture.Name, expected, actual);
                if (difference != null)
                {
                    report.AppendLine(difference);
                }
            }
        }
        finally
        {
            Application.logMessageReceived -= OnLog;
        }

        Time.timeScale = 1f;
        Time.maximumDeltaTime = DefaultMaxStep;
        yield return new ExitPlayMode();
        AssetDatabase.DeleteAsset(GeneratedScene);
        Debug.Log("MenSharp: fixture timings\n" + timings);
        if (record)
        {
            AssetDatabase.Refresh();
            Debug.Log($"MenSharp: recorded {ExpectedFolder} from this run; review the diff before committing it");
        }
        if (skipped.Count > 0)
        {
            Debug.Log("MenSharp: fixtures not run: " + string.Join("; ", skipped));
        }
        Assert.IsTrue(report.Length == 0, "\n" + report);
    }

    [UnityTearDown]
    public IEnumerator LeavePlayModeAfterAFailure()
    {
        LogAssert.ignoreFailingMessages = false;
        if (Application.isPlaying)
        {
            Time.timeScale = 1f;
            Time.maximumDeltaTime = DefaultMaxStep;
            yield return new ExitPlayMode();
        }
        AssetDatabase.DeleteAsset(GeneratedScene);
    }

    // ------------------------------------------------------------ the scene

    private static void BuildScene()
    {
        MenSharpTestScene.EnsureFolder("Assets/Tests/Generated");
        Extras.Clear();
        var scene = EditorSceneManager.NewScene(NewSceneSetup.EmptyScene, NewSceneMode.Single);

        // far enough apart that a raycast from one cube cannot hit another
        var objects = new List<GameObject>();
        int slot = 0;
        foreach (Fixture fixture in Fixtures)
        {
            objects.Add(MenSharpTestScene.AddProxy(fixture.ObjectName, fixture.Type, Vector3.right * (4 * slot++)));
        }

        // what Verify clones: VRChat can only clone objects already in the
        // scene, and this one starts disabled
        GameObject cloneMe = GameObject.CreatePrimitive(PrimitiveType.Cube);
        cloneMe.name = "Clone Me";
        cloneMe.transform.position = Vector3.right * (4 * slot++);
        cloneMe.SetActive(false);
        Extras["Clone Me"] = cloneMe;

        // the UdonSharp behaviour VerifyUdonSharp talks to
        var counter = new GameObject("UCounter");
        counter.transform.position = Vector3.right * (4 * slot++);
        AddUdonSharp(counter, "UCounter");
        Extras["UCounter"] = counter;

        foreach (Fixture fixture in Fixtures)
        {
            fixture.Setup?.Invoke(GameObject.Find(fixture.ObjectName));
        }

        MenSharpProxy.SyncThenTransfer(objects, false);
        foreach (GameObject target in objects)
        {
            Assert.IsNotNull(target.GetComponent<UdonBehaviour>(), target.name);
        }
        Assert.IsTrue(EditorSceneManager.SaveScene(scene, GeneratedScene));
    }

    /// An UdonSharp behaviour the way its inspector adds one: the proxy
    /// plus the backing UdonBehaviour running the UdonSharp program.
    private static void AddUdonSharp(GameObject target, string typeName)
    {
        Type type = MenSharpTestScene.FindType(typeName);
        Assert.IsTrue(typeof(UdonSharpBehaviour).IsAssignableFrom(type), typeName);
        Assert.IsNotNull(UdonSharpUndo.AddComponent(target, type), typeName);
    }

    /// UdonSharp compiles a behaviour only through a program asset that
    /// points at its script. UCounter's is not in Git (UdonSharp rewrites
    /// it on every compile), so it is made here on first use.
    private static void EnsureUdonSharpProgram()
    {
        if (AssetDatabase.LoadAssetAtPath<UdonSharpProgramAsset>(UdonSharpProgram) == null)
        {
            var script = AssetDatabase.LoadAssetAtPath<MonoScript>(UdonSharpScript);
            Assert.IsNotNull(script, UdonSharpScript);
            var program = ScriptableObject.CreateInstance<UdonSharpProgramAsset>();
            program.sourceCsScript = script;
            AssetDatabase.CreateAsset(program, UdonSharpProgram);
            AssetDatabase.SaveAssets();
            // the type → program lookup was built before the asset existed
            typeof(UdonSharpEditorUtility)
                .GetMethod("ResetCaches", BindingFlags.Static | BindingFlags.NonPublic)
                ?.Invoke(null, null);
        }
        UdonSharpCompilerV1.CompileSync();
        Assert.IsNotNull(
            UdonSharpEditorUtility.GetUdonSharpProgramAsset(MenSharpTestScene.FindType("UCounter")),
            "UdonSharp has no program asset for UCounter");
    }

    // -------------------------------------------------------------- the log

    private static readonly List<string> Captured = new List<string>();

    private static void OnLog(string condition, string trace, LogType type)
    {
        if (type != LogType.Warning)
        {
            Captured.AddRange(Normalize(condition));
        }
    }

    private static readonly Regex SourcePosition = new Regex(@"\.cs:\d+:\d+", RegexOptions.Compiled);
    private static readonly Regex Approximate = new Regex(@"~\d+(\.\d+)?", RegexOptions.Compiled);
    private const string VmHaltReport = "An exception occurred during Udon execution";

    /// A log entry as lines the comparison can rely on: source positions
    /// (which move with every edit of a fixture) and values the fixture
    /// itself calls approximate (`~1.0`) are masked; the VM's halt report
    /// is cut to its first line (the rest is a heap dump).
    private static IEnumerable<string> Normalize(string condition)
    {
        string[] lines = condition.Replace("\r", "").Split('\n');
        if (lines[0].Contains(VmHaltReport))
        {
            lines = new[] { lines[0] };
        }
        int count = lines.Length;
        while (count > 0 && lines[count - 1].Trim().Length == 0)
        {
            count--;
        }
        for (int index = 0; index < count; index++)
        {
            string line = lines[index].TrimEnd();
            line = SourcePosition.Replace(line, ".cs:L:C");
            line = Approximate.Replace(line, "~N");
            yield return line;
        }
    }

    private static string[] ReadExpected(string fixture)
    {
        string path = Path.Combine(MenSharpTestScene.ProjectRoot, ExpectedFolder, fixture + ".txt");
        if (!File.Exists(path))
        {
            return null;
        }
        return File.ReadAllLines(path).SelectMany(Normalize).ToArray();
    }

    private static void WriteLines(string folder, string fixture, string[] lines)
    {
        Directory.CreateDirectory(folder);
        File.WriteAllText(Path.Combine(folder, fixture + ".txt"), string.Join("\n", lines) + "\n");
    }

    private static string Compare(string fixture, string[] expected, string[] actual)
    {
        if (expected == null)
        {
            return $"{fixture}: no {ExpectedFolder}/{fixture}.txt — run once with {RecordVariable}=1 to make it";
        }
        if (expected.SequenceEqual(actual))
        {
            return null;
        }
        var text = new StringBuilder();
        text.AppendLine($"{fixture}: logged {actual.Length} line(s), expected {expected.Length}");
        int shared = Math.Min(expected.Length, actual.Length);
        for (int index = 0; index < Math.Max(expected.Length, actual.Length); index++)
        {
            string want = index < expected.Length ? expected[index] : null;
            string got = index < actual.Length ? actual[index] : null;
            if (index < shared && want == got)
            {
                continue;
            }
            text.AppendLine($"  line {index + 1}");
            text.AppendLine($"    expected: {want ?? "<nothing>"}");
            text.AppendLine($"    actual:   {got ?? "<nothing>"}");
        }
        return text.ToString();
    }
}
#endif

# MenSharp Unity integration tests

A minimal Unity 2022.3.22f1 project that checks the one boundary the Rust
test suite cannot: the package is installed the way a user installs it, the
compiler's output is assembled into `MenSharpProgramAsset`s by the real
VRChat SDK, and the resulting bytecode runs in the SDK's Udon VM.

`Assets/MenSharp` holds the manual verification corpus from the development
world (the `Verify*` behaviours) plus three small black-box programs. The
suite in `Assets/Tests/Editor/MenSharpIntegrationTests.cs` has two tests:

- every fixture compiles and becomes an SDK program asset, and recompiling
  unchanged sources rewrites none of them;
- the black-box programs execute in the SDK's Udon VM in play mode:
  synchronous code, LINQ over the mini-corlib, `async` resumption across
  frames, and a call from one behaviour into another.

The `Verify*` behaviours are still read by a person, in a world: they need
ClientSim, a player and clicks, and they report by log line.

## Running it locally

```sh
./tools/test-unity.sh                          # the whole suite
./tools/test-unity.sh GeneratedProgramsExecute  # one test, by NUnit filter
```

The script builds the compiler, stages the package into `Packages/`, links
the SDK in, and runs Unity headless. It uses `UNITY_EDITOR` and
`MENSHARP_VRC_PROJECT` when set; otherwise it looks for Unity 2022.3.22f1
under Unity Hub's default location and borrows the SDK packages from
`~/ALCOM/Projects/MenSharpTest` by symlink. With `vrc-get` installed the
pinned SDK is resolved without a donor project instead.

Results go to `artifacts/unity-tests/results.xml` (NUnit) and
`artifacts/unity-tests/Editor.log`. A warm run takes about a minute; the
first import of the SDK takes several.

## What is (not) in Git

Only sources, settings and metas are tracked. The staged package, the SDK
packages, Unity's `Library`, generated program assets, the test scene, logs
and results are ignored — no SDK code or binaries ever enter the repository
(see `data/udon/README.md`). `Assets/UdonSharp/Settings` is tracked because
it holds the one setting the tests need on a fresh checkout: UdonSharp's
scanner skips `Assets/MenSharp`, which is C# 9 and would otherwise break its
own compile pass.

## On GitHub Actions

The `unity-integration` job in `.github/workflows/ci.yml` resolves the
pinned SDK with `vrc-get`, builds the compiler as a static (musl) binary so
it runs inside GameCI's container, and runs this same assembly with
`game-ci/unity-test-runner`. The Unity `Library` folder is cached between
runs.

The repository needs the licensing secrets GameCI asks for: `UNITY_LICENSE`
(the contents of a `.ulf` file for a personal licence), plus `UNITY_EMAIL`
and `UNITY_PASSWORD`. The job is skipped for pull requests from forks, which
cannot read them.

# MenSharp Unity integration tests

A minimal Unity 2022.3.22f1 project that checks the one boundary the Rust
test suite cannot: the package is installed the way a user installs it, the
compiler's output is assembled into `MenSharpProgramAsset`s by the real
VRChat SDK, and the resulting bytecode runs in the SDK's Udon VM.

`Assets/MenSharp` holds the manual verification corpus from the development
world (the `Verify*` behaviours) plus three small black-box programs. The
suite in `Assets/Tests/Editor` has three tests:

- every fixture compiles and becomes an SDK program asset, and recompiling
  unchanged sources rewrites none of them
  (`MenSharpIntegrationTests.CompilerCreatesEveryManualFixtureAsAnSdkProgramAsset`);
- the black-box programs execute in the SDK's Udon VM in play mode:
  synchronous code, LINQ over the mini-corlib, `async` resumption across
  frames, and a call from one behaviour into another
  (`MenSharpIntegrationTests.GeneratedProgramsExecuteInTheSdkUdonVm`);
- the `Verify*` corpus runs the way a person runs it in the world — each
  behaviour's `Interact` (and `Start`, where it logs) raised on the SDK's
  real UdonBehaviour — and what each one logs is compared line by line
  with `Assets/Tests/Editor/Expected/<Fixture>.txt`
  (`MenSharpVerifyFixtureTests.ManualFixturesLogWhatTheirHeadersPromise`).

The expected files are the "Expected:" blocks from the fixtures' own
headers, pinned to what the test scene produces (Unity's own formatting,
`(1.00, 3.00, 1.00)` for a Vector3, and UdonSharp's, `1` for one of its
enums). Source positions in stack traces and values a fixture itself calls
approximate (`~1.0`) are masked before comparing. The fixtures' waits are in
game time, which the test runs twenty times faster than the clock (a
headless editor's frames are slow and uneven) with the step of a single
frame capped, so a two-second wait costs a fraction of a second and the
order of the waits holds. After a deliberate change
to a fixture, re-pin with

```sh
MENSHARP_VERIFY_RECORD=1 ./tools/test-unity.sh MenSharpVerifyFixtureTests
```

and review the diff of `Expected/` before committing it. Add
`MENSHARP_VERIFY_ONLY=VerifyLinq` to run (or re-pin) one fixture. Every run
also writes what it saw to `artifacts/unity-tests/verify/`.

Not covered: `VerifyNetwork` (network events need ClientSim, which needs a
scene descriptor and a player; it is in the scene, so it still has to
compile and pair) and, for `VerifyExternCrash`, the editor-side line that
names where the VM halted — it is logged from inside a log callback, which
the test's own callback does not see. Both remain manual.

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
first import of the SDK takes several more.

## What is (not) in Git

Only sources, settings and metas are tracked. The staged package, the SDK
packages, Unity's `Library`, generated program assets, the test scene, logs
and results are ignored — no SDK code or binaries ever enter the repository
(see `data/udon/README.md`). `Assets/UdonSharp/Settings` is tracked because
it holds the one setting the tests need on a fresh checkout: UdonSharp's
scanner skips `Assets/MenSharp`, which is C# 9 and would otherwise break its
own compile pass. UdonSharp's program asset for `UCounter` is not tracked
(UdonSharp rewrites it on every compile); the fixture test creates it on
first use.

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

Two repository variables (Settings > Secrets and variables > Actions >
Variables) move the job to a self-hosted runner:

- `MENSHARP_UNITY_RUNNER`: a runner label — `self-hosted`, or one runner's
  name such as `msci-01`. Unset, the job runs on `ubuntu-latest`. With only
  this set, the job still uses GameCI's container, so the runner needs
  Docker (and the runner's user in the `docker` group), and the licence
  secrets still apply; the container image is cached on the runner after
  the first run.
- `MENSHARP_UNITY_EDITOR`: the path of a Unity 2022.3.22f1 editor installed
  on that runner, for example
  `/home/ci/Unity/Hub/Editor/2022.3.22f1/Editor/Unity`. With this set the
  job skips the container and runs `tools/test-unity.sh` with that editor,
  which uses the runner's own activation — no licence secrets, no Docker.

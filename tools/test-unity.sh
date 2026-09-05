#!/usr/bin/env bash
# Runs the SDK-backed integration suite without opening the Unity UI — the
# same thing CI does, against the local Unity install.
#
#   tools/test-unity.sh [nunit test filter]
#
# Environment:
#   UNITY_EDITOR           the Unity 2022.3.22f1 binary (default: Unity Hub's)
#   MENSHARP_VRC_PROJECT   the Worlds project to borrow the SDK from (default:
#                          ~/ALCOM/Projects/MenSharpTest when vrc-get is absent)
#
# Results land in artifacts/unity-tests: results.xml (NUnit) and Editor.log.
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
project="$repo/tests/unity-project"
artifacts="$repo/artifacts/unity-tests"
unity="${UNITY_EDITOR:-$HOME/Unity/Hub/Editor/2022.3.22f1/Editor/Unity}"
filter="${1:-}"

[[ -x "$unity" ]] || {
    echo "Unity editor not found at $unity; set UNITY_EDITOR" >&2
    exit 1
}

if [[ -z "${MENSHARP_VRC_PROJECT:-}" ]] && ! command -v vrc-get >/dev/null 2>&1; then
    default_donor="$HOME/ALCOM/Projects/MenSharpTest"
    if [[ -d "$default_donor/Packages/com.vrchat.worlds" ]]; then
        export MENSHARP_VRC_PROJECT="$default_donor"
    fi
fi

"$repo/tools/prepare-unity-tests.sh"

# a stale results.xml must never pass for a run that produced none
rm -rf "$artifacts"
mkdir -p "$artifacts"

arguments=(
    -batchmode
    -nographics
    -projectPath "$project"
    -runTests
    -testPlatform EditMode
    -assemblyNames ProjectTesca.MenSharp.IntegrationTests
    -testResults "$artifacts/results.xml"
    -logFile "$artifacts/Editor.log"
)
if [[ -n "$filter" ]]; then
    arguments+=(-testFilter "$filter")
fi

# Unity exits 2 when a test fails and 3 when the run itself broke; the
# results file (or its absence) says which
status=0
"$unity" "${arguments[@]}" || status=$?

if [[ ! -s "$artifacts/results.xml" ]]; then
    echo "Unity produced no test results (exit $status); see $artifacts/Editor.log" >&2
    grep -E 'error CS|Exception|Assertion' "$artifacts/Editor.log" | head -20 >&2 || true
    exit 1
fi
if ! grep -Eq '<test-run [^>]*result="Passed"' "$artifacts/results.xml"; then
    echo "Unity tests did not all pass (exit $status); see $artifacts/results.xml" >&2
    grep -E '<test-case ' "$artifacts/results.xml" \
        | sed -E 's/.*name="([^"]+)".*result="([^"]+)".*/  \2  \1/' >&2 || true
    exit 1
fi
echo "Unity tests passed: $artifacts/results.xml"

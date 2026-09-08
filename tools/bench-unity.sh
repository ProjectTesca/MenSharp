#!/usr/bin/env bash
# Compile-speed benchmark, UdonSharp against MenSharp, inside a headless
# Unity editor over the corpus tools/prepare-bench.sh stages.
#
#   tools/bench-unity.sh
#
# Environment: as tools/test-unity.sh (UNITY_EDITOR, MENSHARP_VRC_PROJECT).
# Results land in artifacts/bench/results.txt, the log in artifacts/bench/Editor.log.
set -euo pipefail
repo="$(cd "$(dirname "$0")/.." && pwd)"
project="$repo/tests/unity-project"
artifacts="$repo/artifacts/bench"
unity="${UNITY_EDITOR:-$HOME/Unity/Hub/Editor/2022.3.22f1/Editor/Unity}"
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
"$repo/tools/prepare-bench.sh"

rm -rf "$artifacts"
mkdir -p "$artifacts"
export MENSHARP_BENCH_RESULTS="$artifacts/results.txt"

status=0
"$unity" -batchmode -nographics -projectPath "$project" \
    -executeMethod MenSharpBenchmark.Run -quit \
    -logFile "$artifacts/Editor.log" || status=$?

if [[ ! -s "$artifacts/results.txt" ]]; then
    echo "Unity produced no results (exit $status); see $artifacts/Editor.log" >&2
    grep -E 'error CS|Exception|Assertion' "$artifacts/Editor.log" | head -20 >&2 || true
    exit 1
fi
cat "$artifacts/results.txt"

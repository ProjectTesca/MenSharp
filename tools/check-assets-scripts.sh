#!/usr/bin/env bash
# Type-checks a project's Assets/MenSharp scripts the way Unity does.
#
#   tools/check-assets-scripts.sh [unity project] [unity editor data dir]
#
# M# accepts newer C# than Unity does: Unity 2022.3 compiles Assets scripts as
# C# 9, and a script it cannot compile has no class — so Unity refuses to add
# the component, whatever M# thinks of the file. This runs Roslyn over those
# scripts at Unity's own language version, which catches that in a second
# instead of after a round trip through the editor. Skips (exit 0) when the
# project, the editor or a .NET SDK is missing — CI has none of them.
set -euo pipefail

project="${1:-$HOME/ALCOM/Projects/MenSharpTest}"
unity_data="${2:-$HOME/Unity/Hub/Editor/2022.3.22f1/Editor/Data}"
# Unity 2021.3 and later fix the language version of Assets scripts at C# 9
langversion="${MENSHARP_LANGVERSION:-9.0}"

skip() {
    echo "skipped: $1" >&2
    exit 0
}

[[ -d "$unity_data" ]] || skip "no Unity install at $unity_data"
[[ -d "$project/Library/ScriptAssemblies" ]] || skip "no built Unity project at $project"
compile_dir="$project/Assets/MenSharp"
[[ -d "$compile_dir" ]] || skip "no $compile_dir"

csc="$( { find /usr/share/dotnet/sdk /usr/lib/dotnet/sdk -name csc.dll -path '*Roslyn*' 2>/dev/null || true; } | sort | tail -1)"
[[ -n "$csc" ]] || skip "no Roslyn csc.dll from a .NET SDK"

assemblies="$project/Library/ScriptAssemblies"
worlds="$project/Packages/com.vrchat.worlds/Runtime"
base_sdk="$project/Packages/com.vrchat.base/Runtime/VRCSDK/Plugins"

references=()
for candidate in \
    "$unity_data/MonoBleedingEdge/lib/mono/unityaot-linux/mscorlib.dll" \
    "$unity_data/UnityReferenceAssemblies/unity-4.8-api/Facades/netstandard.dll" \
    "$unity_data/MonoBleedingEdge/lib/mono/unityaot-linux/System.dll" \
    "$unity_data/MonoBleedingEdge/lib/mono/unityaot-linux/System.Core.dll" \
    "$unity_data/Managed/UnityEngine/UnityEngine.dll" \
    "$unity_data"/Managed/UnityEngine/UnityEngine.*Module.dll \
    "$assemblies/MenSharpTestUSharp.dll" \
    "$assemblies/UdonSharp.Lib.dll" \
    "$assemblies/UdonSharp.Runtime.dll" \
    "$assemblies/VRC.Udon.dll" \
    "$worlds/Udon/External/VRC.Udon.Common.dll" \
    "$base_sdk/VRCSDKBase.dll" \
    "$worlds/VRCSDK/Plugins/VRCSDK3.dll"
do
    [[ -f "$candidate" ]] && references+=("-r:$candidate")
done

output="$(mktemp -d)"
trap 'rm -rf "$output"' EXIT

echo "type-checking $compile_dir as C# $langversion (what Unity uses)"
# the package's Runtime sources are compiled in rather than referenced as
# Unity's built dll, so a twin class added to the package counts before
# Unity has rebuilt it
runtime_dir="$project/Packages/com.projecttesca.mensharp/Runtime"
[[ -d "$runtime_dir" ]] || runtime_dir="$(cd "$(dirname "$0")/.." && pwd)/unity/com.projecttesca.mensharp/Runtime"
dotnet "$csc" -nologo -target:library -nostdlib -langversion:"$langversion" \
    "${references[@]}" -out:"$output/assets.dll" "$compile_dir"/*.cs "$runtime_dir"/*.cs
echo "ok"

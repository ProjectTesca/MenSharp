#!/usr/bin/env bash
# Type-checks the Unity package's C# without opening Unity.
#
#   tools/check-unity-scripts.sh [unity project] [unity editor data dir]
#
# Unity only reports script errors after an import round-trip, which makes a
# typo in the editor scripts cost a full editor restart to find. This compiles
# them with Roslyn against the very assemblies Unity would use, so the same
# errors show up in a second. It needs a Unity install and a VRChat project to
# borrow reference assemblies from, and skips (exit 0) when either is missing —
# CI has neither.
set -euo pipefail

project="${1:-$HOME/ALCOM/Projects/MenSharpTest}"
unity_data="${2:-$HOME/Unity/Hub/Editor/2022.3.22f1/Editor/Data}"
repo="$(cd "$(dirname "$0")/.." && pwd)"
package="$repo/unity/com.projecttesca.mensharp"

skip() {
    echo "skipped: $1" >&2
    exit 0
}

[[ -d "$unity_data" ]] || skip "no Unity install at $unity_data"
[[ -d "$project/Library/ScriptAssemblies" ]] || skip "no built Unity project at $project"

# `|| true`: find exits non-zero for the SDK root that does not exist here,
# and pipefail would otherwise take the whole script down with it
csc="$( { find /usr/share/dotnet/sdk /usr/lib/dotnet/sdk -name csc.dll -path '*Roslyn*' 2>/dev/null || true; } | sort | tail -1)"
[[ -n "$csc" ]] || skip "no Roslyn csc.dll from a .NET SDK"

scripts="$project/Library/ScriptAssemblies"
worlds="$project/Packages/com.vrchat.worlds"
base="$project/Packages/com.vrchat.base"

refs=()
for assembly in \
    "$unity_data/Managed/UnityEngine.dll" \
    "$unity_data/Managed/UnityEditor.dll" \
    "$unity_data"/Managed/UnityEngine/*.dll \
    "$unity_data/NetStandard/ref/2.1.0/netstandard.dll" \
    "$unity_data/NetStandard/compat/2.1.0/shims/netfx/mscorlib.dll" \
    "$scripts/VRC.Udon.dll" \
    "$scripts/VRC.Udon.Editor.dll" \
    "$scripts/VRC.SDKBase.dll" \
    "$worlds/Runtime/Udon/External/VRC.Udon.Common.dll" \
    "$worlds/Editor/Udon/External/VRC.Udon.EditorBindings.dll" \
    "$worlds/Editor/Udon/External/VRC.Udon.UAssembly.dll" \
    "$worlds/Runtime/VRCSDK/Plugins/VRCSDK3.dll" \
    "$base/Runtime/VRCSDK/Plugins/VRCSDKBase.dll" \
    "$base/Runtime/VRCSDK/Dependencies/Managed/System.Collections.Immutable.dll"
do
    [[ -f "$assembly" ]] && refs+=("-r:$assembly")
done

output="$(mktemp -d)/mensharp-editor.dll"
echo "type-checking $package (.NET SDK $(basename "$(dirname "$(dirname "$(dirname "$csc")")")"))"
# CS1701 is the SDK's own System.Collections.Immutable binding redirect
dotnet "$csc" -nologo -target:library -nostdlib+ -noconfig -langversion:9 \
    -define:UNITY_EDITOR "${refs[@]}" -out:"$output" \
    "$package"/Runtime/*.cs "$package"/Editor/*.cs \
    2>&1 | grep -v 'warning CS1701' || true

[[ -f "$output" ]] || { echo "the Unity scripts do not compile" >&2; exit 1; }
echo "ok"

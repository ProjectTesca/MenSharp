#!/usr/bin/env bash
# Stages the compile-speed benchmark corpus into the Unity test project, as
# two variants of the same sources:
#
#   Assets/Bench/MenSharp/    MenSharpBehaviour subclasses (namespace MenSharpBench)
#   Assets/Bench/UdonSharp/   UdonSharpBehaviour subclasses (namespace UdonSharpBench)
#
# The sources are tests/bench-corpus/*.cs (written in the subset both
# compilers accept) plus the UdonSharp sample scripts that ship with the
# VRChat Worlds SDK, borrowed from the donor project at run time and never
# committed. Both output folders are git-ignored.
#
#   tools/prepare-bench.sh
#
# Environment:
#   MENSHARP_VRC_PROJECT   the Worlds project whose SDK samples to borrow
#                          (default: ~/ALCOM/Projects/MenSharpTest)
set -euo pipefail
repo="$(cd "$(dirname "$0")/.." && pwd)"
project="$repo/tests/unity-project"
corpus="$repo/tests/bench-corpus"
donor="${MENSHARP_VRC_PROJECT:-$HOME/ALCOM/Projects/MenSharpTest}"
samples="$donor/Packages/com.vrchat.worlds/Integrations/UdonSharp/Samples~"

mensharp="$project/Assets/Bench/MenSharp"
udonsharp="$project/Assets/Bench/UdonSharp"
rm -rf "$mensharp" "$udonsharp"
mkdir -p "$mensharp" "$udonsharp"

# UdonSharp spells these as overrides of UdonSharpBehaviour virtuals;
# MenSharp recognises them by name. Same source, one word apart.
events='OnOwnershipRequest|Interact|OnDeserialization|OnPreSerialization|OnPostSerialization|OnPlayerJoined|OnPlayerLeft|OnPlayerTriggerEnter|OnPlayerTriggerExit|OnPlayerTriggerStay|OnOwnershipTransferred|OnPickup|OnDrop|OnPickupUseDown|OnPickupUseUp|OnPlayerRespawn|OnStationEntered|OnStationExited'

to_udonsharp() {
    sed -E "1s/^\xEF\xBB\xBF//; s/using MenSharp;/using UdonSharp;/; s/MenSharpBehaviour/UdonSharpBehaviour/g; s/namespace MenSharpBench/namespace UdonSharpBench/; s/public (void|bool) ($events)\(/public override \1 \2(/"
}
to_mensharp() {
    sed -E "1s/^\xEF\xBB\xBF//; s/using UdonSharp;/using MenSharp;/; s/UdonSharpBehaviour/MenSharpBehaviour/g; s/namespace UdonSharp\.Examples/namespace MenSharpBench.Examples/; s/public override (void|bool) ($events)\(/public \1 \2(/; s/BehaviourSyncMode\.NoVariableSync/BehaviourSyncMode.None/"
}

count=0
for source in "$corpus"/*.cs; do
    name="$(basename "$source")"
    cp "$source" "$mensharp/$name"
    to_udonsharp < "$source" > "$udonsharp/$name"
    count=$((count + 1))
done

# the SDK's own UdonSharp samples: the tutorial and utility scripts (the
# custom-inspector pair needs editor scripts, PlayerModSetter destroys itself)
if [[ -d "$samples" ]]; then
    while IFS= read -r source; do
        name="$(basename "$source")"
        case "$name" in PlayerModSetter.cs) continue ;; esac
        # the samples reach UdonSharpBehaviour through their enclosing
        # namespace; both variants get an explicit using instead
        {
            grep -q '^using UdonSharp;' "$source" || echo 'using UdonSharp;'
            cat "$source"
        } | sed -E "1s/^\xEF\xBB\xBF//; s/^\xEF\xBB\xBFusing/using/; s/namespace UdonSharp\.Examples/namespace UdonSharpBench.Examples/" > "$udonsharp/$name"
        {
            grep -q '^using UdonSharp;' "$source" || echo 'using UdonSharp;'
            cat "$source"
        } | to_mensharp > "$mensharp/$name"
        count=$((count + 1))
    done < <(find "$samples/Tutorials" "$samples/Utilities" -name '*.cs' | sort)
else
    echo "no SDK samples at $samples; corpus is tests/bench-corpus only" >&2
fi

cat > "$mensharp/Bench.MenSharp.asmdef" <<'JSON'
{
    "name": "Bench.MenSharp",
    "rootNamespace": "",
    "references": [
        "ProjectTesca.MenSharp.Runtime",
        "VRC.Udon"
    ],
    "includePlatforms": [],
    "excludePlatforms": [],
    "allowUnsafeCode": false,
    "overrideReferences": false,
    "precompiledReferences": [],
    "autoReferenced": true,
    "defineConstraints": [],
    "versionDefines": [],
    "noEngineReferences": false
}
JSON
echo "staged $count scripts x 2 variants under $project/Assets/Bench"

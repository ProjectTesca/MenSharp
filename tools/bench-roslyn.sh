#!/usr/bin/env bash
# Wall-clock comparison against Roslyn (`csc`) on the same sources.
#
#   tools/bench-roslyn.sh [--runs N] [--warmup N]
#
# Both compilers are given the *same* .cs files and the *same* reference
# assemblies, and both are measured end to end as the user invokes them:
# process start, references read, sources compiled, output written.
#
# The corpus is tests/bench-corpus scaled by duplication (each copy gets its
# own namespace, so the class count grows with it), plus one trivial file as
# a "floor" measurement — what each compiler costs before it has looked at
# any real code.
#
# Four configurations:
#
#   mensharp        C# -> one Udon program per behaviour (.uasm/.meta.json/.uprog)
#   mensharp-check  the front end only: parse, resolve, type-check, no output
#   roslyn-warm     csc -shared, i.e. through a VBCSCompiler build server that
#                   is already running and JIT-warm (what an incremental
#                   `dotnet build` gets)
#   roslyn-cold     csc without a build server: a fresh process every run, so
#                   the .NET host starts and Roslyn is JIT-compiled each time
#                   (what the first build after opening a project gets)
#
# M# has no build server and no warm mode: every run is a cold run.
#
# Every (configuration, corpus size) pair is measured once per repetition, in
# a fresh random order each time. Runs of one configuration are therefore
# spread over the whole session: neither CPU frequency drift nor the state a
# neighbouring run leaves the machine in can favour one configuration.
#
# Environment:
#   UNITY_EDITOR_DATA     Unity's Editor/Data (default: Hub's 2022.3.22f1)
#   MENSHARP_VRC_PROJECT  a Worlds project whose Packages/ and
#                         Library/ScriptAssemblies/ hold the SDK and the M#
#                         runtime assembly (default: ~/ALCOM/Projects/Test)
#   DOTNET_SDK            the .NET SDK directory holding Roslyn/bincore/csc.dll
#                         (default: the newest under /usr/share/dotnet/sdk)
#
# Samples land in artifacts/bench-roslyn/samples.tsv (one line per run) and
# the aggregate in summary.tsv. docs/bench/roslyn.typ draws them.
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
out="$repo/artifacts/bench-roslyn"
runs=20
warmup=3

while [[ $# -gt 0 ]]; do
    case "$1" in
        --runs) runs="$2"; shift 2 ;;
        --warmup) warmup="$2"; shift 2 ;;
        *) echo "unknown argument: $1" >&2; exit 1 ;;
    esac
done

unity="${UNITY_EDITOR_DATA:-$HOME/Unity/Hub/Editor/2022.3.22f1/Editor/Data}"
project="${MENSHARP_VRC_PROJECT:-$HOME/ALCOM/Projects/Test}"
sdk="${DOTNET_SDK:-$(ls -d /usr/share/dotnet/sdk/*/ 2>/dev/null | sort -V | tail -1)}"
csc="${sdk%/}/Roslyn/bincore/csc.dll"

references=(
    # mscorlib and the netstandard facade Unity itself hands to Roslyn
    "$unity/MonoBleedingEdge/lib/mono/unityjit-linux/mscorlib.dll"
    "$unity/MonoBleedingEdge/lib/mono/4.5/Facades/netstandard.dll"
    "$unity/Managed/UnityEngine/UnityEngine.CoreModule.dll"
    "$unity/Managed/UnityEngine/UnityEngine.PhysicsModule.dll"
    # MenSharpBehaviour and the attributes, as the editor sees them
    "$project/Library/ScriptAssemblies/ProjectTesca.MenSharp.Runtime.dll"
    "$project/Library/ScriptAssemblies/VRC.Udon.dll"
    "$project/Packages/com.vrchat.worlds/Runtime/Udon/External/VRC.Udon.Common.dll"
    "$project/Packages/com.vrchat.base/Runtime/VRCSDK/Plugins/VRCSDKBase.dll"
    "$project/Packages/com.vrchat.worlds/Runtime/VRCSDK/Plugins/VRCSDK3.dll"
)
for reference in "${references[@]}"; do
    [[ -f "$reference" ]] || { echo "missing reference: $reference" >&2; exit 1; }
done
[[ -f "$csc" ]] || { echo "csc not found at $csc; set DOTNET_SDK" >&2; exit 1; }

cargo build --release -p men-sharp --manifest-path "$repo/Cargo.toml"
mensharp="$repo/target/release/men-sharp"

scales=(1 2 4 8 16 32)

rm -rf "$out"
mkdir -p "$out"

# scale 0: the floor — one behaviour with one empty method
mkdir -p "$out/corpus-x0"
cat > "$out/corpus-x0/Empty.cs" <<'CS'
using MenSharp;
using UnityEngine;

namespace MenSharpBench.C0
{
    public class Empty : MenSharpBehaviour
    {
        public void Start()
        {
        }
    }
}
CS

for scale in "${scales[@]}"; do
    mkdir -p "$out/corpus-x$scale"
    for copy in $(seq 1 "$scale"); do
        for source in "$repo/tests/bench-corpus"/*.cs; do
            name="$(basename "$source" .cs)"
            sed -E "s/namespace MenSharpBench/namespace MenSharpBench.C$copy/" \
                "$source" > "$out/corpus-x$scale/${name}_$copy.cs"
        done
    done
done

mensharp_arguments=()
roslyn_arguments=()
for reference in "${references[@]}"; do
    mensharp_arguments+=(--reference "$reference")
    roslyn_arguments+=(-r:"$reference")
done

run_mensharp() {
    rm -rf "$out/programs"
    mkdir -p "$out/programs"
    "$mensharp" "${mensharp_arguments[@]}" --emit-udon-all --out-dir "$out/programs" \
        --error-format short "$out/corpus-x$1"/*.cs >/dev/null 2>&1
}
run_mensharp_check() {
    "$mensharp" "${mensharp_arguments[@]}" --error-format short \
        "$out/corpus-x$1"/*.cs >/dev/null 2>&1
}
run_roslyn_warm() {
    dotnet exec "$csc" -nologo -noconfig -nostdlib+ -shared -target:library \
        -langversion:9.0 -debug- -optimize- -out:"$out/bench.dll" \
        "${roslyn_arguments[@]}" "$out/corpus-x$1"/*.cs >/dev/null 2>&1
}
run_roslyn_cold() {
    dotnet exec "$csc" -nologo -noconfig -nostdlib+ -target:library \
        -langversion:9.0 -debug- -optimize- -out:"$out/bench.dll" \
        "${roslyn_arguments[@]}" "$out/corpus-x$1"/*.cs >/dev/null 2>&1
}

configurations=(mensharp mensharp_check roslyn_warm roslyn_cold)

# a compile that fails is not a compile: every configuration is checked once
# before anything is timed
for scale in 0 "${scales[@]}"; do
    for configuration in "${configurations[@]}"; do
        "run_$configuration" "$scale" || {
            echo "$configuration failed on corpus x$scale; rerun it by hand to see why" >&2
            exit 1
        }
    done
done

for _ in $(seq 1 "$warmup"); do
    for scale in 0 "${scales[@]}"; do
        for configuration in "${configurations[@]}"; do
            "run_$configuration" "$scale"
        done
    done
done

cells=()
for scale in 0 "${scales[@]}"; do
    for configuration in "${configurations[@]}"; do
        cells+=("$configuration $scale")
    done
done

printf 'configuration\tscale\tmicroseconds\n' > "$out/samples.tsv"
for rep in $(seq 1 "$runs"); do
    while read -r configuration scale; do
        start=$(date +%s%N)
        "run_$configuration" "$scale"
        end=$(date +%s%N)
        printf '%s\t%s\t%s\n' "$configuration" "$scale" "$(((end - start) / 1000))" \
            >> "$out/samples.tsv"
    done < <(printf '%s\n' "${cells[@]}" | shuf)
    echo "rep $rep/$runs" >&2
done

python3 - "$out" "${scales[@]}" <<'PY' | tee "$out/summary.tsv"
import statistics, subprocess, sys, collections, pathlib

out = pathlib.Path(sys.argv[1])
scales = [0] + [int(scale) for scale in sys.argv[2:]]

samples = collections.defaultdict(list)
with (out / "samples.tsv").open() as handle:
    next(handle)
    for line in handle:
        configuration, scale, microseconds = line.split()
        samples[(configuration, int(scale))].append(int(microseconds) / 1000)

def corpus(scale):
    files = sorted((out / f"corpus-x{scale}").glob("*.cs"))
    lines = sum(len(path.read_text().splitlines()) for path in files)
    return len(files), lines

print("configuration\tscale\tfiles\tlines\truns\tmean_ms\tsd_ms\tmin_ms")
for scale in scales:
    files, lines = corpus(scale)
    for configuration in ("mensharp", "mensharp_check", "roslyn_warm", "roslyn_cold"):
        values = samples.get((configuration, scale))
        if not values:
            continue
        print(f"{configuration}\t{scale}\t{files}\t{lines}\t{len(values)}"
              f"\t{statistics.mean(values):.1f}\t{statistics.stdev(values):.1f}\t{min(values):.1f}")
PY

echo "samples: $out/samples.tsv" >&2
echo "summary: $out/summary.tsv" >&2

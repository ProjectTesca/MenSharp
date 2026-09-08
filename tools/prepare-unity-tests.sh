#!/usr/bin/env bash
# Stages what the Unity integration project needs and cannot keep in Git:
# the package under test with a freshly built compiler, and the VRChat SDK
# packages it depends on.
#
#   tools/prepare-unity-tests.sh
#
# Environment:
#   MENSHARP_CARGO_TARGET      build the compiler for this Rust target instead
#                              of the host — CI uses x86_64-unknown-linux-musl,
#                              a static binary that runs in GameCI's container
#                              whatever glibc it ships
#   MENSHARP_VRC_PROJECT       an ALCOM/VCC Worlds project to borrow the SDK
#                              packages from (symlinked, never copied)
#   MENSHARP_SKIP_VPM_RESOLVE  "1" when the SDK is already in place (CI resolves
#                              it with vrc-get before calling this)
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
project="$repo/tests/unity-project"
packages="$project/Packages"
mensharp="$packages/io.tesca.mensharp"

if [[ -n "${MENSHARP_CARGO_TARGET:-}" ]]; then
    cargo build --release -p men-sharp --manifest-path "$repo/Cargo.toml" \
        --target "$MENSHARP_CARGO_TARGET"
    binary="$repo/target/$MENSHARP_CARGO_TARGET/release/men-sharp"
else
    cargo build --release -p men-sharp --manifest-path "$repo/Cargo.toml"
    binary="$repo/target/release/men-sharp"
fi

rm -rf "$mensharp"
mkdir -p "$mensharp"
cp -r "$repo/unity/io.tesca.mensharp/." "$mensharp/"
mkdir -p "$mensharp/Compiler~"
cp "$binary" "$mensharp/Compiler~/men-sharp-linux-x64"
chmod +x "$mensharp/Compiler~/men-sharp-linux-x64"


if [[ "${MENSHARP_SKIP_VPM_RESOLVE:-}" == "1" ]]; then
    :
elif command -v vrc-get >/dev/null 2>&1; then
    vrc-get resolve --project "$project"
elif [[ -n "${MENSHARP_VRC_PROJECT:-}" ]]; then
    donor="${MENSHARP_VRC_PROJECT}"
    for name in com.vrchat.base com.vrchat.worlds; do
        source_package="$donor/Packages/$name"
        [[ -d "$source_package" ]] || {
            echo "missing $source_package" >&2
            exit 1
        }
        rm -rf "$packages/$name"
        ln -s "$source_package" "$packages/$name"
    done
else
    echo "vrc-get is not installed and MENSHARP_VRC_PROJECT is unset" >&2
    echo "set MENSHARP_VRC_PROJECT to an ALCOM/VCC Worlds project" >&2
    exit 1
fi

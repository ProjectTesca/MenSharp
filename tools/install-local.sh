#!/usr/bin/env bash
# Installs the MenSharp Unity package (with a locally built compiler binary)
# into a Unity project as an embedded package — the dev loop for working on
# MenSharp itself, no CI round-trip needed.
#
#   tools/install-local.sh ~/ALCOM/Projects/MenSharpTest
#
# Re-run after changing the compiler or the editor scripts; Unity picks the
# changes up on focus.
set -euo pipefail

if [[ $# -ne 1 ]]; then
    echo "usage: tools/install-local.sh <unity project directory>" >&2
    exit 1
fi
project="$1"
if [[ ! -d "$project/Packages" ]]; then
    echo "error: $project does not look like a Unity project (no Packages/)" >&2
    exit 1
fi

repo="$(cd "$(dirname "$0")/.." && pwd)"

echo "building the compiler (release)..."
cargo build --release -p men-sharp --manifest-path "$repo/Cargo.toml"

destination="$project/Packages/io.tesca.mensharp"
echo "installing package to $destination"
mkdir -p "$destination"
cp -r "$repo/unity/io.tesca.mensharp/." "$destination/"

mkdir -p "$destination/Compiler~"
case "$(uname -s)" in
    Linux)  binary="men-sharp-linux-x64" ;;
    Darwin)
        if [[ "$(uname -m)" == "arm64" ]]; then
            binary="men-sharp-macos-arm64"
        else
            binary="men-sharp-macos-x64"
        fi
        ;;
    *)      binary="men-sharp-windows-x64.exe" ;;
esac
cp "$repo/target/release/men-sharp" "$destination/Compiler~/$binary"
chmod +x "$destination/Compiler~/$binary" 2>/dev/null || true

echo "done — open the project and use MenSharp > Compile All"

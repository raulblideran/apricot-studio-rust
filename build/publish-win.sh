#!/usr/bin/env bash
# Builds the Windows release: dist/apricot-studio-<version>-windows-x64.zip
#
# Runs on Windows (Git Bash, which is what the Release workflow uses). The zip
# carries the app alone: on first run it offers to download ffmpeg, and the
# VapourSynth set QTGMC needs, into a Codex folder beside the .exe.
#
# The folder inside the zip is always "ApricotStudio", so unzipping a new
# version over an old one keeps the Codex folder already downloaded.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
ZIP="apricot-studio-$VERSION-windows-x64.zip"

cargo build --release

STAGE="$ROOT/dist/ApricotStudio"
rm -rf "$STAGE" && mkdir -p "$STAGE"
cp target/release/apricot-studio.exe "$STAGE/"
cp LICENSE "$STAGE/LICENSE.txt"
cp README.md "$STAGE/README.txt"
cp CHANGELOG.md "$STAGE/CHANGELOG.txt"

cd "$ROOT/dist"
rm -f "$ZIP"
if command -v 7z >/dev/null; then
  7z a -tzip -mx=7 "$ZIP" ApricotStudio >/dev/null
else
  powershell -NoProfile -Command "Compress-Archive -Path ApricotStudio -DestinationPath '$ZIP'"
fi
echo "Built $ROOT/dist/$ZIP (version $VERSION)"
ls -la "$STAGE"

#!/usr/bin/env bash
# Produce cargo-sources.json, so a Flathub build can fetch the crates offline.
#
# The manifest builds with `--share=network` by default, which is the honest
# thing for a local build and is not allowed on Flathub. Running this writes the
# list of every crate the build needs; add it to the apricot-studio module's
# `sources` and delete the build-args line.
set -euo pipefail

cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

GENERATOR="${TMPDIR:-/tmp}/flatpak-cargo-generator.py"
if [[ ! -f "$GENERATOR" ]]; then
    curl -fsSL -o "$GENERATOR" \
        https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/master/cargo/flatpak-cargo-generator.py
fi

python3 -m pip install --user --quiet aiohttp toml 2>/dev/null || true
python3 "$GENERATOR" Cargo.lock -o packaging/flatpak/cargo-sources.json

echo "Wrote packaging/flatpak/cargo-sources.json"
echo "Add this to the apricot-studio module and drop the --share=network line:"
echo "      - cargo-sources.json"

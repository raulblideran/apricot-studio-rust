#!/usr/bin/env bash
# Builds the Flatpak, installs it for the current user, and writes a bundle:
#   dist/apricot-studio-<version>.flatpak
# Needs org.flatpak.Builder plus the 25.08 Sdk and its rust-stable extension
# (see README). The first build compiles x264, x265 and ffmpeg; rebuilds only
# redo the app.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP_ID=io.github.raulblideran.ApricotStudio
cd "$ROOT"

flatpak run org.flatpak.Builder --user --force-clean --install \
    --repo=repo build-dir "packaging/$APP_ID.yaml"

# The version comes from Cargo.toml, not typed here, so the bundle can't be
# named after the previous release.
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)

# --runtime-repo tells the bundle where its runtime comes from; without it the
# bundle only installs on machines that already have the 25.08 runtime.
mkdir -p dist
flatpak build-bundle --runtime-repo=https://flathub.org/repo/flathub.flatpakrepo \
    repo "dist/apricot-studio-$VERSION.flatpak" "$APP_ID" master
echo "Built dist/apricot-studio-$VERSION.flatpak (installed as $APP_ID)"

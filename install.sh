#!/usr/bin/env bash
# Install Apricot Studio for the current user only.
#
# Everything lands under ~/.local, so nothing touches a read-only base image
# and the install survives system updates. Run with --uninstall to remove it.
set -euo pipefail

SRC="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN="$HOME/.local/bin"
APPS="$HOME/.local/share/applications"
ICONS="$HOME/.local/share/icons/hicolor/scalable/apps"

LAUNCHER="$BIN/apricot-studio"
ENTRY="$APPS/io.github.raulblideran.ApricotStudio.desktop"
ICON="$ICONS/io.github.raulblideran.ApricotStudio.svg"

if [[ "${1:-}" == "--uninstall" ]]; then
    rm -f "$LAUNCHER" "$ENTRY" "$ICON"
    update-desktop-database "$APPS" 2>/dev/null || true
    echo "Removed Apricot Studio."
    exit 0
fi

command -v cargo >/dev/null || { echo "error: cargo not found — install Rust from https://rustup.rs" >&2; exit 1; }
for tool in ffmpeg ffprobe; do
    command -v "$tool" >/dev/null || { echo "error: $tool not found" >&2; exit 1; }
done

echo "Building (this takes a few minutes the first time)…"
cargo build --release --manifest-path "$SRC/Cargo.toml"

mkdir -p "$BIN" "$APPS" "$ICONS"
install -Dm755 "$SRC/target/release/apricot-studio" "$LAUNCHER"
install -Dm644 "$SRC/packaging/io.github.raulblideran.ApricotStudio.svg" "$ICON"
# The entry gets the full path rather than the bare name: ~/.local/bin is not on
# every PATH, and a menu entry that silently does nothing is worse than none.
sed -e "s|^Exec=apricot-studio|Exec=$LAUNCHER|" \
    -e "s|^TryExec=apricot-studio|TryExec=$LAUNCHER|" \
    "$SRC/packaging/io.github.raulblideran.ApricotStudio.desktop" > "$ENTRY"
chmod 644 "$ENTRY"

update-desktop-database "$APPS" 2>/dev/null || true
gtk-update-icon-cache -f -t "$HOME/.local/share/icons/hicolor" 2>/dev/null || true

echo "Installed Apricot Studio."
echo "  binary : $LAUNCHER"
echo "  menu   : $ENTRY"
echo
echo "Launch it from the app menu, or right-click a video in your file manager -> Open With."
case ":$PATH:" in
    *":$BIN:"*) echo "You can also just run: apricot-studio /path/to/video.mp4" ;;
    *) echo "Note: $BIN is not on your PATH, so run it as: $LAUNCHER video.mp4" ;;
esac

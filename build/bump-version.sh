#!/usr/bin/env bash
# Sets the app version everywhere it lives:
#   ./build/bump-version.sh 1.3.0 "Short summary of the release"
# Updates Cargo.toml and Cargo.lock (the --version flag, the zip name), adds a
# CHANGELOG.md section and a metainfo release entry (the Flatpak's version).
# Fill in the changelog after.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VERSION="${1:?usage: bump-version.sh <major.minor.patch> [summary]}"
SUMMARY="${2:-}"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "version must look like 1.2.3" >&2; exit 1; }
DATE="$(date +%F)"
META="$ROOT/packaging/io.github.raulblideran.ApricotStudio.metainfo.xml"

CURRENT=$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)
if [[ "$CURRENT" == "$VERSION" ]]; then echo "already at $VERSION"; exit 0; fi

# Only the package's own line: the first `version =` in the file.
sed -i "0,/^version = \"$CURRENT\"/s//version = \"$VERSION\"/" "$ROOT/Cargo.toml"
(cd "$ROOT" && cargo update -p apricot-studio --offline >/dev/null 2>&1) || \
  echo "warning: Cargo.lock not refreshed; the next cargo build will do it" >&2

python3 - "$ROOT/CHANGELOG.md" "$META" "$VERSION" "$DATE" "$SUMMARY" <<'PY'
import sys, html
changelog, meta, version, date, summary = sys.argv[1:]
item = summary or "Describe this release"
s = open(changelog, encoding="utf-8").read()
i = s.index("\n## ")
s = s[:i] + f"\n## {version} — {date}\n\n- {item}\n" + s[i:]
open(changelog, "w", encoding="utf-8").write(s)
m = open(meta, encoding="utf-8").read()
m = m.replace("  <releases>\n", f'  <releases>\n    <release version="{version}" date="{date}">\n'
              f'      <description>\n        <p>{html.escape(item)}</p>\n      </description>\n    </release>\n', 1)
open(meta, "w", encoding="utf-8").write(m)
PY
echo "Version $CURRENT -> $VERSION. Edit CHANGELOG.md to list the changes, then tag v$VERSION"

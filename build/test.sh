#!/usr/bin/env bash
# Runs every test suite. Arguments go through to run-tests.sh (--fast, -v).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
exec "$ROOT/run-tests.sh" "$@"

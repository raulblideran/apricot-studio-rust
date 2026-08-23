#!/usr/bin/env bash
# Apricot Studio's test suite.
#
#   ./run-tests.sh           everything, including real ffmpeg exports
#   ./run-tests.sh --fast    unit, command and theme tests only, no encoding
#   ./run-tests.sh -v        verbose, one line per test
#
# Needs a Rust toolchain and ffmpeg. Nothing else: the window tests run
# headlessly through egui_kittest, so there is no display to arrange and no
# platform window is ever opened.
set -uo pipefail

cd "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

FAST=0
ARGS=()
for arg in "$@"; do
    case "$arg" in
        --fast) FAST=1 ;;
        -h|--help) sed -n '2,10p' "${BASH_SOURCE[0]}" | sed 's/^# \?//'; exit 0 ;;
        -v|--verbose) ARGS+=(--nocapture) ;;
        *) ARGS+=("$arg") ;;
    esac
done

command -v cargo >/dev/null || { echo "error: cargo not found" >&2; exit 1; }
for tool in ffmpeg ffprobe; do
    command -v "$tool" >/dev/null || {
        echo "warning: $tool not found — the tests that need it will skip" >&2; }
done

# The suite writes its settings to a directory of its own, so a test run can
# never overwrite the theme or the recent files of the person running it.
export APRICOT_CONFIG_DIR="${TMPDIR:-/tmp}/apricot-test-config-$$"
trap 'rm -rf "$APRICOT_CONFIG_DIR"' EXIT

# Unit-level suites first: they are instant, and a failure there explains a
# failure in the slow ones.
SUITES=(units commands theme sandbox edges window)
if (( FAST )); then
    echo "running fast tests (no encoding)"
else
    SUITES+=(encode)
fi

start=$(date +%s)
status=0
for suite in "${SUITES[@]}"; do
    # The window suite drives one window at a time: each test redirects the
    # settings directory through the environment, which is per-process.
    threads=()
    [[ "$suite" == "window" ]] && threads=(--test-threads=1)
    if ! cargo test --test "$suite" -- "${threads[@]+"${threads[@]}"}" \
            "${ARGS[@]+"${ARGS[@]}"}"; then
        status=1
    fi
done
elapsed=$(( $(date +%s) - start ))

echo
if (( status == 0 )); then
    echo "all tests passed in ${elapsed}s"
else
    echo "TESTS FAILED (${elapsed}s)" >&2
fi
exit $status

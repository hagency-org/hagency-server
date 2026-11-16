#!/usr/bin/env bash
# Regression test: the scrub helper only resolves demo-* dirs under the root.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=data_dir_guard.sh
. "$here/data_dir_guard.sh"
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/root/demo-ann" "$tmp/root/other" "$tmp/root/demo-ann/demo-x"
export HAGENCY_DEMO_DATA_ROOT="$tmp/root"
fail=0
expect_ok() { got="$(resolve_demo_data_dir "$here" "$1" 2>/dev/null)" && [ "$got" = "$(cd "$1" && pwd -P)" ] || { echo "FAIL accept $1"; fail=1; }; }
expect_no() { if resolve_demo_data_dir "$here" "$1" >/dev/null 2>&1; then echo "FAIL reject $1"; fail=1; fi; }
expect_ok "$tmp/root/demo-ann"
expect_ok "$tmp/root/other/../demo-ann"
expect_no "."
expect_no "/"
expect_no "$HOME"
expect_no "$tmp/root"
expect_no "$tmp/root/other"
expect_no "$tmp/root/demo-ann/demo-x"
expect_no "$tmp/root/missing"
ln -s "$tmp/root/other" "$tmp/root/demo-link"
expect_no "$tmp/root/demo-link"
[ "$fail" = 0 ] && echo "test_data_dir_guard: 10 passed" || exit 1

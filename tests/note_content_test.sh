#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
test_dir="$(mktemp -d "${TMPDIR:-/tmp}/apple-note-content-test.XXXXXX")"
trap 'rm -rf "$test_dir"' EXIT
/usr/bin/clang -fobjc-arc -fblocks -framework Foundation -framework AppKit \
  -framework CoreData -framework CloudKit tests/note_content_test.m -o "$test_dir/test"
"$test_dir/test"

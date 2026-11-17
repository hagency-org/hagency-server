#!/usr/bin/env bash
# Keeps a GitFab JSONL journal (events or Runner receipts) in a Git repository (Annex 3 §7.1: stored
# in Git, history never rewritten).
#
#   scripts/gitfab-journal-git.sh <journal.jsonl> <git repository directory> [name in the repository]
#
# Each run commits the journal when it has grown. The committed copy must be an exact prefix of the
# journal: a journal whose earlier lines changed, or that shrank, is refused and nothing is committed,
# so the repository history shows every line as first written. Run it from cron or a systemd timer
# after the server and the Runner append. The repository is created when missing; pushing it to a
# remote is left to the operator (`git -C <repository> push`).
#
# Exit 0 committed or nothing new, 1 the journal was rewritten, 2 usage or setup error.
set -euo pipefail

journal=${1:-}
repo=${2:-}
name=${3:-$(basename "${journal:-journal.jsonl}")}
if [ -z "$journal" ] || [ -z "$repo" ] || [ ! -f "$journal" ]; then
    echo "usage: $0 <journal.jsonl> <git repository directory> [name in the repository]" >&2
    exit 2
fi
case "$name" in */* | .* | '') echo "name must be a plain file name" >&2; exit 2 ;; esac

if [ ! -d "$repo/.git" ]; then
    mkdir -p "$repo"
    git -C "$repo" init -q
fi
git_() { git -C "$repo" -c user.name="${GITFAB_GIT_NAME:-gitfab}" -c user.email="${GITFAB_GIT_EMAIL:-gitfab@localhost}" "$@"; }

size() { wc -c <"$1" | tr -d ' '; }
digest() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi; }  # "-" reads stdin

copy="$repo/$name"
# A snapshot, so a write landing while this runs is taken next time, not half now.
snapshot=$(mktemp)
trap 'rm -f "$snapshot"' EXIT
cp "$journal" "$snapshot"

if [ -f "$copy" ]; then
    # The committed copy must be untouched in the working tree, and an exact prefix of the journal.
    if ! git -C "$repo" diff --quiet -- "$name" 2>/dev/null; then
        echo "$copy differs from its last commit; refusing to continue" >&2
        exit 1
    fi
    old=$(size "$copy")
    new=$(size "$snapshot")
    # head -c and a digest rather than cmp -n, which BSD cmp gets wrong at the shorter file's end.
    if [ "$new" -lt "$old" ] || [ "$(head -c "$old" "$snapshot" | digest -)" != "$(digest "$copy")" ]; then
        echo "$journal no longer starts with the committed $name ($old bytes): earlier lines changed; nothing committed" >&2
        exit 1
    fi
    if [ "$new" -eq "$old" ]; then
        echo "nothing new"
        exit 0
    fi
fi
# Commit whole lines only; a line still being written waits for the next run.
lines=$(grep -c '' "$snapshot" || true)
if [ -n "$(tail -c 1 "$snapshot")" ]; then
    lines=$((lines - 1))
    head -n "$lines" "$snapshot" >"$snapshot.lines" && mv "$snapshot.lines" "$snapshot"
    if [ -f "$copy" ] && [ "$(size "$snapshot")" -le "$(size "$copy")" ]; then
        echo "nothing new"
        exit 0
    fi
fi
cp "$snapshot" "$copy"
git_ add -- "$name"
git_ commit -q -m "$name: $lines lines, sha256 $(digest "$copy")"
echo "committed $name: $lines lines"

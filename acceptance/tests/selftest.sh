#!/usr/bin/env bash
# Checks the acceptance commands themselves with throwaway keys and events: clean signed events pass,
# a changed event fails, incomplete configuration is refused, and every bundle gets a manifest without local
# paths. Touches nothing outside a temporary directory.
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
export GITFAB_EVIDENCE_DIR="$tmp/evidence" GITFAB_KEYS_DIR="$tmp/keys" GITFAB_CONFIG=/dev/null
root=$(cd "$here/.." && pwd)
gitfab=${GITFAB_BIN:-}
for candidate in "$root/target/release/gitfab" "$root/target/debug/gitfab"; do
    [ -n "$gitfab" ] || { [ -x "$candidate" ] && gitfab=$candidate; } || true
done
[ -n "$gitfab" ] || gitfab=$(command -v gitfab) || { echo "build gitfab first" >&2; exit 2; }
export GITFAB_BIN="$gitfab"
failures=0
expect() {
    local want=$1 name=$2 got
    shift 2
    set +e
    "$@" >/dev/null 2>"$tmp/stderr"
    got=$?
    set -e
    if [ "$got" = "$want" ]; then
        echo "ok   $name (exit $got)"
    else
        echo "FAIL $name: exit $got, expected $want"
        sed 's/^/     /' "$tmp/stderr"
        failures=$((failures + 1))
    fi
}

"$gitfab" keygen --key-id gitfab-selftest --dir "$tmp/keys" >/dev/null
memory() {
    printf '{"schema_version":"gitfab.memory.v1","event_id":"%s","timestamp":"2026-11-12T03:00:00Z","project":"DEMO-05","module":"ledger","spec":"spec/DEMO-05.md@v1","contributor":"@impl:example.org","action":"execute","input_digest":"sha256:%064d","output_digest":"sha256:%064d","token_delta":%s,"task_id":"task-1","signature":null}' "$1" 1 2 "$2"
}
for spec in "m1 5" "m2 3"; do
    set -- $spec
    memory "$1" "$2" >"$tmp/$1.json"
    "$gitfab" sign --key "$tmp/keys/gitfab-selftest.key" --key-id gitfab-selftest \
        --public-key-ref "keys/gitfab-selftest.pub" "$tmp/$1.json" >>"$tmp/events.jsonl"
done
# The private key is not needed again and must never be near a bundle.
rm "$tmp/keys/gitfab-selftest.key"

ACCEPTANCE_RUN_ID=ar-selftest-events expect 0 "verify-events passes signed events" \
    "$here/verify-events.sh" "$tmp/events.jsonl"
ACCEPTANCE_RUN_ID=ar-selftest-ledger expect 0 "verify-ledger recomputes DEMO-05" \
    "$here/verify-ledger.sh" DEMO-05 "$tmp/events.jsonl"
total=$(tr -d ' \n' <"$tmp/evidence/ar-selftest-ledger/ledger-report.json")
case "$total" in
    *'"ok":true'*8*) echo "ok   ledger total includes 8" ;;
    *) echo "FAIL ledger report: $total"; failures=$((failures + 1)) ;;
esac
ACCEPTANCE_RUN_ID=ar-selftest-other expect 1 "verify-ledger fails a project with no events" \
    "$here/verify-ledger.sh" DEMO-04 "$tmp/events.jsonl"

sed 's/"token_delta":5/"token_delta":50/' "$tmp/events.jsonl" >"$tmp/changed.jsonl"
ACCEPTANCE_RUN_ID=ar-selftest-changed expect 1 "verify-events fails a changed event" \
    "$here/verify-events.sh" "$tmp/changed.jsonl"
ACCEPTANCE_RUN_ID=ar-selftest-changed-ledger expect 1 "verify-ledger fails a changed event" \
    "$here/verify-ledger.sh" DEMO-05 "$tmp/changed.jsonl"
mkdir -p "$tmp/projects"
printf 'PROJECT_ID=DEMO-05\nBACKEND=github\nREPO=example/demo-05\n' >"$tmp/projects/DEMO-05.env"
GITFAB_PROJECTS_DIR="$tmp/projects" ACCEPTANCE_RUN_ID=ar-selftest-release expect 1 \
    "verify-release fails without a signed Release event" "$here/verify-release.sh" DEMO-05 "$tmp/events.jsonl"
GITFAB_PROJECTS_DIR="$tmp/projects" ACCEPTANCE_RUN_ID=ar-selftest-release-unknown expect 2 \
    "verify-release refuses an unknown project" "$here/verify-release.sh" DEMO-09 "$tmp/events.jsonl"
printf 'PROJECT_ID=DEMO-02\nBACKEND=github\nREPO=example/demo-02\n' >"$tmp/projects/DEMO-02.env"
GITFAB_PROJECTS_DIR="$tmp/projects" ACCEPTANCE_RUN_ID=ar-selftest-run-backend expect 2 \
    "run-project refuses a backend the project is not recorded for" "$here/run-project.sh" DEMO-02 --backend gitcode
GITFAB_PROJECTS_DIR="$tmp/projects" ACCEPTANCE_RUN_ID=ar-selftest-run-config expect 2 \
    "run-project refuses an incomplete project" "$here/run-project.sh" DEMO-02 --backend github
GITFAB_PROJECTS_DIR="$tmp/projects" ACCEPTANCE_RUN_ID=ar-selftest-rollback expect 2 \
    "rollback refuses to run without the Runner configuration" "$here/rollback.sh" DEMO-02
ACCEPTANCE_RUN_ID=bad-id expect 2 "a malformed run id is refused" "$here/verify-events.sh" "$tmp/events.jsonl"

for bundle in "$tmp"/evidence/ar-*; do
    if [ ! -f "$bundle/manifest.json" ]; then
        echo "FAIL $(basename "$bundle") has no manifest"; failures=$((failures + 1))
    elif grep -rqF -e "$tmp" -e "$HOME" "$bundle"; then
        echo "FAIL $(basename "$bundle") contains a local path"; failures=$((failures + 1))
    fi
done
echo "manifests checked: $(ls -d "$tmp"/evidence/ar-* | wc -l | tr -d ' ') bundles"

[ "$failures" -eq 0 ] && echo "selftest passed" || { echo "selftest: $failures failure(s)"; exit 1; }

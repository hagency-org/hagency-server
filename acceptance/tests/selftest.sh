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

expect 2 "verify-blackbox needs the Runner receipts" "$here/verify-blackbox.sh" DEMO-01
expect 2 "verify-blackbox needs a known project" "$here/verify-blackbox.sh" DEMO-09
for p in DEMO-01 DEMO-02 DEMO-03 DEMO-04 DEMO-05; do
    expect 0 "$p has valid black-box cases" python3 -c 'import json,sys; c=json.load(open(sys.argv[1])); assert any(k in c for k in ("startup","http","cli","container"))' "$here/fixtures/$p/cases.json"
done

expect 0 "black-box placeholders expand, including repeat" python3 -c '
import sys; sys.path.insert(0, sys.argv[1]); import blackbox as b
v = b.substitute({"t": "${repeat:字:200}", "u": "${repeat:a:3}-${sha}"}, {"sha": "c0"})
assert len(v["t"]) == 200 and set(v["t"]) == {"字"} and v["u"] == "aaa-c0", v' "$here/lib"

# Retention: an intact bundle archives and checks; a changed one is refused; a changed archive fails.
bundle=$(basename "$(ls -d "$tmp"/evidence/ar-* | head -1)")
expect 0 "archive an intact bundle" "$here/archive.sh" "$bundle" --retain-until 2028-12-31
expect 0 "an archive checks intact" "$here/archive.sh" --check "$tmp/evidence/archive/$bundle.tar.gz"
expect 1 "an archive is never overwritten" "$here/archive.sh" "$bundle" --retain-until 2028-12-31
printf 'x' >>"$tmp/evidence/archive/$bundle.tar.gz"
expect 1 "a changed archive fails its check" "$here/archive.sh" --check "$tmp/evidence/archive/$bundle.tar.gz"
other=$(basename "$(ls -d "$tmp"/evidence/ar-* | sed -n 2p)")
printf 'x' >>"$tmp/evidence/$other/$(ls "$tmp/evidence/$other" | grep -v manifest.json | head -1)"
expect 1 "a bundle changed after its manifest is not archived" "$here/archive.sh" "$other" --retain-until 2028-12-31

# The confirmed Spec file: verify-release compares its Git blob at the run's first Execute, at the
# Release and on the platform, through a stand-in for gitfab-forge (reads only, nothing leaves here).
spec_case() { # name, blob letter at the first Execute, at the Release, on the platform
    local name=$1 dir=$tmp/spec-$1
    mkdir -p "$dir/projects" "$dir/bin"
    printf 'PROJECT_ID=DEMO-02\nBACKEND=github\nREPO=o/demo-02\nBRANCH=main\nBASELINE_SHA=%040d\n' 0 >"$dir/projects/DEMO-02.env"
    : >"$dir/token" && chmod 600 "$dir/token"
    printf '#!/usr/bin/env bash\nfor a; do case "$a" in release-info) echo "{\\"tag\\":\\"t\\"}"; exit 0;; checks) echo "{\\"headSha\\":\\"%040d\\"}"; exit 0;; file) echo "{\\"blob\\":\\"%s\\"}"; exit 0;; esac; done; exit 2\n' 0 "$(printf '%040d' 0 | tr 0 "$4")" >"$dir/bin/forge"
    chmod 755 "$dir/bin/forge"
    "$gitfab" keygen --key-id spec-selftest --dir "$dir/keys" >/dev/null
    : >"$dir/events.jsonl"
    for pb in "execute $2" "release $3"; do
        python3 -c '
import json,sys
phase,blob=sys.argv[1],sys.argv[2]
refs=["https://github.com/o/demo-02/releases/tag/t","https://github.com/o/demo-02/pull/7"] if phase=="release" else []
print(json.dumps({"schema_version":"gitfab.task.v1","event_id":"run-1."+phase,"timestamp":"2026-11-14T03:00:00Z","project_id":"DEMO-02","task_id":"task-1","run_id":"run-1","matrix_room_id":"!r:x","matrix_thread_event_id":"$root","git_backend":"github","repo_ref":"https://github.com/o/demo-02","baseline_sha":"0"*40,"spec_ref":"spec/DEMO-02.md@v1","phase":phase,"status":"succeeded","actor_id":"@a:x","input_digest":"sha256:"+"a"*64,"output_digest":None,"artifact_refs":refs,"request_id":"r","signature":None,"round":1,"spec_revision":1,"spec_blob":blob*40,"commit_sha":"0"*40}))' ${pb} >"$dir/e.json"
        "$gitfab" sign --key "$dir/keys/spec-selftest.key" --key-id spec-selftest "$dir/e.json" | tr -d '\n' >>"$dir/events.jsonl"
        echo >>"$dir/events.jsonl"
    done
    GITFAB_PROJECTS_DIR=$dir/projects GITFAB_KEYS_DIR=$dir/keys GITFAB_GITHUB_TOKEN_FILE=$dir/token GITFAB_FORGE_BIN=$dir/bin/forge \
        ACCEPTANCE_RUN_ID=ar-spec-$name "$here/verify-release.sh" DEMO-02 "$dir/events.jsonl" >/dev/null 2>&1 || true
    python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); print(next(c["status"] for c in r["checks"] if c["name"]=="spec.unchanged"))' \
        "$GITFAB_EVIDENCE_DIR/ar-spec-$name/verify-release-report.json"
}
for c in "same b b b ok" "edited-before-release b e e fail" "edited-on-platform b b f fail"; do
    set -- $c
    got=$(spec_case "$1" "$2" "$3" "$4")
    if [ "$got" = "$5" ]; then echo "ok   Spec file $1: spec.unchanged $got"; else echo "FAIL Spec file $1: spec.unchanged $got, expected $5"; failures=$((failures + 1)); fi
done

[ "$failures" -eq 0 ] && echo "selftest passed" || { echo "selftest: $failures failure(s)"; exit 1; }

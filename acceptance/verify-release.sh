#!/usr/bin/env bash
# Verifies a project's Release (Annex 1 Git backend, Annex 3 §9, §10): the journal's events verify, the
# last signed Release event of the project names a Release and a Pull Request, the Release exists on the
# platform and the Pull Request's checks passed. Platform reads only; nothing is created or changed.
#
# Deployment receipts (container restart, health check, version endpoint, receipt signature) are not
# verified by this package yet: when everything else passes it exits 3, so a run can never take this
# for a full pass.
#
#   ./acceptance/verify-release.sh DEMO-02            the journal named by GITFAB_JOURNAL
#   ./acceptance/verify-release.sh DEMO-02 FILE.jsonl a given journal
. "$(dirname "$0")/lib/common.sh"

PROJECT_ID=${1:-}
if [ -z "$PROJECT_ID" ]; then
    echo "usage: verify-release.sh DEMO-0N [FILE.jsonl] (or set GITFAB_JOURNAL)" >&2
    exit 2
fi
journal=${2:-${GITFAB_JOURNAL:-}}

if ! load_project "$PROJECT_ID"; then
    check config fail "acceptance/projects/$PROJECT_ID.env missing or unreadable"
    finish 2
fi
REPO_REF=$REPO BASELINE_SHA=$BASELINE_SHA
case "$BACKEND" in
    github | gitcode) ;;
    *) check config fail "BACKEND must be github or gitcode"; finish 2 ;;
esac
gitfab=$(gitfab_bin) || { check tool.gitfab fail "gitfab not found"; finish 2; }
forge=$(gitfab_forge_bin) || { check tool.gitfab_forge fail "gitfab-forge not found: cargo build -p hagency-gitfab-forge"; finish 2; }
if [ -z "$journal" ] || [ ! -f "$journal" ]; then
    check events.input fail "no journal: pass FILE.jsonl or set GITFAB_JOURNAL"
    finish 2
fi
keys=${GITFAB_KEYS_DIR:-$ACCEPTANCE_DIR/keys}
if ! ls "$keys"/*.pub >/dev/null 2>&1; then
    check keys fail "no pinned public keys in $(scrub "$keys")"
    finish 2
fi

# 1. Every event verifies, so the Release event below is trustworthy in context.
if "$gitfab" verify --keys "$keys" "$journal" >"$RUN_DIR/events-verify.json"; then
    check events.verify ok "journal verifies"
else
    check events.verify fail "see events-verify.json"
fi

# 2. The project's last signed Release event and its links.
if ! "$gitfab" find --keys "$keys" --project "$PROJECT_ID" --phase release --status succeeded "$journal" \
    >"$RUN_DIR/release-event.json"; then
    check release.event fail "no signed succeeded Release event for $PROJECT_ID"
    finish
fi
event=$(tr -d '\n' <"$RUN_DIR/release-event.json")
release_url=$(printf '%s' "$event" | json_field artifact_ref_0)
pull_url=$(printf '%s' "$event" | json_field artifact_ref_1)
run_id=$(printf '%s' "$event" | json_field run_id)
case "$release_url" in
    https://*/releases/*) ;;
    *) check release.event fail "run $run_id: the Release event carries no Release link"; finish ;;
esac
case "$pull_url" in
    https://*/pull/[0-9]* | https://*/pulls/[0-9]* | https://*/merge_requests/[0-9]*) ;;
    *) check release.event fail "run $run_id: the Release event carries no Pull Request link"; finish ;;
esac
check release.event ok "run $run_id: $release_url"
tag=${release_url##*/}
number=${pull_url##*/}

# 3. The platform agrees (read only). The token stays in its file.
token_file=$(backend_token_file "$BACKEND")
if [ -z "$token_file" ] || [ ! -f "$token_file" ]; then
    check release.platform fail "no $BACKEND token file configured"
    finish
fi
platform() {
    GITFAB_TOKEN_FILE=$token_file "$forge" --backend "$BACKEND" --api "$(backend_api "$BACKEND")" "$@"
}
if platform release-info "$REPO" --tag "$tag" >"$RUN_DIR/release-platform.json" 2>"$RUN_DIR/.forge-err"; then
    check release.platform ok "$BACKEND has Release $tag"
else
    check release.platform fail "$BACKEND: $(tr -d '\n' <"$RUN_DIR/release-platform.json" "$RUN_DIR/.forge-err")"
fi
if platform checks "$REPO" "$number" >"$RUN_DIR/release-checks.json" 2>"$RUN_DIR/.forge-err"; then
    check release.checks ok "Pull Request $number checks passed"
else
    check release.checks fail "Pull Request $number: $(tr -d '\n' <"$RUN_DIR/release-checks.json" "$RUN_DIR/.forge-err")"
fi
rm -f "$RUN_DIR/.forge-err"
FINAL_SHA=$(json_field headSha <"$RUN_DIR/release-checks.json")

# 4. Not yet verifiable here; never reported as a pass.
if [ -z "$FAILED_STAGE" ]; then
    check release.deployment fail "deployment receipts (Annex 3 §9) are not verified by this package yet"
    finish 3
fi
finish

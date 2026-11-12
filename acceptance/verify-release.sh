#!/usr/bin/env bash
# Verifies a project's Release (Annex 1 Git backend, Annex 3 §9, §10): the journal's events verify, the
# last signed Release event of the project names a Release and a Pull Request, the Release exists on the
# platform and the Pull Request's checks passed. Platform reads only; nothing is created or changed.
#
# Then the Runner's deployment receipt (Annex 3 §9): it verifies, its health check passed, it names the
# Pull Request's head commit, its SBOM file matches, the project's container runs its image, and the
# service answers health and version for this Release now. Needs GITFAB_RECEIPTS, GITFAB_SBOM_DIR, the
# Runner's public key among the pinned keys, Docker, and DEPLOY_PORT in the project file.
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

# 4. The Runner's signed deployment receipt for this Release, and the deployment as it runs now.
receipts=${GITFAB_RECEIPTS:-}
if [ -z "$receipts" ] || [ ! -f "$receipts" ]; then
    check deploy.receipts fail "no receipt journal: set GITFAB_RECEIPTS"
    finish
fi
if "$gitfab" verify --keys "$keys" "$receipts" >"$RUN_DIR/receipts-verify.json"; then
    check deploy.receipts ok "receipt journal verifies"
else
    check deploy.receipts fail "see receipts-verify.json"
fi
if ! "$gitfab" receipt --keys "$keys" --project "$PROJECT_ID" --tag "$tag" "$receipts" >"$RUN_DIR/deploy-receipt.json"; then
    check deploy.receipt fail "no signed receipt for $PROJECT_ID $tag"
    finish
fi
receipt=$(tr -d '\n' <"$RUN_DIR/deploy-receipt.json")
field() { printf '%s' "$receipt" | json_field "$1"; }
image=$(field image_digest)
if [ "$(field healthcheck_result)" = passed ]; then
    check deploy.receipt ok "$(field release_id) by $(field runner_id), image $image"
else
    check deploy.receipt fail "$(field release_id): health check $(field healthcheck_result)"
fi
if [ -n "$FINAL_SHA" ] && [ "$(field commit_sha)" != "$FINAL_SHA" ]; then
    check deploy.commit fail "receipt commit $(field commit_sha), Pull Request head $FINAL_SHA"
else
    check deploy.commit ok "$(field commit_sha)"
fi
sbom=${GITFAB_SBOM_DIR:-}/$(basename "$(field sbom_ref)")
if [ -z "${GITFAB_SBOM_DIR:-}" ] || [ ! -f "$sbom" ]; then
    check deploy.sbom fail "$(field sbom_ref) not found in GITFAB_SBOM_DIR"
elif [ "sha256:$(sha256_file "$sbom")" != "$(field sbom_digest)" ]; then
    check deploy.sbom fail "$(field sbom_ref) does not match the receipt"
elif "$gitfab" verify-sbom --keys "$keys" "$sbom" >"$RUN_DIR/sbom-verify.json"; then
    check deploy.sbom ok "$(field sbom_ref) matches the receipt; its .sha256 and signed attestation verify"
    bases=$(tr -d '\n ' <"$RUN_DIR/sbom-verify.json" | sed -n 's/.*"base_images":\[\(.*\)\],"failures".*/\1/p')
    case "$bases" in
        '' | *'"digest":null'*) check deploy.base_images fail "base image digests not all recorded: ${bases:-none}" ;;
        *)
            # One acceptance run must not see a base image change (Annex 3 §9): every project verified
            # under this acceptance_run_id adds its bases here, and a different digest for a name fails.
            seen="$RUN_DIR/base-images.txt"
            drift=""
            for pair in $(printf '%s' "$bases" | sed 's/},{/}\n{/g' | sed -n 's/.*"digest":"\([^"]*\)","name":"\([^"]*\)".*/\2=\1/p'); do
                name_=${pair%%=*} digest_=${pair#*=}
                earlier=$(awk -F= -v n="$name_" -v d="$digest_" '$1 == n && $2 != d { print; exit }' "$seen" 2>/dev/null || true)
                if [ -n "$earlier" ]; then drift="$drift $name_ (${earlier#*=} then $digest_)"; fi
                grep -qxF "$pair" "$seen" 2>/dev/null || printf '%s\n' "$pair" >>"$seen"
            done
            if [ -n "$drift" ]; then
                check deploy.base_images fail "base image changed within $ACCEPTANCE_RUN_ID:$drift"
            else
                check deploy.base_images ok "$bases"
            fi ;;
    esac
else
    check deploy.sbom fail "see sbom-verify.json"
fi
name="gitfab-$(printf '%s' "$PROJECT_ID" | tr 'A-Z' 'a-z')"
running=$(docker inspect -f '{{.State.Running}} {{.Image}}' "$name" 2>/dev/null || true)
if [ "$running" = "true $image" ]; then
    check deploy.container ok "$name runs $image"
else
    check deploy.container fail "$name: ${running:-not found}, expected running $image"
fi
if [ -z "$DEPLOY_PORT" ]; then
    check deploy.live fail "DEPLOY_PORT not set for $PROJECT_ID"
else
    http_get "http://127.0.0.1:$DEPLOY_PORT$HEALTH_PATH"
    health=$HTTP_STATUS
    http_get "http://127.0.0.1:$DEPLOY_PORT$VERSION_PATH"
    case "$health:$HTTP_STATUS:$HTTP_BODY" in
        200:200:*"$tag"*) check deploy.live ok "health 200, version $tag" ;;
        *) check deploy.live fail "health $health, version HTTP $HTTP_STATUS: $(printf '%s' "$HTTP_BODY" | head -c 120)" ;;
    esac
fi
finish

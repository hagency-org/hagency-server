#!/usr/bin/env bash
# Rolls a project back to its previous good Release and verifies it (Annex 3 §9, §10): the container
# restarts on the target image, the health check passes, the version endpoint names the target Release
# and the Runner's new receipt verifies.
#
#   ./acceptance/rollback.sh DEMO-01
#
# Configuration: GITFAB_RECEIPTS (the Runner's receipt journal), GITFAB_RUNNER_KEY_FILE (mode 0600),
# GITFAB_RUNNER_KEY_ID, GITFAB_RUNNER_ID, the Runner's public key among the pinned keys, Docker, and
# DEPLOY_PORT (CONTAINER_PORT, HEALTH_PATH, VERSION_PATH optional) in the project file.
. "$(dirname "$0")/lib/common.sh"

PROJECT_ID=${1:-}
if [ -z "$PROJECT_ID" ]; then
    echo "usage: rollback.sh DEMO-0N" >&2
    exit 2
fi
if ! load_project "$PROJECT_ID"; then
    check config fail "acceptance/projects/$PROJECT_ID.env missing or unreadable"
    finish 2
fi
REPO_REF=$REPO BASELINE_SHA=$BASELINE_SHA
missing=""
for key in GITFAB_RECEIPTS GITFAB_RUNNER_KEY_FILE GITFAB_RUNNER_KEY_ID GITFAB_RUNNER_ID; do
    [ -n "${!key:-}" ] || missing="$missing $key"
done
[ -n "$DEPLOY_PORT" ] || missing="$missing DEPLOY_PORT"
if [ -n "$missing" ]; then
    check config fail "not set:$missing"
    finish 2
fi
gitfab=$(gitfab_bin) || { check tool.gitfab fail "gitfab not found"; finish 2; }
runner=$(gitfab_runner_bin) || { check tool.gitfab_runner fail "gitfab-runner not found: cargo build -p hagency-gitfab-runner"; finish 2; }
keys=${GITFAB_KEYS_DIR:-$ACCEPTANCE_DIR/keys}
if ! ls "$keys"/*.pub >/dev/null 2>&1; then
    check keys fail "no pinned public keys in $(scrub "$keys")"
    finish 2
fi

receipt() { "$gitfab" receipt --keys "$keys" --project "$PROJECT_ID" "$GITFAB_RECEIPTS" 2>/dev/null | tr -d '\n'; }
before=$(receipt)
from_tag=$(printf '%s' "$before" | json_field tag)
from_id=$(printf '%s' "$before" | json_field release_id)
if [ -z "$from_tag" ]; then
    check rollback.before fail "no signed receipt for $PROJECT_ID"
    finish
fi
check rollback.before ok "$PROJECT_ID runs $from_id"

set +e
"$runner" rollback --project "$PROJECT_ID" --key "$GITFAB_RUNNER_KEY_FILE" --key-id "$GITFAB_RUNNER_KEY_ID" \
    --runner-id "$GITFAB_RUNNER_ID" --receipts "$GITFAB_RECEIPTS" --port "$DEPLOY_PORT" \
    --container-port "$CONTAINER_PORT" --health-path "$HEALTH_PATH" --version-path "$VERSION_PATH" \
    >"$RUN_DIR/rollback-run.json" 2>"$RUN_DIR/rollback-run.err"
code=$?
set -e
if [ "$code" -eq 0 ]; then
    check rollback.run ok "$(tr -d '\n' <"$RUN_DIR/rollback-run.json")"
else
    check rollback.run fail "gitfab-runner exit $code: $(tr -d '\n' <"$RUN_DIR/rollback-run.err" "$RUN_DIR/rollback-run.json")"
fi
rm -f "$RUN_DIR/rollback-run.err"

if "$gitfab" verify --keys "$keys" "$GITFAB_RECEIPTS" >"$RUN_DIR/receipts-verify.json"; then
    check rollback.receipts ok "receipt journal verifies"
else
    check rollback.receipts fail "see receipts-verify.json"
fi
after=$(receipt)
printf '%s\n' "$after" >"$RUN_DIR/rollback-receipt.json"
tag=$(printf '%s' "$after" | json_field tag)
image=$(printf '%s' "$after" | json_field image_digest)
if [ "$(printf '%s' "$after" | json_field rollback_result)" = passed ] && [ "$tag" != "$from_tag" ]; then
    check rollback.receipt ok "$(printf '%s' "$after" | json_field release_id): $from_tag -> $tag"
else
    check rollback.receipt fail "the last receipt is not a passed rollback away from $from_tag"
fi
name="gitfab-$(printf '%s' "$PROJECT_ID" | tr 'A-Z' 'a-z')"
running=$(docker inspect -f '{{.State.Running}} {{.Image}}' "$name" 2>/dev/null || true)
[ "$running" = "true $image" ] && check rollback.container ok "$name runs $image" \
    || check rollback.container fail "$name: ${running:-not found}, expected running $image"
http_get "http://127.0.0.1:$DEPLOY_PORT$HEALTH_PATH"
health=$HTTP_STATUS
http_get "http://127.0.0.1:$DEPLOY_PORT$VERSION_PATH"
case "$health:$HTTP_STATUS:$HTTP_BODY" in
    200:200:*"$tag"*) check rollback.live ok "health 200, version $tag" ;;
    *) check rollback.live fail "health $health, version HTTP $HTTP_STATUS" ;;
esac
finish

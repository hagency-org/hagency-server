#!/usr/bin/env bash
# Verifies every GitFab event (Annex 3 §7, §8): schema, Ed25519 signature on each signed event, and an
# identity event for each key event. Keys come from acceptance/keys (pinned at T0); without pinned keys
# the server's published keys are used and the run says so.
#
#   ./acceptance/verify-events.sh                 the journal named by GITFAB_JOURNAL
#   ./acceptance/verify-events.sh FILE.jsonl...   given event files
. "$(dirname "$0")/lib/common.sh"

gitfab=$(gitfab_bin) || { check tool.gitfab fail "gitfab not found"; finish; }
if [ "$#" -gt 0 ]; then
    files=("$@")
elif [ -n "${GITFAB_JOURNAL:-}" ]; then
    files=("$GITFAB_JOURNAL")
else
    echo "usage: verify-events.sh [FILE.jsonl...] (or set GITFAB_JOURNAL)" >&2
    exit 2
fi
for f in "${files[@]}"; do
    [ -f "$f" ] || { check events.input fail "$(basename "$f") not found"; finish; }
done

keys=${GITFAB_KEYS_DIR:-$ACCEPTANCE_DIR/keys}
if ls "$keys"/*.pub >/dev/null 2>&1; then
    check keys ok "pinned keys: $(cd "$keys" && ls ./*.pub | sed 's|^\./||' | tr '\n' ' ')"
elif [ -n "${GITFAB_SERVER_URL:-}" ]; then
    http_get "${GITFAB_SERVER_URL%/}/hagency/gitfab/v1/public-keys"
    key_id=$(printf '%s' "$HTTP_BODY" | json_field keyId)
    public=$(printf '%s' "$HTTP_BODY" | json_field publicKeyBase64)
    if [ "$HTTP_STATUS" != 200 ] || [ -z "$key_id" ] || [ -z "$public" ]; then
        check keys fail "no GitFab key published (HTTP $HTTP_STATUS)"
        finish
    fi
    keys="$RUN_DIR/keys"
    mkdir -p "$keys"
    printf '%s\n' "$public" >"$keys/$key_id.pub"
    check keys warn "no pinned keys; using the keys the server publishes"
else
    check keys fail "no keys in acceptance/keys and GITFAB_SERVER_URL not set"
    finish
fi

if "$gitfab" verify --keys "$keys" "${files[@]}" >"$RUN_DIR/events-verify.json"; then
    check events.verify ok "$(tr -d '\n ' <"$RUN_DIR/events-verify.json" | sed -n 's/.*"events":\([0-9]*\).*"signed":\([0-9]*\).*/\1 events, \2 signed/p')"
else
    check events.verify fail "see events-verify.json"
fi
finish

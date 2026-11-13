#!/usr/bin/env bash
# verify-blackbox.sh DEMO-0N [--tag <Release tag>]
#
# Runs the project's frozen black-box cases (fixtures/<project>/cases.json) against the image the
# Runner deployed, independently of what the Agents reported (Annex 2 §3.1: the frozen test command
# is run). The image, Release tag and commit come from the signed deployment receipt; a fresh
# container of that image is started on a temporary port, so the live deployment is not touched.
# Startup cases run the image with their own environment; container cases inspect the image.
#
# Exit 0 every case passed; 1 a case failed; 2 configuration missing (no cases file, no receipt,
# no Docker).
. "$(dirname "$0")/lib/common.sh"

PROJECT_ID=${1:-}
[ -n "$PROJECT_ID" ] || { echo "usage: verify-blackbox.sh DEMO-0N [--tag TAG]" >&2; exit 2; }
tag_arg=()
if [ "${2:-}" = --tag ] && [ -n "${3:-}" ]; then tag_arg=(--tag "$3"); fi

if ! load_project "$PROJECT_ID"; then
    check config fail "acceptance/projects/$PROJECT_ID.env missing or unreadable"
    finish 2
fi
REPO_REF=$REPO BASELINE_SHA=$BASELINE_SHA
cases="$ACCEPTANCE_DIR/fixtures/$PROJECT_ID/cases.json"
if [ ! -f "$cases" ]; then
    check blackbox.cases fail "no fixtures/$PROJECT_ID/cases.json"
    finish 2
fi
check blackbox.cases ok "fixtures/$PROJECT_ID/cases.json sha256:$(sha256_file "$cases")"
gitfab=$(gitfab_bin) || { check tool.gitfab fail "gitfab not found"; finish 2; }
command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1 || { check tool.docker fail "Docker is not available"; finish 2; }
keys=${GITFAB_KEYS_DIR:-$ACCEPTANCE_DIR/keys}
receipts=${GITFAB_RECEIPTS:-}
if [ -z "$receipts" ] || [ ! -f "$receipts" ]; then
    check deploy.receipts fail "no receipt journal: set GITFAB_RECEIPTS"
    finish 2
fi
if ! "$gitfab" receipt --keys "$keys" --project "$PROJECT_ID" ${tag_arg[@]+"${tag_arg[@]}"} "$receipts" >"$RUN_DIR/blackbox-receipt.json"; then
    check deploy.receipt fail "no signed receipt for $PROJECT_ID"
    finish 2
fi
receipt=$(tr -d '\n' <"$RUN_DIR/blackbox-receipt.json")
field() { printf '%s' "$receipt" | json_field "$1"; }
image=$(field image_digest) tag=$(field tag) FINAL_SHA=$(field commit_sha)
check deploy.receipt ok "$(field release_id): $tag at $FINAL_SHA, image $image"

code=0
python3 "$ACCEPTANCE_DIR/lib/blackbox.py" "$cases" "$image" --release "$tag" --sha "$FINAL_SHA" \
    --port "${CONTAINER_PORT:-8080}" >"$RUN_DIR/blackbox-result.json" 2>"$RUN_DIR/.blackbox-err" || code=$?
rm -f "$RUN_DIR/.blackbox-err"
if [ "$code" = 2 ]; then
    check blackbox.run fail "$(tr -d '\n' <"$RUN_DIR/blackbox-result.json" | head -c 200)"
    finish 2
fi
# One check per section of cases.json, with the failed cases named.
for section in startup http container; do
    summary=$(python3 - "$RUN_DIR/blackbox-result.json" "$section" <<'EOF'
import json, sys
s = json.load(open(sys.argv[1]))["sections"].get(sys.argv[2])
if s is None:
    print("absent")
elif s["failed"]:
    print("fail " + "; ".join(f"{f['case']}: {f['why']}" for f in s["failed"])[:400])
else:
    print(f"ok {s['passed']} cases passed")
EOF
)
    case "$summary" in
        absent) ;;
        ok*) check "blackbox.$section" ok "${summary#ok }" ;;
        *) check "blackbox.$section" fail "${summary#fail }" ;;
    esac
done
finish

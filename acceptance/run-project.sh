#!/usr/bin/env bash
# Runs one demo project through GitFab (Annex 3 §10): posts the request in the project Room as the
# acceptance Human, which becomes the task Thread's root (Annex 3: Human requests are anchored in Matrix
# events), starts the run, follows it to its end and then verifies its events and its Release with the
# same acceptance_run_id, including the Runner's deployment receipt.
#
#   ./acceptance/run-project.sh DEMO-02 --backend github [--timeout-minutes 240]
#
# Configuration (acceptance/gitfab.env): GITFAB_SERVER_URL, GITFAB_MATRIX_URL, GITFAB_JOURNAL, and two
# credential files of the acceptance Human, each mode 0600: GITFAB_MATRIX_TOKEN_FILE (Matrix access token,
# to post the request) and GITFAB_SESSION_TOKEN_FILE (Hagency session, to start and follow the run). The
# project's acceptance/projects/<id>.env names the Hagency project, the Room, the three roles, the Spec
# and the goal.
#
# The Spec waits for the Human's confirmation in the Thread (/gitfab approve). GITFAB_AUTO_APPROVE_SPEC=1
# confirms it from here instead, for unattended script runs. A run that stops for a Human decision or
# fails ends this command with exit 1 and names the stage.
. "$(dirname "$0")/lib/common.sh"

PROJECT_ID=${1:-}
shift || true
want_backend="" timeout_minutes=240
while [ "$#" -gt 0 ]; do
    case "$1" in
        --backend) want_backend=${2:-}; shift 2 ;;
        --timeout-minutes) timeout_minutes=${2:-}; shift 2 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done
if [ -z "$PROJECT_ID" ] || [ -z "$want_backend" ]; then
    echo "usage: run-project.sh DEMO-0N --backend github|gitcode [--timeout-minutes N]" >&2
    exit 2
fi
case "$timeout_minutes" in '' | *[!0-9]*) echo "--timeout-minutes must be a number" >&2; exit 2 ;; esac

if ! load_project "$PROJECT_ID"; then
    check config fail "acceptance/projects/$PROJECT_ID.env missing or unreadable"
    finish 2
fi
REPO_REF=$REPO
if [ "$BACKEND" != "$want_backend" ]; then
    check config fail "--backend $want_backend, but $PROJECT_ID is recorded for ${BACKEND:-<unset>}"
    finish 2
fi
missing=""
for key in REPO BASELINE_SHA HAGENCY_PROJECT ROOM_ID COORDINATOR IMPLEMENTER REVIEWER SPEC_REF GOAL; do
    value=${!key}
    case "$value" in "" | *【*) missing="$missing $key" ;; esac
done
# Optional; a project whose PRD fixes the Release tag format (DEMO-01) lists it with a placeholder.
case "$RELEASE_TAG" in *【*) missing="$missing RELEASE_TAG" ;; esac
for key in GITFAB_SERVER_URL GITFAB_MATRIX_URL GITFAB_MATRIX_TOKEN_FILE GITFAB_SESSION_TOKEN_FILE; do
    [ -n "${!key:-}" ] || missing="$missing $key"
done
if [ -n "$missing" ]; then
    check config fail "not filled in:$missing"
    finish 2
fi
check config ok "$PROJECT_ID $BACKEND $REPO at $BASELINE_SHA"
server=${GITFAB_SERVER_URL%/}
matrix=${GITFAB_MATRIX_URL%/}

json_text() { local s=$1; s=${s//\\/\\\\}; s=${s//\"/\\\"}; s=${s//$'\n'/\\n}; printf '"%s"' "$s"; }
urlencode() {
    local s=$1 out="" c i
    for ((i = 0; i < ${#s}; i++)); do
        c=${s:i:1}
        case "$c" in [a-zA-Z0-9.~_-]) out+=$c ;; *) out+=$(printf '%%%02X' "'$c") ;; esac
    done
    printf '%s' "$out"
}

# 1. The Human's request in the Room: the root of the task Thread.
task_id="acc-$(printf '%s' "$PROJECT_ID" | tr 'A-Z' 'a-z')-${ACCEPTANCE_RUN_ID#ar-}"
request="GitFab 验收运行 ${ACCEPTANCE_RUN_ID}：${PROJECT_ID}（${BACKEND} ${REPO}@${BASELINE_SHA}）"
http_send PUT "$matrix/_matrix/client/v3/rooms/$(urlencode "$ROOM_ID")/send/m.room.message/$task_id" \
    "$GITFAB_MATRIX_TOKEN_FILE" "{\"msgtype\":\"m.text\",\"body\":$(json_text "$request")}"
root=$(printf '%s' "$HTTP_BODY" | json_field event_id)
if [ "$HTTP_STATUS" != 200 ] || [ -z "$root" ]; then
    check run.request fail "HTTP $HTTP_STATUS posting the request to $ROOM_ID"
    finish
fi
check run.request ok "task $task_id, Thread root $root"

# 2. Start the run.
runs="$server/hagency/api/v1/projects/$(urlencode "$HAGENCY_PROJECT")/gitfab/runs"
start="{\"taskId\":$(json_text "$task_id"),\"roomId\":$(json_text "$ROOM_ID"),\"threadRoot\":$(json_text "$root"),"
start+="\"bindings\":{\"coordinator\":$(json_text "$COORDINATOR"),\"implementer\":$(json_text "$IMPLEMENTER"),"
start+="\"reviewer\":$(json_text "$REVIEWER")},\"context\":{\"goal\":$(json_text "$GOAL"),"
start+="\"gitBackend\":$(json_text "$BACKEND"),\"repoRef\":$(json_text "$REPO"),"
start+="\"baselineSha\":$(json_text "$BASELINE_SHA"),\"specRef\":$(json_text "$SPEC_REF"),"
start+="\"projectId\":$(json_text "$PROJECT_ID")${RELEASE_TAG:+,\"releaseTag\":$(json_text "$RELEASE_TAG")}}}"
http_send POST "$runs" "$GITFAB_SESSION_TOKEN_FILE" "$start"
run_id=$(printf '%s' "$HTTP_BODY" | json_field id)
[ -n "$run_id" ] || run_id=$(printf '%s' "$HTTP_BODY" | json_field runId)
if [ "${HTTP_STATUS#2}" = "$HTTP_STATUS" ] || [ -z "$run_id" ]; then
    check run.start fail "HTTP $HTTP_STATUS starting the run: $(printf '%s' "$HTTP_BODY" | json_field code)"
    finish
fi
check run.start ok "run $run_id"

# 3. Follow it. Every state seen is kept in the bundle.
deadline=$(($(date +%s) + timeout_minutes * 60))
approved="" last=""
while :; do
    http_get "$runs/$run_id" "$GITFAB_SESSION_TOKEN_FILE"
    if [ "$HTTP_STATUS" != 200 ]; then
        check run.follow fail "HTTP $HTTP_STATUS reading run $run_id"
        finish
    fi
    # Only the run's state carries "kind" and "reason" in this view.
    kind=$(printf '%s' "$HTTP_BODY" | json_field kind)
    if [ "$kind" != "$last" ]; then
        printf '%s %s\n' "$(now_utc)" "$(printf '%s' "$HTTP_BODY" | tr -d '\n')" >>"$RUN_DIR/run-states.jsonl"
        log "run $run_id: $kind"
        last=$kind
    fi
    case "$kind" in
        completed) check run.result ok "run $run_id completed"; break ;;
        failed)
            check run.result fail "run $run_id failed: $(printf '%s' "$HTTP_BODY" | json_field reason)"
            finish ;;
        manual_intervention)
            check run.result fail "run $run_id stopped for a Human: $(printf '%s' "$HTTP_BODY" | json_field reason)"
            finish ;;
        awaiting_spec_approval)
            if [ "${GITFAB_AUTO_APPROVE_SPEC:-}" = 1 ] && [ -z "$approved" ]; then
                http_send POST "$runs/$run_id/spec" "$GITFAB_SESSION_TOKEN_FILE" '{"decision":"approve"}'
                if [ "${HTTP_STATUS#2}" = "$HTTP_STATUS" ]; then
                    check run.spec fail "HTTP $HTTP_STATUS confirming the Spec"
                    finish
                fi
                check run.spec ok "Spec confirmed by the acceptance script (GITFAB_AUTO_APPROVE_SPEC=1)"
                approved=1
            fi ;;
    esac
    if [ "$(date +%s)" -ge "$deadline" ]; then
        check run.result fail "run $run_id still $kind after $timeout_minutes minutes"
        finish
    fi
    sleep "${GITFAB_POLL_SECONDS:-10}"
done

# 4. The same acceptance run verifies what the run produced.
here=$(dirname "$0")
sub() {
    local name=$1 code
    shift
    set +e
    ACCEPTANCE_RUN_ID=$ACCEPTANCE_RUN_ID "$@" >/dev/null
    code=$?
    set -e
    case "$code" in
        0) check "$name" ok "passed" ;;
        3) check "$name" fail "passed what it can check; part is not available yet (exit 3)"; pending=1 ;;
        *) check "$name" fail "exit $code, see its report in this bundle" ;;
    esac
}
pending=""
sub verify.events "$here/verify-events.sh"
sub verify.release "$here/verify-release.sh" "$PROJECT_ID"
FINAL_SHA=$(json_field final_sha <"$RUN_DIR/verify-release-report.json" 2>/dev/null || true)
# The frozen black-box cases, run by the acceptance side on the Release verify-release just checked.
release_tag=$(json_field tag <"$RUN_DIR/deploy-receipt.json" 2>/dev/null || true)
sub verify.blackbox "$here/verify-blackbox.sh" "$PROJECT_ID" ${release_tag:+--tag "$release_tag"}
if [ "$FAILED_STAGE" = verify.release ] && [ -n "$pending" ]; then
    finish 3
fi
finish

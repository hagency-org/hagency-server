# Shared by the acceptance commands (Annex 3 §10.1). Sourced, never run.
#
# - Exit 0 only when every required check passed; a failed check exits 1, a usage or setup error 2,
#   a command not yet available 3.
# - Each run has one acceptance_run_id. Export ACCEPTANCE_RUN_ID to put several commands into the same
#   evidence bundle evidence/<acceptance_run_id>/.
# - Credentials are read from files named in the configuration, passed to curl on standard input, and
#   never printed, logged or written to the bundle.
#
# Portable to bash 3.2 (macOS) and later; needs coreutils or BSD equivalents, curl and git.

set -euo pipefail

ACCEPTANCE_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
REPO_ROOT=$(cd "$ACCEPTANCE_DIR/.." && pwd)
GITFAB_CONFIG=${GITFAB_CONFIG:-$ACCEPTANCE_DIR/gitfab.env}
if [ -f "$GITFAB_CONFIG" ]; then
    # shellcheck disable=SC1090
    . "$GITFAB_CONFIG"
fi
EVIDENCE_ROOT=${GITFAB_EVIDENCE_DIR:-$REPO_ROOT/evidence}

gitfab_bin() {
    if [ -n "${GITFAB_BIN:-}" ]; then
        printf '%s' "$GITFAB_BIN"
    elif command -v gitfab >/dev/null 2>&1; then
        command -v gitfab
    elif [ -x "$REPO_ROOT/target/release/gitfab" ]; then
        printf '%s' "$REPO_ROOT/target/release/gitfab"
    elif [ -x "$REPO_ROOT/target/debug/gitfab" ]; then
        printf '%s' "$REPO_ROOT/target/debug/gitfab"
    else
        return 1
    fi
}

now_utc() { date -u +%Y-%m-%dT%H:%M:%SZ; }

new_run_id() {
    printf 'ar-%s-%s' "$(date -u +%Y%m%dT%H%M%SZ)" \
        "$(od -An -N4 -tx1 /dev/urandom | tr -d ' \n')"
}

ACCEPTANCE_RUN_ID=${ACCEPTANCE_RUN_ID:-$(new_run_id)}
case "$ACCEPTANCE_RUN_ID" in
    ar-*) ;;
    *) echo "ACCEPTANCE_RUN_ID must start with ar-" >&2; exit 2 ;;
esac
RUN_DIR=$EVIDENCE_ROOT/$ACCEPTANCE_RUN_ID
mkdir -p "$RUN_DIR"
STARTED_AT=$(now_utc)
COMMAND_NAME=$(basename "$0" .sh)
FAILED_STAGE=""
CHECKS=""
PROJECT_ID=${PROJECT_ID:-}
REPO_REF=${REPO_REF:-}
BASELINE_SHA=${BASELINE_SHA:-}
FINAL_SHA=${FINAL_SHA:-}

log() { printf '%s %s %s\n' "$(now_utc)" "$COMMAND_NAME" "$*" >&2; }

# Text safe to keep: local home and working paths become placeholders.
scrub() {
    local s=$1
    s=${s//"$REPO_ROOT"/<repo>}
    if [ -n "${HOME:-}" ]; then s=${s//"$HOME"/<home>}; fi
    printf '%s' "$s"
}

json_str() {
    local s
    s=$(scrub "$1")
    s=${s//\\/\\\\}
    s=${s//\"/\\\"}
    s=${s//$'\n'/\\n}
    s=${s//$'\r'/\\r}
    s=${s//$'\t'/\\t}
    printf '"%s"' "$s"
}

# check <name> <ok|warn|fail|skip> <detail>. A fail marks the first failed stage.
check() {
    local name=$1 status=$2 detail=${3:-}
    local entry
    entry="{\"name\":$(json_str "$name"),\"status\":$(json_str "$status"),\"detail\":$(json_str "$detail")}"
    if [ -z "$CHECKS" ]; then CHECKS=$entry; else CHECKS="$CHECKS,$entry"; fi
    if [ "$status" = fail ] && [ -z "$FAILED_STAGE" ]; then FAILED_STAGE=$name; fi
    log "$status $name${detail:+: $(scrub "$detail")}"
}

sha256_file() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d' ' -f1
    else
        shasum -a 256 "$1" | cut -d' ' -f1
    fi
}

sha256_text() {
    if command -v sha256sum >/dev/null 2>&1; then
        printf '%s' "$1" | sha256sum | cut -d' ' -f1
    else
        printf '%s' "$1" | shasum -a 256 | cut -d' ' -f1
    fi
}

# One digest over every file of a directory: sorted "sha256  relative/path" lines, hashed.
sha256_tree() {
    local dir=$1 list="" f
    [ -d "$dir" ] || { printf 'absent'; return; }
    while IFS= read -r f; do
        list="$list$(sha256_file "$dir/$f")  $f"$'\n'
    done < <(cd "$dir" && find . -type f ! -name '.DS_Store' | sed 's|^\./||' | LC_ALL=C sort)
    sha256_text "$list"
}

# A credential file must exist and be readable by its owner only.
read_secret() {
    local file=$1 mode
    [ -f "$file" ] || return 1
    mode=$(stat -f '%Lp' "$file" 2>/dev/null || stat -c '%a' "$file")
    case "$mode" in
        600 | 400) ;;
        *) log "warn credential file is mode $mode, expected 0600"; ;;
    esac
    tr -d '\r\n' <"$file"
}

# http_get <url> [token-file] sets HTTP_STATUS and HTTP_BODY in the calling shell (call it directly, not
# inside $(...)). The token goes to curl on standard input, so it is not in the process list.
HTTP_STATUS=000
HTTP_BODY=""
http_get() {
    local url=$1 token_file=${2:-} out token=""
    HTTP_STATUS=000
    HTTP_BODY=""
    if [ -n "$token_file" ]; then
        token=$(read_secret "$token_file") || return 0
    fi
    out=$(mktemp)
    HTTP_STATUS=$(
        {
            if [ -n "$token" ]; then printf 'header = "Authorization: Bearer %s"\n' "$token"; fi
            printf 'header = "Accept: application/json"\n'
            printf 'header = "User-Agent: gitfab-acceptance"\n'
        } | curl -sS -K - -o "$out" -w '%{http_code}' --max-time 20 "$url" 2>/dev/null
    ) || HTTP_STATUS=000
    HTTP_BODY=$(cat "$out")
    rm -f "$out"
}

# Minimal JSON field read for flat API responses: json_field <key> reads stdin.
json_field() {
    local key=$1
    tr -d '\n' | sed -n "s/.*\"$key\"[[:space:]]*:[[:space:]]*\"\\([^\"]*\\)\".*/\\1/p" | head -n1
}

# Writes evidence/<id>/<command>-report.json and the bundle manifest, prints the report path, and exits
# 0 only when nothing failed; otherwise with $1 (default 1).
finish() {
    local ok=true code=0 report
    if [ -n "$FAILED_STAGE" ]; then ok=false; code=${1:-1}; fi
    report="$RUN_DIR/$COMMAND_NAME-report.json"
    {
        printf '{\n'
        printf '  "command": %s,\n' "$(json_str "$COMMAND_NAME")"
        printf '  "acceptance_run_id": %s,\n' "$(json_str "$ACCEPTANCE_RUN_ID")"
        printf '  "started_at": %s,\n' "$(json_str "$STARTED_AT")"
        printf '  "finished_at": %s,\n' "$(json_str "$(now_utc)")"
        printf '  "project_id": %s,\n' "$(json_str "$PROJECT_ID")"
        printf '  "repo": %s,\n' "$(json_str "$REPO_REF")"
        printf '  "baseline_sha": %s,\n' "$(json_str "$BASELINE_SHA")"
        printf '  "final_sha": %s,\n' "$(json_str "$FINAL_SHA")"
        printf '  "environment_digest": %s,\n' "$(json_str "${ENVIRONMENT_DIGEST:-}")"
        printf '  "failed_stage": %s,\n' "$(json_str "$FAILED_STAGE")"
        printf '  "ok": %s,\n' "$ok"
        printf '  "checks": [%s]\n' "$CHECKS"
        printf '}\n'
    } >"$report"
    local gitfab
    if gitfab=$(gitfab_bin); then
        if ! "$gitfab" manifest "$RUN_DIR" >/dev/null; then
            log "fail manifest: the bundle holds forbidden material; see: gitfab manifest $(scrub "$RUN_DIR")"
            code=1
        fi
    fi
    log "done ok=$ok acceptance_run_id=$ACCEPTANCE_RUN_ID report=$(scrub "$report")"
    printf '%s\n' "$(scrub "$report")"
    exit "$code"
}

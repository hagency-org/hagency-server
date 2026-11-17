#!/usr/bin/env bash
# Preflight (Annex 3 §3, §10): probes the environment, services and repositories without side effects and
# writes evidence/<acceptance_run_id>/preflight.json. Exit 0 when every required check passed; items that
# do not block the core flow are recorded as warnings (Annex 3 §2).
#
#   ./acceptance/preflight.sh            all projects in acceptance/projects/*.env
#   ./acceptance/preflight.sh DEMO-02    one project
. "$(dirname "$0")/lib/common.sh"

only=${1:-}

# --- Linux Runner (Annex 3 §3.1) ---
os_name=$(uname -s)
if [ -r /etc/os-release ]; then
    distro=$(. /etc/os-release && printf '%s' "${PRETTY_NAME:-$NAME}")
elif command -v sw_vers >/dev/null 2>&1; then
    distro="macOS $(sw_vers -productVersion)"
else
    distro=$os_name
fi
kernel=$(uname -r)
arch=$(uname -m)
cpus=$(getconf _NPROCESSORS_ONLN 2>/dev/null || sysctl -n hw.ncpu)
if [ -r /proc/meminfo ]; then
    mem_gib=$(awk '/MemTotal/ {printf "%d", $2/1024/1024}' /proc/meminfo)
else
    mem_gib=$(( $(sysctl -n hw.memsize) / 1024 / 1024 / 1024 ))
fi
disk_gib=$(df -Pk "$EVIDENCE_ROOT" | awk 'NR==2 {printf "%d", $4/1024/1024}')

[ "$os_name" = Linux ] && check runner.os ok "$distro" \
    || check runner.os warn "$distro: the acceptance Runner is Linux (Annex 3 §3.1)"
check runner.kernel ok "$kernel"
[ "$arch" = x86_64 ] && check runner.arch ok "$arch" \
    || check runner.arch warn "$arch: x86_64 unless confirmed in writing"
[ "$cpus" -ge 4 ] && check runner.cpu ok "$cpus vCPU" || check runner.cpu warn "$cpus vCPU, reference ≥ 4"
[ "$mem_gib" -ge 8 ] && check runner.memory ok "$mem_gib GiB" || check runner.memory warn "$mem_gib GiB, reference ≥ 8"
[ "$disk_gib" -ge 40 ] && check runner.disk ok "$disk_gib GiB free" || check runner.disk warn "$disk_gib GiB free, reference ≥ 40"
if command -v timedatectl >/dev/null 2>&1; then
    synced=$(timedatectl show -p NTPSynchronized --value 2>/dev/null || echo unknown)
    [ "$synced" = yes ] && check runner.clock ok "NTP synchronized" || check runner.clock warn "NTP synchronized: $synced"
else
    check runner.clock warn "timedatectl unavailable; record the NTP source by hand"
fi

# --- Toolchain ---
version_of() { "$@" 2>/dev/null | head -n1; }
runtime=""
for candidate in docker podman; do
    if command -v "$candidate" >/dev/null 2>&1; then
        runtime="$candidate $(version_of "$candidate" --version)"
        break
    fi
done
[ -n "$runtime" ] && check tool.container_runtime ok "$runtime" \
    || check tool.container_runtime fail "neither docker nor podman is installed"
if command -v docker >/dev/null 2>&1 && docker buildx version >/dev/null 2>&1; then
    check tool.oci_builder ok "$(version_of docker buildx version)"
elif command -v buildah >/dev/null 2>&1; then
    check tool.oci_builder ok "$(version_of buildah --version)"
else
    check tool.oci_builder warn "no buildx or buildah found"
fi
for tool in rustc cargo git curl; do
    if command -v "$tool" >/dev/null 2>&1; then
        check "tool.$tool" ok "$(version_of "$tool" --version)"
    else
        check "tool.$tool" fail "$tool is not installed"
    fi
done
command -v node >/dev/null 2>&1 && check tool.node ok "$(version_of node --version)" \
    || check tool.node skip "not used by the Runner"
if gitfab=$(gitfab_bin); then
    check tool.gitfab ok "$(scrub "$gitfab")"
else
    check tool.gitfab fail "gitfab not found: cargo build --release -p hagency-gitfab-events"
fi

# --- Services (Annex 3 §3.2); each probe is a read ---
if [ -n "${GITFAB_MATRIX_URL:-}" ]; then
    http_get "${GITFAB_MATRIX_URL%/}/_matrix/client/versions"
    case "$HTTP_STATUS:$HTTP_BODY" in
        200:*versions*) check service.matrix ok "${GITFAB_MATRIX_URL%/}" ;;
        *) check service.matrix fail "HTTP $HTTP_STATUS from /_matrix/client/versions" ;;
    esac
else
    check service.matrix warn "GITFAB_MATRIX_URL not set"
fi
if [ -n "${GITFAB_SERVER_URL:-}" ]; then
    http_get "${GITFAB_SERVER_URL%/}/healthz"
    [ "$HTTP_STATUS" = 200 ] && check service.hagency ok "${GITFAB_SERVER_URL%/}/healthz" \
        || check service.hagency fail "HTTP $HTTP_STATUS from /healthz"
    http_get "${GITFAB_SERVER_URL%/}/hagency/gitfab/v1/public-keys"
    key_id=$(printf '%s' "$HTTP_BODY" | json_field keyId)
    public=$(printf '%s' "$HTTP_BODY" | json_field publicKeyBase64)
    if [ "$HTTP_STATUS" = 200 ] && [ -n "$key_id" ] && [ -n "$public" ]; then
        mkdir -p "$RUN_DIR/keys"
        printf '%s\n' "$public" >"$RUN_DIR/keys/$key_id.pub"
        pinned="${GITFAB_KEYS_DIR:-$ACCEPTANCE_DIR/keys}/$key_id.pub"
        if [ ! -f "$pinned" ]; then
            check service.public_key warn "$key_id published, no pinned copy in acceptance/keys to compare"
        elif [ "$(tr -d '\r\n' <"$pinned")" = "$public" ]; then
            check service.public_key ok "$key_id matches the pinned key"
        else
            check service.public_key fail "$key_id differs from the pinned key"
        fi
    else
        check service.public_key fail "no GitFab key published at /hagency/gitfab/v1/public-keys (HTTP $HTTP_STATUS)"
    fi
else
    check service.hagency warn "GITFAB_SERVER_URL not set"
fi
if [ -n "${GITFAB_REGISTRY_URL:-}" ]; then
    http_get "${GITFAB_REGISTRY_URL%/}/v2/"
    case "$HTTP_STATUS" in
        200 | 401) check service.registry ok "${GITFAB_REGISTRY_URL%/}/v2/ answers ($HTTP_STATUS)" ;;
        *) check service.registry fail "HTTP $HTTP_STATUS from /v2/" ;;
    esac
else
    check service.registry warn "GITFAB_REGISTRY_URL not set"
fi

# --- Projects: repository, baseline and PRD (Annex 3 §4.1, §10.2) ---
api_for() {
    case "$1" in
        github) printf '%s' "${GITFAB_GITHUB_API:-https://api.github.com}" ;;
        gitcode) printf '%s' "${GITFAB_GITCODE_API:-https://api.gitcode.com/api/v5}" ;;
    esac
}
token_for() {
    case "$1" in
        github) printf '%s' "${GITFAB_GITHUB_TOKEN_FILE:-}" ;;
        gitcode) printf '%s' "${GITFAB_GITCODE_TOKEN_FILE:-}" ;;
    esac
}
projects=""
seen_projects=0
for env in "$ACCEPTANCE_DIR"/projects/*.env; do
    [ -f "$env" ] || continue
    # Read as data, never executed: KEY=VALUE lines with known keys only.
    PROJECT_ID="" BACKEND="" REPO="" BRANCH="" BASELINE_SHA="" PRD="" PRD_SHA256="" bad=""
    while IFS= read -r line || [ -n "$line" ]; do
        case "$line" in '' | '#'*) continue ;; esac
        key=${line%%=*}
        case "$key" in
            HAGENCY_PROJECT | ROOM_ID | COORDINATOR | IMPLEMENTER | REVIEWER | SPEC_REF | GOAL | DEPLOY_PORT \
                | RELEASE_TAG | CONTAINER_PORT | HEALTH_PATH | VERSION_PATH) ;;
            PROJECT_ID | BACKEND | REPO | BRANCH | BASELINE_SHA | PRD | PRD_SHA256)
                [ "$key" != "$line" ] && printf -v "$key" '%s' "${line#*=}" || bad=$line ;;
            *) bad=$line ;;
        esac
    done <"$env"
    name=$(basename "$env" .env)
    if [ -n "$bad" ] || [ -z "$PROJECT_ID" ]; then
        check "$name.config" fail "acceptance/projects/$name.env: unreadable line or missing PROJECT_ID"
        continue
    fi
    [ -z "$only" ] || [ "$PROJECT_ID" = "$only" ] || continue
    printf '%s|%s|%s|%s|%s|%s|%s\n' "$PROJECT_ID" "$BACKEND" "$REPO" "$BRANCH" "$BASELINE_SHA" "$PRD" "$PRD_SHA256"
done >"$RUN_DIR/.projects.tmp"
project_lines=$(cat "$RUN_DIR/.projects.tmp")
rm -f "$RUN_DIR/.projects.tmp"
# The report names the project asked for, not the last file read.
PROJECT_ID=$only REPO_REF="" BASELINE_SHA=""
while IFS='|' read -r pid backend repo branch baseline prd prd_sha; do
    [ -n "$pid" ] || continue
    seen_projects=$((seen_projects + 1))
    projects="$projects $pid"
    case "$backend" in
        github | gitcode) ;;
        *) check "$pid.backend" fail "BACKEND must be github or gitcode"; continue ;;
    esac
    if [ -n "$prd" ] && [ -f "$ACCEPTANCE_DIR/$prd" ]; then
        actual=$(sha256_file "$ACCEPTANCE_DIR/$prd")
        if [ -z "$prd_sha" ]; then
            check "$pid.prd" warn "$prd sha256 $actual, not pinned"
        elif [ "$actual" = "$prd_sha" ]; then
            check "$pid.prd" ok "$prd sha256 $actual"
        else
            check "$pid.prd" fail "$prd sha256 $actual, pinned $prd_sha"
        fi
    else
        check "$pid.prd" warn "PRD file not found in acceptance/${prd:-<unset>}"
    fi
    case "$repo$baseline" in
        *【*) check "$pid.config" warn "placeholders remain in acceptance/projects/$pid.env"; continue ;;
    esac
    token_file=$(token_for "$backend")
    if [ -z "$token_file" ] || [ ! -f "$token_file" ]; then
        check "$pid.token" warn "no $backend token file; probing as anonymous"
        token_file=""
    fi
    api=$(api_for "$backend")
    http_get "$api/repos/$repo" "$token_file"
    if [ "$HTTP_STATUS" = 200 ]; then
        default=$(printf '%s' "$HTTP_BODY" | json_field default_branch)
        check "$pid.repo" ok "$backend $repo, default branch ${default:-unknown}"
    else
        check "$pid.repo" fail "HTTP $HTTP_STATUS reading $backend $repo"
        continue
    fi
    if [ -z "$baseline" ]; then
        http_get "$api/repos/$repo/commits/${branch:-${default:-main}}" "$token_file"
        head=$(printf '%s' "$HTTP_BODY" | json_field sha)
        check "$pid.baseline" warn "BASELINE_SHA not pinned; ${branch:-${default:-main}} is at ${head:-unknown}"
    else
        http_get "$api/repos/$repo/commits/$baseline" "$token_file"
        [ "$HTTP_STATUS" = 200 ] && check "$pid.baseline" ok "$baseline" \
            || check "$pid.baseline" fail "HTTP $HTTP_STATUS reading commit $baseline"
    fi
done <<EOF_PROJECTS
$project_lines
EOF_PROJECTS
if [ "$seen_projects" -eq 0 ]; then
    check projects warn "no project in acceptance/projects/*.env${only:+ matching $only}"
fi

# --- Digests of what the acceptance run is pinned to (Annex 3 §10.2) ---
scripts_digest=$(sha256_tree "$ACCEPTANCE_DIR" | head -c 64)
fixtures_digest=$(sha256_tree "$ACCEPTANCE_DIR/fixtures")
check digest.acceptance_package ok "$scripts_digest"
check digest.fixtures ok "$fixtures_digest"

environment="$distro|$kernel|$arch|$runtime|$(version_of rustc --version)|$(version_of cargo --version)"
ENVIRONMENT_DIGEST="sha256:$(sha256_text "$environment")"

{
    printf '{\n'
    printf '  "acceptance_run_id": %s,\n' "$(json_str "$ACCEPTANCE_RUN_ID")"
    printf '  "generated_at": %s,\n' "$(json_str "$(now_utc)")"
    printf '  "runner": {"os": %s, "kernel": %s, "arch": %s, "cpus": %s, "memory_gib": %s, "disk_free_gib": %s},\n' \
        "$(json_str "$distro")" "$(json_str "$kernel")" "$(json_str "$arch")" "$cpus" "$mem_gib" "$disk_gib"
    printf '  "container_runtime": %s,\n' "$(json_str "$runtime")"
    printf '  "projects": %s,\n' "$(json_str "${projects# }")"
    printf '  "acceptance_package_sha256": %s,\n' "$(json_str "$scripts_digest")"
    printf '  "fixtures_sha256": %s,\n' "$(json_str "$fixtures_digest")"
    printf '  "environment_digest": %s,\n' "$(json_str "$ENVIRONMENT_DIGEST")"
    printf '  "checks": [%s]\n' "$CHECKS"
    printf '}\n'
} >"$RUN_DIR/preflight.json"

finish

#!/usr/bin/env bash
# Archives an acceptance evidence bundle for retention (Annex 3 §12: acceptance logs are kept until 30
# days after the warranty ends), and checks an archive later.
#
#   ./acceptance/archive.sh ar-…  --retain-until 2028-06-30   # archive evidence/ar-…
#   ./acceptance/archive.sh --check evidence/archive/ar-….tar.gz
#
# Archiving refuses a bundle whose files no longer match its manifest. It writes, under
# evidence/archive/: <id>.tar.gz, <id>.tar.gz.sha256 (sha256sum format) and <id>.retention.json
# (run id, archive and manifest digests, archived_at, retain_until). It never changes the bundle.
# Checking recomputes the archive digest, unpacks it into a temporary directory and checks the
# manifest again. Exit 0 intact, 1 something differs, 2 usage or missing input.
set -euo pipefail
# Standalone: lib/common.sh would open a new evidence bundle, and archiving must not create one.
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
if [ -n "${GITFAB_BIN:-}" ]; then gitfab=$GITFAB_BIN
elif command -v gitfab >/dev/null 2>&1; then gitfab=$(command -v gitfab)
elif [ -x "$root/target/release/gitfab" ]; then gitfab=$root/target/release/gitfab
elif [ -x "$root/target/debug/gitfab" ]; then gitfab=$root/target/debug/gitfab
else echo "gitfab not found (set GITFAB_BIN)" >&2; exit 2
fi
evidence=${GITFAB_EVIDENCE_DIR:-$root/evidence}
out="$evidence/archive"
sha256_file() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi; }
now_utc() { date -u +%Y-%m-%dT%H:%M:%SZ; }
# Only validated run ids, hex digests and dates are written, so quoting is all they need.
json_str() { printf '"%s"' "$1"; }

if [ "${1:-}" = --check ]; then
    archive=${2:-}
    [ -f "$archive" ] && [ -f "$archive.sha256" ] || { echo "usage: archive.sh --check <id>.tar.gz (with its .sha256)" >&2; exit 2; }
    want=$(cut -d' ' -f1 <"$archive.sha256")
    if [ "$(sha256_file "$archive")" != "$want" ]; then
        echo "archive digest differs from $(basename "$archive").sha256" >&2
        exit 1
    fi
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT
    tar -xzf "$archive" -C "$tmp"
    id=$(basename "$archive" .tar.gz)
    if "$gitfab" check-manifest "$tmp/$id"; then
        echo "intact: $id"
        exit 0
    fi
    exit 1
fi

id=${1:-}
[ "${2:-}" = --retain-until ] && [ -n "${3:-}" ] || { echo "usage: archive.sh <acceptance_run_id> --retain-until YYYY-MM-DD | --check <archive>" >&2; exit 2; }
retain=$3
case "$id" in ar-*[!a-zA-Z0-9T-]*|'') echo "invalid run id: $id" >&2; exit 2 ;; ar-*) ;; *) echo "invalid run id: $id" >&2; exit 2 ;; esac
case "$retain" in [0-9][0-9][0-9][0-9]-[0-1][0-9]-[0-3][0-9]) ;; *) echo "retain-until must be YYYY-MM-DD" >&2; exit 2 ;; esac
[ -d "$evidence/$id" ] || { echo "no bundle $evidence/$id" >&2; exit 2; }
if ! "$gitfab" check-manifest "$evidence/$id" >"$evidence/$id.check.json.tmp"; then
    cat "$evidence/$id.check.json.tmp" >&2
    rm -f "$evidence/$id.check.json.tmp"
    exit 1
fi
rm -f "$evidence/$id.check.json.tmp"
mkdir -p "$out"
archive="$out/$id.tar.gz"
[ ! -e "$archive" ] || { echo "$archive exists; archives are never overwritten" >&2; exit 1; }
tar -czf "$archive" -C "$evidence" "$id"
digest=$(sha256_file "$archive")
printf '%s  %s\n' "$digest" "$id.tar.gz" >"$archive.sha256"
{
    printf '{\n'
    printf '  "acceptance_run_id": %s,\n' "$(json_str "$id")"
    printf '  "archive": %s,\n' "$(json_str "$id.tar.gz")"
    printf '  "archive_sha256": %s,\n' "$(json_str "$digest")"
    printf '  "manifest_sha256": %s,\n' "$(json_str "$(sha256_file "$evidence/$id/manifest.json")")"
    printf '  "archived_at": %s,\n' "$(json_str "$(now_utc)")"
    printf '  "retain_until": %s\n' "$(json_str "$retain")"
    printf '}\n'
} >"$out/$id.retention.json"
echo "archived: $archive ($digest), retain until $retain"

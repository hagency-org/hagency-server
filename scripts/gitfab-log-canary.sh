#!/usr/bin/env bash
# Checks that a running Hagency server does not write credentials into its log (Annex 3 §4.3).
#
# Sends requests carrying a random, worthless canary value as a query-string access_token, a
# Bearer Authorization header, a Cookie and a login password, then looks for the canary in the
# log. Also counts common credential patterns already in the log. Prints counts only, never log
# content.
#
#   scripts/gitfab-log-canary.sh http://127.0.0.1:8099 .run/gitfab-dev/server.log
#
# Exit 0: nothing found. 1: the canary or a credential pattern is in the log. 2: usage.
set -euo pipefail

base=${1:-}
log=${2:-}
if [[ -z $base || -z $log || ! -r $log ]]; then
  echo "usage: $0 <server base URL> <readable server log file>" >&2
  exit 2
fi
base=${base%/}
canary="canary$(od -An -tx1 -N8 /dev/urandom | tr -d ' \n')"

send() {
  curl -s -o /dev/null -w '%{http_code}' --max-time 10 "$@" || printf 'unreachable'
}
login=$(printf '{"type":"m.login.password","identifier":{"type":"m.id.user","user":"gitfab-canary"},"password":"%s"}' "$canary")
codes=()
codes+=("$(send -H "Authorization: Bearer $canary" -H "Cookie: session=$canary" \
  "$base/_matrix/client/v3/account/whoami?access_token=$canary")")
codes+=("$(send -H "Authorization: Bearer $canary" "$base/hagency/gitfab/v1/agents?token=$canary")")
codes+=("$(send -H "Cookie: session=$canary" "$base/hagency/api/v1/me?password=$canary")")
codes+=("$(send -X POST -H 'Content-Type: application/json' -d "$login" "$base/_matrix/client/v3/login")")
sleep 1

failed=0
hits=$(grep -c -- "$canary" "$log" || true)
printf 'requests answered: %s\n' "${codes[*]}"
printf '%-28s %s\n' "canary in log" "$hits"
[[ $hits == 0 ]] || failed=1
# Patterns that only appear when a credential value is written out.
while IFS='|' read -r name pattern; do
  n=$(grep -cE -- "$pattern" "$log" || true)
  printf '%-28s %s\n' "$name" "$n"
  [[ $n == 0 ]] || failed=1
done <<'EOF'
Bearer value|Bearer [A-Za-z0-9._~+/=-]{8,}
access_token in a URL|access_token=[^&[:space:]]{8,}
Authorization header|Authorization:
Cookie header|Cookie: [^[:space:]]
database URL with password|postgres(ql)?://[^:/@[:space:]]+:[^@[:space:]]+@
GitHub token|gh[pousr]_[A-Za-z0-9]{20,}|github_pat_
private key|BEGIN [A-Z ]*PRIVATE KEY
EOF
if [[ $failed == 0 ]]; then
  echo "result: no credential found in the log"
else
  echo "result: credential material found in the log" >&2
fi
exit "$failed"

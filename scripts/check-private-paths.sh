#!/bin/sh
# Keeps the operator's local environment out of commits: home-directory paths
# (macOS and Windows), Claude/Codex scratch paths, the River host name and the
# agents' working logs. Placeholder home names used by tests are allowed.
# Usage: scripts/check-private-paths.sh [--staged|--all]
#   --staged (default, used by the pre-commit hook) checks what is being added.
#   --all checks every tracked file.
set -eu
mode=${1:---staged}
self=scripts/check-private-paths.sh
pattern='/Users/[A-Za-z][A-Za-z0-9._-]*/|[A-Za-z]:(\\)+Users(\\)+[A-Za-z][A-Za-z0-9._-]*|/private/tmp/claude-|-Users-[A-Za-z][A-Za-z0-9._-]*-home|(^|[^A-Za-z0-9])mini4([^A-Za-z0-9]|$)'
placeholders='(/|-|\\)Users(/|-|\\)+(example|owner|fixture|someone|a|x|usage-oracle|test|tester|user|username|name|me|you)([/\\-]|$)'
logs='^docs/(progress|agent-knowledge)\.md$'
status=0
if [ "$mode" = --all ]; then
    if git ls-files | grep -Eq "$logs"; then
        echo "Agent working logs are tracked:"; git ls-files | grep -E "$logs"; status=1
    fi
    found=$(git grep -noIE "$pattern" -- . ":!$self" | grep -vE "$placeholders" || true)
else
    if git ls-files | grep -Eq "$logs"; then
        echo "Refusing to commit the agents' working logs (docs/progress.md, docs/agent-knowledge.md); keep them local."
        status=1
    fi
    found=$(git diff --cached -U0 --no-color -- . ":!$self" | grep -E '^\+[^+]' | grep -oE "$pattern" | grep -vE "$placeholders" || true)
fi
if [ -n "$found" ]; then echo "Local paths or host names:"; echo "$found" | head -20; status=1; fi
if [ $status -ne 0 ]; then
    echo "Use placeholders instead (~/, <scratch>, <server-home>/, the River host)."
fi
exit $status

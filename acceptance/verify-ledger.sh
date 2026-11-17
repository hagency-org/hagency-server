#!/usr/bin/env bash
# Recomputes one project's token ledger from its memory and token events (Annex 3 §7.4) and checks their
# signatures; fails on any invalid line, conflicting event or bad correction.
#
#   ./acceptance/verify-ledger.sh DEMO-05 [FILE.jsonl...]   (default: the journal named by GITFAB_JOURNAL)
. "$(dirname "$0")/lib/common.sh"

[ "$#" -ge 1 ] || { echo "usage: verify-ledger.sh PROJECT_ID [FILE.jsonl...]" >&2; exit 2; }
PROJECT_ID=$1
shift
gitfab=$(gitfab_bin) || { check tool.gitfab fail "gitfab not found"; finish; }
if [ "$#" -gt 0 ]; then files=("$@"); else files=("${GITFAB_JOURNAL:?set GITFAB_JOURNAL or pass files}"); fi

if "$gitfab" export --project "$PROJECT_ID" --out "$RUN_DIR" "${files[@]}" >"$RUN_DIR/ledger-export.json"; then
    check ledger.export ok "$(tr -d '\n ' <"$RUN_DIR/ledger-export.json" | sed -n 's/.*"files":\({[^}]*}\).*/\1/p')"
else
    check ledger.export fail "see ledger-export.json"
    finish
fi
keys=${GITFAB_KEYS_DIR:-$ACCEPTANCE_DIR/keys}
ledger_files=("$RUN_DIR/memory-events.jsonl" "$RUN_DIR/token-events.jsonl")
if ls "$keys"/*.pub >/dev/null 2>&1; then
    if "$gitfab" verify --keys "$keys" "${ledger_files[@]}" >"$RUN_DIR/ledger-verify.json"; then
        check ledger.signatures ok "memory and token events verify"
    else
        check ledger.signatures fail "see ledger-verify.json"
    fi
else
    check ledger.signatures fail "no pinned keys in acceptance/keys"
fi
if [ ! -s "$RUN_DIR/memory-events.jsonl" ] && [ ! -s "$RUN_DIR/token-events.jsonl" ]; then
    check ledger.recompute fail "no memory or token events for $PROJECT_ID"
elif "$gitfab" ledger "${ledger_files[@]}" >"$RUN_DIR/ledger-report.json"; then
    check ledger.recompute ok "totals in ledger-report.json"
else
    check ledger.recompute fail "see ledger-report.json"
fi
finish

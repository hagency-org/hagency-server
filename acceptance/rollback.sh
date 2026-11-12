#!/usr/bin/env bash
# Reference entry from Annex 3 §10. Not yet available: it exits 3 and records that, so an acceptance run
# can never mistake it for a pass.
. "$(dirname "$0")/lib/common.sh"
PROJECT_ID=${1:-}
check rollback fail "not yet implemented in this acceptance package"
finish 3

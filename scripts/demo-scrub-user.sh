#!/usr/bin/env bash
# Remove a DEMO account so its username can be created again, on one instance.
# Usage: scripts/demo-scrub-user.sh <username> <alpha|beta> [desktop-data-dir]
#
# Deletes the Pasion user (with FK children) and the Palpo user rows, and wipes
# the persona desktop data dir if given (only a demo-* dir directly under the
# demo data root; see demo-scrub/data_dir_guard.sh). Run it with every desktop/host of that
# account stopped. It does NOT touch the hagency server DB: Agents there are
# permanent by design (trigger `permanent_agent_identity`); a re-created user
# with the SAME username gets the same MXID and inherits them, so prefer a new
# username afterwards (documented in the walkthrough).
set -euo pipefail
user="${1:?username}"; inst="${2:?alpha|beta}"; data="${3:-}"
case "$inst" in alpha) tls=8450; suf=fa;; beta) tls=8452; suf=fb;; *) echo "instance must be alpha or beta"; exit 2;; esac
here="$(cd "$(dirname "$0")" && pwd)"
given="$data"
if [ -n "$data" ]; then # validate before any delete, so a bad path changes nothing
  . "$here/demo-scrub/data_dir_guard.sh"
  data="$(resolve_demo_data_dir "$here" "$data")" || exit 2
fi
python3 -I "$here/demo-scrub/pasion_delete.py" "pasion_$suf" "$user"
python3 -I "$here/demo-scrub/palpo_delete.py" "palpo_$suf" "@$user:localhost:$tls"
if [ -n "$data" ]; then
  if pgrep -f "HAGENCY_DESKTOP_DATA_DIR=($given|$data)" >/dev/null 2>&1; then echo "a desktop still uses $data; stop it first"; exit 1; fi
  rm -rf "${data:?}" && echo "wiped $data"
fi
echo "done: create a NEW username next time (same name resurrects the old Agents)"

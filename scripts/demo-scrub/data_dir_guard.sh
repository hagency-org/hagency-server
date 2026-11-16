# Sourced by demo-scrub-user.sh. Prints the canonical persona data dir, or
# fails: only a demo-* directory directly under the demo data root may be
# wiped, so a typo such as "." or "$HOME" can never reach `rm -rf`.
# Root: $HAGENCY_DEMO_DATA_ROOT, else <workspace>/hagency-desktop/.run.
resolve_demo_data_dir() {
  local here="$1" data="$2" root dir
  root="${HAGENCY_DEMO_DATA_ROOT:-$here/../../hagency-desktop/.run}"
  root="$(cd "$root" 2>/dev/null && pwd -P)" || { echo "demo data root missing: $root" >&2; return 1; }
  dir="$(cd "$data" 2>/dev/null && pwd -P)" || { echo "not a directory: $data" >&2; return 1; }
  if [ "$(dirname "$dir")" != "$root" ]; then echo "refusing $dir: not directly under $root" >&2; return 1; fi
  case "$(basename "$dir")" in demo-?*) ;; *) echo "refusing $dir: name must start with demo-" >&2; return 1;; esac
  printf '%s\n' "$dir"
}

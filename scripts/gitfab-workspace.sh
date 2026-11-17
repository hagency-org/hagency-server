#!/usr/bin/env bash
# Opens an isolated Git worktree for one GitFab task on the execution device (Annex 3 §5: work only in
# the authorised repository and on the task branch).
#
#   GITFAB_ALLOWED_REPOS="https://github.com/o/r https://gitcode.com/o/r" \
#     scripts/gitfab-workspace.sh <repository URL> <baseline SHA> agent/<project>/<task> <directory>
#
# The repository must be listed in GITFAB_ALLOWED_REPOS (space or comma separated) and its URL may not
# carry credentials; the execution device's own Git credentials are used for fetch and push. One bare
# cache per repository lives under GITFAB_WORKSPACE_CACHE (default ~/.cache/gitfab/repos), with the
# remote's branches under refs/remotes/origin so they never overwrite a task's local branch. Each task
# gets its own `git worktree`:
#   - a new task branch starts at the baseline;
#   - a task branch already on the remote is continued from there, if it contains the baseline.
# The worktree's own pre-push hook lets it push only its task branch to the authorised repository, and
# refuses deleting the branch or rewriting what is already on the remote. The hook is a local guard
# (`git push --no-verify` skips it); the platform's branch protection and a Token scoped to the
# repository are what enforce it.
#
# Prints {"dir","branch","baseline","head","repo"}. Exit 0 ready, 1 refused (repository not allowed,
# baseline missing, branch not based on the baseline), 2 usage or setup error.
set -euo pipefail

usage() {
    echo "usage: GITFAB_ALLOWED_REPOS=<urls> $0 <repository URL> <baseline SHA> agent/<project>/<task> <directory>" >&2
    exit 2
}
refuse() {
    echo "refused: $*" >&2
    exit 1
}
[ $# -eq 4 ] || usage
url=$1 baseline=$2 branch=$3 dir=$4

normalize() {
    local u=${1%/}
    printf '%s' "${u%.git}"
}
case "$url" in
    *://*@*) refuse "the repository URL carries credentials; configure them in Git on this device instead" ;;
esac
[[ "$baseline" =~ ^[0-9a-f]{40}$ ]] || { echo "baseline must be a full 40-character commit SHA" >&2; exit 2; }
seg='[A-Za-z0-9][A-Za-z0-9._-]{0,63}'
if ! [[ "$branch" =~ ^agent/$seg/$seg$ ]] || [[ "$branch" == *..* ]] || [[ "$branch" == *.lock ]]; then
    echo "branch must be agent/<project>/<task>" >&2
    exit 2
fi
[ -n "${GITFAB_ALLOWED_REPOS:-}" ] || { echo "GITFAB_ALLOWED_REPOS is not set" >&2; exit 2; }
want=$(normalize "$url")
allowed=""
for candidate in ${GITFAB_ALLOWED_REPOS//,/ }; do
    [ "$(normalize "$candidate")" = "$want" ] && allowed=1
done
[ -n "$allowed" ] || refuse "$want is not in GITFAB_ALLOWED_REPOS"
if [ -e "$dir" ] && [ -n "$(ls -A "$dir" 2>/dev/null)" ]; then
    echo "$dir exists and is not empty" >&2
    exit 2
fi

cache_root=${GITFAB_WORKSPACE_CACHE:-$HOME/.cache/gitfab/repos}
key=$(printf '%s' "$want" | git hash-object --stdin)
cache=$cache_root/$key.git
if [ ! -d "$cache" ]; then
    mkdir -p "$cache_root"
    git init --quiet --bare "$cache"
    git -C "$cache" remote add origin "$url"
    git -C "$cache" config remote.origin.fetch '+refs/heads/*:refs/remotes/origin/*'
    # Per-worktree settings (the push guard) need worktreeConfig; core.bare must then be the cache's
    # own setting, or every worktree would inherit it and refuse to commit.
    git -C "$cache" config extensions.worktreeConfig true
    git -C "$cache" config --unset core.bare
    git -C "$cache" config --worktree core.bare true
fi
[ "$(normalize "$(git -C "$cache" remote get-url origin)")" = "$want" ] || { echo "cache $cache points elsewhere" >&2; exit 2; }
git -C "$cache" fetch --quiet --prune origin
git -C "$cache" cat-file -e "$baseline^{commit}" 2>/dev/null || refuse "baseline $baseline is not in $want"

remote_ref=refs/remotes/origin/$branch
if git -C "$cache" show-ref --verify --quiet "$remote_ref"; then
    git -C "$cache" merge-base --is-ancestor "$baseline" "$remote_ref" \
        || refuse "$branch on the remote does not contain the baseline $baseline"
    start=$remote_ref
else
    start=$baseline
fi
git -C "$cache" show-ref --verify --quiet "refs/heads/$branch" \
    && refuse "$branch is already checked out from this cache; finish or remove that worktree first"
git -C "$cache" worktree add --quiet -b "$branch" "$dir" "$start"
dir=$(cd "$dir" && pwd)

hooks=$cache/gitfab-hooks/$(printf '%s' "$branch" | git hash-object --stdin)
mkdir -p "$hooks"
cat >"$hooks/pre-push" <<HOOK
#!/usr/bin/env bash
# GitFab task guard: push only $branch to $want, never delete it or rewrite what the remote has.
set -euo pipefail
normalize() { local u=\${1%/}; printf '%s' "\${u%.git}"; }
[ "\$(normalize "\$2")" = "$want" ] || { echo "gitfab: this worktree pushes only to $want" >&2; exit 1; }
zero=0000000000000000000000000000000000000000
while read -r local_ref local_sha remote_ref remote_sha; do
    if [ "\$remote_ref" != "refs/heads/$branch" ]; then
        echo "gitfab: this worktree pushes only $branch, not \${remote_ref#refs/heads/}" >&2; exit 1
    fi
    if [ "\$local_sha" = "\$zero" ]; then
        echo "gitfab: deleting $branch is not allowed" >&2; exit 1
    fi
    if [ "\$remote_sha" != "\$zero" ] && ! git merge-base --is-ancestor "\$remote_sha" "\$local_sha" 2>/dev/null; then
        echo "gitfab: rewriting $branch on the remote is not allowed; add commits instead" >&2; exit 1
    fi
done
HOOK
chmod 755 "$hooks/pre-push"
git -C "$dir" config --worktree core.hooksPath "$hooks"
git -C "$dir" config --worktree push.default upstream
git -C "$dir" config --worktree "branch.$branch.remote" origin
git -C "$dir" config --worktree "branch.$branch.merge" "refs/heads/$branch"

printf '{"dir":"%s","branch":"%s","baseline":"%s","head":"%s","repo":"%s"}\n' \
    "$dir" "$branch" "$baseline" "$(git -C "$dir" rev-parse HEAD)" "$want"

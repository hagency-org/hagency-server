#!/usr/bin/env bash
# Checks scripts/gitfab-workspace.sh against throwaway local repositories: only the allowed repository
# and a well-formed task branch are accepted, the worktree starts at the baseline (or continues the task
# branch already pushed), and the worktree's guard lets it push only its own branch, never deleting or
# rewriting it. Touches nothing outside a temporary directory.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
ws=$here/gitfab-workspace.sh
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
export GITFAB_WORKSPACE_CACHE=$tmp/cache HOME=$tmp/home GIT_CONFIG_NOSYSTEM=1
mkdir -p "$HOME"
git config --global user.name gitfab-test
git config --global user.email gitfab-test@localhost
git config --global init.defaultBranch main
failures=0
expect() {
    local want=$1 name=$2 got
    shift 2
    set +e
    "$@" >"$tmp/out" 2>"$tmp/err"
    got=$?
    set -e
    if [ "$got" = "$want" ]; then
        echo "ok   $name (exit $got)"
    else
        echo "FAIL $name: exit $got, expected $want"
        sed 's/^/     /' "$tmp/err"
        failures=$((failures + 1))
    fi
}
check() {
    if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; failures=$((failures + 1)); fi
}

# An "authorised" remote with one commit on main, and another repository that is not authorised.
git init --quiet --bare "$tmp/remote.git"
git init --quiet --bare "$tmp/other.git"
git clone --quiet "$tmp/remote.git" "$tmp/seed" 2>/dev/null
echo base >"$tmp/seed/README.md"
git -C "$tmp/seed" add README.md
git -C "$tmp/seed" commit --quiet -m base
git -C "$tmp/seed" push --quiet origin main
base=$(git -C "$tmp/seed" rev-parse HEAD)
url=file://$tmp/remote.git
other=file://$tmp/other.git
export GITFAB_ALLOWED_REPOS="$url/, https://example.invalid/x"
branch=agent/DEMO-02/task-1

expect 1 "a repository not in the list is refused" "$ws" "$other" "$base" "$branch" "$tmp/w0"
expect 1 "credentials in the URL are refused" "$ws" "https://user:secret@example.invalid/x" "$base" "$branch" "$tmp/w0"
expect 2 "a branch outside agent/<project>/<task> is a usage error" "$ws" "$url" "$base" main "$tmp/w0"
expect 2 "a short baseline is a usage error" "$ws" "$url" "${base:0:12}" "$branch" "$tmp/w0"
expect 1 "a baseline the repository lacks is refused" "$ws" "$url" "$(printf '%040d' 0 | tr 0 a)" "$branch" "$tmp/w0"
GITFAB_ALLOWED_REPOS= expect 2 "no allow list is a setup error" "$ws" "$url" "$base" "$branch" "$tmp/w0"

expect 0 "the task worktree opens at the baseline" "$ws" "$url" "$base" "$branch" "$tmp/w1"
w1=$tmp/w1
check "it is on the task branch" '[ "$(git -C "$w1" branch --show-current)" = "$branch" ]'
check "it starts at the baseline" '[ "$(git -C "$w1" rev-parse HEAD)" = "$base" ]'
check "the report names the worktree" 'grep -q "\"branch\":\"$branch\"" "$tmp/out"'

echo change >>"$w1/README.md"
git -C "$w1" commit --quiet -am change
expect 0 "pushing the task branch works" git -C "$w1" push --quiet origin "$branch"
check "the remote has the task commit" '[ "$(git --git-dir="$tmp/remote.git" rev-parse "$branch")" = "$(git -C "$w1" rev-parse HEAD)" ]'
expect 0 "a plain git push goes to the task branch" bash -c "echo more >>'$w1/README.md' && git -C '$w1' commit --quiet -am more && git -C '$w1' push --quiet"
expect 1 "pushing to main is refused" git -C "$w1" push origin HEAD:main
expect 1 "pushing another branch is refused" git -C "$w1" push origin HEAD:refs/heads/agent/DEMO-02/other
expect 1 "deleting the task branch is refused" git -C "$w1" push origin ":$branch"
git -C "$w1" reset --quiet --hard "$base"
echo rewritten >>"$w1/README.md"
git -C "$w1" commit --quiet -am rewritten
expect 1 "rewriting the pushed branch is refused" git -C "$w1" push --force origin "$branch"
expect 1 "pushing to another repository is refused" git -C "$w1" push "$other" "HEAD:$branch"
check "main on the remote is untouched" '[ "$(git --git-dir="$tmp/remote.git" rev-parse main)" = "$base" ]'
check "the hook is not part of the worktree" '[ -z "$(git -C "$w1" status --porcelain --untracked-files=all)" ]'

expect 1 "the same branch twice from one cache is refused" "$ws" "$url" "$base" "$branch" "$tmp/w2"
# Another device (a fresh cache) continues the branch already on the remote.
GITFAB_WORKSPACE_CACHE=$tmp/cache2 expect 0 "a task branch already pushed is continued" "$ws" "$url" "$base" "$branch" "$tmp/w3"
check "it continues from the remote branch" '[ "$(git -C "$tmp/w3" rev-parse HEAD)" = "$(git --git-dir="$tmp/remote.git" rev-parse "$branch")" ]'
expect 2 "a non-empty directory is not reused" "$ws" "$url" "$base" agent/DEMO-02/task-2 "$tmp/w1"

# A repository address with shell syntax in it is a value in the generated hook, never code.
odd="$tmp/odd\$(touch\${IFS}PWNED)\"q\".git"
git clone --quiet --bare "$tmp/remote.git" "$odd" 2>/dev/null
GITFAB_ALLOWED_REPOS="file://$odd" expect 0 "a repository address with shell syntax opens" "$ws" "file://$odd" "$base" agent/DEMO-02/odd "$tmp/w4"
expect 0 "pushing from it works" git -C "$tmp/w4" push --quiet origin agent/DEMO-02/odd
check "nothing in the address ran" '[ -z "$(find "$tmp" -name PWNED)" ]'

if [ "$failures" -ne 0 ]; then
    echo "$failures check(s) failed"
    exit 1
fi
echo "gitfab-workspace checks passed"

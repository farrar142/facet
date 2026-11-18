#!/usr/bin/env bash
# Tests of .github/factory/merge-check.sh (main must be on the branch before the push, #434)
# against throwaway git repositories.
set -u
here=$(cd "$(dirname "$0")" && pwd)
script="$here/../merge-check.sh"
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@t
root=$(mktemp -d)
trap 'rm -r "$root"' EXIT
passes=0 fails=0 current=
ok() { passes=$((passes + 1)); }
bad() { echo "  FAIL [$current] $*"; fails=$((fails + 1)); }
eq() { if [ "$2" = "$3" ]; then ok; else bad "$1: expected [$2], got [$3]"; fi; }
has() { case "$3" in *"$2"*) ok ;; *) bad "$1: [$2] not in [$3]" ;; esac; }

# setup <name>: main and a PR branch that change the same line of a.txt differently; the PR branch
# is checked out, as after "Prepare branch" in conflict mode.
setup() {
  current=$1
  repo=$(mktemp -d "$root/r.XXXXXX")
  export GITHUB_OUTPUT="$repo.out"; : > "$GITHUB_OUTPUT"
  git init -q "$repo"
  cd "$repo" || exit 1
  git config core.autocrlf false
  echo base > a.txt; git add a.txt; git commit -q -m base; git branch -M main
  git switch -q -c pr
  echo ticket > a.txt; git commit -q -am ticket
  git switch -q main
  echo main > a.txt; git commit -q -am main
  main=$(git rev-parse HEAD)
  git switch -q pr
}
check() { out=$(bash "$script" "$main" 2>&1); code=$?; }

setup "a conflict-mode run that left the branch still conflicting with main is not a success"
# "Rebase onto main" merged and stopped on the conflict; the resolving agent did nothing.
git merge -q --no-edit main > /dev/null 2>&1
check
eq exit 1 "$code"
has reason "충돌을 풀지 않고 끝남" "$out"
has "unmerged file" "a.txt" "$out"
has "reason output" "reason=충돌을 풀지 않고 끝남" "$(cat "$GITHUB_OUTPUT")"

setup "no merge: the agent aborted it and the branch is the PR's old head"
git merge -q --no-edit main > /dev/null 2>&1; git merge --abort
check
eq exit 1 "$code"
has reason "main($(git rev-parse --short "$main"))이 들어가지 않았다" "$out"

setup "merge in progress with the conflict resolved but not committed"
git merge -q --no-edit main > /dev/null 2>&1
echo both > a.txt; git add a.txt
check
eq exit 1 "$code"
has reason "MERGE_HEAD" "$out"

setup "unmerged file left after the merge state is gone"
git merge -q --no-edit main > /dev/null 2>&1
rm .git/MERGE_HEAD
check
eq exit 1 "$code"
has reason "풀리지 않은 파일" "$out"

setup "rebase stopped on the conflict"
git rebase main > /dev/null 2>&1
check
eq exit 1 "$code"
has reason "rebase" "$out"

setup "conflict resolved and committed"
git merge -q --no-edit main > /dev/null 2>&1
echo both > a.txt; git add a.txt; git commit -q --no-edit
check
eq exit 0 "$code"
eq "no reason" "" "$(cat "$GITHUB_OUTPUT")"

setup "the ticket's commits rebased onto main (normal mode)"
git reset -q --hard "$main"; echo other > b.txt; git add b.txt; git commit -q -m b
check
eq exit 0 "$code"

setup "main moved after the rebase step: the recorded commit is what counts"
git merge -q --no-edit main > /dev/null 2>&1
echo both > a.txt; git add a.txt; git commit -q --no-edit
git switch -q main; echo later > d.txt; git add d.txt; git commit -q -m later; git switch -q pr
check
eq exit 0 "$code"

cd "$root" || exit 1
echo "merge-check: $passes passed, $fails failed"
[ "$fails" -eq 0 ]

#!/usr/bin/env bash
# Checks that main really landed on the branch before the Implementer pushes it (#434).
#
#   merge-check.sh <main commit>
#
# Run in the repository after "Rebase onto main" and "Resolve conflicts" (factory-implementer.yml,
# "Require main merged"). The branch is fine when no merge or rebase is still in progress, no file is
# left unmerged, and HEAD contains <main commit> (the main the rebase step fetched: a later fetch by
# the agent must not move the target). Otherwise it prints the reason and exits 1, and the run is
# recorded as an Implementer failure instead of going to Verify: a conflict-mode run whose agent
# ended without merging main (runs 44389000388, 44393130003 for #342) pushed the PR's old head
# ("Everything up-to-date"), and Verify sent it back for the same conflict until the limit ran out.
set -u
[ $# -eq 1 ] || { echo 'usage: merge-check.sh <main commit>' >&2; exit 2; }
main=$1

# merge_check <main commit>: prints why main is not on the branch, or nothing; returns 1 or 0.
merge_check() {
  local gitdir unmerged
  gitdir=$(git rev-parse --git-dir) || { echo "not a git repository"; return 1; }
  if git rev-parse -q --verify MERGE_HEAD > /dev/null; then
    unmerged=$(git diff --name-only --diff-filter=U | tr '\n' ' ')
    echo "충돌을 풀지 않고 끝남: merge가 끝나지 않았다(MERGE_HEAD 남음; 풀리지 않은 파일: ${unmerged:-없음})"
    return 1
  fi
  if [ -d "$gitdir/rebase-merge" ] || [ -d "$gitdir/rebase-apply" ]; then
    echo "충돌을 풀지 않고 끝남: rebase가 끝나지 않았다"
    return 1
  fi
  unmerged=$(git diff --name-only --diff-filter=U | tr '\n' ' ')
  if [ -n "$unmerged" ]; then
    echo "충돌을 풀지 않고 끝남: 풀리지 않은 파일이 남았다: $unmerged"
    return 1
  fi
  if ! git merge-base --is-ancestor "$main" HEAD; then
    echo "충돌을 풀지 않고 끝남: 브랜치(HEAD $(git rev-parse --short HEAD))에 main($(git rev-parse --short "$main"))이 들어가지 않았다"
    return 1
  fi
}

if reason=$(merge_check "$main"); then
  echo "main $(git rev-parse --short "$main") is on the branch"
  exit 0
fi
echo "::error::$reason"
# Record failure (factory-implementer.yml) puts this in the failure comment.
[ -n "${GITHUB_OUTPUT:-}" ] && printf 'reason=%s\n' "$reason" >> "$GITHUB_OUTPUT"
exit 1

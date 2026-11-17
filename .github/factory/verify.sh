#!/usr/bin/env bash
# The scripted half of the factory Verify run (factory-verify.yml, spec #249 "Verify").
#
#   verify.sh mech <N> <base>            machine checks of ticket #N's branch (the checkout's HEAD)
#                                        against <base> (origin/main). Prints every problem and
#                                        exits 1 when there is one:
#                                        1. no `#[ignore = "#N"]` is left in any tracked *.rs file;
#                                        2. the branch's change to the acceptance tests
#                                           (crates/*/tests/spec_*.rs, `git diff <base>...HEAD`)
#                                           only deletes `#[ignore = "#N"]` lines. Any other added,
#                                           deleted or changed line (an assertion, a removed test,
#                                           another ticket's ignore) is "the acceptance test changed";
#                                        3. no `todo!("#N")` stub (Odin's placeholder for a name of
#                                           ticket #N's contract) is left in any tracked *.rs file.
#   verify.sh sendback <N> <PR> <file>   send ticket #N back: comment `<!-- factory-sendback -->` and the
#                                        reason (<file>) on the ticket, close the PR, delete its
#                                        branch and its job-less bot ci.yml runs (prune-runs.sh), drop `in-progress`. On the ticket's second send-back
#                                        hand it to a human (`ready-for-human`, `<!-- factory-human -->`)
#                                        and move the chain on (dispatch-next.sh); otherwise label it
#                                        `ready-for-agent` again and dispatch factory-implementer.yml.
#   verify.sh pass <N> <PR> [report]     squash-merge the PR (retrying while GitHub computes
#                                        mergeability), delete its branch and its job-less bot
#                                        ci.yml runs (prune-runs.sh), comment the report on the
#                                        PR, close the ticket, dispatch ci.yml and release.yml on main
#                                        and then the next ticket (dispatch-next.sh). Exits 1 when the
#                                        PR could not be merged (nothing else is done then).
#
# Environment: REPO (or GITHUB_REPOSITORY), GH_TOKEN; RUN_URL (link in comments, optional);
# FACTORY_DISPATCH_NEXT (command run instead of dispatch-next.sh, tests); FACTORY_PRUNE_RUNS (command
# run with the branch instead of prune-runs.sh, tests); FACTORY_MERGE_WAIT
# (seconds between merge attempts, default 15). Needs bash, git, gh and jq.
set -u
here=$(cd "$(dirname "$0")" && pwd)
REPO=${REPO:-${GITHUB_REPOSITORY:-}}
SENDBACK='<!-- factory-sendback -->'
HUMAN='<!-- factory-human -->'
SENDBACK_LIMIT=2

j() { jq "$@" | tr -d '\r'; }

usage() {
  echo "usage: verify.sh mech <N> <base> | sendback <N> <PR> <reason file> | pass <N> <PR> [report file]" >&2
  exit 2
}

# --- mech ----------------------------------------------------------------------------------------

mech() {
  local n=$1 base=$2 attr stub problems=0 left line file
  attr="#[ignore = \"#$n\"]"
  stub="todo!(\"#$n\")"

  left=$(git grep -n -F "$attr" HEAD -- '*.rs' | sed 's/^HEAD://')
  if [ -n "$left" ]; then
    echo "- \`$attr\`이 남아 있다(이 티켓의 인수 시험을 모두 켜야 한다):"
    printf '%s\n' "$left" | sed 's/^/  - /'
    problems=1
  fi

  # Odin stubs the contract names the acceptance tests use with todo!("#N") (odin.md); the ticket
  # replaces every one of them with the real thing.
  left=$(git grep -n -F "$stub" HEAD -- '*.rs' | sed 's/^HEAD://')
  if [ -n "$left" ]; then
    echo "- \`$stub\` 스텁이 남아 있다(계약의 이름을 실제로 구현해야 한다):"
    printf '%s\n' "$left" | sed 's/^/  - /'
    problems=1
  fi

  # Only deletions of this ticket's ignore lines may touch the acceptance tests.
  file=
  while IFS= read -r line; do
    line=${line%$'\r'}
    case "$line" in
      'diff --git '*) file=${line##* b/} ;;
      '+++ '* | '--- '*) ;;
      +*)
        echo "- 인수 시험을 고쳤다(줄을 더함) $file: \`${line#+}\`"
        problems=1 ;;
      -*)
        local body=${line#-}
        local trimmed
        trimmed=$(printf '%s' "$body" | sed 's/^[[:space:]]*//; s/[[:space:]]*$//')
        if [ "$trimmed" = "$attr" ]; then
          :
        elif printf '%s' "$trimmed" | grep -Eq '^#\[ignore = "#[0-9]+"\]$'; then
          echo "- 다른 티켓의 인수 시험을 켰다 $file: \`$trimmed\`"
          problems=1
        else
          echo "- 인수 시험을 고쳤다(줄을 지움) $file: \`$body\`"
          problems=1
        fi ;;
    esac
  done < <(git diff --unified=0 --no-color --no-ext-diff --no-renames "$base"...HEAD -- 'crates/*/tests/spec_*.rs')

  if [ "$problems" -ne 0 ]; then return 1; fi
  echo "기계 검사 통과: \`$attr\`·\`$stub\` 없음, 인수 시험의 변경은 이 티켓의 ignore 줄 지우기뿐"
}

# --- GitHub helpers ------------------------------------------------------------------------------

comment() { gh api "repos/$REPO/issues/$1/comments" -f "body=$2" >/dev/null; }
add_label() { gh api "repos/$REPO/issues/$1/labels" -f "labels[]=$2" >/dev/null; }
remove_label() { gh api -X DELETE "repos/$REPO/issues/$1/labels/$2" >/dev/null 2>&1 || true; }
dispatch() { # dispatch <workflow file> [<input>=<value> ...]
  local wf=$1; shift
  local args=() kv
  for kv in "$@"; do args+=(-f "inputs[${kv%%=*}]=${kv#*=}"); done
  gh api -X POST "repos/$REPO/actions/workflows/$wf/dispatches" -f ref=main "${args[@]}" >/dev/null
}
# Delete the job-less ci.yml pull_request runs GitHub left on a factory PR branch (prune-runs.sh, #270).
prune_runs() {
  [ -n "$1" ] || return 0
  if [ -n "${FACTORY_PRUNE_RUNS:-}" ]; then $FACTORY_PRUNE_RUNS "$1"; else bash "$here/prune-runs.sh" "$1"; fi
}
next_ticket() {
  if [ -n "${FACTORY_DISPATCH_NEXT:-}" ]; then
    $FACTORY_DISPATCH_NEXT
  else
    bash "$here/dispatch-next.sh"
  fi
}
pr_branch() { gh api "repos/$REPO/pulls/$1" | j -r '.head.ref // empty'; }
delete_branch() {
  [ -n "$1" ] || return 0
  gh api -X DELETE "repos/$REPO/git/refs/heads/$1" >/dev/null 2>&1 || echo "::warning::could not delete branch $1"
}
sendback_count() {
  gh api "repos/$REPO/issues/$1/comments?per_page=100" --paginate \
    | j -s --arg m "$SENDBACK" '[.[][] | select(.body | contains($m))] | length'
}
run_link() { [ -n "${RUN_URL:-}" ] && printf ' (%s)' "$RUN_URL"; }

# --- sendback ------------------------------------------------------------------------------------

sendback() {
  local n=$1 pr=$2 file=$3 reason branch count
  reason=$(cat "$file" 2>/dev/null)
  [ -n "$reason" ] || reason="(사유가 남지 않았다)"
  branch=$(pr_branch "$pr")

  comment "$n" "$SENDBACK
Verify가 PR #$pr을 되돌렸다$(run_link).

$reason"
  gh api -X PATCH "repos/$REPO/pulls/$pr" -f state=closed >/dev/null || echo "::warning::could not close PR #$pr"
  delete_branch "$branch"
  prune_runs "$branch"
  remove_label "$n" in-progress

  count=$(sendback_count "$n")
  if [ "${count:-0}" -ge "$SENDBACK_LIMIT" ]; then
    add_label "$n" ready-for-human
    comment "$n" "$HUMAN
Verify가 이 티켓을 ${count}번 되돌렸다. 공장은 더 돌리지 않는다: 사람이 위 되돌림 사유를 보고 티켓이나 인수 시험을 고친 뒤 \`ready-for-human\`을 떼고 \`ready-for-agent\`를 붙인다."
    echo "#$n: send-back $count, handed to a human"
    next_ticket
  else
    add_label "$n" ready-for-agent
    dispatch factory-implementer.yml "issue=$n"
    echo "#$n: send-back $count, dispatched the Implementer again"
  fi
}

# --- pass ----------------------------------------------------------------------------------------

pass() {
  local n=$1 pr=$2 report=${3:-} branch attempt merged=false
  branch=$(pr_branch "$pr")
  for attempt in 1 2 3 4; do
    if gh api -X PUT "repos/$REPO/pulls/$pr/merge" -f merge_method=squash >/dev/null; then
      merged=true; break
    fi
    echo "::notice::merge attempt $attempt of PR #$pr failed; GitHub may still be computing mergeability"
    sleep "${FACTORY_MERGE_WAIT:-15}"
  done
  if ! $merged; then
    echo "::error::could not merge PR #$pr"
    return 1
  fi
  delete_branch "$branch"
  prune_runs "$branch"
  if [ -n "$report" ] && [ -s "$report" ]; then
    gh api "repos/$REPO/issues/$pr/comments" -f "body=Verify$(run_link): 통과

$(cat "$report")" >/dev/null || true
  fi
  # A GITHUB_TOKEN merge does not reliably close the ticket via `Closes #N`, and fires no
  # `issues: closed` or push event: close it, build main and move the chain on here.
  comment "$n" "Merged in PR #$pr$(run_link)" || true
  gh api -X PATCH "repos/$REPO/issues/$n" -f state=closed -f state_reason=completed >/dev/null \
    || echo "::warning::could not close #$n"
  remove_label "$n" in-progress
  dispatch ci.yml || echo "::warning::could not dispatch ci.yml"
  dispatch release.yml || echo "::warning::could not dispatch release.yml"
  next_ticket
}

case "${1:-}" in
  mech) [ $# -eq 3 ] || usage; mech "$2" "$3" ;;
  sendback) [ $# -eq 4 ] || usage; : "${REPO:?REPO is not set}"; sendback "$2" "$3" "$4" ;;
  pass) [ $# -ge 3 ] || usage; : "${REPO:?REPO is not set}"; pass "$2" "$3" "${4:-}" ;;
  *) usage ;;
esac

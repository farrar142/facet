#!/usr/bin/env bash
# The scripted half of the factory Odin run (factory-odin.yml, spec #249 "Odin").
#
#   odin.sh state <S>                   what the Odin run of spec #S has to do. Writes to $GITHUB_OUTPUT
#                                       (stdout when unset) mode=, pr= and branch=:
#                                         mode=none    no open spec PR (head factory/spec-<S>): warn and stop
#                                         mode=empty   the spec PR changes no file (Decompose wrote no test):
#                                                      no agent, `odin.sh empty`
#                                         mode=review  the agent reviews the spec PR
#   odin.sh verdict <S> [file]          after the agent: verdict=question when the spec has `needs-decision`
#                                       or the verdict file (default .factory/odin-verdict) says `question`;
#                                       verdict=go for `pass` / `fixed`; verdict=missing otherwise.
#   odin.sh question <S> <PR> [report]  the agent asked: comment the report on the spec PR. Nothing is
#                                       merged and no ticket is labelled; the run resumes when a person
#                                       answers and removes `needs-decision`.
#   odin.sh human <S> <PR> <reason>     hand the spec to a human: comment the reason (a file) on the spec
#                                       PR, label the spec `ready-for-human` and comment
#                                       `<!-- factory-human -->` with the reason on it. Nothing is merged.
#   odin.sh pass <S> <PR> [report]      the checks passed and the workflow pushed Odin's fixes: comment the
#                                       report on the spec PR, squash-merge it (retrying while GitHub
#                                       computes mergeability), delete its branch, build main, label the
#                                       spec's open tickets `ready-for-agent` and dispatch the first one
#                                       (dispatch-next.sh --tickets-only: the lowest-numbered approved
#                                       ticket with no open blocker). Exits 1 when the PR could not be
#                                       merged (nothing else is done then).
#   odin.sh empty <S> <PR>              the spec PR has no test: close it and delete its branch, then
#                                       label the tickets and dispatch the first one as `pass` does.
#
# Environment: REPO (or GITHUB_REPOSITORY), GH_TOKEN, BASE (default main); RUN_URL (link in comments,
# optional); FACTORY_DISPATCH_NEXT (command run instead of dispatch-next.sh --tickets-only, tests);
# FACTORY_MERGE_WAIT (seconds between merge attempts, default 15). Needs bash, gh and jq.
set -u
here=$(cd "$(dirname "$0")" && pwd)
REPO=${REPO:-${GITHUB_REPOSITORY:-}}
BASE=${BASE:-main}
HUMAN='<!-- factory-human -->'

j() { jq "$@" | tr -d '\r'; }

output() {
  if [ -n "${GITHUB_OUTPUT:-}" ]; then echo "$1" >> "$GITHUB_OUTPUT"; else echo "$1"; fi
}

usage() {
  echo "usage: odin.sh state <S> | verdict <S> [file] | question <S> <PR> [report] | human <S> <PR> <reason> | pass <S> <PR> [report] | empty <S> <PR>" >&2
  exit 2
}

branch_of() { echo "factory/spec-$1"; }
run_link() { [ -n "${RUN_URL:-}" ] && printf ' (%s)' "$RUN_URL"; }
comment() { gh api "repos/$REPO/issues/$1/comments" -f "body=$2" > /dev/null; }
add_label() { gh api "repos/$REPO/issues/$1/labels" -f "labels[]=$2" > /dev/null; }
has_label() { # has_label <issue> <label>
  [ "$(gh api "repos/$REPO/issues/$1" | j -r --arg l "$2" 'any(.labels[]; .name == $l)')" = true ]
}
delete_branch() {
  gh api -X DELETE "repos/$REPO/git/refs/heads/$1" > /dev/null 2>&1 \
    || echo "::warning::could not delete branch $1"
}
dispatch() { # dispatch <workflow file>
  gh api -X POST "repos/$REPO/actions/workflows/$1/dispatches" -f "ref=$BASE" > /dev/null
}

# Number of the open spec PR of a spec, empty when there is none (as decompose.sh finds it).
spec_pr() {
  gh api "repos/$REPO/pulls?state=open&per_page=100" --paginate \
    | j -s -r --arg b "$(branch_of "$1")" \
      'add // [] | map(select(.state == "open" and .head.ref == $b)) | sort_by(.number) | .[0].number // empty'
}

# Open sub-issues of a spec, ascending.
open_tickets() {
  gh api "repos/$REPO/issues/$1/sub_issues?per_page=100" --paginate \
    | j -s -r 'add // [] | map(select(.state == "open") | .number) | sort | .[]'
}

# --- state ---------------------------------------------------------------------------------------

state() {
  local s=$1 pr files
  pr=$(spec_pr "$s")
  output "branch=$(branch_of "$s")"
  if [ -z "$pr" ]; then
    echo "::warning::#$s has no open spec PR ($(branch_of "$s")); nothing for Odin to review"
    output mode=none
    return 0
  fi
  output "pr=$pr"
  files=$(gh api "repos/$REPO/pulls/$pr" | j -r '.changed_files // 0')
  if [ "${files:-0}" -eq 0 ]; then
    echo "::notice::spec PR #$pr changes no file; the tickets start without acceptance tests"
    output mode=empty
  else
    output mode=review
  fi
}

# --- verdict -------------------------------------------------------------------------------------

verdict() {
  local s=$1 file=${2:-.factory/odin-verdict} v
  v=$(tr -d '[:space:]' < "$file" 2> /dev/null || true)
  if has_label "$s" needs-decision || [ "$v" = question ]; then
    output verdict=question
  elif [ "$v" = pass ] || [ "$v" = fixed ]; then
    output verdict=go
  else
    echo "::warning::Odin left no verdict (${v:-nothing})"
    output verdict=missing
  fi
}

# --- question / human ----------------------------------------------------------------------------

question() {
  local s=$1 pr=$2 report=${3:-}
  if [ -n "$report" ] && [ -s "$report" ]; then
    comment "$pr" "Odin$(run_link): 질문으로 멈춤(#$s의 \`needs-decision\`)

$(cat "$report")" || echo "::warning::could not comment on PR #$pr"
  fi
  echo "::notice::#$s waits for an answer (needs-decision); spec PR #$pr stays open"
}

human() {
  local s=$1 pr=$2 file=$3 reason
  reason=$(cat "$file" 2> /dev/null)
  [ -n "$reason" ] || reason="(까닭이 남지 않았다)"
  comment "$pr" "Odin$(run_link): 병합하지 않음

$reason" || echo "::warning::could not comment on PR #$pr"
  add_label "$s" ready-for-human || echo "::warning::could not label #$s ready-for-human"
  comment "$s" "$HUMAN
Odin이 스펙 PR #$pr을 병합하지 못했다$(run_link). 공장은 이 spec을 더 돌리지 않는다: 사람이 아래 까닭을 보고 스펙 PR을 고친 뒤 \`ready-for-human\`을 떼고 Odin을 다시 돌린다(\`factory-odin.yml\`, 입력 spec=$s).

$reason"
  echo "#$s: handed to a human"
}

# --- approving the tickets -----------------------------------------------------------------------

start_chain() {
  local s=$1 n tickets
  tickets=$(open_tickets "$s")
  if [ -z "$tickets" ]; then
    echo "::warning::#$s has no open tickets to approve"
    return 0
  fi
  for n in $tickets; do
    add_label "$n" ready-for-agent || echo "::warning::could not label #$n ready-for-agent"
  done
  echo "approved: $(echo $tickets)"
  if [ -n "${FACTORY_DISPATCH_NEXT:-}" ]; then
    $FACTORY_DISPATCH_NEXT
  else
    bash "$here/dispatch-next.sh" --tickets-only
  fi
}

# --- pass ----------------------------------------------------------------------------------------

pass() {
  local s=$1 pr=$2 report=${3:-} attempt merged=false
  if [ -n "$report" ] && [ -s "$report" ]; then
    comment "$pr" "Odin$(run_link): 통과

$(cat "$report")" || echo "::warning::could not comment on PR #$pr"
  fi
  for attempt in 1 2 3 4; do
    if gh api -X PUT "repos/$REPO/pulls/$pr/merge" -f merge_method=squash > /dev/null; then
      merged=true
      break
    fi
    echo "::notice::merge attempt $attempt of spec PR #$pr failed; GitHub may still be computing mergeability"
    sleep "${FACTORY_MERGE_WAIT:-15}"
  done
  if ! $merged; then
    echo "::error::could not merge spec PR #$pr"
    return 1
  fi
  delete_branch "$(branch_of "$s")"
  # A GITHUB_TOKEN merge fires no push event: build main here, as verify.sh does.
  dispatch ci.yml || echo "::warning::could not dispatch ci.yml"
  start_chain "$s"
}

# --- empty ---------------------------------------------------------------------------------------

empty() {
  local s=$1 pr=$2
  comment "$pr" "Odin$(run_link): 이 스펙 PR은 바꾼 파일이 없다(인수 시험 0개). 닫고 티켓을 인수 시험 없이 시작한다." \
    || echo "::warning::could not comment on PR #$pr"
  gh api -X PATCH "repos/$REPO/pulls/$pr" -f state=closed > /dev/null || echo "::warning::could not close PR #$pr"
  delete_branch "$(branch_of "$s")"
  start_chain "$s"
}

: "${REPO:?REPO is not set}"
case "${1:-}" in
  state) [ $# -eq 2 ] || usage; state "$2" ;;
  verdict) [ $# -ge 2 ] || usage; verdict "$2" "${3:-}" ;;
  question) [ $# -ge 3 ] || usage; question "$2" "$3" "${4:-}" ;;
  human) [ $# -eq 4 ] || usage; human "$2" "$3" "$4" ;;
  pass) [ $# -ge 3 ] || usage; pass "$2" "$3" "${4:-}" ;;
  empty) [ $# -eq 3 ] || usage; empty "$2" "$3" ;;
  *) usage ;;
esac

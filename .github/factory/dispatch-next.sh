#!/usr/bin/env bash
# Keeps the factory chain going (factory-implementer.yml: the end of every implement run, the
# frontier job and the daily schedule after KST midnight).
#
#   dispatch-next.sh [--tickets-only]
#
# 1. Dispatches the lowest-numbered open `ready-for-agent` ticket with no open blocker whose
#    budget guard (ledger.sh guard) says go. A ticket whose spec is paused or needs a decision is
#    passed over; when the daily budget is spent nothing starts (the schedule resumes after KST
#    midnight).
# 2. With no ticket to start (and without --tickets-only): closes finished specs, resumes a spec
#    whose decompose stopped (no sub-issues and no decompose run going), or labels the next spec of
#    SPEC_QUEUE and dispatches its decompose (decision 2026-11-15, roadmap order).
#
# Environment: REPO, GH_TOKEN, SPEC_QUEUE (spec numbers in order), and the budget variables of
# ledger.sh.
set -u
here=$(cd "$(dirname "$0")" && pwd)
: "${REPO:?REPO is not set}"
tickets_only=false
[ "${1:-}" = --tickets-only ] && tickets_only=true

# go <issue>: run the budget guard; prints its reason and succeeds when it may start.
go() {
  local out; out=$(mktemp)
  GITHUB_OUTPUT=$out bash "$here/ledger.sh" guard "$1" > /dev/null
  local g r
  g=$(sed -n 's/^go=//p' "$out" | tail -n 1)
  r=$(sed -n 's/^reason=//p' "$out" | tail -n 1)
  rm -f "$out"
  echo "$r"
  [ "$g" = true ]
}

for n in $(gh issue list --repo "$REPO" --state open --label ready-for-agent --limit 200 --json number --jq 'sort_by(.number) | .[].number'); do
  open=$(gh api "repos/$REPO/issues/$n" --jq '.issue_dependencies_summary.blocked_by // 0')
  [ "$open" -eq 0 ] || continue
  if reason=$(go "$n"); then
    echo "dispatch #$n"
    gh workflow run factory-implementer.yml --repo "$REPO" -f issue="$n"
    exit 0
  fi
  if [ "$reason" = daily ]; then
    echo "::notice::the daily budget is spent; the schedule resumes after KST midnight"
    exit 0
  fi
  echo "::notice::#$n does not start ($reason); trying the next ticket"
done
echo "::notice::no unblocked approved ticket may start"
$tickets_only && exit 0

# Queue empty: move on to the next spec. Wait while anything is still running or queued.
busy=$(gh issue list --repo "$REPO" --state open --label in-progress --json number --jq length)
queued=$(gh issue list --repo "$REPO" --state open --label ready-for-agent --json number --jq length)
if [ "$busy" -gt 0 ] || [ "$queued" -gt 0 ]; then
  echo "::notice::$busy in progress, $queued waiting (blockers or budget); not moving to the next spec yet"
  exit 0
fi
for s in $(gh issue list --repo "$REPO" --state open --label spec --json number --jq '.[].number'); do
  read -r total done < <(gh api "repos/$REPO/issues/$s" --jq '"\(.sub_issues_summary.total // 0) \(.sub_issues_summary.completed // 0)"')
  if [ "$total" -eq 0 ]; then
    running=$(gh run list --repo "$REPO" --workflow factory-decompose.yml --limit 20 --json status --jq '[.[] | select(.status != "completed")] | length')
    if [ "$running" -gt 0 ]; then
      echo "::notice::spec #$s is still being decomposed"; exit 0
    fi
    # Its decompose stopped (budget, decision) or failed: start it again if the guard allows.
    if reason=$(go "$s"); then
      echo "decompose #$s again"
      gh workflow run factory-decompose.yml --repo "$REPO" -f issue="$s"
    else
      echo "::notice::spec #$s has no tickets and may not be decomposed now ($reason)"
    fi
    exit 0
  fi
  if [ "$total" -eq "$done" ]; then
    gh issue close "$s" --repo "$REPO" --reason completed --comment "All $total tickets merged."
  fi
done
for s in ${SPEC_QUEUE:-}; do
  read -r state labels < <(gh issue view "$s" --repo "$REPO" --json state,labels --jq '[.state, ([.labels[].name] | join(","))] | @tsv')
  [ "$state" = OPEN ] || continue
  case ",$labels," in *,spec,*) continue;; esac
  if ! reason=$(go "$s"); then
    echo "::notice::next spec #$s may not start ($reason)"
    exit 0
  fi
  echo "next spec: #$s"
  gh issue edit "$s" --repo "$REPO" --add-label spec
  gh workflow run factory-decompose.yml --repo "$REPO" -f issue="$s"
  exit 0
done
echo "::notice::every spec in the queue is labelled; the factory is idle"

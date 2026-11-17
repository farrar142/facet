#!/usr/bin/env bash
# Cost ledger, budgets and the stage guard of the software factory (docs/adr/0001-software-factory.md,
# spec #249 "비용 장부와 상한").
#
# The ledger is the one open issue labelled `factory-ledger`. Every Claude stage adds one comment:
#
#   <!-- ledger -->
#   spec=#S node=<stage> issue=#N run=<run id> cost=<USD> turns=… input=… output=… cache_read=… cache_write=…
#
#   ledger.sh record <node> <issue>   read the action's execution file and comment its cost, turns and
#                                     tokens on the ledger. Never fails the workflow: a missing file,
#                                     ledger issue or jq only warns.
#   ledger.sh spec-total <S>          sum of the ledger comments with spec=#S, in USD
#   ledger.sh day-total [YYYY-MM-DD]  sum of the ledger comments created on that KST date (default: today)
#   ledger.sh spec-of <issue>         the spec of an issue: the first #number in its "## Parent" section,
#                                     or the issue itself when it has none
#   ledger.sh guard <issue>           decide whether a stage may start for <issue>. Writes go=true|false and
#                                     reason=<why> to $GITHUB_OUTPUT (stdout when unset). Stops when the spec
#                                     has `needs-decision` or `factory-paused`, the KST day's total reached the
#                                     daily budget, or the spec's total reached its budget. A budget stop leaves
#                                     its marker comment once (and labels the spec `factory-paused`).
#
# Environment:
#   REPO (or GITHUB_REPOSITORY)   owner/name; gh needs GH_TOKEN
#   FACTORY_SPEC_BUDGET_USD       spec budget when the spec body has no "예산: $N" line (default 60)
#   FACTORY_DAILY_BUDGET_USD      budget of one KST day for the whole factory (default 150)
#   FACTORY_NOW                   "now" as Unix seconds (tests); default the clock
#   RUNNER_TEMP                   where the action writes claude-execution-output.json
#   GITHUB_RUN_ID, GITHUB_OUTPUT  set by Actions
#
# Recorded execution files move to $RUNNER_TEMP/factory-runs/, so a later `record` after a skipped stage
# does not count the same run twice. Needs bash, gh and jq (both GitHub runners have them).
set -u

REPO=${REPO:-${GITHUB_REPOSITORY:-}}
SPEC_BUDGET_DEFAULT=60
DAILY_BUDGET_DEFAULT=150
KST=32400 # seconds east of UTC

# jq on Windows prints CRLF; strip the CR everywhere.
j() { jq "$@" | tr -d '\r'; }

warn() { echo "::warning::ledger: $*"; }

now() { echo "${FACTORY_NOW:-$(date -u +%s)}"; }

# KST date of Unix seconds.
kst_date() { j -rn --argjson t "$1" --argjson o "$KST" '($t + $o) | strftime("%Y-%m-%d")'; }

# Number of the open ledger issue, empty when there is none.
ledger_issue() {
  gh api "repos/$REPO/issues?labels=factory-ledger&state=open&per_page=100" \
    | j -r '[.[] | select(.pull_request | not)] | sort_by(.number) | .[0].number // empty'
}

# All comments of an issue as one JSON object per line: {created_at, body}.
comments() {
  gh api "repos/$REPO/issues/$1/comments?per_page=100" --paginate \
    | j -c '.[] | {created_at, body}'
}

# Ledger comments as "<KST date> <spec> <cost>" lines.
ledger_rows() {
  local n; n=$(ledger_issue)
  [ -n "$n" ] || return 0
  comments "$n" | j -r --argjson o "$KST" 'select(.body | startswith("<!-- ledger -->"))
    | ((.created_at | fromdateiso8601) + $o | strftime("%Y-%m-%d")) as $d
    | (.body | capture("spec=#(?<s>[0-9]+)").s // "0") as $s
    | (.body | capture("cost=(?<c>[0-9.eE+-]+)").c // "0") as $c
    | "\($d) \($s) \($c)"'
}

sum() { awk '{ s += $1 } END { printf "%.4f\n", s + 0 }'; }

spec_total() { ledger_rows | awk -v s="$1" '$2 == s { print $3 }' | sum; }

day_total() {
  local day=${1:-$(kst_date "$(now)")}
  ledger_rows | awk -v d="$day" '$1 == d { print $3 }' | sum
}

issue_json() { gh api "repos/$REPO/issues/$1"; }

# Body of an issue, without CRs.
body_of() { issue_json "$1" | j -r '.body // ""'; }

# First #number in the "## Parent" section of a body on stdin, empty when none.
parent_of_body() {
  awk '/^##[[:space:]]/ { inp = ($0 ~ /^##[[:space:]]+Parent[[:space:]]*$/); next }
       inp && match($0, /#[0-9]+/) { print substr($0, RSTART + 1, RLENGTH - 1); exit }'
}

spec_of() {
  local p; p=$(body_of "$1" | parent_of_body)
  echo "${p:-$1}"
}

# Spec budget: the body's "예산: $N" line, else FACTORY_SPEC_BUDGET_USD, else 60.
spec_budget() {
  local b
  b=$(printf '%s\n' "$1" | sed -n 's/^[[:space:]]*예산:[[:space:]]*\$\{0,1\}\([0-9][0-9]*\(\.[0-9][0-9]*\)\{0,1\}\).*/\1/p' | head -n 1)
  echo "${b:-${FACTORY_SPEC_BUDGET_USD:-$SPEC_BUDGET_DEFAULT}}"
}

reached() { awk -v t="$1" -v b="$2" 'BEGIN { exit !(t + 0 >= b + 0) }'; }

# has_marker <issue> <first line> [<another line>]: a comment starts with the marker line
# (and also has the other line).
has_marker() {
  comments "$1" | j -r --arg m "$2" --arg l "${3:-}" \
    'select((.body | startswith($m))
      and ($l == "" or (.body | split("\n") | map(rtrimstr("\r")) | index($l) != null))) | "yes"' \
    | grep -q yes
}

# The body goes as a raw field, not `-F body=@file`: on the Windows runner gh is a Windows program
# and would not find a Git Bash /tmp path.
post_comment() { gh api "repos/$REPO/issues/$1/comments" -f "body=$2" >/dev/null; }

add_label() { gh api "repos/$REPO/issues/$1/labels" -f "labels[]=$2" >/dev/null; }

output() {
  if [ -n "${GITHUB_OUTPUT:-}" ]; then printf '%s\n' "$@" >> "$GITHUB_OUTPUT"; else printf '%s\n' "$@"; fi
}

record() {
  local node=$1 issue=$2
  local exec="${RUNNER_TEMP:-/tmp}/claude-execution-output.json"
  if [ ! -f "$exec" ]; then warn "no execution file for $node #$issue; nothing recorded"; return 0; fi
  if ! command -v jq >/dev/null 2>&1; then warn "jq is missing; $node #$issue not recorded"; return 0; fi
  local usage
  usage=$(j -r '[(if type == "array" then .[] else . end) | select(.type? == "result")] | last
    | if . == null then "cost=0 turns=0 input=0 output=0 cache_read=0 cache_write=0 note=no-result" else
      "cost=\(.total_cost_usd // 0) turns=\(.num_turns // 0) input=\(.usage.input_tokens // 0) output=\(.usage.output_tokens // 0) cache_read=\(.usage.cache_read_input_tokens // 0) cache_write=\(.usage.cache_creation_input_tokens // 0)" end' "$exec")
  [ -n "$usage" ] || { warn "unreadable execution file for $node #$issue"; usage="cost=0 turns=0 input=0 output=0 cache_read=0 cache_write=0 note=unreadable"; }
  mkdir -p "${RUNNER_TEMP:-/tmp}/factory-runs"
  mv "$exec" "${RUNNER_TEMP:-/tmp}/factory-runs/${node}-${issue}-$(now).json" 2>/dev/null || true

  local spec; spec=$(spec_of "$issue")
  local line="spec=#${spec:-$issue} node=$node issue=#$issue run=${GITHUB_RUN_ID:-local} $usage"
  local n; n=$(ledger_issue)
  if [ -z "$n" ]; then warn "no open factory-ledger issue; not recorded: $line"; return 0; fi
  if post_comment "$n" "<!-- ledger -->
$line"; then
    echo "ledger #$n: $line"
  else
    warn "could not comment on the ledger #$n: $line"
  fi
  return 0
}

guard() {
  local issue=$1 spec json labels body
  spec=$(spec_of "$issue")
  if [ -z "$spec" ] || ! json=$(issue_json "$spec"); then
    echo "::warning::guard: could not read the spec of #$issue; not starting"
    output go=false reason=error
    return 0
  fi
  labels=",$(printf '%s' "$json" | j -r '[.labels[].name] | join(",")'),"
  body=$(printf '%s' "$json" | j -r '.body // ""')
  for l in needs-decision factory-paused; do
    case "$labels" in *",$l,"*)
      echo "::notice::guard: spec #$spec is labelled $l; #$issue does not start"
      output go=false "reason=$l"
      return 0;;
    esac
  done

  local day daily dlimit ledger
  day=$(kst_date "$(now)")
  dlimit=${FACTORY_DAILY_BUDGET_USD:-$DAILY_BUDGET_DEFAULT}
  daily=$(day_total "$day")
  if reached "$daily" "$dlimit"; then
    echo "::warning::guard: the factory spent \$$daily of \$$dlimit on $day (KST); nothing starts until KST midnight"
    ledger=$(ledger_issue)
    if [ -n "$ledger" ] && ! has_marker "$ledger" "<!-- factory-budget-daily $day -->"; then
      post_comment "$ledger" "<!-- factory-budget-daily $day -->
하루 상한에 닿아 공장이 멈췄다: $day(KST) 합계 \$$daily / 상한 \$$dlimit. KST 0시가 지나면 예약 실행이 다시 시작한다." \
        || echo "::warning::guard: could not mark the ledger #$ledger"
    fi
    output go=false reason=daily
    return 0
  fi

  local total budget
  budget=$(spec_budget "$body")
  total=$(spec_total "$spec")
  if reached "$total" "$budget"; then
    echo "::warning::guard: spec #$spec spent \$$total of its \$$budget budget; pausing it"
    add_label "$spec" factory-paused || echo "::warning::guard: could not label #$spec factory-paused"
    if ! has_marker "$spec" "<!-- factory-budget-spec -->" "budget=$budget"; then
      post_comment "$spec" "<!-- factory-budget-spec -->
budget=$budget
이 spec이 예산에 닿아 멈췄다: 합계 \$$total / 예산 \$$budget. 이어 가려면 본문의 \`예산: \$N\` 줄을 올리고 \`factory-paused\` 라벨을 뗀다." \
        || echo "::warning::guard: could not mark #$spec"
    fi
    output go=false reason=spec-budget
    return 0
  fi

  echo "guard: #$issue (spec #$spec) may start: spec \$$total of \$$budget, $day \$$daily of \$$dlimit"
  output go=true reason=
}

usage() {
  echo "usage: ledger.sh record <node> <issue> | spec-total <spec> | day-total [YYYY-MM-DD] | spec-of <issue> | guard <issue>" >&2
  exit 2
}

[ -n "$REPO" ] || { echo "ledger.sh: REPO (or GITHUB_REPOSITORY) is not set" >&2; [ "${1:-}" = record ] && exit 0; exit 2; }

case "${1:-}" in
  record) [ $# -eq 3 ] || usage; record "$2" "$3" ;;
  spec-total) [ $# -eq 2 ] || usage; spec_total "$2" ;;
  day-total) [ $# -le 2 ] || usage; day_total "${2:-}" ;;
  spec-of) [ $# -eq 2 ] || usage; spec_of "$2" ;;
  guard) [ $# -eq 2 ] || usage; guard "$2" ;;
  *) usage ;;
esac
